use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::OnceLock;

use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::gsk;
use gtk::pango;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

/// A trailing space appended to the display label whose glyph is replaced by a shaped
/// box reserving room for the overlaid indicators. A plain space (not U+FFFC) so a
/// select-all + copy of the selectable label yields clean text without a garbage char.
const INDICATOR_GUTTER_CHAR: char = ' ';
const INDICATORS_SPACING: i32 = 6;

/// Pixel size custom-emoji textures are decoded at (drawn at ~1.3 em).
const EMOJI_TEXTURE_SIZE: i32 = 64;
/// Duration of the spoiler reveal animation, in microseconds.
const REVEAL_DURATION: f64 = 350_000.0;

thread_local! {
    /// Messages whose spoilers were revealed this session, so a revealed spoiler stays
    /// revealed when its row is recycled or the chat is reopened. Keyed by
    /// `(chat id, message id)`.
    static REVEALED_SPOILERS: RefCell<HashSet<(i64, i64)>> = RefCell::default();
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/label.ui")]
    pub(crate) struct MessageLabel {
        pub(super) text: RefCell<String>,
        pub(super) indicators: RefCell<Option<ui::MessageIndicators>>,
        pub(super) indicators_size: RefCell<Option<(i32, i32)>>,
        /// Byte index in the display label of the trailing gutter char reserving space for
        /// the overlaid indicators, or `None` when no gutter is reserved.
        pub(super) gutter_index: Cell<Option<u32>>,
        /// Extra rich-text data (spoilers, inline code, custom emoji) when set via
        /// [`MessageLabel::set_rich`]; `None` on the plain markup path.
        pub(super) rich: RefCell<Option<utils::RichInline>>,
        pub(super) session: RefCell<Option<model::ClientStateSession>>,
        /// `(chat id, message id)` for the session-wide revealed-spoilers set.
        pub(super) spoiler_key: Cell<Option<(i64, i64)>>,
        pub(super) spoilers_revealed: Cell<bool>,
        /// A running reveal animation: start frame time (µs) and the click point.
        pub(super) reveal_anim: Cell<Option<(i64, f32, f32)>>,
        pub(super) spoiler_tick: RefCell<Option<gtk::TickCallbackId>>,
        pub(super) emoji_textures: RefCell<HashMap<i64, gdk::Texture>>,
        pub(super) emoji_generation: Cell<u64>,
        #[template_child]
        pub(super) label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageLabel {
        const NAME: &'static str = "PaplMessageLabel";
        type Type = super::MessageLabel;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_css_name("messagelabel");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageLabel {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecString::builder("label")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecObject::builder::<ui::MessageIndicators>("indicators")
                        .construct_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "label" => obj.set_label(value.get().unwrap()),
                "indicators" => obj.set_indicators(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "label" => obj.label().to_value(),
                "indicators" => obj.indicators().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // In-app link schemes (mentions, bot commands, tags) emitted by
            // `utils::convert_to_markup`.
            self.label.connect_activate_link(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, uri| obj.handle_link(uri)
            ));

            // Spoiler reveal and inline-code copy. Capture phase so the selectable
            // label's own caret handling doesn't swallow the press.
            let click = gtk::GestureClick::new();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(clone!(
                #[weak]
                obj,
                move |gesture, _, x, y| {
                    if obj.reveal_spoiler_at(x, y) {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    }
                }
            ));
            click.connect_released(clone!(
                #[weak]
                obj,
                move |gesture, n_press, x, y| {
                    if n_press == 1 && obj.copy_code_at(x, y) {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                    }
                }
            ));
            obj.add_controller(click);
        }

        fn dispose(&self) {
            if let Some(tick) = self.spoiler_tick.take() {
                tick.remove();
            }
            self.label.unparent();
            if let Some(indicators) = self.indicators.take() {
                indicators.unparent();
            }
        }
    }

    impl WidgetImpl for MessageLabel {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();

            if let Some(indicators) = self.indicators.borrow().as_ref() {
                let (_, indicators_size) = indicators.preferred_size();
                let old = self
                    .indicators_size
                    .replace(Some((indicators_size.width(), indicators_size.height())));

                if let Some(old_indicators_size) = old {
                    if indicators_size.width() != old_indicators_size.0
                        || indicators_size.height() != old_indicators_size.1
                    {
                        obj.update_label_attributes(&indicators_size);
                    }
                } else {
                    obj.update_label_attributes(&indicators_size);
                }

                let (mut minimum, mut natural, minimum_baseline, natural_baseline) =
                    self.label.measure(orientation, for_size);
                let (indicators_min, indicators_nat, _, _) =
                    indicators.measure(orientation, for_size);

                minimum = minimum.max(indicators_min);
                natural = natural.max(indicators_nat);

                if orientation == gtk::Orientation::Vertical && obj.is_opposite_text_direction() {
                    minimum += indicators_min;
                    natural += indicators_nat;
                }

                (minimum, natural, minimum_baseline, natural_baseline)
            } else {
                self.label.measure(orientation, for_size)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.label.allocate(width, height, baseline, None);
            if let Some(indicators) = self.indicators.borrow().as_ref() {
                indicators.allocate(width, height, baseline, None);
            }
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.label.request_mode()
        }

        fn direction_changed(&self, _previous_direction: gtk::TextDirection) {
            self.obj().update_label();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.parent_snapshot(snapshot);
            self.obj().snapshot_rich(snapshot);
        }

        fn map(&self) {
            self.parent_map();
            self.obj().update_spoiler_ticker();
        }

        fn unmap(&self) {
            self.parent_unmap();
            if let Some(tick) = self.spoiler_tick.take() {
                tick.remove();
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageLabel(ObjectSubclass<imp::MessageLabel>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for MessageLabel {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageLabel {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn update_label_attributes(&self, indicators_size: &gtk::Requisition) {
        let imp = self.imp();
        let attrs = pango::AttrList::new();

        // Reserved box for the overlaid indicators at the trailing gutter char.
        if let Some(start_index) = imp.gutter_index.get() {
            let width = indicators_size.width() + INDICATORS_SPACING;
            let height = indicators_size.height();
            let logical_rect = pango::Rectangle::new(
                0,
                -(height - (height / 4)) * pango::SCALE,
                width * pango::SCALE,
                height * pango::SCALE,
            );
            let mut shape = pango::AttrShape::new(&logical_rect, &logical_rect);

            shape.set_start_index(start_index);
            shape.set_end_index(start_index + INDICATOR_GUTTER_CHAR.len_utf8() as u32);
            attrs.insert(shape);
        }

        // Reserved squares for inline custom emoji, drawn in `snapshot_rich`.
        if let Some(rich) = imp.rich.borrow().as_ref()
            && !rich.custom_emoji.is_empty()
        {
            let metrics = imp.label.pango_context().metrics(None, None);
            let side = metrics.ascent() + metrics.descent();
            let rect = pango::Rectangle::new(0, -metrics.ascent(), side, side);
            for &(start, end, _) in &rich.custom_emoji {
                let mut shape = pango::AttrShape::new(&rect, &rect);
                shape.set_start_index(start as u32);
                shape.set_end_index(end as u32);
                attrs.insert(shape);
            }
        }

        imp.label.set_attributes(Some(&attrs));
    }

    fn is_opposite_text_direction(&self) -> bool {
        let text = self.imp().text.borrow();
        let text_direction = pango::find_base_dir(&text);
        let widget_direction = self.direction();

        (text_direction == pango::Direction::Rtl && widget_direction == gtk::TextDirection::Ltr)
            || text_direction == pango::Direction::Ltr
                && widget_direction == gtk::TextDirection::Rtl
    }

    fn update_label(&self) {
        let imp = self.imp();
        let text = imp.text.borrow();
        if let Some(indicators) = imp.indicators.borrow().as_ref() {
            if !self.is_opposite_text_direction() {
                imp.gutter_index.set(Some(text.len() as u32));
                imp.label
                    .set_label(&format!("{text}{INDICATOR_GUTTER_CHAR}"));
            } else {
                imp.gutter_index.set(None);
                imp.label.set_label(&text);
            }

            let (_, indicators_size) = indicators.preferred_size();
            self.update_label_attributes(&indicators_size);
        } else {
            imp.gutter_index.set(None);
            imp.label.set_label(&text);
            self.update_label_attributes(&gtk::Requisition::default());
        }
    }

    pub(crate) fn label(&self) -> String {
        self.imp().text.borrow().clone()
    }

    /// Plain Pango-markup path (captions, unsupported content): clears any rich-text
    /// state a recycled label carries.
    pub(crate) fn set_label(&self, label: String) {
        let imp = self.imp();
        imp.rich.replace(None);
        imp.reveal_anim.set(None);
        imp.emoji_generation.set(imp.emoji_generation.get() + 1);
        imp.emoji_textures.borrow_mut().clear();
        self.update_spoiler_ticker();

        let old = imp.text.replace(label);
        if old != *imp.text.borrow() {
            self.update_label();
            self.notify("label");
        } else {
            // The attribute list depends on rich state, which changed even if the
            // markup did not.
            self.update_label();
        }
    }

    /// Rich path: markup plus spoiler / inline-code / custom-emoji ranges.
    /// `spoiler_key` identifies the message in the session-wide revealed-spoilers set.
    pub(crate) fn set_rich(
        &self,
        inline: utils::RichInline,
        session: Option<model::ClientStateSession>,
        spoiler_key: Option<(i64, i64)>,
    ) {
        let imp = self.imp();

        imp.session.replace(session);
        imp.spoiler_key.set(spoiler_key);
        imp.spoilers_revealed.set(
            inline.spoilers.is_empty()
                || spoiler_key
                    .is_some_and(|key| REVEALED_SPOILERS.with(|s| s.borrow().contains(&key))),
        );
        imp.reveal_anim.set(None);
        imp.emoji_generation.set(imp.emoji_generation.get() + 1);
        imp.emoji_textures.borrow_mut().clear();

        let markup = inline.markup.clone();
        imp.rich.replace(Some(inline));

        let old = imp.text.replace(markup);
        if old != *imp.text.borrow() {
            self.notify("label");
        }
        self.update_label();
        self.load_custom_emoji();
        self.update_spoiler_ticker();
        self.queue_draw();
    }

    /// The number of laid-out visual lines (only meaningful once the label has been
    /// allocated). Used by collapsible block quotes to skip the clamp on short text.
    pub(crate) fn line_count(&self) -> i32 {
        self.imp().label.layout().line_count()
    }

    pub(crate) fn add_label_class(&self, class: &str) {
        self.imp().label.add_css_class(class);
    }

    /// Toggles caret text selection. Disabled while the chat is in multi-select mode, so
    /// clicks toggle message selection instead of placing a text cursor.
    pub(crate) fn set_text_selectable(&self, selectable: bool) {
        let label = &self.imp().label;
        if label.is_selectable() != selectable {
            label.set_selectable(selectable);
        }
    }

    pub(crate) fn indicators(&self) -> Option<ui::MessageIndicators> {
        self.imp().indicators.borrow().clone()
    }

    pub(crate) fn set_indicators(&self, indicators: Option<ui::MessageIndicators>) {
        let imp = self.imp();
        let old = imp.indicators.replace(indicators);
        if old != *imp.indicators.borrow() {
            if let Some(old) = old {
                old.unparent();
            }

            if let Some(indicators) = imp.indicators.borrow().as_ref() {
                indicators.set_parent(self);
            }

            self.update_label();
            self.notify("indicators");
        }
    }

    // --- Rich text: links ---

    /// Resolves the in-app link schemes emitted by `utils::convert_to_markup`.
    fn handle_link(&self, uri: &str) -> glib::Propagation {
        if let Some(username) = uri.strip_prefix("mention:") {
            let username = username.trim_start_matches('@').to_string();
            if let Some(session) = utils::ancestor::<_, ui::Session>(self) {
                utils::spawn(async move {
                    let client_id = session.model().unwrap().client_().id();
                    match tdlib::functions::search_public_chat(username, client_id).await {
                        Ok(tdlib::enums::Chat::Chat(chat)) => session.select_chat(chat.id),
                        Err(e) => utils::show_toast(&session, e.message),
                    }
                });
            }
        } else if let Some(user_id) = uri.strip_prefix("mention-id:") {
            if let (Ok(user_id), Some(session)) = (
                user_id.parse::<i64>(),
                utils::ancestor::<_, ui::Session>(self),
            ) {
                session.select_chat(user_id);
            }
        } else if let Some(command) = uri.strip_prefix("command:") {
            self.send_bot_command(command.to_string());
        } else if let Some(tag) = uri.strip_prefix("tag:") {
            self.clipboard().set_text(tag);
            utils::show_toast(self, gettext("Copied to clipboard"));
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    }

    /// Sends a clicked bot command (e.g. `/start`) as a message to the current chat.
    fn send_bot_command(&self, command: String) {
        let Some(chat) = utils::ancestor::<_, ui::ChatHistory>(self).and_then(|h| h.chat()) else {
            return;
        };
        utils::spawn(async move {
            let content = tdlib::enums::InputMessageContent::InputMessageText(
                tdlib::types::InputMessageText {
                    text: tdlib::types::FormattedText {
                        text: command,
                        entities: Vec::new(),
                    },
                    link_preview_options: None,
                    clear_draft: false,
                },
            );
            if let Err(e) = tdlib::functions::send_message(
                chat.id(),
                None,
                None,
                None,
                content,
                chat.session_().client_().id(),
            )
            .await
            {
                log::warn!("Failed to send a bot command: {e:?}");
            }
        });
    }

    // --- Rich text: hit testing ---

    /// The label-layout byte index under widget coordinates `(x, y)`, if inside text.
    fn index_at(&self, x: f64, y: f64) -> Option<usize> {
        let label = &*self.imp().label;
        let point = self.compute_point(label, &graphene::Point::new(x as f32, y as f32))?;
        let (off_x, off_y) = label.layout_offsets();
        let layout = label.layout();
        let (inside, index, _trailing) = layout.xy_to_index(
            (point.x() as i32 - off_x) * pango::SCALE,
            (point.y() as i32 - off_y) * pango::SCALE,
        );
        inside.then_some(index as usize)
    }

    /// Reveals hidden spoilers when `(x, y)` presses one. Returns whether it did.
    fn reveal_spoiler_at(&self, x: f64, y: f64) -> bool {
        let imp = self.imp();
        if imp.spoilers_revealed.get() {
            return false;
        }
        let over_spoiler = matches!(
            (self.index_at(x, y), imp.rich.borrow().as_ref()),
            (Some(index), Some(rich))
                if rich.spoilers.iter().any(|&(s, e)| (s..e).contains(&index))
        );
        if !over_spoiler {
            return false;
        }

        imp.spoilers_revealed.set(true);
        if let Some(key) = imp.spoiler_key.get() {
            REVEALED_SPOILERS.with(|s| s.borrow_mut().insert(key));
        }
        let start = self
            .frame_clock()
            .map(|clock| clock.frame_time())
            .unwrap_or_default();
        imp.reveal_anim.set(Some((start, x as f32, y as f32)));
        self.update_spoiler_ticker();
        self.queue_draw();
        true
    }

    /// Copies the inline-code run under `(x, y)` to the clipboard. Returns whether it did.
    fn copy_code_at(&self, x: f64, y: f64) -> bool {
        let imp = self.imp();
        let Some(index) = self.index_at(x, y) else {
            return false;
        };
        let code = imp.rich.borrow().as_ref().and_then(|rich| {
            rich.codes
                .iter()
                .find(|&&(s, e)| (s..e).contains(&index))
                .map(|&(s, e)| rich.plain[s..e].to_string())
        });
        match code {
            Some(code) => {
                self.clipboard().set_text(&code);
                utils::show_toast(self, gettext("Code copied to clipboard"));
                true
            }
            None => false,
        }
    }

    // --- Rich text: custom emoji ---

    /// Resolves this label's custom-emoji ids to sticker preview textures.
    fn load_custom_emoji(&self) {
        let imp = self.imp();
        let Some(rich) = imp.rich.borrow().clone() else {
            return;
        };
        if rich.custom_emoji.is_empty() {
            return;
        }
        let Some(session) = imp.session.borrow().clone() else {
            return;
        };

        let mut ids: Vec<i64> = rich.custom_emoji.iter().map(|&(.., id)| id).collect();
        ids.sort_unstable();
        ids.dedup();
        let generation = imp.emoji_generation.get();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let client_id = session.client_().id();
                let Ok(tdlib::enums::Stickers::Stickers(data)) =
                    tdlib::functions::get_custom_emoji_stickers(ids, client_id).await
                else {
                    return;
                };
                for sticker in data.stickers {
                    if obj.imp().emoji_generation.get() != generation {
                        return;
                    }
                    let tdlib::enums::StickerFullType::CustomEmoji(full_type) = &sticker.full_type
                    else {
                        continue;
                    };
                    let emoji_id = full_type.custom_emoji_id;
                    let demand = Rc::new(Cell::new(true));
                    if let Ok(texture) =
                        ui::load_picker_preview(&sticker, EMOJI_TEXTURE_SIZE, &session, demand)
                            .await
                        && obj.imp().emoji_generation.get() == generation
                    {
                        obj.imp()
                            .emoji_textures
                            .borrow_mut()
                            .insert(emoji_id, texture);
                        obj.queue_draw();
                    }
                }
            }
        ));
    }

    // --- Rich text: drawing ---

    /// The widget-space rectangles covering byte range `start..end` of the label text,
    /// one per intersected layout line.
    fn range_rects(&self, start: usize, end: usize) -> Vec<graphene::Rect> {
        let label = &self.imp().label;
        let layout = label.layout();
        let (off_x, off_y) = label.layout_offsets();
        let origin = label
            .compute_bounds(self)
            .map(|b| (b.x(), b.y()))
            .unwrap_or_default();

        let mut rects = Vec::new();
        for i in 0..layout.line_count() {
            let Some(line) = layout.line(i) else {
                continue;
            };
            let line_start = line.start_index() as usize;
            let line_end = line_start + line.length() as usize;
            let s = start.max(line_start);
            let e = end.min(line_end);
            if s >= e {
                continue;
            }

            let x0 = line.index_to_x(s as i32, false);
            let x1 = if e >= line_end {
                let (_, logical) = line.extents();
                x0.min(logical.x()) + logical.width()
            } else {
                line.index_to_x(e as i32, false)
            };
            let pos = layout.index_to_pos(s as i32);
            let (x0, x1) = (x0.min(x1), x0.max(x1));

            rects.push(graphene::Rect::new(
                origin.0 + off_x as f32 + (x0 / pango::SCALE) as f32,
                origin.1 + off_y as f32 + (pos.y() / pango::SCALE) as f32,
                ((x1 - x0) / pango::SCALE) as f32,
                (pos.height() / pango::SCALE) as f32,
            ));
        }
        rects
    }

    /// Draws custom emoji into their reserved boxes and the animated spoiler overlay.
    fn snapshot_rich(&self, snapshot: &gtk::Snapshot) {
        let imp = self.imp();
        let Some(rich) = imp.rich.borrow().clone() else {
            return;
        };

        // Custom emoji textures over their shape-attribute boxes.
        if !rich.custom_emoji.is_empty() {
            let label = &imp.label;
            let layout = label.layout();
            let (off_x, off_y) = label.layout_offsets();
            let origin = label
                .compute_bounds(self)
                .map(|b| (b.x(), b.y()))
                .unwrap_or_default();
            let textures = imp.emoji_textures.borrow();
            for &(start, _, id) in &rich.custom_emoji {
                let Some(texture) = textures.get(&id) else {
                    continue;
                };
                let pos = layout.index_to_pos(start as i32);
                let side = (pos.height() / pango::SCALE) as f32;
                snapshot.append_texture(
                    texture,
                    &graphene::Rect::new(
                        origin.0 + off_x as f32 + (pos.x() / pango::SCALE) as f32,
                        origin.1 + off_y as f32 + (pos.y() / pango::SCALE) as f32,
                        side,
                        side,
                    ),
                );
            }
        }

        // Spoiler overlay: opaque bubble-colored cover with twinkling particles, cut
        // open by an expanding circle while the reveal animation runs.
        if !rich.spoilers.is_empty() {
            let anim = imp.reveal_anim.get();
            if imp.spoilers_revealed.get() && anim.is_none() {
                return;
            }

            let now = self
                .frame_clock()
                .map(|clock| clock.frame_time())
                .unwrap_or_default();
            let (progress, click) = match anim {
                Some((start, x, y)) => {
                    let progress = ((now - start) as f64 / REVEAL_DURATION).clamp(0.0, 1.0);
                    if progress >= 1.0 {
                        imp.reveal_anim.set(None);
                        self.update_spoiler_ticker();
                        return;
                    }
                    (progress as f32, Some((x, y)))
                }
                None => (0.0, None),
            };

            #[allow(deprecated)]
            let style = self.style_context();
            let outgoing = utils::ancestor::<_, ui::MessageBubble>(self)
                .is_some_and(|bubble| bubble.has_css_class("outgoing"));
            #[allow(deprecated)]
            let cover = style
                .lookup_color(if outgoing {
                    "accent_bg_color"
                } else {
                    "message_bubble_bg"
                })
                .unwrap_or_else(|| gdk::RGBA::new(0.5, 0.5, 0.5, 1.0));
            #[allow(deprecated)]
            let particle = style.color();

            let rects: Vec<graphene::Rect> = rich
                .spoilers
                .iter()
                .flat_map(|&(s, e)| self.range_rects(s, e))
                .collect();

            // The expanding reveal circle, sized to cover the farthest overlay corner.
            let circle = click.map(|(cx, cy)| {
                let max_dist = rects
                    .iter()
                    .flat_map(|r| {
                        [
                            (r.x(), r.y()),
                            (r.x() + r.width(), r.y()),
                            (r.x(), r.y() + r.height()),
                            (r.x() + r.width(), r.y() + r.height()),
                        ]
                    })
                    .map(|(x, y)| (x - cx).hypot(y - cy))
                    .fold(0.0f32, f32::max);
                (cx, cy, max_dist * progress)
            });

            let builder = gsk::PathBuilder::new();
            for rect in &rects {
                builder.add_rounded_rect(&gsk::RoundedRect::from_rect(*rect, 4.0));
            }
            if let Some((cx, cy, r)) = circle {
                builder.add_circle(&graphene::Point::new(cx, cy), r);
            }
            snapshot.append_fill(&builder.to_path(), gsk::FillRule::EvenOdd, &cover);

            // Twinkling dust: deterministic pseudo-random drift per rect, skipping
            // particles inside the reveal circle.
            let t = now as f64 / 1_000_000.0;
            for (rect_index, rect) in rects.iter().enumerate() {
                let count = ((rect.width() * rect.height()) / 28.0).clamp(4.0, 256.0) as usize;
                for i in 0..count {
                    let seed = (rect_index * 512 + i) as f64;
                    let fx = (seed * 12.9898).sin().abs().fract();
                    let fy = (seed * 78.233).sin().abs().fract();
                    let speed = 0.02 + (seed * 3.14).sin().abs().fract() * 0.05;
                    let x = rect.x() + (((fx + t * speed) % 1.0) * rect.width() as f64) as f32;
                    let y =
                        rect.y() + (((fy + t * speed * 0.7) % 1.0) * rect.height() as f64) as f32;
                    if let Some((cx, cy, r)) = circle
                        && (x - cx).hypot(y - cy) < r
                    {
                        continue;
                    }
                    let alpha = 0.25 + 0.6 * (0.5 + 0.5 * (t * 2.5 + seed).sin()) as f32;
                    snapshot.append_color(
                        &gdk::RGBA::new(
                            particle.red(),
                            particle.green(),
                            particle.blue(),
                            alpha * (1.0 - progress),
                        ),
                        &graphene::Rect::new(x - 0.75, y - 0.75, 1.5, 1.5),
                    );
                }
            }
        }
    }

    /// Keeps a frame-clock ticker running exactly while an overlay animates: hidden
    /// spoilers twinkle, and the reveal animation plays out.
    fn update_spoiler_ticker(&self) {
        let imp = self.imp();
        let needs_ticker = self.is_mapped()
            && imp
                .rich
                .borrow()
                .as_ref()
                .is_some_and(|rich| !rich.spoilers.is_empty())
            && (!imp.spoilers_revealed.get() || imp.reveal_anim.get().is_some());

        if needs_ticker {
            if imp.spoiler_tick.borrow().is_none() {
                let tick = self.add_tick_callback(|obj, _| {
                    obj.queue_draw();
                    glib::ControlFlow::Continue
                });
                imp.spoiler_tick.replace(Some(tick));
            }
        } else if let Some(tick) = imp.spoiler_tick.take() {
            tick.remove();
        }
    }
}
