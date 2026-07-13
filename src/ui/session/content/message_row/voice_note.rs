use std::cell::Cell;
use std::cell::RefCell;

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
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/voice_note.ui")]
    #[properties(wrapper_type = super::MessageVoiceNote)]
    pub(crate) struct MessageVoiceNote {
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        // File id of a not-yet-downloaded voice note, fetched on first play; 0
        // once downloaded.
        pub(super) pending_file_id: Cell<i32>,
        pub(super) duration: Cell<i32>,
        pub(super) samples: RefCell<Vec<u8>>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) waveform: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        pub(super) duration_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageVoiceNote {
        const NAME: &'static str = "PaplMessageVoiceNote";
        type Type = super::MessageVoiceNote;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageVoiceNote {
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

            self.waveform.set_draw_func(clone!(
                #[weak(rename_to = obj)]
                self,
                move |area, cr, width, height| {
                    draw_waveform(&obj.samples.borrow(), area, cr, width, height);
                }
            ));
        }
    }

    impl MessageVoiceNote {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessageVoiceNote {}
    impl ui::MessageBaseImpl for MessageVoiceNote {}
}

glib::wrapper! {
    pub(crate) struct MessageVoiceNote(ObjectSubclass<imp::MessageVoiceNote>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageVoiceNote {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        // Reset any previous playback state.
        if let Some(media) = imp.media.take() {
            media.pause();
        }
        imp.play_button
            .set_icon_name("media-playback-start-symbolic");

        imp.message_bubble.update_from_message(message, true);

        if let tdlib::enums::MessageContent::MessageVoiceNote(data) = message.content().0 {
            let voice_note = data.voice_note;
            imp.duration.set(voice_note.duration);
            imp.duration_label
                .set_label(&format_duration(voice_note.duration));

            imp.samples.replace(decode_waveform(&voice_note.waveform));
            imp.waveform.queue_draw();

            if voice_note.voice.local.is_downloading_completed {
                imp.pending_file_id.set(0);
            } else {
                imp.pending_file_id.set(voice_note.voice.id);
            }
        }

        imp.message.set(Some(message));
        self.notify("message");
    }
}

impl MessageVoiceNote {
    fn toggle_playback(&self) {
        let imp = self.imp();

        // Already have a player: just toggle play/pause.
        if let Some(media) = imp.media.borrow().clone() {
            if media.is_playing() {
                media.pause();
            } else {
                media.play();
            }
            return;
        }

        // First play: download (if needed) then start.
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
                    // Already downloaded; re-read the current content for the path.
                    match obj.imp().message.upgrade().map(|m| m.content().0) {
                        Some(tdlib::enums::MessageContent::MessageVoiceNote(data))
                            if data.voice_note.voice.local.is_downloading_completed =>
                        {
                            data.voice_note.voice.local.path
                        }
                        _ => return,
                    }
                } else {
                    match session.download_file(file_id).await {
                        Ok(file) => file.local.path,
                        Err(e) => {
                            log::warn!("Failed to download a voice note: {e:?}");
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

        // Reflect play/pause in the button icon.
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

        // Show elapsed time while playing; reset to the full duration at the end.
        media.connect_timestamp_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |media| {
                let imp = obj.imp();
                if media.is_ended() {
                    imp.duration_label
                        .set_label(&format_duration(imp.duration.get()));
                } else {
                    let seconds = (media.timestamp() / 1_000_000) as i32;
                    imp.duration_label.set_label(&format_duration(seconds));
                }
            }
        ));

        imp.pending_file_id.set(0);
        imp.media.replace(Some(media));
    }
}

fn format_duration(seconds: i32) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}")
}

/// Decodes TDLib's base64 waveform (5 bits per sample) into 0..=31 amplitudes.
fn decode_waveform(waveform: &str) -> Vec<u8> {
    let data = glib::base64_decode(waveform);
    let sample_count = data.len() * 8 / 5;

    (0..sample_count)
        .map(|i| {
            let bit = i * 5;
            let byte = bit / 8;
            let shift = bit % 8;
            let mut value = u16::from(data[byte]) >> shift;
            if shift > 3 && byte + 1 < data.len() {
                value |= u16::from(data[byte + 1]) << (8 - shift);
            }
            (value & 0x1f) as u8
        })
        .collect()
}

fn draw_waveform(
    samples: &[u8],
    area: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    width: i32,
    height: i32,
) {
    if samples.is_empty() {
        return;
    }

    let color = area.color();
    cr.set_source_rgba(
        color.red() as f64,
        color.green() as f64,
        color.blue() as f64,
        0.6,
    );

    let n = samples.len() as f64;
    let bar_gap = 1.0;
    let bar_width = (width as f64 / n - bar_gap).max(1.0);
    let h = height as f64;

    for (i, &sample) in samples.iter().enumerate() {
        let bar_height = ((sample as f64 / 31.0) * h).max(2.0);
        let x = i as f64 * (bar_width + bar_gap);
        cr.rectangle(x, (h - bar_height) / 2.0, bar_width, bar_height);
    }

    let _ = cr.fill();
}
