//! Builders for block-level message text: code blocks (`Pre`/`PreCode`) and block
//! quotes, rendered as real widgets between the bubble's text paragraphs
//! (tdesktop-style).

use gettextrs::gettext;
use glib::clone;
use gtk::glib;
use gtk::pango;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

/// Total visual lines an expandable quote is capped to while collapsed (tdesktop
/// collapses quotes to three lines).
const COLLAPSED_QUOTE_LINES: i32 = 3;

/// The widget for one parsed [`utils::RichBlock`].
pub(super) fn block_widget(
    block: utils::RichBlock,
    session: &model::ClientStateSession,
    spoiler_key: (i64, i64),
) -> gtk::Widget {
    match block {
        utils::RichBlock::Paragraph(inline) => {
            let label = ui::MessageLabel::new();
            label.set_rich(inline, Some(session.clone()), Some(spoiler_key));
            label.upcast()
        }
        utils::RichBlock::Code { code, language } => code_block(&code, &language),
        utils::RichBlock::Quote { inline, expandable } => {
            quote_block(inline, expandable, session, spoiler_key)
        }
    }
}

/// A tinted, rounded code block with a language tag and a copy button.
fn code_block(code: &str, language: &str) -> gtk::Widget {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["code-block"])
        .build();

    let header = gtk::Box::builder().spacing(6).build();
    let language_label = gtk::Label::builder()
        .label(language)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .css_classes(["code-language"])
        .build();
    header.append(&language_label);

    let copy_button = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text(gettext("Copy code"))
        .valign(gtk::Align::Start)
        .css_classes(["flat", "code-copy"])
        .build();
    let code_owned = code.to_string();
    copy_button.connect_clicked(move |button| {
        button.clipboard().set_text(&code_owned);
        utils::show_toast(button, gettext("Code copied to clipboard"));
    });
    header.append(&copy_button);
    root.append(&header);

    let code_label = gtk::Label::builder()
        .label(code)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .selectable(true)
        .css_classes(["code-text"])
        .build();
    root.append(&code_label);

    root.upcast()
}

/// An accent-tinted block quote with a leading bar; expandable quotes collapse to
/// [`COLLAPSED_QUOTE_LINES`] total lines behind a [`HeightClamp`] and toggle on click.
fn quote_block(
    inline: utils::RichInline,
    expandable: bool,
    session: &model::ClientStateSession,
    spoiler_key: (i64, i64),
) -> gtk::Widget {
    let root = gtk::Box::builder()
        .spacing(8)
        .css_classes(["quote-block"])
        .build();

    let bar = gtk::Box::builder().css_classes(["quote-bar"]).build();
    root.append(&bar);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    let label = ui::MessageLabel::new();
    label.set_rich(inline, Some(session.clone()), Some(spoiler_key));

    if !expandable {
        content.append(&label);
        root.append(&content);
        return root.upcast();
    }

    // The collapsed height: three text lines, from the label's font metrics.
    let metrics = label.pango_context().metrics(None, None);
    let line_height = (metrics.ascent() + metrics.descent()) / pango::SCALE;
    let collapsed_height = line_height * COLLAPSED_QUOTE_LINES;

    let clamp = HeightClamp::new(&label);
    clamp.set_max_height(collapsed_height);
    content.append(&clamp);

    let chevron = gtk::Image::builder()
        .icon_name("go-down-symbolic")
        .pixel_size(12)
        .halign(gtk::Align::End)
        .css_classes(["quote-chevron"])
        .build();
    content.append(&chevron);
    root.append(&content);

    // Short quotes have nothing to expand: drop the clamp and the chevron once the
    // label has a layout to count lines on.
    label.connect_map(clone!(
        #[weak]
        clamp,
        #[weak]
        chevron,
        move |label| {
            let label = label.clone();
            let clamp = clamp.clone();
            let chevron = chevron.clone();
            glib::idle_add_local_once(move || {
                if label.line_count() <= COLLAPSED_QUOTE_LINES {
                    clamp.set_max_height(-1);
                    chevron.set_visible(false);
                }
            });
        }
    ));

    let expanded = std::cell::Cell::new(false);
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(clone!(
        #[weak]
        clamp,
        #[weak]
        chevron,
        move |gesture, _, _, _| {
            if !chevron.is_visible() {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let now_expanded = !expanded.get();
            expanded.set(now_expanded);
            clamp.set_max_height(if now_expanded { -1 } else { collapsed_height });
            chevron.set_icon_name(Some(if now_expanded {
                "go-up-symbolic"
            } else {
                "go-down-symbolic"
            }));
        }
    ));
    root.add_controller(gesture);

    root.upcast()
}

mod imp_clamp {
    use super::*;

    /// Clamps its single child's height to `max_height` pixels (child overflow is
    /// clipped); `-1` removes the clamp. Used to collapse expandable quotes to a fixed
    /// number of text lines — `gtk::Label`'s own `lines` limit is per paragraph, so it
    /// cannot cap multi-paragraph quotes.
    #[derive(Debug, Default)]
    pub(crate) struct HeightClamp {
        pub(super) child: glib::WeakRef<gtk::Widget>,
        pub(super) max_height: std::cell::Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HeightClamp {
        const NAME: &'static str = "PaplHeightClamp";
        type Type = super::HeightClamp;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for HeightClamp {
        fn dispose(&self) {
            if let Some(child) = self.child.upgrade() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for HeightClamp {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.child.upgrade() else {
                return (0, 0, -1, -1);
            };
            let (min, nat, min_baseline, nat_baseline) = child.measure(orientation, for_size);
            let max = self.max_height.get();
            if orientation == gtk::Orientation::Vertical && max >= 0 {
                (min.min(max), nat.min(max), -1, -1)
            } else {
                (min, nat, min_baseline, nat_baseline)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.child.upgrade() else {
                return;
            };
            // Give the child its full natural height for this width; anything beyond
            // our own (clamped) height is clipped by `overflow: hidden`.
            let (_, natural, ..) = child.measure(gtk::Orientation::Vertical, width);
            child.allocate(width, natural.max(height), baseline, None);
        }

        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }
    }
}

glib::wrapper! {
    pub(crate) struct HeightClamp(ObjectSubclass<imp_clamp::HeightClamp>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl HeightClamp {
    fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let obj: Self = glib::Object::new();
        obj.set_overflow(gtk::Overflow::Hidden);
        child.as_ref().set_parent(&obj);
        obj.imp().child.set(Some(child.as_ref()));
        obj
    }

    fn set_max_height(&self, max_height: i32) {
        self.imp().max_height.set(max_height);
        self.queue_resize();
    }
}
