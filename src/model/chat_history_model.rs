use std::cell::Cell;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::VecDeque;

use gio::prelude::*;
use gio::subclass::prelude::*;
use glib::Properties;
use glib::clone;
use gtk::gio;
use gtk::glib;
use thiserror::Error;

use crate::model;

#[derive(Error, Debug)]
pub(crate) enum ChatHistoryError {
    #[error("The chat history is already loading messages")]
    AlreadyLoading,
    #[error("TDLib error: {0:?}")]
    Tdlib(tdlib::types::Error),
    #[error("The chat is no longer available")]
    ChatUnavailable,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::ChatHistoryModel)]
    pub(crate) struct ChatHistoryModel {
        #[property(get)]
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) new_message_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) deleted_message_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) is_loading: Cell<bool>,
        pub(super) list: RefCell<VecDeque<model::ChatHistoryItem>>,
        /// Whether the loaded window includes the chat's newest message. `false` after
        /// jumping to an old message, until the gap is closed by loading newer ones.
        pub(super) newest_loaded: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistoryModel {
        const NAME: &'static str = "ChatHistoryModel";
        type Type = super::ChatHistoryModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ChatHistoryModel {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn dispose(&self) {
            if let Some(chat) = self.chat.upgrade() {
                if let Some(handler) = self.new_message_handler.take() {
                    chat.disconnect(handler);
                }
                if let Some(handler) = self.deleted_message_handler.take() {
                    chat.disconnect(handler);
                }
            }
        }
    }

    impl ListModelImpl for ChatHistoryModel {
        fn item_type(&self) -> glib::Type {
            model::ChatHistoryItem::static_type()
        }

        fn n_items(&self) -> u32 {
            self.list.borrow().len() as u32
        }

        /// The interface is exposed in ascending time order (oldest item at position 0)
        /// so a plain, non-reversed `GtkListView` renders new messages at the bottom.
        /// Internal storage stays newest-first.
        fn item(&self, position: u32) -> Option<glib::Object> {
            let list = self.list.borrow();
            let index = list.len().checked_sub(1)?.checked_sub(position as usize)?;
            list.get(index)
                .map(glib::object::Cast::upcast_ref::<glib::Object>)
                .cloned()
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatHistoryModel(ObjectSubclass<imp::ChatHistoryModel>)
        @implements gio::ListModel;
}

impl ChatHistoryModel {
    pub(crate) fn new(chat: &model::Chat) -> Self {
        let obj: ChatHistoryModel = glib::Object::new();

        obj.imp().chat.set(Some(chat));
        obj.imp().newest_loaded.set(true);

        let new_message_handler = chat.connect_new_message(clone!(
            #[weak]
            obj,
            move |_, message| {
                // While showing a jumped-to window that doesn't reach the newest
                // message, appending a new message at the front would fake a
                // contiguous history; it arrives once the gap is closed instead.
                if obj.imp().newest_loaded.get() {
                    obj.push_front(message);
                }
            }
        ));
        let deleted_message_handler = chat.connect_deleted_message(clone!(
            #[weak]
            obj,
            move |_, message| {
                obj.remove(message);
            }
        ));
        obj.imp()
            .new_message_handler
            .replace(Some(new_message_handler));
        obj.imp()
            .deleted_message_handler
            .replace(Some(deleted_message_handler));

        obj
    }

    /// Loads older messages from this chat history.
    ///
    /// Returns `true` when more messages can be loaded.
    pub(crate) async fn load_older_messages(&self, limit: i32) -> Result<bool, ChatHistoryError> {
        let imp = self.imp();

        if imp.is_loading.get() {
            return Err(ChatHistoryError::AlreadyLoading);
        }

        let oldest_message_id = imp
            .list
            .borrow()
            .iter()
            .rev()
            .find_map(|item| item.message())
            .map(|m| m.id())
            .unwrap_or_default();

        let chat = self.chat().ok_or(ChatHistoryError::ChatUnavailable)?;
        imp.is_loading.set(true);
        let result = chat.get_chat_history(oldest_message_id, limit).await;

        imp.is_loading.set(false);

        let messages = result.map_err(ChatHistoryError::Tdlib)?;

        if messages.is_empty() {
            return Ok(false);
        }

        self.append(messages);
        Ok(true)
    }

    /// Whether the loaded window includes the chat's newest message (i.e. no gap below).
    pub(crate) fn newest_loaded(&self) -> bool {
        self.imp().newest_loaded.get()
    }

    /// Jumps the window: clears the list and loads messages *around* `message_id` in a
    /// single request, so revealing an arbitrarily old message is instant.
    pub(crate) async fn load_around_message(
        &self,
        message_id: i64,
    ) -> Result<(), ChatHistoryError> {
        let imp = self.imp();
        if imp.is_loading.get() {
            return Err(ChatHistoryError::AlreadyLoading);
        }
        let chat = self.chat().ok_or(ChatHistoryError::ChatUnavailable)?;

        imp.is_loading.set(true);
        let result = chat.get_chat_history_around(message_id, -25, 51).await;
        imp.is_loading.set(false);
        let messages = result.map_err(ChatHistoryError::Tdlib)?;

        let removed = imp.list.borrow().len();
        imp.list.borrow_mut().clear();
        self.upcast_ref::<gio::ListModel>()
            .items_changed(0, removed as u32, 0);

        let newest_loaded = chat.last_message().is_none_or(|last| {
            messages
                .first()
                .is_some_and(|message| message.id() >= last.id())
        });
        imp.newest_loaded.set(newest_loaded);
        if !messages.is_empty() {
            self.append(messages);
        }
        Ok(())
    }

    /// Loads messages newer than the window's front (after a jump), closing the gap
    /// towards the bottom. Returns `true` while more newer messages remain.
    pub(crate) async fn load_newer_messages(&self, limit: i32) -> Result<bool, ChatHistoryError> {
        let imp = self.imp();
        if imp.newest_loaded.get() {
            return Ok(false);
        }
        if imp.is_loading.get() {
            return Err(ChatHistoryError::AlreadyLoading);
        }
        let Some(newest_id) = imp
            .list
            .borrow()
            .iter()
            .find_map(|item| item.message().map(|message| message.id()))
        else {
            return Ok(false);
        };
        let chat = self.chat().ok_or(ChatHistoryError::ChatUnavailable)?;

        imp.is_loading.set(true);
        let result = chat
            .get_chat_history_around(newest_id, -limit, limit + 1)
            .await;
        imp.is_loading.set(false);
        let mut messages = result.map_err(ChatHistoryError::Tdlib)?;
        messages.retain(|message| message.id() > newest_id);

        if messages.is_empty() {
            imp.newest_loaded.set(true);
            return Ok(false);
        }
        let reached_newest = chat.last_message().is_none_or(|last| {
            messages
                .first()
                .is_some_and(|message| message.id() >= last.id())
        });
        imp.newest_loaded.set(reached_newest);
        self.prepend(messages);
        Ok(!reached_newest)
    }

    /// Clears a jumped-to window and reloads the history from the newest message.
    pub(crate) async fn reload_to_newest(&self) -> Result<(), ChatHistoryError> {
        let imp = self.imp();
        if imp.is_loading.get() {
            return Err(ChatHistoryError::AlreadyLoading);
        }
        let removed = imp.list.borrow().len();
        imp.list.borrow_mut().clear();
        self.upcast_ref::<gio::ListModel>()
            .items_changed(0, removed as u32, 0);
        imp.newest_loaded.set(true);
        // A single request can come back short: TDLib serves the messages it
        // already has in memory and streams the rest from its database
        // asynchronously, so right after a jump the newest page may hold only a
        // couple of items. Accepting that left a few bubbles stretched across the
        // top of an otherwise empty view; keep pulling until the window holds a
        // full page (more than any viewport shows) or the history runs out.
        let mut empty_retries = 0;
        for _ in 0..10 {
            if imp.list.borrow().len() >= 30 {
                return Ok(());
            }
            match self.load_older_messages(30).await {
                Ok(true) => {}
                Ok(false) => {
                    // An empty newest page in a chat that does have messages is
                    // a failed server fetch, not the end of history; give the
                    // connection a moment and try again a few times.
                    if empty_retries < 3
                        && self.chat().and_then(|c| c.last_message()).is_some()
                        && imp.list.borrow().is_empty()
                    {
                        empty_retries += 1;
                        glib::timeout_future(std::time::Duration::from_millis(500)).await;
                        continue;
                    }
                    break;
                }
                Err(ChatHistoryError::AlreadyLoading) => {
                    glib::timeout_future(std::time::Duration::from_millis(100)).await;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn emit_items_changed_with_dividers(&self, position: u32, removed: u32, added: u32) {
        let imp = self.imp();

        // The caller has already applied its (position, removed, added) change to the
        // newest-first storage; this is the count before that change. Divider fixes below
        // widen the same range, keeping it valid against this base.
        let old_len = imp.list.borrow().len() + removed as usize - added as usize;

        let added = {
            let position = position as usize;
            let added = added as usize;

            let mut list = imp.list.borrow_mut();
            let Some(added) = insert_day_dividers(&mut list, position, added) else {
                return;
            };
            added as u32
        };

        let removed = {
            let mut list = imp.list.borrow_mut();
            remove_trailing_day_divider(&mut list, position as usize, removed as usize) as u32
        };

        let (position, removed) = {
            let mut list = imp.list.borrow_mut();
            remove_duplicate_day_divider(&mut list, position as usize, removed, added)
        };

        let position = view_emit_position(old_len, position as usize, removed as usize) as u32;
        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, removed, added);
    }

    /// For a message `item`, reports whether it is the first (topmost) and/or last
    /// (bottommost) message of a run of consecutive messages from the same sender, so
    /// the row can suppress the repeated avatar/name and tighten the shared corners.
    /// Non-message items and unknown items are treated as standalone `(true, true)`.
    pub(crate) fn group_flags(&self, item: &model::ChatHistoryItem) -> (bool, bool) {
        let list = self.imp().list.borrow();
        let Some(index) = list.iter().position(|candidate| candidate == item) else {
            return (true, true);
        };
        let Some(message) = list[index].message() else {
            return (true, true);
        };
        let groups_with = |other: Option<&model::ChatHistoryItem>| {
            other
                .and_then(model::ChatHistoryItem::message)
                .is_some_and(|other| {
                    other.sender().id() == message.sender().id()
                        && other.is_outgoing() == message.is_outgoing()
                })
        };
        // The list runs newest (index 0) to oldest, so the visually-above (older)
        // neighbour is at index + 1 and the visually-below (newer) one at index - 1.
        let is_start = !groups_with(list.get(index + 1));
        let is_end = !groups_with(index.checked_sub(1).and_then(|i| list.get(i)));
        (is_start, is_end)
    }

    /// For a selected message `item`, reports whether it is the first (topmost) and/or
    /// last (bottommost) row of a run of consecutively selected messages, so the
    /// selection tint can round only the run's outer corners and stay seamless inside.
    /// Dividers break a run. Unknown items are treated as standalone `(true, true)`.
    pub(crate) fn selection_run_flags(
        &self,
        item: &model::ChatHistoryItem,
        is_selected: &dyn Fn(i64) -> bool,
    ) -> (bool, bool) {
        let list = self.imp().list.borrow();
        let Some(index) = list.iter().position(|candidate| candidate == item) else {
            return (true, true);
        };
        let selected_neighbour = |other: Option<&model::ChatHistoryItem>| {
            other
                .and_then(model::ChatHistoryItem::message)
                .is_some_and(|message| is_selected(message.id()))
        };
        // The list runs newest (index 0) to oldest, so the visually-above (older)
        // neighbour is at index + 1 and the visually-below (newer) one at index - 1.
        let is_start = !selected_neighbour(list.get(index + 1));
        let is_end = !selected_neighbour(index.checked_sub(1).and_then(|i| list.get(i)));
        (is_start, is_end)
    }

    /// Inserts an "Unread messages" divider just before the first unread message (the
    /// oldest message with id greater than `last_read_id`), if the read/unread boundary
    /// is present in the currently loaded range and no divider exists yet.
    ///
    /// Returns `true` once a divider is present (either freshly inserted or already
    /// there), so callers can stop loading older messages.
    pub(crate) fn insert_unread_divider(&self, last_read_id: i64) -> bool {
        let imp = self.imp();

        let position = {
            let list = imp.list.borrow();
            if list
                .iter()
                .any(|item| matches!(item.type_(), model::ChatHistoryItemType::UnreadDivider(_)))
            {
                return true;
            }
            // The list runs newest (index 0) to oldest, so unread messages come first.
            // Find the first already-read message that follows an unread one — the
            // divider goes right before it.
            let mut seen_unread = false;
            let mut boundary = None;
            for (index, item) in list.iter().enumerate() {
                // Albums count through their primary (oldest member): a mixed
                // album stays on the unread side until its first message is read.
                if let Some(message) = item.message() {
                    if message.id() > last_read_id {
                        seen_unread = true;
                    } else if seen_unread {
                        boundary = Some(index);
                        break;
                    }
                }
            }
            boundary
        };

        let Some(position) = position else {
            return false;
        };

        let old_len = {
            let mut list = imp.list.borrow_mut();
            let old_len = list.len();
            list.insert(
                position,
                model::ChatHistoryItem::for_unread_divider(last_read_id),
            );
            old_len
        };
        let position = view_emit_position(old_len, position, 0) as u32;
        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, 0, 1);
        true
    }

    fn push_front(&self, message: model::Message) {
        self.add_messages(vec![message], true);
    }

    fn append(&self, messages: Vec<model::Message>) {
        self.add_messages(messages, false);
    }

    /// Inserts newer messages (given newest → oldest) at the front of the window.
    fn prepend(&self, messages: Vec<model::Message>) {
        self.add_messages(messages, true);
    }

    /// Inserts a newest-first batch of messages at the front (newer than the
    /// window) or the back (older than it), collapsing consecutive members of
    /// one media album into a single item — including across the batch
    /// boundary, so albums arriving message by message or split over two
    /// fetched pages still form one item.
    fn add_messages(&self, messages: Vec<model::Message>, front: bool) {
        if messages.is_empty() {
            return;
        }
        let imp = self.imp();
        let mut items = group_messages(messages);

        if front {
            // Late album members: TDLib can deliver an album's messages with
            // delay and interleaved with unrelated newer messages (e.g. while
            // it streams an older post from its database), so the album they
            // belong to may sit below the window's newest items. Join by
            // album identity wherever it lives, not only at the front.
            while let Some(edge) = items.len().checked_sub(1) {
                let Some(key) = album_key(&items[edge]) else {
                    break;
                };
                let position = {
                    let list = imp.list.borrow();
                    list.iter().position(|item| album_key(item) == Some(key))
                };
                let Some(position) = position else {
                    break;
                };
                let batch_item = items.remove(edge);
                {
                    let mut list = imp.list.borrow_mut();
                    let existing = list.remove(position).expect("position found moments ago");
                    let merged = merge_album_items(batch_item, existing);
                    list.insert(position, merged);
                }
                self.emit_items_changed_with_dividers(position as u32, 1, 1);
            }
        }

        // Everything in the batch may have joined existing albums already.
        if items.is_empty() {
            return;
        }

        let (position, removed, added) = {
            let mut list = imp.list.borrow_mut();
            let old_len = list.len();

            // The batch item adjacent to the window may continue an album that
            // the boundary item started (or vice versa).
            let mut removed = 0;
            if front {
                let batch_edge = items.len() - 1;
                if list
                    .front()
                    .is_some_and(|boundary| mergeable_albums(boundary, &items[batch_edge]))
                {
                    let batch_item = items.remove(batch_edge);
                    let boundary_item = list.pop_front().expect("front checked above");
                    // The batch is newer than the window's front item.
                    let merged = merge_album_items(batch_item, boundary_item);
                    items.push(merged);
                    removed = 1;
                }
            } else {
                // Appending older messages. A trailing day divider left over
                // from the previous page must not block the album merge: an
                // album never spans midnight, so when the album continues
                // across it the divider is redundant and goes with it.
                let mut boundary_index = match list.len() {
                    0 => None,
                    len => Some(len - 1),
                };
                let mut skipped_divider = false;
                if let Some(index) = boundary_index
                    && index > 0
                    && matches!(
                        list[index].type_(),
                        model::ChatHistoryItemType::DayDivider(_)
                    )
                {
                    boundary_index = Some(index - 1);
                    skipped_divider = true;
                }
                if let Some(index) = boundary_index
                    && mergeable_albums(&list[index], &items[0])
                {
                    let batch_item = items.remove(0);
                    let boundary_item = list.remove(index).expect("index checked above");
                    if skipped_divider {
                        // Removing the boundary shifted the divider into its
                        // slot; it is redundant now that the same-day album
                        // continues into the batch.
                        list.remove(index);
                    }
                    // The window item is newer than the batch.
                    let merged = merge_album_items(boundary_item, batch_item);
                    items.insert(0, merged);
                    removed = 1 + skipped_divider as usize;
                }
            }

            if front {
                for item in items.into_iter().rev() {
                    list.push_front(item);
                }
            } else {
                list.extend(items);
            }

            let added = list.len() - old_len + removed;
            let position = if front { 0 } else { list.len() - added };
            (position, removed, added)
        };

        self.emit_items_changed_with_dividers(position as u32, removed as u32, added as u32);
    }

    fn remove(&self, message: model::Message) {
        let imp = self.imp();

        let Some(index) = ({
            let list = imp.list.borrow();

            // The elements in this list are ordered. While the day dividers are ordered
            // only by their date time, the messages are additionally sorted by their id. We
            // can exploit this by applying a binary search.
            let timestamp = glib::DateTime::from_unix_utc(message.date() as i64).ok();
            let result = timestamp.and_then(|timestamp| {
                list.binary_search_by(|m| match m.type_() {
                    model::ChatHistoryItemType::Message(other_message) => {
                        message.id().cmp(&other_message.id())
                    }
                    model::ChatHistoryItemType::Album(members) => {
                        // An album spans a contiguous id range; anything inside
                        // it matches, anything outside keeps the search
                        // monotonic across the descending storage.
                        let oldest = members.first().map(|m| m.id()).unwrap_or_default();
                        let newest = members.last().map(|m| m.id()).unwrap_or_default();
                        if message.id() > newest {
                            Ordering::Less
                        } else if message.id() < oldest {
                            Ordering::Greater
                        } else {
                            Ordering::Equal
                        }
                    }
                    model::ChatHistoryItemType::DayDivider(date_time) => {
                        let ordering = timestamp.cmp(date_time);
                        if ordering == Ordering::Equal {
                            // We found the day divider of the message. Therefore, the message
                            // must be among the following elements.
                            Ordering::Greater
                        } else {
                            ordering
                        }
                    }
                    model::ChatHistoryItemType::UnreadDivider(boundary_id) => {
                        // The divider sits just above the boundary message (the read
                        // message with id == boundary, at the next-higher index). Never
                        // report Equal so the search can't match the divider itself; on
                        // an exact-boundary target report `Less`, routing the search
                        // toward that message (a higher index) to keep the comparator
                        // monotonic, rather than away from it.
                        let ordering = message.id().cmp(boundary_id);
                        if ordering == Ordering::Equal {
                            Ordering::Less
                        } else {
                            ordering
                        }
                    }
                })
                .ok()
            });
            result.or_else(|| {
                log::warn!(
                    "Message {} was not found at its expected chat history position",
                    message.id()
                );
                list.iter().position(|item| {
                    item.messages()
                        .iter()
                        .any(|other_message| other_message.id() == message.id())
                })
            })
        }) else {
            return;
        };

        // Put this in a block, so that we only need to borrow the list once and the runtime
        // borrow checker does not panic when the change notification borrows the list again.
        let (removed, added) = {
            let mut list = imp.list.borrow_mut();
            if let Some(members) = list[index].album_members().map(<[model::Message]>::to_vec) {
                let remaining: Vec<model::Message> = members
                    .into_iter()
                    .filter(|member| member.id() != message.id())
                    .collect();
                if remaining.is_empty() {
                    list.remove(index);
                    (1, 0)
                } else if remaining.len() == 1 {
                    // A one-member album is just a message again.
                    list[index] =
                        model::ChatHistoryItem::for_message(remaining.into_iter().next().unwrap());
                    (1, 1)
                } else {
                    list[index] = model::ChatHistoryItem::for_album(remaining);
                    (1, 1)
                }
            } else {
                list.remove(index);
                (1, 0)
            }
        };

        self.emit_items_changed_with_dividers(index as u32, removed, added);
    }
}

/// Translates a change notification from newest-first storage coordinates to the
/// reversed interface exposed by [`ChatHistoryModel`] as a `ListModel`. A change
/// replacing model storage positions `position..position + removed` in a list of
/// `old_len` items lands at view position `old_len - removed - position`: storage
/// index 0 (newest) is the last view row, storage index `old_len - 1` (oldest) is
/// the first.
fn view_emit_position(old_len: usize, position: usize, removed: usize) -> usize {
    old_len
        .checked_sub(position + removed)
        .expect("change range within old item count")
}

/// Groups a newest-first batch of messages into display items: consecutive
/// members of one media album collapse into a single album item (members
/// stored oldest-first), everything else stays a plain message item.
fn group_messages(messages: Vec<model::Message>) -> Vec<model::ChatHistoryItem> {
    let keys: Vec<(i64, i64, i64)> = messages
        .iter()
        .map(|message| {
            (
                message.id(),
                message.media_album_id(),
                message.sender().id(),
            )
        })
        .collect();

    let mut items = Vec::new();
    for (start, len) in album_group_ranges(&keys) {
        let batch = &messages[start..start + len];
        if len > 1 {
            let mut members = batch.to_vec();
            members.sort_by_key(|message| message.id());
            items.push(model::ChatHistoryItem::for_album(members));
        } else {
            items.push(model::ChatHistoryItem::for_message(batch[0].clone()));
        }
    }
    items
}

/// Splits a newest-first list of `(id, album_id, sender_id)` message keys into
/// runs of `(start, len)`: a run is a maximal stretch of consecutive messages
/// sharing one non-zero album id and sender; everything else is a run of one.
fn album_group_ranges(keys: &[(i64, i64, i64)]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut index = 0;
    while index < keys.len() {
        let (_, album_id, sender_id) = keys[index];
        let mut len = 1;
        if album_id != 0 {
            while index + len < keys.len()
                && keys[index + len].1 == album_id
                && keys[index + len].2 == sender_id
            {
                len += 1;
            }
        }
        runs.push((index, len));
        index += len;
    }
    runs
}

/// The album identity of an item: `(album id, sender id)` when it is a
/// (possibly single-member) album, `None` for anything else.
fn album_key(item: &model::ChatHistoryItem) -> Option<(i64, i64)> {
    let first = item.message()?;
    let album_id = first.media_album_id();
    (album_id != 0).then(|| (album_id, first.sender().id()))
}

/// Whether two adjacent items are members of the same album and should live
/// in one item.
fn mergeable_albums(a: &model::ChatHistoryItem, b: &model::ChatHistoryItem) -> bool {
    album_key(a).is_some() && album_key(a) == album_key(b)
}

/// Joins two adjacent items of one album, given in newest → oldest order.
/// The result is an album item whenever more than one message remains.
fn merge_album_items(
    newer: model::ChatHistoryItem,
    older: model::ChatHistoryItem,
) -> model::ChatHistoryItem {
    let mut members = older.messages().to_vec();
    members.extend(newer.messages().iter().cloned());
    members.sort_by_key(|message| message.id());
    if members.len() == 1 {
        model::ChatHistoryItem::for_message(members.into_iter().next().unwrap())
    } else {
        model::ChatHistoryItem::for_album(members)
    }
}

fn insert_day_dividers(
    list: &mut VecDeque<model::ChatHistoryItem>,
    position: usize,
    added: usize,
) -> Option<usize> {
    let Some(end) = position.checked_add(added) else {
        log::warn!("Chat history change range overflowed");
        return None;
    };
    if end > list.len() {
        log::warn!(
            "Chat history change range {position}..{end} exceeds list length {}",
            list.len()
        );
        return None;
    }

    let previous_day = list
        .get(end)
        .and_then(model::ChatHistoryItem::message_timestamp)
        .map(|timestamp| timestamp.ymd());
    let days = list
        .range(position..end)
        .map(|item| item.message_timestamp().map(|timestamp| timestamp.ymd()))
        .collect::<Vec<_>>();
    let divider_offsets = day_divider_offsets(&days, previous_day);

    for offset in divider_offsets.iter().copied() {
        let timestamp = list[position + offset]
            .message_timestamp()
            .expect("divider offsets only identify messages");
        list.insert(
            position + offset + 1,
            model::ChatHistoryItem::for_day_divider(timestamp),
        );
    }

    Some(added + divider_offsets.len())
}

fn remove_trailing_day_divider(
    list: &mut VecDeque<model::ChatHistoryItem>,
    position: usize,
    removed: usize,
) -> usize {
    if removed == 0
        || !matches!(
            list.get(position).map(model::ChatHistoryItem::type_),
            Some(model::ChatHistoryItemType::DayDivider(_))
        )
        || matches!(
            position
                .checked_sub(1)
                .and_then(|index| list.get(index))
                .map(model::ChatHistoryItem::type_),
            Some(model::ChatHistoryItemType::Message(_))
        )
    {
        return removed;
    }

    list.remove(position);
    removed + 1
}

fn remove_duplicate_day_divider(
    list: &mut VecDeque<model::ChatHistoryItem>,
    mut position: usize,
    mut removed: u32,
    added: u32,
) -> (u32, u32) {
    if added > 0 && position > 0 {
        let last_added_day = list
            .get(position)
            .and_then(model::ChatHistoryItem::message_timestamp)
            .map(|timestamp| timestamp.ymd());
        let divider_day = match list.get(position - 1).map(model::ChatHistoryItem::type_) {
            Some(model::ChatHistoryItemType::DayDivider(date)) => Some(date.ymd()),
            _ => None,
        };

        if last_added_day.is_some() && last_added_day == divider_day {
            list.remove(position - 1);
            removed += 1;
            position -= 1;
        }
    }

    (position as u32, removed)
}

fn day_divider_offsets(
    days: &[Option<(i32, i32, i32)>],
    mut previous_day: Option<(i32, i32, i32)>,
) -> Vec<usize> {
    let mut offsets = Vec::new();
    for (offset, day) in days.iter().enumerate().rev() {
        if day.is_some() && *day != previous_day {
            offsets.push(offset);
            previous_day = *day;
        }
    }
    offsets
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use gtk::glib;

    use super::album_group_ranges;
    use super::day_divider_offsets;
    use super::remove_trailing_day_divider;
    use super::view_emit_position;
    use crate::model;

    #[test]
    fn view_emit_positions_reverse_change_ranges() {
        // A new message inserted at storage front lands as the last view row.
        assert_eq!(view_emit_position(30, 0, 0), 30);
        // A batch of older messages appended at storage back inserts at the view top.
        assert_eq!(view_emit_position(30, 30, 0), 0);
        // Deleting storage index j maps to view index n - 1 - j.
        assert_eq!(view_emit_position(30, 5, 1), 24);
        // Clearing the whole window starts at the top of the view.
        assert_eq!(view_emit_position(30, 0, 30), 0);
    }

    #[test]
    fn divider_offsets_mark_each_day_boundary_from_oldest_to_newest() {
        let first = (2026, 7, 13);
        let second = (2026, 7, 12);
        let days = [Some(first), Some(first), Some(second), Some(second)];

        assert_eq!(day_divider_offsets(&days, None), [3, 1]);
        assert_eq!(day_divider_offsets(&days, Some(second)), [1]);
    }

    #[test]
    fn divider_offsets_ignore_non_messages() {
        let day = (2026, 7, 13);

        assert_eq!(day_divider_offsets(&[Some(day), None], None), [0]);
    }

    #[test]
    fn trailing_day_divider_is_removed_at_deleted_message_position() {
        let divider = model::ChatHistoryItem::for_day_divider(
            glib::DateTime::from_utc(2026, 7, 13, 0, 0, 0.0).unwrap(),
        );
        let following_item = model::ChatHistoryItem::for_unread_divider(42);
        let mut list = VecDeque::from([divider, following_item.clone()]);

        assert_eq!(remove_trailing_day_divider(&mut list, 0, 1), 2);
        assert_eq!(list.len(), 1);
        assert_eq!(list.front(), Some(&following_item));
    }

    #[test]
    fn album_group_ranges_without_albums_is_all_singles() {
        let keys = [(30, 0, 7), (29, 0, 7), (28, 0, 8)];
        assert_eq!(album_group_ranges(&keys), [(0, 1), (1, 1), (2, 1)]);
    }

    #[test]
    fn album_group_ranges_collapses_consecutive_members() {
        // Newest-first: members 13, 12, 11 of album 5 from sender 7.
        let keys = [(13, 5, 7), (12, 5, 7), (11, 5, 7), (10, 0, 7)];
        assert_eq!(album_group_ranges(&keys), [(0, 3), (3, 1)]);
    }

    #[test]
    fn album_group_ranges_split_on_sender_or_other_message() {
        // Same album id but an interleaved foreign message ends the run.
        let keys = [(13, 5, 7), (12, 0, 8), (11, 5, 7)];
        assert_eq!(album_group_ranges(&keys), [(0, 1), (1, 1), (2, 1)]);
        // A different sender never joins the album.
        let keys = [(13, 5, 7), (12, 5, 9)];
        assert_eq!(album_group_ranges(&keys), [(0, 1), (1, 1)]);
    }

    #[test]
    fn album_group_ranges_keeps_distinct_albums_apart() {
        let keys = [(15, 6, 7), (14, 6, 7), (13, 5, 7), (12, 5, 7)];
        assert_eq!(album_group_ranges(&keys), [(0, 2), (2, 2)]);
    }

    #[test]
    fn album_group_ranges_of_empty_batch_is_empty() {
        assert_eq!(album_group_ranges(&[]), []);
    }
}
