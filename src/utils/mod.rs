mod async_runtime;
mod image_decode;
mod markup;
mod media;
mod paths;
// Re-exported wholesale: consumers reference utils::proxy_link::… until
// later tasks use the individual names, which keeps unused re-exports
// out of the crate (clippy -D warnings).
pub(crate) mod proxy_link;
mod settings;
mod widgets;

pub(crate) use async_runtime::spawn;
pub(crate) use image_decode::decode_image_from_path;
pub(crate) use image_decode::decode_image_from_path_at_size;
pub(crate) use markup::RichBlock;
pub(crate) use markup::RichInline;
pub(crate) use markup::freplace;
pub(crate) use markup::human_friendly_duration;
pub(crate) use markup::parse_formatted_text;
pub(crate) use markup::parse_rich_text;
pub(crate) use markup::positional_replace;
pub(crate) use media::message_media_local_path;
pub(crate) use paths::data_dir;
pub(crate) use paths::temp_dir;
pub(crate) use settings::PaperPlaneSettings;
pub(crate) use widgets::ancestor;
pub(crate) use widgets::descendant;
pub(crate) use widgets::for_each_descendant;
pub(crate) use widgets::show_toast;

/// Formats a duration in seconds as `M:SS` (used by voice and video notes).
pub(crate) fn format_duration(seconds: i32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}
