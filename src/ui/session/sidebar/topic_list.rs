use std::cell::RefCell;

use glib::Properties;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::TopicList)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/topic_list.ui")]
    pub(crate) struct TopicList {
        /// The forum whose topic list is shown; binding it starts the lazy
        /// fetch of the chat's `ForumTopicList`.
        #[property(get, set = Self::set_chat, nullable, explicit_notify)]
        pub(super) chat: glib::WeakRef<model::Chat>,
        /// The bound chat's topic model, kept for activation lookups.
        pub(super) topics: RefCell<Option<model::ForumTopicList>>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TopicList {
        const NAME: &'static str = "PaplSidebarTopicList";
        type Type = super::TopicList;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            // Reuse the chat list's row styling (badges, padding) verbatim.
            klass.set_css_name("chatlist");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TopicList {
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
            let escape = gtk::EventControllerKey::new();
            let weak = self.obj().downgrade();
            escape.connect_key_pressed(move |_, key, _, _| {
                let Some(list) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if key == gdk::Key::Escape
                    && let Some(sidebar) = utils::ancestor::<_, ui::Sidebar>(&list)
                {
                    sidebar.pop_to_chats();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            self.obj().add_controller(escape);
        }
    }

    impl WidgetImpl for TopicList {}

    #[gtk::template_callbacks]
    impl TopicList {
        fn set_chat(&self, chat: Option<&model::Chat>) {
            let obj = self.obj();
            if self.chat.upgrade().as_ref() == chat {
                return;
            }
            self.chat.set(chat);
            match chat {
                Some(chat) => obj.bind_list(chat),
                None => {
                    *obj.imp().topics.borrow_mut() = None;
                    obj.imp().list_view.set_model(None::<&gtk::NoSelection>);
                }
            }
            obj.notify("chat");
        }

        #[template_callback]
        fn list_activate(&self, position: u32) {
            let obj = self.obj();
            let Some(chat) = obj.chat() else {
                return;
            };
            let Some(topic) = obj
                .imp()
                .list_view
                .model()
                .and_then(|model| model.item(position))
                .and_downcast::<model::Topic>()
            else {
                return;
            };
            if let Some(session) = utils::ancestor::<_, ui::Session>(&*obj) {
                session.open_topic(&chat, topic.forum_topic_id());
                session.show_chat_content();
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct TopicList(ObjectSubclass<imp::TopicList>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TopicList {
    /// Points the list at `chat`'s topic model; rows carry the full topic
    /// identity (colored emoji icon, preview, badges).
    fn bind_list(&self, chat: &model::Chat) {
        let imp = self.imp();
        let factory = topic_row_factory(chat);

        let model = chat.forum_topic_list();
        *self.imp().topics.borrow_mut() = Some(model.clone());
        // Hidden topics (General hidden above the list) stay in the model for
        // lookups but leave the visible rows.
        let filter = gtk::CustomFilter::new(|item| {
            !item
                .downcast_ref::<model::Topic>()
                .is_some_and(|topic| topic.is_hidden())
        });
        let filtered = gtk::FilterListModel::new(Some(model), Some(filter));
        let selection = gtk::NoSelection::new(Some(filtered));
        imp.list_view.set_model(Some(&selection));
        imp.list_view.set_factory(Some(&factory));
    }
}

/// A factory binding `model::Topic` items to full `TopicRow`s. Shared with the
/// forward dialog's topic step.
pub(crate) fn topic_row_factory(chat: &model::Chat) -> gtk::SignalListItemFactory {
    let chat = chat.clone();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        item.set_child(Some(&super::SidebarTopicRow::new()));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(row) = item.child().and_downcast::<super::SidebarTopicRow>() else {
            return;
        };
        let topic = item.item().and_downcast::<model::Topic>();
        row.set_topic(topic.as_ref(), Some(&chat));
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(row) = item.child().and_downcast::<super::SidebarTopicRow>() {
            row.set_topic(None, None);
        }
    });
    factory
}
