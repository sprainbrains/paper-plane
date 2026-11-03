use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::gdk;
use gtk::gio;

use crate::i18n::gettext_f;
use crate::model;
use crate::strings;
use crate::utils;

/// One `ChatPermissions` knob addressed by the panel.
#[derive(Clone, Copy)]
enum Field {
    Basic,
    Audios,
    Documents,
    Photos,
    Videos,
    VideoNotes,
    VoiceNotes,
    Polls,
    Other,
    Previews,
    Reactions,
    EditTag,
    ChangeInfo,
    Invite,
    Pin,
    Topics,
}

fn get(p: &tdlib::types::ChatPermissions, field: Field) -> bool {
    match field {
        Field::Basic => p.can_send_basic_messages,
        Field::Audios => p.can_send_audios,
        Field::Documents => p.can_send_documents,
        Field::Photos => p.can_send_photos,
        Field::Videos => p.can_send_videos,
        Field::VideoNotes => p.can_send_video_notes,
        Field::VoiceNotes => p.can_send_voice_notes,
        Field::Polls => p.can_send_polls,
        Field::Other => p.can_send_other_messages,
        Field::Previews => p.can_add_link_previews,
        Field::Reactions => p.can_react_to_messages,
        Field::EditTag => p.can_edit_tag,
        Field::ChangeInfo => p.can_change_info,
        Field::Invite => p.can_invite_users,
        Field::Pin => p.can_pin_messages,
        Field::Topics => p.can_create_topics,
    }
}

fn set(p: &mut tdlib::types::ChatPermissions, field: Field, on: bool) {
    match field {
        Field::Basic => p.can_send_basic_messages = on,
        Field::Audios => p.can_send_audios = on,
        Field::Documents => p.can_send_documents = on,
        Field::Photos => p.can_send_photos = on,
        Field::Videos => p.can_send_videos = on,
        Field::VideoNotes => p.can_send_video_notes = on,
        Field::VoiceNotes => p.can_send_voice_notes = on,
        Field::Polls => p.can_send_polls = on,
        Field::Other => p.can_send_other_messages = on,
        Field::Previews => p.can_add_link_previews = on,
        Field::Reactions => p.can_react_to_messages = on,
        Field::EditTag => p.can_edit_tag = on,
        Field::ChangeInfo => p.can_change_info = on,
        Field::Invite => p.can_invite_users = on,
        Field::Pin => p.can_pin_messages = on,
        Field::Topics => p.can_create_topics = on,
    }
}

/// The media group tdesktop nests under "Send Media".
const MEDIA: [(Field, &str); 10] = [
    (Field::Photos, "Photos"),
    (Field::Videos, "Videos"),
    (Field::VideoNotes, "Video Messages"),
    (Field::Audios, "Music"),
    (Field::VoiceNotes, "Voice Messages"),
    (Field::Documents, "Files"),
    (Field::Other, "Stickers and GIFs"),
    (Field::Previews, "Link Previews"),
    (Field::Polls, "Polls"),
    (Field::Reactions, "Reactions"),
];

/// The tdesktop admin delete dialog: report spam, delete everything of the
/// sender, and the full restrictions panel (per-media-type checkboxes, own
/// tag, ban) — applied together with the delete.
/// shortcut: "remove all reactions of the sender" has no TDLib moderation
/// API and is not offered; upgrade if TDLib ever exposes it.
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

    // Show how many messages of this sender TDLib can find.
    {
        let (delete_all, sender) = (delete_all.clone(), sender.clone());
        let name = strings::message_sender(&sender_model, true);
        utils::spawn(async move {
            if let Ok(tdlib::enums::FoundChatMessages::FoundChatMessages(found)) =
                tdlib::functions::search_chat_messages(
                    chat_id,
                    None,
                    String::new(),
                    Some(sender),
                    0,
                    0,
                    1,
                    None,
                    client_id,
                )
                .await
            {
                let label = gettext_f(
                    "Delete All Messages from {sender} ({count})",
                    &[("sender", &name), ("count", &found.total_count.to_string())],
                );
                delete_all.set_label(Some(&label));
            }
        });
    }

    let restrict = gtk::CheckButton::builder()
        .label(gettext("Restrict Permissions"))
        .build();
    restrict.set_visible(can_restrict);
    area.append(&restrict);

    // The restrictions panel: switches for the top-level rights, media as an
    // expandable group of checkboxes (tdesktop's "10/10" row).
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_start(12)
        .build();
    panel.append(
        &gtk::Label::builder()
            .label(gettext("Restrictions for User"))
            .xalign(0.0)
            .build(),
    );

    let mut switches: Vec<(gtk::Switch, Field)> = Vec::new();
    add_switch(
        &panel,
        &mut switches,
        &gettext("Send Messages"),
        Field::Basic,
    );

    // Media master row: label + "n/10" counter + expander + switch.
    let media_row = gtk::Box::builder().spacing(8).build();
    let media_subs: Vec<(gtk::CheckButton, Field)> = Vec::new();
    let media_subs = Rc::new(RefCell::new(media_subs));
    let media_count = gtk::Label::builder().label("10/10").build();
    media_count.add_css_class("dim-label");
    let media_title = gtk::Label::builder()
        .label(gettext("Send Media"))
        .xalign(0.0)
        .hexpand(true)
        .build();
    let media_switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    media_row.append(&media_title);
    media_row.append(&media_count);
    media_row.append(&media_switch);
    panel.append(&media_row);

    let media_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .margin_start(12)
        .build();
    for (field, label) in MEDIA {
        let sub = gtk::CheckButton::builder()
            .label(gettext(label))
            .active(true)
            .build();
        media_box.append(&sub);
        media_subs.borrow_mut().push((sub, field));
    }
    let media_revealer = gtk::Revealer::builder()
        .child(&media_box)
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .build();
    panel.append(&media_revealer);
    media_title.set_cursor(gdk::Cursor::from_name("pointer", None).as_ref());
    let revealer_for_click = media_revealer.clone();
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| {
        revealer_for_click.set_reveal_child(!revealer_for_click.reveals_child());
    });
    media_title.add_controller(click);

    // Sub checkbox <-> master sync.
    let state = Rc::new(RefCell::new((false, false))); // guards
    for (sub, _) in media_subs.borrow().iter() {
        let (media_switch, media_count, media_subs, state) = (
            media_switch.clone(),
            media_count.clone(),
            media_subs.clone(),
            state.clone(),
        );
        sub.connect_toggled(move |_| {
            if state.borrow().0 {
                return;
            }
            state.borrow_mut().0 = true;
            let (on, total) = {
                let subs = media_subs.borrow();
                (
                    subs.iter().filter(|(c, _)| c.is_active()).count(),
                    subs.len(),
                )
            };
            media_count.set_label(&format!("{on}/{total}"));
            media_switch.set_active(on == total);
            state.borrow_mut().0 = false;
        });
    }
    {
        let (media_subs, media_count, state) =
            (media_subs.clone(), media_count.clone(), state.clone());
        media_switch.connect_active_notify(move |switch| {
            if state.borrow().1 {
                return;
            }
            state.borrow_mut().1 = true;
            let on = switch.is_active();
            let subs = media_subs.borrow();
            for (sub, _) in subs.iter() {
                sub.set_active(on);
            }
            media_count.set_label(&format!(
                "{}/{}",
                if on { subs.len() } else { 0 },
                subs.len()
            ));
            state.borrow_mut().1 = false;
        });
    }

    for (label, field) in [
        ("Add Members", Field::Invite),
        ("Create Topics", Field::Topics),
        ("Pin Messages", Field::Pin),
        ("Change Own Tag", Field::EditTag),
        ("Change Group Profile", Field::ChangeInfo),
    ] {
        if matches!(field, Field::Topics) && !chat.is_forum() {
            continue;
        }
        add_switch(&panel, &mut switches, &gettext(label), field);
    }

    let block = gtk::CheckButton::builder()
        .label(gettext("Block User"))
        .active(false)
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

    // Reflect the member's current restrictions in the controls.
    {
        let (restrict, block, switches) = (restrict.clone(), block.clone(), switches.clone());
        let (media_switch, media_count, media_subs, state) = (
            media_switch.clone(),
            media_count.clone(),
            media_subs.clone(),
            state.clone(),
        );
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
                    state.borrow_mut().0 = true;
                    state.borrow_mut().1 = true;
                    for (switch, field) in &switches {
                        switch.set_active(get(&data.permissions, *field));
                    }
                    let mut on = 0usize;
                    for (sub, field) in media_subs.borrow().iter() {
                        let allowed = get(&data.permissions, *field);
                        sub.set_active(allowed);
                        if allowed {
                            on += 1;
                        }
                    }
                    media_count.set_label(&format!("{on}/{}", media_subs.borrow().len()));
                    media_switch.set_active(on == media_subs.borrow().len());
                    state.borrow_mut().0 = false;
                    state.borrow_mut().1 = false;
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
        let restricted = restrict.is_active();
        let banned = block.is_active();
        let mut permissions = tdlib::types::ChatPermissions {
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
        for (switch, field) in &switches {
            set(&mut permissions, *field, switch.is_active());
        }
        for (sub, field) in media_subs.borrow().iter() {
            set(&mut permissions, *field, sub.is_active());
        }
        let ids = ids.clone();
        let sender = sender.clone();
        utils::spawn(async move {
            let mut delete_ids = ids.clone();
            if all
                && let Ok(tdlib::enums::FoundChatMessages::FoundChatMessages(found)) =
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
            if report_spam
                && let Some(supergroup_id) = supergroup_id
                && let Err(e) =
                    tdlib::functions::report_supergroup_spam(supergroup_id, ids, client_id).await
            {
                log::warn!("Failed to report messages as spam: {e:?}");
            }
            if let Err(e) =
                tdlib::functions::delete_messages(chat_id, delete_ids, true, client_id).await
            {
                log::warn!("Error deleting messages: {e:?}");
            }
            if restricted {
                if banned {
                    if let Err(e) = tdlib::functions::set_chat_member_status(
                        chat_id,
                        sender,
                        tdlib::enums::ChatMemberStatus::Banned(
                            tdlib::types::ChatMemberStatusBanned {
                                banned_until_date: 0,
                            },
                        ),
                        client_id,
                    )
                    .await
                    {
                        log::warn!("Failed to block a member: {e:?}");
                    }
                } else {
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
            }
        });
    });
}

fn add_switch(
    panel: &gtk::Box,
    switches: &mut Vec<(gtk::Switch, Field)>,
    label: &str,
    field: Field,
) -> gtk::Switch {
    let switch = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .active(true)
        .build();
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
    switches.push((switch.clone(), field));
    switch
}
