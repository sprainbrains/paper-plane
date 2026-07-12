use std::cell::Cell;
use std::cell::RefCell;
use std::sync::OnceLock;

use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::ui::MessageBaseExt;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/audio.ui")]
    pub(crate) struct MessageAudio {
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        pub(super) pending_file_id: Cell<i32>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) title_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) subtitle_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageAudio {
        const NAME: &'static str = "PaplMessageAudio";
        type Type = super::MessageAudio;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageAudio {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Message>("message")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "message" => obj.set_message(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "message" => self.message.upgrade().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            self.play_button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.obj().toggle_playback();
                }
            ));
        }
    }

    impl WidgetImpl for MessageAudio {}
    impl ui::MessageBaseImpl for MessageAudio {}
}

glib::wrapper! {
    pub(crate) struct MessageAudio(ObjectSubclass<imp::MessageAudio>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageAudio {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        if let Some(media) = imp.media.take() {
            media.pause();
        }
        imp.play_button
            .set_icon_name("media-playback-start-symbolic");

        imp.message_bubble.update_from_message(message, true);

        if let tdlib::enums::MessageContent::MessageAudio(data) = message.content().0 {
            let audio = data.audio;

            let title = if audio.title.is_empty() {
                audio.file_name.clone()
            } else {
                audio.title.clone()
            };
            imp.title_label.set_label(&title);

            let subtitle = if audio.performer.is_empty() {
                gettext("Unknown Artist")
            } else {
                audio.performer.clone()
            };
            imp.subtitle_label.set_label(&subtitle);

            if audio.audio.local.is_downloading_completed {
                imp.pending_file_id.set(0);
            } else {
                imp.pending_file_id.set(audio.audio.id);
            }
        }

        imp.message.set(Some(message));
        self.notify("message");
    }
}

impl MessageAudio {
    fn toggle_playback(&self) {
        let imp = self.imp();

        if let Some(media) = imp.media.borrow().clone() {
            if media.is_playing() {
                media.pause();
            } else {
                media.play();
            }
            return;
        }

        let Some(message) = imp.message.upgrade() else {
            return;
        };
        let file_id = imp.pending_file_id.get();
        let session = message.chat_().session_();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let path = if file_id == 0 {
                    match obj.imp().message.upgrade().map(|m| m.content().0) {
                        Some(tdlib::enums::MessageContent::MessageAudio(data))
                            if data.audio.audio.local.is_downloading_completed =>
                        {
                            data.audio.audio.local.path
                        }
                        _ => return,
                    }
                } else {
                    match session.download_file(file_id).await {
                        Ok(file) => file.local.path,
                        Err(e) => {
                            log::warn!("Failed to download an audio file: {e:?}");
                            return;
                        }
                    }
                };

                obj.start_playback(&path);
            }
        ));
    }

    fn start_playback(&self, path: &str) {
        let imp = self.imp();

        let media = gtk::MediaFile::for_filename(path);
        media.play();

        media.connect_playing_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |media| {
                let icon = if media.is_playing() {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                };
                obj.imp().play_button.set_icon_name(icon);
            }
        ));

        imp.pending_file_id.set(0);
        imp.media.replace(Some(media));
    }
}
