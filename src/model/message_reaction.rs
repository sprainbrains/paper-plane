use std::cell::Cell;
use std::cell::RefCell;

use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::MessageReaction)]
    pub(crate) struct MessageReaction {
        // Not a GObject property: a boxed TDLib enum only needed in Rust to
        // (un)set the reaction. Kept as a plain field.
        pub(super) reaction_type: RefCell<Option<model::BoxedReactionType>>,
        /// The text to display in the chip: the emoji itself for emoji
        /// reactions, a star for paid reactions, or a fallback glyph for
        /// (as-yet-unresolved) custom-emoji reactions.
        #[property(get, set)]
        pub(super) label: RefCell<String>,
        #[property(get, set)]
        pub(super) total_count: Cell<i32>,
        #[property(get, set)]
        pub(super) is_chosen: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageReaction {
        const NAME: &'static str = "MessageReaction";
        type Type = super::MessageReaction;
    }

    impl ObjectImpl for MessageReaction {
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
}

glib::wrapper! {
    pub(crate) struct MessageReaction(ObjectSubclass<imp::MessageReaction>);
}

impl From<&tdlib::types::MessageReaction> for MessageReaction {
    fn from(reaction: &tdlib::types::MessageReaction) -> Self {
        let obj: Self = glib::Object::builder()
            .property("label", reaction_label(&reaction.r#type))
            .property("total-count", reaction.total_count)
            .property("is-chosen", reaction.is_chosen)
            .build();
        obj.imp()
            .reaction_type
            .replace(Some(model::BoxedReactionType(reaction.r#type.clone())));
        obj
    }
}

impl MessageReaction {
    pub(crate) fn reaction_type_(&self) -> tdlib::enums::ReactionType {
        self.imp()
            .reaction_type
            .borrow()
            .as_ref()
            .unwrap()
            .0
            .clone()
    }
}

/// Returns the label to display for a reaction chip. Standard emoji reactions
/// carry the emoji directly; paid reactions show a star; custom-emoji reactions
/// fall back to a neutral glyph until the sticker is resolved.
fn reaction_label(reaction_type: &tdlib::enums::ReactionType) -> String {
    match reaction_type {
        tdlib::enums::ReactionType::Emoji(data) => data.emoji.clone(),
        tdlib::enums::ReactionType::Paid => "⭐".to_string(),
        // TODO: resolve custom_emoji_id -> Sticker via getCustomEmojiStickers
        // and render it animated in the chip.
        tdlib::enums::ReactionType::CustomEmoji(_) => "🙂".to_string(),
    }
}
