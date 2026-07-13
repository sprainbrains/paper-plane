use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use futures::Future;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::graphene;

use crate::expressions;
use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

/// Whether the gesture's current event came from a touchscreen. Selection dragging is
/// mouse-only so touch input keeps kinetic-scrolling the list.
fn is_touch(gesture: &gtk::GestureDrag) -> bool {
    gesture
        .current_event_device()
        .is_some_and(|device| device.source() == gdk::InputSource::Touchscreen)
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_history.ui")]
    pub(crate) struct ChatHistory {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) chat_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) model: RefCell<Option<model::ChatHistoryModel>>,
        pub(super) message_menu: OnceCell<ui::MessageMenu>,
        pub(super) is_auto_scrolling: Cell<bool>,
        pub(super) is_loading_messages: Cell<bool>,
        pub(super) sticky: Cell<bool>,
        pub(super) viewed_message_ids: RefCell<HashSet<i64>>,
        pub(super) viewed_message_ids_changed: Cell<bool>,
        pub(super) selection_mode: Cell<bool>,
        pub(super) selected_message_ids: RefCell<HashSet<i64>>,
        /// State for an in-progress selection drag: whether one is active, the anchor
        /// message it started from (the range is anchor..pointer), the latest pointer in
        /// this widget's coordinates, and the running edge auto-scroll timer.
        pub(super) selection_dragging: Cell<bool>,
        /// Set when a mouse drag began over selectable bubble text (outside selection
        /// mode): the drag is left to the label for text selection, not message sweeping.
        pub(super) drag_skip_for_text: Cell<bool>,
        pub(super) selection_anchor: Cell<Option<i64>>,
        pub(super) selection_drag_point: Cell<(f64, f64)>,
        pub(super) selection_autoscroll: RefCell<Option<glib::SourceId>>,
        pub(super) selection_resweep_source: RefCell<Option<glib::SourceId>>,
        /// While entering/leaving selection mode, the checkbox/toolbar relayout changes
        /// the adjustment over several frames. During this window the scroll is pinned to
        /// `preserved_scroll_value` (for non-drag transitions) so it doesn't jump to the
        /// bottom. `selection_transition_source` releases the window after it settles.
        pub(super) selection_transitioning: Cell<bool>,
        pub(super) preserved_scroll_value: Cell<f64>,
        pub(super) selection_transition_source: RefCell<Option<glib::SourceId>>,
        #[template_child]
        pub(super) selection_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) selection_count_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) header_avatar: TemplateChild<ui::Avatar>,
        #[template_child]
        pub(super) header_typing_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) header_typing_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) send_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) content_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) background: TemplateChild<ui::Background>,
        pub(super) send_animation: RefCell<Option<adw::SpringAnimation>>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) chat_action_bar: TemplateChild<ui::ChatActionBar>,
        #[template_child]
        pub(super) pinned_message_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) pinned_message_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) unpin_button: TemplateChild<gtk::Button>,
        pub(super) pinned_message_id: Cell<i64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistory {
        const NAME: &'static str = "PaplChatHistory";
        type Type = super::ChatHistory;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action("chat-history.view-info", None, move |widget, _, _| {
                widget.open_info_dialog();
            });
            klass.install_action("chat-history.scroll-down", None, move |widget, _, _| {
                widget.scroll_down();
            });
            klass.install_action(
                "chat-history.reply",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.imp().chat_action_bar.reply_to_message_id(message_id);
                },
            );
            klass.install_action(
                "chat-history.edit",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.imp().chat_action_bar.edit_message_id(message_id);
                },
            );
            klass.install_action_async(
                "chat-history.leave-chat",
                None,
                |widget, _, _| async move {
                    widget.show_leave_chat_dialog().await;
                },
            );

            klass.install_action("chat-history.selection-clear", None, |widget, _, _| {
                widget.clear_selection();
            });
            klass.install_action("chat-history.selection-forward", None, |widget, _, _| {
                widget.forward_selection();
            });
            klass.install_action_async(
                "chat-history.selection-delete",
                None,
                |widget, _, _| async move {
                    widget.delete_selection().await;
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatHistory {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecBoolean::builder("sticky")
                        .read_only()
                        .build(),
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
                "sticky" => obj.set_sticky(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "chat" => obj.chat().to_value(),
                "sticky" => obj.sticky().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.setup_expressions();

            self.unpin_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.unpin_message();
                }
            ));

            // Clicking the pinned-message bar reveals the message in the history.
            let pinned_gesture = gtk::GestureClick::new();
            pinned_gesture.connect_released(clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    obj.scroll_to_pinned();
                }
            ));
            self.pinned_message_bar.add_controller(pinned_gesture);

            let adj = self.list_view.vadjustment().unwrap();
            adj.connect_value_changed(clone!(
                #[weak]
                obj,
                move |adj| {
                    obj.view_messages();

                    // While sweeping a selection, any scroll (wheel or auto-scroll)
                    // brings new messages under the pointer — extend the range to them.
                    if obj.imp().selection_dragging.get() {
                        // pick() reflects the pre-scroll layout, so defer the resweep to
                        // idle (after the frame clock re-lays-out the rows).
                        obj.schedule_deferred_resweep();
                    }

                    let imp = obj.imp();

                    if imp.is_loading_messages.get() {
                        return;
                    }

                    // A selection-mode relayout (checkboxes/toolbar) changes the
                    // adjustment but carries no scroll intent — pin the view and don't
                    // latch `sticky` or load more.
                    if obj.pin_scroll_during_transition() {
                        return;
                    }

                    if imp.is_auto_scrolling.get() {
                        if adj.value() + adj.page_size() >= adj.upper() {
                            imp.is_auto_scrolling.set(false);
                            obj.set_sticky(true);
                        }
                    } else {
                        obj.set_sticky(adj.value() + adj.page_size() >= adj.upper());

                        if adj.value() >= adj.page_size() * 2.0
                            && adj.upper() > adj.page_size() * 2.0
                        {
                            return;
                        }

                        if let Some(model) = imp.model.borrow().as_ref() {
                            imp.is_loading_messages.set(true);

                            utils::spawn(clone!(
                                #[weak]
                                obj,
                                #[weak]
                                model,
                                async move {
                                    obj.imp().is_loading_messages.set(false);

                                    if let Err(model::ChatHistoryError::Tdlib(e)) =
                                        model.load_older_messages(2).await
                                    {
                                        log::warn!("Couldn't load more chat messages: {e:?}");
                                    }
                                }
                            ));
                        }
                    }
                }
            ));

            // While a selection drag holds the pointer grab, the GtkScrolledWindow never
            // processes wheel events, so the list can't scroll to bring more messages
            // under the pointer. Intercept scroll in the capture phase and drive the
            // adjustment ourselves (which re-fires value-changed → resweep_selection);
            // when not dragging, let scrolling proceed normally.
            let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
            scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
            scroll.connect_scroll(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _dx, dy| {
                    if !obj.imp().selection_dragging.get() {
                        return glib::Propagation::Proceed;
                    }
                    let adj = obj.imp().list_view.vadjustment().unwrap();
                    let step = adj.step_increment().max(40.0);
                    let max = (adj.upper() - adj.page_size()).max(adj.lower());
                    adj.set_value((adj.value() + dy * step).clamp(adj.lower(), max));
                    glib::Propagation::Stop
                }
            ));
            self.scrolled_window.add_controller(scroll);

            // The selection drag lives on the list widget (which is never recycled),
            // not on an individual message row — otherwise scrolling the starting row
            // off-screen recycles it and GTK cancels the drag mid-selection.
            let list_view = self.list_view.get();
            let drag = gtk::GestureDrag::new();
            drag.set_button(1);
            drag.set_propagation_phase(gtk::PropagationPhase::Capture);
            drag.connect_drag_begin(clone!(
                #[weak]
                obj,
                #[weak]
                list_view,
                move |gesture, start_x, start_y| {
                    // Mouse-only: on touch, leave the drag to the list's kinetic scroll.
                    if is_touch(gesture) {
                        return;
                    }
                    // Outside selection mode, a drag starting on selectable bubble text is
                    // a text selection — leave it to the label rather than sweeping rows.
                    let skip = !obj.is_selection_mode()
                        && obj.point_over_selectable_text(&list_view, start_x, start_y);
                    obj.imp().drag_skip_for_text.set(skip);
                    if !skip {
                        obj.begin_selection_drag();
                    }
                }
            ));
            drag.connect_drag_update(clone!(
                #[weak]
                obj,
                #[weak]
                list_view,
                move |gesture, offset_x, offset_y| {
                    if is_touch(gesture) || obj.imp().drag_skip_for_text.get() {
                        return;
                    }
                    // Require a clear drag so plain clicks (open media/links) are unaffected.
                    if offset_x.hypot(offset_y) < 8.0 {
                        return;
                    }
                    let Some((start_x, start_y)) = gesture.start_point() else {
                        return;
                    };
                    obj.sweep_selection(
                        &list_view,
                        start_x,
                        start_y,
                        start_x + offset_x,
                        start_y + offset_y,
                    );
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            ));
            drag.connect_drag_end(clone!(
                #[weak]
                obj,
                move |_, _, _| obj.end_selection_drag()
            ));
            drag.connect_cancel(clone!(
                #[weak]
                obj,
                move |_, _| obj.end_selection_drag()
            ));
            list_view.add_controller(drag);

            adj.connect_upper_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    if obj.pin_scroll_during_transition() {
                        return;
                    }
                    if obj.sticky() || obj.imp().is_auto_scrolling.get() {
                        obj.scroll_down();
                    }
                }
            ));
        }

        fn dispose(&self) {
            if let Some(chat) = self.obj().chat() {
                perform_chat_action(&chat, tdlib::functions::close_chat);
            }
        }
    }

    impl WidgetImpl for ChatHistory {
        fn direction_changed(&self, previous_direction: gtk::TextDirection) {
            let obj = self.obj();

            if obj.direction() == previous_direction {
                return;
            }

            if let Some(menu) = self.message_menu.get() {
                menu.set_halign(if obj.direction() == gtk::TextDirection::Rtl {
                    gtk::Align::End
                } else {
                    gtk::Align::Start
                });
            }
        }
    }

    impl BinImpl for ChatHistory {}
}

glib::wrapper! {
    pub(crate) struct ChatHistory(ObjectSubclass<imp::ChatHistory>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ChatHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistory {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn setup_expressions(&self) {
        let chat_expression = Self::this_expression("chat");

        // Chat title
        expressions::chat_display_name(&chat_expression).bind(
            &*self.imp().window_title,
            "title",
            Some(self),
        );

        // Chat avatar in the header (also serves as identity when the sidebar is collapsed).
        chat_expression.bind(&*self.imp().header_avatar, "item", Some(self));

        // Typing status in the header: the label shows the current chat action (re-evaluated
        // whenever the chat's actions change) and the row (label + animated dots) is only
        // visible while someone is acting.
        let actions_expression = chat_expression.chain_property::<model::Chat>("actions");
        gtk::ClosureExpression::new::<String>(
            [actions_expression.clone().upcast()],
            glib::closure!(
                |_: super::ChatHistory, actions: Option<model::ChatActionList>| {
                    actions
                        .and_then(|actions| actions.last())
                        .map(|action| strings::chat_action(&action))
                        .unwrap_or_default()
                }
            ),
        )
        .bind(&*self.imp().header_typing_label, "label", Some(self));
        gtk::ClosureExpression::new::<bool>(
            [actions_expression.upcast()],
            glib::closure!(
                |_: super::ChatHistory, actions: Option<model::ChatActionList>| {
                    actions.is_some_and(|actions| actions.n_items() > 0)
                }
            ),
        )
        .bind(&*self.imp().header_typing_box, "visible", Some(self));
    }

    fn open_info_dialog(&self) {
        if let Some(chat) = self.chat() {
            ui::ChatInfoWindow::new(&self.parent_window(), &chat).present();
        }
    }

    async fn show_leave_chat_dialog(&self) {
        if let Some(chat) = self.chat() {
            let dialog = ui::destructive_confirmation_dialog(
                &gettext("Leave chat?"),
                &gettext("Do you want to leave this chat?"),
                ("no", &gettext("_No")),
                ("yes", &gettext("_Yes")),
            );
            dialog.set_close_response("no");

            if dialog.choose_future(Some(self)).await == "yes" {
                match tdlib::functions::leave_chat(chat.id(), chat.session_().client_().id()).await
                {
                    Ok(_) => {
                        // Unselect recently left chat
                        if let Some(sidebar) = utils::ancestor::<_, ui::Sidebar>(self) {
                            sidebar.set_selected_chat(Option::<model::Chat>::None);
                        }
                    }
                    Err(e) => log::warn!("Failed to leave chat: {e:?}"),
                }
            }
        }
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.root()?.downcast().ok()
    }

    fn request_sponsored_message(&self, chat: &model::Chat, list: &gio::ListStore) {
        utils::spawn(clone!(
            #[weak]
            chat,
            #[weak]
            list,
            async move {
                match model::SponsoredMessage::request(&chat).await {
                    Ok(sponsored_message) => {
                        if let Some(sponsored_message) = sponsored_message {
                            list.append(&sponsored_message);
                        }
                    }
                    Err(e) => {
                        if e.code != 404 {
                            log::warn!("Failed to request a SponsoredMessage: {e:?}");
                        }
                    }
                }
            }
        ));
    }

    pub(crate) fn add_to_viewed_message_ids(&self, message_id: i64) {
        let imp = self.imp();
        if imp.viewed_message_ids.borrow_mut().insert(message_id) {
            imp.viewed_message_ids_changed.set(true);
        }
    }

    pub(crate) fn remove_from_viewed_message_ids(&self, message_id: i64) {
        let imp = self.imp();
        if imp.viewed_message_ids.borrow_mut().remove(&message_id) {
            imp.viewed_message_ids_changed.set(true);
        }
    }

    pub(crate) fn message_menu(&self) -> &ui::MessageMenu {
        self.imp().message_menu.get_or_init(|| {
            let menu = ui::MessageMenu::new();

            menu.set_halign(if self.direction() == gtk::TextDirection::Rtl {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });

            menu
        })
    }

    pub(crate) fn handle_paste_action(&self) {
        self.imp().chat_action_bar.handle_paste_action();
    }

    pub(crate) fn insert_emoji(&self, emoji: &str) {
        self.imp().chat_action_bar.insert_emoji(emoji);
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        self.imp()
            .chat_action_bar
            .insert_custom_emoji(emoji, custom_emoji_id);
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    pub(crate) fn set_chat(&self, chat: Option<&model::Chat>) {
        let old_chat = self.chat();
        if chat == old_chat.as_ref() {
            return;
        }

        let imp = self.imp();

        // Reset the pinned-message bar; it's re-populated below if the new chat
        // has a pinned message.
        imp.pinned_message_bar.set_visible(false);

        if let Some(chat) = old_chat {
            chat.disconnect(imp.chat_handler.replace(None).unwrap());
            perform_chat_action(chat.as_ref(), tdlib::functions::close_chat);
        }

        if let Some(chat) = chat {
            self.fetch_pinned_message(chat);

            self.action_set_enabled(
                "chat-history.leave-chat",
                match chat.chat_type() {
                    model::ChatType::BasicGroup(data) => {
                        data.status().0 != tdlib::enums::ChatMemberStatus::Left
                    }
                    model::ChatType::Supergroup(data) => {
                        data.status().0 != tdlib::enums::ChatMemberStatus::Left
                    }
                    _ => false,
                },
            );

            let model = model::ChatHistoryModel::new(chat);

            // A new or deleted message flips its neighbour's grouping; refresh the
            // visible rows so the repeated avatar/name/corners update at once. Older
            // messages load at high positions (off-screen), so skip those bulk appends.
            model.connect_items_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, position, removed, _| {
                    if position == 0 || removed > 0 {
                        obj.refresh_grouping_visuals();
                    }
                }
            ));

            // Request sponsored message, if needed
            let list_view_model: gio::ListModel = if matches!(chat.chat_type(), model::ChatType::Supergroup(supergroup) if supergroup.is_channel())
            {
                let list = gio::ListStore::new::<gio::ListModel>();

                // We need to create a list here so that we can append the sponsored message
                // to the chat history in the GtkListView using a GtkFlattenListModel
                let sponsored_message_list = gio::ListStore::new::<model::SponsoredMessage>();
                list.append(&sponsored_message_list);
                self.request_sponsored_message(chat, &sponsored_message_list);

                list.append(&model);

                gtk::FlattenListModel::new(Some(list)).upcast()
            } else {
                model.clone().upcast()
            };

            let last_read_inbox_message_id = chat.last_read_inbox_message_id();
            let unread_count = chat.unread_count();
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                model,
                async move {
                    let imp = obj.imp();

                    imp.is_loading_messages.set(true);

                    let scrollbar = imp.scrolled_window.vscrollbar();
                    scrollbar.set_visible(false);

                    let adj = imp.list_view.vadjustment().unwrap();
                    adj.set_value(0.0);

                    while adj.value() == 0.0 {
                        match model.load_older_messages(2).await {
                            Ok(can_load_more) => {
                                if !can_load_more {
                                    break;
                                }
                            }
                            Err(e) => {
                                log::warn!("Couldn't load initial history messages: {e}");
                                break;
                            }
                        }
                    }

                    scrollbar.set_visible(true);
                    imp.is_loading_messages.set(false);

                    // If the chat has unread messages, place an "Unread messages" divider
                    // and open at the first unread message instead of pinning to the
                    // bottom. Load older messages until the read/unread boundary is in
                    // range (capped, so a huge backlog doesn't load forever).
                    let mut jumped = false;
                    if unread_count > 0 && last_read_inbox_message_id != 0 {
                        let mut guard = 0;
                        while !model.insert_unread_divider(last_read_inbox_message_id) && guard < 40
                        {
                            match model.load_older_messages(10).await {
                                Ok(true) => guard += 1,
                                _ => break,
                            }
                        }
                        if let Some(position) = obj.unread_divider_position() {
                            obj.set_sticky(false);
                            imp.list_view
                                .scroll_to(position, gtk::ListScrollFlags::empty(), None);
                            jumped = true;
                        }
                    }

                    if !jumped {
                        obj.set_sticky(true);
                    }

                    obj.view_messages();
                }
            ));

            let handler = chat.connect_new_message(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, msg| {
                    if msg.is_outgoing() {
                        obj.imp().background.animate();
                        // Reveal the message the user just sent: stick to the
                        // bottom and scroll down, even if the view wasn't already
                        // pinned there (otherwise an own message lands below the
                        // fold and only shows up after reloading the chat).
                        obj.set_sticky(true);
                        obj.scroll_down();
                    }
                }
            ));
            imp.chat_handler.replace(Some(handler));

            let selection = gtk::NoSelection::new(Some(list_view_model));
            imp.list_view.set_model(Some(&selection));

            imp.model.replace(Some(model));

            perform_chat_action(chat, tdlib::functions::open_chat);
        }

        imp.chat.set(chat);
        self.notify("chat");
    }

    /// The chat history data model backing the list, if a chat is loaded.
    pub(crate) fn history_model(&self) -> Option<model::ChatHistoryModel> {
        self.imp().model.borrow().clone()
    }

    // --- Multi-message selection (long-press / drag to select) ---

    pub(crate) fn is_selection_mode(&self) -> bool {
        self.imp().selection_mode.get()
    }

    /// Enters selection mode (if needed) and adds `message_id` to the selection.
    pub(crate) fn select_message(&self, message_id: i64) {
        let imp = self.imp();
        let newly_entered = !imp.selection_mode.replace(true);
        let inserted = imp.selected_message_ids.borrow_mut().insert(message_id);
        imp.selection_anchor.set(Some(message_id));
        if newly_entered {
            self.guard_selection_relayout();
            imp.selection_bar.set_visible(true);
        }
        if newly_entered || inserted {
            self.update_selection_ui();
            self.refresh_selection_visuals();
        }
    }

    /// Toggles `message_id` in the selection; leaves selection mode once the last
    /// message is deselected.
    pub(crate) fn toggle_message_selection(&self, message_id: i64) {
        {
            let mut ids = self.imp().selected_message_ids.borrow_mut();
            if !ids.insert(message_id) {
                ids.remove(&message_id);
            }
        }
        // The clicked message becomes the anchor for a subsequent shift-click range.
        self.imp().selection_anchor.set(Some(message_id));
        if self.imp().selected_message_ids.borrow().is_empty() {
            self.clear_selection();
        } else {
            self.imp().selection_mode.set(true);
            self.imp().selection_bar.set_visible(true);
            self.update_selection_ui();
            self.refresh_selection_visuals();
        }
    }

    /// Extends the selection to the range between the current anchor and `message_id`
    /// (Shift+click). Replaces the current selection with that contiguous range.
    pub(crate) fn extend_selection_to(&self, message_id: i64) {
        let anchor = self.imp().selection_anchor.get().unwrap_or(message_id);
        self.select_message_range(anchor, message_id);
    }

    pub(crate) fn is_message_selected(&self, message_id: i64) -> bool {
        self.imp()
            .selected_message_ids
            .borrow()
            .contains(&message_id)
    }

    pub(crate) fn clear_selection(&self) {
        self.stop_selection_autoscroll();
        if let Some(source) = self.imp().selection_resweep_source.borrow_mut().take() {
            source.remove();
        }
        // Leaving selection mode hides the checkboxes/toolbar — a geometry change that
        // must not re-pin the view to the bottom.
        self.guard_selection_relayout();
        let imp = self.imp();
        imp.selection_mode.set(false);
        imp.selection_dragging.set(false);
        imp.selection_anchor.set(None);
        imp.selected_message_ids.borrow_mut().clear();
        imp.selection_bar.set_visible(false);
        self.refresh_selection_visuals();
    }

    /// Pins the scroll position across a selection-mode geometry change (checkboxes and
    /// toolbar appearing/disappearing). The list re-measures over several frames, so a
    /// one-shot guard misses the late `upper` change that re-pins to the bottom; instead
    /// the value-changed / upper-notify handlers restore `preserved_scroll_value` for the
    /// whole window (skipping the pin while a drag is actively scrolling).
    fn guard_selection_relayout(&self) {
        let imp = self.imp();
        if let Some(adj) = self.imp().list_view.vadjustment() {
            imp.preserved_scroll_value.set(adj.value());
        }
        imp.selection_transitioning.set(true);
        if let Some(source) = imp.selection_transition_source.borrow_mut().take() {
            source.remove();
        }
        let source = glib::timeout_add_local_once(
            std::time::Duration::from_millis(250),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    obj.imp().selection_transition_source.borrow_mut().take();
                    obj.imp().selection_transitioning.set(false);
                }
            ),
        );
        imp.selection_transition_source.replace(Some(source));
    }

    /// Restores the pinned scroll value during a selection-mode transition (unless a drag
    /// is actively scrolling), so the relayout can't drift the view. Returns whether the
    /// caller should skip its normal sticky/scroll handling.
    fn pin_scroll_during_transition(&self) -> bool {
        let imp = self.imp();
        if !imp.selection_transitioning.get() {
            return false;
        }
        if !imp.selection_dragging.get()
            && let Some(adj) = imp.list_view.vadjustment()
        {
            let max = (adj.upper() - adj.page_size()).max(adj.lower());
            let target = imp.preserved_scroll_value.get().clamp(adj.lower(), max);
            if (adj.value() - target).abs() > 0.5 {
                adj.set_value(target);
            }
        }
        true
    }

    /// Begins a selection drag: the next sweep anchors here and the selection becomes
    /// the range from the anchor to the pointer.
    pub(crate) fn begin_selection_drag(&self) {
        let imp = self.imp();
        imp.selection_dragging.set(true);
        imp.selection_anchor.set(None);
    }

    /// Whether the point (in `list_view` coordinates) lands on a selectable label, i.e. a
    /// bubble's text — where a drag should select text rather than sweep message rows.
    fn point_over_selectable_text(&self, list_view: &gtk::ListView, x: f64, y: f64) -> bool {
        let mut widget = list_view.pick(x, y, gtk::PickFlags::DEFAULT);
        while let Some(w) = widget {
            if w.downcast_ref::<gtk::Label>()
                .is_some_and(|label| label.is_selectable())
            {
                return true;
            }
            widget = w.parent();
        }
        false
    }

    /// Ends an in-progress selection drag (releases the edge auto-scroll).
    pub(crate) fn end_selection_drag(&self) {
        self.imp().selection_dragging.set(false);
        self.stop_selection_autoscroll();
        if let Some(source) = self.imp().selection_resweep_source.borrow_mut().take() {
            source.remove();
        }
    }

    /// Extends the selection to cover every message between the drag anchor (the message
    /// under `ax`/`ay`) and the pointer (`cx`/`cy`), all in `source`'s coordinate space.
    pub(crate) fn sweep_selection(
        &self,
        source: &impl IsA<gtk::Widget>,
        ax: f64,
        ay: f64,
        cx: f64,
        cy: f64,
    ) {
        // Anchor to the message the drag started on (once per drag).
        if self.imp().selection_anchor.get().is_none()
            && let Some(point) =
                source.compute_point(self, &graphene::Point::new(ax as f32, ay as f32))
            && let Some(id) = self.message_id_at_point(point.x() as f64, point.y() as f64)
        {
            self.imp().selection_anchor.set(Some(id));
        }

        if let Some(point) = source.compute_point(self, &graphene::Point::new(cx as f32, cy as f32))
        {
            self.imp()
                .selection_drag_point
                .set((point.x() as f64, point.y() as f64));
        }
        self.resweep_selection();
    }

    /// Recomputes the drag selection from the stored pointer — invoked on pointer moves
    /// and whenever the list scrolls (wheel or auto-scroll), so scrolled-in messages join.
    fn resweep_selection(&self) {
        let Some(anchor) = self.imp().selection_anchor.get() else {
            return;
        };
        let (hx, hy) = self.imp().selection_drag_point.get();
        // Clamp the pick point into the message list so a pointer over the scrollbar or
        // up in the header still extends the range to the message at the nearest edge.
        let (px, py) = self.clamp_point_to_list(hx, hy);
        if let Some(current) = self.message_id_at_point(px, py) {
            self.select_message_range(anchor, current);
        }
        self.update_selection_autoscroll(hy);
    }

    /// Re-runs the sweep after the list re-lays-out. `pick()` reflects the pre-scroll
    /// layout immediately after a scroll changes the adjustment, so a synchronous
    /// resweep keeps hitting the same message; idle runs after the frame clock's layout,
    /// so `pick()` then sees the rows in their new positions. Debounced to one pending.
    fn schedule_deferred_resweep(&self) {
        if self.imp().selection_resweep_source.borrow().is_some() {
            return;
        }
        let source = glib::idle_add_local_once(clone!(
            #[weak(rename_to = obj)]
            self,
            move || {
                obj.imp().selection_resweep_source.borrow_mut().take();
                if obj.imp().selection_dragging.get() {
                    obj.resweep_selection();
                }
            }
        ));
        self.imp().selection_resweep_source.replace(Some(source));
    }

    fn clamp_point_to_list(&self, hx: f64, hy: f64) -> (f64, f64) {
        let Some(bounds) = self.imp().scrolled_window.compute_bounds(self) else {
            return (hx, hy);
        };
        let (left, right) = (
            bounds.x() as f64 + 8.0,
            (bounds.x() + bounds.width()) as f64 - 8.0,
        );
        let (top, bottom) = (
            bounds.y() as f64 + 4.0,
            (bounds.y() + bounds.height()) as f64 - 4.0,
        );
        (hx.clamp(left, right), hy.clamp(top, bottom))
    }

    fn message_id_at_point(&self, hx: f64, hy: f64) -> Option<i64> {
        self.pick(hx, hy, gtk::PickFlags::DEFAULT)
            .and_then(|picked| utils::ancestor::<_, ui::MessageRow>(&picked))
            .and_then(|row| row.message().downcast::<model::Message>().ok())
            .map(|message| message.id())
    }

    /// Sets the selection to exactly the loaded messages between the ids `a` and `b`
    /// (inclusive). Messages are ordered by id, so the range is every loaded message
    /// whose id falls between them.
    fn select_message_range(&self, a: i64, b: i64) {
        let (lo, hi) = (a.min(b), a.max(b));
        let Some(model) = self.history_model() else {
            return;
        };
        let mut ids = HashSet::new();
        for position in 0..model.n_items() {
            if let Some(item) = model
                .item(position)
                .and_downcast::<model::ChatHistoryItem>()
                && let Some(message) = item.message()
                && (lo..=hi).contains(&message.id())
            {
                ids.insert(message.id());
            }
        }
        let imp = self.imp();
        // Guard the first sweep that enters selection mode (checkboxes first appear), but
        // not during an active drag — the drag legitimately scrolls, and the guard would
        // suppress its load-more. Later sweeps just change which rows are checked.
        if !imp.selection_mode.replace(true) && !imp.selection_dragging.get() {
            self.guard_selection_relayout();
        }
        imp.selection_bar.set_visible(true);
        *imp.selected_message_ids.borrow_mut() = ids;
        self.update_selection_ui();
        self.refresh_selection_visuals();
    }

    /// The auto-scroll direction for a selection drag whose pointer is at height `hy`:
    /// -1 near the top edge (or above the list), 1 near the bottom edge (or below), 0
    /// in between.
    fn selection_autoscroll_direction(&self, hy: f64) -> f64 {
        const EDGE: f64 = 40.0;
        let Some(bounds) = self.imp().scrolled_window.compute_bounds(self) else {
            return 0.0;
        };
        if hy < bounds.y() as f64 + EDGE {
            -1.0
        } else if hy > (bounds.y() + bounds.height()) as f64 - EDGE {
            1.0
        } else {
            0.0
        }
    }

    /// While sweeping a selection, scrolls the history when the pointer nears the top or
    /// bottom edge so the drag can select messages beyond the visible area. The scroll
    /// re-triggers `resweep_selection` via the adjustment's value-changed handler.
    fn update_selection_autoscroll(&self, hy: f64) {
        let direction = self.selection_autoscroll_direction(hy);
        if !self.imp().selection_dragging.get() || direction == 0.0 {
            self.stop_selection_autoscroll();
            return;
        }
        if self.imp().selection_autoscroll.borrow().is_some() {
            return;
        }

        let source = glib::timeout_add_local(
            std::time::Duration::from_millis(16),
            clone!(
                #[weak(rename_to = obj)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    let (_, hy) = obj.imp().selection_drag_point.get();
                    let direction = obj.selection_autoscroll_direction(hy);
                    if !obj.imp().selection_dragging.get() || direction == 0.0 {
                        obj.imp().selection_autoscroll.borrow_mut().take();
                        return glib::ControlFlow::Break;
                    }
                    let adj = obj.imp().list_view.vadjustment().unwrap();
                    let max = (adj.upper() - adj.page_size()).max(adj.lower());
                    adj.set_value((adj.value() + direction * 24.0).clamp(adj.lower(), max));
                    glib::ControlFlow::Continue
                }
            ),
        );
        self.imp().selection_autoscroll.replace(Some(source));
    }

    fn stop_selection_autoscroll(&self) {
        if let Some(source) = self.imp().selection_autoscroll.borrow_mut().take() {
            source.remove();
        }
    }

    fn selected_ids(&self) -> Vec<i64> {
        let mut ids: Vec<i64> = self
            .imp()
            .selected_message_ids
            .borrow()
            .iter()
            .copied()
            .collect();
        // TDLib's forward_messages/delete_messages require strictly increasing ids.
        ids.sort_unstable();
        ids
    }

    fn update_selection_ui(&self) {
        let count = self.imp().selected_message_ids.borrow().len();
        self.imp()
            .selection_count_label
            .set_label(&gettext("{n} selected").replace("{n}", &count.to_string()));
        self.action_set_enabled("chat-history.selection-forward", count > 0);
        self.action_set_enabled("chat-history.selection-delete", count > 0);
    }

    fn refresh_selection_visuals(&self) {
        fn walk(widget: &gtk::Widget) {
            let mut child = widget.first_child();
            while let Some(widget) = child {
                if let Some(row) = widget.downcast_ref::<ui::ChatHistoryRow>() {
                    row.update_selection_visual();
                }
                walk(&widget);
                child = widget.next_sibling();
            }
        }
        walk(self.imp().list_view.upcast_ref());
    }

    /// Recomputes same-sender grouping for every visible row. Called when the model
    /// changes, so a neighbour whose grouping flipped (e.g. a new message arrives from
    /// the same sender, or one is deleted) updates its avatar/name/corners immediately
    /// instead of only when it is scrolled off and rebound.
    pub(crate) fn refresh_grouping_visuals(&self) {
        fn walk(widget: &gtk::Widget) {
            let mut child = widget.first_child();
            while let Some(widget) = child {
                if let Some(row) = widget.downcast_ref::<ui::ChatHistoryRow>() {
                    row.apply_message_grouping();
                }
                walk(&widget);
                child = widget.next_sibling();
            }
        }
        walk(self.imp().list_view.upcast_ref());
    }

    fn forward_selection(&self) {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        let Some(chat) = self.chat() else {
            return;
        };
        let Some(session) = utils::ancestor::<_, ui::Session>(self) else {
            return;
        };
        let parent = self.root().and_downcast::<gtk::Window>();
        ui::ForwardWindow::new(parent.as_ref(), &session, chat.id(), ids).present();
        self.clear_selection();
    }

    async fn delete_selection(&self) {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return;
        }
        let Some(chat) = self.chat() else {
            return;
        };
        let dialog = ui::destructive_confirmation_dialog(
            &gettext("Delete Messages"),
            &gettext("Delete the selected messages?"),
            ("no", &gettext("_Cancel")),
            ("yes", &gettext("_Delete")),
        );
        if dialog.choose_future(Some(self)).await == "yes" {
            let client_id = chat.session_().client_().id();
            if let Err(e) =
                tdlib::functions::delete_messages(chat.id(), ids, false, client_id).await
            {
                log::warn!("Failed to delete selected messages: {e:?}");
            }
            self.clear_selection();
        }
    }

    /// Returns the list position of the "Unread messages" divider, if present.
    fn unread_divider_position(&self) -> Option<u32> {
        let model = self.imp().list_view.model()?;
        (0..model.n_items()).find(|pos| {
            model
                .item(*pos)
                .and_downcast::<model::ChatHistoryItem>()
                .is_some_and(|item| {
                    matches!(item.type_(), model::ChatHistoryItemType::UnreadDivider(_))
                })
        })
    }

    /// The position of `message_id` in the loaded model, if present.
    fn message_position(&self, message_id: i64) -> Option<u32> {
        let model = self.imp().list_view.model()?;
        (0..model.n_items()).find(|pos| {
            model
                .item(*pos)
                .and_downcast::<model::ChatHistoryItem>()
                .and_then(|item| item.message().map(|message| message.id()))
                == Some(message_id)
        })
    }

    /// Scrolls the history to the pinned message, if it is currently loaded.
    fn scroll_to_pinned(&self) {
        let message_id = self.imp().pinned_message_id.get();
        match self.message_position(message_id) {
            Some(position) => {
                self.imp()
                    .list_view
                    .scroll_to(position, gtk::ListScrollFlags::FOCUS, None)
            }
            None => log::debug!("Pinned message {message_id} is not in the loaded history"),
        }
    }

    /// Scrolls to `message_id` (if loaded), centres it in the view, and briefly flashes
    /// it — used when following a reply, jump, or link so the user can spot the target.
    /// Morphs the just-sent text out of the composer and up into the history: a ghost that
    /// starts as the grey composer field springs upward, narrows to a bubble, and tints to
    /// the accent colour — so the message appears to fly into its place. `start` is the
    /// composer's rectangle in this widget's coordinates. The real message arrives
    /// separately via the new-message handler.
    pub(crate) fn play_send_animation(&self, text: &str, start: Option<graphene::Rect>) {
        let text = text.trim();
        let Some(start) = start.filter(|_| !text.is_empty()) else {
            return;
        };
        let imp = self.imp();

        // Two stacked layers with the same text: a grey "field" underneath and an accent
        // "bubble" on top whose opacity is animated in, cross-fading grey → blue.
        let make_label = |class: &str| {
            let label = gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            layer.set_hexpand(true);
            layer.set_vexpand(true);
            layer.add_css_class(class);
            layer.append(&label);
            layer
        };
        let base = make_label("send-ghost-base");
        let accent = make_label("send-ghost-accent");
        accent.set_opacity(0.0);

        let ghost = gtk::Overlay::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .can_target(false)
            .child(&base)
            .build();
        ghost.add_overlay(&accent);
        ghost.set_margin_start(start.x() as i32);
        ghost.set_margin_top(start.y() as i32);
        ghost.set_size_request(start.width() as i32, start.height() as i32);
        imp.send_overlay.add_overlay(&ghost);

        // Target the real bubble's place: a text-width bubble at the bottom-right of the
        // (clamped, max-800) message column, just above the composer — where the sent
        // message actually lands.
        let overlay_width = imp.send_overlay.width() as f32;
        let text_natural = gtk::Label::new(Some(text))
            .measure(gtk::Orientation::Horizontal, -1)
            .1 as f32;
        let (content_right, list_bottom) =
            match imp.scrolled_window.compute_bounds(&*imp.send_overlay) {
                Some(bounds) => {
                    let content_width = bounds.width().min(800.0);
                    let content_right = bounds.x() + (bounds.width() + content_width) / 2.0;
                    // Leave the row's small trailing margin, matching an outgoing bubble.
                    (content_right - 8.0, bounds.y() + bounds.height())
                }
                None => (overlay_width - 12.0, start.y()),
            };
        let end_height = start.height();
        let max_width = (content_right - 12.0).max(120.0);
        // The ghost's two layers add 24px of horizontal padding around the text.
        let end_width = (text_natural + 24.0).clamp(96.0, max_width);
        let end_x = (content_right - end_width).max(12.0);
        let end_y = (list_bottom - end_height - 6.0).max(6.0);

        let target = adw::CallbackAnimationTarget::new(clone!(
            #[weak]
            ghost,
            #[weak]
            accent,
            move |value| {
                let progress = value as f32;
                let lerp = |a: f32, b: f32| a + (b - a) * progress;
                ghost.set_margin_start(lerp(start.x(), end_x) as i32);
                ghost.set_margin_top(lerp(start.y(), end_y) as i32);
                ghost.set_size_request(
                    lerp(start.width(), end_width) as i32,
                    lerp(start.height(), end_height) as i32,
                );
                accent.set_opacity(value.clamp(0.0, 1.0));
            }
        ));
        // A springy, macOS-like motion: underdamped so it settles with a soft bounce.
        let params = adw::SpringParams::new(0.45, 3.3, 100.0);
        let animation = adw::SpringAnimation::new(&ghost, 0.0, 1.0, params, target);
        animation.set_initial_velocity(10.0);
        animation.connect_done(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            ghost,
            move |_| {
                obj.imp().send_overlay.remove_overlay(&ghost);
                obj.imp().send_animation.replace(None);
            }
        ));
        animation.play();
        imp.send_animation.replace(Some(animation));
    }

    pub(crate) fn focus_message(&self, message_id: i64) {
        let Some(position) = self.message_position(message_id) else {
            return;
        };
        self.set_sticky(false);
        self.imp()
            .list_view
            .scroll_to(position, gtk::ListScrollFlags::empty(), None);
        // Centre + flash once the row has been realised at its new position.
        glib::idle_add_local_once(clone!(
            #[weak(rename_to = obj)]
            self,
            move || obj.centre_and_flash_message(message_id)
        ));
    }

    fn centre_and_flash_message(&self, message_id: i64) {
        let Some(row) = self.find_message_row(message_id) else {
            return;
        };
        let imp = self.imp();
        if let Some(bounds) = row.compute_bounds(&*imp.scrolled_window) {
            let adj = imp.list_view.vadjustment().unwrap();
            let viewport = imp.scrolled_window.height() as f64;
            let target =
                adj.value() + bounds.y() as f64 - (viewport - bounds.height() as f64) / 2.0;
            let max = (adj.upper() - adj.page_size()).max(adj.lower());
            adj.set_value(target.clamp(adj.lower(), max));
        }
        // Flash the inner message row, not the outer ChatHistoryRow: the group-start top
        // margin lives inside the outer row's box, so flashing it would paint empty space
        // above the bubble with none below. The inner row's box hugs the content evenly.
        let flash_target = row.child().unwrap_or_else(|| row.clone().upcast());
        flash_target.add_css_class("message-flash");
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(1500),
            clone!(
                #[weak]
                flash_target,
                move || flash_target.remove_css_class("message-flash")
            ),
        );
    }

    /// The realised `ChatHistoryRow` currently displaying `message_id`, if any.
    fn find_message_row(&self, message_id: i64) -> Option<ui::ChatHistoryRow> {
        fn walk(widget: &gtk::Widget, message_id: i64) -> Option<ui::ChatHistoryRow> {
            let mut child = widget.first_child();
            while let Some(widget) = child {
                if let Some(row) = widget.downcast_ref::<ui::ChatHistoryRow>()
                    && row
                        .item()
                        .and_downcast::<model::ChatHistoryItem>()
                        .and_then(|item| item.message().map(|message| message.id()))
                        == Some(message_id)
                {
                    return Some(row.clone());
                }
                if let Some(found) = walk(&widget, message_id) {
                    return Some(found);
                }
                child = widget.next_sibling();
            }
            None
        }
        walk(self.imp().list_view.upcast_ref(), message_id)
    }

    /// Unpins the message currently shown in the pinned-message bar.
    fn unpin_message(&self) {
        let imp = self.imp();
        let Some(chat) = self.chat() else {
            return;
        };
        let message_id = imp.pinned_message_id.get();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();

        imp.pinned_message_bar.set_visible(false);

        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::unpin_chat_message(chat_id, message_id, client_id).await
            {
                log::warn!("Failed to unpin a message: {e:?}");
            }
        });
    }

    /// Fetches the chat's pinned message (if any) and shows a preview of it in
    /// the bar below the header. TDLib returns an error when nothing is pinned,
    /// in which case the bar stays hidden.
    fn fetch_pinned_message(&self, chat: &model::Chat) {
        let chat = chat.clone();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Ok(tdlib::enums::Message::Message(td_message)) =
                    tdlib::functions::get_chat_pinned_message(chat_id, client_id).await
                {
                    // Ignore the result if the user has since switched chats.
                    if obj.chat().as_ref() != Some(&chat) {
                        return;
                    }

                    let message = model::Message::new(&chat, td_message);
                    let imp = obj.imp();
                    imp.pinned_message_id.set(message.id());
                    imp.pinned_message_label
                        .set_label(&strings::message_content(&message));
                    imp.pinned_message_bar.set_visible(true);
                }
            }
        ));
    }

    pub(crate) fn sticky(&self) -> bool {
        self.imp().sticky.get()
    }

    fn set_sticky(&self, sticky: bool) {
        if self.sticky() == sticky {
            return;
        }

        self.imp().sticky.set(sticky);
        self.notify("sticky");
    }

    fn scroll_down(&self) {
        let imp = self.imp();

        imp.is_auto_scrolling.set(true);

        imp.scrolled_window
            .emit_by_name::<bool>("scroll-child", &[&gtk::ScrollType::End, &false]);
    }

    /// Jumps to the newest message, pinning the view to the bottom. Used when the user
    /// re-activates the already-open chat in the sidebar.
    pub(crate) fn scroll_to_latest(&self) {
        self.set_sticky(true);
        self.scroll_down();
    }

    pub(crate) fn view_messages(&self) {
        let imp = self.imp();

        if imp.viewed_message_ids_changed.get() {
            imp.viewed_message_ids_changed.set(false);

            let chat = self.chat().unwrap();
            let chat_id = chat.id();
            let client_id = chat.session_().client_().id();
            let viewed_message_ids =
                Vec::from_iter(imp.viewed_message_ids.borrow().iter().copied());

            utils::spawn(async move {
                tdlib::functions::view_messages(chat_id, viewed_message_ids, None, true, client_id)
                    .await
                    .unwrap();
            });
        }
    }
}

fn perform_chat_action<F, Fut>(chat: &model::Chat, op: F)
where
    F: Fn(i64, i32) -> Fut + 'static,
    Fut: Future<Output = Result<(), tdlib::types::Error>>,
{
    utils::spawn(clone!(
        #[weak]
        chat,
        async move {
            op(chat.id(), chat.session_().client_().id()).await.unwrap();
        }
    ));
}
