use std::cell::Cell;
use std::cell::RefCell;
use std::marker::PhantomData;

use gio::subclass::prelude::*;
use glib::Properties;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::model;
use crate::types::ChatFolderId;
use crate::utils;

fn changed_range<T: PartialEq>(old: &[T], new: &[T]) -> Option<(u32, u32, u32)> {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(old_item, new_item)| old_item == new_item)
        .count();

    if prefix == old.len() && prefix == new.len() {
        return None;
    }

    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(old_item, new_item)| old_item == new_item)
        .count();

    Some((
        prefix as u32,
        (old.len() - prefix - suffix) as u32,
        (new.len() - prefix - suffix) as u32,
    ))
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::ChatFolderList)]
    pub(crate) struct ChatFolderList {
        pub(super) list: RefCell<Vec<model::ChatList>>,
        #[property(get, set, construct_only)]
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        #[property(get, set, construct_only)]
        pub(super) main_chat_list: glib::WeakRef<model::ChatList>,
        #[property(get)]
        pub(super) main_chat_list_position: Cell<i32>,
        #[property(get = |this: &Self| !this.list.borrow().is_empty())]
        pub(super) has_folders: PhantomData<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatFolderList {
        const NAME: &'static str = "ChatFolderList";
        type Type = super::ChatFolderList;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ChatFolderList {
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
                .connect_items_changed(|obj, _, _, _| obj.notify("has-folders"));
        }
    }

    impl ListModelImpl for ChatFolderList {
        fn item_type(&self) -> glib::Type {
            model::ChatList::static_type()
        }

        fn n_items(&self) -> u32 {
            self.list.borrow().len() as u32 + 1
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            let obj = self.obj();

            if obj.main_chat_list_position() == position as i32 {
                obj.main_chat_list().and_upcast()
            } else {
                self.list
                    .borrow()
                    .get(if (position as i32) < obj.main_chat_list_position() {
                        position
                    } else {
                        position - 1
                    } as usize)
                    .map(|item| item.upcast_ref())
                    .cloned()
            }
        }
    }

    impl ChatFolderList {
        pub(super) fn set_main_chat_list_position(&self, position: i32) {
            let obj = self.obj();
            if obj.main_chat_list_position() == position {
                return;
            }

            let old_items = obj.snapshot();
            self.main_chat_list_position.set(position);
            let new_items = obj.snapshot();

            if let Some((position, removed, added)) = changed_range(&old_items, &new_items) {
                obj.items_changed(position, removed, added);
            }
            obj.notify_main_chat_list_position();
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatFolderList(ObjectSubclass<imp::ChatFolderList>) @implements gio::ListModel;
}

impl From<&model::ClientStateSession> for ChatFolderList {
    fn from(session: &model::ClientStateSession) -> Self {
        glib::Object::builder()
            .property("session", session)
            .property("main-chat-list", session.main_chat_list())
            .build()
    }
}

impl ChatFolderList {
    weak_getter!(session_, session, model::ClientStateSession);

    /// The combined-model position (folders interleaved with the main chat list) of a
    /// chat list, or `None` if it isn't in this model.
    pub(crate) fn position_of(&self, chat_list: &model::ChatList) -> Option<u32> {
        (0..self.n_items()).find(|&position| {
            self.item(position)
                .and_downcast::<model::ChatList>()
                .as_ref()
                == Some(chat_list)
        })
    }

    /// Moves the entry at `from_position` to `to_position` in the combined model and asks
    /// TDLib to persist the new folder order; the model re-syncs from `updateChatFolders`.
    pub(crate) fn reorder(&self, from_position: u32, to_position: u32) {
        if from_position == to_position {
            return;
        }

        // Reconstruct the combined order: folders in list order, with the main chat list
        // (represented as `None`) at its current position.
        let folders: Vec<i32> = self
            .imp()
            .list
            .borrow()
            .iter()
            .map(|chat_list| chat_list.list_type().chat_folder_id().unwrap())
            .collect();
        let main_position = (self.main_chat_list_position() as usize).min(folders.len());
        let mut entries: Vec<Option<i32>> = folders.into_iter().map(Some).collect();
        entries.insert(main_position, None);

        let from = from_position as usize;
        if from >= entries.len() {
            return;
        }
        let entry = entries.remove(from);
        entries.insert((to_position as usize).min(entries.len()), entry);

        let new_main_position = entries.iter().position(Option::is_none).unwrap_or(0) as i32;
        let new_folder_ids: Vec<i32> = entries.into_iter().flatten().collect();

        let client_id = self.session_().client_().id();
        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::reorder_chat_folders(new_folder_ids, new_main_position, client_id)
                    .await
            {
                log::warn!("Failed to reorder chat folders: {e:?}");
            }
        });
    }

    pub(crate) fn get_or_create(&self, id: ChatFolderId) -> model::ChatList {
        let mut list = self.imp().list.borrow_mut();
        list.iter()
            .find(|chat_list| chat_list.list_type().chat_folder_id().unwrap() == id.get())
            .cloned()
            .unwrap_or_else(|| {
                let chat_list = model::ChatList::new(
                    &self.session_(),
                    tdlib::enums::ChatList::Folder(tdlib::types::ChatListFolder {
                        chat_folder_id: id.get(),
                    }),
                );

                list.push(chat_list.to_owned());
                let position = list.len() as u32;

                drop(list);

                self.items_changed(position, 0, 1);

                chat_list
            })
    }

    pub(crate) fn handle_update(&self, update: tdlib::enums::Update) {
        if let tdlib::enums::Update::ChatFolders(update) = update {
            let imp = self.imp();
            let old_items = self.snapshot();
            let old_order = imp.list.borrow().clone();
            let old_main_chat_list_position = self.main_chat_list_position();
            let main_chat_list_position = update.main_chat_list_position;
            let mut order = Vec::with_capacity(update.chat_folders.len());

            for info in update.chat_folders {
                let chat_list = old_order
                    .iter()
                    .find(|chat_list| chat_list.list_type().chat_folder_id().unwrap() == info.id)
                    .cloned()
                    .unwrap_or_else(|| {
                        model::ChatList::new(
                            &self.session_(),
                            tdlib::enums::ChatList::Folder(tdlib::types::ChatListFolder {
                                chat_folder_id: info.id,
                            }),
                        )
                    });

                chat_list.set_title(info.name.text.text.as_str());
                chat_list.set_icon(info.icon.name.as_str());
                order.push(chat_list);
            }

            // The main list is inserted among the folders at this position, so an
            // out-of-range value from TDLib would make `item()` return `None` below
            // `n_items()`, violating the `ListModel` contract. Clamp it.
            let main_chat_list_position = main_chat_list_position.clamp(0, order.len() as i32);

            if old_order == order {
                imp.set_main_chat_list_position(main_chat_list_position);
                return;
            }

            *imp.list.borrow_mut() = order;
            imp.main_chat_list_position.set(main_chat_list_position);

            let new_items = self.snapshot();
            if let Some((position, removed, added)) = changed_range(&old_items, &new_items) {
                self.items_changed(position, removed, added);
            }
            if old_main_chat_list_position != main_chat_list_position {
                self.notify_main_chat_list_position();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::changed_range;

    #[test]
    fn changed_range_is_minimal_for_removals() {
        assert_eq!(changed_range(&[0, 1, 2, 3, 4], &[0, 2, 4]), Some((1, 3, 1)));
    }

    #[test]
    fn changed_range_covers_reordering() {
        assert_eq!(changed_range(&[0, 1, 2, 3], &[0, 3, 1, 2]), Some((1, 3, 3)));
    }

    #[test]
    fn changed_range_covers_main_chat_list_move() {
        assert_eq!(changed_range(&[0, 1, 2, 3], &[1, 2, 0, 3]), Some((0, 3, 3)));
    }
}
