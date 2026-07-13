use gettextrs::gettext;
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::ChatActionBar;
use crate::model;
use crate::strings;
use crate::types::MessageId;
use crate::utils;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum ChatActionBarState {
    #[default]
    Composing,
    Replying(i64),
    Editing(i64),
}

impl ChatActionBarState {
    pub(super) fn replying(&self) -> i64 {
        match self {
            Self::Replying(message_id) => *message_id,
            _ => 0,
        }
    }
}

impl ChatActionBar {
    pub(super) fn cancel_action(&self) {
        use ChatActionBarState::*;

        if let Editing(_) = self.imp().state.get() {
            if let Some(draft_message) = self.chat().and_then(|c| c.draft_message()) {
                self.load_draft_message(draft_message);
            } else {
                self.reset();
            }
        } else {
            self.set_state(Composing);
        }
    }

    pub(super) fn set_state(&self, state: ChatActionBarState) {
        let imp = self.imp();
        if imp.state.get() == state {
            return;
        }

        let generation = imp.state_change_generation.get().wrapping_add(1);
        imp.state_change_generation.set(generation);

        if let ChatActionBarState::Editing(_) = imp.state.get() {
            self.imp().message_entry.set_formatted_text(None);
        }

        if let ChatActionBarState::Editing(_) = state {
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    if obj.imp().state_change_generation.get() != generation {
                        return;
                    }
                    obj.save_message_as_draft().await;
                    if obj.imp().state_change_generation.get() == generation {
                        obj.apply_state(state, generation);
                    }
                }
            ));
            return;
        }

        self.apply_state(state, generation);
    }

    fn apply_state(&self, state: ChatActionBarState, generation: u64) {
        let imp = self.imp();
        imp.state.set(state);

        self.update_top_bar();
        self.update_send_button();

        if let ChatActionBarState::Editing(message_id) = state {
            self.load_message_to_edit(message_id, generation);
        }
    }

    fn update_top_bar(&self) {
        use ChatActionBarState::*;
        let imp = self.imp();

        match imp.state.get() {
            Composing => {
                imp.top_bar_title_label.set_text(None);
                imp.top_bar_message_label.set_text(None);
                imp.top_bar_revealer.set_reveal_child(false);
            }
            Replying(message_id) => {
                if let Some(message) = self.chat().and_then(|c| c.message(message_id)) {
                    imp.top_bar_title_label
                        .set_text(Some(&strings::message_sender(&message.sender(), true)));
                    imp.top_bar_message_label
                        .set_text(Some(&strings::message_content(&message)));
                } else {
                    imp.top_bar_title_label.set_text(Some(&gettext("Unknown")));
                    imp.top_bar_message_label
                        .set_text(Some(&gettext("Deleted Message")));
                }

                imp.top_bar_image
                    .set_icon_name(Some("mail-reply-sender-symbolic"));
                imp.top_bar_revealer.set_reveal_child(true);
            }
            Editing(message_id) => {
                if let Some(message) = self.chat().and_then(|c| c.message(message_id)) {
                    imp.top_bar_title_label
                        .set_text(Some(&gettext("Edit Message")));
                    imp.top_bar_message_label
                        .set_text(Some(&strings::message_content(&message)));
                } else {
                    imp.top_bar_title_label.set_text(Some(&gettext("Unknown")));
                    imp.top_bar_message_label
                        .set_text(Some(&gettext("Deleted Message")));
                }

                imp.top_bar_image.set_icon_name(Some("edit-symbolic"));
                imp.top_bar_revealer.set_reveal_child(true);
            }
        }
    }

    fn update_send_button(&self) {
        let imp = self.imp();
        match imp.state.get() {
            ChatActionBarState::Editing(_) => {
                imp.send_message_button.set_icon_name("done-symbolic");
            }
            _ => {
                imp.send_message_button.set_icon_name("go-up-symbolic");
            }
        }
    }

    fn load_message_to_edit(&self, id: MessageId, generation: u64) {
        if let Some(chat) = self.chat() {
            let client_id = chat.session_().client_().id();

            if let Some(message) = chat.message(id) {
                let text = match message.content().0 {
                    tdlib::enums::MessageContent::MessageText(data) => Some(data.text),
                    other => super::super::captioned_media_caption(&other).cloned(),
                };

                if let Some(text) = text {
                    utils::spawn(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        async move {
                            match tdlib::functions::get_markdown_text(text, client_id).await {
                                Ok(tdlib::enums::FormattedText::FormattedText(markdown_text)) => {
                                    if obj.imp().state_change_generation.get() != generation
                                        || obj.imp().state.get() != ChatActionBarState::Editing(id)
                                    {
                                        return;
                                    }
                                    obj.imp().message_entry.set_formatted_text(Some(
                                        model::BoxedFormattedText(markdown_text),
                                    ));
                                }
                                Err(e) => {
                                    log::warn!("Failed to load message text for editing: {e:?}")
                                }
                            }
                        }
                    ));
                }
            }
        }
    }

    pub(super) fn reset(&self) {
        self.set_state(ChatActionBarState::Composing);
        self.imp().message_entry.set_formatted_text(None);
    }
}
