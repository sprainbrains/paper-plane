mod file_status;
mod row;
mod status_indicator;

use std::cell::RefCell;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

pub(crate) use self::row::DocumentRow;
pub(crate) use self::status_indicator::StatusIndicator;
use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/document/mod.ui")]
    #[properties(wrapper_type = super::MessageDocument)]
    pub(crate) struct MessageDocument {
        pub(super) handler_id: RefCell<Option<glib::SignalHandlerId>>,
        #[property(
            type = Option<glib::Object>,
            get = Self::message,
            set = Self::set_message,
            explicit_notify,
            nullable
        )]
        pub(super) message: glib::WeakRef<model::Message>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) row: TemplateChild<ui::DocumentRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageDocument {
        const NAME: &'static str = "PaplMessageDocument";
        type Type = super::MessageDocument;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageDocument {
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
            if let Some(handler_id) = self.handler_id.take()
                && let Some(message) = self.message.upgrade()
            {
                message.disconnect(handler_id);
            }
        }
    }

    impl WidgetImpl for MessageDocument {}

    impl ui::MessageBaseImpl for MessageDocument {}

    impl MessageDocument {
        fn message(&self) -> Option<glib::Object> {
            self.message.upgrade().map(|message| message.upcast())
        }

        fn set_message(&self, message: Option<glib::Object>) {
            let message = message.unwrap().downcast::<model::Message>().unwrap();
            ui::MessageBaseExt::set_message(&*self.obj(), &message);
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageDocument(ObjectSubclass<imp::MessageDocument>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageDocument {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        if let Some(handler_id) = imp.handler_id.take()
            && let Some(previous_message) = imp.message.upgrade()
        {
            previous_message.disconnect(handler_id);
        }

        imp.message.set(Some(message));
        imp.message_bubble.update_from_message(message, false);
        imp.row.set_message(message);

        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                obj.update_caption(message);
            }
        ));
        imp.handler_id.replace(Some(handler_id));

        self.update_caption(message);

        self.notify("message");
    }
}

impl MessageDocument {
    /// The bubble label mirrors the document's caption; the file row itself
    /// (thumbnail, name, size, click) is a shared [`ui::DocumentRow`].
    fn update_caption(&self, message: &model::Message) {
        if let tdlib::enums::MessageContent::MessageDocument(data) = message.content().0 {
            let message_text = utils::parse_formatted_text(data.caption);
            self.imp().message_bubble.set_label(message_text);
        }
    }
}
