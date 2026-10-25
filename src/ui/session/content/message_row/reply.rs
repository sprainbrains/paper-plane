use std::cell::Cell;
use std::cell::RefCell;

use gettextrs::gettext;
use glib::ParamSpec;
use glib::Properties;
use glib::Value;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::MessageReply)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/reply.ui")]
    pub(crate) struct MessageReply {
        pub(super) sender_color_class: RefCell<Option<String>>,
        pub(super) bindings: RefCell<Vec<gtk::ExpressionWatch>>,
        /// The replied-to message id when it can be jumped to (0 otherwise), so
        /// clicking the preview can jump to it.
        pub(super) replied_message_id: Cell<i64>,
        /// The chat the replied-to message lives in; when it differs from the
        /// viewing chat the jump switches chats (Replies chat quotes).
        pub(super) replied_chat_id: Cell<i64>,

        #[property(get, set, construct_only)]
        pub(super) message: glib::WeakRef<model::Message>,

        #[template_child]
        pub(super) separator: TemplateChild<gtk::Separator>,
        #[template_child]
        pub(super) labels_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) sender_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) message_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageReply {
        const NAME: &'static str = "PaplMessageReply";
        type Type = super::MessageReply;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_css_name("messagereply");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageReply {
        fn properties() -> &'static [ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &Value, pspec: &ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &ParamSpec) -> Value {
            self.derived_property(id, pspec)
        }

        fn constructed(&self) {
            self.message_label.set_label(&gettext("Loading…"));

            let obj = self.obj();

            // Clicking the reply preview jumps to the original message.
            obj.set_cursor_from_name(Some("pointer"));
            let click = gtk::GestureClick::new();
            click.connect_released(clone!(
                #[weak]
                obj,
                move |gesture, _, _, _| {
                    let message_id = obj.imp().replied_message_id.get();
                    let chat_id = obj.imp().replied_chat_id.get();
                    if message_id != 0
                        && let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(&obj)
                    {
                        let viewing_same_chat = chat_id == 0
                            || chat_history.chat().is_some_and(|chat| chat.id() == chat_id);
                        if viewing_same_chat {
                            chat_history.focus_message(message_id);
                        } else if let Some(session) = utils::ancestor::<_, ui::Session>(&obj) {
                            session.select_chat_and_reveal(chat_id, message_id);
                        }
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    }
                }
            ));
            obj.add_controller(click);

            utils::spawn(clone!(
                #[weak]
                obj,
                async move {
                    obj.load_replied_message().await;
                }
            ));
        }

        fn dispose(&self) {
            self.dispose_template();
        }
    }

    impl WidgetImpl for MessageReply {}
}

glib::wrapper! {
    pub(crate) struct MessageReply(ObjectSubclass<imp::MessageReply>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageReply {
    pub(crate) fn new(message: &model::Message) -> Self {
        glib::Object::builder().property("message", message).build()
    }

    async fn load_replied_message(&self) {
        let imp = self.imp();

        let message = self.message().unwrap();
        let is_outgoing = message.is_outgoing();

        match message.reply_to().unwrap().0 {
            tdlib::enums::MessageReplyTo::Message(reply_to) => {
                let chat = if reply_to.chat_id != 0 {
                    message.chat_().session_().try_chat(reply_to.chat_id)
                } else {
                    Some(message.chat_())
                };
                // The origin chat may be unknown to this account (left, or never
                // loaded): the Replies chat quotes replies from arbitrary chats.
                // Keep the quote inert instead of crashing on it.
                let Some(chat) = chat else {
                    imp.message_label.set_label(&gettext("Deleted message"));
                    return;
                };

                // Same-chat replies focus in place; cross-chat ones (the Replies
                // chat mirrors replies from other chats) switch chats first.
                imp.replied_message_id.set(reply_to.message_id);
                imp.replied_chat_id.set(if reply_to.chat_id != 0 {
                    reply_to.chat_id
                } else {
                    message.chat_().id()
                });

                match chat.fetch_message(reply_to.message_id).await {
                    Ok(message) => self.update_from_message(&message, is_outgoing),
                    Err(_) => imp.message_label.set_label(&gettext("Deleted message")),
                };
            }
            tdlib::enums::MessageReplyTo::Story(_) => {
                imp.message_label.set_label(&gettext("Story"));
            }
        };
    }

    pub(crate) fn set_max_char_width(&self, n_chars: i32) {
        self.imp().message_label.set_max_width_chars(n_chars);
        self.imp().sender_label.set_max_width_chars(n_chars);
    }

    fn update_from_message(&self, replied_message: &model::Message, is_outgoing: bool) {
        let imp = self.imp();
        let mut bindings = imp.bindings.borrow_mut();
        while let Some(binding) = bindings.pop() {
            binding.unwatch();
        }

        // Remove the previous color css class
        let mut sender_color_class = imp.sender_color_class.borrow_mut();
        if let Some(class) = sender_color_class.as_ref() {
            self.remove_css_class(class);
        }
        // Show sender label, if needed
        let show_sender = !matches!(
            replied_message.chat_().chat_type(),
            model::ChatType::Supergroup(data) if data.is_channel()
        );
        if show_sender {
            let sender_name_expression = replied_message.sender_name_expression();
            let sender_binding =
                sender_name_expression.bind(&*imp.sender_label, "label", glib::Object::NONE);

            bindings.push(sender_binding);

            if !is_outgoing {
                // Color sender label
                if let model::MessageSender::User(user) = replied_message.sender() {
                    let classes = &[
                        "sender-text-red",
                        "sender-text-orange",
                        "sender-text-violet",
                        "sender-text-green",
                        "sender-text-cyan",
                        "sender-text-blue",
                        "sender-text-pink",
                    ];

                    let color_class = classes[user.id() as usize % classes.len()];
                    self.add_css_class(color_class);

                    *sender_color_class = Some(color_class.into());
                }
            }
            imp.sender_label.set_visible(true);
        }

        // Set content label expression

        let caption = strings::message_content(replied_message.clone().as_ref());
        imp.message_label.set_label(&caption);
    }
}
