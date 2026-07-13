use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use gtk::gio;
use serde::Deserialize;
use serde::Serialize;

use crate::model;
use crate::utils;

pub(super) const STICKER_SET_CACHE_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct StickerSetSignature {
    title: String,
    name: String,
    size: i32,
    thumbnail_file_id: Option<i32>,
    cover_file_ids: Vec<i32>,
}

impl From<&tdlib::types::StickerSetInfo> for StickerSetSignature {
    fn from(info: &tdlib::types::StickerSetInfo) -> Self {
        Self {
            title: info.title.clone(),
            name: info.name.clone(),
            size: info.size,
            thumbnail_file_id: info.thumbnail.as_ref().map(|thumbnail| thumbnail.file.id),
            cover_file_ids: info
                .covers
                .iter()
                .map(|sticker| sticker.sticker.id)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct CachedStickerSet {
    pub(super) signature: StickerSetSignature,
    pub(super) set: tdlib::types::StickerSet,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct StickerSetCache {
    pub(super) version: u32,
    pub(super) sets: HashMap<i64, CachedStickerSet>,
}

pub(super) fn sticker_set_cache_path(session: &model::ClientStateSession) -> PathBuf {
    utils::data_dir()
        .join(session.client_().database_info().0.directory_base_name)
        .join("sticker-sets-v1.json")
}

pub(super) async fn load_sticker_set_cache(path: PathBuf) -> StickerSetCache {
    let started = Instant::now();
    let result = gio::spawn_blocking(move || {
        let data = fs::read(path)?;
        serde_json::from_slice::<StickerSetCache>(&data)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
    .await;
    match result {
        Ok(Ok(cache)) if cache.version == STICKER_SET_CACHE_VERSION => {
            log::info!(
                target: "paper_plane::content_picker",
                "sticker set cache loaded; sets={} elapsed={:?}",
                cache.sets.len(),
                started.elapsed(),
            );
            cache
        }
        Ok(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            log::warn!(
                target: "paper_plane::content_picker",
                "failed to load sticker set cache: {error}"
            );
            StickerSetCache::default()
        }
        _ => StickerSetCache::default(),
    }
}

pub(super) async fn save_sticker_set_cache(path: PathBuf, cache: StickerSetCache) {
    let set_count = cache.sets.len();
    let started = Instant::now();
    let result = gio::spawn_blocking(move || {
        let data = serde_json::to_vec(&cache)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, data)?;
        fs::rename(temporary, path)
    })
    .await;
    match result {
        Ok(Ok(())) => log::info!(
            target: "paper_plane::content_picker",
            "sticker set cache saved; sets={set_count} elapsed={:?}",
            started.elapsed(),
        ),
        Ok(Err(error)) => log::warn!(
            target: "paper_plane::content_picker",
            "failed to save sticker set cache: {error}"
        ),
        Err(_) => log::warn!(
            target: "paper_plane::content_picker",
            "sticker set cache writer failed"
        ),
    }
}
