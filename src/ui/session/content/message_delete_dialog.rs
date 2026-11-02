use adw::prelude::*;
use gettextrs::gettext;
use gtk::gio;
use gtk::glib;

use crate::i18n::gettext_f;
use crate::model;
use crate::strings;
use crate::utils;

/// Which `ChatPermissions` knob(s) a panel switch controls.
#[derive(Clone, Copy)]
enum Permission {
    Messages,
    Media,
    Polls,
    Other,
    Previews,
    Reactions,
    ChangeInfo,
    Invite,
    Pin,
    Topics,
}

/// The tdesktop admin delete dialog: besides deleting the message it can
/// report spam, delete every message of the sender, and restrict or block
/// them.
/// shortcut: tdesktop's per-media-type sub-toggles collapse into one "Send
/// Media" switch and "remove all reactions" is not offered; upgrade when
/// members need the fine-grained matrix.
pub(crate) fn show(parent: &impl IsA<gtk::Widget>, message: &model::Message, ids: Vec<i64>) {
    let chat = message.chat_();
    let session = chat.session_();
    let client_id = session.client_().id();
    let chat_id = chat.id();
    let sender_model = message.sender();
    let Some(user) = sender_model.as_user() else {
        return;
    };
    let sender =
        tdlib::enums::MessageSender::User(tdlib::types::MessageSenderUser { user_id: user.id() });
    let supergroup_id = chat.chat_type().supergroup().map(|group| group.id());
    let can_restrict = match chat.chat_type() {
        model::ChatType::Supergroup(group) => match group.status().0 {
            tdlib::enums::ChatMemberStatus::Creator(_) => true,
            tdlib::enums::ChatMemberStatus::Administrator(admin) => {
                admin.rights.can_restrict_members
            }
            _ => false,
        },
        // Basic-group admins hold every applicable right implicitly.
        model::ChatType::BasicGroup(_) => true,
        _ => false,
    };

    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Delete This Message"))
        .build();
    dialog.add_response("cancel", &gettext("_Cancel"));
    dialog.add_response("delete", &gettext("_Delete"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let area = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();

    // Reporting spam is only exposed for supergroups and channels.
    let report = gtk::CheckButton::builder()
        .label(gettext("Report Spam"))
        .build();
    report.set_visible(supergroup_id.is_some());
    area.append(&report);

    let delete_all = gtk::CheckButton::builder()
        .label(gettext_f(
            "Delete All Messages from {sender}",
            &[("sender", &strings::message_sender(&sender_model, true))],
        ))
        .build();
    area.append(&delete_all);

    let restrict = gtk::CheckButton::builder()
        .label(gettext("Restrict Member"))
        .build();
    restrict.set_visible(can_restrict);
    area.append(&restrict);

    let mut switches: Vec<(gtk::Switch, Permission)> = Vec::new();
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(12)
        .build();
    for (label, permission) in [
        (gettext("Send Messages"), Permission::Messages),
        (gettext("Send Media"), Permission::Media),
        (gettext("Create Polls"), Permission::Polls),
        (gettext("Send Stickers and GIFs"), Permission::Other),
        (gettext("Web Page Previews"), Permission::Previews),
        (gettext("Reactions"), Permission::Reactions),
        (gettext("Change Chat Info"), Permission::ChangeInfo),
        (gettext("Invite Users"), Permission::Invite),
        (gettext("Pin Messages"), Permission::Pin),
    ] {
        add_switch(&panel, &mut switches, &label, permission);
    }
    if chat.is_forum() {
        add_switch(
            &panel,
            &mut switches,
            &gettext("Create Topics"),
            Permission::Topics,
        );
    }

    let block = gtk::CheckButton::builder()
        .label(gettext("Block User"))
        .build();
    panel.append(&block);

    let revealer = gtk::Revealer::builder()
        .child(&panel)
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .build();
    let revealer_for_toggle = revealer.clone();
    restrict.connect_toggled(move |button| {
        revealer_for_toggle.set_reveal_child(button.is_active());
    });
    area.append(&revealer);

    dialog.set_extra_child(Some(&area));

    // Show the member's current restrictions instead of a wall of switches
    // already flipped off by an earlier ban.
    {
        let (restrict, block, switches) = (restrict.clone(), block.clone(), switches.clone());
        let sender = sender.clone();
        utils::spawn(async move {
            let Ok(tdlib::enums::ChatMember::ChatMember(member)) =
                tdlib::functions::get_chat_member(chat_id, sender, client_id).await
            else {
                return;
            };
            match member.status {
                tdlib::enums::ChatMemberStatus::Restricted(data) => {
                    restrict.set_active(true);
                    for (switch, permission) in &switches {
                        switch.set_active(allows(&data.permissions, *permission));
                    }
                }
                tdlib::enums::ChatMemberStatus::Banned(_) => {
                    restrict.set_active(true);
                    block.set_active(true);
                }
                _ => {}
            }
        });
    }

    dialog.choose(Some(parent), gio::Cancellable::NONE, move |response| {
        if response != "delete" {
            return;
        }
        let report_spam = report.is_active();
        let all = delete_all.is_active();
        let banned = block.is_active();
        let restricted = !banned && restrict.is_active();
        let permissions = compose(&switches);
        let ids = ids.clone();
        let sender = sender.clone();
        utils::spawn(async move {
            let mut delete_ids = ids.clone();
            if all {
                if let Ok(tdlib::enums::FoundChatMessages::FoundChatMessages(found)) =
                    tdlib::functions::search_chat_messages(
                        chat_id,
                        None,
                        String::new(),
                        Some(sender.clone()),
                        0,
                        0,
                        100,
                        None,
                        client_id,
                    )
                    .await
                {
                    delete_ids.extend(found.messages.into_iter().map(|m| m.id));
                    delete_ids.sort_unstable();
                    delete_ids.dedup();
                }
            }
            if report_spam {
                if let Some(supergroup_id) = supergroup_id {
                    if let Err(e) =
                        tdlib::functions::report_supergroup_spam(supergroup_id, ids, client_id)
                            .await
                    {
                        log::warn!("Failed to report messages as spam: {e:?}");
                    }
                }
            }
            if let Err(e) =
                tdlib::functions::delete_messages(chat_id, delete_ids, true, client_id).await
            {
                log::warn!("Error deleting messages: {e:?}");
            }
            if banned {
                if let Err(e) = tdlib::functions::set_chat_member_status(
                    chat_id,
                    sender,
                    tdlib::enums::ChatMemberStatus::Banned(tdlib::types::ChatMemberStatusBanned {
                        banned_until_date: 0,
                    }),
                    client_id,
                )
                .await
                {
                    log::warn!("Failed to block a member: {e:?}");
                }
            } else if restricted {
                if let Err(e) = tdlib::functions::set_chat_member_status(
                    chat_id,
                    sender,
                    tdlib::enums::ChatMemberStatus::Restricted(
                        tdlib::types::ChatMemberStatusRestricted {
                            is_member: true,
                            restricted_until_date: 0,
                            permissions,
                        },
                    ),
                    client_id,
                )
                .await
                {
                    log::warn!("Failed to restrict a member: {e:?}");
                }
            }
        });
    });
}

fn add_switch(
    panel: &gtk::Box,
    switches: &mut Vec<(gtk::Switch, Permission)>,
    label: &str,
    permission: Permission,
) {
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    let row = gtk::Box::builder().spacing(8).build();
    row.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    row.append(&switch);
    panel.append(&row);
    switches.push((switch, permission));
}

fn allows(p: &tdlib::types::ChatPermissions, permission: Permission) -> bool {
    match permission {
        Permission::Messages => p.can_send_basic_messages,
        Permission::Media => {
            p.can_send_audios
                && p.can_send_documents
                && p.can_send_photos
                && p.can_send_videos
                && p.can_send_video_notes
                && p.can_send_voice_notes
        }
        Permission::Polls => p.can_send_polls,
        Permission::Other => p.can_send_other_messages,
        Permission::Previews => p.can_add_link_previews,
        Permission::Reactions => p.can_react_to_messages,
        Permission::ChangeInfo => p.can_change_info,
        Permission::Invite => p.can_invite_users,
        Permission::Pin => p.can_pin_messages,
        Permission::Topics => p.can_create_topics,
    }
}

/// Unrestricted baseline with the switches applied on top.
fn compose(switches: &[(gtk::Switch, Permission)]) -> tdlib::types::ChatPermissions {
    let mut p = tdlib::types::ChatPermissions {
        can_send_basic_messages: true,
        can_send_audios: true,
        can_send_documents: true,
        can_send_photos: true,
        can_send_videos: true,
        can_send_video_notes: true,
        can_send_voice_notes: true,
        can_send_polls: true,
        can_send_other_messages: true,
        can_add_link_previews: true,
        can_react_to_messages: true,
        can_edit_tag: true,
        can_change_info: true,
        can_invite_users: true,
        can_pin_messages: true,
        can_create_topics: true,
    };
    for (switch, permission) in switches {
        let on = switch.is_active();
        match permission {
            Permission::Messages => p.can_send_basic_messages = on,
            Permission::Media => {
                p.can_send_audios = on;
                p.can_send_documents = on;
                p.can_send_photos = on;
                p.can_send_videos = on;
                p.can_send_video_notes = on;
                p.can_send_voice_notes = on;
            }
            Permission::Polls => p.can_send_polls = on,
            Permission::Other => p.can_send_other_messages = on,
            Permission::Previews => p.can_add_link_previews = on,
            Permission::Reactions => p.can_react_to_messages = on,
            Permission::ChangeInfo => p.can_change_info = on,
            Permission::Invite => p.can_invite_users = on,
            Permission::Pin => p.can_pin_messages = on,
            Permission::Topics => p.can_create_topics = on,
        }
    }
    p
}
