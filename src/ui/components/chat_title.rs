use std::cell::OnceCell;
use std::sync::OnceLock;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::expressions;
use crate::model;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/components/chat_title.ui")]
    pub(crate) struct ChatTitle {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) chat_group: OnceCell<glib::SignalGroup>,
        pub(super) session_group: OnceCell<glib::SignalGroup>,
        /// Targets the chat's verifiable participant (a User or Supergroup); both
        /// expose `verification-status`, so the group type is the common Object.
        pub(super) verify_group: OnceCell<glib::SignalGroup>,
        #[template_child]
        pub(super) source_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) title_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) verified_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) scam_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) mute_icon: TemplateChild<gtk::Image>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatTitle {
        const NAME: &'static str = "PaplChatTitle";
        type Type = super::ChatTitle;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatTitle {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();
            match pspec.name() {
                "chat" => obj.set_chat(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();
            match pspec.name() {
                "chat" => obj.chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().create_signal_groups();
        }
    }

    impl WidgetImpl for ChatTitle {}

    impl BoxImpl for ChatTitle {}
}

glib::wrapper! {
    pub(crate) struct ChatTitle(ObjectSubclass<imp::ChatTitle>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ChatTitle {
    #[allow(unused)]
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    pub(crate) fn set_chat(&self, chat: Option<&model::Chat>) {
        if self.chat().as_ref() == chat {
            return;
        }

        let imp = self.imp();
        imp.chat.set(chat);

        // Retarget the observers. The chat type never changes for a Chat, so the
        // verifiable participant is resolved once here.
        imp.chat_group.get().unwrap().set_target(chat);
        let session = chat.map(|chat| chat.session_());
        imp.session_group
            .get()
            .unwrap()
            .set_target(session.as_ref());
        let verifiable: Option<glib::Object> = chat.and_then(|chat| match chat.chat_type() {
            model::ChatType::Private(user) => Some(user.upcast()),
            model::ChatType::Supergroup(supergroup) => Some(supergroup.upcast()),
            _ => None,
        });
        imp.verify_group
            .get()
            .unwrap()
            .set_target(verifiable.as_ref());

        self.update_all();
        self.notify("chat");
    }

    fn create_signal_groups(&self) {
        let imp = self.imp();

        let chat_group = glib::SignalGroup::new::<model::Chat>();
        chat_group.connect_notify_local(
            Some("title"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| obj.update_title()
            ),
        );
        chat_group.connect_notify_local(
            Some("source"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| obj.update_source()
            ),
        );
        chat_group.connect_notify_local(
            Some("notification-settings"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| obj.update_mute()
            ),
        );
        imp.chat_group.set(chat_group).ok();

        let session_group = glib::SignalGroup::new::<model::ClientStateSession>();
        for scope in [
            "private-chats-notification-settings",
            "group-chats-notification-settings",
            "channel-chats-notification-settings",
        ] {
            session_group.connect_notify_local(
                Some(scope),
                clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move |_, _| obj.update_mute()
                ),
            );
        }
        imp.session_group.set(session_group).ok();

        let verify_group = glib::SignalGroup::new::<glib::Object>();
        verify_group.connect_notify_local(
            Some("verification-status"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| obj.update_verification()
            ),
        );
        imp.verify_group.set(verify_group).ok();
    }

    fn update_all(&self) {
        self.update_title();
        self.update_source();
        self.update_verification();
        self.update_mute();
    }

    fn update_title(&self) {
        let imp = self.imp();
        match self.chat() {
            Some(chat) => imp
                .title_label
                .set_text(&expressions::chat_display_name_str(&chat)),
            None => imp.title_label.set_text(""),
        }
    }

    fn update_source(&self) {
        let imp = self.imp();
        let tooltip = match self.chat().and_then(|chat| chat.source()) {
            Some(model::BoxedChatSource(source)) => match source {
                tdlib::enums::ChatSource::PublicServiceAnnouncement(data) => {
                    Some((true, data.text))
                }
                tdlib::enums::ChatSource::MtprotoProxy => Some((true, String::new())),
            },
            None => None,
        };

        match tooltip {
            Some((visible, text)) => {
                imp.source_icon.set_visible(visible);
                imp.source_icon
                    .set_tooltip_text((!text.is_empty()).then_some(text.as_str()));
            }
            None => {
                imp.source_icon.set_visible(false);
                imp.source_icon.set_tooltip_text(None::<&str>);
            }
        }
    }

    fn update_verification(&self) {
        let imp = self.imp();
        let status = self.chat().and_then(|chat| chat.verification_status());

        let (verified, scam) = match status {
            Some(status) => (status.is_verified, status.is_scam || status.is_fake),
            None => (false, false),
        };

        // A scam/fake warning takes precedence over the plain verified check.
        imp.scam_icon.set_visible(scam);
        imp.verified_icon.set_visible(verified && !scam);
    }

    fn update_mute(&self) {
        let imp = self.imp();
        let muted = self
            .chat()
            .is_some_and(|chat| !chat.is_own_chat() && chat.is_muted());
        imp.mute_icon.set_visible(muted);
    }
}

impl Default for ChatTitle {
    fn default() -> Self {
        Self::new()
    }
}
