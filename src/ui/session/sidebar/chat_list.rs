use std::cell::Cell;
use std::cell::RefCell;

use glib::Properties;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::ChatList)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/chat_list.ui")]
    pub(crate) struct ChatList {
        pub(super) marked_as_unread_handler: RefCell<Option<(model::Chat, glib::SignalHandlerId)>>,
        #[property(get, set = Self::set_selected_chat, nullable, explicit_notify)]
        pub(super) selected_chat: glib::WeakRef<model::Chat>,
        #[property(get, set)]
        pub(super) chat_list: glib::WeakRef<model::ChatList>,
        #[property(get, set)]
        pub(super) compact: Cell<bool>,
        #[template_child]
        pub(super) selection: TemplateChild<ui::SidebarSelection>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatList {
        const NAME: &'static str = "PaplSidebarChatList";
        type Type = super::ChatList;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.set_css_name("chatlist");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatList {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn dispose(&self) {
            self.disconnect_marked_as_unread_handler();
            self.dispose_template();
            // The unnamed template root child is not tracked by
            // dispose_template(); leaving it attached warns on finalize.
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for ChatList {}

    #[gtk::template_callbacks]
    impl ChatList {
        fn disconnect_marked_as_unread_handler(&self) {
            if let Some((chat, handler)) = self.marked_as_unread_handler.take() {
                chat.disconnect(handler);
            }
        }

        pub(crate) fn set_selected_chat(&self, selected_chat: Option<&model::Chat>) {
            let obj = &*self.obj();
            if obj.selected_chat().as_ref() == selected_chat {
                return;
            }

            self.disconnect_marked_as_unread_handler();

            match selected_chat {
                Some(chat) => {
                    let handler_id = chat.connect_notify_local(
                        Some("is-marked-as-unread"),
                        clone!(
                            #[weak(rename_to = obj)]
                            self,
                            move |chat, _| {
                                if chat.is_marked_as_unread() {
                                    obj.set_selected_chat(None);
                                }
                            }
                        ),
                    );
                    self.marked_as_unread_handler
                        .replace(Some((chat.clone(), handler_id)));

                    self.selection.set_selected_chat(Some(chat));

                    if chat.is_marked_as_unread() {
                        utils::spawn(clone!(
                            #[weak]
                            chat,
                            async move {
                                if let Err(e) = chat.mark_as_read().await {
                                    log::warn!("Error on toggling chat's unread state: {e:?}");
                                }
                            }
                        ));
                    }
                }
                None => self.selection.set_selected_chat(None),
            }

            self.selected_chat.set(selected_chat);

            obj.notify_selected_chat();
        }

        #[template_callback]
        fn on_notify_chat_list(&self) {
            let obj = self.obj();

            // Lazily load the chats of whichever list just became visible. `fetch`
            // is idempotent (it loads each list only once), so switching folders
            // back and forth is cheap, and folders that are never opened are never
            // loaded.
            if let Some(chat_list) = obj.chat_list() {
                chat_list.fetch();
            }

            self.selection
                .set_selected_chat(obj.selected_chat().as_ref());
        }

        #[template_callback]
        fn list_activate(&self, pos: u32) {
            let obj = &*self.obj();
            let previously_selected = obj.selected_chat();
            self.selection.set_selected_position(pos);

            obj.set_selected_chat(self.selection.selected_chat());

            // Reveal the chat. On a collapsed (mobile) layout this navigates away from
            // the sidebar; the chat rows live inside the sidebar's own AdwNavigationView,
            // so a bubbled "navigation.push" action would be swallowed by it instead of
            // reaching the outer split view — drive the split view through Session instead.
            //
            // Re-activating the chat that is already open drops the view to its newest
            // message (the set_selected_chat above is a no-op in that case).
            let newly_selected = obj.selected_chat();
            if let Some(session) = utils::ancestor::<_, ui::Session>(obj) {
                session.show_chat_content();
                if newly_selected.is_some() && newly_selected == previously_selected {
                    session.scroll_content_to_latest();
                }
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatList(ObjectSubclass<imp::ChatList>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
