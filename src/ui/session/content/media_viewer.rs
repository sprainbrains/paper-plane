use std::cell::Cell;
use std::cell::RefCell;

use adw::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::graphene;
use gtk::subclass::prelude::*;

use crate::model;
use crate::types::MessageId;
use crate::utils;

// Peak-memory fallback cap for decoding a full-resolution photo. Telegram caps
// uploaded photos well below this, so the sized path is only ever reached for
// pathological originals that exceed the decoder's allocation budget.
const FALLBACK_MAX_DIMENSION: u32 = 7680;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/media_viewer.ui")]
    pub(crate) struct MediaViewer {
        pub(super) message: RefCell<Option<model::Message>>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        pub(super) open_animation: RefCell<Option<adw::SpringAnimation>>,
        // Open-animation progress: 0 at the start of the reveal, settling at 1 (the spring
        // overshoots slightly past 1 for the springy feel). 1 means "not animating", so the
        // snapshot override takes its cheap early-out path.
        pub(super) progress: Cell<f64>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) video: TemplateChild<gtk::Video>,
        #[template_child]
        pub(super) close_button: TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MediaViewer {
        const NAME: &'static str = "PaplMediaViewer";
        type Type = super::MediaViewer;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.set_css_name("mediaviewer");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MediaViewer {
        fn constructed(&self) {
            self.parent_constructed();
            self.progress.set(1.0);
        }

        fn dispose(&self) {
            let mut child = self.obj().first_child();
            while let Some(child_) = child {
                child = child_.next_sibling();
                child_.unparent();
            }
        }
    }

    impl WidgetImpl for MediaViewer {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let progress = self.progress.get();
            if progress >= 1.0 {
                self.parent_snapshot(snapshot);
                return;
            }

            let obj = self.obj();
            let width = obj.width() as f32;
            let height = obj.height() as f32;

            // Subtle "grow from the bottom": scale up from 96% about the bottom-centre
            // anchor while fading in. Driven by a spring, so it overshoots 1.0 a touch and
            // settles — a gentle pop rather than a bold zoom.
            let scale = 0.96 + 0.04 * progress as f32;

            snapshot.push_opacity(progress.clamp(0.0, 1.0));
            snapshot.save();
            snapshot.translate(&graphene::Point::new(width / 2.0, height));
            snapshot.scale(scale, scale);
            snapshot.translate(&graphene::Point::new(-width / 2.0, -height));
            self.parent_snapshot(snapshot);
            snapshot.restore();
            snapshot.pop();
        }
    }

    #[gtk::template_callbacks]
    impl MediaViewer {
        #[template_callback]
        fn on_key_pressed(
            &self,
            key: gdk::Key,
            _: u32,
            _: gdk::ModifierType,
            _: &gtk::EventControllerKey,
        ) -> glib::Propagation {
            if key == gdk::Key::Escape {
                self.obj().close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }

        #[template_callback]
        fn on_close_clicked(&self) {
            self.obj().close();
        }
    }
}

glib::wrapper! {
    pub(crate) struct MediaViewer(ObjectSubclass<imp::MediaViewer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MediaViewer {
    /// Opens the viewer on `message`, replacing whatever was showing. Only photo,
    /// video, and animation messages are supported; anything else closes the viewer.
    pub(crate) fn open(&self, message: &model::Message) {
        self.teardown();

        let imp = self.imp();
        imp.message.replace(Some(message.clone()));
        self.set_visible(true);
        self.grab_focus();

        match message.content().0 {
            tdlib::enums::MessageContent::MessagePhoto(data) => {
                self.show_photo(message, data.photo);
            }
            tdlib::enums::MessageContent::MessageVideo(data) => {
                self.show_video(message, data.video.video, false, false);
            }
            tdlib::enums::MessageContent::MessageAnimation(data) => {
                self.show_video(message, data.animation.animation, true, true);
            }
            _ => {
                self.close();
                return;
            }
        }

        self.animate_open();
    }

    /// Plays the subtle spring reveal (grow-from-bottom + fade). Driven per-frame through
    /// `progress`, which the `snapshot` override reads.
    fn animate_open(&self) {
        let imp = self.imp();
        if let Some(animation) = imp.open_animation.take() {
            animation.pause();
        }
        imp.progress.set(0.0);
        self.queue_draw();

        let target = adw::CallbackAnimationTarget::new(clone!(
            #[weak(rename_to = obj)]
            self,
            move |value| {
                obj.imp().progress.set(value);
                obj.queue_draw();
            }
        ));
        let animation = adw::SpringAnimation::new(
            self,
            0.0,
            1.0,
            adw::SpringParams::new(0.82, 1.0, 260.0),
            target,
        );
        animation.set_epsilon(0.001);
        animation.play();

        imp.open_animation.replace(Some(animation));
    }

    pub(crate) fn close(&self) {
        self.teardown();
        self.set_visible(false);
    }

    /// Stops playback and releases the current stream, texture, and message. Runs on
    /// close and at the start of every `open`, so nothing from a previous item leaks
    /// into the next.
    fn teardown(&self) {
        let imp = self.imp();
        if let Some(animation) = imp.open_animation.take() {
            animation.pause();
        }
        imp.progress.set(1.0);
        if let Some(media) = imp.media.take() {
            media.pause();
        }
        imp.video.set_media_stream(gtk::MediaStream::NONE);
        imp.picture.set_paintable(gdk::Paintable::NONE);
        imp.message.replace(None);
    }

    fn show_photo(&self, message: &model::Message, photo: tdlib::types::Photo) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("photo");

        // Show the blurred minithumbnail immediately for instant feedback.
        if let Some(texture) = photo.minithumbnail.as_ref().and_then(minithumbnail_texture) {
            imp.picture.set_paintable(Some(&texture));
        }

        // The largest size is last (TDLib returns them ascending); fullscreen wants it.
        let Some(size) = photo.sizes.last() else {
            log::warn!("Photo message {} has no available sizes", message.id());
            return;
        };
        let file = size.photo.clone();
        let message_id = message.id();

        if file.local.is_downloading_completed {
            self.decode_and_show(file.local.path, message_id);
        } else {
            let session = message.chat_().session_();
            let file_id = file.id;
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                session,
                async move {
                    match session.download_file(file_id).await {
                        Ok(file) => obj.decode_and_show(file.local.path, message_id),
                        Err(e) => log::warn!("Failed to download a photo for the viewer: {e:?}"),
                    }
                }
            ));
        }
    }

    fn decode_and_show(&self, path: String, message_id: MessageId) {
        if !self.is_current(message_id) {
            return;
        }
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result = gio::spawn_blocking(move || {
                    utils::decode_image_from_path(&path).or_else(|_| {
                        utils::decode_image_from_path_at_size(&path, Some(FALLBACK_MAX_DIMENSION))
                    })
                })
                .await;

                match result {
                    Ok(Ok(texture)) => {
                        if obj.is_current(message_id) {
                            obj.imp().picture.set_paintable(Some(&texture));
                        }
                    }
                    Ok(Err(e)) => log::warn!("Failed to decode a photo for the viewer: {e:?}"),
                    Err(e) => log::warn!("Photo decoder task failed: {e:?}"),
                }
            }
        ));
    }

    fn show_video(
        &self,
        message: &model::Message,
        file: tdlib::types::File,
        muted: bool,
        loop_: bool,
    ) {
        let imp = self.imp();
        imp.stack.set_visible_child_name("video");
        let message_id = message.id();

        if file.local.is_downloading_completed {
            self.play_file(&file.local.path, message_id, muted, loop_);
        } else {
            let session = message.chat_().session_();
            let file_id = file.id;
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                session,
                async move {
                    match session.download_file(file_id).await {
                        Ok(file) => obj.play_file(&file.local.path, message_id, muted, loop_),
                        Err(e) => log::warn!("Failed to download a video for the viewer: {e:?}"),
                    }
                }
            ));
        }
    }

    fn play_file(&self, path: &str, message_id: MessageId, muted: bool, loop_: bool) {
        if !self.is_current(message_id) {
            return;
        }
        let imp = self.imp();

        let media = gtk::MediaFile::for_filename(path);
        media.set_muted(muted);
        imp.video.set_autoplay(true);
        imp.video.set_loop(loop_);
        imp.video.set_media_stream(Some(&media));
        media.play();

        imp.media.replace(Some(media));
    }

    /// Whether the viewer is still showing `message_id` — false once it has been closed
    /// or reopened on a different message while a download/decode was in flight.
    fn is_current(&self, message_id: MessageId) -> bool {
        self.is_visible()
            && self
                .imp()
                .message
                .borrow()
                .as_ref()
                .is_some_and(|message| message.id() == message_id)
    }
}

fn minithumbnail_texture(minithumbnail: &tdlib::types::Minithumbnail) -> Option<gdk::Texture> {
    gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
        &minithumbnail.data,
    )))
    .ok()
}
