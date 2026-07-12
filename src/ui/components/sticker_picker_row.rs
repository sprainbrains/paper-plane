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

const CELL_SIZE: i32 = 68;
const STICKER_SIZE: i32 = 64;
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
            obj.add_tick_callback(|obj, _| {
                if obj.is_scrolling() {
                    obj.clear_animations();
                } else {
                    obj.start_animations();
                    let shimmer = obj.imp().shimmer.get() + 0.025;
                    obj.imp()
                        .shimmer
                        .set(if shimmer > 1.4 { -0.4 } else { shimmer });
                    obj.queue_draw();
                }
                glib::ControlFlow::Continue
            });
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
            let size = if orientation == gtk::Orientation::Horizontal {
                CELL_SIZE * 5
            } else {
                CELL_SIZE
            };
            (size, size, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let width = obj.width() as f32;
            let cell_width = width / 5.0;
            for (index, preview) in self.previews.borrow().iter().enumerate() {
                let aspect = preview.sticker.width as f32 / preview.sticker.height.max(1) as f32;
                let (draw_width, draw_height) = if aspect >= 1.0 {
                    (STICKER_SIZE as f32, STICKER_SIZE as f32 / aspect)
                } else {
                    (STICKER_SIZE as f32 * aspect, STICKER_SIZE as f32)
                };
                let x = index as f32 * cell_width + (cell_width - draw_width) / 2.0;
                let y = (CELL_SIZE as f32 - draw_height) / 2.0;
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
            let cell_width = width / 5;
            for (index, animation) in self.animations.borrow().iter() {
                animation.allocate(
                    STICKER_SIZE,
                    STICKER_SIZE,
                    baseline,
                    Some(gsk::Transform::new().translate(&graphene::Point::new(
                        (*index as i32 * cell_width + (cell_width - STICKER_SIZE) / 2) as f32,
                        ((CELL_SIZE - STICKER_SIZE) / 2) as f32,
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
        let obj: Self = glib::Object::new();
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
        obj.load_previews();
        obj
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
        let index = (x / (self.width() as f64 / 5.0)).floor() as usize;
        if index >= count {
            return;
        }
        let sticker = self.imp().previews.borrow()[index].sticker.clone();
        if let Some(activate) = &*self.imp().activate.borrow() {
            activate(sticker);
        }
    }

    fn load_previews(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let generation = self.imp().generation.get();
        for index in 0..self.imp().previews.borrow().len() {
            let sticker = self.imp().previews.borrow()[index].sticker.clone();
            let client_id = session.client_().id();
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                session,
                async move {
                    let outline_started = std::time::Instant::now();
                    if let Ok(tdlib::enums::Outline::Outline(outline)) =
                        tdlib::functions::get_sticker_outline(
                            sticker.sticker.id,
                            false,
                            false,
                            client_id,
                        )
                        .await
                        && let Some(path) = outline_path(&outline)
                        && obj.imp().generation.get() == generation
                    {
                        obj.imp().previews.borrow_mut()[index].outline = Some(path);
                        obj.queue_draw();
                    }
                    log::debug!(target: "paper_plane::sticker", "outline ready; file_id={} elapsed={:?}", sticker.sticker.id, outline_started.elapsed());

                    while obj.is_scrolling() {
                        glib::timeout_future(Duration::from_millis(50)).await;
                    }
                    if let Ok(texture) = super::load_picker_preview(
                        &sticker,
                        STICKER_SIZE * obj.scale_factor().max(1),
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
        }
    }

    fn start_animations(&self) {
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
            animation.set_longer_side_size(STICKER_SIZE);
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
