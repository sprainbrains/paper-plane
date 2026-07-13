use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::rc::Rc;
use std::rc::Weak;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::model;
use crate::utils;

const MAX_CONCURRENT_STICKER_DOWNLOADS: usize = 8;
const MAX_CONCURRENT_STICKER_DECODES: usize = 4;
const PREVIEW_CACHE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PreviewKey {
    file_id: i32,
    size: i32,
}

#[derive(Default)]
struct PreviewCache {
    textures: HashMap<PreviewKey, gdk::Texture>,
    order: VecDeque<PreviewKey>,
    waiters: HashMap<PreviewKey, Vec<PreviewWaiter>>,
}

struct PreviewWaiter {
    sender: async_channel::Sender<Result<gdk::Texture, String>>,
    demand: Weak<Cell<bool>>,
}

impl PreviewCache {
    fn get(&mut self, key: PreviewKey) -> Option<gdk::Texture> {
        let texture = self.textures.get(&key)?.clone();
        self.touch(key);
        Some(texture)
    }

    fn touch(&mut self, key: PreviewKey) {
        self.order.retain(|candidate| *candidate != key);
        self.order.push_back(key);
    }

    fn insert(&mut self, key: PreviewKey, texture: gdk::Texture) {
        self.textures.insert(key, texture);
        self.touch(key);
        while self.order.len() > PREVIEW_CACHE_CAPACITY {
            if let Some(evicted) = self.order.pop_front() {
                self.textures.remove(&evicted);
            }
        }
    }
}

thread_local! {
    static PREVIEW_CACHE: RefCell<PreviewCache> = RefCell::default();
}

fn limiter(slots: usize) -> (async_channel::Sender<()>, async_channel::Receiver<()>) {
    let (sender, receiver) = async_channel::bounded(slots);
    for _ in 0..slots {
        sender.try_send(()).unwrap();
    }
    (sender, receiver)
}

fn download_limiter() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    static LIMITER: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
        OnceLock::new();
    LIMITER.get_or_init(|| limiter(MAX_CONCURRENT_STICKER_DOWNLOADS))
}

fn decode_limiter() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    static LIMITER: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
        OnceLock::new();
    LIMITER.get_or_init(|| limiter(MAX_CONCURRENT_STICKER_DECODES))
}

struct Permit(&'static async_channel::Sender<()>);

impl Permit {
    async fn acquire(
        limiter: &'static (async_channel::Sender<()>, async_channel::Receiver<()>),
    ) -> Self {
        limiter.1.recv().await.unwrap();
        Self(&limiter.0)
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.try_send(()).unwrap();
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::Sticker)]
    pub(crate) struct Sticker {
        pub(super) file_id: Cell<i32>,
        pub(super) generation: Cell<u64>,
        pub(super) aspect_ratio: Cell<f64>,
        pub(super) child: RefCell<Option<gtk::Widget>>,

        #[property(get, set = Self::set_longer_side_size)]
        pub(super) longer_side_size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sticker {
        const NAME: &'static str = "PaplSticker";
        type Type = super::Sticker;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Sticker {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            Self::derived_set_property(self, id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            Self::derived_property(self, id, pspec)
        }

        fn dispose(&self) {
            self.generation.set(self.generation.get().wrapping_add(1));
            if let Some(child) = self.child.replace(None) {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Sticker {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if let Some(child) = &*self.child.borrow() {
                let _ = child.measure(orientation, for_size);
            }
            let size = self.longer_side_size.get();
            let aspect_ratio = self.aspect_ratio.get();
            let measured = if orientation == gtk::Orientation::Horizontal {
                if aspect_ratio >= 1.0 {
                    size
                } else {
                    (size as f64 * aspect_ratio) as i32
                }
            } else if aspect_ratio >= 1.0 {
                (size as f64 / aspect_ratio) as i32
            } else {
                size
            }
            .max(1);
            (measured, measured, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = &*self.child.borrow() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn map(&self) {
            self.parent_map();
            self.obj().resume_animation();
        }

        fn unmap(&self) {
            self.parent_unmap();
            self.obj().pause_animation();
        }
    }

    impl Sticker {
        fn set_longer_side_size(&self, size: i32) {
            self.longer_side_size.set(size);
            self.obj().queue_resize();
        }
    }
}

glib::wrapper! {
    pub(crate) struct Sticker(ObjectSubclass<imp::Sticker>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sticker {
    pub(crate) fn update_sticker(
        &self,
        sticker: tdlib::types::Sticker,
        looped: bool,
        session: model::ClientStateSession,
    ) {
        self.begin_update(&sticker);
        let file = sticker.sticker;
        let file_id = file.id;
        let format = sticker.format;
        let generation = self.imp().generation.get();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                if let Some(path) = download_sticker_path(file, &session).await {
                    obj.install_full_media(path, file_id, generation, looped, format)
                        .await;
                }
            }
        ));
    }

    fn begin_update(&self, sticker: &tdlib::types::Sticker) {
        let imp = self.imp();
        imp.file_id.set(sticker.sticker.id);
        imp.generation.set(imp.generation.get().wrapping_add(1));
        imp.aspect_ratio
            .set(sticker.width as f64 / sticker.height.max(1) as f64);
        self.set_child(None);
    }

    async fn install_full_media(
        &self,
        path: String,
        file_id: i32,
        generation: u64,
        looped: bool,
        format: tdlib::enums::StickerFormat,
    ) {
        if !self.is_current(file_id, generation) {
            return;
        }
        let widget: gtk::Widget = match format {
            tdlib::enums::StickerFormat::Tgs => {
                let animation = rlt::Animation::from_filename(&path);
                animation.set_loop(looped);
                animation.use_cache(looped);
                // Only animate while on-screen; map()/unmap() drive play/pause.
                if self.is_mapped() {
                    animation.play();
                }
                animation.upcast()
            }
            tdlib::enums::StickerFormat::Webp => {
                let target_size = self.longer_side_size().max(1) * self.scale_factor().max(1);
                let key = PreviewKey {
                    file_id,
                    size: target_size,
                };
                // Reuse the decoded texture across ListView recycling so scrolling
                // back over a sticker never re-decodes.
                let cached = PREVIEW_CACHE.with(|cache| cache.borrow_mut().get(key));
                let texture = if let Some(texture) = cached {
                    texture
                } else {
                    let permit = Permit::acquire(decode_limiter()).await;
                    let result = gio::spawn_blocking(move || {
                        utils::decode_image_from_path_at_size(&path, Some(target_size as u32))
                    })
                    .await;
                    drop(permit);
                    match result {
                        Ok(Ok(texture)) => {
                            let texture = texture.upcast::<gdk::Texture>();
                            PREVIEW_CACHE
                                .with(|cache| cache.borrow_mut().insert(key, texture.clone()));
                            texture
                        }
                        Ok(Err(error)) => {
                            log::warn!(target: "paper_plane::sticker", "failed to decode sticker {file_id}: {error}");
                            return;
                        }
                        Err(_) => {
                            log::warn!(target: "paper_plane::sticker", "sticker decode task failed for {file_id}");
                            return;
                        }
                    }
                };
                let picture = gtk::Picture::new();
                picture.set_paintable(Some(&texture));
                picture.upcast()
            }
            tdlib::enums::StickerFormat::Webm => {
                let media = gtk::MediaFile::for_file(&gio::File::for_path(path));
                media.set_loop(looped);
                media.set_muted(true);
                if self.is_mapped() {
                    media.play();
                }
                let picture = gtk::Picture::for_paintable(&media);
                picture.set_content_fit(gtk::ContentFit::Contain);
                picture.set_can_shrink(true);
                picture.upcast()
            }
        };
        if self.is_current(file_id, generation) {
            self.set_child(Some(widget));
        }
    }

    fn is_current(&self, file_id: i32, generation: u64) -> bool {
        self.imp().file_id.get() == file_id && self.imp().generation.get() == generation
    }

    pub(crate) fn play_animation(&self) {
        if let Some(animation) = &*self.imp().child.borrow()
            && let Some(animation) = animation.downcast_ref::<rlt::Animation>()
            && !animation.is_playing()
        {
            animation.play();
        }
    }

    /// Resume playback when the sticker scrolls back into view.
    fn resume_animation(&self) {
        let child = self.imp().child.borrow();
        let Some(child) = child.as_ref() else {
            return;
        };
        if let Some(animation) = child.downcast_ref::<rlt::Animation>() {
            if !animation.is_playing() {
                animation.play();
            }
        } else if let Some(picture) = child.downcast_ref::<gtk::Picture>()
            && let Some(media) = picture.paintable().and_downcast::<gtk::MediaFile>()
        {
            media.play();
        }
    }

    /// Pause playback (keeping the last frame on screen) while off-screen so a chat
    /// full of animated stickers doesn't keep every decoder running.
    fn pause_animation(&self) {
        let child = self.imp().child.borrow();
        let Some(child) = child.as_ref() else {
            return;
        };
        if let Some(animation) = child.downcast_ref::<rlt::Animation>() {
            animation.pause();
        } else if let Some(picture) = child.downcast_ref::<gtk::Picture>()
            && let Some(media) = picture.paintable().and_downcast::<gtk::MediaFile>()
        {
            media.pause();
        }
    }

    fn set_child(&self, child: Option<gtk::Widget>) {
        let imp = self.imp();
        if let Some(ref child) = child {
            child.set_parent(self);
        }
        if let Some(old) = imp.child.replace(child) {
            old.unparent();
        }
    }
}

fn select_preview_file(
    thumbnail: Option<&tdlib::types::Thumbnail>,
    full_file: &tdlib::types::File,
    target_size: i32,
    accept_small_thumbnail: bool,
) -> tdlib::types::File {
    thumbnail
        .filter(|thumbnail| {
            thumbnail_is_sufficient(
                thumbnail.width,
                thumbnail.height,
                target_size,
                accept_small_thumbnail,
            )
        })
        .map(|thumbnail| thumbnail.file.clone())
        .unwrap_or_else(|| full_file.clone())
}

fn thumbnail_is_sufficient(
    width: i32,
    height: i32,
    target_size: i32,
    accept_small_thumbnail: bool,
) -> bool {
    accept_small_thumbnail || width.max(height) >= target_size
}

/// Downloads a sticker's file (bounded by the shared download limiter) and returns
/// its local path. Shared by the sticker widget and the picker rows.
pub(crate) async fn download_sticker_path(
    file: tdlib::types::File,
    session: &model::ClientStateSession,
) -> Option<String> {
    if file.local.is_downloading_completed {
        return Some(file.local.path);
    }
    let queue_started = Instant::now();
    let permit = Permit::acquire(download_limiter()).await;
    let queue_elapsed = queue_started.elapsed();
    let download_started = Instant::now();
    let result = session.download_file(file.id).await;
    let download_elapsed = download_started.elapsed();
    drop(permit);
    match result {
        Ok(file) => {
            log::debug!(
                target: "paper_plane::sticker",
                "preview download finished; file_id={} queue={queue_elapsed:?} download={download_elapsed:?}",
                file.id,
            );
            Some(file.local.path)
        }
        Err(error) => {
            log::warn!(target: "paper_plane::sticker", "failed to download sticker preview {}: {error:?}", file.id);
            None
        }
    }
}

async fn load_preview_texture(
    file: tdlib::types::File,
    target_size: i32,
    session: &model::ClientStateSession,
    demand: Rc<Cell<bool>>,
) -> Result<gdk::Texture, String> {
    let key = PreviewKey {
        file_id: file.id,
        size: target_size,
    };
    if let Some(texture) = PREVIEW_CACHE.with(|cache| cache.borrow_mut().get(key)) {
        return Ok(texture);
    }

    let (receiver, initiate) = PREVIEW_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let (sender, receiver) = async_channel::bounded(1);
        let waiter = PreviewWaiter {
            sender,
            demand: Rc::downgrade(&demand),
        };
        if let Some(waiters) = cache.waiters.get_mut(&key) {
            waiters.push(waiter);
            (receiver, false)
        } else {
            cache.waiters.insert(key, vec![waiter]);
            (receiver, true)
        }
    });
    if initiate {
        let session = session.clone();
        utils::spawn(async move {
            let result = load_preview_texture_uncached(key, file, target_size, &session).await;
            PREVIEW_CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                if let Ok(texture) = &result {
                    cache.insert(key, texture.clone());
                }
                for waiter in cache.waiters.remove(&key).unwrap_or_default() {
                    if waiter.demand.upgrade().is_some_and(|demand| demand.get()) {
                        let _ = waiter.sender.try_send(result.clone());
                    }
                }
            });
        });
    }
    receiver.recv().await.map_err(|error| error.to_string())?
}

pub(crate) async fn load_picker_preview(
    sticker: &tdlib::types::Sticker,
    target_size: i32,
    session: &model::ClientStateSession,
    demand: Rc<Cell<bool>>,
) -> Result<gdk::Texture, String> {
    let file = select_preview_file(
        sticker.thumbnail.as_ref(),
        &sticker.sticker,
        target_size,
        !matches!(&sticker.format, tdlib::enums::StickerFormat::Webp),
    );
    load_preview_texture(file, target_size, session, demand).await
}

pub(crate) async fn load_picker_file_preview(
    file: tdlib::types::File,
    target_size: i32,
    session: &model::ClientStateSession,
    demand: Rc<Cell<bool>>,
) -> Result<gdk::Texture, String> {
    load_preview_texture(file, target_size, session, demand).await
}

fn preview_is_demanded(key: PreviewKey) -> bool {
    PREVIEW_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let Some(waiters) = cache.waiters.get_mut(&key) else {
            return false;
        };
        waiters.retain(|waiter| waiter.demand.upgrade().is_some_and(|demand| demand.get()));
        !waiters.is_empty()
    })
}

async fn acquire_demanded_decode(key: PreviewKey) -> Option<Permit> {
    loop {
        if !preview_is_demanded(key) {
            return None;
        }
        match decode_limiter().1.try_recv() {
            Ok(()) => return Some(Permit(&decode_limiter().0)),
            Err(async_channel::TryRecvError::Empty) => {
                glib::timeout_future(Duration::from_millis(5)).await;
            }
            Err(async_channel::TryRecvError::Closed) => return None,
        }
    }
}

async fn load_preview_texture_uncached(
    key: PreviewKey,
    file: tdlib::types::File,
    target_size: i32,
    session: &model::ClientStateSession,
) -> Result<gdk::Texture, String> {
    let total_started = Instant::now();
    async {
        let download_started = Instant::now();
        let path = download_sticker_path(file, session)
            .await
            .ok_or_else(|| "download failed".to_string())?;
        let download_elapsed = download_started.elapsed();
        let queue_started = Instant::now();
        let permit = acquire_demanded_decode(key)
            .await
            .ok_or_else(|| "preview no longer visible".to_string())?;
        let decode_queue_elapsed = queue_started.elapsed();
        let decode_started = Instant::now();
        let size = target_size as u32;
        let decoded =
            gio::spawn_blocking(move || utils::decode_image_from_path_at_size(&path, Some(size)))
                .await
                .map_err(|_| "decode task failed".to_string())?
                .map(|texture| texture.upcast::<gdk::Texture>())
                .map_err(|error| error.to_string());
        let decode_elapsed = decode_started.elapsed();
        drop(permit);
        log_preview_timing(
            key.file_id,
            download_elapsed,
            decode_queue_elapsed,
            decode_elapsed,
            total_started.elapsed(),
        );
        decoded
    }
    .await
}

fn log_preview_timing(
    file_id: i32,
    download: Duration,
    decode_queue: Duration,
    decode: Duration,
    total: Duration,
) {
    log::debug!(
        target: "paper_plane::sticker",
        "preview loaded; file_id={file_id} download={download:?} decode_queue={decode_queue:?} decode={decode:?} total={total:?}",
    );
    if total.as_millis() > 250 {
        log::warn!(
            target: "paper_plane::sticker",
            "slow sticker preview load; file_id={file_id} download={download:?} decode_queue={decode_queue:?} decode={decode:?} total={total:?}",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_cache_evicts_oldest_entry() {
        let mut cache = PreviewCache::default();
        for file_id in 0..=PREVIEW_CACHE_CAPACITY as i32 {
            cache.order.push_back(PreviewKey { file_id, size: 64 });
        }
        while cache.order.len() > PREVIEW_CACHE_CAPACITY {
            cache.order.pop_front();
        }
        assert_eq!(cache.order.front().unwrap().file_id, 1);
    }

    #[test]
    fn preview_cache_returns_inserted_texture() {
        let mut cache = PreviewCache::default();
        let key = PreviewKey {
            file_id: 7,
            size: 64,
        };
        let bytes = glib::Bytes::from_static(&[0u8, 0, 0, 255]);
        let texture: gdk::Texture =
            gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::R8g8b8a8, &bytes, 4).upcast();
        cache.insert(key, texture);
        assert!(cache.get(key).is_some());
        assert_eq!(cache.order.len(), 1);
    }

    #[test]
    fn static_preview_requires_enough_pixels_for_scale() {
        assert!(thumbnail_is_sufficient(128, 96, 128, false));
        assert!(!thumbnail_is_sufficient(100, 100, 128, false));
    }

    #[test]
    fn animated_preview_accepts_small_thumbnail() {
        assert!(thumbnail_is_sufficient(32, 32, 128, true));
    }
}
