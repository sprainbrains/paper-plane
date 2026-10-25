mod background;
mod chat_action_bar;
mod chat_history;
mod chat_history_row;
mod chat_info_window;
mod content_picker;
mod event_row;
mod forward_window;
mod media_viewer;
mod message_menu;
mod message_menu_reactions;
mod message_row;
mod send_media_window;

use std::cell::Cell;
use std::sync::OnceLock;

use adw::subclass::prelude::*;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;

pub(crate) use self::background::Background;
pub(crate) use self::chat_action_bar::ChatActionBar;
pub(crate) use self::chat_history::ChatHistory;
pub(crate) use self::chat_history::MessageDragPayload;
pub(crate) use self::chat_history_row::ChatHistoryRow;
pub(crate) use self::chat_info_window::ChatInfoWindow;
pub(crate) use self::content_picker::ContentPicker;
pub(crate) use self::event_row::EventRow;
pub(crate) use self::forward_window::ForwardWindow;
pub(crate) use self::media_viewer::MediaViewer;
pub(crate) use self::message_menu::MessageMenu;
pub(crate) use self::message_menu_reactions::MessageMenuReactions;
pub(crate) use self::message_row::DocumentRow;
pub(crate) use self::message_row::MediaPicture;
pub(crate) use self::message_row::MessageAlbum;
pub(crate) use self::message_row::MessageAudio;
pub(crate) use self::message_row::MessageBase;
pub(crate) use self::message_row::MessageBaseExt;
pub(crate) use self::message_row::MessageBaseImpl;
pub(crate) use self::message_row::MessageBubble;
pub(crate) use self::message_row::MessageCommentsBar;
pub(crate) use self::message_row::MessageDocument;
pub(crate) use self::message_row::MessageDocumentStatusIndicator;
pub(crate) use self::message_row::MessageIndicators;
pub(crate) use self::message_row::MessageLabel;
pub(crate) use self::message_row::MessageLocation;
pub(crate) use self::message_row::MessagePhoto;
pub(crate) use self::message_row::MessagePoll;
pub(crate) use self::message_row::MessageReactions;
pub(crate) use self::message_row::MessageReply;
pub(crate) use self::message_row::MessageSticker;
pub(crate) use self::message_row::MessageText;
pub(crate) use self::message_row::MessageVenue;
pub(crate) use self::message_row::MessageVideo;
pub(crate) use self::message_row::MessageVideoNote;
pub(crate) use self::message_row::MessageViewInChatBar;
pub(crate) use self::message_row::MessageVoiceNote;
pub(crate) use self::message_row::Row as MessageRow;
pub(crate) use self::send_media_window::SendMediaWindow;
use crate::model;

fn captioned_media_caption(
    content: &tdlib::enums::MessageContent,
) -> Option<&tdlib::types::FormattedText> {
    use tdlib::enums::MessageContent::*;

    match content {
        MessagePhoto(data) => Some(&data.caption),
        MessageVideo(data) => Some(&data.caption),
        MessageAnimation(data) => Some(&data.caption),
        MessageDocument(data) => Some(&data.caption),
        MessageAudio(data) => Some(&data.caption),
        MessageVoiceNote(data) => Some(&data.caption),
        _ => None,
    }
}

/// Edge of one album cell in logical pixels. Shared by the chat bubble grid
/// (`message_row::album`) and the send-media preview grid.
pub(crate) const ALBUM_CELL_SIZE: i32 = 140;
/// Gap between album cells; mirrors the grid spacing in both templates.
pub(crate) const ALBUM_CELL_SPACING: i32 = 2;

/// Grid placements `(col, row, width, height)` for an album of `n` members,
/// in member order. Even counts fill a plain two-column grid; odd counts give
/// the first member a tall cell spanning the top two rows (the layout
/// Telegram apps use).
pub(crate) fn album_placements(n: usize) -> Vec<(i32, i32, i32, i32)> {
    let mut cells = Vec::new();
    if n == 0 {
        return cells;
    }
    if n % 2 == 1 {
        cells.push((0, 0, 1, 2));
    }
    let mut placed = cells.len();
    let mut row = 0;
    while placed < n {
        for col in 0..2 {
            // The tall first cell owns column 0 of the top two rows.
            if n % 2 == 1 && col == 0 && row < 2 {
                continue;
            }
            cells.push((col, row, 1, 1));
            placed += 1;
            if placed == n {
                break;
            }
        }
        row += 1;
    }
    cells
}

/// TDLib groups at most ten messages into one album.
pub(crate) const ALBUM_MAX_MEMBERS: usize = 10;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/mod.ui")]
    pub(crate) struct Content {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) compact: Cell<bool>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) unselected_chat_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub(super) chat_history: TemplateChild<ChatHistory>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Content {
        const NAME: &'static str = "Content";
        type Type = super::Content;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Content {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecBoolean::builder("compact").build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "chat" => {
                    let chat = value.get().unwrap();
                    obj.set_chat(chat);
                }
                "compact" => {
                    let compact = value.get().unwrap();
                    if self.compact.replace(compact) != compact {
                        obj.notify("compact");
                    }
                }
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "chat" => obj.chat().to_value(),
                "compact" => self.compact.get().to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl WidgetImpl for Content {}
    impl BinImpl for Content {}
}

glib::wrapper! {
    pub(crate) struct Content(ObjectSubclass<imp::Content>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Content {
    fn default() -> Self {
        Self::new()
    }
}

impl Content {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn handle_paste_action(&self) {
        self.imp().chat_history.handle_paste_action();
    }

    pub(crate) fn insert_emoji(&self, emoji: &str) {
        self.imp().chat_history.insert_emoji(emoji);
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        self.imp()
            .chat_history
            .insert_custom_emoji(emoji, custom_emoji_id);
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    /// Jumps the open chat's history to its newest message.
    pub(crate) fn scroll_history_to_latest(&self) {
        self.imp().chat_history.scroll_to_latest();
    }

    /// Arms the header back arrow for the next switch to `to_chat` (see
    /// `ChatHistory::arm_return_to`).
    pub(crate) fn arm_return_to(&self, from_chat: i64, to_chat: i64) {
        self.imp().chat_history.arm_return_to(from_chat, to_chat);
    }

    /// Records a message to reveal after the next switch to `chat_id` (see
    /// `ChatHistory::arm_reveal`).
    pub(crate) fn arm_reveal(&self, chat_id: i64, message_id: i64) {
        self.imp().chat_history.arm_reveal(chat_id, message_id);
    }

    fn set_chat(&self, chat: Option<&model::Chat>) {
        if self.chat().as_ref() == chat {
            return;
        }

        let imp = self.imp();
        if chat.is_some() {
            imp.stack.set_visible_child(&imp.chat_history.get());
        } else {
            imp.stack.set_visible_child(&imp.unselected_chat_view.get());
        }

        imp.chat.set(chat);

        self.notify("chat");
    }
}
