use gettextrs::gettext;

use crate::i18n::gettext_f;
use crate::model;
use crate::strings::message_sender;

pub(crate) fn chat_action(action: &model::ChatAction) -> String {
    use tdlib::enums::ChatAction::*;

    let chat = action.chat_();

    let show_sender = matches!(
        chat.chat_type(),
        model::ChatType::BasicGroup(_) | model::ChatType::Supergroup(_)
    );

    let td_action = &action.action_type().0;

    let action_group = chat.actions().group(td_action);

    // The literal templates must stay inline at each gettext_f call so xgettext extracts
    // them; only the repeated sender lookup is factored out.
    let sender = |i: usize| message_sender(&action_group[i].sender(), false);

    match td_action {
        ChoosingContact => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f("{sender} is choosing a contact", &[("sender", &sender(0))]),
                    2 => gettext_f(
                        "{sender1} and {sender2} are choosing contacts",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are choosing contacts",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("choosing a contact")
            }
        }
        ChoosingLocation => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f("{sender} is choosing a location", &[("sender", &sender(0))]),
                    2 => gettext_f(
                        "{sender1} and {sender2} are choosing locations",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are choosing locations",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("choosing a location")
            }
        }
        ChoosingSticker => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f("{sender} is choosing a sticker", &[("sender", &sender(0))]),
                    2 => gettext_f(
                        "{sender1} and {sender2} are choosing stickers",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are choosing stickers",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("choosing a sticker")
            }
        }
        RecordingVideo => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f("{sender} is recording a video", &[("sender", &sender(0))]),
                    2 => gettext_f(
                        "{sender1} and {sender2} are recording videos",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are recording videos",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("recording a video")
            }
        }
        RecordingVideoNote => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is recording a video note",
                        &[("sender", &sender(0))],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are recording video notes",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are recording video notes",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("recording a video note")
            }
        }
        RecordingVoiceNote => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is recording a voice note",
                        &[("sender", &sender(0))],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are recording voice notes",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are recording voice notes",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("recording a voice note")
            }
        }
        StartPlayingGame => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f("{sender} is playing a game", &[("sender", &sender(0))]),
                    2 => gettext_f(
                        "{sender1} and {sender2} are playing games",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are playing games",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext("playing a game")
            }
        }
        Typing => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f("{sender} is typing", &[("sender", &sender(0))]),
                    2 => gettext_f(
                        "{sender1} and {sender2} are typing",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f("{num} people are typing", &[("num", &len.to_string())]),
                }
            } else {
                gettext("typing")
            }
        }
        UploadingDocument(action) => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is uploading a document ({progress}%)",
                        &[
                            ("sender", &sender(0)),
                            ("progress", &action.progress.to_string()),
                        ],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are uploading documents",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are uploading documents",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext_f(
                    "uploading a document ({progress}%)",
                    &[("progress", &action.progress.to_string())],
                )
            }
        }
        UploadingPhoto(action) => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is uploading a photo ({progress}%)",
                        &[
                            ("sender", &sender(0)),
                            ("progress", &action.progress.to_string()),
                        ],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are uploading photos",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are uploading photos",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext_f(
                    "uploading a photo ({progress}%)",
                    &[("progress", &action.progress.to_string())],
                )
            }
        }
        UploadingVideo(action) => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is uploading a video ({progress}%)",
                        &[
                            ("sender", &sender(0)),
                            ("progress", &action.progress.to_string()),
                        ],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are uploading videos",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are uploading videos",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext_f(
                    "uploading a video ({progress}%)",
                    &[("progress", &action.progress.to_string())],
                )
            }
        }
        UploadingVideoNote(action) => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is uploading a video note ({progress}%)",
                        &[
                            ("sender", &sender(0)),
                            ("progress", &action.progress.to_string()),
                        ],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are uploading video notes",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are uploading video notes",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext_f(
                    "uploading a video note ({progress}%)",
                    &[("progress", &action.progress.to_string())],
                )
            }
        }
        UploadingVoiceNote(action) => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is uploading a voice note ({progress}%)",
                        &[
                            ("sender", &sender(0)),
                            ("progress", &action.progress.to_string()),
                        ],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are uploading voice notes",
                        &[("sender1", &sender(0)), ("sender2", &sender(1))],
                    ),
                    len => gettext_f(
                        "{num} people are uploading voice notes",
                        &[("num", &len.to_string())],
                    ),
                }
            } else {
                gettext_f(
                    "uploading a voice note ({progress}%)",
                    &[("progress", &action.progress.to_string())],
                )
            }
        }
        WatchingAnimations(action) => {
            if show_sender {
                match action_group.len() {
                    1 => gettext_f(
                        "{sender} is watching an animation {emoji}",
                        &[("sender", &sender(0)), ("emoji", &action.emoji)],
                    ),
                    2 => gettext_f(
                        "{sender1} and {sender2} are watching animations {emoji}",
                        &[
                            ("sender1", &sender(0)),
                            ("sender2", &sender(1)),
                            ("emoji", &action.emoji),
                        ],
                    ),
                    len => gettext_f(
                        "{num} people are watching animations {emoji}",
                        &[("num", &len.to_string()), ("emoji", &action.emoji)],
                    ),
                }
            } else {
                gettext("watching an animation")
            }
        }
        Cancel => unreachable!(),
    }
}
