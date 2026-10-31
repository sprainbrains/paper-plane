use std::cell::Cell;
use std::cell::RefCell;

use adw::subclass::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;

use crate::model;
use crate::types::ChatId;
use crate::types::MessageId;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/forward_window.ui")]
    pub(crate) struct ForwardWindow {
        pub(super) session: RefCell<Option<ui::Session>>,
        pub(super) from_chat_id: Cell<ChatId>,
        pub(super) message_ids: RefCell<Vec<MessageId>>,
        pub(super) search_text: RefCell<String>,
        /// The forum whose topic step is currently shown (forward target).
        pub(super) forum_chat: RefCell<Option<model::Chat>>,
        #[template_child]
        pub(super) search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) forward_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) topic_list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) topics_back_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) filter_model: TemplateChild<gtk::FilterListModel>,
        #[template_child]
        pub(super) custom_filter: TemplateChild<gtk::CustomFilter>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ForwardWindow {
        const NAME: &'static str = "PaplForwardWindow";
        type Type = super::ForwardWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ForwardWindow {
        fn constructed(&self) {
            self.parent_constructed();

            // Match chats whose title contains the current search text.
            self.custom_filter.set_filter_func(clone!(
                #[weak(rename_to = obj)]
                self,
                #[upgrade_or]
                false,
                move |item| {
                    let search = obj.search_text.borrow().to_lowercase();
                    if search.is_empty() {
                        return true;
                    }
                    item.downcast_ref::<model::ChatListItem>()
                        .map(|item| item.chat_().title().to_lowercase().contains(&search))
                        .unwrap_or(false)
                }
            ));
        }
    }

    impl WidgetImpl for ForwardWindow {}
    impl WindowImpl for ForwardWindow {}
    impl AdwWindowImpl for ForwardWindow {}

    #[gtk::template_callbacks]
    impl ForwardWindow {
        #[template_callback]
        fn search_changed(&self) {
            self.search_text
                .replace(self.search_entry.text().to_string());
            self.custom_filter.changed(gtk::FilterChange::Different);
        }

        #[template_callback]
        fn list_activate(&self, pos: u32) {
            let obj = self.obj();
            let Some(item) = self
                .list_view
                .model()
                .and_then(|model| model.item(pos))
                .and_downcast::<model::ChatListItem>()
            else {
                return;
            };

            let chat = item.chat_();
            // A topics-mode forum asks *which* topic to forward into first
            // (tdesktop); unified-mode forums forward at chat level.
            if chat.is_forum() && chat.view_as_topics() {
                obj.enter_topic_step(&chat);
                return;
            }
            obj.forward_to(chat.id(), None);
            obj.close();
        }

        #[template_callback]
        fn topic_activate(&self, pos: u32) {
            let obj = self.obj();
            let Some(chat) = self.forum_chat.borrow().clone() else {
                return;
            };
            let Some(topic) = self
                .topic_list_view
                .model()
                .and_then(|model| model.item(pos))
                .and_downcast::<model::Topic>()
            else {
                return;
            };
            let topic = tdlib::enums::MessageTopic::Forum(tdlib::types::MessageTopicForum {
                forum_topic_id: topic.forum_topic_id(),
            });
            obj.forward_to(chat.id(), Some(topic));
            obj.close();
        }

        #[template_callback]
        fn back_to_chats(&self) {
            self.forward_stack.set_visible_child_name("chats");
            self.topics_back_button.set_visible(false);
            self.search_entry.set_sensitive(true);
        }
    }
}

glib::wrapper! {
    pub(crate) struct ForwardWindow(ObjectSubclass<imp::ForwardWindow>)
        @extends gtk::Widget, gtk::Window, adw::Window,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl ForwardWindow {
    pub(crate) fn new(
        parent: Option<&gtk::Window>,
        session: &ui::Session,
        from_chat_id: ChatId,
        message_ids: Vec<MessageId>,
    ) -> Self {
        let obj: Self = glib::Object::builder()
            .property("transient-for", parent)
            .build();

        let imp = obj.imp();
        imp.session.replace(Some(session.clone()));
        imp.from_chat_id.set(from_chat_id);
        imp.message_ids.replace(message_ids);

        if let Some(model) = session.model() {
            imp.filter_model.set_model(Some(&model.main_chat_list()));
        }

        obj
    }

    /// Swaps the picker to the chosen forum's topic step: full topic rows over
    /// the (lazily fetched) topic list; chat search is meaningless there.
    fn enter_topic_step(&self, chat: &model::Chat) {
        let imp = self.imp();
        imp.forum_chat.replace(Some(chat.clone()));
        let list = chat.forum_topic_list();
        imp.topic_list_view
            .set_model(Some(&gtk::NoSelection::new(Some(list))));
        imp.topic_list_view
            .set_factory(Some(&super::super::sidebar::topic_row_factory(chat)));
        imp.forward_stack.set_visible_child_name("topics");
        imp.topics_back_button.set_visible(true);
        imp.search_entry.set_sensitive(false);
    }

    fn forward_to(&self, to_chat_id: ChatId, topic: Option<tdlib::enums::MessageTopic>) {
        let imp = self.imp();
        let Some(session) = imp.session.borrow().clone() else {
            return;
        };
        let Some(model) = session.model() else {
            return;
        };
        let client_id = model.client_().id();
        let from_chat_id = imp.from_chat_id.get();
        let message_ids = imp.message_ids.borrow().clone();

        utils::spawn(async move {
            if let Err(e) = tdlib::functions::forward_messages(
                to_chat_id,
                topic,
                from_chat_id,
                message_ids,
                None,
                false,
                false,
                client_id,
            )
            .await
            {
                log::warn!("Failed to forward a message: {e:?}");
            }
        });
    }
}
