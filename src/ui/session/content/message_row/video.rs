use std::cell::Cell;
use std::cell::RefCell;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
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

            // A single primary click opens the video or GIF in the fullscreen viewer. Full
            // videos are still only downloaded there, on demand, to avoid eagerly fetching
            // every video in view (see issue #410). The `n_press` guard leaves double-click
            // free for the row's reply gesture, matching the photo widget.
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(clone!(
                #[weak(rename_to = obj)]
                self,
                move |gesture, n_press, _, _| {
                    if n_press != 1 {
                        return;
                    }
                    let Some(message) = obj.message.upgrade() else {
                        return;
                    };
                    if let Some(window) = utils::ancestor::<_, ui::Window>(&*obj.obj()) {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        window.open_media_viewer(&message);
                    }
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

        match content {
            tdlib::enums::MessageContent::MessageAnimation(data) => {
                imp.indicator.set_label("GIF");
                imp.is_animation.set(true);
                imp.play_button.set_visible(false);
                imp.message_bubble
                    .set_label(utils::parse_formatted_text(data.caption));
                imp.picture
                    .set_aspect_ratio(data.animation.width as f64 / data.animation.height as f64);

                if !imp.demand.get() {
                    return;
                }

                // GIFs are small; keep auto-playing them inline (muted, looping).
                let file = data.animation.animation;
                if file.local.is_downloading_completed {
                    self.load_animation(&file.local.path, file.id, generation);
                } else {
                    imp.picture.set_paintable(
                        minithumbnail_texture(data.animation.minithumbnail).as_ref(),
                    );

                    let file_id = file.id;
                    utils::spawn(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        #[weak]
                        session,
                        async move {
                            match session.download_file(file_id).await {
                                Ok(file) => {
                                    obj.load_animation(&file.local.path, file_id, generation)
                                }
                                Err(e) => log::warn!("Failed to download a GIF: {e:?}"),
                            }
                        }
                    ));
                }
            }
            tdlib::enums::MessageContent::MessageVideo(data) => {
                imp.is_animation.set(false);
                imp.play_button.set_visible(true);
                self.update_remaining_time(data.video.duration as i64);
                imp.message_bubble
                    .set_label(utils::parse_formatted_text(data.caption));
                imp.picture
                    .set_aspect_ratio(data.video.width as f64 / data.video.height as f64);

                if !imp.demand.get() {
                    return;
                }

                // Videos are never played inline — a click opens the fullscreen viewer.
                // Show the blurred minithumbnail instantly, then the crisp thumbnail.
                imp.picture
                    .set_paintable(minithumbnail_texture(data.video.minithumbnail).as_ref());
                if let Some(thumbnail) = data.video.thumbnail {
                    self.load_thumbnail(thumbnail.file, generation, session);
                }
            }
            // Content can change in place (e.g. a TTL video becoming
            // `MessageExpiredVideo`); the row is rebound elsewhere, so just skip.
            _ => {}
        }
    }

    fn load_animation(&self, path: &str, file_id: i32, generation: u64) {
        let imp = self.imp();
        if !self.is_current(file_id, generation) {
            return;
        }

        let media = gtk::MediaFile::for_filename(path);
        media.set_muted(true);
        media.set_loop(true);
        media.play();

        imp.picture.set_paintable(Some(&media));
        imp.media.replace(Some(media));
    }

    /// Downloads (if needed) and decodes a video's static thumbnail, replacing the
    /// blurred minithumbnail with a crisp still. Videos no longer play inline, so this
    /// is what the bubble shows.
    fn load_thumbnail(
        &self,
        file: tdlib::types::File,
        generation: u64,
        session: &model::ClientStateSession,
    ) {
        if file.local.is_downloading_completed {
            self.decode_thumbnail(file.local.path, generation);
        } else {
            let file_id = file.id;
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                session,
                async move {
                    match session.download_file(file_id).await {
                        Ok(file) => obj.decode_thumbnail(file.local.path, generation),
                        Err(e) => log::warn!("Failed to download a video thumbnail: {e:?}"),
                    }
                }
            ));
        }
    }

    fn decode_thumbnail(&self, path: String, generation: u64) {
        if !self.is_generation_current(generation) {
            return;
        }
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                match gio::spawn_blocking(move || utils::decode_image_from_path(&path)).await {
                    Ok(Ok(texture)) => {
                        if obj.is_generation_current(generation) {
                            obj.imp().picture.set_paintable(Some(&texture));
                        }
                    }
                    Ok(Err(e)) => log::warn!("Failed to decode a video thumbnail: {e:?}"),
                    Err(e) => log::warn!("Video thumbnail decoder task failed: {e:?}"),
                }
            }
        ));
    }

    /// Whether an in-flight load still applies: the row is mapped, on-demand, and its
    /// content has not been rebound (generation unchanged) since the load started.
    fn is_generation_current(&self, generation: u64) -> bool {
        let imp = self.imp();
        imp.demand.get() && self.is_mapped() && imp.generation.get() == generation
    }

    fn is_current(&self, file_id: i32, generation: u64) -> bool {
        if !self.is_generation_current(generation) {
            return false;
        }
        self.imp()
            .message
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

fn minithumbnail_texture(
    minithumbnail: Option<tdlib::types::Minithumbnail>,
) -> Option<gdk::Texture> {
    minithumbnail.and_then(|m| {
        gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(&m.data))).ok()
    })
}
