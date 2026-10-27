use std::cell::Cell;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::OnceLock;

use adw::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;
use gtk::gsk;
use gtk::subclass::prelude::*;

use crate::i18n::gettext_f;
use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

const MAX_WIDTH: i32 = 400;

/// Bubble corner radii: full for a standalone/outer corner, tight where a bubble abuts
/// its same-sender neighbour in a run.
const RADIUS: f32 = 15.0;
const RADIUS_TIGHT: f32 = 6.0;

/// Tail geometry (tunable). `START` is where on the bottom edge the tail begins,
/// `OVERHANG` how far past the bubble edge the tip reaches, `HEIGHT` how far up the
/// side it blends back in.
const TAIL_START: f32 = 9.0;
const TAIL_OVERHANG: f32 = 4.0;
const TAIL_HEIGHT: f32 = 12.0;

/// Where the message indicators (timestamp etc.) currently live. The template starts
/// them overlaid on the content (see `bubble.blp`), and they migrate as the message
/// gains a text label or reactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum IndicatorPlacement {
    /// Overlaid bottom-right on media / an empty-caption bubble.
    #[default]
    Overlay,
    /// Trailing the text inside the message label.
    Label,
    /// Bottom-right of the reaction row.
    Reactions,
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
        /// Watches `reply-markup` so a bot editing only its keyboard (content
        /// unchanged, no content notify) still rebuilds the buttons.
        pub(super) markup_watch: RefCell<Option<gtk::ExpressionWatch>>,
        pub(super) hover_pill: RefCell<Option<gtk::Box>>,
        pub(super) hover_reaction_button: RefCell<Option<gtk::Button>>,
        #[template_child]
        pub(super) box_: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) sender_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) forward_label: TemplateChild<gtk::Label>,
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
        #[template_child]
        pub(super) comments_bar: TemplateChild<ui::MessageCommentsBar>,
        #[template_child]
        pub(super) view_in_chat_bar: TemplateChild<ui::MessageViewInChatBar>,
        #[template_child]
        pub(super) keyboard_box: TemplateChild<gtk::Box>,
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
            let obj = self.obj();
            obj.setup_hover_actions();

            // Clicking "Forwarded from …" opens the origin chat/user.
            self.forward_label.set_cursor_from_name(Some("pointer"));
            let gesture = gtk::GestureClick::new();
            gesture.connect_released(clone!(
                #[weak]
                obj,
                move |gesture, _, _, _| {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    obj.open_forward_origin();
                }
            ));
            self.forward_label.add_controller(gesture);

            // Tapping the comments bar opens the post's discussion thread.
            self.comments_bar.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.open_thread()
            ));
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
        /// uniform 1px rim shadow on every side (so a light bubble — tail included — reads
        /// against a light background) and, in high-contrast, a hairline border.
        fn snapshot_shape(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f32;
            let h = obj.height() as f32;
            if w <= 0.0 || h <= 0.0 {
                return;
            }

            let outgoing = obj.has_css_class("outgoing");
            let tail = self.group_end.get();
            // Corners tighten where a bubble abuts its same-sender neighbour. Media
            // pictures follow along via the row's `grouped-*` CSS classes (see style.css);
            // map bubbles keep full corners to match their self-clipping map child.
            let map = obj.has_css_class("location") || obj.has_css_class("venue");
            let top = if map || self.group_start.get() {
                RADIUS
            } else {
                RADIUS_TIGHT
            };
            let bottom = if map || tail { RADIUS } else { RADIUS_TIGHT };

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

            // Native GSK path nodes (analytic anti-aliasing, like the theme's own rounded
            // rectangles) rather than a cairo texture — the latter fringes at the corners
            // when resampled on a fractional display scale.
            let path = bubble_path(w, h, top, bottom, outgoing, tail);
            let shadow = gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.1);

            // A 2px stroke centered on the outline; the fill covers its inner half, leaving
            // a uniform 1px rim on every side (tail included).
            snapshot.append_stroke(&path, &gsk::Stroke::new(2.0), &shadow);
            snapshot.append_fill(&path, gsk::FillRule::Winding, &color);

            if adw::StyleManager::default().is_high_contrast() {
                #[allow(deprecated)]
                let border = style
                    .lookup_color("borders")
                    .unwrap_or_else(|| gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.15));
                snapshot.append_stroke(&path, &gsk::Stroke::new(1.0), &border);
            }
        }
    }
}

/// Builds the bubble outline as a GSK path: a rounded rectangle with `top`/`bottom` corner
/// radii, except the sender-side bottom corner, which is replaced by a tail when `tail`
/// (bottom-left for incoming, bottom-right for outgoing). The tail is a smooth sweep
/// from ~`TAIL_HEIGHT` up the side down to a tip that overhangs the edge by
/// ~`TAIL_OVERHANG`, rejoining the bottom edge at `TAIL_START`.
fn bubble_path(w: f32, h: f32, top: f32, bottom: f32, outgoing: bool, tail: bool) -> gsk::Path {
    let b = gsk::PathBuilder::new();
    b.move_to(top, 0.0);
    b.line_to(w - top, 0.0);
    b.html_arc_to(w, 0.0, w, top, top); // top-right

    if outgoing && tail {
        // Right edge down to where the tail attaches, a smooth sweep out to the tip,
        // then back along the bottom edge — the mirror of the incoming sweep below.
        b.line_to(w, h - TAIL_HEIGHT);
        b.cubic_to(
            w,
            h - 8.0,
            w + TAIL_OVERHANG * 0.2,
            h - 5.0,
            w + TAIL_OVERHANG,
            h - 1.5,
        );
        b.cubic_to(
            w + TAIL_OVERHANG * 0.4,
            h,
            w - TAIL_START + 4.0,
            h,
            w - TAIL_START,
            h,
        );
    } else {
        b.line_to(w, h - bottom);
        b.html_arc_to(w, h, w - bottom, h, bottom); // bottom-right
    }

    if !outgoing && tail {
        // Bottom edge to the tail root, a smooth sweep out past the corner to the tip,
        // then back up to rejoin the left edge.
        b.line_to(TAIL_START, h);
        b.cubic_to(
            TAIL_START - 4.0,
            h,
            -TAIL_OVERHANG * 0.4,
            h,
            -TAIL_OVERHANG,
            h - 1.5,
        );
        b.cubic_to(
            -TAIL_OVERHANG * 0.2,
            h - 5.0,
            0.0,
            h - 8.0,
            0.0,
            h - TAIL_HEIGHT,
        );
    } else {
        b.line_to(bottom, h);
        b.html_arc_to(0.0, h, 0.0, h - bottom, bottom); // bottom-left
    }

    b.line_to(0.0, top);
    b.html_arc_to(0.0, 0.0, top, 0.0, top); // top-left
    b.close();
    b.to_path()
}

glib::wrapper! {
    pub(crate) struct MessageBubble(ObjectSubclass<imp::MessageBubble>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Opens a bot-provided URL in the system browser.
fn open_external_link(url: &str) {
    if url.is_empty() {
        return;
    }
    gtk::UriLauncher::new(url).launch(gtk::Window::NONE, gio::Cancellable::NONE, |_| {});
}

impl MessageBubble {
    /// Opens the chat/user a forwarded message originates from (hidden origins have
    /// nothing to open).
    fn open_forward_origin(&self) {
        let Some(message) = self.imp().message.upgrade() else {
            return;
        };
        let Some(info) = message.forward_info() else {
            return;
        };
        let Some(session) = utils::ancestor::<_, ui::Session>(self) else {
            return;
        };
        match info.origin() {
            model::MessageForwardOrigin::User(user) => session.select_chat_with_return(user.id()),
            model::MessageForwardOrigin::Chat { chat }
            | model::MessageForwardOrigin::Channel { chat } => {
                session.select_chat_with_return(chat.id())
            }
            _ => {}
        }
    }

    /// Opens the discussion thread of the bound channel post, if any.
    fn open_thread(&self) {
        let Some(message) = self.imp().message.upgrade() else {
            return;
        };
        if let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(self) {
            chat_history.open_thread(&message);
        }
    }

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
        let chat = message.chat_();
        let Some(reaction) = chat.session_().default_reaction_type() else {
            return;
        };
        if !reaction_allowed(&chat, &reaction.0) {
            return;
        }
        super::reactions::toggle_reaction(&message, reaction.0);
    }

    /// Refreshes the hover reaction button's glyph for `message`'s session default.
    fn update_hover_reaction(&self, message: &model::Message) {
        let imp = self.imp();
        let Some(button) = imp.hover_reaction_button.borrow().clone() else {
            return;
        };
        let chat = message.chat_();
        let session = chat.session_();
        match session.default_reaction_type() {
            Some(reaction) if reaction_allowed(&chat, &reaction.0) => {
                let icon = super::reactions::reaction_icon_widget(
                    reaction.0,
                    &session,
                    16,
                    super::reactions::ReactionPresentation::Glyph,
                );
                button.set_child(Some(&icon));
                button.set_visible(true);
            }
            _ => button.set_visible(false),
        }
    }

    pub(crate) fn update_from_message(&self, message: &model::Message, force_hide_sender: bool) {
        let imp = self.imp();

        imp.message.set(Some(message));
        if let Some(watch) = imp.markup_watch.borrow_mut().take() {
            watch.unwatch();
        }
        let watch = model::Message::this_expression("reply-markup").watch(
            Some(message),
            clone!(
                #[weak]
                message,
                #[weak(rename_to = bubble)]
                self,
                move || bubble.update_reply_markup(&message)
            ),
        );
        imp.markup_watch.replace(Some(watch));
        self.update_hover_reaction(message);

        imp.indicators.set_message(message.upcast_ref());
        imp.reactions.set_message(Some(message.clone()));
        imp.comments_bar.set_message(Some(message));
        imp.view_in_chat_bar.set_message(Some(message));
        self.update_reply_markup(message);

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

        // tdesktop-style "Forwarded from …" header. The user's own chat (Saved Messages)
        // shows the origin as the sender instead, so no header there.
        let forward_origin = message
            .forward_info()
            .filter(|_| !message.chat_().is_own_chat())
            .map(|info| info.origin());
        if let Some(origin) = forward_origin {
            let name = match &origin {
                model::MessageForwardOrigin::User(user) => strings::user_display_name(user, true),
                model::MessageForwardOrigin::Chat { chat }
                | model::MessageForwardOrigin::Channel { chat } => chat.title(),
                model::MessageForwardOrigin::HiddenUser { sender_name }
                | model::MessageForwardOrigin::MessageImport { sender_name } => sender_name.clone(),
            };
            self.imp()
                .forward_label
                .set_label(&gettext_f("Forwarded from {name}", &[("name", &name)]));
            self.imp().forward_label.set_visible(true);
            // Media pictures overlap the bubble padding with negative margins; this class
            // makes them yield below the header (same mechanism as `with-reply`).
            self.add_css_class("with-forward");
        } else {
            self.imp().forward_label.set_visible(false);
            self.remove_css_class("with-forward");
        }

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
        // reactions/reply/forward header left over from a previously bound real message.
        imp.reactions.set_message(None::<model::Message>);
        imp.comments_bar.set_message(None);
        imp.view_in_chat_bar.set_message(None);
        imp.message_reply_bin.set_child(gtk::Widget::NONE);
        self.remove_css_class("with-reply");
        imp.forward_label.set_visible(false);
        self.remove_css_class("with-forward");

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
        self.render_blocks(utils::parse_rich_text(formatted), message);
    }

    /// Renders a rich message (`MessageRichMessage`): the block list comes from
    /// the TDLib page-block tree instead of entity parsing, but shares the
    /// paragraph/code/quote/photo widgets.
    pub(crate) fn set_rich_blocks(
        &self,
        rich: &tdlib::types::RichMessage,
        message: &model::Message,
    ) {
        self.render_blocks(utils::rich_message_to_blocks(rich), message);
    }

    fn render_blocks(&self, mut blocks: Vec<utils::RichBlock>, message: &model::Message) {
        let imp = self.imp();
        let session = message.chat_().session_();
        let spoiler_key = (message.chat_().id(), message.id());

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

    /// Rebuilds the bot inline keyboard under the message text. The whole box
    /// is recreated on every bind and on markup edits (`reply-markup` notify),
    /// so button closures never outlive the message they were built for.
    fn update_reply_markup(&self, message: &model::Message) {
        let imp = self.imp();
        while let Some(child) = imp.keyboard_box.first_child() {
            imp.keyboard_box.remove(&child);
        }

        let Some(model::BoxedReplyMarkup(tdlib::enums::ReplyMarkup::InlineKeyboard(keyboard))) =
            message.reply_markup()
        else {
            imp.keyboard_box.set_visible(false);
            return;
        };

        for row in keyboard.rows {
            if row.is_empty() {
                continue;
            }
            let row_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .homogeneous(true)
                .build();
            for button in row {
                row_box.append(&self.keyboard_button(button, message));
            }
            imp.keyboard_box.append(&row_box);
        }
        imp.keyboard_box
            .set_visible(imp.keyboard_box.first_child().is_some());
    }

    fn keyboard_button(
        &self,
        button: tdlib::types::InlineKeyboardButton,
        message: &model::Message,
    ) -> gtk::Widget {
        use tdlib::enums::InlineKeyboardButtonType as T;

        let widget = gtk::Button::builder()
            .label(&button.text)
            .tooltip_text(&button.text)
            .css_classes(["inline-keyboard-button"])
            .build();

        let chat_id = message.chat_().id();
        let message_id = message.id();
        let session = message.chat_().session_();
        match button.r#type {
            T::Url(data) => {
                widget.connect_clicked(move |_| open_external_link(&data.url));
            }
            T::LoginUrl(data) => {
                widget.connect_clicked(move |_| open_external_link(&data.url));
            }
            T::WebApp(data) => {
                // A full web-app surface is not implemented; opening in the
                // browser keeps the button useful.
                widget.connect_clicked(move |_| open_external_link(&data.url));
            }
            T::Callback(data) => {
                let payload = tdlib::enums::CallbackQueryPayload::Data(
                    tdlib::types::CallbackQueryPayloadData { data: data.data },
                );
                widget.connect_clicked(move |button| {
                    let session = session.clone();
                    let button = button.clone();
                    let payload = payload.clone();
                    utils::spawn(async move {
                        let client_id = session.client_().id();
                        match tdlib::functions::get_callback_query_answer(
                            chat_id, message_id, payload, client_id,
                        )
                        .await
                        {
                            Ok(tdlib::enums::CallbackQueryAnswer::CallbackQueryAnswer(answer)) => {
                                if !answer.url.is_empty() {
                                    open_external_link(&answer.url);
                                }
                                if answer.show_alert && !answer.text.is_empty() {
                                    utils::show_toast(&button, answer.text);
                                }
                            }
                            Err(e) => {
                                log::warn!("Callback query failed: {e:?}");
                                utils::show_toast(
                                    &button,
                                    gettext("The button is no longer available"),
                                );
                            }
                        }
                    });
                });
            }
            T::CopyText(data) => {
                let text = data.text;
                widget.connect_clicked(move |button| {
                    button.clipboard().set_text(&text);
                    utils::show_toast(button, gettext("Copied to clipboard"));
                });
            }
            // Request-style, game, pay, password, switch-inline and user
            // buttons need flows this client does not implement yet: keep them
            // visible (bots compose them into one UI) but say so on press.
            _ => {
                widget.connect_clicked(move |button| {
                    utils::show_toast(
                        button,
                        gettext("This button is not supported by your version of Paper Plane."),
                    );
                });
            }
        }
        widget.upcast()
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
        // An empty label with block widgets above it (text ending in a code block or quote)
        // still owns the indicators — its reserved trailing gutter places the time
        // bottom-right under the last block. Only an empty label with no blocks (bare media)
        // overlays them on the picture instead.
        let target = if has_reactions {
            IndicatorPlacement::Reactions
        } else if imp.message_label.label().is_empty() && !imp.blocks_box.get_visible() {
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

/// Whether `reaction_type` may be used in `chat`. A chat with unrestricted (`All`) or
/// not-yet-loaded reactions permits everything; a restricted chat permits only its set.
fn reaction_allowed(chat: &model::Chat, reaction_type: &tdlib::enums::ReactionType) -> bool {
    match chat.available_reactions() {
        Some(tdlib::enums::ChatAvailableReactions::Some(some)) => {
            let key = model::reaction_type_key(reaction_type);
            some.reactions
                .iter()
                .any(|allowed| model::reaction_type_key(allowed) == key)
        }
        _ => true,
    }
}
