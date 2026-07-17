use anyhow::anyhow;
use glib::clone;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::ChatActionBarState;
use super::PHOTO_MIME_TYPES;
use crate::model;
use crate::types::MessageId;
use crate::ui;
use crate::utils;

impl super::ChatActionBar {
    pub(super) async fn save_message_as_draft(&self) {
        if let Some(chat) = self.chat() {
            let reply_to_message_id =
                if let ChatActionBarState::Replying(id) = self.imp().state.get() {
                    id
                } else {
                    0
                };
            let formatted_text = self.imp().message_entry.formatted_text();
            Self::save_draft_for(chat, reply_to_message_id, formatted_text).await;
        }
    }

    /// Persists `formatted_text` as `chat`'s draft. Takes its inputs by value so `set_chat`
    /// can capture the OUTGOING chat's state and text before the composer is swapped to the
    /// new chat, then save them once this deferred future runs.
    pub(super) async fn save_draft_for(
        chat: model::Chat,
        reply_to_message_id: MessageId,
        formatted_text: Option<model::BoxedFormattedText>,
    ) {
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();

        let draft_message = match formatted_text {
            Some(formatted_text) => {
                let text = tdlib::functions::parse_markdown(formatted_text.0.clone(), client_id)
                    .await
                    .map(|tdlib::enums::FormattedText::FormattedText(text)| text)
                    .unwrap_or(formatted_text.0);
                Some(tdlib::types::DraftMessage {
                    reply_to: (reply_to_message_id != 0).then(|| {
                        tdlib::enums::InputMessageReplyTo::Message(
                            tdlib::types::InputMessageReplyToMessage {
                                message_id: reply_to_message_id,
                                ..Default::default()
                            },
                        )
                    }),
                    date: glib::DateTime::now_local().unwrap().to_unix() as i32,
                    input_message_text: tdlib::enums::InputMessageContent::InputMessageText(
                        tdlib::types::InputMessageText {
                            text,
                            link_preview_options: None,
                            clear_draft: true,
                        },
                    ),
                    effect_id: 0,
                    suggested_post_info: None,
                })
            }
            None => None,
        };

        if let Err(e) =
            tdlib::functions::set_chat_draft_message(chat_id, None, draft_message, client_id).await
        {
            log::warn!("Error setting a draft message: {e:?}");
        }
    }

    pub(super) fn load_draft_message(&self, message: model::BoxedDraftMessage) {
        let imp = self.imp();

        let reply_to_message_id = match &message.0.reply_to {
            Some(tdlib::enums::InputMessageReplyTo::Message(m)) => m.message_id,
            _ => 0,
        };
        if reply_to_message_id != 0 {
            self.set_state(ChatActionBarState::Replying(reply_to_message_id));
        } else {
            self.set_state(ChatActionBarState::Composing);
        }

        if let tdlib::enums::InputMessageContent::InputMessageText(content) =
            message.0.input_message_text
        {
            imp.message_entry
                .set_formatted_text(Some(model::BoxedFormattedText(content.text)));
        } else {
            log::warn!(
                "Unexpected draft message type: {:?}",
                message.0.input_message_text
            );
            imp.message_entry.set_formatted_text(None);
        }
    }

    pub(super) async fn send_chat_action(&self, action: tdlib::enums::ChatAction) {
        let imp = self.imp();
        if imp.chat_action_in_cooldown.get() {
            return;
        }

        if let Some(chat) = self.chat() {
            let client_id = chat.session_().client_().id();
            let chat_id = chat.id();

            // Enable chat action cooldown right away
            imp.chat_action_in_cooldown.set(true);

            // Send typing action
            let result =
                tdlib::functions::send_chat_action(chat_id, None, Some(action), client_id).await;
            if result.is_ok() {
                glib::timeout_add_seconds_local_once(
                    5,
                    clone!(
                        #[weak(rename_to = obj)]
                        self,
                        move || {
                            obj.imp().chat_action_in_cooldown.set(false);
                        }
                    ),
                );
            } else {
                imp.chat_action_in_cooldown.set(false);
            }
        }
    }

    pub(crate) fn handle_paste_action(&self) {
        if let Some(chat) = self.chat() {
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    if let Err(e) = obj.handle_image_clipboard(chat).await {
                        log::warn!("Error on pasting an image: {e:?}");
                    }
                }
            ));
        }
    }

    async fn handle_image_clipboard(&self, chat: model::Chat) -> Result<(), anyhow::Error> {
        if let Ok((stream, mime)) = self
            .clipboard()
            .read_future(PHOTO_MIME_TYPES, glib::Priority::DEFAULT)
            .await
        {
            let extension = match mime.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                other => return Err(anyhow!("Unsupported clipboard image mime type: {other}")),
            };

            let temp_dir = utils::temp_dir()
                .ok_or_else(|| anyhow!("The temporary directory doesn't exist"))?;
            let path = temp_dir.join("clipboard").with_extension(extension);

            save_stream_to_file(stream, &path).await?;

            let parent = self.root().and_downcast().unwrap();
            let path = path.to_str().unwrap().to_string();
            ui::SendMediaWindow::new(&parent, &chat, path, self.imp().state.get().replying())
                .present();
        }

        Ok(())
    }
}

async fn save_stream_to_file(
    stream: gio::InputStream,
    path: impl AsRef<std::path::Path>,
) -> Result<(), glib::Error> {
    let file = gio::File::for_path(path);
    let file_stream = file
        .replace_future(
            None,
            false,
            gio::FileCreateFlags::REPLACE_DESTINATION,
            glib::Priority::DEFAULT,
        )
        .await?;

    file_stream
        .splice_future(
            &stream,
            gio::OutputStreamSpliceFlags::CLOSE_SOURCE | gio::OutputStreamSpliceFlags::CLOSE_TARGET,
            glib::Priority::DEFAULT,
        )
        .await?;

    Ok(())
}
