use std::cell::Cell;
use std::cell::OnceCell;
use std::sync::OnceLock;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::expressions;
use crate::model;
use crate::ui;
use crate::utils;

/// Longer-side size, in pixels, of the leading external-verifier badge.
const VERIFIER_SIZE: i32 = 18;

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
        /// The custom-emoji id currently shown (or requested) as the external
        /// verifier, so an in-flight fetch that raced a retarget is dropped.
        pub(super) verifier_id: Cell<i64>,
        #[template_child]
        pub(super) verifier_icon: TemplateChild<ui::Sticker>,
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

    /// Trailing Telegram check / scam-fake badge, plus the leading external
    /// (bot-provided) verifier badge — all read from the same verification status.
    fn update_verification(&self) {
        let imp = self.imp();
        let status = self.chat().and_then(|chat| chat.verification_status());

        let (verified, scam, verifier_emoji) = match &status {
            Some(status) => (
                status.is_verified,
                status.is_scam || status.is_fake,
                status.bot_verification_icon_custom_emoji_id,
            ),
            None => (false, false, 0),
        };

        // A scam/fake warning takes precedence over the plain verified check.
        imp.scam_icon.set_visible(scam);
        imp.verified_icon.set_visible(verified && !scam);

        self.update_verifier(verifier_emoji);
    }

    /// Loads the external verifier's custom emoji into the leading badge. A zero
    /// id (or no chat) hides it. The stored id guards against a fetch that raced a
    /// retarget to another chat.
    fn update_verifier(&self, emoji_id: i64) {
        let imp = self.imp();
        imp.verifier_id.set(emoji_id);

        if emoji_id == 0 {
            imp.verifier_icon.set_visible(false);
            return;
        }

        let Some(chat) = self.chat() else {
            imp.verifier_icon.set_visible(false);
            return;
        };
        let session = chat.session_();
        let client_id = session.client_().id();
        imp.verifier_icon.set_visible(true);
        imp.verifier_icon.set_longer_side_size(VERIFIER_SIZE);

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let Ok(tdlib::enums::Stickers::Stickers(stickers)) =
                    tdlib::functions::get_custom_emoji_stickers(vec![emoji_id], client_id).await
                else {
                    return;
                };
                // Drop the result if the widget now shows a different verifier.
                if obj.imp().verifier_id.get() != emoji_id {
                    return;
                }
                if let Some(sticker) = stickers.stickers.into_iter().next() {
                    obj.imp()
                        .verifier_icon
                        .update_sticker(sticker, true, session);
                }
            }
        ));
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
