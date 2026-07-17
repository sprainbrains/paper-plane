use std::cell::RefCell;

use gettextrs::gettext;
use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/text.ui")]
    #[properties(wrapper_type = super::MessageText)]
    pub(crate) struct MessageText {
        pub(super) bindings: RefCell<Vec<gtk::ExpressionWatch>>,
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<glib::Object>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageText {
        const NAME: &'static str = "PaplMessageText";
        type Type = super::MessageText;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageText {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }

    impl MessageText {
        fn set_message(&self, message: Option<glib::Object>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessageText {}
    impl ui::MessageBaseImpl for MessageText {}
}

glib::wrapper! {
    pub(crate) struct MessageText(ObjectSubclass<imp::MessageText>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageText {
    type Message = glib::Object;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        let mut bindings = imp.bindings.borrow_mut();

        while let Some(binding) = bindings.pop() {
            binding.unwatch();
        }

        if let Some(message) = message.downcast_ref::<model::Message>() {
            imp.message_bubble.update_from_message(message, false);

            // Render the content as rich text (blocks, spoilers, custom emoji) rather
            // than a flat markup string: once now, and again whenever it changes (a
            // watch, unlike a bind, does not fire at creation).
            apply_rich_text(&imp.message_bubble, message);
            let text_binding = model::Message::this_expression("content").watch(
                Some(message),
                clone!(
                    #[weak(rename_to = bubble)]
                    imp.message_bubble,
                    #[weak]
                    message,
                    move || apply_rich_text(&bubble, &message)
                ),
            );
            bindings.push(text_binding);
        } else if let Some(sponsored_message) = message.downcast_ref::<model::SponsoredMessage>() {
            imp.message_bubble
                .update_from_sponsored_message(sponsored_message);

            // Set content label expression
            let text_binding = model::SponsoredMessage::this_expression("content")
                .chain_closure::<String>(closure!(
                    |_: model::SponsoredMessage, content: model::BoxedMessageContent| {
                        format_message_content_text(content.0)
                    }
                ))
                .bind(&*imp.message_bubble, "label", Some(sponsored_message));
            bindings.push(text_binding);
        } else {
            unreachable!("Unexpected message type: {:?}", message);
        }

        imp.message.set(Some(message));
        self.notify("message");
    }
}

fn apply_rich_text(bubble: &ui::MessageBubble, message: &model::Message) {
    match message.content().0 {
        tdlib::enums::MessageContent::MessageText(content) => {
            bubble.set_formatted_text(&content.text, message);
        }
        _ => bubble.set_label(unsupported_notice()),
    }
}

fn format_message_content_text(content: tdlib::enums::MessageContent) -> String {
    match content {
        tdlib::enums::MessageContent::MessageText(content) => {
            utils::parse_formatted_text(content.text)
        }
        _ => unsupported_notice(),
    }
}

/// TDLib reports content newer than the TDLib version this build links against as
/// `MessageUnsupported`; the client cannot render what TDLib does not decode.
fn unsupported_notice() -> String {
    format!(
        "<i>{}</i>",
        gettext("This message is not supported by your version of Paper Plane. Please update.")
    )
}
