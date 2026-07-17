use std::cell::Cell;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::OnceLock;

use adw::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::graphene;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

const MAX_WIDTH: i32 = 400;

/// Bubble corner radii: full for a standalone/outer corner, tight where a bubble abuts
/// its same-sender neighbour in a run. Keep in sync with the `.grouped-*` CSS radii.
const RADIUS: f64 = 15.0;
const RADIUS_TIGHT: f64 = 6.0;

/// Tail geometry (tunable), for the code-drawn outline of a group's last bubble.
/// `START` is how far along the bottom edge the tail begins, `OVERHANG` how far past the
/// bubble edge the tip reaches, `HEIGHT` how far up the side it blends back in.
const TAIL_START: f64 = 9.0;
const TAIL_OVERHANG: f64 = 4.0;
const TAIL_HEIGHT: f64 = 12.0;

/// Where the message indicators (timestamp etc.) currently live. The template starts
/// them overlaid on the content (see `bubble.blp`), and they migrate as the message
/// gains a text label or reactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndicatorPlacement {
    /// Overlaid bottom-right on media / an empty-caption bubble.
    Overlay,
    /// Trailing the text inside the message label.
    Label,
    /// Bottom-right of the reaction row.
    Reactions,
}

impl Default for IndicatorPlacement {
    fn default() -> Self {
        Self::Overlay
    }
}
const SENDER_COLOR_CLASSES: &[&str] = &[
    "sender-text-red",
    "sender-text-orange",
    "sender-text-violet",
    "sender-text-green",
    "sender-text-cyan",
    "sender-text-blue",
    "sender-text-pink",
];

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/bubble.ui")]
    pub(crate) struct MessageBubble {
        pub(super) sender_color_class: RefCell<Option<String>>,
        pub(super) sender_binding: RefCell<Option<gtk::ExpressionWatch>>,
        pub(super) indicator_placement: Cell<IndicatorPlacement>,
        pub(super) group_start: Cell<bool>,
        pub(super) group_end: Cell<bool>,
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) hover_pill: RefCell<Option<gtk::Box>>,
        pub(super) hover_reaction_button: RefCell<Option<gtk::Button>>,
        #[template_child]
        pub(super) box_: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) sender_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) message_reply_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub(super) prefix_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub(super) blocks_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) message_label: TemplateChild<ui::MessageLabel>,
        #[template_child]
        pub(super) indicators: TemplateChild<ui::MessageIndicators>,
        #[template_child]
        pub(super) suffix_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub(super) reactions: TemplateChild<ui::MessageReactions>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageBubble {
        const NAME: &'static str = "PaplMessageBubble";
        type Type = super::MessageBubble;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_css_name("messagebubble");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageBubble {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<gtk::Widget>("prefix")
                        .write_only()
                        .build(),
                    glib::ParamSpecString::builder("label").write_only().build(),
                    glib::ParamSpecObject::builder::<gtk::Widget>("suffix")
                        .write_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "prefix" => obj.set_prefix(value.get().unwrap()),
                "label" => obj.set_label(value.get().unwrap()),
                "suffix" => obj.set_suffix(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_hover_actions();
        }

        fn dispose(&self) {
            self.dispose_template();
        }
    }

    impl WidgetImpl for MessageBubble {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // Limit the widget width
            if orientation == gtk::Orientation::Horizontal {
                let (minimum, natural, minimum_baseline, natural_baseline) =
                    self.box_.measure(orientation, for_size);

                (
                    minimum.min(MAX_WIDTH),
                    natural.min(MAX_WIDTH),
                    minimum_baseline,
                    natural_baseline,
                )
            } else {
                let adjusted_for_size = for_size.min(MAX_WIDTH);
                self.box_.measure(orientation, adjusted_for_size)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.box_.allocate(width, height, baseline, None);
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            // Every bubble draws its own background as one continuous filled outline — a
            // rounded rectangle whose sender-side bottom corner flows into a tail on the
            // last bubble of a run — so the corner, body, tail, and shadow are one shape.
            // The CSS paints no background (see style.css); only the content sits on top.
            self.snapshot_shape(snapshot);
            self.parent_snapshot(snapshot);
        }
    }

    impl MessageBubble {
        /// Paints the bubble outline (rounded rect + tail) as a single filled path, with a
        /// drop shadow (below and toward the tail, so a light bubble's tail stays visible on
        /// a light background) and, in high-contrast, a hairline border.
        fn snapshot_shape(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f64;
            let h = obj.height() as f64;
            if w <= 0.0 || h <= 0.0 {
                return;
            }

            let outgoing = obj.has_css_class("outgoing");
            let tail = self.group_end.get();
            // Corners tighten where a bubble abuts its same-sender neighbour; media / map
            // bubbles keep full corners to match their self-clipping picture/map.
            let media = obj.has_css_class("media")
                || obj.has_css_class("location")
                || obj.has_css_class("venue");
            let top = if media || self.group_start.get() {
                RADIUS
            } else {
                RADIUS_TIGHT
            };
            let bottom = if media || tail { RADIUS } else { RADIUS_TIGHT };

            #[allow(deprecated)]
            let style = obj.style_context();
            #[allow(deprecated)]
            let color = style
                .lookup_color(if outgoing {
                    "accent_bg_color"
                } else {
                    "message_bubble_bg"
                })
                .unwrap_or_else(|| gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 1.0));

            let bounds = graphene::Rect::new(
                (-TAIL_OVERHANG - 2.0) as f32,
                -1.0,
                (w + 2.0 * (TAIL_OVERHANG + 2.0)) as f32,
                (h + 2.0) as f32,
            );
            let ctx = snapshot.append_cairo(&bounds);

            // Drop shadow: the outline filled 1px down in translucent black.
            let _ = ctx.save();
            ctx.translate(0.0, 1.0);
            append_bubble_path(&ctx, w, h, top, bottom, outgoing, tail);
            ctx.set_source_rgba(0.0, 0.0, 0.0, 0.1);
            let _ = ctx.fill();
            let _ = ctx.restore();

            // A second shadow offset toward the tail, so the tail casts a shadow on its
            // outer edge and reads against a same-colored background. Clipped to the tail's
            // band so it doesn't shade the whole sender-side edge (which showed as a
            // discolored rim along the corners on a vivid dark wallpaper).
            if tail {
                let tail_dx = if outgoing { 1.5 } else { -1.5 };
                let (cx0, cx1) = if outgoing {
                    (w - TAIL_START - 2.0, w + TAIL_OVERHANG + 2.0)
                } else {
                    (-TAIL_OVERHANG - 2.0, TAIL_START + 2.0)
                };
                let _ = ctx.save();
                ctx.rectangle(cx0, h - TAIL_HEIGHT - 2.0, cx1 - cx0, TAIL_HEIGHT + 4.0);
                ctx.clip();
                ctx.translate(tail_dx, 1.0);
                append_bubble_path(&ctx, w, h, top, bottom, outgoing, tail);
                ctx.set_source_rgba(0.0, 0.0, 0.0, 0.1);
                let _ = ctx.fill();
                let _ = ctx.restore();
            }

            append_bubble_path(&ctx, w, h, top, bottom, outgoing, tail);
            ctx.set_source_rgba(
                color.red() as f64,
                color.green() as f64,
                color.blue() as f64,
                color.alpha() as f64,
            );
            ctx.fill_preserve().ok();

            if adw::StyleManager::default().is_high_contrast() {
                #[allow(deprecated)]
                let border = style
                    .lookup_color("borders")
                    .unwrap_or_else(|| gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.15));
                ctx.set_source_rgba(
                    border.red() as f64,
                    border.green() as f64,
                    border.blue() as f64,
                    border.alpha() as f64,
                );
                ctx.set_line_width(1.0);
                let _ = ctx.stroke();
            } else {
                ctx.new_path();
            }
        }
    }
}

/// Appends the bubble outline to `ctx`: a rounded rectangle with `top`/`bottom` corner
/// radii, except the sender-side bottom corner, which is replaced by a tail when `tail`
/// (bottom-left for incoming, bottom-right for outgoing).
fn append_bubble_path(
    ctx: &gtk::cairo::Context,
    w: f64,
    h: f64,
    top: f64,
    bottom: f64,
    outgoing: bool,
    tail: bool,
) {
    use std::f64::consts::FRAC_PI_2;
    use std::f64::consts::PI;

    ctx.new_path();
    ctx.move_to(top, 0.0);
    ctx.line_to(w - top, 0.0);
    ctx.arc(w - top, top, top, -FRAC_PI_2, 0.0); // top-right

    if outgoing && tail {
        // Right edge down to the tail blend, then out to the tip and back to the bottom edge.
        ctx.line_to(w, h - TAIL_HEIGHT);
        ctx.curve_to(w, h - 8.0, w + TAIL_OVERHANG * 0.2, h - 5.0, w + TAIL_OVERHANG, h - 1.5);
        ctx.curve_to(w + TAIL_OVERHANG * 0.4, h, w - TAIL_START + 4.0, h, w - TAIL_START, h);
    } else {
        ctx.line_to(w, h - bottom);
        ctx.arc(w - bottom, h - bottom, bottom, 0.0, FRAC_PI_2); // bottom-right
    }

    if !outgoing && tail {
        ctx.line_to(TAIL_START, h);
        ctx.curve_to(TAIL_START - 4.0, h, -TAIL_OVERHANG * 0.4, h, -TAIL_OVERHANG, h - 1.5);
        ctx.curve_to(-TAIL_OVERHANG * 0.2, h - 5.0, 0.0, h - 8.0, 0.0, h - TAIL_HEIGHT);
    } else {
        ctx.line_to(bottom, h);
        ctx.arc(bottom, h - bottom, bottom, FRAC_PI_2, PI); // bottom-left
    }

    ctx.line_to(0.0, top);
    ctx.arc(top, top, top, PI, 1.5 * PI); // top-left
    ctx.close_path();
}

glib::wrapper! {
    pub(crate) struct MessageBubble(ObjectSubclass<imp::MessageBubble>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageBubble {
    /// Builds the hover-actions island (Reply + the favorite reaction) overlaid at the
    /// bubble's top-trailing corner, revealed only while the pointer is over the bubble
    /// and never in multi-select mode.
    fn setup_hover_actions(&self) {
        let imp = self.imp();

        let reply_button = hover_icon_button("mail-reply-sender-symbolic");
        reply_button.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.hover_reply()
        ));

        let reaction_button = hover_icon_button("");
        reaction_button.set_visible(false);
        reaction_button.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.hover_react()
        ));

        let pill = gtk::Box::builder()
            .css_classes(["hover-actions"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .visible(false)
            .build();
        pill.append(&reply_button);
        pill.append(&reaction_button);

        imp.overlay.add_overlay(&pill);
        imp.hover_pill.replace(Some(pill));
        imp.hover_reaction_button.replace(Some(reaction_button));

        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _| obj.set_hover_actions_revealed(true)
        ));
        motion.connect_leave(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.set_hover_actions_revealed(false)
        ));
        self.add_controller(motion);
    }

    fn set_hover_actions_revealed(&self, revealed: bool) {
        let imp = self.imp();
        // Never while selecting: the pill would fight the selection click/drag.
        let in_selection = utils::ancestor::<_, ui::ChatHistory>(self)
            .is_some_and(|history| history.is_selection_mode());
        if let Some(pill) = imp.hover_pill.borrow().as_ref() {
            pill.set_visible(revealed && !in_selection && imp.message.upgrade().is_some());
        }
    }

    fn hover_reply(&self) {
        if let Some(message) = self.imp().message.upgrade() {
            let _ = self.activate_action("chat-history.reply", Some(&message.id().to_variant()));
        }
    }

    fn hover_react(&self) {
        let Some(message) = self.imp().message.upgrade() else {
            return;
        };
        let Some(reaction) = message.chat_().session_().default_reaction_type() else {
            return;
        };
        super::reactions::toggle_reaction(&message, reaction.0);
    }

    /// Refreshes the hover reaction button's glyph for `message`'s session default.
    fn update_hover_reaction(&self, message: &model::Message) {
        let imp = self.imp();
        let Some(button) = imp.hover_reaction_button.borrow().clone() else {
            return;
        };
        let session = message.chat_().session_();
        match session.default_reaction_type() {
            Some(reaction) => {
                let icon = super::reactions::reaction_icon_widget(
                    reaction.0,
                    &session,
                    16,
                    super::reactions::ReactionPresentation::Glyph,
                );
                button.set_child(Some(&icon));
                button.set_visible(true);
            }
            None => button.set_visible(false),
        }
    }

    pub(crate) fn update_from_message(&self, message: &model::Message, force_hide_sender: bool) {
        let imp = self.imp();

        imp.message.set(Some(message));
        self.update_hover_reaction(message);

        imp.indicators.set_message(message.upcast_ref());
        imp.reactions.set_message(Some(message.clone()));

        let is_channel = if let model::ChatType::Supergroup(data) = message.chat_().chat_type() {
            data.is_channel()
        } else {
            false
        };

        if message.is_outgoing() && !is_channel {
            self.add_css_class("outgoing");
        } else {
            self.remove_css_class("outgoing");
        }

        if let Some(binding) = imp.sender_binding.take() {
            binding.unwatch();
        }

        let show_sender = if force_hide_sender {
            None
        } else if message.chat_().is_own_chat() {
            if message.is_outgoing() {
                None
            } else {
                Some(message.forward_info().unwrap().origin().id())
            }
        } else if message.is_outgoing() {
            if matches!(message.sender(), model::MessageSender::Chat(_)) {
                Some(Some(message.sender().id()))
            } else {
                None
            }
        } else if matches!(
            message.chat_().chat_type(),
            model::ChatType::BasicGroup(_) | model::ChatType::Supergroup(_)
        ) {
            Some(Some(message.sender().id()))
        } else {
            None
        };

        // Handle MessageReply (both message and story replies)
        if message.reply_to().is_some() {
            let reply = ui::MessageReply::new(message);

            // FIXME: Do not show message reply when message is being deleted
            imp.message_reply_bin.set_child(Some(&reply));

            self.add_css_class("with-reply");
        } else {
            imp.message_reply_bin.set_child(gtk::Widget::NONE);

            self.remove_css_class("with-reply");
        }

        // Show sender label, if needed
        if let Some(maybe_id) = show_sender {
            let sender_name_expression = message.sender_display_name_expression();
            let sender_binding =
                sender_name_expression.bind(&*imp.sender_label, "label", glib::Object::NONE);
            imp.sender_binding.replace(Some(sender_binding));

            self.update_sender_color(maybe_id);

            imp.sender_label.set_visible(true);
        } else {
            if let Some(old_class) = imp.sender_color_class.take() {
                imp.sender_label.remove_css_class(&old_class);
            }

            imp.sender_label.set_label("");
            imp.sender_label.set_visible(false);
        }

        // Reactions were (re)bound above; place the indicators accordingly. Message types
        // that don't drive the bubble's `label` (polls, location, audio…) rely on this
        // call, while caption types re-run it from `set_label`.
        self.update_indicators_position();
    }

    pub(crate) fn update_from_sponsored_message(
        &self,
        sponsored_message: &model::SponsoredMessage,
    ) {
        let imp = self.imp();

        // Sponsored messages have no reply/reaction affordances; keep the hover pill dormant.
        imp.message.set(None);
        if let Some(button) = imp.hover_reaction_button.borrow().as_ref() {
            button.set_visible(false);
        }

        imp.indicators.set_message(sponsored_message.upcast_ref());

        // Bubbles are recycled between real and sponsored messages, so clear any
        // reactions/reply left over from a previously bound real message.
        imp.reactions.set_message(None::<model::Message>);
        imp.message_reply_bin.set_child(gtk::Widget::NONE);
        self.remove_css_class("with-reply");

        self.remove_css_class("outgoing");

        if let Some(binding) = imp.sender_binding.take() {
            binding.unwatch();
        }

        imp.sender_label
            .set_label(&sponsored_message.sponsor_label());

        let label_hash = hash_label(&sponsored_message.sponsor_label());

        self.update_sender_color(Some(label_hash));

        imp.sender_label.set_visible(true);
    }

    /// Hides the sender name, used for messages combined with an earlier one from the
    /// same sender (only the first message of a run keeps the name).
    pub(crate) fn hide_sender(&self) {
        self.imp().sender_label.set_visible(false);
    }

    /// Records this bubble's position within its same-sender run, which drives its drawn
    /// outline: the last bubble of a run (`is_group_end`) gets the tail; `is_group_start`
    /// controls whether the top corners are full or tightened.
    pub(crate) fn set_shape(&self, is_group_start: bool, is_group_end: bool) {
        let imp = self.imp();
        imp.group_start.set(is_group_start);
        imp.group_end.set(is_group_end);
        self.queue_draw();
    }

    pub(crate) fn set_prefix(&self, prefix: Option<&gtk::Widget>) {
        self.imp().prefix_bin.set_child(prefix);
    }

    pub(crate) fn set_label(&self, label: String) {
        let imp = self.imp();

        // The plain markup path (captions, sponsored/unsupported content) renders no
        // block widgets; clear any left over from a recycled rich text message.
        self.clear_blocks();
        imp.message_label.set_halign(gtk::Align::Fill);

        if label.is_empty() {
            imp.message_label.set_label(String::new());
            imp.message_label.set_visible(false);

            self.remove_css_class("with-label");
        } else {
            imp.message_label.set_label(label);
            imp.message_label.set_visible(true);

            self.add_css_class("with-label");
        }

        self.update_indicators_position();
    }

    /// Renders a message's full rich text: paragraphs, code blocks, and quotes as block
    /// widgets, with the trailing paragraph in the main label so the time indicators
    /// nest into its last line (as with plain text).
    pub(crate) fn set_formatted_text(
        &self,
        formatted: &tdlib::types::FormattedText,
        message: &model::Message,
    ) {
        let imp = self.imp();
        let session = message.chat_().session_();
        let spoiler_key = (message.chat_().id(), message.id());

        let mut blocks = utils::parse_rich_text(formatted);
        let tail = match blocks.last() {
            Some(utils::RichBlock::Paragraph(_)) => match blocks.pop() {
                Some(utils::RichBlock::Paragraph(inline)) => inline,
                _ => unreachable!(),
            },
            // Text ends in a code block / quote: the main label carries only the
            // indicators, right-aligned under the block.
            _ => utils::RichInline::default(),
        };

        self.clear_blocks();
        for block in blocks {
            imp.blocks_box
                .append(&super::blocks::block_widget(block, &session, spoiler_key));
        }
        imp.blocks_box
            .set_visible(imp.blocks_box.first_child().is_some());

        // With no trailing paragraph the label carries only the time indicators; align
        // it to the trailing edge so the date sits bottom-right under the last block.
        imp.message_label.set_halign(if tail.plain.is_empty() {
            gtk::Align::End
        } else {
            gtk::Align::Fill
        });
        imp.message_label
            .set_rich(tail, Some(session), Some(spoiler_key));
        imp.message_label.set_visible(true);
        self.add_css_class("with-label");

        self.update_indicators_position();
    }

    fn clear_blocks(&self) {
        let blocks_box = &self.imp().blocks_box;
        while let Some(child) = blocks_box.first_child() {
            blocks_box.remove(&child);
        }
        blocks_box.set_visible(false);
    }

    pub(crate) fn add_message_label_class(&self, class: &str) {
        self.imp().message_label.add_label_class(class);
    }

    pub(crate) fn set_suffix(&self, prefix: Option<&gtk::Widget>) {
        self.imp().suffix_bin.set_child(prefix);
    }

    fn update_sender_color(&self, sender_id: Option<i64>) {
        let imp = self.imp();

        if let Some(old_class) = imp.sender_color_class.take() {
            imp.sender_label.remove_css_class(&old_class);
        }

        let color_class = SENDER_COLOR_CLASSES[sender_id
            .map(|id| id as usize)
            .unwrap_or_else(|| hash_label(&imp.sender_label.label()) as usize)
            % SENDER_COLOR_CLASSES.len()];

        imp.sender_label.add_css_class(color_class);
        imp.sender_color_class.replace(Some(color_class.into()));
    }

    /// Places the message indicators in whichever slot suits the message's current
    /// content: on the reaction row when it has reactions (so the time sits bottom-right
    /// beside them), trailing the text when it has a caption/text, or overlaid on media
    /// otherwise. Idempotent — only reparents when the target slot actually changes.
    pub(crate) fn update_indicators_position(&self) {
        let imp = self.imp();

        // The reactions widget's own `visible` flag (set from its reaction count) — not the
        // ancestor-aware `is_visible()`, which is false while the row is still unmapped at
        // bind time and would misplace the indicators.
        let has_reactions = imp.reactions.get_visible();
        let target = if has_reactions {
            IndicatorPlacement::Reactions
        } else if imp.message_label.label().is_empty() {
            IndicatorPlacement::Overlay
        } else {
            IndicatorPlacement::Label
        };

        if imp.indicator_placement.get() == target {
            return;
        }

        match imp.indicator_placement.get() {
            IndicatorPlacement::Overlay => imp.overlay.remove_overlay(&*imp.indicators),
            IndicatorPlacement::Label => imp.message_label.set_indicators(None),
            IndicatorPlacement::Reactions => imp.reactions.set_indicators(None),
        }

        match target {
            IndicatorPlacement::Overlay => imp.overlay.add_overlay(&*imp.indicators),
            IndicatorPlacement::Label => imp
                .message_label
                .set_indicators(Some(imp.indicators.clone())),
            IndicatorPlacement::Reactions => imp.reactions.set_indicators(Some(&imp.indicators)),
        }

        imp.indicator_placement.set(target);
    }
}

/// A small flat, circular icon button for the hover-actions island.
fn hover_icon_button(icon_name: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .css_classes(["flat", "circular", "hover-action-button"])
        .valign(gtk::Align::Center)
        .build();
    if !icon_name.is_empty() {
        button.set_icon_name(icon_name);
    }
    button
}

fn hash_label(label: &str) -> i64 {
    let mut hasher = DefaultHasher::new();
    label.hash(&mut hasher);
    hasher.finish() as i64
}
