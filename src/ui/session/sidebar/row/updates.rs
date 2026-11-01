use gettextrs::gettext;
use glib::clone;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

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

            let typing_indicator = &imp.typing_indicator;
            if let Some(actions) = chat.actions().last().map(|a| strings::chat_action(&a)) {
                label.set_text(Some(&actions));
                label.remove_css_class("dim-label");
                label.add_css_class("accent");
                // Drop the label's expansion so the animated dots hug the typing text.
                label.set_hexpand(false);
                typing_indicator.set_visible(true);
            } else {
                label.set_hexpand(true);
                typing_indicator.set_visible(false);

                if let Some(draft_message) = chat.draft_message().map(|d| draft_message_text(d.0)) {
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
            let chat = item.chat_();
            let css_class = if chat.is_muted() {
                "unread-count-muted"
            } else {
                "unread-count-unmuted"
            };
            self.imp()
                .unread_count_label
                .set_css_classes(&["unread-count", css_class]);
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
        MessageText(_)
        | MessageLocation(_)
        | MessageLiveLocation(_)
        | MessageSticker(_)
        | MessagePhoto(_)
        | MessageAudio(_)
        | MessageAnimation(_)
        | MessageVideo(_)
        | MessageDocument(_)
        | MessageVoiceNote(_)
        | MessageCall(_)
        | MessageVenue(_) => {}
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
    .and_then(|thumbnail| {
        gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
            &thumbnail.data,
        )))
        .ok()
    })
}

fn draft_message_text(message: tdlib::types::DraftMessage) -> String {
    match message.content {
        tdlib::enums::DraftMessageContent::Text(data) => data.text.text,
        other => {
            log::warn!("Unexpected draft message content: {other:?}");
            String::new()
        }
    }
}

pub(crate) fn timestamp_text(date: i64) -> glib::GString {
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

/// Chips for a collapsed forum row: the topic of the chat's last message
/// first, then the next most recent topic. `recent` is (topic id, last
/// message date) pairs; unknown topics are ignored.
fn forum_chip_topics(last_topic: Option<i32>, recent: &[(i32, i32)]) -> Vec<i32> {
    let mut chips = Vec::with_capacity(2);
    if let Some(id) = last_topic.filter(|id| recent.iter().any(|(topic, _)| topic == id)) {
        chips.push(id);
    }
    for (id, _) in recent {
        if chips.len() == 2 {
            break;
        }
        if !chips.contains(id) {
            chips.push(*id);
        }
    }
    chips
}

impl super::Row {
    /// Renders the collapsed forum row (tdesktop image 1): up to two topic
    /// chips above the `Author: message` line, the pair wrapped in one
    /// outline. Chips show the topic's dot color + name.
    /// shortcut: chips use a colored dot, not the topic emoji image; upgrade
    /// when the sidebar shares the custom-emoji decode with topic rows.
    pub(super) fn update_forum_chips(&self) {
        let imp = self.imp();
        let Some(chat) = self.item().map(|item| item.chat_()) else {
            return;
        };

        let list = chat.try_forum_topic_list();
        imp.forum_list_signal_group
            .get()
            .unwrap()
            .set_target(list.as_ref());

        let is_forum = chat.is_forum();
        if is_forum {
            imp.sidebar_avatar.add_css_class("forum-squircle");
        } else {
            imp.sidebar_avatar.remove_css_class("forum-squircle");
        }

        let mut chips: Vec<(i32, String, i32, i64)> = Vec::new();
        if let (true, Some(list)) = (is_forum, list.as_ref()) {
            let mut topics: Vec<(i32, i32, String, i32, i64)> = (0..list.n_items())
                .filter_map(|i| list.item(i).and_downcast::<model::Topic>())
                .map(|topic| {
                    let date = topic.last_message().map(|m| m.date()).unwrap_or(0);
                    (
                        topic.forum_topic_id(),
                        date,
                        topic.name(),
                        topic.icon_color(),
                        topic.icon_custom_emoji_id(),
                    )
                })
                .collect();
            topics.sort_by_key(|(_, date, _, _, _)| std::cmp::Reverse(*date));
            let recency: Vec<(i32, i32)> = topics
                .iter()
                .map(|(id, date, _, _, _)| (*id, *date))
                .collect();
            chips = forum_chip_topics(
                chat.last_message().and_then(|m| m.topic_forum_id()),
                &recency,
            )
            .iter()
            .filter_map(|id| {
                topics
                    .iter()
                    .find(|(topic, _, _, _, _)| topic == id)
                    .map(|(topic, _, name, color, emoji)| (*topic, name.clone(), *color, *emoji))
            })
            .collect();
        }

        let slots = [
            (
                imp.forum_chip_1.get(),
                imp.forum_chip_1_icon.get(),
                imp.forum_chip_1_emoji.get(),
                imp.forum_chip_1_name.get(),
            ),
            (
                imp.forum_chip_2.get(),
                imp.forum_chip_2_icon.get(),
                imp.forum_chip_2_emoji.get(),
                imp.forum_chip_2_name.get(),
            ),
        ];
        for (index, (slot, icon, emoji_pic, name)) in slots.into_iter().enumerate() {
            match chips.get(index) {
                Some((id, topic_name, color, emoji)) => {
                    icon.set_markup(&format!("<span foreground='#{color:06X}'>●</span>"));
                    name.set_text(topic_name);
                    // tdesktop shows the topic emoji when it has one, the
                    // colored dot otherwise; the texture arrives async.
                    let loaded = imp.forum_emoji_loaded_for[index].get() == *emoji && *emoji != 0;
                    icon.set_visible(*emoji == 0 || !loaded);
                    emoji_pic.set_visible(loaded);
                    slot.set_visible(true);
                    imp.forum_chip_topics[index].set(*id);
                }
                None => {
                    slot.set_visible(false);
                    imp.forum_chip_topics[index].set(0);
                    imp.forum_emoji_loaded_for[index].set(0);
                    emoji_pic.set_visible(false);
                }
            }
        }
        imp.forum_chips_box.set_visible(!chips.is_empty());

        // Off-screen rows do zero work (AGENTS.md): decode only while mapped.
        if self.is_mapped() {
            for (index, (_, _, _, emoji)) in chips.iter().enumerate() {
                if *emoji != 0 {
                    self.load_chip_emoji(index, *emoji, &chat);
                }
            }
        }
    }

    /// Decodes one chip's topic emoji while the row is demanded; mirrors
    /// `TopicRow::load_emoji` at chip size.
    fn load_chip_emoji(&self, index: usize, emoji_id: i64, chat: &model::Chat) {
        let imp = self.imp();
        let demand = imp.forum_demand.clone();
        demand.set(self.is_mapped());
        if !demand.get() || imp.forum_emoji_loaded_for[index].get() == emoji_id {
            return;
        }
        imp.forum_emoji_loaded_for[index].set(emoji_id);
        let session = chat.session_();
        utils::spawn(clone!(
            #[weak(rename_to = row)]
            self,
            async move {
                let client_id = session.client_().id();
                let Ok(tdlib::enums::Stickers::Stickers(data)) =
                    tdlib::functions::get_custom_emoji_stickers(vec![emoji_id], client_id).await
                else {
                    return;
                };
                let Some(sticker) = data.stickers.into_iter().next() else {
                    return;
                };
                let Ok(texture) =
                    ui::load_picker_preview(&sticker, 16, &session, demand.clone()).await
                else {
                    return;
                };
                // The row may have been recycled to another chat meanwhile.
                let imp = row.imp();
                if !demand.get()
                    || imp.forum_emoji_loaded_for[index].get() != emoji_id
                    || imp.forum_chip_topics[index].get() == 0
                {
                    return;
                }
                let (icon, emoji_pic) = if index == 0 {
                    (
                        imp.forum_chip_1_icon.get().clone(),
                        imp.forum_chip_1_emoji.get().clone(),
                    )
                } else {
                    (
                        imp.forum_chip_2_icon.get().clone(),
                        imp.forum_chip_2_emoji.get().clone(),
                    )
                };
                emoji_pic.set_paintable(Some(&texture));
                emoji_pic.set_visible(true);
                icon.set_visible(false);
            }
        ));
    }

    /// Opens the topic shown on chip `index` directly in the content pane.
    pub(super) fn open_chip_topic(&self, index: usize) {
        let topic_id = self.imp().forum_chip_topics[index].get();
        let Some(chat) = self
            .item()
            .map(|item| item.chat_())
            .filter(|_| topic_id != 0)
        else {
            return;
        };
        if let Some(session) = crate::utils::ancestor::<_, crate::ui::Session>(self) {
            session.open_topic(&chat, topic_id);
            session.show_chat_content();
        }
    }

    /// On map: kick the lazy topic-list fetch a collapsed forum row needs for
    /// its chips (the list dedupes concurrent refreshes).
    pub(super) fn on_map_forum(&self) {
        let Some(chat) = self.item().map(|item| item.chat_()) else {
            return;
        };
        if chat.is_forum() {
            self.imp().forum_demand.set(true);
            chat.forum_topic_list();
            self.update_forum_chips();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::forum_chip_topics;

    #[test]
    fn chips_lead_with_last_message_topic_then_recency() {
        let recent = [(5, 100), (2, 90), (1, 80)];
        // Leftmost chip = topic of the last message, then the freshest others.
        assert_eq!(forum_chip_topics(Some(2), &recent), [2, 5]);
        // No last-message topic known: two most recent.
        assert_eq!(forum_chip_topics(None, &recent), [5, 2]);
        // Unknown last-message topic is ignored, not shown.
        assert_eq!(forum_chip_topics(Some(99), &recent), [5, 2]);
        // A one-topic forum shows a single chip.
        assert_eq!(forum_chip_topics(Some(7), &[(7, 10)]), [7]);
    }
}
