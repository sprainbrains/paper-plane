use std::cell::Cell;
use std::cell::OnceCell;
use std::sync::OnceLock;

use glib::Properties;
use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::glib;

use crate::model;

#[derive(Clone, Debug, glib::Boxed)]
#[boxed_type(name = "ChatHistoryItemType")]
pub(crate) enum ChatHistoryItemType {
    Message(model::Message),
    /// Consecutive messages sharing one `media_album_id`, displayed as a single
    /// grid bubble. Members are stored oldest-first; the first one is the
    /// album's primary (its sender, date, caption and menu target).
    Album(Vec<model::Message>),
    DayDivider(glib::DateTime),
    /// A "unread messages" separator placed before the first unread message. Carries
    /// the `last_read_inbox_message_id` boundary it was created for.
    UnreadDivider(i64),
    /// The channel post a comment thread is anchored to, rendered as the topmost
    /// row of the thread window. Deliberately hidden from `message()`/pagination:
    /// its id lives in the channel's id space, not the discussion's.
    ThreadAnchor(model::Message),
    /// The "Start of discussion" pill between the thread anchor and its comments.
    ThreadIntro,
    /// Invisible filler pinned at the oldest end (screen top). It absorbs the
    /// slack whenever the loaded window is shorter than the viewport, so short
    /// chats sit at the bottom the way Telegram renders them; with taller
    /// content it collapses to zero height. It is not a message and never
    /// participates in pagination, grouping or dividers. The height to fill is
    /// published on the item's `slack` property (a single instance is shared
    /// per window), so the row needs no widget-tree access to render it.
    Spacer,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::ChatHistoryItem)]
    pub(crate) struct ChatHistoryItem {
        #[property(set, construct_only)]
        pub(super) type_: OnceCell<ChatHistoryItemType>,
        /// Fill height for the spacer variant, owned by the chat history view.
        pub(super) slack: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistoryItem {
        const NAME: &'static str = "ChatHistoryItem";
        type Type = super::ChatHistoryItem;
    }

    impl ObjectImpl for ChatHistoryItem {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                let mut props = Self::derived_properties().to_vec();
                props.push(
                    glib::ParamSpecInt::builder("slack")
                        .minimum(0)
                        .default_value(0)
                        .explicit_notify()
                        .build(),
                );
                props
            })
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "slack" => {
                    let slack = value.get().unwrap();
                    if self.slack.get() != slack {
                        self.slack.set(slack);
                        self.obj().notify("slack");
                    }
                }
                _ => self.derived_set_property(id, value, pspec),
            }
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "slack" => self.slack.get().to_value(),
                _ => self.derived_property(id, pspec),
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatHistoryItem(ObjectSubclass<imp::ChatHistoryItem>);
}

impl ChatHistoryItem {
    pub(crate) fn for_message(message: model::Message) -> Self {
        let type_ = ChatHistoryItemType::Message(message);
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn for_album(members: Vec<model::Message>) -> Self {
        let type_ = ChatHistoryItemType::Album(members);
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn for_day_divider(day: glib::DateTime) -> Self {
        let type_ = ChatHistoryItemType::DayDivider(day);
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn for_unread_divider(last_read_message_id: i64) -> Self {
        let type_ = ChatHistoryItemType::UnreadDivider(last_read_message_id);
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn for_thread_anchor(message: model::Message) -> Self {
        let type_ = ChatHistoryItemType::ThreadAnchor(message);
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn for_thread_intro() -> Self {
        let type_ = ChatHistoryItemType::ThreadIntro;
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn for_spacer() -> Self {
        let type_ = ChatHistoryItemType::Spacer;
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn slack(&self) -> i32 {
        self.imp().slack.get()
    }

    pub(crate) fn set_slack(&self, slack: i32) {
        self.set_property("slack", slack.max(0));
    }

    pub(crate) fn type_(&self) -> &ChatHistoryItemType {
        self.imp().type_.get().unwrap()
    }

    /// The message this item acts on: the plain message, or the primary
    /// (oldest, caption-carrying) member of an album.
    pub(crate) fn message(&self) -> Option<&model::Message> {
        match self.type_() {
            ChatHistoryItemType::Message(message) => Some(message),
            ChatHistoryItemType::Album(members) => members.first(),
            _ => None,
        }
    }

    /// Every message the item displays or acts on: one for plain messages,
    /// all members for albums, nothing for dividers.
    pub(crate) fn messages(&self) -> &[model::Message] {
        match self.type_() {
            ChatHistoryItemType::Message(message) => std::slice::from_ref(message),
            ChatHistoryItemType::Album(members) => members,
            _ => &[],
        }
    }

    pub(crate) fn album_members(&self) -> Option<&[model::Message]> {
        match self.type_() {
            ChatHistoryItemType::Album(members) => Some(members),
            _ => None,
        }
    }

    pub(crate) fn message_timestamp(&self) -> Option<glib::DateTime> {
        self.message()
            .and_then(|message| glib::DateTime::from_unix_utc(message.date().into()).ok())
            .and_then(|timestamp| timestamp.to_local().ok())
    }
}
