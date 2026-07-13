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
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::model;
use crate::utils;

const MAX_CONCURRENT_STICKER_DOWNLOADS: usize = 8;
const MAX_CONCURRENT_STICKER_DECODES: usize = 4;
const PREVIEW_CACHE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct PreviewKey {
    pub(super) file_id: i32,
    pub(super) size: i32,
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

pub(super) fn cached_texture(key: PreviewKey) -> Option<gdk::Texture> {
    PREVIEW_CACHE.with(|cache| cache.borrow_mut().get(key))
}

pub(super) fn cache_texture(key: PreviewKey, texture: gdk::Texture) {
    PREVIEW_CACHE.with(|cache| cache.borrow_mut().insert(key, texture));
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

pub(super) struct Permit(&'static async_channel::Sender<()>);

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

pub(super) async fn acquire_decode() -> Permit {
    Permit::acquire(decode_limiter()).await
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

pub(super) async fn download_sticker_path(
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
    if let Some(texture) = cached_texture(key) {
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
