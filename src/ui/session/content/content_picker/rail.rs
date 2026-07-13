use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::ContentPicker;
use super::EmojiRow;
use super::StickerRow;

const RAIL_SECTION_KEY: &str = "papl-rail-section";

#[derive(Debug)]
pub(super) struct RailCover {
    pub(super) button: glib::WeakRef<gtk::Button>,
    pub(super) picture: glib::WeakRef<gtk::Picture>,
    pub(super) cover: Option<tdlib::types::Sticker>,
    pub(super) thumbnail: Option<tdlib::types::File>,
    pub(super) demand: Rc<Cell<bool>>,
    pub(super) loaded: bool,
}

pub(super) fn tag_rail_section(widget: &impl IsA<gtk::Widget>, section: String) {
    // Safe: only a String is stored under this key and it is cleared with the row.
    unsafe { widget.set_data(RAIL_SECTION_KEY, section) };
}

fn widget_rail_section(widget: &gtk::Widget) -> Option<String> {
    // Safe: tag_rail_section is the only writer and always stores a String.
    unsafe {
        widget
            .data::<String>(RAIL_SECTION_KEY)
            .map(|value| value.as_ref().clone())
    }
}

impl ContentPicker {
    pub(super) fn emoji_section_name_at(&self, position: u32) -> Option<String> {
        let model = self.emoji_model();
        (0..=position).rev().find_map(|p| {
            let item = model.item(p).and_downcast::<glib::BoxedAnyObject>()?;
            match &*item.borrow::<EmojiRow>() {
                EmojiRow::Header { name, .. } => Some(name.clone()),
                _ => None,
            }
        })
    }

    pub(super) fn sticker_section_name_at(&self, position: u32) -> Option<String> {
        let model = self.sticker_model();
        (0..=position).rev().find_map(|p| {
            let item = model.item(p).and_downcast::<glib::BoxedAnyObject>()?;
            match &*item.borrow::<StickerRow>() {
                StickerRow::Header { title, .. } => Some(title.clone()),
                _ => None,
            }
        })
    }

    pub(super) fn update_active_rail_for(&self, list: &gtk::ListView, page: &gtk::ScrolledWindow) {
        if !self
            .imp()
            .content_stack
            .visible_child()
            .is_some_and(|visible| &visible == page.upcast_ref::<gtk::Widget>())
        {
            return;
        }
        // Highlight the section of whatever row is rendered at the top of the viewport.
        // `pick()` reflects what the user currently sees, so the rail stays correct during
        // scrolling without waiting for a post-scroll relayout (every row is tagged with
        // its section in the list factory).
        let x = (list.width() / 2).max(1) as f64;
        let mut widget = list.pick(x, 6.0, gtk::PickFlags::DEFAULT);
        while let Some(w) = widget {
            if let Some(section) = widget_rail_section(&w) {
                self.set_active_rail(&section);
                return;
            }
            if w.eq(list) {
                break;
            }
            widget = w.parent();
        }
    }
}
