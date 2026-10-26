use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
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
        /// The live spacer widget while this row is bound to a spacer item.
        pub(super) spacer: RefCell<Option<ChatSpacer>>,
        /// `notify::slack` connection: the item it was attached to (weakly)
        /// and its handler id; cleared whenever the row is bound to anything
        /// else. The target is remembered because spacer items are swapped
        /// when a history window is rebuilt — an id from a dead object must
        /// never be handed to another instance's `disconnect`.
        pub(super) spacer_handler:
            RefCell<Option<(glib::WeakRef<model::ChatHistoryItem>, glib::SignalHandlerId)>>,
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
            self.obj().apply_message_grouping();
            self.obj().update_selection_visual();
        }

        fn unmap(&self) {
            self.parent_unmap();
        }
    }
    impl BinImpl for ChatHistoryRow {}
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

    pub(crate) fn set_item(&self, item: Option<glib::Object>) {
        if self.item() == item {
            return;
        }

        if let Some(ref item) = item {
            if let Some(item) = item.downcast_ref::<model::ChatHistoryItem>() {
                let is_spacer = matches!(item.type_(), model::ChatHistoryItemType::Spacer);
                if !is_spacer {
                    if let Some((item, handler)) = self.imp().spacer_handler.take()
                        && let Some(item) = item.upgrade()
                    {
                        item.disconnect(handler);
                    }
                    self.imp().spacer.replace(None);
                }
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
                            }
                            _ => {
                                self.set_comments_suppressed(false);
                                self.update_or_create_message_row(message.to_owned().upcast());
                            }
                        }
                    }
                    // The album widget takes the whole item; the row chrome uses
                    // its primary message.
                    model::ChatHistoryItemType::Album(_) => {
                        self.set_comments_suppressed(false);
                        self.update_or_create_message_row(item.to_owned().upcast());
                    }
                    // The thread's anchor post: bound like a message, but it *is*
                    // the thread, so its comments bar is pinned off.
                    model::ChatHistoryItemType::ThreadAnchor(message) => {
                        self.update_or_create_message_row(message.to_owned().upcast());
                        self.set_comments_suppressed(true);
                    }
                    model::ChatHistoryItemType::ThreadIntro => {
                        let child = self.get_or_create_event_row();
                        child.set_label(&gettext("Start of discussion"));
                    }
                    // Invisible filler that soaks up the slack when the loaded
                    // window is shorter than the viewport (see `show_spacer`).
                    model::ChatHistoryItemType::Spacer => self.show_spacer(item),
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

        // Recycled rows must not stay invisible because the send animation hid them
        // while they displayed a different message.
        self.set_opacity(1.0);

        self.imp().item.replace(item);
        self.notify("item");

        self.apply_message_grouping();
        self.update_selection_visual();
    }

    /// Reflects multi-select state on this row: a subtle accent tint when selected, plus
    /// the trailing checkbox (shown for every message while in selection mode). The tint
    /// classes go on the list-item wrapper (this widget's parent) so consecutive
    /// selected rows paint one seamless block, rounded only at the run's outer corners.
    pub(crate) fn update_selection_visual(&self) {
        let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(self) else {
            return;
        };
        let in_selection_mode = chat_history.is_selection_mode();
        let item = self.item().and_downcast::<model::ChatHistoryItem>();
        let selected = item
            .as_ref()
            .and_then(|item| item.message().map(|message| message.id()))
            .is_some_and(|id| chat_history.is_message_selected(id));

        let target = self.parent().unwrap_or_else(|| self.clone().upcast());
        for class in [
            "message-selected",
            "selection-run-start",
            "selection-run-end",
        ] {
            target.remove_css_class(class);
        }
        if selected {
            target.add_css_class("message-selected");
            let (is_start, is_end) = item
                .zip(chat_history.history_model())
                .map(|(item, model)| {
                    model.selection_run_flags(&item, &|id| chat_history.is_message_selected(id))
                })
                .unwrap_or((true, true));
            if is_start {
                target.add_css_class("selection-run-start");
            }
            if is_end {
                target.add_css_class("selection-run-end");
            }
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

    /// Binds this row to a spacer item: an invisible widget filling the
    /// viewport slack. The slack lives on the item (published by the chat
    /// history view), never in the widget tree — rows can bind while detached
    /// from the list, so ancestor lookups are unreliable here.
    fn show_spacer(&self, item: &model::ChatHistoryItem) {
        if let Some((prev, handler)) = self.imp().spacer_handler.take()
            && let Some(prev) = prev.upgrade()
        {
            prev.disconnect(handler);
        }
        let spacer = ChatSpacer::new();
        self.set_child(Some(&spacer));
        self.imp().spacer.replace(Some(spacer));
        self.apply_spacer_slack(item);
        let handler = item.connect_notify_local(
            Some("slack"),
            clone!(
                #[weak(rename_to = row)]
                self,
                move |item, _| row.apply_spacer_slack(item)
            ),
        );
        let weak = glib::WeakRef::new();
        weak.set(Some(item));
        self.imp().spacer_handler.replace(Some((weak, handler)));
    }

    fn apply_spacer_slack(&self, item: &model::ChatHistoryItem) {
        let imp = self.imp();
        let Some(spacer) = imp.spacer.borrow().clone() else {
            return;
        };
        spacer.set_slack(item.slack());
    }

    /// Toggles the comments bar on this row's (possibly reused) message content.
    /// Recycled rows carry the previous item's state into the next bind, so the
    /// flag is set *before* rebinding for plain messages and after for anchors
    /// (the bind itself would otherwise show the bar on the thread anchor).
    fn set_comments_suppressed(&self, suppressed: bool) {
        if let Some(bar) = utils::descendant::<_, ui::MessageCommentsBar>(self) {
            bar.set_suppressed(suppressed);
        }
    }

    fn get_or_create_event_row(&self) -> ui::EventRow {
        if let Some(Ok(child)) = self.child().map(|w| w.downcast::<ui::EventRow>()) {
            // Reset per-reuse state so a recycled row can't keep a previous item's
            // thumbnail (e.g. a "changed the group photo" event recycled into a divider).
            child.set_minithumbnail(None);
            child
        } else {
            let child = ui::EventRow::new();
            self.set_child(Some(&child));
            child
        }
    }
}

/// Invisible bottom-fill row: reports zero minimum height and `slack` as its
/// natural height, so it absorbs viewport slack without ever forcing a
/// minimum the list cannot satisfy mid-transition (a hard `height-request`
/// would emit allocation criticals while the window or page size changes).
mod spacer_imp {
    use gtk::subclass::prelude::WidgetImpl as GtkWidgetImpl;

    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct ChatSpacer {
        pub(super) slack: std::cell::Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatSpacer {
        const NAME: &'static str = "PaplChatSpacer";
        type Type = super::ChatSpacer;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ChatSpacer {}

    impl GtkWidgetImpl for ChatSpacer {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Vertical {
                (0, self.slack.get().max(0), -1, -1)
            } else {
                (0, for_size.max(0), -1, -1)
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatSpacer(ObjectSubclass<spacer_imp::ChatSpacer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ChatSpacer {
    fn new() -> Self {
        glib::Object::builder().build()
    }

    fn slack(&self) -> i32 {
        self.imp().slack.get()
    }

    fn set_slack(&self, slack: i32) {
        if self.slack() != slack {
            self.imp().slack.set(slack);
            self.queue_resize();
        }
    }
}
