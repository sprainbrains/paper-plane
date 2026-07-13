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
use crate::utils;

const DEFAULT_CELL_SIZE: i32 = 68;
const DEFAULT_STICKER_SIZE: i32 = 64;
const DEFAULT_COLUMNS: i32 = 5;
type ActivateSticker = Box<dyn Fn(tdlib::types::Sticker)>;

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
    }

    impl Default for StickerPickerRow {
        fn default() -> Self {
            Self {
                previews: RefCell::default(),
                scrolling: RefCell::default(),
                session: glib::WeakRef::new(),
                activate: RefCell::default(),
                generation: Cell::default(),
                shimmer: Cell::new(-0.4),
                demand: Rc::new(Cell::new(true)),
                cell_size: Cell::new(DEFAULT_CELL_SIZE),
                sticker_size: Cell::new(DEFAULT_STICKER_SIZE),
                columns: Cell::new(DEFAULT_COLUMNS),
                load_outlines: Cell::new(true),
                animate: Cell::new(true),
                ticking: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StickerPickerRow {
        const NAME: &'static str = "PaplStickerPickerRow";
        type Type = super::StickerPickerRow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for StickerPickerRow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let click = gtk::GestureClick::new();
            click.connect_released(clone!(
                #[weak]
                obj,
                move |_, _, x, _| obj.activate_at(x)
            ));
            obj.add_controller(click);

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
        }
    }

    impl WidgetImpl for StickerPickerRow {
        fn measure(&self, orientation: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            let cell_size = self.cell_size.get();
            let size = if orientation == gtk::Orientation::Horizontal {
                cell_size * self.columns.get()
            } else {
                cell_size
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let cell_size = self.cell_size.get();
            let sticker_size = self.sticker_size.get() as f32;
            let width = obj.width() as f32;
            let cell_width = width / self.columns.get() as f32;
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
        }
    }
}

glib::wrapper! {
    pub(crate) struct StickerPickerRow(ObjectSubclass<imp::StickerPickerRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl StickerPickerRow {
    pub(crate) fn new(
        stickers: &[tdlib::types::Sticker],
        session: &model::ClientStateSession,
        scrolling: Rc<Cell<bool>>,
        activate: impl Fn(tdlib::types::Sticker) + 'static,
    ) -> Self {
        Self::new_sized(
            stickers,
            session,
            scrolling,
            DEFAULT_CELL_SIZE,
            DEFAULT_STICKER_SIZE,
            DEFAULT_COLUMNS,
            true,
            true,
            activate,
        )
    }

    /// Like [`Self::new`], but with a custom cell size, sticker size, and column
    /// count — used to render custom emoji at emoji size in the emoji picker.
    /// `load_outlines` should be `false` for dense custom-emoji rows; `animate`
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

    fn activate_at(&self, x: f64) {
        let count = self.imp().previews.borrow().len();
        let index = (x / (self.width() as f64 / self.imp().columns.get() as f64)).floor() as usize;
        if index >= count {
            return;
        }
        let sticker = self.imp().previews.borrow()[index].sticker.clone();
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
                if let Ok(texture) = super::load_picker_preview(
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
                let Some(path) = super::download_sticker_path(file, &session).await else {
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
            let _ = super::load_picker_preview(&sticker, size, &session, Rc::new(Cell::new(true)))
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
