mod contacts_window;
mod content;
mod preferences_window;
mod row;
mod sidebar;
mod switcher;

use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;

pub(crate) use self::contacts_window::ContactsWindow;
pub(crate) use self::contacts_window::Row as ContactRow;
pub(crate) use self::content::Background;
pub(crate) use self::content::ChatActionBar;
pub(crate) use self::content::ChatHistory;
pub(crate) use self::content::ChatHistoryRow;
pub(crate) use self::content::ChatInfoWindow;
pub(crate) use self::content::Content;
pub(crate) use self::content::ContentPicker;
pub(crate) use self::content::EventRow;
pub(crate) use self::content::ForwardWindow;
pub(crate) use self::content::MediaPicture;
pub(crate) use self::content::MediaViewer;
pub(crate) use self::content::MessageAudio;
pub(crate) use self::content::MessageBase;
pub(crate) use self::content::MessageBaseExt;
pub(crate) use self::content::MessageBaseImpl;
pub(crate) use self::content::MessageBubble;
pub(crate) use self::content::MessageDocument;
pub(crate) use self::content::MessageDocumentStatusIndicator;
pub(crate) use self::content::MessageDragPayload;
pub(crate) use self::content::MessageIndicators;
pub(crate) use self::content::MessageLabel;
pub(crate) use self::content::MessageLocation;
pub(crate) use self::content::MessageMenu;
pub(crate) use self::content::MessageMenuReactions;
pub(crate) use self::content::MessagePhoto;
pub(crate) use self::content::MessagePoll;
pub(crate) use self::content::MessageReactions;
pub(crate) use self::content::MessageReply;
pub(crate) use self::content::MessageRow;
pub(crate) use self::content::MessageSticker;
pub(crate) use self::content::MessageText;
pub(crate) use self::content::MessageVenue;
pub(crate) use self::content::MessageVideo;
pub(crate) use self::content::MessageVideoNote;
pub(crate) use self::content::MessageVoiceNote;
pub(crate) use self::content::SendMediaWindow;
pub(crate) use self::preferences_window::PreferencesWindow;
pub(crate) use self::row::Row;
pub(crate) use self::sidebar::Avatar as SidebarAvatar;
pub(crate) use self::sidebar::ChatFolderBar as SidebarChatFolderBar;
pub(crate) use self::sidebar::ChatFolderIcon as SidebarChatFolderIcon;
pub(crate) use self::sidebar::ChatFolderRow as SidebarChatFolderRow;
pub(crate) use self::sidebar::ChatFolderSelection as SidebarChatFolderSelection;
pub(crate) use self::sidebar::ChatList as SidebarChatList;
pub(crate) use self::sidebar::MiniThumbnail as SidebarMiniThumbnail;
pub(crate) use self::sidebar::Row as SidebarRow;
pub(crate) use self::sidebar::Search as SidebarSearch;
pub(crate) use self::sidebar::SearchItemRow as SidebarSearchItemRow;
pub(crate) use self::sidebar::SearchRow as SidebarSearchRow;
pub(crate) use self::sidebar::SearchSection as SidebarSearchSection;
pub(crate) use self::sidebar::SearchSectionRow as SidebarSearchSectionRow;
pub(crate) use self::sidebar::SearchSectionType as SidebarSearchSectionType;
pub(crate) use self::sidebar::Selection as SidebarSelection;
pub(crate) use self::sidebar::Sidebar;
pub(crate) use self::switcher::Switcher;
use crate::model;
use crate::types::ChatId;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/mod.ui")]
    pub(crate) struct Session {
        pub(super) model: glib::WeakRef<model::ClientStateSession>,
        #[template_child]
        pub(super) split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub(super) sidebar: TemplateChild<ui::Sidebar>,
        #[template_child]
        pub(super) content: TemplateChild<ui::Content>,
        #[template_child]
        pub(super) content_picker: TemplateChild<ui::ContentPicker>,
        #[template_child]
        pub(super) content_picker_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub(super) sidebar_resize_handle: TemplateChild<gtk::Separator>,
        #[template_child]
        pub(super) picker_resize_handle: TemplateChild<gtk::Separator>,
        pub(super) content_picker_popover: RefCell<Option<gtk::Popover>>,
        pub(super) popup_content_picker: RefCell<Option<ui::ContentPicker>>,
        pub(super) content_picker_close_timeout: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Session {
        const NAME: &'static str = "Session";
        type Type = super::Session;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action("session.show-preferences", None, move |widget, _, _| {
                let preferences = ui::PreferencesWindow::new(widget);
                preferences.present(Some(widget));
            });
            klass.install_action("session.show-contacts", None, move |widget, _, _| {
                let parent = widget.root().and_then(|r| r.downcast().ok());
                let contacts = ui::ContactsWindow::new(parent.as_ref(), widget.clone());

                contacts.connect_contact_activated(clone!(
                    #[weak]
                    widget,
                    move |_, user_id| {
                        widget.select_chat(user_id);
                    }
                ));

                contacts.present();
            });

            klass.add_binding_action(
                gdk::Key::F,
                gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
                "session.begin-chats-search",
            );
            klass.install_action("session.begin-chats-search", None, |widget, _, _| {
                widget.begin_chats_search();
            });

            klass.add_binding_action(
                gdk::Key::v,
                gdk::ModifierType::CONTROL_MASK,
                "session.paste",
            );
            klass.install_action("session.paste", None, move |widget, _, _| {
                widget.handle_paste_action();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Session {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::ClientStateSession>("model")
                        .construct_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "model" => self.model.set(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "model" => self.obj().model().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_resizable_panels();
        }

        fn dispose(&self) {
            if let Some(source) = self.content_picker_close_timeout.borrow_mut().take() {
                source.remove();
            }
            if let Some(popover) = self.content_picker_popover.borrow_mut().take() {
                popover.unparent();
            }
            self.popup_content_picker.borrow_mut().take();
        }
    }

    impl WidgetImpl for Session {}
    impl BinImpl for Session {}
}

glib::wrapper! {
    pub(crate) struct Session(ObjectSubclass<imp::Session>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl From<&model::ClientStateSession> for Session {
    fn from(model: &model::ClientStateSession) -> Self {
        glib::Object::builder().property("model", model).build()
    }
}

const SIDEBAR_MIN_WIDTH: i32 = 240;
const SIDEBAR_MAX_WIDTH: i32 = 500;
const PICKER_MIN_WIDTH: i32 = 260;
const PICKER_MAX_WIDTH: i32 = 560;

impl Session {
    pub(crate) fn model(&self) -> Option<model::ClientStateSession> {
        self.imp().model.upgrade()
    }

    /// Fixes each split view's sidebar to its saved width and makes the divider handle
    /// draggable to resize it, persisting the new width.
    fn setup_resizable_panels(&self) {
        let settings = utils::PaperPlaneSettings::default();
        let imp = self.imp();

        // Chat list sidebar (start): dragging the handle right widens it.
        let sidebar_width = settings
            .int("sidebar-width")
            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH) as f64;
        let split = imp.split_view.get();
        split.set_min_sidebar_width(sidebar_width);
        split.set_max_sidebar_width(sidebar_width);
        let split_apply = split.downgrade();
        self.setup_panel_resize(
            imp.sidebar_resize_handle.upcast_ref(),
            true,
            SIDEBAR_MIN_WIDTH,
            SIDEBAR_MAX_WIDTH,
            "sidebar-width",
            move |width| {
                if let Some(split) = split_apply.upgrade() {
                    split.set_min_sidebar_width(width);
                    split.set_max_sidebar_width(width);
                }
            },
        );

        // Content picker (end): dragging the handle left widens it.
        let picker_width = settings
            .int("content-picker-width")
            .clamp(PICKER_MIN_WIDTH, PICKER_MAX_WIDTH) as f64;
        let picker_split = imp.content_picker_split.get();
        picker_split.set_min_sidebar_width(picker_width);
        picker_split.set_max_sidebar_width(picker_width);
        let picker_apply = picker_split.downgrade();
        let picker_reflow = imp.content_picker.downgrade();
        self.setup_panel_resize(
            imp.picker_resize_handle.upcast_ref(),
            false,
            PICKER_MIN_WIDTH,
            PICKER_MAX_WIDTH,
            "content-picker-width",
            move |width| {
                if let Some(split) = picker_apply.upgrade() {
                    split.set_min_sidebar_width(width);
                    split.set_max_sidebar_width(width);
                }
                // The picker's own `size_allocate` override does not reliably fire for the
                // templated BoxLayout widget, so re-chunk the sticker/emoji grid to the new
                // width from here — the only place the (pinned) picker width changes.
                if let Some(picker) = picker_reflow.upgrade() {
                    picker.reflow_now();
                }
            },
        );
    }

    /// Wires a resizable panel divider. `from_start` is true for a sidebar on the left
    /// (width grows as the handle moves right) and false for one on the right.
    ///
    /// The width is tracked live during the drag. The handle sits inside the panel it
    /// resizes, so naively using the drag offset would feed back into itself and
    /// flicker; instead the pointer is projected into this widget's own (stable)
    /// coordinate space each frame with `compute_point`, which cancels the handle's
    /// movement out exactly, giving smooth live resizing.
    fn setup_panel_resize(
        &self,
        handle: &gtk::Widget,
        from_start: bool,
        min: i32,
        max: i32,
        key: &'static str,
        apply: impl Fn(f64) + 'static,
    ) {
        handle.set_cursor_from_name(Some("col-resize"));

        let drag = gtk::GestureDrag::new();
        let min = min as f64;
        let max = max as f64;
        let last_width = Rc::new(Cell::new(0));

        drag.connect_drag_begin(clone!(
            #[weak]
            handle,
            move |_, _, _| handle.add_css_class("dragging")
        ));
        drag.connect_drag_update(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            handle,
            #[strong]
            last_width,
            move |gesture, offset_x, offset_y| {
                let Some((start_x, start_y)) = gesture.start_point() else {
                    return;
                };
                let point =
                    graphene::Point::new((start_x + offset_x) as f32, (start_y + offset_y) as f32);
                let Some(point) = handle.compute_point(&obj, &point) else {
                    return;
                };
                let pointer_x = point.x() as f64;
                let width = if from_start {
                    pointer_x
                } else {
                    obj.width() as f64 - pointer_x
                }
                .clamp(min, max);
                last_width.set(width as i32);
                apply(width);
            }
        ));
        drag.connect_drag_end(clone!(
            #[weak]
            handle,
            #[strong]
            last_width,
            move |_, _, _| {
                handle.remove_css_class("dragging");
                if last_width.get() > 0 {
                    let _ = utils::PaperPlaneSettings::default().set_int(key, last_width.get());
                }
            }
        ));
        handle.add_controller(drag);
    }

    pub(crate) fn select_chat(&self, id: ChatId) {
        match self.model().unwrap().try_chat(id) {
            Some(chat) => self.imp().sidebar.set_selected_chat(Some(&chat)),
            None => utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    match tdlib::functions::create_private_chat(
                        id,
                        true,
                        obj.model().unwrap().client_().id(),
                    )
                    .await
                    {
                        Ok(tdlib::enums::Chat::Chat(data)) => obj
                            .imp()
                            .sidebar
                            .set_selected_chat(obj.model().unwrap().try_chat(data.id).as_ref()),
                        Err(e) => log::warn!("Failed to create private chat: {e:?}"),
                    }
                }
            )),
        }
    }

    /// Switches to `chat_id` remembering the currently open chat as the place to
    /// return to: the header shows a back arrow until the user navigates away
    /// normally. Used by jumps that originate inside a chat (the chat info
    /// window's open row, forward headers, avatar popovers).
    pub(crate) fn select_chat_with_return(&self, chat_id: ChatId) {
        if let Some(current) = self.imp().content.chat()
            && current.id() != chat_id
        {
            self.imp().content.arm_return_to(current.id(), chat_id);
        }
        self.select_chat(chat_id);
    }

    /// Opens the chat info window for `chat_id`, creating the private chat first
    /// when needed. Mention links use this instead of jumping straight into the
    /// chat, so the current conversation is never left by surprise.
    pub(crate) fn show_chat_info(&self, chat_id: ChatId) {
        let Some(model) = self.model() else {
            log::warn!("show_chat_info: no session model");
            return;
        };
        if let Some(chat) = model.try_chat(chat_id) {
            self.present_chat_info(&chat);
            return;
        }
        let client_id = model.client_().id();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                match tdlib::functions::create_private_chat(chat_id, true, client_id).await {
                    Ok(tdlib::enums::Chat::Chat(chat)) => {
                        let model = obj.model().unwrap();
                        match model.try_chat(chat.id) {
                            Some(chat) => obj.present_chat_info(&chat),
                            None => obj.select_chat(chat_id),
                        }
                    }
                    Err(e) => utils::show_toast(&obj, e.message),
                }
            }
        ));
    }

    fn present_chat_info(&self, chat: &model::Chat) {
        let parent = self.root().and_then(|r| r.downcast().ok());
        ui::ChatInfoWindow::new(&parent, chat).present();
    }

    pub(crate) fn handle_paste_action(&self) {
        self.imp().content.handle_paste_action();
    }

    /// Jumps the open chat's history to its newest message (used when the user
    /// re-activates the already-selected chat).
    pub(crate) fn scroll_content_to_latest(&self) {
        self.imp().content.scroll_history_to_latest();
    }

    /// Reveals the content (chat) page. On a collapsed (mobile) layout this navigates
    /// away from the chat-list sidebar; when expanded both are already visible, so it
    /// has no visible effect.
    pub(crate) fn show_chat_content(&self) {
        self.imp().split_view.set_show_content(true);
    }

    /// Opens the content-picker sidebar on the sticker tab, showing a single sticker
    /// pack (triggered by "Show Sticker Pack" in a sticker message's menu).
    pub(crate) fn show_sticker_pack(&self, set_id: i64) {
        self.imp().content_picker_split.set_show_sidebar(true);
        self.imp().content_picker.show_sticker_pack(set_id);
    }

    pub(crate) fn toggle_content_picker_sidebar(&self) {
        let split = &self.imp().content_picker_split;
        let show = !split.shows_sidebar();
        split.set_show_sidebar(show);
        if show {
            self.imp().content_picker.refresh();
        }
    }

    pub(crate) fn show_content_picker_popup(&self, anchor: &gtk::Image) {
        // The hover popover is a stand-in for the picker sidebar; when that sidebar is
        // already visible (side-by-side or as its own overlay), don't stack a popover on it.
        if self.imp().content_picker_split.shows_sidebar() {
            return;
        }
        self.cancel_content_picker_close();
        let popover = self.content_picker_popover(anchor);
        if let Some(picker) = &*self.imp().popup_content_picker.borrow() {
            picker.set_property("chat", self.imp().content.chat());
            picker.refresh();
        }
        popover.popup();
    }

    pub(crate) fn schedule_content_picker_close(&self) {
        self.cancel_content_picker_close();
        let source = glib::timeout_add_local_once(
            std::time::Duration::from_millis(180),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    obj.imp().content_picker_close_timeout.borrow_mut().take();
                    if let Some(popover) = &*obj.imp().content_picker_popover.borrow() {
                        popover.popdown();
                    }
                }
            ),
        );
        *self.imp().content_picker_close_timeout.borrow_mut() = Some(source);
    }

    fn cancel_content_picker_close(&self) {
        if let Some(source) = self.imp().content_picker_close_timeout.borrow_mut().take() {
            source.remove();
        }
    }

    fn content_picker_popover(&self, anchor: &gtk::Image) -> gtk::Popover {
        if let Some(popover) = &*self.imp().content_picker_popover.borrow() {
            return popover.clone();
        }
        let popover = gtk::Popover::builder()
            .has_arrow(true)
            .autohide(false)
            .position(gtk::PositionType::Top)
            .build();
        let picker: ui::ContentPicker = glib::Object::builder()
            .property("session", self.model())
            .property("chat", self.imp().content.chat())
            .build();
        picker.set_size_request(360, 540);
        popover.set_child(Some(&picker));
        popover.set_parent(anchor);
        picker.connect_content_picked(clone!(
            #[weak]
            popover,
            move |_| popover.popdown()
        ));
        let motion = gtk::EventControllerMotion::new();
        motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        motion.connect_enter(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _| obj.cancel_content_picker_close()
        ));
        motion.connect_leave(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.schedule_content_picker_close()
        ));
        popover.add_controller(motion);
        let picker_motion = gtk::EventControllerMotion::new();
        picker_motion.set_propagation_phase(gtk::PropagationPhase::Capture);
        picker_motion.connect_enter(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _| obj.cancel_content_picker_close()
        ));
        picker_motion.connect_leave(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.schedule_content_picker_close()
        ));
        picker.add_controller(picker_motion);
        let picker_press = gtk::GestureClick::new();
        picker_press.set_propagation_phase(gtk::PropagationPhase::Capture);
        picker_press.connect_pressed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, _, _, _| {
                obj.cancel_content_picker_close();
                gesture.set_state(gtk::EventSequenceState::Denied);
            }
        ));
        picker.add_controller(picker_press);
        *self.imp().popup_content_picker.borrow_mut() = Some(picker);
        *self.imp().content_picker_popover.borrow_mut() = Some(popover.clone());
        popover
    }

    pub(crate) fn insert_emoji(&self, emoji: &str) {
        self.imp().content.insert_emoji(emoji);
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        self.imp()
            .content
            .insert_custom_emoji(emoji, custom_emoji_id);
    }

    pub(crate) fn begin_chats_search(&self) {
        let imp = self.imp();
        imp.split_view.set_show_content(false);
        imp.sidebar.begin_chats_search();
    }
}
