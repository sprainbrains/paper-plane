use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use gettextrs::gettext;
use glib::Properties;
use glib::clone;
use glib::subclass::Signal;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::types::ChatId;
use crate::types::MessageId;
use crate::utils;

/// Telegram's system "Replies" bot user id (global, not per account). TDLib
/// delivers the chat title untranslated, so the client localizes it.
const REPLIES_BOT_USER_ID: ChatId = 1271266957;

#[derive(Clone, Debug, glib::Boxed)]
#[boxed_type(name = "ChatType")]
pub(crate) enum ChatType {
    Private(model::User),
    BasicGroup(model::BasicGroup),
    Supergroup(model::Supergroup),
    Secret(model::SecretChat),
}

impl ChatType {
    pub(crate) fn from_td_object(
        _type: &tdlib::enums::ChatType,
        session: &model::ClientStateSession,
    ) -> Self {
        use tdlib::enums::ChatType::*;

        match _type {
            Private(data) => {
                let user = session.user_or_fetch(data.user_id);
                Self::Private(user)
            }
            BasicGroup(data) => {
                // The type-before-id guarantee covers the update stream but not
                // direct request results; a placeholder keeps chat
                // construction from panicking and converges when the real
                // object arrives.
                let basic_group = session.basic_group_or_fetch(data.basic_group_id);
                Self::BasicGroup(basic_group)
            }
            Supergroup(data) => {
                let supergroup = session.supergroup_or_fetch(data.supergroup_id, data.is_channel);
                Self::Supergroup(supergroup)
            }
            Secret(data) => {
                let secret_chat = session.secret_chat(data.secret_chat_id.into());
                Self::Secret(secret_chat)
            }
        }
    }

    pub(crate) fn user(&self) -> Option<model::User> {
        Some(match self {
            ChatType::Private(user) => user.to_owned(),
            ChatType::Secret(secret_chat) => secret_chat.user_(),
            _ => return None,
        })
    }

    pub(crate) fn basic_group(&self) -> Option<&model::BasicGroup> {
        Some(match self {
            ChatType::BasicGroup(basic_group) => basic_group,
            _ => return None,
        })
    }

    pub(crate) fn supergroup(&self) -> Option<&model::Supergroup> {
        Some(match self {
            ChatType::Supergroup(supergroup) => supergroup,
            _ => return None,
        })
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::Chat)]
    pub(crate) struct Chat {
        pub(super) messages: RefCell<HashMap<i64, model::Message>>,
        #[property(get, set, construct_only)]
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        #[property(get, set, construct_only)]
        pub(super) id: Cell<ChatId>,
        #[property(get, set, construct_only)]
        pub(super) chat_type: OnceCell<ChatType>,
        #[property(get)]
        pub(super) block_list: RefCell<Option<model::BoxedBlockList>>,
        #[property(get)]
        pub(super) title: RefCell<String>,
        #[property(get)]
        pub(super) avatar: RefCell<Option<model::Avatar>>,
        #[property(get)]
        pub(super) last_read_outbox_message_id: Cell<MessageId>,
        #[property(get)]
        pub(super) last_read_inbox_message_id: Cell<MessageId>,
        #[property(get)]
        pub(super) is_marked_as_unread: Cell<bool>,
        #[property(get)]
        pub(super) last_message: RefCell<Option<model::Message>>,
        #[property(get)]
        pub(super) unread_mention_count: Cell<i32>,
        #[property(get)]
        pub(super) unread_count: Cell<i32>,
        #[property(get)]
        pub(super) draft_message: RefCell<Option<model::BoxedDraftMessage>>,
        #[property(get)]
        pub(super) notification_settings: RefCell<model::BoxedChatNotificationSettings>,
        #[property(get = Self::actions)]
        pub(super) actions: OnceCell<model::ChatActionList>,
        #[property(get)]
        pub(super) permissions: RefCell<model::BoxedChatPermissions>,
        /// Which reactions the chat allows (`All` vs a restricted `Some(list)`); used to
        /// constrain the reaction picker so it never offers a disallowed reaction.
        pub(super) available_reactions: RefCell<Option<tdlib::enums::ChatAvailableReactions>>,
        /// Bot commands available in this chat (private bot chat, or the bots of
        /// a group), fetched on chat switch; drives the composer's menu button
        /// and '/'-completion.
        #[property(get = Self::bot_commands)]
        pub(super) bot_commands: RefCell<model::BoxedBotCommands>,
        /// The bot's `botMenuButton`; when it carries a URL the menu button
        /// opens that Web App instead of the command list.
        #[property(get = Self::bot_menu_button, nullable)]
        pub(super) bot_menu_button: RefCell<Option<model::BoxedBotMenuButton>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Chat {
        const NAME: &'static str = "Chat";
        type Type = super::Chat;
    }

    impl ObjectImpl for Chat {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("new-message")
                        .param_types([model::Message::static_type()])
                        .build(),
                    Signal::builder("deleted-message")
                        .param_types([model::Message::static_type()])
                        .build(),
                ]
            })
        }

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

    impl Chat {
        pub(crate) fn actions(&self) -> model::ChatActionList {
            self.actions
                .get_or_init(|| model::ChatActionList::from(&*self.obj()))
                .to_owned()
        }

        pub(crate) fn bot_commands(&self) -> model::BoxedBotCommands {
            self.bot_commands.borrow().clone()
        }

        pub(crate) fn bot_menu_button(&self) -> Option<model::BoxedBotMenuButton> {
            self.bot_menu_button.borrow().clone()
        }
    }
}

glib::wrapper! {
    pub(crate) struct Chat(ObjectSubclass<imp::Chat>);
}

impl Chat {
    pub(crate) fn new(session: &model::ClientStateSession, td_chat: tdlib::types::Chat) -> Self {
        let obj: Self = glib::Object::builder()
            .property("session", session)
            .property("id", td_chat.id)
            .property(
                "chat-type",
                ChatType::from_td_object(&td_chat.r#type, session),
            )
            .build();

        let imp = obj.imp();

        imp.block_list
            .replace(td_chat.block_list.map(model::BoxedBlockList));
        imp.title.replace(obj.localized_title(td_chat.title));
        imp.avatar.replace(td_chat.photo.map(model::Avatar::from));
        imp.last_read_outbox_message_id
            .set(td_chat.last_read_outbox_message_id);
        imp.last_read_inbox_message_id
            .set(td_chat.last_read_inbox_message_id);
        imp.is_marked_as_unread.set(td_chat.is_marked_as_unread);
        imp.last_message.replace(
            td_chat
                .last_message
                .map(|message| obj.upsert_message(message)),
        );
        imp.unread_mention_count.set(td_chat.unread_mention_count);
        imp.unread_count.set(td_chat.unread_count);
        imp.draft_message
            .replace(td_chat.draft_message.map(model::BoxedDraftMessage));
        imp.notification_settings
            .replace(model::BoxedChatNotificationSettings(
                td_chat.notification_settings,
            ));
        imp.permissions
            .replace(model::BoxedChatPermissions(td_chat.permissions));
        imp.available_reactions
            .replace(Some(td_chat.available_reactions));

        obj
    }

    /// Placeholder for a chat referenced (as a message sender) before TDLib
    /// announced it. Sender chats are always supergroups or channels, so the
    /// id range identifies the type; `update_from_td` completes the object in
    /// place once the real announcement arrives.
    pub(crate) fn new_placeholder_for_sender(
        session: &model::ClientStateSession,
        chat_id: ChatId,
    ) -> Self {
        let (supergroup_id, is_channel) = if chat_id < -1000000000000000 {
            (-(chat_id + 1000000000000000), true)
        } else {
            (-(chat_id + 1000000000000), false)
        };
        let chat_type =
            ChatType::Supergroup(session.supergroup_or_fetch(supergroup_id, is_channel));
        glib::Object::builder()
            .property("session", session)
            .property("id", chat_id)
            .property("chat-type", chat_type)
            .build()
    }

    /// Applies a real `tdlib::types::Chat` to an existing (placeholder) object
    /// so everything already bound to it — messages, rows, headers — converges
    /// on the true data without orphaning references.
    pub(crate) fn update_from_td(&self, td_chat: tdlib::types::Chat) {
        self.set_block_list(td_chat.block_list.map(model::BoxedBlockList));
        self.set_title(self.localized_title(td_chat.title));
        self.set_avatar(td_chat.photo.map(model::Avatar::from));
        self.set_last_read_outbox_message_id(td_chat.last_read_outbox_message_id);
        self.set_last_read_inbox_message_id(td_chat.last_read_inbox_message_id);
        self.set_marked_as_unread(td_chat.is_marked_as_unread);
        self.set_last_message(
            td_chat
                .last_message
                .map(|message| self.upsert_message(message)),
        );
        self.set_unread_mention_count(td_chat.unread_mention_count);
        self.set_unread_count(td_chat.unread_count);
        self.set_draft_message(td_chat.draft_message.map(model::BoxedDraftMessage));
        self.set_notification_settings(model::BoxedChatNotificationSettings(
            td_chat.notification_settings,
        ));
        self.set_permissions(model::BoxedChatPermissions(td_chat.permissions));
        self.imp()
            .available_reactions
            .replace(Some(td_chat.available_reactions));
    }

    pub(crate) fn handle_update(&self, update: tdlib::enums::Update) {
        use tdlib::enums::Update::*;

        let imp = self.imp();

        match update {
            ChatAction(update) => {
                self.actions().handle_update(update);
                // TODO: Remove this at some point. Widgets should use the `items-changed` signal
                // for updating their state in the future.
                self.notify_actions();
            }
            ChatDraftMessage(update) => {
                self.set_draft_message(update.draft_message.map(model::BoxedDraftMessage));
            }
            ChatBlockList(update) => {
                self.set_block_list(update.block_list.map(model::BoxedBlockList))
            }
            ChatIsMarkedAsUnread(update) => self.set_marked_as_unread(update.is_marked_as_unread),
            ChatLastMessage(update) => {
                self.set_last_message(update.last_message.map(|m| self.upsert_message(m)));
            }
            ChatNotificationSettings(update) => {
                self.set_notification_settings(model::BoxedChatNotificationSettings(
                    update.notification_settings,
                ));
            }
            ChatPermissions(update) => {
                self.set_permissions(model::BoxedChatPermissions(update.permissions))
            }
            ChatAvailableReactions(update) => {
                self.imp()
                    .available_reactions
                    .replace(Some(update.available_reactions));
            }
            ChatPhoto(update) => self.set_avatar(update.photo.map(Into::into)),
            ChatReadInbox(update) => {
                self.set_last_read_inbox_message_id(update.last_read_inbox_message_id);
                self.set_unread_count(update.unread_count);
            }
            ChatReadOutbox(update) => {
                self.set_last_read_outbox_message_id(update.last_read_outbox_message_id);
            }
            ChatTitle(update) => self.set_title(update.title),
            ChatUnreadMentionCount(update) => {
                self.set_unread_mention_count(update.unread_mention_count)
            }
            DeleteMessages(data) => {
                // FIXME: This should be removed after we notify opened and closed chats to TDLib
                // See discussion here: https://t.me/tdlibchat/65304
                if !data.from_cache {
                    let mut messages = imp.messages.borrow_mut();
                    let deleted_messages: Vec<model::Message> = data
                        .message_ids
                        .into_iter()
                        .filter_map(|id| messages.remove(&id))
                        .collect();

                    drop(messages);
                    for message in deleted_messages {
                        self.emit_by_name::<()>("deleted-message", &[&message]);
                    }
                }
            }
            MessageContent(ref data) => {
                if let Some(message) = self.message(data.message_id) {
                    message.handle_update(update);
                }
            }
            MessageEdited(ref data) => {
                if let Some(message) = self.message(data.message_id) {
                    message.handle_update(update);
                }
            }
            MessageIsPinned(ref data) => {
                if let Some(message) = self.message(data.message_id) {
                    message.handle_update(update);
                }
            }
            MessageInteractionInfo(ref data) => {
                if let Some(message) = self.message(data.message_id) {
                    message.handle_update(update);
                }
            }
            MessageSendSucceeded(data) => {
                let message_id = data.message.id;
                let old_message = imp.messages.borrow_mut().remove(&data.old_message_id);
                if let Some(message) = old_message.as_ref() {
                    self.emit_by_name::<()>("deleted-message", &[message]);
                }

                let message = if let Some(message) = self.message(message_id).or(old_message) {
                    message.update_from_td_message(data.message);
                    imp.messages
                        .borrow_mut()
                        .insert(message_id, message.clone());
                    message
                } else {
                    self.upsert_message(data.message)
                };
                self.emit_by_name::<()>("new-message", &[&message]);
            }
            NewMessage(data) => {
                let message = self.upsert_message(data.message);
                self.emit_by_name::<()>("new-message", &[&message]);
            }
            MessageMentionRead(update) => {
                self.set_unread_mention_count(update.unread_mention_count)
            }
            _ => {}
        }
    }

    weak_getter!(session_, session, model::ClientStateSession);

    pub(crate) fn is_blocked(&self) -> bool {
        matches!(
            self.block_list(),
            Some(model::BoxedBlockList(tdlib::enums::BlockList::Main))
        )
    }

    fn set_block_list(&self, block_list: Option<model::BoxedBlockList>) {
        if self.block_list() == block_list {
            return;
        }
        self.imp().block_list.replace(block_list);
        self.notify_block_list();
    }

    fn set_title(&self, title: String) {
        let title = self.localized_title(title);
        if self.title() == title {
            return;
        }
        self.imp().title.replace(title);
        self.notify_title();
    }

    /// Whether this is the private chat with the system "Replies" bot.
    pub(crate) fn is_replies_chat(&self) -> bool {
        match self.chat_type() {
            ChatType::Private(user) => {
                user.id() == REPLIES_BOT_USER_ID || user.username() == "replies"
            }
            _ => false,
        }
    }

    /// Maps TDLib's untranslated Replies chat title to the UI locale.
    fn localized_title(&self, title: String) -> String {
        if self.is_replies_chat() {
            gettext("Replies")
        } else {
            title
        }
    }

    fn set_avatar(&self, avatar: Option<model::Avatar>) {
        if self.avatar() == avatar {
            return;
        }
        self.imp().avatar.replace(avatar);
        self.notify_avatar();
    }

    fn set_last_read_outbox_message_id(&self, id: MessageId) {
        if self.last_read_outbox_message_id() == id {
            return;
        }
        self.imp().last_read_outbox_message_id.set(id);
        self.notify_last_read_outbox_message_id();
    }

    fn set_last_read_inbox_message_id(&self, id: MessageId) {
        if self.last_read_inbox_message_id() == id {
            return;
        }
        self.imp().last_read_inbox_message_id.set(id);
        self.notify_last_read_inbox_message_id();
    }

    fn set_marked_as_unread(&self, is_marked_as_unread: bool) {
        if self.is_marked_as_unread() == is_marked_as_unread {
            return;
        }
        self.imp().is_marked_as_unread.set(is_marked_as_unread);
        self.notify_is_marked_as_unread();
    }

    fn set_last_message(&self, last_message: Option<model::Message>) {
        if self.last_message() == last_message {
            return;
        }
        self.imp().last_message.replace(last_message);
        self.notify_last_message();
    }

    fn set_unread_mention_count(&self, unread_mention_count: i32) {
        if self.unread_mention_count() == unread_mention_count {
            return;
        }
        self.imp().unread_mention_count.set(unread_mention_count);
        self.notify_unread_mention_count();
    }

    fn set_unread_count(&self, unread_count: i32) {
        if self.unread_count() == unread_count {
            return;
        }
        self.imp().unread_count.set(unread_count);
        self.notify_unread_count()
    }

    fn set_draft_message(&self, draft_message: Option<model::BoxedDraftMessage>) {
        if self.draft_message() == draft_message {
            return;
        }
        self.imp().draft_message.replace(draft_message);
        self.notify_draft_message();
    }

    fn set_notification_settings(
        &self,
        notification_settings: model::BoxedChatNotificationSettings,
    ) {
        if self.notification_settings() == notification_settings {
            return;
        }
        self.imp()
            .notification_settings
            .replace(notification_settings);
        self.notify_notification_settings();
    }

    /// The Telegram-issued verification state of the chat's participant or group,
    /// if any. Resolved through the chat type (private user or supergroup).
    pub(crate) fn verification_status(&self) -> Option<tdlib::types::VerificationStatus> {
        match self.chat_type() {
            model::ChatType::Private(user) => user.verification_status().map(|v| v.0),
            model::ChatType::Supergroup(supergroup) => {
                supergroup.verification_status().map(|v| v.0)
            }
            _ => None,
        }
    }

    /// Whether the chat's notifications are effectively muted, resolving the
    /// chat's own setting against the session's scope default.
    pub(crate) fn is_muted(&self) -> bool {
        let session = self.session_();
        let scope = match self.chat_type() {
            model::ChatType::Private(_) | model::ChatType::Secret(_) => {
                session.private_chats_notification_settings()
            }
            model::ChatType::Supergroup(ref supergroup) if supergroup.is_channel() => {
                session.channel_chats_notification_settings()
            }
            _ => session.group_chats_notification_settings(),
        };
        resolve_muted(&self.notification_settings().0, &scope.0)
    }

    pub(crate) fn is_own_chat(&self) -> bool {
        self.chat_type().user() == Some(self.session_().me_())
    }

    pub(crate) fn available_reactions(&self) -> Option<tdlib::enums::ChatAvailableReactions> {
        self.imp().available_reactions.borrow().clone()
    }

    fn set_bot_commands(&self, commands: Vec<tdlib::types::BotCommand>) {
        let next = model::BoxedBotCommands(commands);
        if *self.imp().bot_commands.borrow() == next {
            return;
        }
        self.imp().bot_commands.replace(next);
        self.notify("bot-commands");
    }

    fn set_bot_menu_button(&self, button: Option<tdlib::types::BotMenuButton>) {
        let next = button.map(model::BoxedBotMenuButton);
        if *self.imp().bot_menu_button.borrow() == next {
            return;
        }
        self.imp().bot_menu_button.replace(next);
        self.notify("bot-menu-button");
    }

    fn clear_bot_commands(&self) {
        self.set_bot_commands(Vec::new());
        self.set_bot_menu_button(None);
    }

    /// Fetches this chat's commands (and bot menu button): `botInfo` for a
    /// private chat with a bot, `bot_commands` from the group full info
    /// otherwise. TDLib caches full-info responses, so calling this on every
    /// chat switch is cheap; results notify only when they changed.
    pub(crate) fn fetch_bot_commands(&self) {
        let client_id = self.session_().client_().id();
        match self.chat_type() {
            model::ChatType::Private(user) => {
                let user_id = user.id();
                utils::spawn(clone!(
                    #[weak(rename_to = chat)]
                    self,
                    async move {
                        match tdlib::functions::get_user_full_info(user_id, client_id).await {
                            Ok(tdlib::enums::UserFullInfo::UserFullInfo(info)) => {
                                match info.bot_info {
                                    Some(bot) => {
                                        chat.set_bot_commands(bot.commands);
                                        chat.set_bot_menu_button(bot.menu_button);
                                    }
                                    None => chat.clear_bot_commands(),
                                }
                            }
                            Err(e) => {
                                log::debug!("Failed to fetch bot info for {user_id}: {e:?}")
                            }
                        }
                    }
                ));
            }
            model::ChatType::Supergroup(data) => {
                let supergroup_id = data.id();
                utils::spawn(clone!(
                    #[weak(rename_to = chat)]
                    self,
                    async move {
                        let result =
                            tdlib::functions::get_supergroup_full_info(supergroup_id, client_id)
                                .await;
                        match result {
                            Ok(tdlib::enums::SupergroupFullInfo::SupergroupFullInfo(info)) => {
                                chat.set_bot_commands(group_bot_commands(
                                    &chat.session_(),
                                    &info.bot_commands,
                                ));
                                chat.set_bot_menu_button(None);
                            }
                            Err(e) => log::debug!("Failed to fetch group commands: {e:?}"),
                        }
                    }
                ));
            }
            model::ChatType::BasicGroup(data) => {
                let basic_group_id = data.id();
                utils::spawn(clone!(
                    #[weak(rename_to = chat)]
                    self,
                    async move {
                        let result =
                            tdlib::functions::get_basic_group_full_info(basic_group_id, client_id)
                                .await;
                        match result {
                            Ok(tdlib::enums::BasicGroupFullInfo::BasicGroupFullInfo(info)) => {
                                chat.set_bot_commands(group_bot_commands(
                                    &chat.session_(),
                                    &info.bot_commands,
                                ));
                                chat.set_bot_menu_button(None);
                            }
                            Err(e) => log::debug!("Failed to fetch group commands: {e:?}"),
                        }
                    }
                ));
            }
            model::ChatType::Secret(_) => {}
        }
    }

    fn set_permissions(&self, permissions: model::BoxedChatPermissions) {
        if self.permissions() == permissions {
            return;
        }
        self.imp().permissions.replace(permissions);
        self.notify_permissions();
    }

    pub(crate) fn connect_new_message<F: Fn(&Self, model::Message) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("new-message", true, move |values| {
            let obj = values[0].get().unwrap();
            let message = values[1].get().unwrap();
            f(obj, message);
            None
        })
    }

    pub(crate) fn connect_deleted_message<F: Fn(&Self, model::Message) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("deleted-message", true, move |values| {
            let obj = values[0].get().unwrap();
            let message = values[1].get().unwrap();
            f(obj, message);
            None
        })
    }

    /// Returns the `Message` of the specified id, if present in the cache.
    pub(crate) fn message(&self, id: MessageId) -> Option<model::Message> {
        self.imp().messages.borrow().get(&id).cloned()
    }

    pub(crate) fn upsert_message(&self, td_message: tdlib::types::Message) -> model::Message {
        let id = td_message.id;
        if let Some(message) = self.message(id) {
            message.update_from_td_message(td_message);
            return message;
        }

        let message = model::Message::new(self, td_message);
        self.imp().messages.borrow_mut().insert(id, message.clone());
        message
    }

    /// Returns the `Message` of the specified id, if present in the cache. Otherwise it
    /// fetches it from the server and then it returns the result.
    pub(crate) async fn fetch_message(
        &self,
        id: MessageId,
    ) -> Result<model::Message, tdlib::types::Error> {
        if let Some(message) = self.message(id) {
            return Ok(message);
        }

        let client_id = self.session_().client_().id();
        let result = tdlib::functions::get_message(self.id(), id, client_id).await;

        let message = result.map(|r| {
            let tdlib::enums::Message::Message(message) = r;
            message
        })?;
        self.session_()
            .ensure_sender_chats(std::iter::once(&message))
            .await;
        Ok(self.upsert_message(message))
    }

    /// Loads a page of messages newest-first. `from_id` is the newest id to
    /// exclude (0 for the latest page); a negative `offset` shifts the window
    /// toward newer messages, so a window *around* `from_id` can be fetched in one
    /// request.
    pub(crate) async fn get_chat_history_around(
        &self,
        from_id: MessageId,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<model::Message>, tdlib::types::Error> {
        let client_id = self.session_().client_().id();
        let result =
            tdlib::functions::get_chat_history(self.id(), from_id, offset, limit, false, client_id)
                .await;

        let tdlib::enums::Messages::Messages(data) = result?;

        self.session_()
            .ensure_sender_chats(data.messages.iter().flatten())
            .await;
        let loaded_messages: Vec<model::Message> = data
            .messages
            .into_iter()
            .flatten()
            .map(|m| self.upsert_message(m))
            .collect();

        Ok(loaded_messages)
    }

    /// Loads a comment thread's messages. `self` must be the discussion chat and
    /// `message_thread_id` the thread's unique id — TDLib keys thread history by it
    /// (it equals the anchor message id inside the discussion group, *not* the
    /// channel post id). Newest-first, like the other history loaders.
    pub(crate) async fn get_thread_history(
        &self,
        message_thread_id: i64,
        from_id: MessageId,
        offset: i32,
        limit: i32,
    ) -> Result<Vec<model::Message>, tdlib::types::Error> {
        let client_id = self.session_().client_().id();
        let result = tdlib::functions::get_message_thread_history(
            self.id(),
            message_thread_id,
            from_id,
            offset,
            limit,
            client_id,
        )
        .await;

        let tdlib::enums::Messages::Messages(data) = result?;

        self.session_()
            .ensure_sender_chats(data.messages.iter().flatten())
            .await;
        Ok(data
            .messages
            .into_iter()
            .flatten()
            .map(|m| self.upsert_message(m))
            .collect())
    }

    pub(crate) async fn mark_as_read(&self) -> Result<(), tdlib::types::Error> {
        if let Some(message) = self.last_message() {
            tdlib::functions::view_messages(
                self.id(),
                vec![message.id()],
                None,
                true,
                self.session_().client_().id(),
            )
            .await?;
        }

        tdlib::functions::toggle_chat_is_marked_as_unread(
            self.id(),
            false,
            self.session_().client_().id(),
        )
        .await
    }

    pub(crate) async fn mark_as_unread(&self) -> Result<(), tdlib::types::Error> {
        tdlib::functions::toggle_chat_is_marked_as_unread(
            self.id(),
            true,
            self.session_().client_().id(),
        )
        .await
    }
}

/// Whether a chat is muted, given its own notification settings and the session's
/// scope default for its chat type. When the chat defers to the default the scope
/// decides; otherwise the chat's own `mute_for` does.
fn resolve_muted(
    settings: &tdlib::types::ChatNotificationSettings,
    scope: &tdlib::types::ScopeNotificationSettings,
) -> bool {
    if settings.use_default_mute_for {
        scope.mute_for > 0 || settings.mute_for > 0
    } else {
        settings.mute_for > 0
    }
}

/// Group commands must name their bot: a bare `/start` in a multi-bot group
/// goes to whichever bot Telegram picks, so each command is qualified with
/// `@botusername` (what Telegram Desktop shows and inserts) before merging.
fn group_bot_commands(
    session: &model::ClientStateSession,
    entries: &[tdlib::types::BotCommands],
) -> Vec<tdlib::types::BotCommand> {
    let suffixed: Vec<tdlib::types::BotCommands> = entries
        .iter()
        .map(|entry| {
            let suffix = session
                .try_user(entry.bot_user_id)
                .map(|user| user.username().to_string())
                .filter(|name| !name.is_empty())
                .map(|name| format!("@{name}"))
                .unwrap_or_default();
            tdlib::types::BotCommands {
                bot_user_id: entry.bot_user_id,
                commands: entry
                    .commands
                    .iter()
                    .map(|command| tdlib::types::BotCommand {
                        command: format!("{}{suffix}", command.command.trim_start_matches('/')),
                        ..command.clone()
                    })
                    .collect(),
            }
        })
        .collect();
    merge_bot_commands(&suffixed)
}

/// Flattens the per-bot command groups of a group chat into one list, keeping
/// the first command with each name (the menu shows one entry per command).
fn merge_bot_commands(entries: &[tdlib::types::BotCommands]) -> Vec<tdlib::types::BotCommand> {
    let mut merged: Vec<tdlib::types::BotCommand> = Vec::new();
    for entry in entries {
        for command in &entry.commands {
            if !merged.iter().any(|seen| seen.command == command.command) {
                merged.push(command.clone());
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use tdlib::types::BotCommand;
    use tdlib::types::BotCommands;
    use tdlib::types::ChatNotificationSettings;
    use tdlib::types::ScopeNotificationSettings;

    use super::merge_bot_commands;
    use super::resolve_muted;

    fn bot_cmd(command: &str) -> BotCommand {
        BotCommand {
            command: command.to_string(),
            description: "description".to_string(),
            is_ephemeral: false,
        }
    }

    #[test]
    fn merge_bot_commands_keeps_first_per_command() {
        let one = BotCommands {
            bot_user_id: 1,
            commands: vec![bot_cmd("start"), bot_cmd("help")],
        };
        let two = BotCommands {
            bot_user_id: 2,
            commands: vec![bot_cmd("help"), bot_cmd("id")],
        };

        let merged = merge_bot_commands(&[one, two]);

        assert_eq!(
            merged
                .iter()
                .map(|c| c.command.as_str())
                .collect::<Vec<_>>(),
            ["start", "help", "id"]
        );
    }

    #[test]
    fn merge_bot_commands_of_nothing_is_empty() {
        assert!(merge_bot_commands(&[]).is_empty());
    }

    fn chat_settings(use_default: bool, mute_for: i32) -> ChatNotificationSettings {
        ChatNotificationSettings {
            use_default_mute_for: use_default,
            mute_for,
            ..Default::default()
        }
    }

    fn scope_settings(mute_for: i32) -> ScopeNotificationSettings {
        ScopeNotificationSettings {
            mute_for,
            ..Default::default()
        }
    }

    #[test]
    fn defers_to_scope_when_use_default() {
        assert!(resolve_muted(&chat_settings(true, 0), &scope_settings(10)));
        assert!(!resolve_muted(&chat_settings(true, 0), &scope_settings(0)));
    }

    #[test]
    fn own_setting_wins_when_not_default() {
        assert!(resolve_muted(&chat_settings(false, 10), &scope_settings(0)));
        assert!(!resolve_muted(
            &chat_settings(false, 0),
            &scope_settings(10)
        ));
    }
}
