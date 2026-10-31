use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;

use glib::Properties;
use glib::clone;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::utils;

/// How many topics to request per `getForumTopics` batch.
const TOPICS_FETCH_LIMIT: i32 = 100;

/// Merges `incoming` (id, order) pairs into `existing` (incoming wins on
/// duplicate ids) and returns the result sorted by order descending, ties by
/// id descending. Pure so the sort contract is testable without a session.
fn merge_orders(existing: &[(i32, i64)], incoming: &[(i32, i64)]) -> Vec<(i32, i64)> {
    let mut merged = existing.to_vec();
    for &(id, order) in incoming {
        match merged.iter_mut().find(|(existing, _)| *existing == id) {
            Some(slot) => slot.1 = order,
            None => merged.push((id, order)),
        }
    }
    merged.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    merged
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::ForumTopicList)]
    pub(crate) struct ForumTopicList {
        #[property(get)]
        pub(super) loading: Cell<bool>,
        pub(super) chat: glib::WeakRef<model::Chat>,
        /// Topics in display order (TDLib `order`, descending).
        pub(super) topics: RefCell<Vec<model::Topic>>,
        /// topic id -> the topic object, so refreshes update in place and every
        /// consumer resolves the same `Topic` (object identity).
        pub(super) index: RefCell<HashMap<i32, model::Topic>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ForumTopicList {
        const NAME: &'static str = "ForumTopicList";
        type Type = super::ForumTopicList;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ForumTopicList {
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

    impl ListModelImpl for ForumTopicList {
        fn item_type(&self) -> glib::Type {
            model::Topic::static_type()
        }

        fn n_items(&self) -> u32 {
            self.topics.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            self.topics
                .borrow()
                .get(position as usize)
                .map(|topic| topic.clone().upcast::<glib::Object>())
        }
    }
}

glib::wrapper! {
    pub(crate) struct ForumTopicList(ObjectSubclass<imp::ForumTopicList>)
        @implements gio::ListModel;
}

impl ForumTopicList {
    pub(crate) fn new(chat: &model::Chat) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().chat.set(Some(chat));
        obj
    }

    pub(crate) fn topic(&self, forum_topic_id: i32) -> Option<model::Topic> {
        self.imp().index.borrow().get(&forum_topic_id).cloned()
    }

    fn set_loading(&self, loading: bool) {
        if self.loading() == loading {
            return;
        }
        self.imp().loading.set(loading);
        self.notify_loading();
    }

    /// Fetches the topic list from TDLib (all pages). Repeated calls refresh in
    /// place; concurrent calls are collapsed into the one in flight.
    pub(crate) fn refresh(&self) {
        let imp = self.imp();
        if imp.loading.get() {
            return;
        }
        let Some(chat) = imp.chat.upgrade() else {
            return;
        };
        self.set_loading(true);

        let chat_id = chat.id();
        let client_id = chat.session_().client_().id();
        utils::spawn(clone!(
            #[weak(rename_to = list)]
            self,
            async move {
                let mut topics = Vec::new();
                let mut offsets = (0i32, 0i64, 0i32);
                loop {
                    match tdlib::functions::get_forum_topics(
                        chat_id,
                        String::new(),
                        offsets.0,
                        offsets.1,
                        offsets.2,
                        TOPICS_FETCH_LIMIT,
                        client_id,
                    )
                    .await
                    {
                        Ok(tdlib::enums::ForumTopics::ForumTopics(data)) => {
                            let page_len = data.topics.len();
                            offsets = (
                                data.next_offset_date,
                                data.next_offset_message_id,
                                data.next_offset_forum_topic_id,
                            );
                            topics.extend(data.topics);
                            if page_len == 0
                                || topics.len() >= data.total_count as usize
                                || page_len < TOPICS_FETCH_LIMIT as usize
                            {
                                break;
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to load forum topics of {chat_id}: {e:?}");
                            break;
                        }
                    }
                }
                list.set_loading(false);
                list.apply_topics(&topics);
            }
        ));
    }

    /// Applies a fetched page set: existing topics are updated in place (object
    /// identity preserved), new ones appended, then the display order is
    /// recomputed. One full-range `items-changed` is emitted when the sequence
    /// moved — correct, if coarser than a minimal diff; topic lists are short
    /// and refreshes rare (no order in `updateForumTopic`).
    fn apply_topics(&self, td_topics: &[tdlib::types::ForumTopic]) {
        let imp = self.imp();
        let Some(chat) = imp.chat.upgrade() else {
            return;
        };

        let mut incoming = Vec::with_capacity(td_topics.len());
        for td in td_topics {
            let id = td.info.forum_topic_id;
            let last_message = td
                .last_message
                .clone()
                .map(|message| chat.upsert_message(message));
            // Look the topic up and drop the borrow before re-entering: the
            // arms mutate `index` and emit signals (AGENTS.md borrow rule).
            let existing = imp.index.borrow().get(&id).cloned();
            match existing {
                Some(topic) => {
                    let last_changed = topic.last_message().map(|m| m.id())
                        != last_message.as_ref().map(|m| m.id());
                    topic.set_last_message(last_message.as_ref());
                    let changed = topic.update_from_td(td) | last_changed;
                    if changed {
                        topic.notify_changed();
                    }
                }
                None => {
                    let topic = model::Topic::from_td(td);
                    topic.set_last_message(last_message.as_ref());
                    imp.topics.borrow_mut().push(topic.clone());
                    imp.index.borrow_mut().insert(id, topic);
                }
            }
            incoming.push((id, td.order));
        }

        let pairs: Vec<(i32, i64)> = imp
            .topics
            .borrow()
            .iter()
            .map(|topic| (topic.forum_topic_id(), topic.order()))
            .collect();
        let sorted = merge_orders(&pairs, &incoming);
        let moved = sorted.iter().zip(pairs.iter()).any(|(a, b)| a != b);
        if moved {
            let old_len = imp.topics.borrow().len() as u32;
            let mut by_id: HashMap<_, _> = imp
                .topics
                .borrow_mut()
                .drain(..)
                .map(|topic| (topic.forum_topic_id(), topic))
                .collect();
            // `sorted` holds exactly the ids just drained; the filter_map keeps
            // the borrow checker happy while rebuilding in display order.
            let reordered: Vec<model::Topic> = sorted
                .iter()
                .filter_map(|(id, _)| by_id.remove(id))
                .collect();
            let new_len = reordered.len() as u32;
            *imp.topics.borrow_mut() = reordered;
            self.upcast_ref::<gio::ListModel>()
                .items_changed(0, old_len, new_len);
        }
    }

    pub(crate) fn apply_update(&self, update: &tdlib::types::UpdateForumTopic) {
        if let Some(topic) = self.topic(update.forum_topic_id) {
            topic.apply_update(update);
        }
    }

    /// Routes an `updateForumTopicInfo`: updates a known topic, or inserts a
    /// newly created one at the top of the list.
    /// shortcut: a new topic lands above pinned ones until the next `refresh()`
    /// re-sorts by the real TDLib order.
    pub(crate) fn apply_info_update(&self, info: &tdlib::types::ForumTopicInfo) {
        let imp = self.imp();
        if let Some(topic) = self.topic(info.forum_topic_id) {
            topic.apply_info_update(info);
            return;
        }
        let topic = model::Topic::from_info(info);
        imp.topics.borrow_mut().insert(0, topic.clone());
        imp.index.borrow_mut().insert(info.forum_topic_id, topic);
        self.upcast_ref::<gio::ListModel>().items_changed(0, 0, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::merge_orders;

    #[test]
    fn merged_topic_orders_desc_and_keeps_identity() {
        // A fresh page appends unseen topics, sorted by `order` descending.
        assert_eq!(
            merge_orders(&[(2, 20), (1, 10)], &[(3, 5)]),
            vec![(2, 20), (1, 10), (3, 5)]
        );
        // An update to an existing id changes its position, not its identity.
        assert_eq!(
            merge_orders(&[(2, 20), (1, 10)], &[(1, 30)]),
            vec![(1, 30), (2, 20)]
        );
    }
}
