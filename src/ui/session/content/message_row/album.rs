use std::cell::Cell;
use std::cell::RefCell;

use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::super::ALBUM_CELL_SIZE;
use super::super::ALBUM_CELL_SPACING;
use super::super::album_placements;
use super::base::MessageHandler;
use crate::model;
use crate::ui;
use crate::utils;

/// The photo file best matching a `target_px` square cell: the smallest
/// available size that still covers it, falling back to the largest one.
/// Videos and animations carry a single thumbnail file instead of a size
/// ladder and are used as-is.
fn cell_photo_file(
    content: &tdlib::enums::MessageContent,
    target_px: i32,
) -> Option<tdlib::types::File> {
    use tdlib::enums::MessageContent::*;
    let sizes: &[tdlib::types::PhotoSize] = match content {
        MessagePhoto(data) => &data.photo.sizes,
        MessageVideo(data) => {
            return data.video.thumbnail.as_ref().map(|t| t.file.clone());
        }
        MessageAnimation(data) => {
            return data.animation.thumbnail.as_ref().map(|t| t.file.clone());
        }
        _ => return None,
    };

    let mut covering: Option<(i64, tdlib::types::File)> = None;
    let mut biggest: Option<(i64, tdlib::types::File)> = None;
    for size in sizes {
        let area = size.width as i64 * size.height as i64;
        if size.width.min(size.height) >= target_px
            && covering.as_ref().is_none_or(|(a, _)| area < *a)
        {
            covering = Some((area, size.photo.clone()));
        }
        if biggest.as_ref().is_none_or(|(a, _)| area > *a) {
            biggest = Some((area, size.photo.clone()));
        }
    }
    covering.or(biggest).map(|(_, file)| file)
}

/// A rendered cell waiting for (or resuming) its crisp photo.
#[derive(Clone, Debug)]
struct CellEntry {
    picture: ui::MediaPicture,
    file: tdlib::types::File,
    /// Cell edge in device pixels; also the cover-crop bake size.
    target_px: i32,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/album.ui")]
    #[properties(wrapper_type = super::MessageAlbum)]
    pub(crate) struct MessageAlbum {
        pub(super) binding: RefCell<Option<gtk::ExpressionWatch>>,
        pub(super) handler: MessageHandler,
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::ChatHistoryItem>,
        /// Bumped on every rebuild and on unmap; an in-flight cell load whose
        /// generation no longer matches is dropped.
        pub(super) generation: Cell<u64>,
        /// Whether the row is mapped: off-screen cells download nothing
        /// (see the virtualized-list rules in AGENTS.md).
        pub(super) demand: Cell<bool>,
        pub(super) cells: RefCell<Vec<CellEntry>>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) album_grid: TemplateChild<gtk::Grid>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageAlbum {
        const NAME: &'static str = "PaplMessageAlbum";
        type Type = super::MessageAlbum;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageAlbum {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }

    impl WidgetImpl for MessageAlbum {
        fn map(&self) {
            self.parent_map();
            self.demand.set(true);
            self.generation.set(self.generation.get() + 1);
            let generation = self.generation.get();
            let cells = self.cells.borrow().clone();
            let obj = &*self.obj();
            for cell in cells {
                obj.load_cell(&cell, generation);
            }
        }

        fn unmap(&self) {
            self.demand.set(false);
            // Abandon in-flight loads: the reply would fail the generation
            // check and touch nothing.
            self.generation.set(self.generation.get() + 1);
            self.parent_unmap();
        }
    }

    impl ui::MessageBaseImpl for MessageAlbum {}

    impl MessageAlbum {
        fn set_message(&self, message: Option<model::ChatHistoryItem>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageAlbum(ObjectSubclass<imp::MessageAlbum>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageAlbum {
    type Message = model::ChatHistoryItem;

    fn set_message(&self, item: &Self::Message) {
        let imp = self.imp();

        let old_item = imp.message.upgrade();
        if old_item.as_ref() == Some(item) {
            return;
        }

        let Some(primary) = item.message().cloned() else {
            return;
        };

        if let Some(binding) = imp.binding.borrow_mut().take() {
            binding.unwatch();
        }
        imp.handler.disconnect();
        imp.message.set(Some(item));
        imp.message_bubble.update_from_message(&primary, true);

        // The caption lives on the album's primary (first) member; keep the
        // bubble label in sync with edits.
        let binding = model::Message::this_expression("content")
            .chain_closure::<String>(closure!(
                |_: model::Message, content: model::BoxedMessageContent| {
                    use tdlib::enums::MessageContent::*;
                    match content.0 {
                        MessagePhoto(data) => utils::parse_formatted_text(data.caption),
                        MessageVideo(data) => utils::parse_formatted_text(data.caption),
                        _ => String::new(),
                    }
                }
            ))
            .bind(&*imp.message_bubble, "label", Some(&primary));
        imp.binding.replace(Some(binding));

        // A member's content can change in place (e.g. TTL expiry); repaint the
        // grid from the primary's notification like the photo row does.
        let handler_id = primary.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.rebuild_grid(),
        ));
        imp.handler.set(&primary, handler_id);

        self.rebuild_grid();

        self.notify("message");
    }
}

impl MessageAlbum {
    /// Rebuilds the cell grid from the album's current members.
    fn rebuild_grid(&self) {
        let imp = self.imp();
        let Some(item) = imp.message.upgrade() else {
            return;
        };
        // Invalidate every in-flight load from the previous content.
        imp.generation.set(imp.generation.get() + 1);
        imp.cells.borrow_mut().clear();

        while let Some(child) = imp.album_grid.first_child() {
            imp.album_grid.remove(&child);
        }

        let members: Vec<model::Message> = match item.album_members() {
            Some(members) => members.to_vec(),
            // A single-member "album" collapsed in the model; show the one
            // message full-width rather than as a cell.
            None => item.message().into_iter().cloned().collect(),
        };
        let placements = match item.album_members() {
            Some(_) => album_placements(members.len()),
            None => vec![(0, 0, 2, 1)],
        };

        let generation = imp.generation.get();
        let scale = self.scale_factor().max(1);
        for (member, (col, row, w, h)) in members.iter().zip(placements) {
            let width = if w == 2 {
                ALBUM_CELL_SIZE * 2 + ALBUM_CELL_SPACING
            } else {
                ALBUM_CELL_SIZE
            };
            let height = if h == 2 {
                ALBUM_CELL_SIZE * 2 + ALBUM_CELL_SPACING
            } else {
                ALBUM_CELL_SIZE
            };
            let target_px = width * scale;
            let cell = self.build_cell(member, width, height, target_px);
            imp.album_grid.attach(&cell.widget, col, row, w, h);
            if let Some(entry) = cell.entry {
                imp.cells.borrow_mut().push(entry.clone());
                if imp.demand.get() {
                    self.load_cell(&entry, generation);
                }
            }
        }
    }

    /// One square cell: the minithumbnail as an instant placeholder, upgraded
    /// to a downloaded crisp photo by `load_cell`; a play badge on video
    /// cells, and a click that opens the full media.
    fn build_cell(
        &self,
        message: &model::Message,
        width: i32,
        height: i32,
        target_px: i32,
    ) -> BuiltCell {
        let picture: ui::MediaPicture = glib::Object::new();
        picture.set_aspect_ratio(1.0);
        picture.set_size_request(width, height);
        picture.set_hexpand(true);
        picture.set_vexpand(true);

        let content = message.content().0;
        let placeholder = utils::content_minithumbnail(&content).and_then(|mini| {
            gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(&mini.data))).ok()
        });
        if let Some(texture) = placeholder.as_ref() {
            if let Some(paintable) = utils::cover_paintable(texture, target_px as f32) {
                picture.set_paintable(Some(&paintable));
            } else {
                picture.set_paintable(Some(texture.upcast_ref::<gdk::Paintable>()));
            }
        }

        let file = cell_photo_file(&content, target_px);

        let is_video = matches!(
            content,
            tdlib::enums::MessageContent::MessageVideo(_)
                | tdlib::enums::MessageContent::MessageAnimation(_)
                | tdlib::enums::MessageContent::MessageVideoNote(_)
        );
        let widget: gtk::Widget = if is_video {
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&picture));
            let badge = gtk::Image::from_icon_name("media-playback-start-symbolic");
            badge.set_halign(gtk::Align::Center);
            badge.set_valign(gtk::Align::Center);
            badge.set_pixel_size(40);
            badge.set_opacity(0.85);
            overlay.add_overlay(&badge);
            overlay.upcast()
        } else {
            picture.clone().upcast()
        };
        widget.set_cursor_from_name(Some("pointer"));

        let message = message.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(clone!(
            #[weak]
            widget,
            move |gesture, n_press, _, _| {
                if n_press != 1 {
                    return;
                }
                if let Some(window) = utils::ancestor::<_, ui::Window>(&widget) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    window.open_media_viewer(&message);
                }
            }
        ));
        widget.add_controller(gesture);

        BuiltCell {
            widget,
            entry: file.map(|file| CellEntry {
                picture,
                file,
                target_px,
            }),
        }
    }

    /// Upgrade one cell from placeholder to its real photo: decode straight
    /// from the local file when TDLib already has it, otherwise download
    /// first. The generation check (plus the demand flag) makes every reply
    /// that raced a rebuild or an unmap a no-op.
    fn load_cell(&self, cell: &CellEntry, generation: u64) {
        let imp = self.imp();
        if !imp.demand.get() || imp.generation.get() != generation {
            return;
        }
        let file = cell.file.clone();
        if file.local.is_downloading_completed && !file.local.path.is_empty() {
            self.decode_cell(cell, file.local.path, generation);
            return;
        }
        let Some(session) = imp
            .message
            .upgrade()
            .and_then(|item| item.message().cloned())
            .map(|message| message.chat_().session_())
        else {
            return;
        };
        let cell = cell.clone();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                match session.download_file(file.id).await {
                    Ok(file) => obj.decode_cell(&cell, file.local.path, generation),
                    Err(e) => {
                        log::debug!("album cell {} failed to download: {}", file.id, e.message)
                    }
                }
            }
        ));
    }

    fn decode_cell(&self, cell: &CellEntry, path: String, generation: u64) {
        if path.is_empty() {
            return;
        }
        let picture = cell.picture.clone();
        let file_id = cell.file.id;
        let target_px = cell.target_px;
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let decoded =
                    match gio::spawn_blocking(move || utils::decode_image_from_path(&path)).await {
                        Ok(Ok(texture)) => texture,
                        Ok(Err(e)) => {
                            log::debug!("album cell decode failed: {e:?}");
                            return;
                        }
                        Err(e) => {
                            log::debug!("album cell decode task failed: {e:?}");
                            return;
                        }
                    };
                let imp = obj.imp();
                if !imp.demand.get() || imp.generation.get() != generation || !obj.is_mapped() {
                    return;
                }
                // The exact picture widget must still be the one showing this
                // file (the grid may have been rebuilt in the meantime).
                let current = imp
                    .cells
                    .borrow()
                    .iter()
                    .any(|entry| entry.picture == picture && entry.file.id == file_id);
                if !current {
                    return;
                }
                if let Some(paintable) =
                    utils::cover_paintable(decoded.upcast_ref::<gdk::Texture>(), target_px as f32)
                {
                    picture.set_paintable(Some(&paintable));
                }
            }
        ));
    }
}

struct BuiltCell {
    widget: gtk::Widget,
    entry: Option<CellEntry>,
}

#[cfg(test)]
mod tests {
    use super::album_placements;

    #[test]
    fn placements_cover_the_grid_without_overlap() {
        for n in 1..=10usize {
            let cells = album_placements(n);
            assert_eq!(
                cells.len(),
                n,
                "one placement per member of a {n}-cell album"
            );

            let mut occupied = vec![vec![false; 2]; 6];
            for (col, row, w, h) in cells {
                for r in row..row + h {
                    for c in col..col + w {
                        assert!(!occupied[r as usize][c as usize], "overlap in {n}");
                        occupied[r as usize][c as usize] = true;
                    }
                }
            }
        }
    }

    #[test]
    fn odd_albums_start_with_the_tall_cell() {
        assert_eq!(album_placements(3)[0], (0, 0, 1, 2));
        assert_eq!(album_placements(5)[0], (0, 0, 1, 2));
        // Even albums are a plain grid.
        assert_eq!(album_placements(2), [(0, 0, 1, 1), (1, 0, 1, 1)]);
        assert_eq!(album_placements(4)[0], (0, 0, 1, 1));
    }

    #[test]
    fn empty_album_has_no_placements() {
        assert!(album_placements(0).is_empty());
    }
}
