use gettextrs::gettext;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::strings;

impl super::Row {
    pub(super) fn update_message_status_icon(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let icon = &imp.message_status_icon;

            if chat.is_own_chat() {
                icon.set_visible(false);
            } else if let Some(message) = chat.last_message().filter(model::Message::is_outgoing) {
                let (icon_name, css_class) = match message.sending_state() {
                    Some(state) => match state.0 {
                        tdlib::enums::MessageSendingState::Failed(_) => {
                            ("message-failed-symbolic", "error")
                        }
                        tdlib::enums::MessageSendingState::Pending(_) => {
                            ("message-pending-symbolic", "dim-label")
                        }
                    },
                    None => (
                        if message.id() == chat.last_read_outbox_message_id() {
                            "message-read-symbolic"
                        } else {
                            "message-unread-right-symbolic"
                        },
                        "accent",
                    ),
                };

                icon.set_icon_name(Some(icon_name));
                icon.set_css_classes(&[css_class]);
                icon.set_visible(true);
            } else {
                icon.set_visible(false);
            }
        }
    }

    pub(super) fn update_timestamp(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let label = &imp.timestamp_label;

            let date = chat
                .draft_message()
                .map(|m| m.0.date)
                .or_else(|| chat.last_message().map(|m| m.date()));

            if let Some(date) = date {
                label.set_label(&timestamp_text(date as i64));
                label.set_visible(true);
            } else {
                label.set_visible(false);
            }
        }
    }

    pub(super) fn update_subtitle_prefix_label(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let label = &imp.subtitle_prefix_label;

            if chat.actions().last().is_some() {
                label.set_visible(false);
            } else if chat.draft_message().is_some() {
                label.set_label(&gettext("Draft:"));
                label.add_css_class("error");
                label.set_visible(true);
            } else if let Some(sender) = chat.last_message().and_then(sender_label) {
                label.set_label(&format!("{sender}:"));
                label.remove_css_class("error");
                label.set_visible(true);
            } else {
                label.set_visible(false);
            }
        }
    }

    pub(super) fn update_minithumbnail(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let minithumbnail = &imp.minithumbnail;

            if chat.actions().n_items() > 0 || chat.draft_message().is_some() {
                minithumbnail.set_visible(false);
            } else if let Some(texture) = chat.last_message().and_then(message_thumbnail_texture) {
                minithumbnail.set_paintable(Some(texture.upcast()));
                minithumbnail.set_visible(true);
            } else {
                minithumbnail.set_visible(false);
            }
        }
    }

    pub(super) fn update_subtitle_label(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let label = &imp.subtitle_label;

            if let Some(actions) = chat.actions().last().map(|a| strings::chat_action(&a)) {
                label.set_text(Some(&actions));
                label.remove_css_class("dim-label");
                label.add_css_class("accent");
            } else if let Some(draft_message) =
                chat.draft_message().map(|d| draft_message_text(d.0))
            {
                label.set_text(Some(&draft_message));
                label.add_css_class("dim-label");
                label.remove_css_class("accent");
            } else if let Some(last_message) =
                chat.last_message().map(|m| strings::message_content(&m))
            {
                label.set_text(Some(&last_message));
                label.add_css_class("dim-label");
                label.remove_css_class("accent");
            } else {
                label.set_text(None);
            }
        }
    }

    pub(super) fn update_status_stack(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let stack = &imp.status_stack;

            if chat.unread_mention_count() > 0 {
                stack.set_visible_child(&*imp.unread_mention_label);
            } else if chat.unread_count() > 0 || chat.is_marked_as_unread() {
                stack.set_visible_child(&*imp.unread_count_label);
            } else if item.is_pinned() {
                stack.set_visible_child(&*imp.pin_icon);
            } else {
                stack.set_visible_child_name("empty");
            }
        }
    }

    pub(super) fn update_unread_count_style(&self) {
        if let Some(item) = self.item() {
            let imp = self.imp();
            let chat = item.chat_();
            let label = &imp.unread_count_label;

            let notification_settings = chat.notification_settings();
            let scope_notification_settings = match chat.chat_type() {
                model::ChatType::Private(_) | model::ChatType::Secret(_) => {
                    chat.session_().private_chats_notification_settings()
                }
                model::ChatType::Supergroup(supergroup) if supergroup.is_channel() => {
                    chat.session_().channel_chats_notification_settings()
                }
                _ => chat.session_().group_chats_notification_settings(),
            };

            let css_class = if notification_settings.0.use_default_mute_for {
                if scope_notification_settings.0.mute_for > 0
                    || notification_settings.0.mute_for > 0
                {
                    "unread-count-muted"
                } else {
                    "unread-count-unmuted"
                }
            } else if notification_settings.0.mute_for > 0 {
                "unread-count-muted"
            } else {
                "unread-count-unmuted"
            };

            label.set_css_classes(&["unread-count", css_class]);
        }
    }

    pub(super) fn update_actions(&self) {
        if let Some(item) = self.item() {
            let chat = item.chat_();

            if chat.is_own_chat() {
                self.update_archive_actions(false, false);
            } else {
                match item.chat_list_type().0 {
                    tdlib::enums::ChatList::Main => self.update_archive_actions(true, false),
                    tdlib::enums::ChatList::Archive => self.update_archive_actions(false, true),
                    _ => self.update_archive_actions(false, false),
                }
            }

            self.update_pin_actions(!item.is_pinned(), item.is_pinned());

            if chat.unread_count() > 0 {
                self.update_mark_as_unread_actions(false, true);
            } else {
                self.update_mark_as_unread_actions(
                    !chat.is_marked_as_unread(),
                    chat.is_marked_as_unread(),
                );
            }
        } else {
            self.update_archive_actions(false, false);
            self.update_pin_actions(false, false);
            self.update_mark_as_unread_actions(false, false);
        }
    }

    fn update_archive_actions(&self, archive: bool, unarchive: bool) {
        self.action_set_enabled("sidebar-row.archive", archive);
        self.action_set_enabled("sidebar-row.unarchive", unarchive);
    }

    fn update_pin_actions(&self, pin: bool, unpin: bool) {
        self.action_set_enabled("sidebar-row.pin", pin);
        self.action_set_enabled("sidebar-row.unpin", unpin);
    }

    fn update_mark_as_unread_actions(&self, unread: bool, read: bool) {
        self.action_set_enabled("sidebar-row.mark-as-unread", unread);
        self.action_set_enabled("sidebar-row.mark-as-read", read);
    }
}

fn sender_label(message: model::Message) -> Option<String> {
    use tdlib::enums::MessageContent::*;

    // Just use a sender label for specific messages.
    match message.content().0 {
        MessageText(_) | MessageLocation(_) | MessageSticker(_) | MessagePhoto(_)
        | MessageAudio(_) | MessageAnimation(_) | MessageVideo(_) | MessageDocument(_)
        | MessageVoiceNote(_) | MessageCall(_) | MessageVenue(_) => {}
        _ => return None,
    }

    if message.chat_().is_own_chat() {
        if message.is_outgoing() {
            None
        } else {
            message
                .forward_info()
                .map(|forward_info| forward_info.origin())
                .map(|forward_origin| match forward_origin {
                    model::MessageForwardOrigin::User(user) => {
                        strings::user_display_name(&user, false)
                    }
                    model::MessageForwardOrigin::Chat { chat, .. }
                    | model::MessageForwardOrigin::Channel { chat, .. } => chat.title(),
                    model::MessageForwardOrigin::HiddenUser { sender_name }
                    | model::MessageForwardOrigin::MessageImport { sender_name } => {
                        sender_name.clone()
                    }
                })
        }
    } else {
        let show_sender = match message.chat_().chat_type() {
            model::ChatType::BasicGroup(_) => true,
            model::ChatType::Supergroup(supergroup) => !supergroup.is_channel(),
            model::ChatType::Private(_) | model::ChatType::Secret(_) => message.is_outgoing(),
        };
        if show_sender {
            Some(if message.is_outgoing() {
                gettext("You")
            } else {
                strings::message_sender(&message.sender(), false)
            })
        } else {
            None
        }
    }
}

fn message_thumbnail_texture(message: model::Message) -> Option<gdk::Texture> {
    use tdlib::enums::MessageContent::*;

    match message.content().0 {
        MessageAnimation(data) => data.animation.minithumbnail,
        MessageAudio(data) => data.audio.album_cover_minithumbnail,
        MessageChatChangePhoto(data) => data.photo.minithumbnail,
        MessageDocument(data) => data.document.minithumbnail,
        MessagePhoto(data) => data.photo.minithumbnail,
        MessageVideo(data) => data.video.minithumbnail,
        _ => None,
    }
    .map(|thumbnail| {
        gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
            &thumbnail.data,
        )))
        .unwrap()
    })
}

fn draft_message_text(message: tdlib::types::DraftMessage) -> String {
    match message.input_message_text {
        tdlib::enums::InputMessageContent::InputMessageText(data) => data.text.text,
        other => {
            log::warn!("Unexpected draft message type: {other:?}");
            String::new()
        }
    }
}

fn timestamp_text(date: i64) -> glib::GString {
    let datetime_now = glib::DateTime::now_local().unwrap();
    let datetime = glib::DateTime::from_unix_utc(date)
        .and_then(|t| t.to_local())
        .unwrap();

    let difference = datetime_now.difference(&datetime);
    let hours_difference = difference.as_hours();
    let days_difference = difference.as_days();

    if hours_difference <= 16 {
        datetime.format(&gettext("%l:%M %p"))
    } else if days_difference < 6 {
        // Show the day of the week
        datetime.format("%a")
    } else if days_difference < 364 {
        // Show the day and the month
        datetime.format("%d %b")
    } else {
        // Show the entire date
        datetime.format("%x")
    }
    .unwrap()
}
