use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::ContentPicker;
use super::EmojiRow;
use super::StickerRow;
use crate::utils;

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
        let viewport_height = page.height() as f32;
        let mut top_section: Option<(f32, String)> = None;
        for item_widget in utils::ChildIter::from(list.upcast_ref::<gtk::Widget>()) {
            let Some(bounds) = item_widget.compute_bounds(page) else {
                continue;
            };
            let top = bounds.y();
            let bottom = top + bounds.height();
            if bottom <= 1.0 || top >= viewport_height {
                continue;
            }
            if top_section.as_ref().is_none_or(|(t, _)| top < *t)
                && let Some(row) = item_widget.first_child()
                && let Some(section) = widget_rail_section(&row)
            {
                top_section = Some((top, section));
            }
        }
        if let Some((_, section)) = top_section {
            self.set_active_rail(&section);
        }
    }
}
