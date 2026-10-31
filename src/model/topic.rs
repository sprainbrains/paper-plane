use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use glib::Properties;
use glib::subclass::Signal;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;

/// One forum topic. Holds the latest TDLib snapshot of a `ForumTopic` split
/// into flat fields. The UI reads the getters and re-renders on `changed`:
/// one signal per real change instead of a storm of per-property notifies,
/// mirroring how message rows refresh.
mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::Topic)]
    pub(crate) struct Topic {
        #[property(get, set, construct_only)]
        pub(super) forum_topic_id: Cell<i32>,
        pub(super) name: RefCell<String>,
        pub(super) icon_color: RefCell<tdlib::types::ForumTopicIcon>,
        pub(super) is_general: Cell<bool>,
        pub(super) is_closed: Cell<bool>,
        pub(super) is_hidden: Cell<bool>,
        pub(super) creator: RefCell<Option<tdlib::enums::MessageSender>>,
        pub(super) is_pinned: Cell<bool>,
        pub(super) unread_count: Cell<i32>,
        /// Null until a snapshot carrying settings arrives.
        pub(super) notification_settings: RefCell<Option<tdlib::types::ChatNotificationSettings>>,
        pub(super) draft: RefCell<Option<tdlib::types::DraftMessage>>,
        /// TDLib sort key (list order is descending); Rust-side only.
        pub(super) order: Cell<i64>,
        /// Last message of the topic, owned by the chat. Rust-side only.
        pub(super) last_message: glib::WeakRef<model::Message>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Topic {
        const NAME: &'static str = "Topic";
        type Type = super::Topic;
    }

    impl ObjectImpl for Topic {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("changed").build()])
        }
    }
}

glib::wrapper! {
    pub(crate) struct Topic(ObjectSubclass<imp::Topic>);
}

impl Topic {
    pub(crate) fn from_td(td: &tdlib::types::ForumTopic) -> Self {
        let obj = Self::from_info(&td.info);
        obj.update_from_td(td);
        obj
    }

    /// A topic known only through its info (e.g. freshly created): state fields
    /// stay at their defaults until a full snapshot arrives.
    pub(crate) fn from_info(info: &tdlib::types::ForumTopicInfo) -> Self {
        let obj: Self = glib::Object::builder()
            .property("forum-topic-id", info.forum_topic_id)
            .build();
        obj.apply_info(info);
        obj
    }

    pub(crate) fn name(&self) -> String {
        self.imp().name.borrow().clone()
    }

    pub(crate) fn icon_color(&self) -> i32 {
        self.imp().icon_color.borrow().color
    }

    pub(crate) fn icon_custom_emoji_id(&self) -> i64 {
        self.imp().icon_color.borrow().custom_emoji_id
    }

    pub(crate) fn is_general(&self) -> bool {
        self.imp().is_general.get()
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.imp().is_closed.get()
    }

    pub(crate) fn is_hidden(&self) -> bool {
        self.imp().is_hidden.get()
    }

    pub(crate) fn is_pinned(&self) -> bool {
        self.imp().is_pinned.get()
    }

    pub(crate) fn is_muted(&self) -> bool {
        self.notification_settings()
            .is_some_and(|settings| !settings.use_default_mute_for && settings.mute_for > 0)
    }

    pub(crate) fn unread_count(&self) -> i32 {
        self.imp().unread_count.get()
    }

    pub(crate) fn order(&self) -> i64 {
        self.imp().order.get()
    }

    pub(crate) fn creator(&self) -> Option<tdlib::enums::MessageSender> {
        self.imp().creator.borrow().clone()
    }

    pub(crate) fn draft(&self) -> Option<tdlib::types::DraftMessage> {
        self.imp().draft.borrow().clone()
    }

    pub(crate) fn last_message(&self) -> Option<model::Message> {
        self.imp().last_message.upgrade()
    }

    /// The topic's notification settings, once a snapshot carried them. Task 12
    /// edits these in place when muting.
    pub(crate) fn notification_settings(&self) -> Option<tdlib::types::ChatNotificationSettings> {
        self.imp().notification_settings.borrow().clone()
    }

    /// Applies a full snapshot without signalling: the list emits `changed`
    /// once after this and [`set_last_message`](Self::set_last_message).
    /// Returns whether anything visible moved (`order` alone does not count).
    pub(crate) fn update_from_td(&self, td: &tdlib::types::ForumTopic) -> bool {
        let imp = self.imp();
        let info_changed = self.apply_info(&td.info);
        imp.is_pinned.set(td.is_pinned);
        imp.unread_count.set(td.unread_count);
        let settings_changed =
            imp.notification_settings.borrow().as_ref() != Some(&td.notification_settings);
        imp.notification_settings
            .replace(Some(td.notification_settings.clone()));
        let draft_changed = *imp.draft.borrow() != td.draft_message;
        imp.draft.replace(td.draft_message.clone());
        imp.order.set(td.order);
        info_changed || settings_changed || draft_changed
    }

    /// Stores the model object of the topic's last message (owned by the chat).
    pub(crate) fn set_last_message(&self, message: Option<&model::Message>) {
        let imp = self.imp();
        imp.last_message.set(message);
    }

    pub(crate) fn notify_changed(&self) {
        self.emit_by_name::<()>("changed", &[]);
    }

    /// Routes an `updateForumTopic` (pin/read/mute/draft changes) onto the
    /// topic, signalling once when anything visible moved.
    pub(crate) fn apply_update(&self, update: &tdlib::types::UpdateForumTopic) {
        let imp = self.imp();
        let mut changed = false;
        if imp.is_pinned.get() != update.is_pinned {
            imp.is_pinned.set(update.is_pinned);
            changed = true;
        }
        if imp.notification_settings.borrow().as_ref() != Some(&update.notification_settings) {
            imp.notification_settings
                .replace(Some(update.notification_settings.clone()));
            changed = true;
        }
        if *imp.draft.borrow() != update.draft_message {
            imp.draft.replace(update.draft_message.clone());
            changed = true;
        }
        if changed {
            self.notify_changed();
        }
    }

    /// Routes an `updateForumTopicInfo` (rename, icon, close, hide) onto the
    /// topic, signalling once when anything visible moved.
    pub(crate) fn apply_info_update(&self, info: &tdlib::types::ForumTopicInfo) {
        if self.apply_info(info) {
            self.notify_changed();
        }
    }

    fn apply_info(&self, info: &tdlib::types::ForumTopicInfo) -> bool {
        let imp = self.imp();
        let mut changed = false;
        if *imp.name.borrow() != info.name {
            imp.name.replace(info.name.clone());
            changed = true;
        }
        if *imp.icon_color.borrow() != info.icon {
            imp.icon_color.replace(info.icon.clone());
            changed = true;
        }
        for (slot, value) in [
            (&imp.is_general, info.is_general),
            (&imp.is_closed, info.is_closed),
            (&imp.is_hidden, info.is_hidden),
        ] {
            if slot.get() != value {
                slot.set(value);
                changed = true;
            }
        }
        if *imp.creator.borrow() != Some(info.creator_id.clone()) {
            imp.creator.replace(Some(info.creator_id.clone()));
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gtk::prelude::*;

    use super::Topic;

    fn settings(mute_for: i32) -> tdlib::types::ChatNotificationSettings {
        tdlib::types::ChatNotificationSettings {
            use_default_mute_for: mute_for == 0,
            mute_for,
            use_default_sound: true,
            sound_id: 0,
            use_default_show_preview: true,
            show_preview: true,
            use_default_mute_stories: true,
            mute_stories: false,
            use_default_story_sound: true,
            story_sound_id: 0,
            use_default_show_story_poster: true,
            show_story_poster: true,
            use_default_disable_pinned_message_notifications: true,
            disable_pinned_message_notifications: false,
            use_default_disable_mention_notifications: true,
            disable_mention_notifications: false,
        }
    }

    fn forum_topic(name: &str) -> tdlib::types::ForumTopic {
        tdlib::types::ForumTopic {
            info: forum_topic_info(name),
            last_message: None,
            order: 10,
            is_pinned: false,
            unread_count: 0,
            last_read_inbox_message_id: 0,
            last_read_outbox_message_id: 0,
            unread_mention_count: 0,
            unread_reaction_count: 0,
            unread_poll_vote_count: 0,
            notification_settings: settings(0),
            draft_message: None,
        }
    }

    fn forum_topic_info(name: &str) -> tdlib::types::ForumTopicInfo {
        tdlib::types::ForumTopicInfo {
            chat_id: -1001,
            forum_topic_id: 7,
            name: name.into(),
            icon: tdlib::types::ForumTopicIcon {
                color: 0x6FB9F0,
                custom_emoji_id: 0,
            },
            creation_date: 0,
            creator_id: tdlib::enums::MessageSender::User(tdlib::types::MessageSenderUser {
                user_id: 1,
            }),
            is_general: false,
            is_outgoing: false,
            is_closed: false,
            is_hidden: false,
            is_name_implicit: false,
        }
    }

    #[test]
    fn topic_changed_signal_fires_only_on_real_change() {
        let topic = Topic::from_td(&forum_topic("Games"));
        let count = Rc::new(Cell::new(0));
        topic.connect_local("changed", false, {
            let count = count.clone();
            move |_| {
                count.set(count.get() + 1);
                None
            }
        });

        topic.apply_info_update(&forum_topic_info("Games"));
        assert_eq!(count.get(), 0, "identical data must not signal");

        topic.apply_info_update(&forum_topic_info("Rust"));
        assert_eq!(count.get(), 1, "one rename, one signal");
        assert_eq!(topic.name(), "Rust");
    }
}
