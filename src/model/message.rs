use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;

use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::expressions;
use crate::model;
use crate::types::MessageSenderId;

#[derive(Clone, Debug, glib::Boxed)]
#[boxed_type(name = "MessageSender")]
pub(crate) enum MessageSender {
    User(model::User),
    Chat(model::Chat),
}

impl MessageSender {
    pub(crate) fn new(
        session: &model::ClientStateSession,
        sender: &tdlib::enums::MessageSender,
    ) -> Self {
        use tdlib::enums::MessageSender::*;

        match sender {
            User(data) => Self::User(session.user_or_fetch(data.user_id)),
            Chat(data) => Self::Chat(session.chat_or_placeholder(data.chat_id)),
        }
    }

    pub(crate) fn as_user(&self) -> Option<&model::User> {
        match self {
            Self::User(user) => Some(user),
            _ => None,
        }
    }

    pub(crate) fn id(&self) -> MessageSenderId {
        match self {
            Self::User(user) => user.id(),
            Self::Chat(chat) => chat.id(),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::Message)]
    pub(crate) struct Message {
        #[property(get, set, construct_only)]
        pub(super) chat: glib::WeakRef<model::Chat>,
        #[property(get, set, construct_only)]
        pub(super) id: Cell<i64>,
        #[property(get, set, construct_only)]
        pub(super) sender: OnceCell<MessageSender>,
        #[property(get, set, construct_only)]
        pub(super) is_outgoing: Cell<bool>,
        #[property(get, set, construct_only)]
        pub(super) can_be_edited: Cell<bool>,
        #[property(get, set, construct_only)]
        pub(super) can_be_deleted_only_for_self: Cell<bool>,
        #[property(get, set, construct_only)]
        pub(super) can_be_deleted_for_all_users: Cell<bool>,
        #[property(get, set, construct_only)]
        pub(super) sending_state: RefCell<Option<model::BoxedMessageSendingState>>,
        #[property(get, set, construct_only)]
        pub(super) date: Cell<i32>,
        #[property(get, set, construct_only)]
        pub(super) interaction_info: OnceCell<model::MessageInteractionInfo>,
        #[property(get, set, construct_only)]
        pub(super) forward_info: RefCell<Option<model::MessageForwardInfo>>,
        #[property(get, set, construct_only)]
        pub(super) reply_to: RefCell<Option<model::BoxedMessageReplyTo>>,
        #[property(get, set, construct_only, nullable)]
        pub(super) reply_markup: RefCell<Option<model::BoxedReplyMarkup>>,
        /// Album grouping key; 0 when the message is not part of a media album.
        #[property(get, set, construct_only)]
        pub(super) media_album_id: Cell<i64>,
        #[property(get)]
        pub(super) content: RefCell<model::BoxedMessageContent>,
        #[property(get)]
        pub(super) is_edited: Cell<bool>,
        #[property(get)]
        pub(super) is_pinned: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Message {
        const NAME: &'static str = "Message";
        type Type = super::Message;
    }

    impl ObjectImpl for Message {
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
}

glib::wrapper! {
    pub(crate) struct Message(ObjectSubclass<imp::Message>);
}

impl Message {
    pub(crate) fn new(chat: &model::Chat, td_message: tdlib::types::Message) -> Self {
        let obj: Self = glib::Object::builder()
            .property("chat", chat)
            .property("id", td_message.id)
            .property(
                "sender",
                model::MessageSender::new(&chat.session_(), &td_message.sender_id),
            )
            .property("is-outgoing", td_message.is_outgoing)
            // TODO: `can_be_edited`/`can_be_deleted_only_for_self`/
            // `can_be_deleted_for_all_users` were removed from `message` in newer
            // TDLib; they now come from `getMessageProperties`. Until that async
            // lookup is wired up, approximate from `is_outgoing` — except that a
            // chat with oneself has no "everyone", so revoke-delete never applies
            // to Saved Messages.
            .property("can-be-edited", td_message.is_outgoing)
            .property("can-be-deleted-only-for-self", true)
            .property(
                "can-be-deleted-for-all-users",
                td_message.is_outgoing && !chat.is_own_chat(),
            )
            .property(
                "sending-state",
                td_message
                    .sending_state
                    .map(model::BoxedMessageSendingState),
            )
            .property("date", td_message.date)
            .property(
                "interaction-info",
                model::MessageInteractionInfo::from(td_message.interaction_info),
            )
            .property(
                "forward-info",
                td_message
                    .forward_info
                    .map(|forward_info| model::MessageForwardInfo::new(chat, forward_info)),
            )
            .property(
                "reply-to",
                td_message.reply_to.map(model::BoxedMessageReplyTo),
            )
            .property("media-album-id", td_message.media_album_id)
            .property(
                "reply-markup",
                td_message.reply_markup.map(model::BoxedReplyMarkup),
            )
            .build();

        let imp = obj.imp();

        imp.content
            .replace(model::BoxedMessageContent(td_message.content));
        imp.is_edited.set(td_message.edit_date > 0);
        imp.is_pinned.set(td_message.is_pinned);

        obj
    }

    weak_getter!(chat_, chat, model::Chat);

    pub(crate) fn update_from_td_message(&self, td_message: tdlib::types::Message) {
        let imp = self.imp();

        if self.id() != td_message.id {
            imp.id.set(td_message.id);
            self.notify_id();
        }

        if self.is_outgoing() != td_message.is_outgoing {
            imp.is_outgoing.set(td_message.is_outgoing);
            self.notify_is_outgoing();
        }

        // See the constructor's note about these approximated properties.
        if self.can_be_edited() != td_message.is_outgoing {
            imp.can_be_edited.set(td_message.is_outgoing);
            self.notify_can_be_edited();
        }
        if !self.can_be_deleted_only_for_self() {
            imp.can_be_deleted_only_for_self.set(true);
            self.notify_can_be_deleted_only_for_self();
        }
        let can_revoke = td_message.is_outgoing && !self.chat_().is_own_chat();
        if self.can_be_deleted_for_all_users() != can_revoke {
            imp.can_be_deleted_for_all_users.set(can_revoke);
            self.notify_can_be_deleted_for_all_users();
        }

        imp.sending_state.replace(
            td_message
                .sending_state
                .map(model::BoxedMessageSendingState),
        );
        self.notify_sending_state();

        if self.date() != td_message.date {
            imp.date.set(td_message.date);
            self.notify_date();
        }

        self.interaction_info().update(td_message.interaction_info);
        imp.forward_info.replace(
            td_message
                .forward_info
                .map(|forward_info| model::MessageForwardInfo::new(&self.chat_(), forward_info)),
        );
        self.notify_forward_info();

        let reply_to = td_message.reply_to.map(model::BoxedMessageReplyTo);
        if self.reply_to() != reply_to {
            imp.reply_to.replace(reply_to);
            self.notify_reply_to();
        }

        // A bot editing only its inline keyboard keeps the content identical, so
        // the markup notify must fire unconditionally for the bubble to rebuild.
        imp.reply_markup
            .replace(td_message.reply_markup.map(model::BoxedReplyMarkup));
        self.notify_reply_markup();

        self.set_content(model::BoxedMessageContent(td_message.content));
        self.set_is_edited(td_message.edit_date > 0);
        self.set_is_pinned(td_message.is_pinned);
    }

    pub(crate) fn handle_update(&self, update: tdlib::enums::Update) {
        use tdlib::enums::Update::*;

        match update {
            MessageContent(data) => {
                let new_content = model::BoxedMessageContent(data.new_content);
                self.set_content(new_content);
            }
            MessageEdited(data) => self.set_is_edited(data.edit_date > 0),
            MessageIsPinned(data) => self.set_is_pinned(data.is_pinned),
            MessageInteractionInfo(data) => self.interaction_info().update(data.interaction_info),
            _ => {}
        }
    }

    fn set_content(&self, content: model::BoxedMessageContent) {
        if self.content() == content {
            return;
        }
        self.imp().content.replace(content);
        self.notify_content();
    }

    fn set_is_edited(&self, is_edited: bool) {
        if self.is_edited() == is_edited {
            return;
        }
        self.imp().is_edited.set(is_edited);
        self.notify_is_edited();
    }

    fn set_is_pinned(&self, is_pinned: bool) {
        if self.is_pinned() == is_pinned {
            return;
        }
        self.imp().is_pinned.set(is_pinned);
        self.notify_is_pinned();
    }

    /// The forwarder this message is visually attributed to (avatar and bubble
    /// grouping): in Saved Messages any forward; in private and secret chats
    /// only forwards from someone other than the chat's peer — the peer's own
    /// messages need no avatar anchor (tdesktop). `None` means attribute to
    /// the sender. Kept in sync with `ChatHistoryModel::group_identity`.
    pub(crate) fn attributed_forward_origin(&self) -> Option<model::MessageForwardOrigin> {
        let origin = self.forward_info()?.origin();
        let chat = self.chat_();
        if chat.is_own_chat() {
            return Some(origin);
        }
        let peer = match chat.chat_type() {
            model::ChatType::Private(user) => user.id(),
            model::ChatType::Secret(secret) => secret.user()?.id(),
            _ => return None,
        };
        (!matches!(
            &origin,
            model::MessageForwardOrigin::User(user) if user.id() == peer
        ))
        .then_some(origin)
    }

    pub(crate) fn sender_name_expression(&self) -> gtk::Expression {
        match self.sender() {
            MessageSender::User(user) => {
                let user_expression = gtk::ConstantExpression::new(user);
                expressions::user_display_name(&user_expression)
            }
            MessageSender::Chat(chat) => gtk::ConstantExpression::new(chat)
                .chain_property::<model::Chat>("title")
                .upcast(),
        }
    }

    pub(crate) fn sender_display_name_expression(&self) -> gtk::Expression {
        if self.chat_().is_own_chat() {
            self.forward_info()
                .map(|forward_info| forward_info.origin())
                .map(|forward_origin| match forward_origin {
                    model::MessageForwardOrigin::User(user) => {
                        let user_expression = gtk::ObjectExpression::new(&user);
                        expressions::user_display_name(&user_expression)
                    }
                    model::MessageForwardOrigin::Chat { chat, .. }
                    | model::MessageForwardOrigin::Channel { chat, .. } => {
                        gtk::ConstantExpression::new(chat)
                            .chain_property::<model::Chat>("title")
                            .upcast()
                    }
                    model::MessageForwardOrigin::HiddenUser { sender_name }
                    | model::MessageForwardOrigin::MessageImport { sender_name } => {
                        gtk::ConstantExpression::new(sender_name).upcast()
                    }
                })
                .unwrap_or_else(|| self.sender_name_expression())
        } else {
            self.sender_name_expression()
        }
    }
}
