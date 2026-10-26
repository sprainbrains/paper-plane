use std::cell::OnceCell;

use gettextrs::gettext;
use glib::Properties;
use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::i18n::gettext_f;
use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::Row)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/chat_folder/row.ui")]
    pub(crate) struct Row {
        /// This row's context menu; see the sidebar row's comment on why the
        /// popover must not be shared between rows.
        pub(super) menu: std::cell::OnceCell<gtk::PopoverMenu>,
        #[property(get, set, construct_only)]
        pub(super) chat_folder_bar: OnceCell<ui::SidebarChatFolderBar>,
        #[property(get, set)]
        pub(super) chat_list: glib::WeakRef<model::ChatList>,
        #[template_child]
        pub(super) title_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "PaplSidebarChatFolderRow";
        type Type = super::Row;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.set_css_name("chatfolderrow");

            klass.install_action("chat-folder-row.edit", None, |widget, _, _| {
                widget.edit_chat_folder();
            });
            klass.install_action_async("chat-folder-row.remove", None, |widget, _, _| async move {
                widget.remove_chat_folder().await;
            });
            klass.install_action_async(
                "chat-folder-row.mark-read",
                None,
                |widget, _, _| async move {
                    widget.mark_all_as_read().await;
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Row {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();

            let chat_list_expr = Self::Type::this_expression("chat-list");

            gtk::ClosureExpression::new::<String>(
                [
                    &chat_list_expr.chain_property::<model::ChatList>("list-type"),
                    &chat_list_expr.chain_property::<model::ChatList>("title"),
                ],
                closure!(
                    |_: Self::Type, list_type: model::BoxedChatListType, title: String| {
                        use tdlib::enums::ChatList;

                        match list_type.0 {
                            ChatList::Main => gettext("All Chats"),
                            ChatList::Archive => gettext("Archived Chats"),
                            _ => title,
                        }
                    }
                ),
            )
            .bind(&self.title_label.get(), "label", Some(obj));

            obj.setup_dnd();
        }

        fn dispose(&self) {
            self.dispose_template();
            if let Some(menu) = self.menu.get() {
                menu.unparent();
            }
            // `dispose_template` leaves the template's root box parented on this
            // widget subclass. Unparent whatever remains so list recycling
            // doesn't finalize the row with live children.
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Row {}

    #[gtk::template_callbacks]
    impl Row {
        #[template_callback]
        fn on_notify_chat_list(&self) {
            let obj = self.obj();
            obj.action_set_enabled("chat-folder-row.remove", obj.is_folder());
        }

        #[template_callback]
        fn on_button_3_pressed(gesture: &gtk::GestureClick, _: i32, _: f64, _: f64) {
            gesture.set_state(gtk::EventSequenceState::Claimed);
        }

        #[template_callback]
        fn on_button_3_released(&self, _n_press: i32, x: f64, y: f64) {
            self.show_menu(x as i32, y as i32);
        }

        #[template_callback]
        fn on_long_pressed(&self, x: f64, y: f64) {
            self.show_menu(x as i32, y as i32);
        }

        fn show_menu(&self, x: i32, y: i32) {
            let obj = &*self.obj();

            if obj.is_folder() {
                let popover_menu = self.menu.get_or_init(|| {
                    gtk::Builder::from_resource(
                        "/app/drey/paper-plane/ui/session/sidebar/chat_folder/row_menu.ui",
                    )
                    .object::<gtk::PopoverMenu>("popover_menu")
                    .unwrap()
                });

                if popover_menu.parent().is_none() {
                    popover_menu.set_parent(obj);
                }
                popover_menu.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 0, 0)));
                popover_menu.set_halign(match obj.direction() {
                    gtk::TextDirection::Ltr => gtk::Align::Start,
                    _ => gtk::Align::End,
                });

                popover_menu.popup();
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct Row(ObjectSubclass<imp::Row>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Row {
    pub(crate) fn new(
        chat_folder_bar: &ui::SidebarChatFolderBar,
        chat_list: &model::ChatList,
    ) -> Self {
        glib::Object::builder()
            .property("chat-folder-bar", chat_folder_bar)
            .property("chat-list", chat_list)
            .build()
    }

    /// Sets up drag-to-reorder: folder tabs can be dragged onto any tab to move to that
    /// position; the drop asks the model to persist the new order via TDLib.
    fn setup_dnd(&self) {
        let drag = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        drag.connect_prepare(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            None,
            move |_, _, _| {
                // Only actual folders can be reordered (not "All Chats").
                let position = obj.is_folder().then(|| obj.position_in_bar()).flatten()?;
                Some(gdk::ContentProvider::for_value(
                    &(position as i32).to_value(),
                ))
            }
        ));
        self.add_controller(drag);

        let drop = gtk::DropTarget::new(i32::static_type(), gdk::DragAction::MOVE);
        drop.connect_enter(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            gdk::DragAction::empty(),
            move |_, _, _| {
                obj.set_drop_indicator(true);
                gdk::DragAction::MOVE
            }
        ));
        drop.connect_leave(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.set_drop_indicator(false)
        ));
        drop.connect_drop(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                obj.set_drop_indicator(false);
                let Ok(from) = value.get::<i32>() else {
                    return false;
                };
                let Some(to) = obj.position_in_bar() else {
                    return false;
                };
                let Some(list) = obj.chat_folder_bar().chat_folder_list() else {
                    return false;
                };
                // reorder() removes `from` before inserting at `to`. On a forward drag
                // that shift moves the target down by one, so "before target" is `to - 1`;
                // otherwise the item lands after the target, contradicting the leading-edge
                // drop indicator.
                let from = from as u32;
                let to = if from < to { to - 1 } else { to };
                list.reorder(from, to);
                true
            }
        ));
        self.add_controller(drop);
    }

    /// Shows or hides the "insert here" line on this tab's leading edge — a top line in the
    /// vertical rail, a left line in the horizontal strip.
    fn set_drop_indicator(&self, show: bool) {
        self.remove_css_class("drop-before-h");
        self.remove_css_class("drop-before-v");
        if show {
            if self.chat_folder_bar().is_vertical() {
                self.add_css_class("drop-before-h");
            } else {
                self.add_css_class("drop-before-v");
            }
        }
    }

    /// This row's position in the combined folder bar (folders + "All Chats").
    fn position_in_bar(&self) -> Option<u32> {
        let list = self.chat_folder_bar().chat_folder_list()?;
        let chat_list = self.chat_list()?;
        list.position_of(&chat_list)
    }

    /// Opens the folder editor (rename + choose member chats) for this folder.
    fn edit_chat_folder(&self) {
        let Some(folder_id) = self
            .chat_list()
            .and_then(|chat_list| chat_list.list_type().chat_folder_id())
        else {
            return;
        };
        let Some(session) = utils::ancestor::<_, ui::Session>(self) else {
            return;
        };
        let window = self.root().and_downcast::<gtk::Window>();
        super::EditWindow::new(window.as_ref(), &session, folder_id).present();
    }

    pub(crate) async fn remove_chat_folder(&self) {
        if let Some(chat_list) = self.chat_list()
            && let Err(e) = chat_list.delete().await
        {
            utils::show_toast(
                self,
                gettext_f(
                    "Failed to remove folder: {error}",
                    &[("error", &e.to_string())],
                ),
            );
        }
    }

    /// Marks every chat in this folder as read. TDLib has no folder-level operation, so
    /// this drives the folder's (lazily-loaded) chat list to completion, then marks each.
    pub(crate) async fn mark_all_as_read(&self) {
        let Some(chat_list) = self.chat_list() else {
            return;
        };

        // Drive the lazily-loaded list to completion so the whole folder is marked, not
        // just the currently-visible subset.
        chat_list.fetch();
        while chat_list.loading() {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }

        // Snapshot the chats before awaiting: mark_as_read() yields the main loop, during
        // which the live model can reorder and shift indices, causing skips.
        let chats: Vec<model::Chat> = (0..chat_list.n_items())
            .filter_map(|position| {
                chat_list
                    .item(position)
                    .and_downcast::<model::ChatListItem>()
                    .and_then(|item| item.chat())
            })
            .collect();

        let mut failed = false;
        for chat in chats {
            if (chat.unread_count() > 0 || chat.is_marked_as_unread())
                && let Err(e) = chat.mark_as_read().await
            {
                log::warn!("Failed to mark a chat as read: {e:?}");
                failed = true;
            }
        }
        if failed {
            utils::show_toast(self, gettext("Failed to mark some chats as read"));
        }
    }

    pub(crate) fn is_folder(&self) -> bool {
        self.chat_list()
            .filter(|chat_list| {
                matches!(
                    chat_list.list_type().0,
                    tdlib::enums::ChatList::Folder { .. }
                )
            })
            .is_some()
    }
}
