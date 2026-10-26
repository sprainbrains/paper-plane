use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

use glib::Properties;
use glib::clone;
use glib::prelude::*;
use glib::subclass::prelude::*;
use gtk::glib;

use crate::model;
use crate::types::ChatId;
use crate::types::SecretChatId;
use crate::types::UserId;
use crate::utils;

mod downloads;

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::ClientStateSession)]
    pub(crate) struct ClientStateSession {
        pub(super) chats: RefCell<HashMap<i64, model::Chat>>,
        pub(super) users: RefCell<HashMap<i64, model::User>>,
        pub(super) basic_groups: RefCell<HashMap<i64, model::BasicGroup>>,
        pub(super) supergroups: RefCell<HashMap<i64, model::Supergroup>>,
        pub(super) secret_chats: RefCell<HashMap<SecretChatId, model::SecretChat>>,
        pub(super) downloading_files:
            RefCell<HashMap<i32, Vec<async_channel::Sender<tdlib::types::File>>>>,

        #[property(get, set, construct_only)]
        pub(super) client: glib::WeakRef<model::Client>,
        #[property(get)]
        pub(super) me: glib::WeakRef<model::User>,
        #[property(get = Self::main_chat_list)]
        pub(super) main_chat_list: OnceCell<model::ChatList>,
        #[property(get = Self::archive_chat_list)]
        pub(super) archive_chat_list: OnceCell<model::ChatList>,
        #[property(get = Self::chat_folder_list)]
        pub(super) chat_folder_list: OnceCell<model::ChatFolderList>,
        #[property(get)]
        pub(super) private_chats_notification_settings:
            RefCell<model::BoxedScopeNotificationSettings>,
        #[property(get)]
        pub(super) group_chats_notification_settings:
            RefCell<model::BoxedScopeNotificationSettings>,
        #[property(get)]
        pub(super) channel_chats_notification_settings:
            RefCell<model::BoxedScopeNotificationSettings>,
        /// The user's server-side default ("favorite") reaction, delivered via
        /// `updateDefaultReactionType`. Not a GObject property because
        /// `ReactionType` has no meaningful default value.
        pub(super) default_reaction_type: RefCell<Option<model::BoxedReactionType>>,
        /// Cache of resolved standard-emoji reaction assets (static icon, animations),
        /// keyed by emoji. Reaction emoji are a tiny, heavily repeated set, so caching
        /// avoids an async round-trip per reaction chip and gives the tap-burst a
        /// ready source for the animations the badge just resolved.
        pub(super) emoji_reactions:
            RefCell<HashMap<String, std::rc::Rc<tdlib::types::EmojiReaction>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClientStateSession {
        const NAME: &'static str = "ClientStateSession";
        type Type = super::ClientStateSession;
    }

    impl ObjectImpl for ClientStateSession {
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

            let obj = &*self.obj();

            // Seed the chat lists from the local database right away. The
            // server-side option below is a request that can stay pending
            // indefinitely without connectivity, so it must not gate the load.
            obj.fetch_chats();

            utils::spawn(clone!(
                #[weak]
                obj,
                async move {
                    if let Err(e) = tdlib::functions::set_option(
                        "notification_group_count_max".to_string(),
                        Some(tdlib::enums::OptionValue::Integer(
                            tdlib::types::OptionValueInteger { value: 5 },
                        )),
                        obj.client_().id(),
                    )
                    .await
                    {
                        log::warn!("Error setting the notification_group_count_max option: {e:?}");
                    }
                }
            ));
        }
    }

    impl ClientStateSession {
        /// Returns the main chat list.
        pub(crate) fn main_chat_list(&self) -> model::ChatList {
            self.main_chat_list
                .get_or_init(|| model::ChatList::new(&self.obj(), tdlib::enums::ChatList::Main))
                .to_owned()
        }

        /// Returns the list of archived chats.
        pub(crate) fn archive_chat_list(&self) -> model::ChatList {
            self.archive_chat_list
                .get_or_init(|| model::ChatList::new(&self.obj(), tdlib::enums::ChatList::Archive))
                .to_owned()
        }

        pub(crate) fn chat_folder_list(&self) -> model::ChatFolderList {
            self.chat_folder_list
                .get_or_init(|| model::ChatFolderList::from(&*self.obj()))
                .to_owned()
        }
    }
}

glib::wrapper! {
    pub(crate) struct ClientStateSession(ObjectSubclass<imp::ClientStateSession>);
}

impl ClientStateSession {
    pub(crate) fn new(client: &model::Client, me: tdlib::types::User) -> Self {
        let obj: Self = glib::Object::builder().property("client", client).build();
        obj.imp().me.set(Some(&obj.upsert_user(me)));
        obj
    }

    weak_getter!(client_, client, model::Client);
    weak_getter!(me_, me, model::User);

    fn chat_list(&self, chat_list: tdlib::enums::ChatList) -> model::ChatList {
        match chat_list {
            tdlib::enums::ChatList::Main => self.main_chat_list(),
            tdlib::enums::ChatList::Archive => self.archive_chat_list(),
            tdlib::enums::ChatList::Folder(folder) => self
                .chat_folder_list()
                .get_or_create(folder.chat_folder_id.into()),
        }
    }

    /// Returns the `model::Chat` of the specified id, if present.
    pub(crate) fn try_chat(&self, id: ChatId) -> Option<model::Chat> {
        self.imp().chats.borrow().get(&id).cloned()
    }

    /// Returns the `model::Chat` of the specified id. Panics if the chat is not present.
    ///
    /// Note that TDLib guarantees that types are always returned before their ids,
    /// so if you use an id returned by TDLib, it should be expected that the
    /// relative `model::Chat` exists in the list.
    pub(crate) fn chat(&self, id: ChatId) -> model::Chat {
        self.try_chat(id)
            .expect("Failed to get expected model::Chat")
    }

    /// Returns the `model::Chat` of the specified id, materializing it through
    /// `getChat` when the chat never surfaced in an update (e.g. a discussion
    /// group only referenced by a channel's `linked_chat_id`, reached through
    /// the Replies bot). Returns `None` when TDLib cannot provide the chat.
    pub(crate) async fn chat_or_fetch(&self, id: ChatId) -> Option<model::Chat> {
        if let Some(chat) = self.try_chat(id) {
            return Some(chat);
        }
        let client_id = self.client_().id();
        if let Err(e) = tdlib::functions::get_chat(id, client_id).await {
            log::warn!("Failed to fetch chat {id}: {e:?}");
            return None;
        }
        // The chat lands in our map through `updateNewChat`; normally the
        // update precedes the response, but give the update loop a beat in
        // case the response was processed first.
        for _ in 0..20 {
            if let Some(chat) = self.try_chat(id) {
                return Some(chat);
            }
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        log::warn!("Chat {id} did not materialize after getChat");
        None
    }

    pub(crate) fn upsert_user(&self, td_user: tdlib::types::User) -> model::User {
        let mut users = self.imp().users.borrow_mut();

        match users.entry(td_user.id) {
            Entry::Occupied(entry) => {
                let user = entry.get();
                user.update(td_user);
                user.to_owned()
            }
            Entry::Vacant(entry) => {
                let user = model::User::new(self, td_user);
                entry.insert(user.clone());
                user
            }
        }
    }

    /// Returns the `model::User` of the specified id, or `None` if it has not been
    /// loaded yet.
    pub(crate) fn try_user(&self, user_id: UserId) -> Option<model::User> {
        self.imp().users.borrow().get(&user_id).cloned()
    }

    /// Returns the `model::User` of the specified id, inserting a placeholder and
    /// fetching the real user from TDLib in the background if it is not yet
    /// cached.
    ///
    /// The "types are returned before their ids" guarantee holds for the update
    /// stream, but not always for results of direct requests such as
    /// `getChatHistory`, which can reference a sender that has not been pushed to
    /// the client yet. Using this instead of [`Self::try_user`] there keeps message
    /// construction from panicking on a not-yet-known sender.
    pub(crate) fn user_or_fetch(&self, user_id: UserId) -> model::User {
        if let Some(user) = self.try_user(user_id) {
            return user;
        }

        log::warn!("User {user_id} referenced before being loaded; fetching it");

        let placeholder = model::User::new_placeholder(self, user_id);
        self.imp()
            .users
            .borrow_mut()
            .insert(user_id, placeholder.clone());

        let client_id = self.client_().id();
        utils::spawn(clone!(
            #[weak(rename_to = session)]
            self,
            async move {
                match tdlib::functions::get_user(user_id, client_id).await {
                    Ok(tdlib::enums::User::User(td_user)) => {
                        session.upsert_user(td_user);
                    }
                    Err(e) => log::warn!("Failed to fetch user {user_id}: {e:?}"),
                }
            }
        ));

        placeholder
    }

    /// Returns the `model::BasicGroup` of the specified id. Panics if the basic group is not present.
    ///
    /// Note that TDLib guarantees that types are always returned before their ids,
    /// so if you use an id returned by TDLib, it should be expected that the
    /// relative `model::BasicGroup` exists in the list.
    pub(crate) fn basic_group(&self, basic_group_id: i64) -> model::BasicGroup {
        self.imp()
            .basic_groups
            .borrow()
            .get(&basic_group_id)
            .expect("Failed to get expected model::BasicGroup")
            .clone()
    }

    /// Returns the `model::Supergroup` of the specified id. Panics if the supergroup is not present.
    ///
    /// Note that TDLib guarantees that types are always returned before their ids,
    /// so if you use an id returned by TDLib, it should be expected that the
    /// relative `model::Supergroup` exists in the list.
    pub(crate) fn supergroup(&self, supergroup_id: i64) -> model::Supergroup {
        self.imp()
            .supergroups
            .borrow()
            .get(&supergroup_id)
            .expect("Failed to get expected model::Supergroup")
            .clone()
    }

    /// Returns the `model::SecretChat` of the specified id. Panics if the secret chat is not present.
    ///
    /// Note that TDLib guarantees that types are always returned before their ids,
    /// so if you use an id returned by TDLib, it should be expected that the
    /// relative `model::SecretChat` exists in the list.
    pub(crate) fn secret_chat(&self, id: SecretChatId) -> model::SecretChat {
        self.imp()
            .secret_chats
            .borrow()
            .get(&id)
            .expect("Failed to get expected model::SecretChat")
            .clone()
    }

    pub(crate) fn fetch_chats(&self) {
        self.archive_chat_list().fetch();
        self.main_chat_list().fetch();
    }

    /// Fetches the contacts of the user.
    pub(crate) async fn fetch_contacts(&self) -> Result<Vec<model::User>, tdlib::types::Error> {
        let result = tdlib::functions::get_contacts(self.client_().id()).await;

        result.map(|data| {
            let tdlib::enums::Users::Users(users) = data;
            users
                .user_ids
                .into_iter()
                .map(|id| self.user_or_fetch(id))
                .collect()
        })
    }

    /// The user's server-side default ("favorite") reaction, if known.
    pub(crate) fn default_reaction_type(&self) -> Option<model::BoxedReactionType> {
        self.imp().default_reaction_type.borrow().clone()
    }

    /// Resolves a standard emoji to its reaction assets (static icon and
    /// animations), caching the result. Returns `None` when the emoji has no
    /// active reaction. Concurrent resolves for the same emoji are tolerated
    /// (first writer wins); the underlying `getEmojiReaction` is a local DB read.
    pub(crate) async fn resolve_emoji_reaction(
        &self,
        emoji: String,
    ) -> Option<std::rc::Rc<tdlib::types::EmojiReaction>> {
        if let Some(cached) = self.imp().emoji_reactions.borrow().get(&emoji) {
            return Some(cached.clone());
        }

        let client_id = self.client_().id();
        let tdlib::enums::EmojiReaction::EmojiReaction(reaction) =
            tdlib::functions::get_emoji_reaction(emoji.clone(), client_id)
                .await
                .ok()?;

        let reaction = std::rc::Rc::new(reaction);
        self.imp()
            .emoji_reactions
            .borrow_mut()
            .entry(emoji)
            .or_insert_with(|| reaction.clone());
        Some(reaction)
    }

    fn set_private_chats_notification_settings(
        &self,
        settings: model::BoxedScopeNotificationSettings,
    ) {
        if self.private_chats_notification_settings() == settings {
            return;
        }
        self.imp()
            .private_chats_notification_settings
            .replace(settings);
        self.notify_private_chats_notification_settings();
    }

    fn set_group_chats_notification_settings(
        &self,
        settings: model::BoxedScopeNotificationSettings,
    ) {
        if self.group_chats_notification_settings() == settings {
            return;
        }
        self.imp()
            .group_chats_notification_settings
            .replace(settings);
        self.notify_group_chats_notification_settings();
    }

    fn set_channel_chats_notification_settings(
        &self,
        settings: model::BoxedScopeNotificationSettings,
    ) {
        if self.channel_chats_notification_settings() == settings {
            return;
        }
        self.imp()
            .channel_chats_notification_settings
            .replace(settings);
        self.notify_channel_chats_notification_settings();
    }

    fn handle_chat_position_update(
        &self,
        chat: &model::Chat,
        position: &tdlib::types::ChatPosition,
    ) {
        use tdlib::enums::ChatList::*;

        match &position.list {
            Main => {
                self.main_chat_list().update_chat_position(chat, position);
            }
            Archive => {
                self.archive_chat_list()
                    .update_chat_position(chat, position);
            }
            Folder(data) => {
                self.chat_folder_list()
                    .get_or_create(data.chat_folder_id.into())
                    .update_chat_position(chat, position);
            }
        }
    }

    pub(crate) fn handle_update(&self, update: tdlib::enums::Update) {
        use tdlib::enums::Update::*;

        match update {
            NewChat(data) => {
                // No need to update the chat positions here, tdlib sends
                // the correct chat positions in other updates later
                let chat = model::Chat::new(self, data.chat);
                self.imp().chats.borrow_mut().insert(chat.id(), chat);
            }
            ChatTitle(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatPhoto(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatPermissions(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatAvailableReactions(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatLastMessage(ref data) => {
                let chat = self.chat(data.chat_id);
                for position in &data.positions {
                    self.handle_chat_position_update(&chat, position);
                }
                chat.handle_update(update);
            }
            ChatPosition(ref data) => {
                self.handle_chat_position_update(&self.chat(data.chat_id), &data.position)
            }
            ChatReadInbox(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatReadOutbox(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatDraftMessage(ref data) => {
                let chat = self.chat(data.chat_id);
                for position in &data.positions {
                    self.handle_chat_position_update(&chat, position);
                }
                chat.handle_update(update);
            }
            ChatFolders(_) => self.chat_folder_list().handle_update(update),
            ChatNotificationSettings(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatUnreadMentionCount(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatBlockList(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatIsMarkedAsUnread(ref data) => self.chat(data.chat_id).handle_update(update),
            DeleteMessages(ref data) => self.chat(data.chat_id).handle_update(update),
            ChatAction(ref data) => self.chat(data.chat_id).handle_update(update),
            MessageContent(ref data) => self.chat(data.chat_id).handle_update(update),
            MessageEdited(ref data) => self.chat(data.chat_id).handle_update(update),
            MessageIsPinned(ref data) => self.chat(data.chat_id).handle_update(update),
            MessageInteractionInfo(ref data) => self.chat(data.chat_id).handle_update(update),
            MessageMentionRead(ref data) => self.chat(data.chat_id).handle_update(update),
            MessageSendSucceeded(ref data) => self.chat(data.message.chat_id).handle_update(update),
            NewMessage(ref data) => self.chat(data.message.chat_id).handle_update(update),
            BasicGroup(data) => {
                let mut basic_groups = self.imp().basic_groups.borrow_mut();
                match basic_groups.entry(data.basic_group.id) {
                    Entry::Occupied(entry) => entry.get().update(data.basic_group),
                    Entry::Vacant(entry) => {
                        entry.insert(model::BasicGroup::from(data.basic_group));
                    }
                }
            }
            File(update) => {
                self.handle_file_update(update.file);
            }
            ScopeNotificationSettings(update) => {
                use tdlib::enums::NotificationSettingsScope::*;

                let settings = model::BoxedScopeNotificationSettings(update.notification_settings);
                match update.scope {
                    PrivateChats => {
                        self.set_private_chats_notification_settings(settings);
                    }
                    GroupChats => {
                        self.set_group_chats_notification_settings(settings);
                    }
                    ChannelChats => {
                        self.set_channel_chats_notification_settings(settings);
                    }
                }
            }
            SecretChat(data) => {
                let mut secret_chats = self.imp().secret_chats.borrow_mut();
                match secret_chats.entry(data.secret_chat.id.into()) {
                    Entry::Occupied(entry) => entry.get().update(data.secret_chat),
                    Entry::Vacant(entry) => {
                        entry.insert(model::SecretChat::new(
                            self.user_or_fetch(data.secret_chat.user_id),
                            data.secret_chat,
                        ));
                    }
                }
            }
            Supergroup(data) => {
                let mut supergroups = self.imp().supergroups.borrow_mut();
                match supergroups.entry(data.supergroup.id) {
                    Entry::Occupied(entry) => entry.get().update(data.supergroup),
                    Entry::Vacant(entry) => {
                        entry.insert(model::Supergroup::from(data.supergroup));
                    }
                }
            }
            UnreadChatCount(data) => {
                self.chat_list(data.chat_list)
                    .set_unread_chat_count(data.unread_count);
            }
            UnreadMessageCount(data) => {
                self.chat_list(data.chat_list)
                    .set_unread_message_count(data.unread_count);
            }
            User(data) => {
                self.upsert_user(data.user);
            }
            UserStatus(data) => {
                // The user may not be cached yet; a later `updateUser` carries the
                // status, so it is safe to skip here rather than panic.
                if let Some(user) = self.try_user(data.user_id) {
                    user.update_status(data.status);
                }
            }
            DefaultReactionType(data) => {
                self.imp()
                    .default_reaction_type
                    .replace(Some(model::BoxedReactionType(data.reaction_type)));
            }
            _ => {}
        }
    }
}
