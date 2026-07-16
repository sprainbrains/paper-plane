use std::cell::RefCell;

use adw::prelude::*;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

const MAX_REPLY_CHAR_WIDTH: i32 = 18;

const STICKER_SIZE: i32 = 176;
const EMOJI_SIZE: i32 = 112;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/sticker.ui")]
    #[properties(wrapper_type = super::MessageSticker)]
    pub(crate) struct MessageSticker {
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        #[template_child]
        pub(super) overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) click: TemplateChild<gtk::GestureClick>,
        #[template_child]
        pub(super) sticker: TemplateChild<ui::Sticker>,
        #[template_child]
        pub(super) indicators: TemplateChild<ui::MessageIndicators>,
        pub(super) reply: RefCell<Option<ui::MessageReply>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageSticker {
        const NAME: &'static str = "PaplMessageSticker";
        type Type = super::MessageSticker;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.set_css_name("messagesticker");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageSticker {
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

    impl WidgetImpl for MessageSticker {}
    impl ui::MessageBaseImpl for MessageSticker {}

    #[gtk::template_callbacks]
    impl MessageSticker {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }

        #[template_callback]
        fn on_pressed(&self, _n_press: i32, _x: f64, _y: f64) {
            self.sticker.play_animation();

            // Clicking a sticker reveals its pack (in the sidebar or a popup).
            if let Some(message) = self.message.upgrade()
                && let Some(set_id) = sticker_set_id(&message)
                && let Some(session) = utils::ancestor::<_, ui::Session>(&*self.obj())
            {
                session.show_sticker_pack(set_id);
            }
        }
    }
}

/// The sticker set id of a sticker/animated-emoji message, or `None` if it belongs to
/// no set.
fn sticker_set_id(message: &model::Message) -> Option<i64> {
    let set_id = match message.content().0 {
        tdlib::enums::MessageContent::MessageSticker(data) => data.sticker.set_id,
        tdlib::enums::MessageContent::MessageAnimatedEmoji(data) => {
            data.animated_emoji.sticker.map(|sticker| sticker.set_id)?
        }
        _ => return None,
    };
    (set_id != 0).then_some(set_id)
}

glib::wrapper! {
    pub(crate) struct MessageSticker(ObjectSubclass<imp::MessageSticker>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageSticker {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        imp.message.set(Some(message));

        imp.indicators.set_message(message.upcast_ref());

        // Rows are recycled by the `ListView`, so drop any reply left over from a
        // previously bound message before (maybe) building a new one.
        if let Some(old_reply) = imp.reply.take() {
            old_reply.unparent();
        }

        if message.reply_to().is_some() {
            let reply = ui::MessageReply::new(message);
            reply.set_valign(gtk::Align::Start);
            reply.set_max_char_width(MAX_REPLY_CHAR_WIDTH);

            // FIXME: Do not show message reply when message is being deleted
            // Sticker and the reply should be at the opposite sides of the box
            if message.is_outgoing() {
                reply.insert_before(self, Some(&imp.overlay.get()));
            } else {
                reply.insert_after(self, Some(&imp.overlay.get()));
            }

            imp.reply.replace(Some(reply));
        }

        let (sticker, looped, is_emoji) = match message.content().0 {
            tdlib::enums::MessageContent::MessageSticker(data) => {
                let sticker = data.sticker;
                (sticker, true, false)
            }
            tdlib::enums::MessageContent::MessageAnimatedEmoji(data) => {
                let sticker = data.animated_emoji.sticker.unwrap();
                let looped = matches!(
                    sticker.full_type,
                    tdlib::enums::StickerFullType::CustomEmoji(_)
                );
                (sticker, looped, true)
            }
            _ => unreachable!(),
        };

        // TODO: that should be handled a bit better in the future
        match &sticker.full_type {
            tdlib::enums::StickerFullType::CustomEmoji(data) if data.needs_repainting => {
                self.add_css_class("needs-repainting")
            }
            _ => self.remove_css_class("needs-repainting"),
        }

        let (size, margin_bottom) = if is_emoji {
            (EMOJI_SIZE, 8)
        } else {
            (STICKER_SIZE, 0)
        };

        imp.sticker.set_longer_side_size(size);
        imp.sticker.set_margin_bottom(margin_bottom);

        imp.sticker
            .update_sticker(sticker, looped, message.chat_().session_());

        self.notify("message");
    }
}
