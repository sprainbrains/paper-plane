use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::gsk;

use crate::model;
use crate::utils;

const DEFAULT_CELL_SIZE: i32 = 68;
const DEFAULT_STICKER_SIZE: i32 = 64;
const DEFAULT_COLUMNS: i32 = 5;
type ActivateSticker = Box<dyn Fn(tdlib::types::Sticker)>;

#[derive(Clone)]
struct Preview {
    sticker: tdlib::types::Sticker,
    texture: Option<gdk::Texture>,
    outline: Option<gsk::Path>,
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
        pub(super) animations: RefCell<Vec<(usize, crate::ui::Sticker)>>,
        pub(super) animations_started: Cell<bool>,
        pub(super) demand: Rc<Cell<bool>>,
        pub(super) cell_size: Cell<i32>,
        pub(super) sticker_size: Cell<i32>,
        pub(super) columns: Cell<i32>,
        /// Whether to fetch vector outlines for skeleton placeholders. Disabled for
        /// dense custom-emoji rows, where thousands of `get_sticker_outline` calls
        /// would swamp TDLib for no visible benefit at that size.
        pub(super) load_outlines: Cell<bool>,
        /// Lazy rows do nothing until [`super::StickerPickerRow::set_demanded`] marks
        /// them visible — used for the non-virtualized emoji page, so off-screen
        /// custom emoji don't flood the decode queue or shimmer every frame.
        pub(super) lazy: Cell<bool>,
        /// Whether animated (.tgs) stickers play. Disabled for dense custom-emoji
        /// rows: hundreds of on-screen rlottie players render on the main thread and
        /// stutter scrolling, so those show a static first-frame preview instead.
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
                animations: RefCell::default(),
                animations_started: Cell::default(),
                demand: Rc::new(Cell::new(true)),
                cell_size: Cell::new(DEFAULT_CELL_SIZE),
                sticker_size: Cell::new(DEFAULT_STICKER_SIZE),
                columns: Cell::new(DEFAULT_COLUMNS),
                load_outlines: Cell::new(true),
                lazy: Cell::new(false),
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
        }

        fn dispose(&self) {
            self.demand.set(false);
            self.generation.set(self.generation.get().wrapping_add(1));
            for (_, animation) in self.animations.borrow_mut().drain(..) {
                animation.unparent();
            }
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
                if let Some(texture) = &preview.texture {
                    snapshot.append_texture(texture, &rect);
                } else {
                    obj.snapshot_skeleton(snapshot, preview, &rect);
                }
            }
            for (_, animation) in self.animations.borrow().iter() {
                self.obj().snapshot_child(animation, snapshot);
            }
        }

        fn size_allocate(&self, width: i32, _: i32, baseline: i32) {
            let cell_size = self.cell_size.get();
            let sticker_size = self.sticker_size.get();
            let cell_width = width / self.columns.get();
            for (index, animation) in self.animations.borrow().iter() {
                // GTK requires a measure pass before allocating a child.
                animation.measure(gtk::Orientation::Horizontal, -1);
                animation.measure(gtk::Orientation::Vertical, sticker_size);
                animation.allocate(
                    sticker_size,
                    sticker_size,
                    baseline,
                    Some(gsk::Transform::new().translate(&graphene::Point::new(
                        (*index as i32 * cell_width + (cell_width - sticker_size) / 2) as f32,
                        ((cell_size - sticker_size) / 2) as f32,
                    ))),
                );
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
            false,
            true,
            activate,
        )
    }

    /// Like [`Self::new`], but with a custom cell size, sticker size, and column
    /// count — used to render custom emoji at emoji size in the emoji picker.
    /// `load_outlines` should be `false` for dense custom-emoji rows. When `lazy` is
    /// `true` the row loads nothing until [`Self::set_demanded`] marks it visible.
    /// `animate` should be `false` for dense rows to keep scrolling smooth.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_sized(
        stickers: &[tdlib::types::Sticker],
        session: &model::ClientStateSession,
        scrolling: Rc<Cell<bool>>,
        cell_size: i32,
        sticker_size: i32,
        columns: i32,
        load_outlines: bool,
        lazy: bool,
        animate: bool,
        activate: impl Fn(tdlib::types::Sticker) + 'static,
    ) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().cell_size.set(cell_size);
        obj.imp().sticker_size.set(sticker_size);
        obj.imp().columns.set(columns);
        obj.imp().load_outlines.set(load_outlines);
        obj.imp().lazy.set(lazy);
        obj.imp().animate.set(animate);
        // Lazy rows stay dormant (demand off) until scrolled into view.
        obj.imp().demand.set(!lazy);
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
            })
            .collect();
        if !lazy {
            obj.load_previews();
            obj.start_ticking();
        }
        obj
    }

    /// Marks the row visible or not. When it becomes visible, its previews start
    /// loading and it begins shimmering; when hidden, in-flight decodes are dropped
    /// (via the shared demand flag) and ticking stops. Only meaningful for lazy rows.
    pub(crate) fn set_demanded(&self, demanded: bool) {
        if self.imp().demand.get() == demanded {
            return;
        }
        self.imp().demand.set(demanded);
        if demanded {
            self.load_previews();
            self.start_ticking();
        }
    }

    /// Registers the per-frame callback that shimmers skeletons and starts/stops
    /// animated stickers around scrolling. No-op if already ticking.
    fn start_ticking(&self) {
        if self.imp().ticking.replace(true) {
            return;
        }
        self.add_tick_callback(|obj, _| {
            // Stop entirely once the row is scrolled out of view; set_demanded(true)
            // starts a fresh tick when it returns.
            if obj.imp().lazy.get() && !obj.imp().demand.get() {
                obj.clear_animations();
                obj.imp().ticking.set(false);
                return glib::ControlFlow::Break;
            }

            let scrolling = obj.is_scrolling();
            if scrolling {
                obj.clear_animations();
            } else {
                obj.start_animations();
            }

            // Only spend a frame redrawing while there is still a skeleton to
            // shimmer. Once every preview has a texture, static rows do nothing.
            let has_pending = obj.has_pending_previews();
            if has_pending && !scrolling {
                let shimmer = obj.imp().shimmer.get() + 0.025;
                obj.imp()
                    .shimmer
                    .set(if shimmer > 1.4 { -0.4 } else { shimmer });
                obj.queue_draw();
            }

            // Rows with animated (.tgs) stickers keep a cheap tick so animations
            // can be torn down/restarted around scrolling; the rlottie children
            // repaint themselves and need no queue_draw here. Fully-loaded static
            // rows stop ticking entirely.
            if has_pending || obj.has_animated() {
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

    fn has_animated(&self) -> bool {
        self.imp().animate.get()
            && self
                .imp()
                .previews
                .borrow()
                .iter()
                .any(|preview| matches!(preview.sticker.format, tdlib::enums::StickerFormat::Tgs))
    }

    fn load_previews(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let generation = self.imp().generation.get();
        for index in 0..self.imp().previews.borrow().len() {
            // Skip previews that already have a texture (re-demand after scroll).
            if self.imp().previews.borrow()[index].texture.is_some() {
                continue;
            }
            let sticker = self.imp().previews.borrow()[index].sticker.clone();
            let client_id = session.client_().id();

            // Prioritize the actual preview image — this is what the user waits for.
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                session,
                async move {
                    while obj.is_scrolling() {
                        glib::timeout_future(Duration::from_millis(50)).await;
                    }
                    if let Ok(texture) = super::load_picker_preview(
                        &sticker,
                        obj.imp().sticker_size.get() * obj.scale_factor().max(1),
                        &session,
                        obj.imp().demand.clone(),
                    )
                    .await
                        && obj.imp().generation.get() == generation
                        && !obj.is_scrolling()
                    {
                        obj.imp().previews.borrow_mut()[index].texture = Some(texture);
                        obj.queue_draw();
                    }
                }
            ));

            // The vector outline is only a nicer skeleton; fetch it lazily and only
            // while the texture is still missing so it never competes for TDLib on the
            // critical path. If the image already arrived, we skip the round-trip.
            // Skipped entirely for dense rows (custom emoji), where one TDLib call per
            // sticker would dominate the load.
            if !self.imp().load_outlines.get() {
                continue;
            }
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    glib::timeout_future(Duration::from_millis(120)).await;
                    if obj.imp().generation.get() != generation
                        || obj.imp().previews.borrow()[index].texture.is_some()
                    {
                        return;
                    }
                    let outline_started = std::time::Instant::now();
                    let file_id = obj.imp().previews.borrow()[index].sticker.sticker.id;
                    if let Ok(tdlib::enums::Outline::Outline(outline)) =
                        tdlib::functions::get_sticker_outline(file_id, false, false, client_id).await
                        && let Some(path) = outline_path(&outline)
                        && obj.imp().generation.get() == generation
                        && obj.imp().previews.borrow()[index].texture.is_none()
                    {
                        obj.imp().previews.borrow_mut()[index].outline = Some(path);
                        obj.queue_draw();
                    }
                    log::debug!(target: "paper_plane::sticker", "outline ready; file_id={file_id} elapsed={:?}", outline_started.elapsed());
                }
            ));
        }
    }

    fn start_animations(&self) {
        if !self.imp().animate.get() {
            return;
        }
        if self.imp().animations_started.replace(true) {
            return;
        }
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        for (index, preview) in self.imp().previews.borrow().iter().enumerate() {
            if !matches!(&preview.sticker.format, tdlib::enums::StickerFormat::Tgs) {
                continue;
            }
            let animation: super::Sticker = glib::Object::new();
            animation.set_longer_side_size(self.imp().sticker_size.get());
            animation.update_sticker(preview.sticker.clone(), true, session.clone());
            animation.set_parent(self);
            self.imp().animations.borrow_mut().push((index, animation));
        }
        self.queue_allocate();
    }

    fn clear_animations(&self) {
        if !self.imp().animations_started.replace(false) {
            return;
        }
        for (_, animation) in self.imp().animations.borrow_mut().drain(..) {
            animation.unparent();
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
        let Some(path) = &preview.outline else {
            snapshot.append_color(&base, rect);
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
        let phase = if self.is_scrolling() {
            0.5
        } else {
            self.imp().shimmer.get()
        };
        snapshot.append_linear_gradient(
            &graphene::Rect::new(0.0, 0.0, width, height),
            &graphene::Point::new((phase - 0.35) * width, 0.0),
            &graphene::Point::new((phase + 0.35) * width, 0.0),
            &[
                gsk::ColorStop::new(0.0, base),
                gsk::ColorStop::new(0.5, gdk::RGBA::new(0.7, 0.7, 0.7, 0.32)),
                gsk::ColorStop::new(1.0, base),
            ],
        );
        snapshot.pop();
        snapshot.restore();
    }
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
