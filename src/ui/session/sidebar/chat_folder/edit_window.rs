use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashSet;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;

use crate::i18n::gettext_f;
use crate::model;
use crate::ui;
use crate::utils;

/// Key under which each row's checkbutton stashes its `toggled` handler, so it can be
/// disconnected when the row is recycled for another chat.
const TOGGLED_HANDLER_KEY: &str = "papl-toggled-handler";

/// Which membership set a chat list edits: the included chats or the excluded chats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Membership {
    Include,
    Exclude,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/chat_folder/edit_window.ui")]
    pub(crate) struct EditWindow {
        pub(super) session: RefCell<Option<ui::Session>>,
        pub(super) chat_folder_id: Cell<i32>,
        /// The folder as fetched, kept so any fields the editor doesn't surface survive a save.
        pub(super) folder: RefCell<Option<tdlib::types::ChatFolder>>,
        /// The chats explicitly included (pinned + included) and explicitly excluded.
        pub(super) included: RefCell<HashSet<i64>>,
        pub(super) excluded: RefCell<HashSet<i64>>,
        /// The selected TDLib icon name (empty when the folder has no icon).
        pub(super) icon_name: RefCell<String>,
        pub(super) search_text: RefCell<String>,
        pub(super) exclude_search_text: RefCell<String>,
        #[template_child]
        pub(super) name_entry: TemplateChild<gtk::Entry>,
        #[template_child]
        pub(super) icon_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub(super) search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) filter_model: TemplateChild<gtk::FilterListModel>,
        #[template_child]
        pub(super) custom_filter: TemplateChild<gtk::CustomFilter>,
        #[template_child]
        pub(super) exclude_search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub(super) exclude_list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) exclude_filter_model: TemplateChild<gtk::FilterListModel>,
        #[template_child]
        pub(super) exclude_custom_filter: TemplateChild<gtk::CustomFilter>,
        #[template_child]
        pub(super) include_contacts_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) include_non_contacts_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) include_groups_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) include_channels_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) include_bots_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) exclude_muted_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) exclude_read_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub(super) exclude_archived_row: TemplateChild<adw::SwitchRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EditWindow {
        const NAME: &'static str = "PaplSidebarChatFolderEditWindow";
        type Type = super::EditWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EditWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.setup_search_filter(&self.custom_filter, Membership::Include);
            obj.setup_search_filter(&self.exclude_custom_filter, Membership::Exclude);

            obj.setup_factory(Membership::Include);
            obj.setup_factory(Membership::Exclude);
            obj.setup_icon_picker();
        }
    }

    impl WidgetImpl for EditWindow {}
    impl WindowImpl for EditWindow {}
    impl AdwWindowImpl for EditWindow {}

    #[gtk::template_callbacks]
    impl EditWindow {
        #[template_callback]
        fn on_search_changed(&self) {
            self.search_text
                .replace(self.search_entry.text().to_string());
            self.custom_filter.changed(gtk::FilterChange::Different);
        }

        #[template_callback]
        fn on_exclude_search_changed(&self) {
            self.exclude_search_text
                .replace(self.exclude_search_entry.text().to_string());
            self.exclude_custom_filter
                .changed(gtk::FilterChange::Different);
        }

        #[template_callback]
        fn on_cancel(&self) {
            self.obj().close();
        }

        #[template_callback]
        fn on_save(&self) {
            self.obj().save();
        }
    }
}

glib::wrapper! {
    pub(crate) struct EditWindow(ObjectSubclass<imp::EditWindow>)
        @extends gtk::Widget, gtk::Window, adw::Window,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl EditWindow {
    pub(crate) fn new(
        parent: Option<&gtk::Window>,
        session: &ui::Session,
        chat_folder_id: i32,
    ) -> Self {
        let obj: Self = glib::Object::builder()
            .property("transient-for", parent)
            .build();

        let imp = obj.imp();
        imp.session.replace(Some(session.clone()));
        imp.chat_folder_id.set(chat_folder_id);

        if let Some(model) = session.model() {
            imp.filter_model.set_model(Some(&model.main_chat_list()));
            imp.exclude_filter_model
                .set_model(Some(&model.main_chat_list()));
        }

        obj.load_folder();
        obj
    }

    /// Filters a chat list to titles containing that tab's search text (case-insensitive).
    fn setup_search_filter(&self, filter: &gtk::CustomFilter, membership: Membership) {
        filter.set_filter_func(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            false,
            move |item| {
                let imp = obj.imp();
                let search = match membership {
                    Membership::Include => imp.search_text.borrow(),
                    Membership::Exclude => imp.exclude_search_text.borrow(),
                }
                .to_lowercase();
                if search.is_empty() {
                    return true;
                }
                item.downcast_ref::<model::ChatListItem>()
                    .map(|item| item.chat_().title().to_lowercase().contains(&search))
                    .unwrap_or(false)
            }
        ));
    }

    fn setup_icon_picker(&self) {
        let imp = self.imp();
        let flow = gtk::FlowBox::builder()
            .max_children_per_line(6)
            .min_children_per_line(6)
            .selection_mode(gtk::SelectionMode::None)
            .row_spacing(4)
            .column_spacing(4)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        for (name, symbolic) in super::FOLDER_ICONS {
            let button = gtk::Button::from_icon_name(symbolic);
            button.add_css_class("flat");
            button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.set_selected_icon(name);
                    obj.imp().icon_button.popdown();
                }
            ));
            flow.append(&button);
        }

        let popover = gtk::Popover::builder().child(&flow).build();
        imp.icon_button.set_popover(Some(&popover));
        self.set_selected_icon("");
    }

    /// Reflects `name` as the chosen folder icon, updating the picker button's glyph.
    fn set_selected_icon(&self, name: &str) {
        self.imp()
            .icon_button
            .set_icon_name(super::folder_icon_symbolic(name));
        self.imp().icon_name.replace(name.to_string());
    }

    /// Fetches the folder and fills in the name, icon, chat-type flags, and memberships.
    fn load_folder(&self) {
        let Some(session) = self.imp().session.borrow().clone() else {
            return;
        };
        let Some(model) = session.model() else {
            return;
        };
        let client_id = model.client_().id();
        let chat_folder_id = self.imp().chat_folder_id.get();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let Ok(tdlib::enums::ChatFolder::ChatFolder(folder)) =
                    tdlib::functions::get_chat_folder(chat_folder_id, client_id).await
                else {
                    return;
                };

                let imp = obj.imp();
                imp.name_entry.set_text(&folder.name.text.text);

                obj.set_selected_icon(
                    folder
                        .icon
                        .as_ref()
                        .map(|icon| icon.name.as_str())
                        .unwrap_or(""),
                );

                imp.include_contacts_row.set_active(folder.include_contacts);
                imp.include_non_contacts_row
                    .set_active(folder.include_non_contacts);
                imp.include_groups_row.set_active(folder.include_groups);
                imp.include_channels_row.set_active(folder.include_channels);
                imp.include_bots_row.set_active(folder.include_bots);
                imp.exclude_muted_row.set_active(folder.exclude_muted);
                imp.exclude_read_row.set_active(folder.exclude_read);
                imp.exclude_archived_row.set_active(folder.exclude_archived);

                let included: HashSet<i64> = folder
                    .pinned_chat_ids
                    .iter()
                    .chain(folder.included_chat_ids.iter())
                    .copied()
                    .collect();
                imp.included.replace(included);
                imp.excluded
                    .replace(folder.excluded_chat_ids.iter().copied().collect());
                imp.folder.replace(Some(folder));

                // Rows may have been bound (all unchecked) before this async load finished;
                // re-set the factories so every visible row rebinds against the loaded
                // membership and shows the right checkbox state.
                obj.setup_factory(Membership::Include);
                obj.setup_factory(Membership::Exclude);
            }
        ));
    }

    fn setup_factory(&self, membership: Membership) {
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();

            let row = gtk::Box::builder().spacing(6).build();
            let avatar: ui::Avatar = glib::Object::builder().property("size", 32).build();
            let label = gtk::Inscription::builder()
                .hexpand(true)
                .text_overflow(gtk::InscriptionOverflow::EllipsizeEnd)
                .build();
            let check = gtk::CheckButton::builder()
                .valign(gtk::Align::Center)
                .build();

            row.append(&avatar);
            row.append(&label);
            row.append(&check);
            item.set_child(Some(&row));
        });

        factory.connect_bind(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let Some(chat_item) = item.item().and_downcast::<model::ChatListItem>() else {
                    return;
                };
                let Some(row) = item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let chat = chat_item.chat_();
                let chat_id = chat.id();

                let avatar = row.first_child().and_downcast::<ui::Avatar>().unwrap();
                avatar.set_item(Some(chat.clone().upcast()));
                let label = avatar
                    .next_sibling()
                    .and_downcast::<gtk::Inscription>()
                    .unwrap();
                label.set_text(Some(&chat.title()));
                let check = row.last_child().and_downcast::<gtk::CheckButton>().unwrap();

                // No handler is connected here (unbind removed any previous one), so
                // seeding the checkbox state can't feed back into the membership set.
                check.set_active(obj.membership_set(membership).borrow().contains(&chat_id));

                let handler = check.connect_toggled(clone!(
                    #[weak]
                    obj,
                    move |check| {
                        let mut set = obj.membership_set(membership).borrow_mut();
                        if check.is_active() {
                            set.insert(chat_id);
                        } else {
                            set.remove(&chat_id);
                        }
                    }
                ));
                // Safe: only a SignalHandlerId is stored under this key, removed on unbind.
                unsafe { check.set_data(TOGGLED_HANDLER_KEY, handler) };
            }
        ));

        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(row) = item.child().and_downcast::<gtk::Box>()
                && let Some(check) = row.last_child().and_downcast::<gtk::CheckButton>()
            {
                // Safe: the id was stored by connect_bind above.
                if let Some(handler) =
                    unsafe { check.steal_data::<glib::SignalHandlerId>(TOGGLED_HANDLER_KEY) }
                {
                    check.disconnect(handler);
                }
            }
        });

        match membership {
            Membership::Include => self.imp().list_view.set_factory(Some(&factory)),
            Membership::Exclude => self.imp().exclude_list_view.set_factory(Some(&factory)),
        }
    }

    fn membership_set(&self, membership: Membership) -> &RefCell<HashSet<i64>> {
        match membership {
            Membership::Include => &self.imp().included,
            Membership::Exclude => &self.imp().excluded,
        }
    }

    fn save(&self) {
        let imp = self.imp();
        let Some(session) = imp.session.borrow().clone() else {
            return;
        };
        let Some(model) = session.model() else {
            return;
        };
        let Some(mut folder) = imp.folder.borrow().clone() else {
            return;
        };
        let client_id = model.client_().id();
        let chat_folder_id = imp.chat_folder_id.get();

        // Keep the entities-free title from the entry; changing the text invalidates any
        // old entity offsets, so drop them.
        folder.name.text.text = imp.name_entry.text().trim().to_string();
        folder.name.text.entities = Vec::new();

        folder.icon = {
            let name = imp.icon_name.borrow();
            (!name.is_empty()).then(|| tdlib::types::ChatFolderIcon {
                name: name.clone(),
            })
        };

        folder.include_contacts = imp.include_contacts_row.is_active();
        folder.include_non_contacts = imp.include_non_contacts_row.is_active();
        folder.include_groups = imp.include_groups_row.is_active();
        folder.include_channels = imp.include_channels_row.is_active();
        folder.include_bots = imp.include_bots_row.is_active();
        folder.exclude_muted = imp.exclude_muted_row.is_active();
        folder.exclude_read = imp.exclude_read_row.is_active();
        folder.exclude_archived = imp.exclude_archived_row.is_active();

        let included = imp.included.borrow();
        // A chat can't be both included and excluded; included wins.
        let excluded: HashSet<i64> = imp
            .excluded
            .borrow()
            .difference(&included)
            .copied()
            .collect();

        // Pinned chats stay pinned only while still included; everything else included goes
        // into included_chat_ids.
        folder.pinned_chat_ids.retain(|id| included.contains(id));
        folder.included_chat_ids = included
            .iter()
            .copied()
            .filter(|id| !folder.pinned_chat_ids.contains(id))
            .collect();
        folder.excluded_chat_ids = excluded.into_iter().collect();
        drop(included);

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                match tdlib::functions::edit_chat_folder(chat_folder_id, folder, client_id).await {
                    Ok(_) => obj.close(),
                    Err(e) => utils::show_toast(
                        &obj,
                        gettext_f("Failed to save folder: {error}", &[("error", &e.message)]),
                    ),
                }
            }
        ));
    }
}
