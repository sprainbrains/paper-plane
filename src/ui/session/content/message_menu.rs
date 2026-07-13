use gettextrs::gettext;
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;

/// One icon+label row of the context menu, bound to a `message-row.*` action.
struct MenuEntry {
    action: &'static str,
    icon: &'static str,
    label: &'static str,
    destructive: bool,
}

/// The primary section of message actions. `Add Reaction` is intentionally absent
/// — the reactions bar above the menu (and its chevron to the full picker) fully
/// replaces it.
const PRIMARY: &[MenuEntry] = &[
    MenuEntry {
        action: "message-row.reply",
        icon: "mail-reply-sender-symbolic",
        label: "Reply",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.forward",
        icon: "mail-forward-symbolic",
        label: "Forward",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.copy",
        icon: "edit-copy-symbolic",
        label: "Copy Text",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.copy-link",
        icon: "chain-link-symbolic",
        label: "Copy Link",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.pin",
        icon: "view-pin-symbolic",
        label: "Pin",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.edit",
        icon: "edit-symbolic",
        label: "Edit",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.open",
        icon: "find-location-symbolic",
        label: "Open",
        destructive: false,
    },
    MenuEntry {
        action: "message-row.revoke-delete",
        icon: "user-trash-symbolic",
        label: "Delete for Everyone",
        destructive: true,
    },
    MenuEntry {
        action: "message-row.delete",
        icon: "user-trash-symbolic",
        label: "Delete for Me",
        destructive: true,
    },
];

/// Development-only tools, shown below a separator.
const SECONDARY: &[MenuEntry] = &[MenuEntry {
    action: "message-row.copy-debug",
    icon: "view-reveal-symbolic",
    label: "Copy Debug Entry",
    destructive: false,
}];

mod imp {
    use std::cell::OnceCell;
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct MessageMenu {
        pub(super) reactions: OnceCell<ui::MessageMenuReactions>,
        pub(super) separator: OnceCell<gtk::Separator>,
        /// Each menu row, keyed by its action, so visibility can be recomputed per
        /// message at popup time.
        pub(super) rows: RefCell<Vec<(&'static str, gtk::Button)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageMenu {
        const NAME: &'static str = "PaplMessageMenu";
        type Type = super::MessageMenu;
        type ParentType = gtk::Popover;
    }

    impl ObjectImpl for MessageMenu {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_has_arrow(false);
            obj.add_css_class("message-menu");

            let container = gtk::Box::new(gtk::Orientation::Vertical, 6);

            let reactions: ui::MessageMenuReactions = glib::Object::new();
            container.append(&reactions);
            let _ = self.reactions.set(reactions);

            let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
            card.add_css_class("message-menu-card");

            let mut rows = self.rows.borrow_mut();
            for entry in PRIMARY {
                let button = obj.build_row(entry);
                card.append(&button);
                rows.push((entry.action, button));
            }

            let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
            separator.add_css_class("message-menu-separator");
            card.append(&separator);
            let _ = self.separator.set(separator);

            for entry in SECONDARY {
                let button = obj.build_row(entry);
                card.append(&button);
                rows.push((entry.action, button));
            }
            drop(rows);

            container.append(&card);
            obj.set_child(Some(&container));
        }
    }

    impl WidgetImpl for MessageMenu {}
    impl PopoverImpl for MessageMenu {}
}

glib::wrapper! {
    pub(crate) struct MessageMenu(ObjectSubclass<imp::MessageMenu>)
        @extends gtk::Widget, gtk::Popover,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::ShortcutManager;
}

impl Default for MessageMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageMenu {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn build_row(&self, entry: &MenuEntry) -> gtk::Button {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        content.append(&gtk::Image::from_icon_name(entry.icon));
        let label = gtk::Label::new(Some(&gettext(entry.label)));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        content.append(&label);

        let button = gtk::Button::builder().child(&content).build();
        button.add_css_class("flat");
        button.add_css_class("message-menu-item");
        if entry.destructive {
            button.add_css_class("message-menu-item-destructive");
        }

        let action = entry.action;
        button.connect_clicked(clone!(
            #[weak(rename_to = menu)]
            self,
            move |_| {
                let _ = menu.activate_action(action, None);
                menu.popdown();
            }
        ));

        button
    }

    /// Recomputes each row's visibility and the reactions bar for `row`'s message.
    /// Called just before popping up, since this menu is a shared singleton
    /// re-parented per message.
    pub(crate) fn rebind(&self, row: &ui::MessageRow) {
        let imp = self.imp();
        for (action, button) in imp.rows.borrow().iter() {
            button.set_visible(row.menu_action_enabled(action));
        }
        if let Some(separator) = imp.separator.get() {
            separator.set_visible(row.menu_action_enabled("message-row.copy-debug"));
        }

        if let Some(reactions) = imp.reactions.get() {
            if let Some(message) = row.message().downcast_ref::<model::Message>() {
                reactions.load_for(message);
            } else {
                reactions.set_visible(false);
            }
        }
    }
}
