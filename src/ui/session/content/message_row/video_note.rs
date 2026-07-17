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
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/video_note.ui")]
    #[properties(wrapper_type = super::MessageVideoNote)]
    pub(crate) struct MessageVideoNote {
        pub(super) handler: MessageHandler,
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) generation: Cell<u64>,
        pub(super) demand: Cell<bool>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) picture: TemplateChild<ui::MediaPicture>,
        #[template_child]
        pub(super) indicator: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageVideoNote {
        const NAME: &'static str = "PaplMessageVideoNote";
        type Type = super::MessageVideoNote;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageVideoNote {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }

    impl MessageVideoNote {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessageVideoNote {
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
    impl ui::MessageBaseImpl for MessageVideoNote {}
}

glib::wrapper! {
    pub(crate) struct MessageVideoNote(ObjectSubclass<imp::MessageVideoNote>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageVideoNote {
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

impl MessageVideoNote {
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

        let tdlib::enums::MessageContent::MessageVideoNote(data) = content else {
            return;
        };
        let video_note = data.video_note;

        // Video notes are square/round.
        imp.picture.set_aspect_ratio(1.0);
        imp.indicator
            .set_label(&utils::format_duration(video_note.duration));

        if video_note.video.local.is_downloading_completed {
            if imp.demand.get() {
                self.load_video(
                    &video_note.video.local.path,
                    video_note.video.id,
                    generation,
                );
            }
        } else {
            if imp.demand.get() {
                imp.picture.set_paintable(
                    video_note
                        .minithumbnail
                        .and_then(|m| {
                            gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(
                                &m.data,
                            )))
                            .ok()
                        })
                        .as_ref(),
                );
            }

            // Video notes are small; download and auto-play them muted on loop.
            if !imp.demand.get() {
                return;
            }
            let file_id = video_note.video.id;
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                session,
                async move {
                    match session.download_file(file_id).await {
                        Ok(file) => obj.load_video(&file.local.path, file_id, generation),
                        Err(e) => log::warn!("Failed to download a video note: {e:?}"),
                    }
                }
            ));
        }
    }

    fn load_video(&self, path: &str, file_id: i32, generation: u64) {
        let imp = self.imp();
        if !imp.demand.get()
            || !self.is_mapped()
            || imp.generation.get() != generation
            || !imp.message.upgrade().is_some_and(|message| {
                matches!(message.content().0, tdlib::enums::MessageContent::MessageVideoNote(data) if data.video_note.video.id == file_id)
            })
        {
            return;
        }
        let media = gtk::MediaFile::for_filename(path);
        media.set_muted(true);
        media.set_loop(true);
        media.play();
        imp.picture.set_paintable(Some(&media));
        imp.media.replace(Some(media));
    }
}
