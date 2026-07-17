use std::cell::RefCell;
use std::sync::OnceLock;

use glib::clone;
use glib::subclass::Signal;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;

mod imp {
    use super::*;

    /// A custom emoji inserted into the buffer. The two marks bracket the emoji's plain
    /// text (start with right gravity, end with left gravity) so the range tracks the
    /// glyph as surrounding text is edited; the entity offset/length is re-derived from
    /// the marks on every buffer change instead of being lost.
    #[derive(Debug)]
    pub(crate) struct CustomEmojiAnchor {
        pub(super) start: gtk::TextMark,
        pub(super) end: gtk::TextMark,
        pub(super) custom_emoji_id: i64,
    }

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/components/message_entry.ui")]
    pub(crate) struct MessageEntry {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) formatted_text: RefCell<Option<model::BoxedFormattedText>>,
        pub(super) custom_emojis: RefCell<Vec<CustomEmojiAnchor>>,
        #[template_child]
        pub(super) overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) placeholder: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) emoji_button: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) text_view: TemplateChild<gtk::TextView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageEntry {
        const NAME: &'static str = "PaplMessageEntry";
        type Type = super::MessageEntry;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageEntry {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("activate").build(),
                    Signal::builder("paste-clipboard").build(),
                    Signal::builder("emoji-button-press")
                        .param_types([gtk::Image::static_type()])
                        .build(),
                    Signal::builder("emoji-button-hover")
                        .param_types([gtk::Image::static_type()])
                        .build(),
                    Signal::builder("emoji-button-leave").build(),
                ]
            })
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecBoxed::builder::<model::BoxedFormattedText>("formatted-text")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecString::builder("placeholder-text")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "formatted-text" => obj.set_formatted_text(value.get().unwrap()),
                "placeholder-text" => obj.set_placeholder_text(value.get().unwrap()),
                "chat" => obj.set_chat(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "formatted-text" => obj.formatted_text().to_value(),
                "placeholder-text" => obj.placeholder_text().to_value(),
                "chat" => obj.chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            self.placeholder.connect_text_notify(clone!(
                #[weak]
                obj,
                move |_| obj.notify("placeholder-text")
            ));

            // Handle the enter key to emit the "activate" signal if neither the "ctrl" nor the
            // "shift" modifier are pressed at the same time.
            let key_events = gtk::EventControllerKey::new();
            key_events.connect_key_pressed(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, modifier| {
                    if !modifier.contains(gdk::ModifierType::CONTROL_MASK)
                        && !modifier.contains(gdk::ModifierType::SHIFT_MASK)
                        && (key == gdk::Key::Return || key == gdk::Key::KP_Enter)
                    {
                        obj.emit_by_name::<()>("activate", &[]);
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
            ));
            self.text_view.add_controller(key_events);

            let press = gtk::GestureClick::new();
            press.connect_pressed(clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    obj.emit_by_name::<()>("emoji-button-press", &[&*obj.imp().emoji_button]);
                }
            ));
            self.emoji_button.add_controller(press);
            let hover = gtk::EventControllerMotion::new();
            hover.connect_enter(clone!(
                #[weak]
                obj,
                move |_, _, _| {
                    obj.emit_by_name::<()>("emoji-button-hover", &[&*obj.imp().emoji_button]);
                }
            ));
            hover.connect_leave(clone!(
                #[weak]
                obj,
                move |_| obj.emit_by_name::<()>("emoji-button-leave", &[])
            ));
            self.emoji_button.add_controller(hover);

            self.text_view.buffer().connect_changed(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.text_buffer_changed();
                }
            ));

            self.text_view.connect_paste_clipboard(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.emit_by_name::<()>("paste-clipboard", &[]);
                }
            ));
        }

        fn dispose(&self) {
            self.overlay.unparent();
        }
    }

    impl WidgetImpl for MessageEntry {
        fn grab_focus(&self) -> bool {
            self.text_view.grab_focus()
        }
    }

    #[gtk::template_callbacks]
    impl MessageEntry {
        #[template_callback]
        fn on_gesture_click_pressed(gesture_click: &gtk::GestureClick) {
            gesture_click.set_state(gtk::EventSequenceState::Claimed);
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageEntry(ObjectSubclass<imp::MessageEntry>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageEntry {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn text_buffer_changed(&self) {
        let imp = self.imp();
        let buffer = imp.text_view.buffer();
        let text: String = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .into();

        if text.is_empty() {
            self.clear_custom_emojis();
            imp.formatted_text.replace(None);
            imp.placeholder.set_visible(true);
        } else {
            let entities = self.rebuild_custom_emoji_entities();
            imp.formatted_text
                .replace(Some(model::BoxedFormattedText(tdlib::types::FormattedText {
                    text,
                    entities,
                })));

            imp.placeholder.set_visible(false);
        }

        self.notify("formatted-text");
    }

    /// Re-derives the custom-emoji entities from the tracking marks against the buffer's
    /// current contents, dropping any whose text has been deleted (the marks collapsed).
    fn rebuild_custom_emoji_entities(&self) -> Vec<tdlib::types::TextEntity> {
        let buffer = self.imp().text_view.buffer();
        let start = buffer.start_iter();
        let mut entities = Vec::new();

        self.imp().custom_emojis.borrow_mut().retain(|anchor| {
            let anchor_start = buffer.iter_at_mark(&anchor.start);
            let anchor_end = buffer.iter_at_mark(&anchor.end);
            if anchor_start.offset() >= anchor_end.offset() {
                buffer.delete_mark(&anchor.start);
                buffer.delete_mark(&anchor.end);
                return false;
            }

            let offset = buffer
                .text(&start, &anchor_start, true)
                .encode_utf16()
                .count() as i32;
            let length = buffer
                .text(&anchor_start, &anchor_end, true)
                .encode_utf16()
                .count() as i32;
            entities.push(tdlib::types::TextEntity {
                offset,
                length,
                r#type: tdlib::enums::TextEntityType::CustomEmoji(
                    tdlib::types::TextEntityTypeCustomEmoji {
                        custom_emoji_id: anchor.custom_emoji_id,
                    },
                ),
            });
            true
        });

        entities
    }

    fn clear_custom_emojis(&self) {
        let buffer = self.imp().text_view.buffer();
        for anchor in self.imp().custom_emojis.borrow_mut().drain(..) {
            buffer.delete_mark(&anchor.start);
            buffer.delete_mark(&anchor.end);
        }
    }

    /// Insert text inside the message entry at the cursor position,
    /// deleting eventual selected text
    pub(crate) fn insert_at_cursor(&self, text: &str) {
        let buffer = self.imp().text_view.buffer();
        buffer.begin_user_action();
        buffer.delete_selection(true, true);
        buffer.insert_at_cursor(text);
        buffer.end_user_action();
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        let buffer = self.imp().text_view.buffer();

        buffer.begin_user_action();
        buffer.delete_selection(true, true);
        let start_offset = buffer.cursor_position();
        buffer.insert_at_cursor(emoji);
        buffer.end_user_action();

        let start_iter = buffer.iter_at_offset(start_offset);
        let end_iter = buffer.iter_at_offset(start_offset + emoji.chars().count() as i32);
        // Right gravity on start, left gravity on end: boundary edits land outside the glyph.
        let start = buffer.create_mark(None, &start_iter, false);
        let end = buffer.create_mark(None, &end_iter, true);
        self.imp()
            .custom_emojis
            .borrow_mut()
            .push(imp::CustomEmojiAnchor {
                start,
                end,
                custom_emoji_id,
            });

        self.text_buffer_changed();
    }

    pub(crate) fn formatted_text(&self) -> Option<model::BoxedFormattedText> {
        self.imp().formatted_text.borrow().clone()
    }

    pub(crate) fn set_formatted_text(&self, formatted_text: Option<model::BoxedFormattedText>) {
        if self.formatted_text() == formatted_text {
            return;
        }

        self.clear_custom_emojis();
        let text = formatted_text.map(|f| f.0.text).unwrap_or_default();
        self.imp().text_view.buffer().set_text(&text);
    }

    pub(crate) async fn as_markdown(&self) -> Option<tdlib::types::FormattedText> {
        let text = self.imp().formatted_text.borrow().clone().map(|f| f.0)?;
        let client_id = self.chat().unwrap().session_().client_().id();

        tdlib::functions::parse_markdown(text.clone(), client_id)
            .await
            .map(|text| {
                let tdlib::enums::FormattedText::FormattedText(text) = text;
                text
            })
            .ok()
            .or(Some(text))
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    pub(crate) fn set_chat(&self, chat: Option<model::Chat>) {
        if self.chat() == chat {
            return;
        }

        self.imp().chat.set(chat.as_ref());
        self.notify("chat");
    }

    pub(crate) fn connect_formatted_text_notify<F: Fn(&Self, &glib::ParamSpec) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_notify_local(Some("formatted-text"), f)
    }

    pub(crate) fn placeholder_text(&self) -> Option<glib::GString> {
        self.imp().placeholder.text()
    }

    pub(crate) fn set_placeholder_text(&self, placeholder_text: Option<&str>) {
        self.imp().placeholder.set_text(placeholder_text);
    }

    pub(crate) fn connect_activate<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("activate", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);

            None
        })
    }

    pub(crate) fn connect_paste_clipboard<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("paste-clipboard", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);

            None
        })
    }

    pub(crate) fn connect_emoji_button_press<F: Fn(&Self, gtk::Image) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("emoji-button-press", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let button = values[1].get::<gtk::Image>().unwrap();
            f(&obj, button);

            None
        })
    }

    pub(crate) fn connect_emoji_button_hover<F: Fn(&Self, gtk::Image) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("emoji-button-hover", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let button = values[1].get::<gtk::Image>().unwrap();
            f(&obj, button);
            None
        })
    }

    pub(crate) fn connect_emoji_button_leave<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("emoji-button-leave", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);
            None
        })
    }
}

impl Default for MessageEntry {
    fn default() -> Self {
        Self::new()
    }
}
