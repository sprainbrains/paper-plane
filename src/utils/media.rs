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
