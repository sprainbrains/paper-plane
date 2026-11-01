use std::cell::OnceCell;
use std::sync::OnceLock;

use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod updates;

pub(crate) use self::updates::timestamp_text;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/row.ui")]
    pub(crate) struct Row {
        pub(super) item: glib::WeakRef<model::ChatListItem>,
        /// This row's context menu. Built lazily and parented to this row only:
        /// a popover shared between rows resolves its menu-model actions against
        /// the widget hierarchy captured at construction, so re-parenting it on
        /// every popup silently breaks every item (see the history of this
        /// comment's bug). One popover per row keeps the action muxer chain
        /// stable.
        pub(super) menu: OnceCell<gtk::PopoverMenu>,
        pub(super) item_signal_group: OnceCell<glib::SignalGroup>,
        pub(super) chat_signal_group: OnceCell<glib::SignalGroup>,
        pub(super) session_signal_group: OnceCell<glib::SignalGroup>,
        /// Watches the bound forum's topic list so the chips refresh when it
        /// arrives or re-sorts.
        pub(super) forum_list_signal_group: OnceCell<glib::SignalGroup>,
        /// Topic ids currently shown on the two chips (0 = empty).
        pub(super) forum_chip_topics: [std::cell::Cell<i32>; 2],
        /// Emoji id the loaded chip texture belongs to (0 = none).
        pub(super) forum_emoji_loaded_for: [std::cell::Cell<i64>; 2],
        /// Whether chip emoji work is still demanded (row mapped).
        pub(super) forum_demand: std::rc::Rc<std::cell::Cell<bool>>,
        #[template_child]
        pub(super) sidebar_avatar: TemplateChild<ui::SidebarAvatar>,
        #[template_child]
        pub(super) forum_chips_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) forum_chip_1: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) forum_chip_1_icon: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) forum_chip_1_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) forum_chip_2: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) forum_chip_2_icon: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) forum_chip_2_name: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) forum_chip_1_emoji: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) forum_chip_2_emoji: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) title: TemplateChild<ui::ChatTitle>,
        #[template_child]
        pub(super) message_status_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) timestamp_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) subtitle_prefix_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) minithumbnail: TemplateChild<ui::SidebarMiniThumbnail>,
        #[template_child]
        pub(super) subtitle_label: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) typing_indicator: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) status_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) pin_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) unread_mention_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) unread_count_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "PaplSidebarRow";
        type Type = super::Row;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("sidebar-row.archive", None, move |widget, _, _| {
                widget.toggle_chat_is_archived()
            });
            klass.install_action("sidebar-row.unarchive", None, move |widget, _, _| {
                widget.toggle_chat_is_archived()
            });
            klass.install_action("sidebar-row.pin", None, move |widget, _, _| {
                widget.toggle_chat_is_pinned()
            });
            klass.install_action("sidebar-row.unpin", None, move |widget, _, _| {
                widget.toggle_chat_is_pinned()
            });
            klass.install_action("sidebar-row.mark-as-unread", None, move |widget, _, _| {
                widget.toggle_chat_marked_as_unread()
            });
            klass.install_action("sidebar-row.mark-as-read", None, move |widget, _, _| {
                widget.toggle_chat_marked_as_unread()
            });
            klass.install_action("sidebar-row.open-in-window", None, move |widget, _, _| {
                widget.open_in_separate_window()
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl Row {
        #[template_callback]
        fn on_pressed(&self, _n_press: i32, x: f64, y: f64) {
            log::warn!("MENU-PRESS chat row at {x},{y}");
            self.show_menu(x as i32, y as i32);
        }

        #[template_callback]
        fn on_long_pressed(&self, x: f64, y: f64) {
            self.show_menu(x as i32, y as i32);
        }

        fn menu(&self) -> gtk::PopoverMenu {
            let obj = &*self.obj();
            self.menu
                .get_or_init(|| {
                    let menu = gtk::Builder::from_resource(
                        "/app/drey/paper-plane/ui/session/sidebar/row_menu.ui",
                    )
                    .object::<gtk::PopoverMenu>("menu")
                    .unwrap();

                    menu.set_halign(if obj.direction() == gtk::TextDirection::Rtl {
                        gtk::Align::End
                    } else {
                        gtk::Align::Start
                    });

                    menu
                })
                .clone()
        }

        fn show_menu(&self, x: i32, y: i32) {
            let obj = &*self.obj();
            let menu = self.menu();

            menu.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 0, 0)));
            if menu.parent().is_none() {
                menu.set_parent(obj);
            }
            menu.popup();
        }
    }

    impl ObjectImpl for Row {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::ChatListItem>("item")
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

        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();

            obj.setup_expressions();
            obj.create_signal_groups();

            // Ctrl+click opens the chat in a separate window. Handled in the capture phase
            // so it beats the chat list's single-click-activate; a plain click is denied
            // here and falls through to normal activation.
            let ctrl_click = gtk::GestureClick::new();
            ctrl_click.set_button(gdk::BUTTON_PRIMARY);
            ctrl_click.set_propagation_phase(gtk::PropagationPhase::Capture);
            ctrl_click.connect_pressed(clone!(
                #[weak]
                obj,
                move |gesture, _, _, _| {
                    if gesture
                        .current_event_state()
                        .contains(gdk::ModifierType::CONTROL_MASK)
                    {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        obj.open_in_separate_window();
                    } else {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                    }
                }
            ));
            obj.add_controller(ctrl_click);

            // A message selection dragged from the chat history can be dropped on a chat
            // to forward the messages there.
            let drop_target =
                gtk::DropTarget::new(ui::MessageDragPayload::static_type(), gdk::DragAction::COPY);
            drop_target.connect_drop(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                false,
                move |_, value, _, _| {
                    let Ok(payload) = value.get::<ui::MessageDragPayload>() else {
                        return false;
                    };
                    let Some(item) = obj.item() else {
                        return false;
                    };
                    let chat = item.chat_();
                    let to_chat_id = chat.id();
                    let client_id = chat.session_().client_().id();
                    if let Some(history) = payload.history.upgrade()
                        && history.is_selection_mode()
                    {
                        history.clear_selection();
                    }
                    utils::spawn(clone!(
                        #[weak]
                        obj,
                        async move {
                            if let Err(e) = tdlib::functions::forward_messages(
                                to_chat_id,
                                None,
                                payload.from_chat_id,
                                payload.message_ids,
                                None,
                                payload.send_copy,
                                false,
                                client_id,
                            )
                            .await
                            {
                                utils::show_toast(&obj, e.message);
                            }
                        }
                    ));
                    true
                }
            ));
            obj.add_controller(drop_target);

            // Clicking a topic chip on a collapsed forum row opens that topic
            // directly (tdesktop); the gesture claims the press so the row
            // itself is not activated.
            for (chip, index) in [
                (self.forum_chip_1.get(), 0usize),
                (self.forum_chip_2.get(), 1usize),
            ] {
                let gesture = gtk::GestureClick::new();
                gesture.set_button(gdk::BUTTON_PRIMARY);
                gesture.connect_released(clone!(
                    #[weak]
                    obj,
                    move |gesture, _, _, _| {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        obj.open_chip_topic(index);
                    }
                ));
                chip.add_controller(gesture);
            }

            // The topic list fetch is per-row work: start it when the row is
            // mapped, never on bind (AGENTS.md); the list dedupes refreshes.
            obj.connect_map(|row| row.on_map_forum());
            obj.connect_unmap(|row| row.imp().forum_demand.set(false));
        }

        fn dispose(&self) {
            if let Some(menu) = self.menu.get() {
                menu.unparent();
            }
            self.dispose_template();
            // dispose_template leaves the template's unnamed root children (the avatar and
            // the content box) parented, which warns and leaks on finalize; unparent
            // whatever remains.
            let mut child = self.obj().first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                widget.unparent();
            }
        }
    }

    impl WidgetImpl for Row {}
}

glib::wrapper! {
    pub(crate) struct Row(ObjectSubclass<imp::Row>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Row {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Opens this row's chat in a separate window (or, if it is already shown in another
    /// window, raises that window and shows a toast).
    fn open_in_separate_window(&self) {
        let Some(item) = self.item() else {
            return;
        };
        let chat = item.chat_();
        let client = chat.session_().client_();

        let Some(window) = utils::ancestor::<_, ui::Window>(self) else {
            return;
        };
        let Some(app) = window.application().and_downcast::<crate::Application>() else {
            return;
        };
        app.open_chat_in_window(&client, chat.id(), &window);
    }

    pub(crate) fn toggle_chat_is_archived(&self) {
        if let Some(item) = self.item() {
            let chat = item.chat_();
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    if let Err(e) = tdlib::functions::add_chat_to_list(
                        chat.id(),
                        match item.chat_list_type().0 {
                            tdlib::enums::ChatList::Main => tdlib::enums::ChatList::Archive,
                            tdlib::enums::ChatList::Archive => tdlib::enums::ChatList::Main,
                            _ => return,
                        },
                        chat.session_().client_().id(),
                    )
                    .await
                    {
                        utils::show_toast(&obj, e.message);
                    }
                }
            ));
        }
    }

    fn toggle_chat_is_pinned(&self) {
        if let Some(item) = self.item() {
            utils::spawn(async move {
                if let Err(e) = item.toggle_is_pinned().await {
                    log::warn!("Error on toggling chat's pinned state: {e:?}");
                }
            });
        }
    }

    fn toggle_chat_marked_as_unread(&self) {
        if let Some(chat) = self.item().map(|i| i.chat_()) {
            utils::spawn(async move {
                if let Err(e) = if chat.unread_count() > 0 || chat.is_marked_as_unread() {
                    chat.mark_as_read().await
                } else {
                    chat.mark_as_unread().await
                } {
                    log::warn!("Error on toggling chat's unread state: {e:?}");
                }
            });
        }
    }

    fn setup_expressions(&self) {
        let imp = self.imp();
        let item_expression = Self::this_expression("item");
        let chat_expression = item_expression.chain_property::<model::ChatListItem>("chat");

        // Chat title (name + verification/source/mute badges), rendered by the
        // shared title widget.
        chat_expression.bind(&*imp.title, "chat", Some(self));

        // Chat unread count
        chat_expression
            .chain_property::<model::Chat>("unread-count")
            .chain_closure::<String>(closure!(|_: Self, unread_count: i32| {
                if unread_count > 0 {
                    unread_count.to_string()
                } else {
                    String::new()
                }
            }))
            .bind(&*imp.unread_count_label, "label", Some(self));
    }

    fn create_signal_groups(&self) {
        let imp = self.imp();

        let item_signal_group = glib::SignalGroup::new::<model::ChatListItem>();
        item_signal_group.connect_notify_local(
            Some("is-pinned"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_status_stack();
                    obj.update_actions();
                }
            ),
        );
        imp.item_signal_group.set(item_signal_group).unwrap();

        let chat_signal_group = glib::SignalGroup::new::<model::Chat>();
        chat_signal_group.connect_notify_local(
            Some("is-marked-as-unread"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_status_stack();
                    obj.update_actions();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("unread-count"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_status_stack();
                    obj.update_actions();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("unread-mention-count"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_status_stack();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("last-read-outbox-message-id"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_message_status_icon();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("last-message"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_message_status_icon();
                    obj.update_timestamp();
                    obj.update_subtitle_prefix_label();
                    obj.update_minithumbnail();
                    obj.update_subtitle_label();
                    obj.update_forum_chips();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("draft-message"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_timestamp();
                    obj.update_subtitle_prefix_label();
                    obj.update_minithumbnail();
                    obj.update_subtitle_label();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("actions"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_subtitle_prefix_label();
                    obj.update_minithumbnail();
                    obj.update_subtitle_label();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("notification-settings"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_unread_count_style();
                }
            ),
        );
        imp.chat_signal_group.set(chat_signal_group).unwrap();

        let session_signal_group = glib::SignalGroup::new::<model::ClientStateSession>();
        session_signal_group.connect_notify_local(
            Some("private-chats-notification-settings"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_unread_count_style();
                }
            ),
        );
        session_signal_group.connect_notify_local(
            Some("group-chats-notification-settings"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_unread_count_style();
                }
            ),
        );
        session_signal_group.connect_notify_local(
            Some("channel-chats-notification-settings"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_unread_count_style();
                }
            ),
        );
        imp.session_signal_group.set(session_signal_group).unwrap();

        // A forum's chips follow its topic list; the target is (re)set in
        // `update_forum_chips` since the list is created lazily.
        let forum_list_signal_group = glib::SignalGroup::new::<model::ForumTopicList>();
        let weak = self.downgrade();
        forum_list_signal_group.connect_local("items-changed", false, move |_| {
            if let Some(obj) = weak.upgrade() {
                obj.update_forum_chips();
            }
            None
        });
        imp.forum_list_signal_group
            .set(forum_list_signal_group)
            .unwrap();
    }

    pub(crate) fn item(&self) -> Option<model::ChatListItem> {
        self.imp().item.upgrade()
    }

    pub(crate) fn set_item(&self, item: Option<&model::ChatListItem>) {
        if self.item().as_ref() == item {
            return;
        }

        let imp = self.imp();

        imp.item_signal_group.get().unwrap().set_target(item);
        imp.chat_signal_group
            .get()
            .unwrap()
            .set_target(item.map(|i| i.chat_()).as_ref());
        imp.session_signal_group
            .get()
            .unwrap()
            .set_target(item.map(|i| i.chat_().session_()).as_ref());

        imp.item.set(item);

        self.update_message_status_icon();
        self.update_timestamp();
        self.update_subtitle_prefix_label();
        self.update_minithumbnail();
        self.update_subtitle_label();
        self.update_status_stack();
        self.update_unread_count_style();
        self.update_actions();
        self.update_forum_chips();

        self.notify("item");
    }
}
