use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;

use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct ChatHistoryRow {
        /// An `ChatHistoryItem` or `SponsoredMessage`
        pub(super) item: RefCell<Option<glib::Object>>,
        /// Transient translation applied while the row flies in from the send-animation
        /// staging point (see `ChatHistory::play_send_animation`). Relative to the row's
        /// rest position, so concurrent scrolling can't misplace the landing.
        pub(super) flight_offset: Cell<(f32, f32)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistoryRow {
        const NAME: &'static str = "PaplChatHistoryRow";
        type Type = super::ChatHistoryRow;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ChatHistoryRow {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<glib::Object>("item")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "item" => obj.set_item(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "item" => obj.item().to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl WidgetImpl for ChatHistoryRow {
        fn map(&self) {
            self.parent_map();
            self.perform_history_action(ui::ChatHistory::add_to_viewed_message_ids);
            self.obj().apply_message_grouping();
            self.obj().update_selection_visual();
        }

        fn unmap(&self) {
            self.parent_unmap();
            self.perform_history_action(ui::ChatHistory::remove_from_viewed_message_ids);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let (dx, dy) = self.flight_offset.get();
            if dx == 0.0 && dy == 0.0 {
                self.parent_snapshot(snapshot);
            } else {
                snapshot.translate(&gtk::graphene::Point::new(dx, dy));
                self.parent_snapshot(snapshot);
            }
        }
    }
    impl BinImpl for ChatHistoryRow {}

    impl ChatHistoryRow {
        fn perform_history_action<F: Fn(&ui::ChatHistory, i64)>(&self, op: F) {
            let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(&*self.obj()) else {
                return;
            };

            if let Some(item) = &*self.item.borrow() {
                if let Some(item) = item.downcast_ref::<model::ChatHistoryItem>() {
                    if let model::ChatHistoryItemType::Message(message) = item.type_() {
                        op(&chat_history, message.id());
                    }
                } else if let Some(message) = item.downcast_ref::<model::SponsoredMessage>() {
                    op(&chat_history, message.message_id());
                }
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatHistoryRow(ObjectSubclass<imp::ChatHistoryRow>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ChatHistoryRow {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistoryRow {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn item(&self) -> Option<glib::Object> {
        self.imp().item.borrow().to_owned()
    }

    /// Translates the row's rendering by (`dx`, `dy`) relative to its rest position;
    /// driven each frame by the send animation's flight phase.
    pub(crate) fn set_flight_offset(&self, dx: f32, dy: f32) {
        if self.imp().flight_offset.replace((dx, dy)) != (dx, dy) {
            self.queue_draw();
        }
    }

    pub(crate) fn set_item(&self, item: Option<glib::Object>) {
        if self.item() == item {
            return;
        }

        if let Some(ref item) = item {
            if let Some(item) = item.downcast_ref::<model::ChatHistoryItem>() {
                match item.type_() {
                    model::ChatHistoryItemType::Message(message) => {
                        use tdlib::enums::MessageContent::*;

                        match message.content().0 {
                            MessageChatChangePhoto(data) => {
                                let event_row = self.get_or_create_event_row();
                                event_row.set_label(&strings::message_content(message));
                                event_row.set_minithumbnail(data.photo.minithumbnail.as_ref());
                            }
                            MessageExpiredPhoto
                            | MessageExpiredVideo
                            | MessageCall(_)
                            | MessageBasicGroupChatCreate(_)
                            | MessageSupergroupChatCreate(_)
                            | MessageChatChangeTitle(_)
                            | MessageChatDeletePhoto
                            | MessageChatAddMembers(_)
                            | MessageChatJoinByLink
                            | MessageChatJoinByRequest
                            | MessageChatDeleteMember(_)
                            | MessagePinMessage(_)
                            | MessageScreenshotTaken
                            | MessageGameScore(_)
                            | MessageContactRegistered => {
                                let event_row = self.get_or_create_event_row();
                                event_row.set_label(&strings::message_content(message));
                                // Clear any preview left over from a recycled row.
                                event_row.set_minithumbnail(None);
                            }
                            _ => self.update_or_create_message_row(message.to_owned().upcast()),
                        }
                    }
                    model::ChatHistoryItemType::DayDivider(date) => {
                        let fmt = if date.year() == glib::DateTime::now_local().unwrap().year() {
                            // Translators: This is a date format in the day divider without the year
                            gettext("%B %e")
                        } else {
                            // Translators: This is a date format in the day divider with the year
                            gettext("%B %e, %Y")
                        };
                        let date = date.format(&fmt).unwrap().to_string();

                        let child = self.get_or_create_event_row();
                        child.set_label(&date);
                    }
                    model::ChatHistoryItemType::UnreadDivider(_) => {
                        let child = self.get_or_create_event_row();
                        child.set_label(&gettext("Unread messages"));
                    }
                }
            } else if let Some(sponsored_message) = item.downcast_ref::<model::SponsoredMessage>() {
                let content = &sponsored_message.content().0;
                if !matches!(content, tdlib::enums::MessageContent::MessageText(_)) {
                    log::warn!("Unexpected sponsored message of type: {content:?}");
                }

                self.update_or_create_message_row(sponsored_message.to_owned().upcast());
            } else {
                unreachable!("Unexpected item type: {:?}", item);
            }
        }

        // Recycled rows must not inherit a leftover send-flight translation.
        self.set_flight_offset(0.0, 0.0);

        self.imp().item.replace(item);
        self.notify("item");

        self.apply_message_grouping();
        self.update_selection_visual();
    }

    /// Reflects multi-select state on this row: a subtle accent tint when selected, plus
    /// the trailing checkbox (shown for every message while in selection mode).
    pub(crate) fn update_selection_visual(&self) {
        let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(self) else {
            return;
        };
        let in_selection_mode = chat_history.is_selection_mode();
        let selected = self
            .item()
            .and_downcast::<model::ChatHistoryItem>()
            .and_then(|item| item.message().map(|message| message.id()))
            .is_some_and(|id| chat_history.is_message_selected(id));

        if selected {
            self.add_css_class("message-selected");
        } else {
            self.remove_css_class("message-selected");
        }

        if let Some(row) = self.child().and_downcast::<ui::MessageRow>() {
            row.set_selection_state(in_selection_mode, selected);
        }
    }

    /// Recomputes and applies same-sender grouping visuals to the message row, using
    /// the neighbouring items in the history model. No-op for dividers/event rows.
    pub(crate) fn apply_message_grouping(&self) {
        let Some(child) = self.child().and_downcast::<ui::MessageRow>() else {
            return;
        };
        let Some(item) = self.item().and_downcast::<model::ChatHistoryItem>() else {
            return;
        };
        if item.message().is_none() {
            return;
        }
        let (is_start, is_end) = utils::ancestor::<_, ui::ChatHistory>(self)
            .and_then(|chat_history| chat_history.history_model())
            .map(|model| model.group_flags(&item))
            .unwrap_or((true, true));
        child.apply_grouping(is_start, is_end);
    }

    fn update_or_create_message_row(&self, message: glib::Object) {
        match self
            .child()
            .and_then(|w| w.downcast::<ui::MessageRow>().ok())
        {
            Some(child) => child.set_message(message),
            None => {
                let child = ui::MessageRow::new(&message);
                self.set_child(Some(&child));
            }
        }
    }

    fn get_or_create_event_row(&self) -> ui::EventRow {
        if let Some(Ok(child)) = self.child().map(|w| w.downcast::<ui::EventRow>()) {
            child
        } else {
            let child = ui::EventRow::new();
            self.set_child(Some(&child));
            child
        }
    }
}
