use std::future::Future;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::OnceLock;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use image::ImageReader;
use regex::Regex;
use thiserror::Error;

use crate::APPLICATION_OPTS;
use crate::TEMP_DIR;
use crate::config;

fn protocol_re() -> &'static Regex {
    static PROTOCOL_RE: OnceLock<Regex> = OnceLock::new();
    PROTOCOL_RE.get_or_init(|| Regex::new(r"^\w+://").unwrap())
}

#[derive(Debug)]
pub(crate) struct PaperPlaneSettings(gio::Settings);

impl Default for PaperPlaneSettings {
    fn default() -> Self {
        Self(gio::Settings::new(config::APP_ID))
    }
}

impl Deref for PaperPlaneSettings {
    type Target = gio::Settings;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// Replace variables in the given string with the given dictionary.
///
/// The expected format to replace is `{name}`, where `name` is the first string
/// in the dictionary entry tuple.
// Function taken from Fractal: https://gitlab.gnome.org/GNOME/fractal/-/blob/main/src/utils.rs
pub(crate) fn freplace(s: String, args: &[(&str, &str)]) -> String {
    let mut s = s;
    for (k, v) in args {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

pub(crate) fn linkify(text: &str) -> String {
    if !protocol_re().is_match(text) {
        format!("http://{text}")
    } else {
        text.to_string()
    }
}

pub(crate) fn convert_to_markup(text: String, entity: &tdlib::enums::TextEntityType) -> String {
    use tdlib::enums::TextEntityType::*;

    match entity {
        Url => format!("<a href='{}'>{}</a>", linkify(&text), text),
        EmailAddress => format!("<a href='mailto:{text}'>{text}</a>"),
        PhoneNumber => format!("<a href='tel:{text}'>{text}</a>"),
        Bold => format!("<b>{text}</b>"),
        Italic => format!("<i>{text}</i>"),
        Underline => format!("<u>{text}</u>"),
        Strikethrough => format!("<s>{text}</s>"),
        Code | Pre | PreCode(_) => format!("<tt>{text}</tt>"),
        TextUrl(data) => format!("<a href='{}'>{}</a>", escape(&data.url), text),
        _ => text,
    }
}

pub(crate) fn parse_formatted_text(formatted_text: tdlib::types::FormattedText) -> String {
    let mut entities = formatted_text.entities.iter();
    let mut entity = entities.next();
    let mut output = String::new();
    let mut buffer = String::new();
    let mut is_inside_entity = false;

    // This is the offset in utf16 code units of the text to parse. We need this variable
    // because tdlib stores the offset and length parameters as utf16 code units instead
    // of regular code points.
    let mut code_units_offset = 0;

    for c in formatted_text.text.chars() {
        if !is_inside_entity
            && entity.is_some()
            && code_units_offset >= entity.unwrap().offset as usize
        {
            is_inside_entity = true;

            if !buffer.is_empty() {
                output.push_str(&escape(&buffer));
                buffer = String::new();
            }
        }

        buffer.push(c);
        code_units_offset += c.len_utf16();

        if let Some(entity_) = entity
            && code_units_offset >= (entity_.offset + entity_.length) as usize
        {
            buffer = escape(&buffer);

            entity = loop {
                let entity = entities.next();

                // Handle eventual nested entities
                match entity {
                    Some(entity) => {
                        if entity.offset == entity_.offset {
                            buffer = convert_to_markup(buffer, &entity.r#type);
                        } else {
                            break Some(entity);
                        }
                    }
                    None => break None,
                }
            };

            output.push_str(&convert_to_markup(buffer, &entity_.r#type));
            buffer = String::new();
            is_inside_entity = false;
        }
    }

    // Add the eventual leftovers from the buffer to the output
    if !buffer.is_empty() {
        output.push_str(&escape(&buffer));
    }

    output
}

pub(crate) fn human_friendly_duration(mut seconds: i32) -> String {
    let hours = seconds / (60 * 60);
    if hours > 0 {
        seconds %= 60 * 60;
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            gettext!("{} h {} min {} s", hours, minutes, seconds)
        } else {
            gettext!("{} h {} s", hours, seconds)
        }
    } else {
        let minutes = seconds / 60;
        if minutes > 0 {
            seconds %= 60;
            if seconds > 0 {
                gettext!("{} min {} s", minutes, seconds)
            } else {
                gettext!("{} min", minutes)
            }
        } else {
            gettext!("{} s", seconds)
        }
    }
}

/// Returns the Paper Plane data directory (e.g. /home/bob/.local/share/paper-plane).
pub(crate) fn data_dir() -> &'static PathBuf {
    &APPLICATION_OPTS.get().unwrap().data_dir
}

/// Returns the Paper Plane temp directory (e.g. /tmp/paper-plane2-0).
pub(crate) fn temp_dir() -> Option<&'static PathBuf> {
    TEMP_DIR.get()
}

/// Spawn a future on the default `MainContext`
pub(crate) fn spawn<F: Future<Output = ()> + 'static>(fut: F) {
    let ctx = glib::MainContext::default();
    ctx.spawn_local(fut);
}

#[derive(Error, Debug)]
pub(crate) enum DecodeError {
    #[error("I/O error: {0:?}")]
    IoError(std::io::Error),
    #[error("Image decoding error: {0:?}")]
    ImageError(image::error::ImageError),
    #[error("Decoding for this image format is not currently implemented")]
    Unimplemented,
}

pub(crate) fn decode_image_from_path(path: &str) -> Result<gdk::MemoryTexture, DecodeError> {
    decode_image_from_path_at_size(path, None)
}

pub(crate) fn decode_image_from_path_at_size(
    path: &str,
    maximum_size: Option<u32>,
) -> Result<gdk::MemoryTexture, DecodeError> {
    use image::DynamicImage::*;

    let mut dynamic_image = ImageReader::open(path)
        .map_err(DecodeError::IoError)?
        .decode()
        .map_err(DecodeError::ImageError)?;
    if let Some(maximum_size) = maximum_size
        && (dynamic_image.width() > maximum_size || dynamic_image.height() > maximum_size)
    {
        dynamic_image = dynamic_image.thumbnail(maximum_size, maximum_size);
    }
    let (memory_format, layout, data) = match dynamic_image {
        ImageLuma8(_) => {
            let buffer = dynamic_image.to_rgb8();
            (
                gdk::MemoryFormat::R8g8b8,
                buffer.sample_layout(),
                buffer.into_raw(),
            )
        }
        ImageLumaA8(_) => {
            let buffer = dynamic_image.to_rgba8();
            (
                gdk::MemoryFormat::R8g8b8a8,
                buffer.sample_layout(),
                buffer.into_raw(),
            )
        }
        ImageRgb8(buffer) => (
            gdk::MemoryFormat::R8g8b8,
            buffer.sample_layout(),
            buffer.into_raw(),
        ),
        ImageRgba8(buffer) => (
            gdk::MemoryFormat::R8g8b8a8,
            buffer.sample_layout(),
            buffer.into_raw(),
        ),
        _ => return Err(DecodeError::Unimplemented),
    };

    let bytes = glib::Bytes::from_owned(data);
    let texture = gdk::MemoryTexture::new(
        layout.width as i32,
        layout.height as i32,
        memory_format,
        &bytes,
        layout.height_stride,
    );

    Ok(texture)
}

pub(crate) fn show_toast<W: IsA<gtk::Widget>>(widget: &W, title: impl Into<glib::GString>) {
    let title = title.into();
    let Some(overlay) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    else {
        log::warn!("Could not show toast without a ToastOverlay ancestor: {title}");
        return;
    };

    overlay.add_toast(
        adw::Toast::builder()
            .title(title)
            .timeout(3)
            .priority(adw::ToastPriority::High)
            .build(),
    );
}

pub(crate) fn ancestor<W: IsA<gtk::Widget>, T: IsA<gtk::Widget>>(widget: &W) -> Option<T> {
    widget.ancestor(T::static_type()).and_downcast::<T>()
}

#[allow(dead_code)]
pub(crate) struct ChildIter(Option<gtk::Widget>);
impl From<&gtk::Widget> for ChildIter {
    fn from(widget: &gtk::Widget) -> Self {
        Self(widget.first_child())
    }
}
impl Iterator for ChildIter {
    type Item = gtk::Widget;

    fn next(&mut self) -> Option<Self::Item> {
        let r = self.0.take();
        self.0 = r.as_ref().and_then(|widget| widget.next_sibling());
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(
        offset: i32,
        length: i32,
        r#type: tdlib::enums::TextEntityType,
    ) -> tdlib::types::TextEntity {
        tdlib::types::TextEntity {
            offset,
            length,
            r#type,
        }
    }

    #[test]
    fn formatted_text_uses_utf16_entity_offsets() {
        use tdlib::enums::TextEntityType::Bold;
        use tdlib::enums::TextEntityType::Italic;

        let cases = [
            ("a😀b", vec![entity(1, 2, Bold)], "a<b>😀</b>b"),
            (
                "😀bolditalic",
                vec![entity(2, 4, Bold), entity(6, 6, Italic)],
                "😀<b>bold</b><i>italic</i>",
            ),
            (
                "😀 plain 𝄞bold<&",
                vec![entity(11, 6, Bold)],
                "😀 plain 𝄞<b>bold&lt;&amp;</b>",
            ),
        ];

        for (text, entities, expected) in cases {
            let formatted_text = tdlib::types::FormattedText {
                text: text.to_owned(),
                entities,
            };

            assert_eq!(parse_formatted_text(formatted_text), expected, "{text:?}");
        }
    }

    #[test]
    fn duration_formatting_boundary_cases() {
        let cases = [
            (-1, "-1 s"),
            (0, "0 s"),
            (59, "59 s"),
            (60, "1 min"),
            (61, "1 min 1 s"),
            (3_599, "59 min 59 s"),
            (3_600, "1 h 0 s"),
            (3_601, "1 h 1 s"),
            (3_660, "1 h 1 min 0 s"),
            (3_661, "1 h 1 min 1 s"),
        ];

        for (seconds, expected) in cases {
            assert_eq!(human_friendly_duration(seconds), expected, "{seconds}");
        }
    }
}
