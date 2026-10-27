// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::glib;

/// A boxed `BotCommand` so the `candidate-activated` signal can carry it.
#[derive(Clone, Debug, PartialEq, glib::Boxed)]
#[boxed_type(name = "CommandPopupBotCommand")]
pub(crate) struct PopupBotCommand(pub(crate) tdlib::types::BotCommand);

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct CommandPopup {
        /// Candidates backing the current rows, indexed by row position.
        pub(super) candidates: RefCell<Vec<tdlib::types::BotCommand>>,
        pub(super) list_box: OnceCell<gtk::ListBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CommandPopup {
        const NAME: &'static str = "PaplCommandPopup";
        type Type = super::CommandPopup;
        type ParentType = gtk::Popover;
    }

    impl ObjectImpl for CommandPopup {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: OnceLock<Vec<glib::subclass::Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("candidate-activated")
                        .param_types([PopupBotCommand::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.obj().add_css_class("command-popup");

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
            <super::CommandPopup as gtk::prelude::PopoverExt>::set_child(
                self.obj().as_ref(),
                Some(&scroller),
            );

            let obj = &*self.obj();
            list_box.connect_row_activated(clone!(
                #[weak]
                obj,
                move |_, row| {
                    if let Some(command) = obj.candidate_at(row.index()) {
                        obj.emit_by_name::<()>("candidate-activated", &[&command]);
                    }
                }
            ));
            self.list_box.set(list_box).unwrap();
        }
    }

    impl WidgetImpl for CommandPopup {}

    // `adw::subclass::prelude` also exports a `PopoverImpl` (for `AdwPopover`);
    // this popup extends a plain `GtkPopover`.
    impl gtk::subclass::prelude::PopoverImpl for CommandPopup {}
}

glib::wrapper! {
    pub(crate) struct CommandPopup(ObjectSubclass<imp::CommandPopup>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::ShortcutManager;
}

#[allow(dead_code)]
impl CommandPopup {
    pub(crate) fn new() -> Self {
        glib::Object::builder()
            .property("autohide", true)
            .property("has-arrow", false)
            .build()
    }

    /// Replaces the candidate list, selects the first row, and shows the popup.
    pub(crate) fn set_candidates(&self, commands: &[tdlib::types::BotCommand]) {
        let imp = self.imp();
        let list_box = imp.list_box.get().unwrap();
        imp.candidates.replace(commands.to_vec());
        while let Some(row) = list_box.row_at_index(0) {
            list_box.remove(&row);
        }
        for command in commands {
            list_box.append(&Self::build_row(command));
        }
        let first = list_box.row_at_index(0);
        list_box.select_row(first.as_ref());
        // `popup` rather than `present`: on this GTK version present() left
        // the popover unmapped even with a mapped parent (a plain
        // `gtk::Popover` probe behaved identically), while popup() shows it.
        <Self as gtk::prelude::PopoverExt>::popup(self);
    }

    fn build_row(command: &tdlib::types::BotCommand) -> gtk::ListBoxRow {
        let row_content = adw::ActionRow::builder()
            .title(format!("/{}", command.command))
            .subtitle(&command.description)
            .activatable(false)
            .build();
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
        let command = self
            .imp()
            .list_box
            .get()
            .unwrap()
            .selected_row()
            .and_then(|row| self.candidate_at(row.index()));
        if let Some(command) = command {
            self.emit_by_name::<()>("candidate-activated", &[&command]);
        }
    }

    fn candidate_at(&self, index: i32) -> Option<PopupBotCommand> {
        self.imp()
            .candidates
            .borrow()
            .get(index as usize)
            .cloned()
            .map(PopupBotCommand)
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

    pub(crate) fn connect_candidate_activated<F: Fn(&Self, PopupBotCommand) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("candidate-activated", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let command = values[1].get::<PopupBotCommand>().unwrap();
            f(&obj, command);

            None
        })
    }
}

impl Default for CommandPopup {
    fn default() -> Self {
        Self::new()
    }
}
