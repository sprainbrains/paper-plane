use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;

use gettextrs::gettext;
use glib::Properties;
use glib::clone;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use thiserror::Error;

use crate::model;
use crate::types::ChatId;
use crate::utils;

/// How many chats to request per `loadChats` batch.
const CHATS_FETCH_LIMIT: i32 = 20;
/// How many locally known chats `getChats` seeds the list with before the
/// server-backed `loadChats` pagination takes over.
const CHATS_SEED_LIMIT: i32 = 200;
/// Delay between batches, so chats stream in without blocking the UI.
const CHATS_FETCH_INTERVAL_MS: u64 = 50;

#[derive(Debug, Error)]
pub(crate) enum ChatListError {
    #[error("{0}")]
    NotFolder(String),
    #[error("TDLib error: {0:?}")]
    Tdlib(tdlib::types::Error),
}

/// Sort key for a chat within the list: `(-order, chat_id)`. The negated order
/// gives a descending sort, and the chat id breaks ties so chats sharing an
/// `order` (e.g. several with `order == 0`) still get distinct, stable keys.
type SortKey = (i64, ChatId);

#[derive(Debug)]
struct Entry<T> {
    key: SortKey,
    item: T,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, glib::Enum)]
#[enum_type(name = "ChatListType")]
pub(crate) enum ChatListType {
    #[default]
    Main,
    Archive,
    Folder,
}
impl From<ChatListType> for Option<tdlib::enums::ChatList> {
    fn from(list_type: ChatListType) -> Self {
        match list_type {
            ChatListType::Main => None,
            ChatListType::Archive => Some(tdlib::enums::ChatList::Archive),
            ChatListType::Folder => Some(tdlib::enums::ChatList::Folder(
                tdlib::types::ChatListFolder { chat_folder_id: 0 },
            )),
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::ChatList)]
    pub(crate) struct ChatList {
        // Chats sorted ascending by `SortKey`. A plain `Vec` gives O(1) indexed
        // access for `item()` (which GtkListView calls constantly), while the
        // `index` map turns "find this chat" into an O(1) key lookup plus an
        // O(log n) binary search instead of an O(n) scan that upgrades a weak
        // reference for every element.
        pub(super) list: RefCell<Vec<Entry<model::ChatListItem>>>,
        // chat id -> the chat's current negated order (the first half of its
        // `SortKey`), so a chat can be located without scanning `list`.
        pub(super) index: RefCell<HashMap<ChatId, i64>>,
        #[property(get, set, construct_only)]
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        #[property(get, set, construct_only)]
        pub(super) list_type: RefCell<model::BoxedChatListType>,
        #[property(get, set)]
        pub(super) icon: RefCell<String>,
        #[property(get, set)]
        pub(super) title: RefCell<String>,
        #[property(get, set)]
        pub(super) unread_chat_count: Cell<i32>,
        #[property(get, set)]
        pub(super) unread_message_count: Cell<i32>,
        // `true` while a `fetch()` loop is streaming chats in; drives the sidebar
        // loading spinner.
        #[property(get, set)]
        pub(super) loading: Cell<bool>,
        #[property(get = |this: &Self| this.list.borrow().len() as u32)]
        pub(super) len: PhantomData<u32>,
        // Set once the list has been fully loaded, so re-displaying it does not
        // kick off a redundant fetch.
        pub(super) loaded: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatList {
        const NAME: &'static str = "ChatList";
        type Type = super::ChatList;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ChatList {
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
            self.obj()
                .connect_items_changed(|obj, _, _, _| obj.notify("len"));
        }
    }

    impl ListModelImpl for ChatList {
        fn item_type(&self) -> glib::Type {
            model::ChatListItem::static_type()
        }

        fn n_items(&self) -> u32 {
            self.list.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            self.list
                .borrow()
                .get(position as usize)
                .map(|entry| entry.item.upcast_ref())
                .cloned()
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatList(ObjectSubclass<imp::ChatList>)
        @implements gio::ListModel;
}

impl ChatList {
    pub(crate) fn new(
        session: &model::ClientStateSession,
        list_type: tdlib::enums::ChatList,
    ) -> Self {
        glib::Object::builder()
            .property("session", session)
            .property("list-type", model::BoxedChatListType(list_type))
            .build()
    }

    weak_getter!(session_, session, model::ClientStateSession);

    pub(crate) fn fetch(&self) {
        // Load each list only once, and never run two loaders concurrently. The flag
        // is set synchronously here (before spawning) so two fetch() calls in the same
        // main-loop tick can't both start a loader.
        if self.loading() || self.imp().loaded.get() {
            return;
        }
        self.set_loading(true);

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let client_id = obj.session_().client_().id();

                // Seed from the local database first: `getChats` answers
                // immediately even without connectivity, and TDLib emits the
                // new-chat and position updates for the returned chats, which
                // populate this model. `loadChats` below then guarantees the
                // full server-synced list.
                if let Err(e) = tdlib::functions::get_chats(
                    Some(obj.list_type().0),
                    CHATS_SEED_LIMIT,
                    client_id,
                )
                .await
                {
                    log::warn!("GetChats failed: {e:?}");
                }

                loop {
                    let result = tdlib::functions::load_chats(
                        Some(obj.list_type().0),
                        CHATS_FETCH_LIMIT,
                        client_id,
                    )
                    .await;

                    match result {
                        // Pace the loading so the UI stays responsive and chats stream
                        // in progressively instead of freezing while all load at once.
                        Ok(_) => {
                            glib::timeout_future(std::time::Duration::from_millis(
                                CHATS_FETCH_INTERVAL_MS,
                            ))
                            .await;
                        }
                        // Error 404 means that all chats have been loaded.
                        Err(err) => {
                            if err.code != 404 {
                                log::error!("LoadChats failed: {err:?}");
                            }
                            break;
                        }
                    }
                }

                obj.imp().loaded.set(true);
                obj.set_loading(false);
            }
        ));
    }

    pub(crate) fn update_chat_position(
        &self,
        chat: &model::Chat,
        position: &tdlib::types::ChatPosition,
    ) {
        let imp = self.imp();

        let chat_id = chat.id();
        let new_key = (-position.order, chat_id);

        // O(1) lookup of the chat's current key (if it is already in this list).
        let old_key = imp
            .index
            .borrow()
            .get(&chat_id)
            .map(|order| (*order, chat_id));

        match old_key {
            // Same position: nothing moved, so update the item in place and skip the
            // remove/re-add churn that would needlessly recycle the row widget.
            Some(old_key) if position.order != 0 && old_key == new_key => {
                // Clone out and drop the borrow before `update()`, which notifies and
                // could re-enter this list.
                let item = {
                    let list = imp.list.borrow();
                    list[position_of(&list, old_key)].item.clone()
                };
                item.update(position);
            }
            // `order == 0` means the chat is no longer part of this list.
            Some(old_key) if position.order == 0 => {
                let old_pos = {
                    let mut list = imp.list.borrow_mut();
                    let pos = position_of(&list, old_key);
                    list.remove(pos);
                    pos
                };
                imp.index.borrow_mut().remove(&chat_id);
                self.items_changed(old_pos as u32, 1, 0);
            }
            // The chat moved. Emit a minimal remove + insert (not a single items_changed
            // spanning the whole moved range) so GtkListView only rebinds the moved row
            // rather than every row between the old and new position on each update.
            Some(old_key) => {
                let (old_pos, item) = {
                    let mut list = imp.list.borrow_mut();
                    let old_pos = position_of(&list, old_key);
                    (old_pos, list.remove(old_pos).item)
                };
                imp.index.borrow_mut().remove(&chat_id);
                self.items_changed(old_pos as u32, 1, 0);

                // `update()` and `insert_item()` both notify and could re-enter this list,
                // so the borrow is already dropped by here.
                item.update(position);
                self.insert_item(new_key, item);
            }
            // A not-yet-tracked chat with `order == 0` is not part of this list, so
            // there is nothing to insert.
            None if position.order == 0 => {}
            None => self.insert_item(new_key, model::ChatListItem::new(chat, position)),
        }
    }

    fn insert_item(&self, key: SortKey, item: model::ChatListItem) {
        let imp = self.imp();

        let position = {
            let mut list = imp.list.borrow_mut();
            // Keys are unique, so the binary search always reports the insert point.
            let position = list.partition_point(|entry| entry.key < key);
            list.insert(position, Entry { key, item });
            position
        };
        imp.index.borrow_mut().insert(key.1, key.0);

        self.items_changed(position as u32, 0, 1);
    }

    pub(crate) async fn delete(&self) -> Result<(), ChatListError> {
        match self.list_type().0 {
            tdlib::enums::ChatList::Folder(tdlib::types::ChatListFolder { chat_folder_id }) => {
                tdlib::functions::delete_chat_folder(
                    chat_folder_id,
                    Vec::new(),
                    self.session_().client_().id(),
                )
                .await
                .map_err(ChatListError::Tdlib)
            }
            _ => Err(ChatListError::NotFolder(gettext(
                "Only folders can be deleted.",
            ))),
        }
    }
}

/// Index of the entry with the given (present) key. O(log n) and, unlike a scan
/// by chat id, never upgrades a weak reference.
fn position_of<T>(list: &[Entry<T>], key: SortKey) -> usize {
    list.binary_search_by(|entry| entry.key.cmp(&key))
        .expect("the key was taken from the index, so it must be present")
}

/// Relocate one entry and return its affected range and preserved model item.
///
/// The list must already contain the entry at `old_pos` and must be sorted by
/// key.
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(order: i64, chat_id: ChatId) -> Entry<ChatId> {
        Entry {
            key: (-order, chat_id),
            item: chat_id,
        }
    }

    fn chat_ids(list: &[Entry<ChatId>]) -> Vec<ChatId> {
        list.iter().map(|entry| entry.item).collect()
    }

    #[test]
    fn chat_id_breaks_equal_order_ties() {
        let mut list = vec![entry(20, 1), entry(10, 2), entry(10, 4)];

        let position = list.partition_point(|entry| entry.key < (-10, 3));
        list.insert(position, entry(10, 3));

        assert_eq!(chat_ids(&list), [1, 2, 3, 4]);
        assert_eq!(position_of(&list, (-10, 3)), 2);
    }
}
