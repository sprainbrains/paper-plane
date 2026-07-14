use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
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

/// Cover-crops `texture` into a `size`×`size` paintable (centred, aspect-filling),
/// so a fixed-size preview can't inherit the texture's natural dimensions.
fn cover_paintable(texture: &gdk::Texture, size: f32) -> Option<gdk::Paintable> {
    let (tw, th) = (texture.width() as f32, texture.height() as f32);
    if tw <= 0.0 || th <= 0.0 {
        return None;
    }
    let snapshot = gtk::Snapshot::new();
    let scale = (size / tw).max(size / th);
    let (w, h) = (tw * scale, th * scale);
    snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, size, size));
    snapshot.translate(&graphene::Point::new((size - w) / 2.0, (size - h) / 2.0));
    texture
        .upcast_ref::<gdk::Paintable>()
        .snapshot(&snapshot, w as f64, h as f64);
    snapshot.pop();
    snapshot.to_paintable(Some(&graphene::Size::new(size, size)))
}

/// The inline mini preview of a message's media, when its content carries one.
fn content_minithumbnail(
    content: &tdlib::enums::MessageContent,
) -> Option<&tdlib::types::Minithumbnail> {
    use tdlib::enums::MessageContent::*;
    match content {
        MessagePhoto(data) => data.photo.minithumbnail.as_ref(),
        MessageVideo(data) => data.video.minithumbnail.as_ref(),
        MessageAnimation(data) => data.animation.minithumbnail.as_ref(),
        MessageDocument(data) => data.document.minithumbnail.as_ref(),
        MessageVideoNote(data) => data.video_note.minithumbnail.as_ref(),
        _ => None,
    }
}

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
        /// The selection as it was when the current drag anchored, and whether this drag
        /// removes (started on a selected message) or adds (started on an unselected
        /// one). Each sweep applies the anchor..pointer range on top of the base, so
        /// earlier selections survive subsequent drags.
        pub(super) selection_drag_base: RefCell<HashSet<i64>>,
        pub(super) selection_drag_removes: Cell<bool>,
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
        pub(super) send_animation: RefCell<Option<adw::Animation>>,
        pub(super) send_poll: RefCell<Option<gtk::TickCallbackId>>,
        pub(super) send_ghost: RefCell<Option<gtk::Overlay>>,
        pub(super) send_flight_row: RefCell<Option<ui::ChatHistoryRow>>,
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
        pub(super) pinned_title_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) pinned_dots: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) pinned_thumbnail: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) unpin_button: TemplateChild<gtk::Button>,
        /// Every pinned message of the current chat, newest first, and which one the
        /// pinned bar currently shows. Clicking the bar reveals the shown message and
        /// advances to the next older one, cycling through all of them.
        pub(super) pinned_messages: RefCell<Vec<model::Message>>,
        pub(super) pinned_index: Cell<usize>,
        #[template_child]
        pub(super) pinned_view: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) pinned_view_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) pinned_list_box: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub(super) toast_overlay: TemplateChild<adw::ToastOverlay>,
        /// Chats whose pinned-message panel the user hid (session-local, per chat).
        pub(super) hidden_pinned_chats: RefCell<HashSet<i64>>,
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

            klass.install_action("chat-history.hide-pinned", None, |widget, _, _| {
                widget.hide_pinned_bar();
            });
            klass.install_action("chat-history.pinned-list", None, |widget, _, _| {
                widget.show_pinned_list();
            });
            klass.install_action("chat-history.pinned-list-close", None, |widget, _, _| {
                widget.imp().pinned_view.set_visible(false);
            });
            klass.install_action("chat-history.show-pinned", None, |widget, _, _| {
                widget.show_pinned_bar();
            });

            klass.install_action("chat-history.selection-clear", None, |widget, _, _| {
                widget.clear_selection();
            });
            klass.install_action("chat-history.selection-forward", None, |widget, _, _| {
                widget.forward_selection();
            });
            klass.install_action("chat-history.selection-copy", None, |widget, _, _| {
                widget.copy_selection();
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

                    obj.update_pinned_for_scroll();

                    let newest_loaded = imp
                        .model
                        .borrow()
                        .as_ref()
                        .is_none_or(|model| model.newest_loaded());

                    if imp.is_auto_scrolling.get() {
                        if adj.value() + adj.page_size() >= adj.upper() {
                            imp.is_auto_scrolling.set(false);
                            obj.set_sticky(newest_loaded);
                        }
                    } else {
                        // The bottom of a jumped-to window isn't the bottom of the chat.
                        obj.set_sticky(
                            newest_loaded && adj.value() + adj.page_size() >= adj.upper(),
                        );

                        // Near the bottom of a jumped-to window: pull newer messages in
                        // to close the gap towards the present.
                        if !newest_loaded
                            && adj.value() + adj.page_size() * 2.0 >= adj.upper()
                            && let Some(model) = imp.model.borrow().as_ref()
                        {
                            imp.is_loading_messages.set(true);
                            utils::spawn(clone!(
                                #[weak]
                                obj,
                                #[weak]
                                model,
                                async move {
                                    obj.imp().is_loading_messages.set(false);
                                    if let Err(model::ChatHistoryError::Tdlib(e)) =
                                        model.load_newer_messages(30).await
                                    {
                                        log::warn!("Couldn't load newer chat messages: {e:?}");
                                    }
                                }
                            ));
                            return;
                        }

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

        // A ghost from the previous chat must not float over the new one.
        self.cancel_send_animation();

        // Reset the pinned-message bar; it's re-populated below if the new chat
        // has a pinned message.
        imp.pinned_message_bar.set_visible(false);
        imp.pinned_messages.borrow_mut().clear();
        imp.pinned_index.set(0);
        imp.pinned_view.set_visible(false);

        if let Some(chat) = old_chat {
            chat.disconnect(imp.chat_handler.replace(None).unwrap());
            perform_chat_action(chat.as_ref(), tdlib::functions::close_chat);
        }
        self.action_set_enabled(
            "chat-history.show-pinned",
            chat.is_some_and(|chat| imp.hidden_pinned_chats.borrow().contains(&chat.id())),
        );

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
        // Anchor to the message the drag started on (once per drag), snapshotting the
        // selection it works on top of: starting on a selected message makes this a
        // removing drag, otherwise an adding one.
        if self.imp().selection_anchor.get().is_none()
            && let Some(point) =
                source.compute_point(self, &graphene::Point::new(ax as f32, ay as f32))
            && let Some(id) = self.message_id_at_point(point.x() as f64, point.y() as f64)
        {
            let imp = self.imp();
            imp.selection_anchor.set(Some(id));
            let base = imp.selected_message_ids.borrow().clone();
            imp.selection_drag_removes.set(base.contains(&id));
            imp.selection_drag_base.replace(base);
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
            self.apply_drag_range(anchor, current);
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

    /// Every loaded message id between `a` and `b` (inclusive). Messages are ordered by
    /// id, so the range is every loaded message whose id falls between them.
    fn message_ids_between(&self, a: i64, b: i64) -> HashSet<i64> {
        let (lo, hi) = (a.min(b), a.max(b));
        let mut ids = HashSet::new();
        let Some(model) = self.history_model() else {
            return ids;
        };
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
        ids
    }

    /// Sets the selection to exactly the messages between the ids `a` and `b`
    /// (inclusive). Used by shift-click range extension.
    fn select_message_range(&self, a: i64, b: i64) {
        let ids = self.message_ids_between(a, b);
        self.set_selection(ids);
    }

    /// Applies the drag's anchor..pointer range on top of the selection snapshotted at
    /// the anchor: an adding drag unions the range in, a removing drag subtracts it —
    /// so re-sweeping (including backing the pointer off) live-previews the result
    /// without ever resetting what was selected before the drag.
    fn apply_drag_range(&self, anchor: i64, current: i64) {
        let range = self.message_ids_between(anchor, current);
        let imp = self.imp();
        let base = imp.selection_drag_base.borrow();
        let ids: HashSet<i64> = if imp.selection_drag_removes.get() {
            base.difference(&range).copied().collect()
        } else {
            base.union(&range).copied().collect()
        };
        drop(base);
        self.set_selection(ids);
    }

    fn set_selection(&self, ids: HashSet<i64>) {
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
        self.action_set_enabled("chat-history.selection-copy", count > 0);
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

    /// Copies the selected messages' text to the clipboard, oldest first, each prefixed
    /// with a `[sender dd/mm/yy hh:mm]:` header line and separated by blank lines.
    fn copy_selection(&self) {
        let ids = self.selected_ids();
        let Some(chat) = self.chat() else {
            return;
        };
        let blocks: Vec<String> = ids
            .into_iter()
            .filter_map(|id| chat.message(id))
            .map(|message| {
                let sender = strings::message_sender(&message.sender(), true);
                let when = glib::DateTime::from_unix_local(message.date() as i64)
                    .ok()
                    .and_then(|datetime| datetime.format("%d/%m/%y %H:%M").ok())
                    .unwrap_or_default();
                let text = match message.content().0 {
                    tdlib::enums::MessageContent::MessageText(content) => content.text.text,
                    _ => strings::message_content(&message),
                };
                format!("[{sender} {when}]:\n{text}")
            })
            .collect();
        if blocks.is_empty() {
            return;
        }
        self.clipboard().set_text(&blocks.join("\n\n"));
        self.clear_selection();
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

    /// Reveals the pinned message the bar currently shows, then advances the bar to the
    /// next older pinned message (wrapping), so repeated clicks walk through all of them.
    fn scroll_to_pinned(&self) {
        let imp = self.imp();
        let message_id = {
            let messages = imp.pinned_messages.borrow();
            if messages.is_empty() {
                return;
            }
            let index = imp.pinned_index.get().min(messages.len() - 1);
            messages[index].id()
        };
        self.reveal_message(message_id);
        // No manual index advance: the scroll-follow (update_pinned_for_scroll) lands
        // on the revealed message and switches the bar to the next older pinned one.
    }

    /// Keeps the pinned bar in sync with the scroll position: it always shows the
    /// newest pinned message *above* the viewport centre — scrolling up past a pinned
    /// message advances the bar to the previous (older) one and vice versa, even when
    /// several pinned messages sit close together.
    fn update_pinned_for_scroll(&self) {
        let imp = self.imp();
        let count = imp.pinned_messages.borrow().len();
        if count < 2 {
            return;
        }
        let Some(bounds) = imp.scrolled_window.compute_bounds(self) else {
            return;
        };
        let centre_id = self.message_id_at_point(
            (bounds.x() + bounds.width() / 2.0) as f64,
            (bounds.y() + bounds.height() / 2.0) as f64,
        );
        let Some(centre_id) = centre_id else {
            return;
        };
        let index = {
            let messages = imp.pinned_messages.borrow();
            messages
                .iter()
                .position(|message| message.id() < centre_id)
                .unwrap_or(count - 1)
        };
        if index != imp.pinned_index.get() {
            imp.pinned_index.set(index);
            self.update_pinned_bar();
        }
    }

    /// Scrolls to `message_id` (if loaded), centres it in the view, and briefly flashes
    /// it — used when following a reply, jump, or link so the user can spot the target.
    /// Animates a just-sent message in two phases. Phase 1 is fully deterministic: a ghost
    /// that starts as the grey composer field springs up towards a *staging point* (bottom
    /// of the message column, slightly left of where outgoing bubbles sit), cross-fading
    /// grey → accent and narrowing to the text's bubble size. Meanwhile a per-frame poll
    /// keeps the real message row hidden. Phase 2 begins the moment the real bubble is laid
    /// out (mid-morph is fine): the ghost springs from wherever it currently is onto the
    /// real bubble, re-reading the bubble's live bounds every frame so a settling
    /// auto-scroll can't stale the target, and hands off to the real row when it lands.
    /// Staying in the overlay for the whole trip means the ghost is never clipped at the
    /// composer edge the way rows are.
    /// `start` is the composer's rectangle in this widget's coordinates.
    pub(crate) fn play_send_animation(
        &self,
        text: &str,
        start: Option<graphene::Rect>,
        message_id: i64,
    ) {
        let text = text.trim();
        let Some(start) = start.filter(|_| !text.is_empty()) else {
            return;
        };
        // Respect the system-wide "disable animations" preference.
        if !gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations()) {
            return;
        }
        self.cancel_send_animation();
        let imp = self.imp();

        let staging = self.staging_rect(&start, text);

        // Two stacked layers with the same text: a grey "field" underneath and an accent
        // "bubble" on top whose opacity is animated in, cross-fading grey → accent.
        // Single line, ellipsized: a wrapping label reports a tiny natural width and
        // reflows into a tall multi-line block at the ghost's animated widths.
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
        imp.send_ghost.replace(Some(ghost.clone()));

        // Phase 1: morph the ghost onto the staging rect. The colour cross-fade completes
        // by half-way so an early conversion doesn't pop from grey to accent.
        let target = adw::CallbackAnimationTarget::new(clone!(
            #[weak]
            ghost,
            #[weak]
            accent,
            move |value| {
                let progress = value as f32;
                let lerp = |a: f32, b: f32| a + (b - a) * progress;
                ghost.set_margin_start(lerp(start.x(), staging.x()) as i32);
                ghost.set_margin_top(lerp(start.y(), staging.y()) as i32);
                ghost.set_size_request(
                    lerp(start.width(), staging.width()) as i32,
                    lerp(start.height(), staging.height()) as i32,
                );
                accent.set_opacity((value * 2.0).clamp(0.0, 1.0));
            }
        ));
        let animation = adw::SpringAnimation::new(&ghost, 0.0, 1.0, Self::send_spring(), target);
        animation.set_initial_velocity(0.5);
        animation.play();
        imp.send_animation.replace(Some(animation.clone().upcast()));

        // Poll every frame: keep the real row hidden from the moment it is realised (it can
        // be re-created under us when TDLib swaps the optimistic id, so re-verify each
        // frame), then convert the ghost into the real bubble as soon as the bubble has an
        // allocation and the colour cross-fade is done — the flight continues from wherever
        // the ghost currently is, no waiting for the morph to settle. Bail out with a fade
        // if the row never shows up (send failed, chat switched, …).
        let text = text.to_owned();
        let first_frame_time = Rc::new(Cell::new(0i64));
        let prev_bounds: Rc<Cell<Option<(f32, f32, f32, f32)>>> = Rc::new(Cell::new(None));
        let id = self.add_tick_callback(move |obj, clock| {
            let imp = obj.imp();
            let now = clock.frame_time();
            if first_frame_time.get() == 0 {
                first_frame_time.set(now);
            }

            // Keep the sent row hidden while the ghost stands in for it. The hand-off
            // waits until the bubble's bounds are *stable across two frames and inside
            // the viewport*: right after a send, the ListView's height estimation and
            // the sticky scroll move the row over several frames, and gliding towards
            // those transient positions sent the ghost chasing below the screen.
            let row = obj.acquire_sent_row(message_id, &text);
            let near_staging =
                animation.value() >= 0.8 || animation.state() != adw::AnimationState::Playing;
            let bounds = row
                .as_ref()
                .and_then(|row| utils::descendant::<_, ui::MessageBubble>(row))
                .and_then(|bubble| bubble.compute_bounds(&*imp.send_overlay))
                .filter(|bounds| bounds.width() > 1.0)
                .filter(|bounds| {
                    imp.scrolled_window
                        .compute_bounds(&*imp.send_overlay)
                        .is_some_and(|viewport| {
                            bounds.y() >= viewport.y() - 2.0
                                && bounds.y() + bounds.height()
                                    <= viewport.y() + viewport.height() + 2.0
                        })
                });
            let key = bounds
                .as_ref()
                .map(|rect| (rect.x(), rect.y(), rect.width(), rect.height()));
            let stable = key.is_some() && prev_bounds.get() == key;
            prev_bounds.set(key);

            if near_staging
                && stable
                && let Some(bounds) = bounds
            {
                imp.send_poll.take();
                obj.glide_send_ghost(bounds);
                return glib::ControlFlow::Break;
            }

            if now - first_frame_time.get() > 1_200_000 {
                imp.send_poll.take();
                obj.abort_send_ghost();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        imp.send_poll.replace(Some(id));
    }

    /// (Re-)acquires the just-sent message's realised row and keeps it hidden: the widget
    /// hidden earlier may have been recycled for a different item, or the real row may
    /// only appear (or be re-created by TDLib's id swap) later. Any previously hidden
    /// widget that no longer shows the message gets its opacity back.
    fn acquire_sent_row(&self, message_id: i64, text: &str) -> Option<ui::ChatHistoryRow> {
        let imp = self.imp();
        let row = self.find_sent_row(message_id, text);
        let stored = imp.send_flight_row.borrow().clone();
        if stored != row {
            if let Some(stored) = stored {
                stored.set_opacity(1.0);
            }
            imp.send_flight_row.replace(row.clone());
        }
        if let Some(row) = &row {
            row.set_opacity(0.0);
        }
        row
    }

    /// Phase 2: a short, decisive glide of the ghost from wherever the morph currently
    /// has it onto the real bubble's (settled) rectangle, then the real row takes over.
    /// The ghost lives in the overlay, so unlike rows it is never clipped by the composer
    /// edge on the way.
    fn glide_send_ghost(&self, bounds: graphene::Rect) {
        let imp = self.imp();
        // Freeze the morph; the glide continues from the ghost's current rectangle.
        if let Some(animation) = imp.send_animation.take() {
            animation.pause();
        }
        let from = imp
            .send_ghost
            .borrow()
            .as_ref()
            .and_then(|ghost| ghost.compute_bounds(&*imp.send_overlay));
        let (Some(ghost), Some(from)) = (imp.send_ghost.borrow().clone(), from) else {
            self.abort_send_ghost();
            return;
        };

        let target = adw::CallbackAnimationTarget::new(clone!(
            #[weak]
            ghost,
            move |value| {
                let progress = value as f32;
                let lerp = |a: f32, b: f32| a + (b - a) * progress;
                let width = lerp(from.width(), bounds.width());
                let height = lerp(from.height(), bounds.height());
                // Right edges aligned: outgoing bubbles are anchored right.
                let right = lerp(from.x() + from.width(), bounds.x() + bounds.width());
                ghost.set_margin_start((right - width) as i32);
                ghost.set_margin_top(lerp(from.y(), bounds.y()) as i32);
                ghost.set_size_request(width as i32, height as i32);
            }
        ));
        let animation = adw::TimedAnimation::new(&ghost, 0.0, 1.0, 200, target);
        animation.set_easing(adw::Easing::EaseOutCubic);
        animation.connect_done(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.crossfade_send_handoff()
        ));
        animation.play();
        imp.send_animation.replace(Some(animation.upcast()));
    }

    /// The last stretch of the hand-off: the ghost has landed on the bubble's rectangle,
    /// but it lacks the timestamp/checkmark and, for multi-line messages, is only a
    /// one-line stand-in — a short crossfade (real row in, ghost out) hides the
    /// difference instead of swapping with a pop.
    fn crossfade_send_handoff(&self) {
        let imp = self.imp();
        let row = imp.send_flight_row.borrow().clone();
        let Some(ghost) = imp.send_ghost.borrow().clone() else {
            if let Some(row) = imp.send_flight_row.take() {
                row.set_opacity(1.0);
            }
            return;
        };
        let target = adw::CallbackAnimationTarget::new(clone!(
            #[weak]
            ghost,
            move |value| {
                ghost.set_opacity(1.0 - value);
                if let Some(row) = &row {
                    row.set_opacity(value);
                }
            }
        ));
        let fade = adw::TimedAnimation::new(&ghost, 0.0, 1.0, 120, target);
        fade.connect_done(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                let imp = obj.imp();
                if let Some(ghost) = imp.send_ghost.take() {
                    imp.send_overlay.remove_overlay(&ghost);
                }
                if let Some(row) = imp.send_flight_row.take() {
                    row.set_opacity(1.0);
                }
                imp.send_animation.replace(None);
            }
        ));
        fade.play();
        imp.send_animation.replace(Some(fade.upcast()));
    }

    /// A springy, macOS-like motion: slightly underdamped, settling quickly with at most
    /// one soft overshoot.
    fn send_spring() -> adw::SpringParams {
        adw::SpringParams::new(0.85, 3.3, 100.0)
    }

    /// Stops any in-flight send animation and restores everything it touched. Called
    /// before starting a new one so rapid sends don't fight over the shared state.
    fn cancel_send_animation(&self) {
        let imp = self.imp();
        if let Some(id) = imp.send_poll.take() {
            id.remove();
        }
        if let Some(animation) = imp.send_animation.take() {
            animation.pause();
        }
        if let Some(ghost) = imp.send_ghost.take() {
            imp.send_overlay.remove_overlay(&ghost);
        }
        if let Some(row) = imp.send_flight_row.take() {
            row.set_opacity(1.0);
        }
    }

    /// The row never appeared (send failed, chat switched): fade the ghost out gracefully
    /// instead of leaving it parked at the staging point.
    fn abort_send_ghost(&self) {
        let imp = self.imp();
        if let Some(row) = imp.send_flight_row.take() {
            row.set_opacity(1.0);
        }
        if let Some(animation) = imp.send_animation.take() {
            animation.pause();
        }
        // The ghost stays registered in `send_ghost` until the fade actually removes it,
        // so that a new send cancelling mid-fade still finds and removes it (taking it
        // out up-front leaked it parked on screen when the paused fade never finished).
        let Some(ghost) = imp.send_ghost.borrow().clone() else {
            return;
        };
        let target = adw::PropertyAnimationTarget::new(&ghost, "opacity");
        let fade = adw::TimedAnimation::new(&ghost, 1.0, 0.0, 150, target);
        fade.connect_done(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                let imp = obj.imp();
                if let Some(ghost) = imp.send_ghost.take() {
                    imp.send_overlay.remove_overlay(&ghost);
                }
                imp.send_animation.replace(None);
            }
        ));
        fade.play();
        imp.send_animation.replace(Some(fade.upcast()));
    }

    /// Where the ghost converts into the real bubble: a text-sized bubble rect at the
    /// bottom of the clamped (max-800) message column, slightly left of the outgoing
    /// bubbles' lane (they sit 38px in from the column edge, see style.css).
    fn staging_rect(&self, start: &graphene::Rect, text: &str) -> graphene::Rect {
        let imp = self.imp();
        let (column_right, list_bottom) =
            match imp.scrolled_window.compute_bounds(&*imp.send_overlay) {
                Some(bounds) => {
                    let content_width = bounds.width().min(800.0);
                    (
                        bounds.x() + (bounds.width() + content_width) / 2.0,
                        bounds.y() + bounds.height(),
                    )
                }
                None => (imp.send_overlay.width() as f32 - 12.0, start.y()),
            };
        let right_edge = column_right - 38.0 - 24.0;

        // Measured unwrapped: the ghost is a single ellipsized line, so the staging rect
        // is its natural one-line size capped at the column.
        let label = gtk::Label::new(Some(text));
        let max_text_width = (right_edge - 12.0 - 24.0).max(72.0);
        let text_width =
            (label.measure(gtk::Orientation::Horizontal, -1).1 as f32).clamp(72.0, max_text_width);
        let text_height = label.measure(gtk::Orientation::Vertical, -1).1 as f32;

        let width = text_width + 24.0;
        let height = text_height + 16.0;
        let x = (right_edge - width).max(12.0);
        let y = (list_bottom - height - 8.0).max(6.0);
        graphene::Rect::new(x, y, width, height)
    }

    /// The realised row of the just-sent message. An id match always wins; failing that
    /// (TDLib swaps the optimistic id for the final one via `updateMessageSendSucceeded`),
    /// the *bottom-most message row overall* is accepted iff it is an outgoing message
    /// with exactly the sent text — never an older duplicate higher up, since the fresh
    /// message can only be the last one in the history.
    fn find_sent_row(&self, message_id: i64, text: &str) -> Option<ui::ChatHistoryRow> {
        let imp = self.imp();
        let mut id_match: Option<ui::ChatHistoryRow> = None;
        // (row, y in list coordinates, whether it looks like the sent message)
        let mut bottom: Option<(ui::ChatHistoryRow, f32, bool)> = None;
        let mut stack = vec![imp.list_view.clone().upcast::<gtk::Widget>()];
        while let Some(widget) = stack.pop() {
            let mut child = widget.first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                let Some(row) = widget.downcast_ref::<ui::ChatHistoryRow>() else {
                    stack.push(widget);
                    continue;
                };
                let Some(item) = row.item().and_downcast::<model::ChatHistoryItem>() else {
                    continue;
                };
                let Some(message) = item.message() else {
                    continue;
                };
                if message.id() == message_id {
                    id_match = Some(row.clone());
                }
                if let Some(bounds) = row.compute_bounds(&*imp.list_view)
                    && bottom.as_ref().is_none_or(|(_, y, _)| bounds.y() > *y)
                {
                    let looks_sent = message.is_outgoing()
                        && matches!(
                            &message.content().0,
                            tdlib::enums::MessageContent::MessageText(content)
                                if content.text.text.trim() == text
                        );
                    bottom = Some((row.clone(), bounds.y(), looks_sent));
                }
            }
        }
        id_match.or_else(|| bottom.and_then(|(row, _, looks_sent)| looks_sent.then_some(row)))
    }

    /// Scrolls `message_id` (if loaded) to the viewport centre and flashes it. Centring
    /// is a per-frame settling loop, not a one-shot: right after a jump the ListView's
    /// row heights are still estimates, so a single correction computed from that
    /// transient layout can land far off — the loop keeps re-centring until the row's
    /// position is stable for a few frames (or a deadline passes), then flashes.
    pub(crate) fn focus_message(&self, message_id: i64) {
        let Some(position) = self.message_position(message_id) else {
            return;
        };
        self.set_sticky(false);
        self.imp()
            .list_view
            .scroll_to(position, gtk::ListScrollFlags::empty(), None);

        let stable_frames = Rc::new(Cell::new(0u32));
        let first_frame_time = Rc::new(Cell::new(0i64));
        self.add_tick_callback(move |obj, clock| {
            let imp = obj.imp();
            let now = clock.frame_time();
            if first_frame_time.get() == 0 {
                first_frame_time.set(now);
            }
            let deadline = now - first_frame_time.get() > 700_000;

            match obj.find_message_row(message_id) {
                Some(row) => {
                    if let Some(bounds) = row.compute_bounds(&*imp.scrolled_window) {
                        let adj = imp.list_view.vadjustment().unwrap();
                        let viewport = imp.scrolled_window.height() as f64;
                        let target = adj.value() + bounds.y() as f64
                            - (viewport - bounds.height() as f64) / 2.0;
                        let max = (adj.upper() - adj.page_size()).max(adj.lower());
                        let target = target.clamp(adj.lower(), max);
                        if (target - adj.value()).abs() > 1.0 {
                            adj.set_value(target);
                            stable_frames.set(0);
                        } else {
                            stable_frames.set(stable_frames.get() + 1);
                        }
                    }
                }
                None => {
                    // Not realised yet — re-request the scroll and keep waiting.
                    if let Some(position) = obj.message_position(message_id) {
                        imp.list_view
                            .scroll_to(position, gtk::ListScrollFlags::empty(), None);
                    }
                    stable_frames.set(0);
                }
            }

            if stable_frames.get() >= 3 || deadline {
                obj.flash_message(message_id);
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn flash_message(&self, message_id: i64) {
        let Some(row) = self.find_message_row(message_id) else {
            return;
        };
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
    /// Hides the pinned-message panel for this chat — locally only, the message stays
    /// pinned — and offers an undo via toast (and the header menu's "Show Pinned Panel").
    fn hide_pinned_bar(&self) {
        let Some(chat) = self.chat() else {
            return;
        };
        let imp = self.imp();
        imp.hidden_pinned_chats.borrow_mut().insert(chat.id());
        imp.pinned_message_bar.set_visible(false);
        self.action_set_enabled("chat-history.show-pinned", true);

        let toast = adw::Toast::builder()
            .title(gettext("Pinned panel was hidden"))
            .button_label(gettext("Show"))
            .build();
        toast.connect_button_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.show_pinned_bar()
        ));
        imp.toast_overlay.add_toast(toast);
    }

    /// Reveals this chat's previously hidden pinned-message panel.
    fn show_pinned_bar(&self) {
        let Some(chat) = self.chat() else {
            return;
        };
        self.imp()
            .hidden_pinned_chats
            .borrow_mut()
            .remove(&chat.id());
        self.action_set_enabled("chat-history.show-pinned", false);
        self.update_pinned_bar();
    }

    /// Unpins (for everyone) the pinned message the bar currently shows.
    fn unpin_message(&self) {
        let imp = self.imp();
        let Some(chat) = self.chat() else {
            return;
        };
        let message_id = {
            let mut messages = imp.pinned_messages.borrow_mut();
            if messages.is_empty() {
                return;
            }
            let index = imp.pinned_index.get().min(messages.len() - 1);
            messages.remove(index).id()
        };
        self.update_pinned_bar();

        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();
        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::unpin_chat_message(chat_id, message_id, client_id).await
            {
                log::warn!("Failed to unpin a message: {e:?}");
            }
        });
    }

    /// Re-fetches the pinned messages after a pin/unpin elsewhere (e.g. the message
    /// context menu) so the bar reflects the change immediately.
    pub(crate) fn refresh_pinned_messages(&self) {
        if let Some(chat) = self.chat() {
            self.fetch_pinned_message(&chat);
        }
    }

    /// Fetches all of the chat's pinned messages (newest first) and shows the newest in
    /// the bar below the header; the bar stays hidden when nothing is pinned.
    fn fetch_pinned_message(&self, chat: &model::Chat) {
        let chat = chat.clone();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Ok(tdlib::enums::FoundChatMessages::FoundChatMessages(found)) =
                    tdlib::functions::search_chat_messages(
                        chat_id,
                        None,
                        String::new(),
                        None,
                        0,
                        0,
                        100,
                        Some(tdlib::enums::SearchMessagesFilter::Pinned),
                        client_id,
                    )
                    .await
                {
                    // Ignore the result if the user has since switched chats.
                    if obj.chat().as_ref() != Some(&chat) {
                        return;
                    }
                    let messages: Vec<model::Message> = found
                        .messages
                        .into_iter()
                        .map(|td_message| model::Message::new(&chat, td_message))
                        .collect();
                    let imp = obj.imp();
                    imp.pinned_messages.replace(messages);
                    imp.pinned_index.set(0);
                    obj.update_pinned_bar();
                }
            }
        ));
    }

    /// Refreshes the pinned bar from `pinned_messages`/`pinned_index`: preview text,
    /// numbered title, media minithumbnail, and the segment stack on the left (one line
    /// per pinned message — the shown one in full accent, the rest dimmed). Hides the
    /// bar when nothing is pinned or the user hid the panel for this chat.
    fn update_pinned_bar(&self) {
        let imp = self.imp();
        let messages = imp.pinned_messages.borrow();
        let count = messages.len();
        let hidden = self
            .chat()
            .is_some_and(|chat| imp.hidden_pinned_chats.borrow().contains(&chat.id()));
        if count == 0 || hidden {
            imp.pinned_message_bar.set_visible(false);
            return;
        }
        let index = imp.pinned_index.get().min(count - 1);
        imp.pinned_index.set(index);
        let message = &messages[index];

        imp.pinned_message_label
            .set_label(&strings::message_content(message));
        imp.pinned_title_label.set_label(&if count > 1 {
            // Pinned messages are numbered oldest = 1, like tdesktop.
            gettext("Pinned Message #{n}").replace("{n}", &(count - index).to_string())
        } else {
            gettext("Pinned Message")
        });

        // Cover-crop the minithumbnail into a fixed square paintable: a raw texture in
        // the GtkPicture would report its own (possibly tall) natural size and grow the
        // whole bar.
        let paintable = content_minithumbnail(&message.content().0)
            .and_then(|minithumbnail| {
                gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
                    &minithumbnail.data,
                )))
                .ok()
            })
            .and_then(|texture| cover_paintable(&texture, 34.0));
        imp.pinned_thumbnail.set_paintable(paintable.as_ref());
        imp.pinned_thumbnail.set_visible(paintable.is_some());

        // Rebuild the segment stack: a window of up to 5 lines around the active one.
        // When more pinned messages continue beyond an edge, that edge's line fades to
        // transparent.
        while let Some(child) = imp.pinned_dots.first_child() {
            child.unparent();
        }
        let window = count.min(5);
        let start = index.saturating_sub(2).min(count - window);
        let height = (26 / window as i32).clamp(4, 12);
        // Top line = oldest, bottom = newest, matching the chat's layout (index 0 is
        // the newest pinned message, so the window is walked in reverse).
        for i in (0..window).rev() {
            let dot = gtk::Box::new(gtk::Orientation::Vertical, 0);
            dot.set_size_request(3, height);
            dot.add_css_class("pinned-dot");
            if start + i == index {
                dot.add_css_class("active");
            }
            if i == window - 1 && start + window < count {
                dot.add_css_class("fade-top");
            }
            if i == 0 && start > 0 {
                dot.add_css_class("fade-bottom");
            }
            imp.pinned_dots.append(&dot);
        }

        imp.pinned_message_bar.set_visible(true);
    }

    /// Shows the in-chat pinned-messages page: the chat area is covered by a list of
    /// only the pinned messages (real bubbles), with a back button on top. Clicking a
    /// message jumps to it in the history.
    fn show_pinned_list(&self) {
        let imp = self.imp();
        let messages = imp.pinned_messages.borrow().clone();
        if messages.is_empty() {
            return;
        }

        imp.pinned_view_title
            .set_label(&gettext("{n} Pinned Messages").replace("{n}", &messages.len().to_string()));

        imp.pinned_list_box.remove_all();
        // Oldest at the top, like the history itself.
        for message in messages.iter().rev() {
            let row = gtk::ListBoxRow::builder()
                .activatable(false)
                .selectable(false)
                .build();
            row.set_child(Some(&ui::MessageRow::new(message.clone().upcast_ref())));

            // A capture-phase click anywhere on the row jumps to the message in the
            // history (the bubbles' own gestures would otherwise swallow the click).
            let click = gtk::GestureClick::new();
            click.set_button(1);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            let message_id = message.id();
            click.connect_released(clone!(
                #[weak(rename_to = obj)]
                self,
                move |gesture, _, _, _| {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    obj.imp().pinned_view.set_visible(false);
                    obj.reveal_message(message_id);
                }
            ));
            row.add_controller(click);
            imp.pinned_list_box.append(&row);
        }
        imp.pinned_view.set_visible(true);
    }

    /// Jumps to `message_id`: if it isn't loaded, the history window is re-anchored
    /// *around* it in a single request (no walking through the whole history), then the
    /// message is centred and flashed. Scrolling back down closes the gap batch-wise.
    pub(crate) fn reveal_message(&self, message_id: i64) {
        if self.message_position(message_id).is_some() {
            self.focus_message(message_id);
            return;
        }
        let Some(model) = self.history_model() else {
            return;
        };
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                for _ in 0..10 {
                    match model.load_around_message(message_id).await {
                        Ok(()) => break,
                        // Most likely a scroll-edge load in flight; retry shortly.
                        Err(model::ChatHistoryError::AlreadyLoading) => {
                            glib::timeout_future(std::time::Duration::from_millis(50)).await;
                        }
                        Err(e) => {
                            log::warn!("Couldn't jump to message {message_id}: {e}");
                            return;
                        }
                    }
                }
                obj.set_sticky(false);
                // Let the list realise the fresh window before scrolling to the target.
                glib::idle_add_local_once(clone!(
                    #[weak]
                    obj,
                    move || obj.focus_message(message_id)
                ));
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

        // In a jumped-to window the bottom of the list isn't the bottom of the chat:
        // re-anchor the history at the newest message first.
        if let Some(model) = imp.model.borrow().as_ref().filter(|m| !m.newest_loaded()) {
            let model = model.clone();
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    if model.reload_to_newest().await.is_ok() {
                        obj.set_sticky(true);
                        glib::idle_add_local_once(clone!(
                            #[weak]
                            obj,
                            move || obj.scroll_down()
                        ));
                    }
                }
            ));
            return;
        }

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
