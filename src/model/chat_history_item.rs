use std::cell::OnceCell;

use glib::Properties;
use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::glib;

use crate::model;

#[derive(Clone, Debug, glib::Boxed)]
#[boxed_type(name = "ChatHistoryItemType")]
pub(crate) enum ChatHistoryItemType {
    Message(model::Message),
    DayDivider(glib::DateTime),
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::ChatHistoryItem)]
    pub(crate) struct ChatHistoryItem {
        #[property(set, construct_only)]
        pub(super) type_: OnceCell<ChatHistoryItemType>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistoryItem {
        const NAME: &'static str = "ChatHistoryItem";
        type Type = super::ChatHistoryItem;
    }

    impl ObjectImpl for ChatHistoryItem {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
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

    pub(crate) fn for_day_divider(day: glib::DateTime) -> Self {
        let type_ = ChatHistoryItemType::DayDivider(day);
        glib::Object::builder().property("type", type_).build()
    }

    pub(crate) fn type_(&self) -> &ChatHistoryItemType {
        self.imp().type_.get().unwrap()
    }

    pub(crate) fn message(&self) -> Option<&model::Message> {
        if let ChatHistoryItemType::Message(message) = self.type_() {
            Some(message)
        } else {
            None
        }
    }

    pub(crate) fn message_timestamp(&self) -> Option<glib::DateTime> {
        if let ChatHistoryItemType::Message(message) = self.type_() {
            glib::DateTime::from_unix_utc(message.date().into())
                .and_then(|timestamp| timestamp.to_local())
                .ok()
        } else {
            None
        }
    }
}
