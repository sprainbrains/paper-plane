//! A GTK4 widget that displays a vector Lottie / `.tgs` animation.
//!
//! Unlike a naive "one renderer per widget" design, this crate uses tdesktop's
//! model so that a dense grid of animated stickers/emoji stays smooth:
//!
//! * **One shared render worker thread** for the whole process (not one OS thread
//!   per widget). It owns the parsed `rlottie` animations (an LRU, so memory is
//!   bounded) and rasterizes frames on request.
//! * **One process-wide frame cache** keyed by `(animation, frame, pixel-size)`,
//!   holding `GdkTexture`s that are *shared* across every widget showing the same
//!   sticker at the same size — so a frame is rasterized and uploaded to the GPU
//!   only once, no matter how many cells display it, and stays cached (byte-bounded
//!   LRU) rather than being dropped 1 s after a cell scrolls off.
//!
//! The public [`Animation`] API is unchanged.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Read;
use std::sync::OnceLock;

use flate2::read::GzDecoder;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// Identifies a parsed animation (a hash of its file path).
type AnimId = u64;

/// Opaque identifier for a parsed animation (a hash of its file path). Cheap to copy.
pub type AnimationId = AnimId;

/// Metadata for an animation, obtained via [`load_meta`].
#[derive(Clone, Copy, Debug)]
pub struct AnimationMeta {
    pub id: AnimationId,
    pub total_frames: usize,
    pub framerate: f64,
    pub default_size: (i32, i32),
}

/// Identifies one rasterized frame: `(animation, frame index, width, height)` in
/// device pixels.
type FrameKey = (AnimId, usize, i32, i32);

/// Byte budget for the shared frame-texture cache (~96 MiB). Bounds memory across
/// all animations and sizes regardless of how many are on screen.
const FRAME_CACHE_BUDGET: usize = 96 * 1024 * 1024;

/// How many parsed `rlottie` animations the worker keeps at once.
const MAX_LOADED_ANIMATIONS: usize = 256;

#[derive(Clone, Copy)]
struct Meta {
    totalframe: usize,
    framerate: f64,
    default_size: (i32, i32),
}

/// A command for the shared render worker.
enum Command {
    Load {
        id: AnimId,
        file: gio::File,
        reply: async_channel::Sender<Option<Meta>>,
    },
    Render {
        id: AnimId,
        file: gio::File,
        frame: usize,
        width: i32,
        height: i32,
        reply: async_channel::Sender<gdk::MemoryTexture>,
    },
}

/// Size of the render worker pool. Each worker owns its own parsed-animation cache
/// and handles a fixed shard of animation ids (`id % RENDER_WORKERS`), so any given
/// animation is parsed once but different stickers rasterize in parallel — otherwise
/// a wall of animated emoji serializes behind one thread on the first loop.
const RENDER_WORKERS: usize = 4;

/// The worker responsible for `id` (each id is handled by exactly one worker, so it
/// is parsed a single time).
fn worker(id: AnimId) -> &'static async_channel::Sender<Command> {
    static WORKERS: OnceLock<Vec<async_channel::Sender<Command>>> = OnceLock::new();
    let workers = WORKERS.get_or_init(|| {
        (0..RENDER_WORKERS)
            .map(|i| {
                let (tx, rx) = async_channel::unbounded::<Command>();
                std::thread::Builder::new()
                    .name(format!("lottie-renderer-{i}"))
                    .spawn(move || run_worker(rx))
                    .expect("failed to spawn lottie render worker");
                tx
            })
            .collect()
    });
    &workers[(id as usize) % RENDER_WORKERS]
}

fn run_worker(rx: async_channel::Receiver<Command>) {
    // LRU of parsed animations, owned solely by this thread (rlottie::Animation is
    // not Send). Re-parsed transparently if evicted.
    let mut animations: HashMap<AnimId, rlottie::Animation> = HashMap::new();
    let mut order: VecDeque<AnimId> = VecDeque::new();

    while let Ok(command) = rx.recv_blocking() {
        match command {
            Command::Load { id, file, reply } => {
                let meta =
                    load_animation(&mut animations, &mut order, id, &file).map(|a| meta_of(a));
                let _ = reply.send_blocking(meta);
            }
            Command::Render {
                id,
                file,
                frame,
                width,
                height,
                reply,
            } => {
                if width <= 0 || height <= 0 {
                    continue;
                }
                if let Some(animation) = load_animation(&mut animations, &mut order, id, &file) {
                    let size = rlottie::Size::new(width as usize, height as usize);
                    let mut surface = rlottie::Surface::new(size);
                    animation.render(frame, &mut surface);
                    let data = glib::Bytes::from_owned(surface);
                    let texture = gdk::MemoryTexture::new(
                        width,
                        height,
                        gdk::MemoryFormat::B8g8r8a8,
                        &data,
                        width as usize * 4,
                    );
                    let _ = reply.send_blocking(texture);
                }
            }
        }
    }
}

/// Returns the parsed animation for `id`, parsing (and LRU-inserting) it if needed.
fn load_animation<'a>(
    animations: &'a mut HashMap<AnimId, rlottie::Animation>,
    order: &mut VecDeque<AnimId>,
    id: AnimId,
    file: &gio::File,
) -> Option<&'a mut rlottie::Animation> {
    if !animations.contains_key(&id) {
        let animation = parse_animation(file)?;
        animations.insert(id, animation);
        order.push_back(id);
        while order.len() > MAX_LOADED_ANIMATIONS {
            if let Some(evicted) = order.pop_front() {
                animations.remove(&evicted);
            }
        }
    } else if let Some(pos) = order.iter().position(|other| *other == id) {
        order.remove(pos);
        order.push_back(id);
    }
    animations.get_mut(&id)
}

fn parse_animation(file: &gio::File) -> Option<rlottie::Animation> {
    let path = file.path()?;
    let cache_key = path.file_name()?.to_str()?.to_owned();

    // Plain JSON lottie files load directly.
    if let Some(animation) = rlottie::Animation::from_file(&path) {
        return Some(animation);
    }

    // `.tgs` files are gzip-compressed lottie JSON.
    let data = file.load_contents(gio::Cancellable::NONE).ok()?.0;
    let mut decoder = GzDecoder::new(&*data);
    let mut json = String::new();
    decoder.read_to_string(&mut json).ok()?;
    rlottie::Animation::from_data(json, cache_key, "")
}

fn meta_of(animation: &rlottie::Animation) -> Meta {
    let size = animation.size();
    Meta {
        totalframe: animation.totalframe(),
        framerate: animation.framerate(),
        default_size: (size.width as i32, size.height as i32),
    }
}

/// The stable id for an animation loaded from `file` (a hash of its path).
pub fn animation_id(file: &gio::File) -> AnimationId {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    match file.path() {
        Some(path) => path.hash(&mut hasher),
        None => file.uri().hash(&mut hasher),
    }
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Shared, main-thread frame cache with per-frame render coalescing.
// ---------------------------------------------------------------------------

type FrameNotify = Box<dyn FnOnce(&gdk::MemoryTexture)>;

#[derive(Default)]
struct FrameCache {
    map: HashMap<FrameKey, gdk::MemoryTexture>,
    order: VecDeque<FrameKey>,
    bytes: usize,
    /// Callers waiting for an in-flight render, keyed by frame. The first waiter
    /// initiates the render; the rest just get notified.
    pending: HashMap<FrameKey, Vec<FrameNotify>>,
}

thread_local! {
    static FRAME_CACHE: RefCell<FrameCache> = RefCell::new(FrameCache::default());
}

static CACHE_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CACHE_MISSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `(hits, misses, entries, bytes)` for the shared frame-texture cache.
pub fn frame_cache_stats() -> (u64, u64, usize, usize) {
    use std::sync::atomic::Ordering;
    FRAME_CACHE.with(|cache| {
        let cache = cache.borrow();
        (
            CACHE_HITS.load(Ordering::Relaxed),
            CACHE_MISSES.load(Ordering::Relaxed),
            cache.map.len(),
            cache.bytes,
        )
    })
}

fn frame_bytes(key: &FrameKey) -> usize {
    (key.2.max(0) as usize) * (key.3.max(0) as usize) * 4
}

impl FrameCache {
    fn get(&mut self, key: &FrameKey) -> Option<gdk::MemoryTexture> {
        let texture = self.map.get(key)?.clone();
        if let Some(pos) = self.order.iter().position(|other| other == key) {
            self.order.remove(pos);
        }
        self.order.push_back(*key);
        Some(texture)
    }

    fn insert(&mut self, key: FrameKey, texture: gdk::MemoryTexture) {
        if self.map.insert(key, texture).is_none() {
            self.bytes += frame_bytes(&key);
            self.order.push_back(key);
        } else if let Some(pos) = self.order.iter().position(|other| *other == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
        while self.bytes > FRAME_CACHE_BUDGET && self.order.len() > 1 {
            if let Some(evicted) = self.order.pop_front() {
                if self.map.remove(&evicted).is_some() {
                    self.bytes = self.bytes.saturating_sub(frame_bytes(&evicted));
                }
            }
        }
    }
}

/// Returns `(id, frame, width, height)`'s texture from the shared cache if present,
/// touching its LRU position. Cheap enough to call from `snapshot`/tick. `width` and
/// `height` are device pixels.
pub fn cached_frame(
    id: AnimationId,
    frame: usize,
    width: i32,
    height: i32,
) -> Option<gdk::MemoryTexture> {
    FRAME_CACHE.with(|cache| cache.borrow_mut().get(&(id, frame, width, height)))
}

/// Requests the texture for `(id, frame, width, height)`. If cached, returns it and
/// does **not** call `notify`. Otherwise schedules a render (coalesced across all
/// callers waiting for the same frame) and invokes `notify(&texture)` on the main
/// thread once it lands. `file` lets the worker re-parse the animation if it was
/// evicted from its LRU.
pub fn request_frame(
    id: AnimationId,
    file: &gio::File,
    frame: usize,
    width: i32,
    height: i32,
    notify: impl FnOnce(&gdk::MemoryTexture) + 'static,
) -> Option<gdk::MemoryTexture> {
    let key = (id, frame, width, height);
    if let Some(texture) = FRAME_CACHE.with(|cache| cache.borrow_mut().get(&key)) {
        CACHE_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Some(texture);
    }

    let initiate = FRAME_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let waiters = cache.pending.entry(key).or_default();
        waiters.push(Box::new(notify));
        waiters.len() == 1
    });
    if initiate {
        CACHE_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    if !initiate {
        return None;
    }

    let (reply_tx, reply_rx) = async_channel::bounded(1);
    let sent = worker(id).send_blocking(Command::Render {
        id,
        file: file.clone(),
        frame,
        width,
        height,
        reply: reply_tx,
    });
    if sent.is_err() {
        FRAME_CACHE.with(|cache| {
            cache.borrow_mut().pending.remove(&key);
        });
        return None;
    }

    glib::spawn_future_local(async move {
        let result = reply_rx.recv().await;
        let (texture, waiters) = FRAME_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let waiters = cache.pending.remove(&key).unwrap_or_default();
            let texture = result.ok();
            if let Some(texture) = &texture {
                cache.insert(key, texture.clone());
            }
            (texture, waiters)
        });
        if let Some(texture) = texture {
            for notify in waiters {
                notify(&texture);
            }
        }
    });

    None
}

/// Asynchronously loads an animation's metadata via the shared worker. `callback`
/// runs on the main thread; `None` means the file could not be parsed.
pub fn load_meta(file: gio::File, callback: impl FnOnce(Option<AnimationMeta>) + 'static) {
    let id = animation_id(&file);
    let (reply_tx, reply_rx) = async_channel::bounded(1);
    if worker(id)
        .send_blocking(Command::Load {
            id,
            file,
            reply: reply_tx,
        })
        .is_err()
    {
        callback(None);
        return;
    }
    glib::spawn_future_local(async move {
        let meta = reply_rx.recv().await.ok().flatten().map(|meta| AnimationMeta {
            id,
            total_frames: meta.totalframe,
            framerate: meta.framerate,
            default_size: meta.default_size,
        });
        callback(meta);
    });
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Animation {
        pub(super) id: Cell<AnimId>,
        pub(super) file: RefCell<Option<gio::File>>,

        /// Monotonic time (µs) that maps to frame 0, set on play.
        pub(super) frame_start: Cell<i64>,
        pub(super) frame_num: Cell<usize>,
        pub(super) totalframe: Cell<usize>,
        pub(super) framerate: Cell<f64>,
        pub(super) default_size: Cell<(i32, i32)>,
        /// Logical draw size (before scale factor), tracked from allocation.
        pub(super) size: Cell<(f64, f64)>,
        /// The frame texture to blit on the next snapshot.
        pub(super) current: RefCell<Option<gdk::MemoryTexture>>,

        pub(super) loop_: Cell<bool>,
        pub(super) playing: Cell<bool>,
        pub(super) reversed: Cell<bool>,
        pub(super) use_cache: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Animation {
        const NAME: &'static str = "LottieAnimation";
        type Type = super::Animation;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Animation {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecBoolean::builder("loop").build(),
                    glib::ParamSpecBoolean::builder("playing").build(),
                    glib::ParamSpecDouble::builder("progress")
                        .minimum(0.0)
                        .maximum(1.0)
                        .build(),
                    glib::ParamSpecBoolean::builder("reversed").build(),
                    glib::ParamSpecBoolean::builder("use-cache").build(),
                ]
            })
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "loop" => self.loop_.get().to_value(),
                "playing" => self.playing.get().to_value(),
                "progress" => {
                    let total = self.totalframe.get().max(1);
                    (self.frame_num.get() as f64 / (total.max(2) - 1) as f64).to_value()
                }
                "reversed" => self.reversed.get().to_value(),
                "use-cache" => self.use_cache.get().to_value(),
                _ => unimplemented!(),
            }
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "loop" => self.loop_.set(value.get().unwrap()),
                "playing" => {
                    self.playing.set(value.get().unwrap());
                    // Anchor frame 0 so playback resumes from the current frame.
                    let fps = self.framerate.get().max(1.0);
                    let offset = (self.frame_num.get() as f64 / fps * 1_000_000.0) as i64;
                    self.frame_start.set(glib::monotonic_time() - offset);
                    self.obj().queue_draw();
                }
                "progress" => {
                    let progress: f64 = value.get().unwrap();
                    let total = self.totalframe.get().max(2);
                    let frame_num = ((total - 1) as f64 * progress) as usize;
                    self.obj().request_frame_num(frame_num);
                }
                "reversed" => self.reversed.set(value.get().unwrap()),
                "use-cache" => self.use_cache.set(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }
    }

    impl WidgetImpl for Animation {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();

            let width = widget.width();
            let height = widget.height();
            if width <= 0 || height <= 0 {
                return;
            }

            let (default_width, default_height) = self.default_size.get();
            if default_width <= 0 || default_height <= 0 {
                return;
            }
            let aspect_ratio = default_width as f64 / default_height as f64;
            let widget_aspect_ratio = width as f64 / height as f64;

            let (draw_width, draw_height) = if aspect_ratio < widget_aspect_ratio {
                (height as f64 * aspect_ratio, height as f64)
            } else {
                (width as f64, width as f64 / aspect_ratio)
            };

            self.size.set((draw_width, draw_height));

            if let Some(texture) = &*self.current.borrow() {
                texture.snapshot(snapshot, draw_width, draw_height);
            }
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (default_width, default_height) = self.default_size.get();
            let aspect_ratio = if default_height > 0 {
                default_width as f64 / default_height as f64
            } else {
                1.0
            };

            if for_size < 0 {
                return match orientation {
                    gtk::Orientation::Horizontal => (0, default_width.max(0), -1, -1),
                    _ => (0, default_height.max(0), -1, -1),
                };
            }

            match orientation {
                gtk::Orientation::Vertical => (0, (for_size as f64 * aspect_ratio) as i32, -1, -1),
                _ => (0, (for_size as f64 / aspect_ratio) as i32, -1, -1),
            }
        }
    }
}

glib::wrapper! {
    /// Widget that displays a vector Lottie animation.
    pub struct Animation(ObjectSubclass<imp::Animation>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Animation {
    fn tick(&self, clock: &gdk::FrameClock) -> glib::ControlFlow {
        let imp = self.imp();
        let totalframe = imp.totalframe.get();
        if totalframe == 0 {
            return glib::ControlFlow::Continue;
        }

        if self.is_mapped() && self.is_playing() {
            let fps = imp.framerate.get().max(1.0);
            let elapsed_us = (clock.frame_time() - imp.frame_start.get()).max(0);
            let elapsed_frames = (elapsed_us as f64 / 1_000_000.0 * fps) as i64;
            let frame = elapsed_frames.rem_euclid(totalframe as i64) as usize;

            if frame != imp.frame_num.get() {
                let target = if imp.reversed.get() {
                    totalframe - frame - 1
                } else {
                    frame
                };
                self.request_frame_num(target);
            }

            if frame == totalframe - 1 && !self.is_loop() {
                self.pause();
            }
        }

        glib::ControlFlow::Continue
    }

    /// Applies `frame_num` as the current frame, pulling its texture from the shared
    /// cache (or rendering it once, shared across all widgets showing it).
    fn request_frame_num(&self, frame_num: usize) {
        let imp = self.imp();
        imp.frame_num.set(frame_num);

        let Some(file) = imp.file.borrow().clone() else {
            return;
        };

        let (logical_width, logical_height) = imp.size.get();
        let scale = self.scale_factor().max(1) as f64;
        let width = (logical_width * scale).round() as i32;
        let height = (logical_height * scale).round() as i32;
        if width <= 0 || height <= 0 {
            return;
        }

        let id = imp.id.get();
        let weak = self.downgrade();
        if let Some(texture) = request_frame(id, &file, frame_num, width, height, move |texture| {
            if let Some(obj) = weak.upgrade() {
                obj.set_current(texture.clone());
            }
        }) {
            self.set_current(texture);
        }
    }

    fn set_current(&self, texture: gdk::MemoryTexture) {
        self.imp().current.replace(Some(texture));
        self.queue_draw();
    }

    /// Loads an animation from a file, asynchronously fetching its metadata.
    pub fn open(&self, file: gio::File) {
        let id = animation_id(&file);
        self.imp().id.set(id);
        self.imp().file.replace(Some(file.clone()));

        load_meta(
            file,
            glib::clone!(
                #[weak(rename_to = obj)]
                self,
                move |meta| {
                    if let Some(meta) = meta {
                        let imp = obj.imp();
                        imp.frame_num.set(0);
                        imp.totalframe.set(meta.total_frames);
                        imp.framerate.set(meta.framerate.max(1.0));
                        imp.default_size.set(meta.default_size);
                        imp.size
                            .set((meta.default_size.0 as f64, meta.default_size.1 as f64));
                        obj.request_frame_num(0);
                        obj.add_tick_callback(Self::tick);
                    }
                }
            ),
        );
    }

    /// Creates an animation from a JSON or `.tgs` file.
    pub fn from_file(file: &impl IsA<gio::File>) -> Self {
        let obj: Self = glib::Object::new();
        obj.open(file.to_owned().upcast());
        obj
    }

    /// Creates an animation from a JSON or `.tgs` file at the given path.
    pub fn from_filename(path: &str) -> Self {
        Self::from_file(&gio::File::for_path(path))
    }

    /// Whether the animation caches rasterized frames (always true now — kept for
    /// API compatibility).
    pub fn use_cache(&self, value: bool) {
        self.set_property("use-cache", value);
    }

    /// See [`Animation::use_cache`].
    pub fn set_use_cache(&self, value: bool) {
        self.set_property("use-cache", value);
    }

    /// Whether frames play in reverse order.
    pub fn is_reversed(&self) -> bool {
        self.property("reversed")
    }

    /// Sets reversed or default frame order.
    pub fn set_reversed(&self, value: bool) {
        self.set_property("reversed", value);
    }

    /// Returns the current progress in `0.0..=1.0`.
    pub fn progress(&self) -> f64 {
        self.property("progress")
    }

    /// Sets the current progress in `0.0..=1.0`.
    pub fn set_progress(&self, value: f64) {
        self.set_property("progress", value);
    }

    /// Returns whether the animation is currently playing.
    pub fn is_playing(&self) -> bool {
        self.imp().playing.get()
    }

    /// Plays the animation.
    pub fn play(&self) {
        self.set_property("playing", true);
    }

    /// Pauses the animation, keeping the last frame on screen.
    pub fn pause(&self) {
        self.set_property("playing", false);
    }

    /// Returns whether the animation loops.
    pub fn is_loop(&self) -> bool {
        self.property("loop")
    }

    /// Sets whether the animation loops.
    pub fn set_loop(&self, loop_: bool) {
        self.set_property("loop", loop_);
    }
}
