use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::graphene;
use gtk::gsk;

use crate::model;
use crate::ui;
use crate::utils;

const DEFAULT_CELL_SIZE: i32 = 85;
const DEFAULT_STICKER_SIZE: i32 = 80;
const DEFAULT_COLUMNS: i32 = 4;
type ActivateSticker = Box<dyn Fn(tdlib::types::Sticker)>;
type RemoveSticker = Box<dyn Fn(tdlib::types::Sticker)>;

/// Radius of the hovered-cell remove badge (recents/favorites rows only).
const REMOVE_BADGE_RADIUS: f32 = 9.0;

/// Per-cell Lottie playback state for an animated (`.tgs`) cell. The row draws the
/// cached frame itself (no child widget); frames come from the shared `rlt` cache.
struct AnimatedCell {
    file: gio::File,
    id: rlt::AnimationId,
    meta: Option<rlt::AnimationMeta>,
    frame: usize,
    texture: Option<gdk::MemoryTexture>,
    /// True while a render for this cell is in flight. The tick won't request another
    /// frame until it lands, so the single render worker can't be flooded faster than
    /// it drains (the cell simply drops frames to catch up).
    pending: bool,
}

struct Preview {
    sticker: tdlib::types::Sticker,
    texture: Option<gdk::Texture>,
    outline: Option<gsk::Path>,
    /// `Some` for animated cells once their `.tgs` is downloaded and metadata loaded.
    anim: Option<AnimatedCell>,
}

mod imp {
    use super::*;

    pub(crate) struct StickerPickerRow {
        pub(super) previews: RefCell<Vec<Preview>>,
        pub(super) scrolling: RefCell<Option<Rc<Cell<bool>>>>,
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        pub(super) activate: RefCell<Option<ActivateSticker>>,
        /// Set for the "Recently Used" / "Favorites" rows: the hovered cell then shows a
        /// remove badge and a click on it invokes `remove` instead of `activate`.
        pub(super) remove: RefCell<Option<RemoveSticker>>,
        pub(super) removable: Cell<bool>,
        /// The cell currently under the pointer, drawn with a hover box (and a remove
        /// badge on removable rows). `None` when the pointer is off the row.
        pub(super) hovered: Cell<Option<usize>>,
        pub(super) generation: Cell<u64>,
        pub(super) shimmer: Cell<f32>,
        pub(super) demand: Rc<Cell<bool>>,
        pub(super) cell_size: Cell<i32>,
        pub(super) sticker_size: Cell<i32>,
        pub(super) columns: Cell<i32>,
        /// Whether to fetch vector outlines for skeleton placeholders. Disabled for
        /// dense custom-emoji rows, where thousands of `get_sticker_outline` calls
        /// would swamp TDLib for no visible benefit at that size.
        pub(super) load_outlines: Cell<bool>,
        /// Whether animated (.tgs) cells play (drawn directly from the shared frame
        /// cache in `snapshot`, no child widgets).
        pub(super) animate: Cell<bool>,
        /// Whether a frame-tick callback is currently registered.
        pub(super) ticking: Cell<bool>,
        /// The row's single child widget: plays the hovered `Webm` cell's video
        /// sticker (tdesktop-style — only the cell under the pointer animates).
        /// Hidden (unmapped, zero work) whenever the hovered cell isn't `Webm`;
        /// positioned over it by [`HoverLayout`].
        pub(super) hover_sticker: RefCell<Option<ui::Sticker>>,
    }

    impl Default for StickerPickerRow {
        fn default() -> Self {
            Self {
                previews: RefCell::default(),
                scrolling: RefCell::default(),
                session: glib::WeakRef::new(),
                activate: RefCell::default(),
                remove: RefCell::default(),
                removable: Cell::new(false),
                hovered: Cell::new(None),
                generation: Cell::default(),
                shimmer: Cell::new(-0.4),
                demand: Rc::new(Cell::new(true)),
                cell_size: Cell::new(DEFAULT_CELL_SIZE),
                sticker_size: Cell::new(DEFAULT_STICKER_SIZE),
                columns: Cell::new(DEFAULT_COLUMNS),
                load_outlines: Cell::new(true),
                animate: Cell::new(true),
                ticking: Cell::new(false),
                hover_sticker: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StickerPickerRow {
        const NAME: &'static str = "PaplStickerPickerRow";
        type Type = super::StickerPickerRow;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            // The row paints its cells itself; the layout manager exists to position
            // the single hover-animation child (see `HoverLayout`). It also takes over
            // the row's size request from the former `WidgetImpl::measure`.
            klass.set_layout_manager_type::<super::HoverLayout>();
        }
    }

    impl ObjectImpl for StickerPickerRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let click = gtk::GestureClick::new();
            click.connect_released(clone!(
                #[weak]
                obj,
                move |_, _, x, y| obj.activate_at(x, y)
            ));
            obj.add_controller(click);

            // Track the hovered cell so the row can paint a hover box under it (and, on
            // recents/favorites rows, a remove badge at its top-right corner).
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(clone!(
                #[weak]
                obj,
                move |_, x, _| obj.set_hovered_at(x)
            ));
            motion.connect_leave(clone!(
                #[weak]
                obj,
                move |_| obj.set_hovered(None)
            ));
            obj.add_controller(motion);

            // The hover-animation child. Hidden until a `Webm` cell is hovered;
            // `HoverLayout` allocates it over that cell. `ui::Sticker` downloads and
            // plays the video itself, gated on map/unmap like every other sticker.
            let hover_sticker: ui::Sticker = glib::Object::new();
            hover_sticker.set_child_visible(false);
            hover_sticker.set_parent(&*obj);
            *self.hover_sticker.borrow_mut() = Some(hover_sticker);

            // Only on-screen rows do work (preview decode, animation, frame ticks): they
            // start on `map` and stop on `unmap`, tdesktop-style. This bounds the cost to
            // the viewport even when the `GtkListView` briefly over-realizes rows on its
            // first layout or a scrolled-past row lingers undisposed.
            obj.connect_map(|row| row.start_row_work());
            obj.connect_unmap(|row| row.stop_row_work());
        }

        fn dispose(&self) {
            // Cancels in-flight load/meta/render tasks (they check demand/generation).
            self.demand.set(false);
            self.generation.set(self.generation.get().wrapping_add(1));
            if let Some(hover_sticker) = self.hover_sticker.take() {
                hover_sticker.unparent();
            }
        }
    }

    impl WidgetImpl for StickerPickerRow {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let cell_size = self.cell_size.get();
            let sticker_size = self.sticker_size.get() as f32;
            let width = obj.width() as f32;
            let cell_width = width / self.columns.get() as f32;
            let hovered = self.hovered.get();

            // A grayish rounded box under the hovered cell so it's clear which sticker /
            // custom-emoji is targeted.
            if let Some(index) = hovered {
                let inset = 3.0;
                let rect = graphene::Rect::new(
                    index as f32 * cell_width + inset,
                    inset,
                    cell_width - inset * 2.0,
                    cell_size as f32 - inset * 2.0,
                );
                let radius = 10.0;
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(rect, radius));
                snapshot.append_color(&gdk::RGBA::new(0.5, 0.5, 0.5, 0.16), &rect);
                snapshot.pop();
            }

            for (index, preview) in self.previews.borrow().iter().enumerate() {
                let aspect = preview.sticker.width as f32 / preview.sticker.height.max(1) as f32;
                let (draw_width, draw_height) = if aspect >= 1.0 {
                    (sticker_size, sticker_size / aspect)
                } else {
                    (sticker_size * aspect, sticker_size)
                };
                let x = index as f32 * cell_width + (cell_width - draw_width) / 2.0;
                let y = (cell_size as f32 - draw_height) / 2.0;
                let rect = graphene::Rect::new(x, y, draw_width, draw_height);
                // Exactly one layer per cell: the current animation frame if we have
                // one, else the static thumbnail, else a skeleton placeholder.
                if let Some(texture) = preview.anim.as_ref().and_then(|anim| anim.texture.as_ref())
                {
                    snapshot.append_texture(texture, &rect);
                } else if let Some(texture) = &preview.texture {
                    snapshot.append_texture(texture, &rect);
                } else {
                    obj.snapshot_skeleton(snapshot, preview, &rect);
                }
            }

            // A custom `snapshot` vfunc does not paint child widgets by itself —
            // draw the hover-animation child explicitly, on top of the static
            // previews it replaces.
            if let Some(hover_sticker) = self.hover_sticker.borrow().as_ref()
                && hover_sticker.is_child_visible()
            {
                obj.snapshot_child(hover_sticker, snapshot);
            }

            // A remove badge at the hovered cell's top-right corner on recents/favorites rows.
            if self.removable.get()
                && let Some(index) = hovered
                && index < self.previews.borrow().len()
            {
                let (cx, cy) = obj.remove_badge_center(index);
                let r = REMOVE_BADGE_RADIUS;
                let bounds = graphene::Rect::new(
                    cx - r - 1.0,
                    cy - r - 1.0,
                    (r + 1.0) * 2.0,
                    (r + 1.0) * 2.0,
                );
                let ctx = snapshot.append_cairo(&bounds);
                ctx.arc(cx as f64, cy as f64, r as f64, 0.0, std::f64::consts::TAU);
                ctx.set_source_rgba(0.0, 0.0, 0.0, 0.55);
                let _ = ctx.fill();
                let arm = (r as f64) * 0.45;
                ctx.set_source_rgba(1.0, 1.0, 1.0, 0.95);
                ctx.set_line_width(1.6);
                ctx.set_line_cap(gtk::cairo::LineCap::Round);
                ctx.move_to(cx as f64 - arm, cy as f64 - arm);
                ctx.line_to(cx as f64 + arm, cy as f64 + arm);
                ctx.move_to(cx as f64 + arm, cy as f64 - arm);
                ctx.line_to(cx as f64 - arm, cy as f64 + arm);
                let _ = ctx.stroke();
            }
        }
    }
}

/// Positions the row's single hover-animation child over the hovered cell. Setting
/// this manager also moves the row's size request here (a widget with a layout
/// manager does not use its own `WidgetImpl::measure`), so `measure` reproduces the
/// former cell-based sizing exactly.
mod hover_layout {
    use gtk::subclass::layout_manager::LayoutManagerImpl;

    use super::*;

    #[derive(Default)]
    pub(crate) struct HoverLayout {}

    #[glib::object_subclass]
    impl ObjectSubclass for HoverLayout {
        const NAME: &'static str = "PaplStickerHoverLayout";
        type Type = super::HoverLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for HoverLayout {}

    impl LayoutManagerImpl for HoverLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            _for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let Some(row) = widget.downcast_ref::<super::StickerPickerRow>() else {
                return (0, 0, -1, -1);
            };
            let cell_size = row.imp().cell_size.get();
            if orientation == gtk::Orientation::Horizontal {
                // Minimum is a single cell so the row (and thus the list, the picker, and
                // the whole sidebar) can shrink freely; the picker re-chunks to fewer
                // columns as it narrows. Natural is the full width for the current column
                // count so it fills the panel when there's room.
                (cell_size, cell_size * row.imp().columns.get(), -1, -1)
            } else {
                (cell_size, cell_size, -1, -1)
            }
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, height: i32, _baseline: i32) {
            let Some(row) = widget.downcast_ref::<super::StickerPickerRow>() else {
                return;
            };
            let Some(hover_sticker) = row.imp().hover_sticker.borrow().clone() else {
                return;
            };
            let rect = if hover_sticker.is_child_visible() {
                row.imp()
                    .hovered
                    .get()
                    .and_then(|index| row.cell_sticker_rect(index, width, height))
            } else {
                None
            };
            match rect {
                Some((x, y, w, h)) => {
                    let transform = gsk::Transform::new().translate(&graphene::Point::new(x, y));
                    hover_sticker.allocate(w.round() as i32, h.round() as i32, -1, Some(transform));
                }
                None => hover_sticker.allocate(0, 0, -1, None),
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct HoverLayout(ObjectSubclass<hover_layout::HoverLayout>)
        @extends gtk::LayoutManager;
}

glib::wrapper! {
    pub(crate) struct StickerPickerRow(ObjectSubclass<imp::StickerPickerRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl StickerPickerRow {
    /// The default sticker cell size / glyph size / column count, exposed so the picker can
    /// derive a column count from the available width and re-chunk to match.
    pub(crate) const CELL_SIZE: i32 = DEFAULT_CELL_SIZE;
    pub(crate) const STICKER_SIZE: i32 = DEFAULT_STICKER_SIZE;
    pub(crate) const COLUMNS: i32 = DEFAULT_COLUMNS;

    /// Builds a row that draws `stickers` at a custom cell size, sticker size, and column
    /// count. `load_outlines` should be `false` for dense custom-emoji rows; `animate`
    /// controls whether `.tgs` cells play.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_sized(
        stickers: &[tdlib::types::Sticker],
        session: &model::ClientStateSession,
        scrolling: Rc<Cell<bool>>,
        cell_size: i32,
        sticker_size: i32,
        columns: i32,
        load_outlines: bool,
        animate: bool,
        activate: impl Fn(tdlib::types::Sticker) + 'static,
    ) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().cell_size.set(cell_size);
        obj.imp().sticker_size.set(sticker_size);
        obj.imp().columns.set(columns);
        obj.imp().load_outlines.set(load_outlines);
        obj.imp().animate.set(animate);
        obj.imp().session.set(Some(session));
        *obj.imp().scrolling.borrow_mut() = Some(scrolling);
        *obj.imp().activate.borrow_mut() = Some(Box::new(activate));
        *obj.imp().previews.borrow_mut() = stickers
            .iter()
            .cloned()
            .map(|sticker| Preview {
                sticker,
                texture: None,
                outline: None,
                anim: None,
            })
            .collect();
        // Work (preview decode, animation download, frame ticks) is started on `map`
        // and stopped on `unmap` — see `start_row_work`/`stop_row_work` — so only rows
        // currently on screen decode and animate, tdesktop-style. This bounds the cost
        // to the viewport even when the `GtkListView` over-realizes or a scrolled-past
        // row lingers undisposed.
        obj
    }

    /// Begins (or resumes) this row's work now that it is on screen: preview decode,
    /// animation loading, and the frame tick. Called from `map`.
    fn start_row_work(&self) {
        self.imp().demand.set(true);
        self.load_previews();
        if self.imp().animate.get() {
            self.load_animations();
        }
        self.prefetch_full_media();
        self.start_ticking();
    }

    /// Stops this row's work when it leaves the screen (scrolled out, recycled, or a
    /// leaked-but-undisposed row): cancels in-flight loads via `demand` and lets the
    /// tick break (it checks `is_mapped`). Called from `unmap`.
    fn stop_row_work(&self) {
        self.imp().demand.set(false);
    }

    /// Registers the per-frame callback that advances each animated cell (pulling its
    /// current frame from the shared cache) and shimmers still-loading skeletons.
    /// No-op if already ticking. Animation keeps running during scroll.
    fn start_ticking(&self) {
        if self.imp().ticking.replace(true) {
            return;
        }
        self.add_tick_callback(|obj, clock| {
            // Off-screen rows do no work: stop ticking until the row is mapped again
            // (start_ticking re-registers on the next `map`).
            if !obj.is_mapped() {
                obj.imp().ticking.set(false);
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let scale = obj.scale_factor().max(1);
            let sticker_size = obj.imp().sticker_size.get();
            let mut animating = false;
            let mut dirty = false;

            {
                let mut previews = obj.imp().previews.borrow_mut();
                for (index, preview) in previews.iter_mut().enumerate() {
                    let (pw, ph) = frame_pixels(&preview.sticker, sticker_size, scale);
                    let Some(anim) = preview.anim.as_mut() else {
                        continue;
                    };
                    let Some(meta) = anim.meta else {
                        continue;
                    };
                    animating = true;
                    // Don't pile up requests faster than the worker drains them.
                    if anim.pending {
                        continue;
                    }
                    let frame = frame_index(&meta, now);
                    if frame == anim.frame && anim.texture.is_some() {
                        continue;
                    }
                    anim.frame = frame;
                    // On a cache hit we get the texture now; on a miss the notify fires
                    // in a later turn (no nested borrow) and repaints the cell.
                    let weak = obj.downgrade();
                    if let Some(texture) =
                        rlt::request_frame(anim.id, &anim.file, frame, pw, ph, move |texture| {
                            if let Some(row) = weak.upgrade() {
                                row.set_cell_texture(index, texture.clone());
                            }
                        })
                    {
                        anim.texture = Some(texture);
                        dirty = true;
                    } else {
                        anim.pending = true;
                    }
                }
            }

            // Shimmer still-loading skeletons (frozen while scrolling).
            let has_pending = obj.has_pending_previews();
            if has_pending && !obj.is_scrolling() {
                let shimmer = obj.imp().shimmer.get() + 0.025;
                obj.imp()
                    .shimmer
                    .set(if shimmer > 1.4 { -0.4 } else { shimmer });
                dirty = true;
            }

            if dirty {
                obj.queue_draw();
            }

            if animating || has_pending {
                glib::ControlFlow::Continue
            } else {
                obj.imp().ticking.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    fn is_scrolling(&self) -> bool {
        self.imp()
            .scrolling
            .borrow()
            .as_ref()
            .is_some_and(|scrolling| scrolling.get())
    }

    /// Marks the row as one whose cells can be removed from recents/favorites: hovering a
    /// cell then shows a remove badge, and clicking it calls `remove` instead of `activate`.
    pub(crate) fn set_removable(&self, remove: impl Fn(tdlib::types::Sticker) + 'static) {
        self.imp().removable.set(true);
        *self.imp().remove.borrow_mut() = Some(Box::new(remove));
    }

    /// The cell index under an x offset, or `None` past the last populated cell.
    fn cell_at(&self, x: f64) -> Option<usize> {
        let count = self.imp().previews.borrow().len();
        let cell_width = self.width() as f64 / self.imp().columns.get() as f64;
        if cell_width <= 0.0 {
            return None;
        }
        let index = (x / cell_width).floor() as usize;
        (index < count).then_some(index)
    }

    /// The center of a cell's remove badge, in widget coordinates (top-right of the cell).
    fn remove_badge_center(&self, index: usize) -> (f32, f32) {
        let cell_width = self.width() as f32 / self.imp().columns.get() as f32;
        let inset = REMOVE_BADGE_RADIUS + 2.0;
        (index as f32 * cell_width + cell_width - inset, inset)
    }

    fn set_hovered_at(&self, x: f64) {
        self.set_hovered(self.cell_at(x));
    }

    fn set_hovered(&self, index: Option<usize>) {
        if self.imp().hovered.replace(index) != index {
            self.update_hover_sticker();
            self.queue_draw();
        }
    }

    /// Points the single hover-animation child at the hovered cell when that cell is
    /// a `Webm` sticker, and hides it otherwise. Removable (recents/favorites) rows
    /// keep the static preview: the remove badge is painted below child widgets and
    /// the animation would cover it.
    fn update_hover_sticker(&self) {
        let imp = self.imp();
        let Some(hover_sticker) = imp.hover_sticker.borrow().clone() else {
            return;
        };
        let animated = !imp.removable.get()
            && imp
                .hovered
                .get()
                .and_then(|index| {
                    imp.previews
                        .borrow()
                        .get(index)
                        .map(|preview| preview.sticker.clone())
                })
                .is_some_and(|sticker| matches!(sticker.format, tdlib::enums::StickerFormat::Webm));
        if !animated {
            hover_sticker.set_child_visible(false);
            return;
        }
        let index = imp.hovered.get().expect("animated hover has an index");
        let sticker = imp.previews.borrow()[index].sticker.clone();
        let Some(session) = imp.session.upgrade() else {
            hover_sticker.set_child_visible(false);
            return;
        };
        hover_sticker.set_longer_side_size(imp.sticker_size.get());
        hover_sticker.update_sticker(sticker, true, session);
        hover_sticker.set_child_visible(true);
        // The child's allocation follows the hovered cell, so re-run the layout.
        self.queue_resize();
    }

    /// The draw rect of cell `index`'s sticker (same math as `snapshot`), used to
    /// allocate the hover-animation child exactly over it.
    fn cell_sticker_rect(
        &self,
        index: usize,
        width: i32,
        height: i32,
    ) -> Option<(f32, f32, f32, f32)> {
        let imp = self.imp();
        let previews = imp.previews.borrow();
        let sticker = &previews.get(index)?.sticker;
        let cell_width = width as f32 / imp.columns.get() as f32;
        let sticker_size = imp.sticker_size.get() as f32;
        let aspect = sticker.width as f32 / sticker.height.max(1) as f32;
        let (draw_width, draw_height) = if aspect >= 1.0 {
            (sticker_size, sticker_size / aspect)
        } else {
            (sticker_size * aspect, sticker_size)
        };
        let x = index as f32 * cell_width + (cell_width - draw_width) / 2.0;
        let y = (height as f32 - draw_height) / 2.0;
        Some((x, y, draw_width, draw_height))
    }

    fn activate_at(&self, x: f64, y: f64) {
        let Some(index) = self.cell_at(x) else {
            return;
        };
        let sticker = self.imp().previews.borrow()[index].sticker.clone();

        // A click within the remove badge (recents/favorites rows) removes rather than sends.
        if self.imp().removable.get() {
            let (cx, cy) = self.remove_badge_center(index);
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy <= REMOVE_BADGE_RADIUS * REMOVE_BADGE_RADIUS {
                if let Some(remove) = &*self.imp().remove.borrow() {
                    remove(sticker);
                }
                return;
            }
        }

        if let Some(activate) = &*self.imp().activate.borrow() {
            activate(sticker);
        }
    }

    fn has_pending_previews(&self) -> bool {
        self.imp()
            .previews
            .borrow()
            .iter()
            .any(|preview| preview.texture.is_none())
    }

    fn load_previews(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let generation = self.imp().generation.get();
        // Shared flags cloned into the tasks. Critically, the tasks hold the row only
        // *weakly* — capturing it strongly (as `clone!(#[weak] self, async …)` does,
        // since it upgrades for the whole future) would keep every row scrolled past
        // alive until its deferred load finished, piling up hundreds of live rows
        // that keep decoding, ticking and animating: the real cause of the scroll
        // freeze. `demand` is cleared on dispose, so a dropped row's task bails.
        let demand = self.imp().demand.clone();
        let scrolling = self.imp().scrolling.borrow().clone();
        let sticker_size = self.imp().sticker_size.get();
        let scale = self.scale_factor().max(1);
        let load_outlines = self.imp().load_outlines.get();
        let client_id = session.client_().id();

        for index in 0..self.imp().previews.borrow().len() {
            // Skip previews that already have a texture (re-demand after scroll).
            if self.imp().previews.borrow()[index].texture.is_some() {
                continue;
            }
            let sticker = self.imp().previews.borrow()[index].sticker.clone();

            // Prioritize the actual preview image — this is what the user waits for.
            let weak_obj = self.downgrade();
            let session_weak = session.downgrade();
            let demand_task = demand.clone();
            let scrolling_task = scrolling.clone();
            utils::spawn(async move {
                // Defer while the list is scrolling, but abandon the load if the row
                // was dropped (its demand flag is cleared on dispose).
                while scrolling_task
                    .as_ref()
                    .is_some_and(|scrolling| scrolling.get())
                {
                    if !demand_task.get() {
                        return;
                    }
                    glib::timeout_future(Duration::from_millis(50)).await;
                }
                if !demand_task.get() {
                    return;
                }
                let Some(session) = session_weak.upgrade() else {
                    return;
                };
                if let Ok(texture) = super::sticker_cache::load_picker_preview(
                    &sticker,
                    sticker_size * scale,
                    &session,
                    demand_task.clone(),
                )
                .await
                    && let Some(obj) = weak_obj.upgrade()
                    && obj.imp().generation.get() == generation
                    && !obj.is_scrolling()
                {
                    obj.imp().previews.borrow_mut()[index].texture = Some(texture);
                    obj.queue_draw();
                }
            });

            // The vector outline is only a nicer skeleton; fetch it lazily and only
            // while the texture is still missing so it never competes for TDLib on the
            // critical path. Skipped entirely for dense rows (custom emoji), where one
            // TDLib call per sticker would dominate the load.
            if !load_outlines {
                continue;
            }
            let weak_obj = self.downgrade();
            let demand_task = demand.clone();
            utils::spawn(async move {
                glib::timeout_future(Duration::from_millis(120)).await;
                if !demand_task.get() {
                    return;
                }
                let file_id = {
                    let Some(obj) = weak_obj.upgrade() else {
                        return;
                    };
                    if obj.imp().generation.get() != generation
                        || obj.imp().previews.borrow()[index].texture.is_some()
                    {
                        return;
                    }
                    obj.imp().previews.borrow()[index].sticker.sticker.id
                };
                if let Ok(tdlib::enums::Outline::Outline(outline)) =
                    tdlib::functions::get_sticker_outline(file_id, false, false, client_id).await
                    && let Some(path) = outline_path(&outline)
                    && demand_task.get()
                    && let Some(obj) = weak_obj.upgrade()
                    && obj.imp().generation.get() == generation
                    && obj.imp().previews.borrow()[index].texture.is_none()
                {
                    obj.imp().previews.borrow_mut()[index].outline = Some(path);
                    obj.queue_draw();
                }
            });
        }
    }

    /// Downloads each `.tgs` cell and loads its metadata, so the tick can draw its
    /// frames from the shared cache. Holds the row weakly (like `load_previews`).
    fn load_animations(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let generation = self.imp().generation.get();
        let demand = self.imp().demand.clone();
        let scrolling = self.imp().scrolling.borrow().clone();
        for index in 0..self.imp().previews.borrow().len() {
            if !matches!(
                self.imp().previews.borrow()[index].sticker.format,
                tdlib::enums::StickerFormat::Tgs
            ) {
                continue;
            }
            // Skip cells whose animation is already loaded (re-demand after a remap),
            // mirroring load_previews — otherwise the loaded cell is torn down and
            // re-downloaded, flashing back to its static frame each time.
            if self.imp().previews.borrow()[index].anim.is_some() {
                continue;
            }
            let file = self.imp().previews.borrow()[index].sticker.sticker.clone();
            let weak_obj = self.downgrade();
            let session_weak = session.downgrade();
            let demand_task = demand.clone();
            let scrolling_task = scrolling.clone();
            utils::spawn(async move {
                // Defer downloading/parsing new animations while scrolling — already
                // loaded rows keep playing; rows scrolled past are dropped and bail.
                while scrolling_task
                    .as_ref()
                    .is_some_and(|scrolling| scrolling.get())
                {
                    if !demand_task.get() {
                        return;
                    }
                    glib::timeout_future(Duration::from_millis(50)).await;
                }
                if !demand_task.get() {
                    return;
                }
                let Some(session) = session_weak.upgrade() else {
                    return;
                };
                let Some(path) = super::sticker_cache::download_sticker_path(file, &session).await
                else {
                    return;
                };
                if !demand_task.get() {
                    return;
                }
                let gfile = gio::File::for_path(path);
                let id = rlt::animation_id(&gfile);
                {
                    let Some(obj) = weak_obj.upgrade() else {
                        return;
                    };
                    if obj.imp().generation.get() != generation {
                        return;
                    }
                    obj.set_cell_file(index, gfile.clone(), id);
                }
                rlt::load_meta(gfile, move |meta| {
                    if let Some(meta) = meta
                        && demand_task.get()
                        && let Some(obj) = weak_obj.upgrade()
                        && obj.imp().generation.get() == generation
                    {
                        obj.set_cell_meta(index, meta);
                        obj.start_ticking();
                    }
                });
            });
        }
    }

    /// Warms the full `.webm` files of this row's cells while it is on screen, so
    /// the first hover animates instantly instead of waiting on the download
    /// (tdesktop preloads the panel's animated stickers the same way). Tasks hold
    /// the row weakly, wait for the scroll to settle, and share the preview
    /// download limiter — so previews keep priority and off-screen rows (unmap
    /// clears `demand`) start nothing. Removable rows never show the hover
    /// overlay, so they skip the warm-up.
    fn prefetch_full_media(&self) {
        if self.imp().removable.get() {
            return;
        }
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let demand = self.imp().demand.clone();
        let scrolling = self.imp().scrolling.borrow().clone();
        for index in 0..self.imp().previews.borrow().len() {
            if !matches!(
                self.imp().previews.borrow()[index].sticker.format,
                tdlib::enums::StickerFormat::Webm
            ) {
                continue;
            }
            let file = self.imp().previews.borrow()[index].sticker.sticker.clone();
            let session_weak = session.downgrade();
            let demand_task = demand.clone();
            let scrolling_task = scrolling.clone();
            utils::spawn(async move {
                while scrolling_task
                    .as_ref()
                    .is_some_and(|scrolling| scrolling.get())
                {
                    if !demand_task.get() {
                        return;
                    }
                    glib::timeout_future(Duration::from_millis(50)).await;
                }
                if !demand_task.get() {
                    return;
                }
                let Some(session) = session_weak.upgrade() else {
                    return;
                };
                // The result only warms TDLib's local cache; the hover overlay
                // finds the file already downloaded.
                let _ = super::sticker_cache::download_sticker_path(file, &session).await;
            });
        }
    }

    fn set_cell_file(&self, index: usize, file: gio::File, id: rlt::AnimationId) {
        if let Some(preview) = self.imp().previews.borrow_mut().get_mut(index) {
            preview.anim = Some(AnimatedCell {
                file,
                id,
                meta: None,
                frame: 0,
                texture: None,
                pending: false,
            });
        }
    }

    fn set_cell_meta(&self, index: usize, meta: rlt::AnimationMeta) {
        if let Some(preview) = self.imp().previews.borrow_mut().get_mut(index)
            && let Some(anim) = preview.anim.as_mut()
        {
            anim.meta = Some(meta);
        }
    }

    fn set_cell_texture(&self, index: usize, texture: gdk::MemoryTexture) {
        if let Some(preview) = self.imp().previews.borrow_mut().get_mut(index)
            && let Some(anim) = preview.anim.as_mut()
        {
            anim.texture = Some(texture);
            anim.pending = false;
        }
        self.queue_draw();
    }

    fn snapshot_skeleton(
        &self,
        snapshot: &gtk::Snapshot,
        preview: &Preview,
        rect: &graphene::Rect,
    ) {
        let base = gdk::RGBA::new(0.45, 0.45, 0.45, 0.18);
        let highlight = gdk::RGBA::new(0.7, 0.7, 0.7, 0.32);
        let phase = if self.is_scrolling() {
            0.5
        } else {
            self.imp().shimmer.get()
        };
        let Some(path) = &preview.outline else {
            // No vector outline (e.g. custom emoji): a subtle rounded shimmer instead
            // of a flat gray square.
            let radius = rect.width().min(rect.height()) * 0.18;
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(*rect, radius));
            snapshot.append_linear_gradient(
                rect,
                &graphene::Point::new((phase - 0.35) * rect.width() + rect.x(), rect.y()),
                &graphene::Point::new((phase + 0.35) * rect.width() + rect.x(), rect.y()),
                &[
                    gsk::ColorStop::new(0.0, base),
                    gsk::ColorStop::new(0.5, highlight),
                    gsk::ColorStop::new(1.0, base),
                ],
            );
            snapshot.pop();
            return;
        };
        snapshot.save();
        snapshot.translate(&graphene::Point::new(rect.x(), rect.y()));
        snapshot.scale(
            rect.width() / preview.sticker.width.max(1) as f32,
            rect.height() / preview.sticker.height.max(1) as f32,
        );
        snapshot.push_fill(path, gsk::FillRule::Winding);
        let width = preview.sticker.width.max(1) as f32;
        let height = preview.sticker.height.max(1) as f32;
        snapshot.append_linear_gradient(
            &graphene::Rect::new(0.0, 0.0, width, height),
            &graphene::Point::new((phase - 0.35) * width, 0.0),
            &graphene::Point::new((phase + 0.35) * width, 0.0),
            &[
                gsk::ColorStop::new(0.0, base),
                gsk::ColorStop::new(0.5, highlight),
                gsk::ColorStop::new(1.0, base),
            ],
        );
        snapshot.pop();
        snapshot.restore();
    }
}

/// Fire-and-forget decode of `stickers` at the sticker-picker preview size, so rows
/// about to scroll into view are already in the shared preview cache and appear
/// instantly. Results are cached by `load_picker_preview`; failures are ignored.
pub(crate) fn prefetch_sticker_previews(
    stickers: &[tdlib::types::Sticker],
    session: &model::ClientStateSession,
    scale: i32,
) {
    let size = DEFAULT_STICKER_SIZE * scale.max(1);
    for sticker in stickers {
        let sticker = sticker.clone();
        let session = session.clone();
        utils::spawn(async move {
            let _ = super::sticker_cache::load_picker_preview(
                &sticker,
                size,
                &session,
                Rc::new(Cell::new(true)),
            )
            .await;
        });
    }
}

/// A single process-wide origin so all animations are phase-consistent for free.
fn animation_origin() -> i64 {
    static ORIGIN: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *ORIGIN.get_or_init(glib::monotonic_time)
}

/// The frame index for `meta` at monotonic time `frame_time_us`, looping.
fn frame_index(meta: &rlt::AnimationMeta, frame_time_us: i64) -> usize {
    let fps = meta.framerate.max(1.0);
    let elapsed = (frame_time_us - animation_origin()).max(0) as f64;
    let frames = (elapsed / 1_000_000.0 * fps) as i64;
    frames.rem_euclid(meta.total_frames.max(1) as i64) as usize
}

/// Device-pixel render size for a cell's animation frame (aspect-fit in the cell).
fn frame_pixels(sticker: &tdlib::types::Sticker, sticker_size: i32, scale: i32) -> (i32, i32) {
    let aspect = sticker.width as f32 / sticker.height.max(1) as f32;
    let (w, h) = if aspect >= 1.0 {
        (sticker_size as f32, sticker_size as f32 / aspect)
    } else {
        (sticker_size as f32 * aspect, sticker_size as f32)
    };
    (
        (w * scale as f32).round().max(1.0) as i32,
        (h * scale as f32).round().max(1.0) as i32,
    )
}

fn outline_path(outline: &tdlib::types::Outline) -> Option<gsk::Path> {
    let builder = gsk::PathBuilder::new();
    for path in &outline.paths {
        let mut end = command_end(path.commands.last()?);
        builder.move_to(end.x as f32, end.y as f32);
        for command in &path.commands {
            match command {
                tdlib::enums::VectorPathCommand::Line(line) => {
                    end = &line.end_point;
                    builder.line_to(end.x as f32, end.y as f32);
                }
                tdlib::enums::VectorPathCommand::CubicBezierCurve(curve) => {
                    end = &curve.end_point;
                    builder.cubic_to(
                        curve.start_control_point.x as f32,
                        curve.start_control_point.y as f32,
                        curve.end_control_point.x as f32,
                        curve.end_control_point.y as f32,
                        end.x as f32,
                        end.y as f32,
                    );
                }
            }
        }
        builder.close();
    }
    Some(builder.to_path())
}

fn command_end(command: &tdlib::enums::VectorPathCommand) -> &tdlib::types::Point {
    match command {
        tdlib::enums::VectorPathCommand::Line(line) => &line.end_point,
        tdlib::enums::VectorPathCommand::CubicBezierCurve(curve) => &curve.end_point,
    }
}
