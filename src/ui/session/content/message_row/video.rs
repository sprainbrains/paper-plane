use std::cell::Cell;
use std::cell::RefCell;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::base::MessageHandler;
use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/video.ui")]
    #[properties(wrapper_type = super::MessageVideo)]
    pub(crate) struct MessageVideo {
        pub(super) handler: MessageHandler,
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) is_animation: Cell<bool>,
        // File id of a not-yet-downloaded video to be fetched on click; 0 when
        // there is nothing pending (animation, or already downloaded). Full
        // videos are only downloaded on demand to avoid flooding the download
        // queue (see issue #410).
        pub(super) pending_file_id: Cell<i32>,
        pub(super) generation: Cell<u64>,
        pub(super) demand: Cell<bool>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) picture: TemplateChild<ui::MediaPicture>,
        #[template_child]
        pub(super) indicator: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) play_button: TemplateChild<gtk::Image>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageVideo {
        const NAME: &'static str = "PaplMessageVideo";
        type Type = super::MessageVideo;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageVideo {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn constructed(&self) {
            self.parent_constructed();

            // Download and play the full video only when the user clicks it, to
            // avoid eagerly downloading every video in view (see issue #410).
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _, _, _| {
                    obj.obj().play_pending_video();
                }
            ));
            self.picture.add_controller(gesture);
        }
    }

    impl MessageVideo {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessageVideo {
        fn map(&self) {
            self.parent_map();
            self.demand.set(true);
            if let Some(message) = self.message.upgrade() {
                self.obj()
                    .update_content(message.content().0, &message.chat_().session_());
            }
        }

        fn unmap(&self) {
            self.demand.set(false);
            self.generation.set(self.generation.get().wrapping_add(1));
            if let Some(media) = self.media.borrow().as_ref() {
                media.pause();
            }
            self.parent_unmap();
        }
    }
    impl ui::MessageBaseImpl for MessageVideo {}
}

glib::wrapper! {
    pub(crate) struct MessageVideo(ObjectSubclass<imp::MessageVideo>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageVideo {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        let old_message = imp.message.upgrade();
        if old_message.as_ref() == Some(message) {
            return;
        }

        imp.handler.disconnect();

        imp.message_bubble.update_from_message(message, true);

        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                obj.update_content(message.content().0, &message.chat_().session_());
            }
        ));
        imp.handler.set(message, handler_id);

        imp.message.set(Some(message));
        self.update_content(message.content().0, &message.chat_().session_());

        self.notify("message");
    }
}

impl MessageVideo {
    fn update_content(
        &self,
        content: tdlib::enums::MessageContent,
        session: &model::ClientStateSession,
    ) {
        let imp = self.imp();
        imp.generation.set(imp.generation.get().wrapping_add(1));
        let generation = imp.generation.get();
        if let Some(media) = imp.media.take() {
            media.pause();
        }

        let (caption, file, aspect_ratio, minithumbnail) =
            if let tdlib::enums::MessageContent::MessageAnimation(data) = content {
                imp.indicator.set_label("GIF");
                imp.is_animation.set(true);
                (
                    data.caption,
                    data.animation.animation,
                    data.animation.width as f64 / data.animation.height as f64,
                    data.animation.minithumbnail,
                )
            } else if let tdlib::enums::MessageContent::MessageVideo(data) = content {
                self.update_remaining_time(data.video.duration as i64);
                imp.is_animation.set(false);
                (
                    data.caption,
                    data.video.video,
                    data.video.width as f64 / data.video.height as f64,
                    data.video.minithumbnail,
                )
            } else {
                unreachable!();
            };

        let caption = utils::parse_formatted_text(caption);
        imp.message_bubble.set_label(caption);

        imp.picture.set_aspect_ratio(aspect_ratio);

        let is_animation = imp.is_animation.get();

        if file.local.is_downloading_completed {
            imp.pending_file_id.set(0);
            imp.play_button.set_visible(false);
            if imp.demand.get() {
                self.load_video(&file.local.path, file.id, generation);
            }
        } else {
            if imp.demand.get() {
                imp.picture.set_paintable(
                    minithumbnail
                        .and_then(|m| {
                            gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
                                &m.data,
                            )))
                            .ok()
                        })
                        .as_ref(),
                );
            }

            if is_animation {
                // Animations (GIFs) are small; keep auto-downloading and playing.
                imp.pending_file_id.set(0);
                imp.play_button.set_visible(false);
                if imp.demand.get() {
                    let file_id = file.id;
                    utils::spawn(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        #[weak]
                        session,
                        async move {
                            obj.download_and_play(file_id, generation, &session).await;
                        }
                    ));
                }
            } else {
                // Full videos are only fetched when the user clicks the preview.
                imp.pending_file_id.set(file.id);
                imp.play_button.set_visible(true);
            }
        }
    }

    fn play_pending_video(&self) {
        let imp = self.imp();
        if !imp.demand.get() || !self.is_mapped() {
            return;
        }
        let file_id = imp.pending_file_id.get();
        if file_id == 0 {
            return;
        }
        // Prevent repeated clicks from queuing multiple downloads.
        imp.pending_file_id.set(0);
        imp.play_button.set_visible(false);

        let Some(message) = imp.message.upgrade() else {
            return;
        };
        let session = message.chat_().session_();
        let generation = imp.generation.get();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                obj.download_and_play(file_id, generation, &session).await;
            }
        ));
    }

    async fn download_and_play(
        &self,
        file_id: i32,
        generation: u64,
        session: &model::ClientStateSession,
    ) {
        match session.download_file(file_id).await {
            Ok(file) => {
                self.load_video(&file.local.path, file_id, generation);
            }
            Err(e) => {
                log::warn!("Failed to download a video: {e:?}");
            }
        }
    }

    fn load_video(&self, path: &str, file_id: i32, generation: u64) {
        let imp = self.imp();
        if !self.is_current(file_id, generation) {
            return;
        }

        let media = gtk::MediaFile::for_filename(path);
        media.set_muted(true);
        media.set_loop(true);
        media.play();

        if !imp.is_animation.get() {
            media.connect_timestamp_notify(clone!(
                #[weak(rename_to = obj)]
                self,
                move |media| {
                    let time = (media.duration() - media.timestamp()) / i64::pow(10, 6);
                    obj.update_remaining_time(time);
                }
            ));
        }

        imp.picture.set_paintable(Some(&media));
        imp.media.replace(Some(media));
    }

    fn is_current(&self, file_id: i32, generation: u64) -> bool {
        let imp = self.imp();
        if !imp.demand.get() || !self.is_mapped() || imp.generation.get() != generation {
            return false;
        }
        imp.message
            .upgrade()
            .is_some_and(|message| match message.content().0 {
                tdlib::enums::MessageContent::MessageAnimation(data) => {
                    data.animation.animation.id == file_id
                }
                tdlib::enums::MessageContent::MessageVideo(data) => data.video.video.id == file_id,
                _ => false,
            })
    }

    fn update_remaining_time(&self, time: i64) {
        let imp = self.imp();
        let seconds = time % 60;
        let minutes = (time % (60 * 60)) / 60;
        let hours = time / (60 * 60);

        if hours > 0 {
            imp.indicator
                .set_label(&format!("{hours}:{minutes:02}:{seconds:02}"));
        } else {
            imp.indicator.set_label(&format!("{minutes}:{seconds:02}"));
        }
    }
}
