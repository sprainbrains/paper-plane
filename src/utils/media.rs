use gtk::gdk;
use gtk::graphene;
use gtk::prelude::*;

/// The on-disk path of a message's primary media file, but only once it is fully
/// downloaded — the file that can be exported by dragging the message out.
pub(crate) fn message_media_local_path(content: &tdlib::enums::MessageContent) -> Option<String> {
    use tdlib::enums::MessageContent::*;
    let file = match content {
        MessagePhoto(data) => data.photo.sizes.last()?.photo.clone(),
        MessageVideo(data) => data.video.video.clone(),
        MessageDocument(data) => data.document.document.clone(),
        MessageAudio(data) => data.audio.audio.clone(),
        MessageVoiceNote(data) => data.voice_note.voice.clone(),
        MessageAnimation(data) => data.animation.animation.clone(),
        MessageVideoNote(data) => data.video_note.video.clone(),
        _ => return None,
    };
    (file.local.is_downloading_completed && !file.local.path.is_empty()).then_some(file.local.path)
}

/// Cover-crops `texture` into a `size`×`size` paintable (centred, aspect-filling),
/// so a fixed-size preview can't inherit the texture's natural dimensions.
pub(crate) fn cover_paintable(texture: &gdk::Texture, size: f32) -> Option<gdk::Paintable> {
    let (tw, th) = (texture.width() as f32, texture.height() as f32);
    if tw <= 0.0 || th <= 0.0 {
        return None;
    }
    let snapshot = gtk::Snapshot::new();
    let scale = (size / tw).max(size / th);
    let (w, h) = (tw * scale, th * scale);
    snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, size, size));
    snapshot.translate(&graphene::Point::new((size - w) / 2.0, (size - h) / 2.0));
    texture
        .upcast_ref::<gdk::Paintable>()
        .snapshot(&snapshot, w as f64, h as f64);
    snapshot.pop();
    snapshot.to_paintable(Some(&graphene::Size::new(size, size)))
}

/// The inline mini preview of a message's media, when its content carries one.
pub(crate) fn content_minithumbnail(
    content: &tdlib::enums::MessageContent,
) -> Option<&tdlib::types::Minithumbnail> {
    use tdlib::enums::MessageContent::*;
    match content {
        MessagePhoto(data) => data.photo.minithumbnail.as_ref(),
        MessageVideo(data) => data.video.minithumbnail.as_ref(),
        MessageAnimation(data) => data.animation.minithumbnail.as_ref(),
        MessageDocument(data) => data.document.minithumbnail.as_ref(),
        MessageVideoNote(data) => data.video_note.minithumbnail.as_ref(),
        _ => None,
    }
}
