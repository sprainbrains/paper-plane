// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::glib;

use crate::model;
use crate::strings;
use crate::ui;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct MentionPopup {
        /// Candidates backing the current rows, indexed by row position.
        pub(super) candidates: RefCell<Vec<model::User>>,
        pub(super) list_box: OnceCell<gtk::ListBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MentionPopup {
        const NAME: &'static str = "PaplMentionPopup";
        type Type = super::MentionPopup;
        type ParentType = gtk::Popover;
    }

    impl ObjectImpl for MentionPopup {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("candidate-activated")
                        .param_types([model::User::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.obj().add_css_class("mention-popup");

            // `GtkPopover:child` cannot be filled from a composite template,
            // so the candidate list is built here.
            let list_box = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::Single)
                .build();
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .max_content_height(240)
                .width_request(280)
                .child(&list_box)
                .build();
            // `adw::prelude` has several unrelated `set_content` traits; pick
            // the popover's implementation explicitly.
            <super::MentionPopup as gtk::prelude::PopoverExt>::set_child(
                self.obj().as_ref(),
                Some(&scroller),
            );

            let obj = &*self.obj();
            list_box.connect_row_activated(clone!(
                #[weak]
                obj,
                move |_, row| {
                    if let Some(user) = obj.candidate_at(row.index()) {
                        obj.emit_by_name::<()>("candidate-activated", &[&user]);
                    }
                }
            ));
            self.list_box.set(list_box).unwrap();
        }
    }

    impl WidgetImpl for MentionPopup {}

    // `adw::subclass::prelude` also exports a `PopoverImpl` (for `AdwPopover`);
    // this popup extends a plain `GtkPopover`.
    impl gtk::subclass::prelude::PopoverImpl for MentionPopup {}
}

glib::wrapper! {
    pub(crate) struct MentionPopup(ObjectSubclass<imp::MentionPopup>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::ShortcutManager;
}

impl MentionPopup {
    pub(crate) fn new() -> Self {
        glib::Object::builder()
            .property("autohide", true)
            .property("has-arrow", false)
            .build()
    }

    /// Replaces the candidate list, selects the first row, and shows the popup.
    pub(crate) fn set_candidates(&self, users: &[model::User]) {
        let imp = self.imp();
        let list_box = imp.list_box.get().unwrap();
        imp.candidates.replace(users.to_vec());
        while let Some(row) = list_box.row_at_index(0) {
            list_box.remove(&row);
        }
        for user in users {
            list_box.append(&Self::build_row(user));
        }
        let first = list_box.row_at_index(0);
        list_box.select_row(first.as_ref());
        // `popup` rather than `present`: on this GTK version present() left
        // the popover unmapped even with a mapped parent (a plain
        // `gtk::Popover` probe behaved identically), while popup() shows it.
        <Self as gtk::prelude::PopoverExt>::popup(self);
    }

    fn build_row(user: &model::User) -> gtk::ListBoxRow {
        let row_content = adw::ActionRow::builder()
            .title(strings::user_display_name(user, true))
            .activatable(false)
            .build();
        let avatar = ui::Avatar::new();
        avatar.set_size(32);
        avatar.set_item(Some(user.clone().upcast()));
        row_content.add_prefix(&avatar);
        let username = user.username();
        if !username.is_empty() {
            row_content.set_subtitle(&format!("@{username}"));
        }

        let row = gtk::ListBoxRow::builder().activatable(true).build();
        row.set_child(Some(&row_content));
        row
    }

    /// Moves the highlighted row by `delta`, wrapping around like the list
    /// navigation in other Telegram clients.
    pub(crate) fn move_selected(&self, delta: i32) {
        let imp = self.imp();
        let list_box = imp.list_box.get().unwrap();
        let count = self.candidate_count();
        if count == 0 {
            return;
        }
        let current = list_box.selected_row().map(|row| row.index()).unwrap_or(0);
        let next = (current + delta).rem_euclid(count);
        if let Some(row) = list_box.row_at_index(next) {
            list_box.select_row(Some(&row));
        }
    }

    /// Emits `candidate-activated` for the highlighted row, exactly like a
    /// click on it.
    pub(crate) fn activate_selected(&self) {
        let user = self
            .imp()
            .list_box
            .get()
            .unwrap()
            .selected_row()
            .and_then(|row| self.candidate_at(row.index()));
        if let Some(user) = user {
            self.emit_by_name::<()>("candidate-activated", &[&user]);
        }
    }

    fn candidate_at(&self, index: i32) -> Option<model::User> {
        self.imp().candidates.borrow().get(index as usize).cloned()
    }

    pub(crate) fn candidate_count(&self) -> i32 {
        self.imp().candidates.borrow().len() as i32
    }

    pub(crate) fn selected_index(&self) -> Option<i32> {
        self.imp()
            .list_box
            .get()
            .unwrap()
            .selected_row()
            .map(|row| row.index())
    }

    pub(crate) fn connect_candidate_activated<F: Fn(&Self, &model::User) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("candidate-activated", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let user = values[1].get::<model::User>().unwrap();
            f(&obj, &user);

            None
        })
    }
}

impl Default for MentionPopup {
    fn default() -> Self {
        Self::new()
    }
}
