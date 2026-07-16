mod avatar;
mod chat_folder;
mod chat_list;
mod mini_thumbnail;
mod row;
mod search;
mod selection;

use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;

use glib::Properties;
use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

pub(crate) use self::avatar::Avatar;
pub(crate) use self::chat_folder::Bar as ChatFolderBar;
pub(crate) use self::chat_folder::Icon as ChatFolderIcon;
pub(crate) use self::chat_folder::Row as ChatFolderRow;
pub(crate) use self::chat_folder::Selection as ChatFolderSelection;
pub(crate) use self::chat_list::ChatList;
pub(crate) use self::mini_thumbnail::MiniThumbnail;
pub(crate) use self::row::Row;
pub(crate) use self::search::ItemRow as SearchItemRow;
pub(crate) use self::search::Row as SearchRow;
pub(crate) use self::search::Search;
pub(crate) use self::search::Section as SearchSection;
pub(crate) use self::search::SectionRow as SearchSectionRow;
pub(crate) use self::search::SectionType as SearchSectionType;
pub(crate) use self::selection::Selection;
use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::Sidebar)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/mod.ui")]
    pub(crate) struct Sidebar {
        #[property(get, set)]
        pub(super) compact: Cell<bool>,
        #[property(get, set, nullable)]
        pub(super) selected_chat: glib::WeakRef<model::Chat>,
        #[property(get, set = Self::set_session, explicit_notify)]
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        pub(super) row_menu: OnceCell<gtk::PopoverMenu>,
        // Kept alive so the `vertical-folders` change handler stays connected.
        pub(super) settings: OnceCell<utils::PaperPlaneSettings>,
        // The currently displayed chat list and its "loading" notify handler, so the
        // header shimmer can follow whichever list (folder) is on screen.
        pub(super) loading_handler:
            RefCell<Option<(glib::WeakRef<model::ChatList>, glib::SignalHandlerId)>>,
        // The archive chat list and its items-changed handler, disconnected and
        // reconnected whenever the session changes.
        pub(super) archive_handler:
            RefCell<Option<(glib::WeakRef<model::ChatList>, glib::SignalHandlerId)>>,
        #[template_child]
        pub(super) navigation_view: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub(super) main_view: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub(super) search: TemplateChild<Search>,
        #[template_child]
        pub(super) title_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) archive_unread_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) snow: TemplateChild<ui::Snow>,
        #[template_child]
        pub(super) folder_bar: TemplateChild<ui::SidebarChatFolderBar>,
        #[template_child]
        pub(super) folder_rail: TemplateChild<gtk::Box>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sidebar {
        const NAME: &'static str = "PaplSidebar";
        type Type = super::Sidebar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action("sidebar.show-sessions", None, move |widget, _, _| {
                widget.show_sessions();
            });

            klass.install_action("sidebar.start-search", None, move |widget, _, _| {
                widget.begin_chats_search();
            });

            klass.install_action("sidebar.show-archived-chats", None, move |widget, _, _| {
                widget.show_archived_chats();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl Sidebar {
        fn set_session(&self, session: Option<&model::ClientStateSession>) {
            let obj = &*self.obj();
            if obj.session().as_ref() == session {
                return;
            }

            if let Some((weak, id)) = self.archive_handler.take()
                && let Some(list) = weak.upgrade()
            {
                list.disconnect(id);
            }

            if let Some(session) = session {
                let list = session.archive_chat_list();
                let id = list.connect_items_changed(clone!(
                    #[weak]
                    obj,
                    move |chat_list, _, _, _| {
                        if chat_list.n_items() == 0 {
                            obj.imp().navigation_view.pop_to_tag("chats");
                        }
                    }
                ));
                self.archive_handler.replace(Some((list.downgrade(), id)));
            }

            self.session.set(session);
            obj.notify_session();
        }

        #[template_callback]
        fn close_search(&self) {
            self.navigation_view.pop_to_tag("chats");
        }

        fn update_loading_shimmer(&self) {
            // Drop the handler for the previously displayed list.
            if let Some((weak, id)) = self.loading_handler.take()
                && let Some(list) = weak.upgrade()
            {
                list.disconnect(id);
            }

            let title_stack = self.title_stack.get();
            let list = self.folder_bar.selected_chat_list();

            set_header_loading(&title_stack, list.as_ref().is_some_and(|l| l.loading()));

            if let Some(list) = list {
                let id = list.connect_notify_local(
                    Some("loading"),
                    clone!(
                        #[weak]
                        title_stack,
                        move |list: &model::ChatList, _| {
                            set_header_loading(&title_stack, list.loading());
                        }
                    ),
                );
                self.loading_handler.replace(Some((list.downgrade(), id)));
            }
        }
    }

    impl ObjectImpl for Sidebar {
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

            // Place the chat-folder bar either along the top (horizontal) or as a rail
            // on the far left (vertical), following the setting live.
            let settings = utils::PaperPlaneSettings::default();
            obj.apply_folder_layout(settings.boolean("vertical-folders"));
            settings.connect_changed(
                Some("vertical-folders"),
                clone!(
                    #[weak]
                    obj,
                    move |settings, _| {
                        obj.apply_folder_layout(settings.boolean("vertical-folders"));
                    }
                ),
            );
            let _ = self.settings.set(settings);

            let session_expr = Self::Type::this_expression("session");

            session_expr
                .chain_property::<model::ClientStateSession>("chat-folder-list")
                .chain_property::<model::ChatFolderList>("has-folders")
                .bind(&self.folder_bar.get(), "visible", Some(obj));

            let archive_chat_list_expr =
                session_expr.chain_property::<model::ClientStateSession>("archive-chat-list");

            let archive_chat_list_unread_count_expr =
                archive_chat_list_expr.chain_property::<model::ChatList>("unread-chat-count");

            archive_chat_list_expr
                .chain_property::<model::ChatList>("len")
                .chain_closure::<String>(closure!(|_: Self::Type, len: u32| {
                    if len > 0 {
                        "archive-button"
                    } else {
                        "chats-label"
                    }
                }))
                .bind(&self.title_stack.get(), "visible-child-name", Some(obj));

            archive_chat_list_unread_count_expr.bind(
                &self.archive_unread_label.get(),
                "label",
                Some(obj),
            );
            archive_chat_list_unread_count_expr.bind(
                &self.archive_unread_label.get(),
                "visible",
                Some(obj),
            );

            // Shimmer the header title while the chat list currently on screen is
            // still loading. Re-target whenever the displayed folder changes.
            self.folder_bar.connect_notify_local(
                Some("selected-chat-list"),
                clone!(
                    #[weak]
                    obj,
                    move |_, _| obj.imp().update_loading_shimmer()
                ),
            );
            self.update_loading_shimmer();
        }

        fn dispose(&self) {
            if let Some((weak, id)) = self.archive_handler.take()
                && let Some(list) = weak.upgrade()
            {
                list.disconnect(id);
            }
            if let Some((weak, id)) = self.loading_handler.take()
                && let Some(list) = weak.upgrade()
            {
                list.disconnect(id);
            }
            self.dispose_template();
        }
    }

    impl WidgetImpl for Sidebar {
        fn direction_changed(&self, previous_direction: gtk::TextDirection) {
            let obj = self.obj();

            if obj.direction() == previous_direction {
                return;
            }

            if let Some(menu) = self.row_menu.get() {
                menu.set_halign(if obj.direction() == gtk::TextDirection::Rtl {
                    gtk::Align::End
                } else {
                    gtk::Align::Start
                });
            }
        }
    }

    fn set_header_loading(title_stack: &gtk::Stack, loading: bool) {
        if loading {
            title_stack.add_css_class("chatlist-loading");
        } else {
            title_stack.remove_css_class("chatlist-loading");
        }
    }
}

glib::wrapper! {
    pub(crate) struct Sidebar(ObjectSubclass<imp::Sidebar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sidebar {
    /// Moves the chat-folder bar between the top toolbar (horizontal) and a rail on the
    /// far left (vertical), matching the `vertical-folders` preference.
    fn apply_folder_layout(&self, vertical: bool) {
        let imp = self.imp();
        let bar = imp.folder_bar.get();
        let rail: &gtk::Widget = imp.folder_rail.upcast_ref();
        let in_rail = bar.parent().as_ref() == Some(rail);

        if vertical && !in_rail {
            if bar.parent().is_some() {
                imp.main_view.remove(&bar);
            }
            bar.set_vertical(true);
            bar.set_vexpand(true);
            imp.folder_rail.append(&bar);
        } else if !vertical && in_rail {
            imp.folder_rail.remove(&bar);
            bar.set_vertical(false);
            bar.set_vexpand(false);
            imp.main_view.add_top_bar(&bar);
        }
    }

    pub(crate) fn row_menu(&self) -> &gtk::PopoverMenu {
        self.imp().row_menu.get_or_init(|| {
            let menu =
                gtk::Builder::from_resource("/app/drey/paper-plane/ui/session/sidebar/row_menu.ui")
                    .object::<gtk::PopoverMenu>("menu")
                    .unwrap();

            menu.set_halign(if self.direction() == gtk::TextDirection::Rtl {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });

            // The menu is shared and re-parented onto whichever row was clicked. Detach it
            // when it closes so it is never left parented to a row that is later destroyed
            // (e.g. list recycling or window close), which would dangle its parent pointer.
            menu.connect_closed(|menu| menu.unparent());

            menu
        })
    }

    pub(crate) fn show_sessions(&self) {
        self.imp().navigation_view.push_by_tag("sessions");
    }

    pub(crate) fn begin_chats_search(&self) {
        let imp = self.imp();
        imp.search.reset();
        imp.search.grab_focus();
        imp.navigation_view.push_by_tag("search");
    }

    pub(crate) fn show_archived_chats(&self) {
        self.imp().navigation_view.push_by_tag("archived-chats");
    }
}
