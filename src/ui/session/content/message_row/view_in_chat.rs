use std::cell::Cell;

use gettextrs::gettext;
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

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/view_in_chat.ui")]
    pub(crate) struct MessageViewInChatBar {
        #[template_child]
        pub(super) label: TemplateChild<gtk::Label>,
        /// The original message this bar jumps to; `(0, 0)` while unbound.
        pub(super) target: Cell<(i64, i64)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageViewInChatBar {
        const NAME: &'static str = "PaplMessageViewInChatBar";
        type Type = super::MessageViewInChatBar;
        type ParentType = gtk::Button;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageViewInChatBar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_hexpand(true);
            obj.set_visible(false);
            self.label.set_label(&gettext("View in Chat"));
            obj.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.open_target()
            ));
        }
    }

    impl WidgetImpl for MessageViewInChatBar {}

    impl ButtonImpl for MessageViewInChatBar {}
}

glib::wrapper! {
    pub(crate) struct MessageViewInChatBar(ObjectSubclass<imp::MessageViewInChatBar>)
        @extends gtk::Widget, gtk::Button,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageViewInChatBar {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Binds the bar to a message: shown only when the message mirrors an
    /// original living in another chat (`forward_info.source`).
    pub(crate) fn set_message(&self, message: Option<&model::Message>) {
        let target =
            message.and_then(|message| message.forward_info()?.view_in_chat(message.chat_().id()));

        match target {
            Some((chat_id, message_id)) => {
                self.imp().target.set((chat_id, message_id));
                self.set_visible(true);
            }
            None => {
                self.imp().target.set((0, 0));
                self.set_visible(false);
            }
        }
    }

    fn open_target(&self) {
        let (chat_id, message_id) = self.imp().target.get();
        if chat_id != 0
            && let Some(session) = utils::ancestor::<_, ui::Session>(self)
        {
            session.open_message_origin(chat_id, message_id);
        }
    }
}

impl Default for MessageViewInChatBar {
    fn default() -> Self {
        Self::new()
    }
}
