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
        pub(super) is_loading: Cell<bool>,
        pub(super) list: RefCell<VecDeque<model::ChatHistoryItem>>,
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
    }

    impl ListModelImpl for ChatHistoryModel {
        fn item_type(&self) -> glib::Type {
            model::ChatHistoryItem::static_type()
        }

        fn n_items(&self) -> u32 {
            self.list.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            self.list
                .borrow()
                .get(position as usize)
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

        chat.connect_new_message(clone!(
            #[weak]
            obj,
            move |_, message| {
                obj.push_front(message);
            }
        ));
        chat.connect_deleted_message(clone!(
            #[weak]
            obj,
            move |_, message| {
                obj.remove(message);
            }
        ));

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

    fn emit_items_changed_with_dividers(&self, position: u32, removed: u32, added: u32) {
        let imp = self.imp();

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

        self.upcast_ref::<gio::ListModel>()
            .items_changed(position, removed, added);
    }

    fn push_front(&self, message: model::Message) {
        self.imp()
            .list
            .borrow_mut()
            .push_front(model::ChatHistoryItem::for_message(message));

        self.emit_items_changed_with_dividers(0, 0, 1);
    }

    fn append(&self, messages: Vec<model::Message>) {
        let imp = self.imp();
        let added = messages.len();

        imp.list.borrow_mut().reserve(added);

        for message in messages {
            imp.list
                .borrow_mut()
                .push_back(model::ChatHistoryItem::for_message(message));
        }

        let index = imp.list.borrow().len() - added;
        self.emit_items_changed_with_dividers(index as u32, 0, added as u32);
    }

    fn remove(&self, message: model::Message) {
        let imp = self.imp();

        // Put this in a block, so that we only need to borrow the list once and the runtime
        // borrow checker does not panic when the change notification borrows the list again.
        let Some(index) = ({
            let mut list = imp.list.borrow_mut();

            // The elements in this list are ordered. While the day dividers are ordered
            // only by their date time, the messages are additionally sorted by their id. We
            // can exploit this by applying a binary search.
            let timestamp = glib::DateTime::from_unix_utc(message.date() as i64).ok();
            let result = timestamp.and_then(|timestamp| {
                list.binary_search_by(|m| match m.type_() {
                    model::ChatHistoryItemType::Message(other_message) => {
                        message.id().cmp(&other_message.id())
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
                })
                .ok()
            });
            let index = result.or_else(|| {
                log::warn!(
                    "Message {} was not found at its expected chat history position",
                    message.id()
                );
                list.iter().position(|item| {
                    item.message()
                        .is_some_and(|other_message| other_message.id() == message.id())
                })
            });

            index.and_then(|index| list.remove(index).map(|_| index as u32))
        }) else {
            return;
        };

        self.emit_items_changed_with_dividers(index, 1, 0);
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
        .get(position.saturating_add(1))
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
    mut removed: usize,
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

    let divider_position = position.saturating_add(removed);
    if divider_position < list.len() {
        list.remove(divider_position);
        removed += 1;
    } else {
        log::warn!("Missing trailing chat history day divider");
    }
    removed
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
    use super::day_divider_offsets;

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
}
