use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::base::MessageHandler;
use crate::model;
use crate::ui;
use crate::utils;

/// Edge of one album cell in logical pixels; the grid is always two columns
/// of squares, so the bubble keeps a stable, compact width.
const CELL_SIZE: i32 = 140;
/// Gap between cells; mirrors the grid spacing in the template.
const CELL_SPACING: i32 = 2;

/// Grid placements `(col, row, width, height)` for an album of `n` members,
/// in member order. Even counts fill a plain two-column grid; odd counts give
/// the first member a tall cell spanning the top two rows (the layout
/// Telegram apps use).
fn album_placements(n: usize) -> Vec<(i32, i32, i32, i32)> {
    let mut cells = Vec::new();
    if n == 0 {
        return cells;
    }
    if n % 2 == 1 {
        cells.push((0, 0, 1, 2));
    }
    let mut placed = cells.len();
    let mut row = 0;
    while placed < n {
        for col in 0..2 {
            // The tall first cell owns column 0 of the top two rows.
            if n % 2 == 1 && col == 0 && row < 2 {
                continue;
            }
            cells.push((col, row, 1, 1));
            placed += 1;
            if placed == n {
                break;
            }
        }
        row += 1;
    }
    cells
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/album.ui")]
    #[properties(wrapper_type = super::MessageAlbum)]
    pub(crate) struct MessageAlbum {
        pub(super) binding: std::cell::RefCell<Option<gtk::ExpressionWatch>>,
        pub(super) handler: MessageHandler,
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::ChatHistoryItem>,
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

    impl WidgetImpl for MessageAlbum {}

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

        while let Some(child) = imp.album_grid.first_child() {
            imp.album_grid.remove(&child);
        }

        let members = item.album_members().map(<[model::Message]>::to_vec);
        let Some(members) = members else {
            // A single-member "album" collapsed in the model; show the one
            // message full-width rather than as a cell.
            if let Some(primary) = item.message() {
                let cell = self.build_cell(primary, CELL_SIZE, CELL_SIZE);
                imp.album_grid.attach(&cell, 0, 0, 2, 1);
            }
            return;
        };

        for (member, (col, row, w, h)) in members.iter().zip(album_placements(members.len())) {
            let width = if w == 2 {
                CELL_SIZE * 2 + CELL_SPACING
            } else {
                CELL_SIZE
            };
            let height = if h == 2 {
                CELL_SIZE * 2 + CELL_SPACING
            } else {
                CELL_SIZE
            };
            let cell = self.build_cell(member, width, height);
            imp.album_grid.attach(&cell, col, row, w, h);
        }
    }

    /// One square cell: the member's minithumbnail cover-cropped to fill, a
    /// play badge on video cells, and a click that opens the full media.
    fn build_cell(&self, message: &model::Message, width: i32, height: i32) -> gtk::Widget {
        let picture: ui::MediaPicture = glib::Object::new();
        picture.set_aspect_ratio(width as f64 / height as f64);
        picture.set_size_request(width, height);
        picture.set_hexpand(true);
        picture.set_vexpand(true);

        let content = message.content().0;
        let texture = utils::content_minithumbnail(&content).and_then(|mini| {
            gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(&mini.data))).ok()
        });
        if let Some(texture) = texture.as_ref() {
            if let Some(paintable) = utils::cover_paintable(texture, (width * 2) as f32) {
                picture.set_paintable(Some(&paintable));
            } else {
                picture.set_paintable(Some(texture.upcast_ref::<gdk::Paintable>()));
            }
        }

        let is_video = matches!(
            content,
            tdlib::enums::MessageContent::MessageVideo(_)
                | tdlib::enums::MessageContent::MessageAnimation(_)
                | tdlib::enums::MessageContent::MessageVideoNote(_)
        );
        let cell: gtk::Widget = if is_video {
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
            picture.upcast()
        };
        cell.set_cursor_from_name(Some("pointer"));

        let message = message.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(clone!(
            #[weak]
            cell,
            move |gesture, n_press, _, _| {
                if n_press != 1 {
                    return;
                }
                if let Some(window) = utils::ancestor::<_, ui::Window>(&cell) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    window.open_media_viewer(&message);
                }
            }
        ));
        cell.add_controller(gesture);

        cell
    }
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
