use std::cell::Cell;
use std::cell::RefCell;

use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/audio.ui")]
    #[properties(wrapper_type = super::MessageAudio)]
    pub(crate) struct MessageAudio {
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        pub(super) pending_file_id: Cell<i32>,
        pub(super) generation: Cell<u64>,
        pub(super) demand: Cell<bool>,
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
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageAudio {
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

            self.play_button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.obj().toggle_playback();
                }
            ));
        }
    }

    impl MessageAudio {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessageAudio {
        fn map(&self) {
            self.parent_map();
            self.demand.set(true);
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
        imp.generation.set(imp.generation.get().wrapping_add(1));

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
        if !imp.demand.get() || !self.is_mapped() {
            return;
        }

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
        let generation = imp.generation.get();
        let message_id = message.id();
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

                if obj.is_current(message_id, file_id, generation) {
                    obj.start_playback(&path);
                }
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

    fn is_current(
        &self,
        message_id: crate::types::MessageId,
        file_id: i32,
        generation: u64,
    ) -> bool {
        let imp = self.imp();
        imp.demand.get()
            && self.is_mapped()
            && imp.generation.get() == generation
            && imp.message.upgrade().is_some_and(|message| {
                message.id() == message_id
                    && matches!(message.content().0, tdlib::enums::MessageContent::MessageAudio(data) if file_id == 0 || data.audio.audio.id == file_id)
            })
    }
}
