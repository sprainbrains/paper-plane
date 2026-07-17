use std::cell::Cell;
use std::cell::OnceCell;

use glib::Properties;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::MessageInteractionInfo)]
    pub(crate) struct MessageInteractionInfo {
        #[property(get)]
        pub(super) reply_count: Cell<u32>,
        #[property(get)]
        pub(super) view_count: Cell<i32>,
        pub(super) reactions: OnceCell<gio::ListStore>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageInteractionInfo {
        const NAME: &'static str = "MessageInteractionInfo";
        type Type = super::MessageInteractionInfo;
    }

    impl ObjectImpl for MessageInteractionInfo {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }
        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageInteractionInfo(ObjectSubclass<imp::MessageInteractionInfo>);
}

impl From<Option<tdlib::types::MessageInteractionInfo>> for MessageInteractionInfo {
    fn from(interaction_info: Option<tdlib::types::MessageInteractionInfo>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp()
            .reply_count
            .set(extract_reply_count(interaction_info.as_ref()));
        obj.imp()
            .view_count
            .set(extract_view_count(interaction_info.as_ref()));
        obj.set_reactions_from(interaction_info.as_ref());
        obj
    }
}

impl MessageInteractionInfo {
    pub(crate) fn update(&self, interaction_info: Option<tdlib::types::MessageInteractionInfo>) {
        self.set_reply_count(extract_reply_count(interaction_info.as_ref()));
        self.set_view_count(extract_view_count(interaction_info.as_ref()));
        self.set_reactions_from(interaction_info.as_ref());
    }

    fn set_view_count(&self, view_count: i32) {
        if self.view_count() == view_count {
            return;
        }
        self.imp().view_count.set(view_count);
        self.notify_view_count()
    }

    /// The message's reactions as a `gio::ListStore` of `model::MessageReaction`,
    /// suitable for binding to a widget via `bind_model`.
    pub(crate) fn reactions(&self) -> gio::ListStore {
        self.imp()
            .reactions
            .get_or_init(gio::ListStore::new::<model::MessageReaction>)
            .clone()
    }

    /// Optimistically flips the local user's reaction of `reaction_type` and returns
    /// whether it is now chosen (`true` = added, `false` = removed). Mirrors what the
    /// server will report, so the chip updates instantly; the next
    /// `updateMessageInteractionInfo` splices in the authoritative state.
    pub(crate) fn toggle_reaction_optimistic(
        &self,
        reaction_type: &tdlib::enums::ReactionType,
    ) -> bool {
        let reactions = self.reactions();
        let key = model::reaction_type_key(reaction_type);
        let existing = (0..reactions.n_items()).find_map(|i| {
            let reaction = reactions.item(i)?.downcast::<model::MessageReaction>().ok()?;
            (model::reaction_type_key(&reaction.reaction_type_()) == key).then_some((i, reaction))
        });

        // The chips bind to `items-changed`, not to per-item property notifications,
        // so an in-place count/chosen edit wouldn't refresh a chip. Splice a fresh
        // reaction in the slot instead, which rebuilds exactly that chip. Adds are
        // flagged `should_animate` so the rebuilt chip plays its emoji animation.
        match existing {
            Some((i, reaction)) if reaction.is_chosen() => {
                let count = reaction.total_count() - 1;
                if count <= 0 {
                    reactions.remove(i);
                } else {
                    let replacement =
                        model::MessageReaction::new(reaction_type.clone(), count, false, false);
                    reactions.splice(i, 1, &[replacement]);
                }
                false
            }
            Some((i, reaction)) => {
                let replacement = model::MessageReaction::new(
                    reaction_type.clone(),
                    reaction.total_count() + 1,
                    true,
                    true,
                );
                reactions.splice(i, 1, &[replacement]);
                true
            }
            None => {
                reactions.append(&model::MessageReaction::new(
                    reaction_type.clone(),
                    1,
                    true,
                    true,
                ));
                true
            }
        }
    }

    fn set_reactions_from(&self, interaction_info: Option<&tdlib::types::MessageInteractionInfo>) {
        let reactions = self.reactions();
        let new: Vec<model::MessageReaction> = interaction_info
            .and_then(|info| info.reactions.as_ref())
            .map(|r| {
                r.reactions
                    .iter()
                    .map(model::MessageReaction::from)
                    .collect()
            })
            .unwrap_or_default();

        // Skip the splice when nothing changed, so an optimistic (still-animating)
        // chip survives an identical server confirmation instead of being rebuilt
        // static mid-animation. Also spares a full rebuild on view-count-only updates.
        if reactions_unchanged(&reactions, &new) {
            return;
        }

        // Otherwise reactions changed wholesale, so replace the contents in one splice.
        reactions.splice(0, reactions.n_items(), &new);
    }

    fn set_reply_count(&self, reply_count: u32) {
        if self.reply_count() == reply_count {
            return;
        }
        self.imp().reply_count.set(reply_count);
        self.notify_reply_count()
    }
}

/// Whether the current reaction chips already match `new` by identity, count, and
/// chosen state — in which case a re-splice would only churn the widgets.
fn reactions_unchanged(store: &gio::ListStore, new: &[model::MessageReaction]) -> bool {
    if store.n_items() as usize != new.len() {
        return false;
    }
    new.iter().enumerate().all(|(i, new_reaction)| {
        store
            .item(i as u32)
            .and_downcast::<model::MessageReaction>()
            .is_some_and(|current| {
                model::reaction_type_key(&current.reaction_type_())
                    == model::reaction_type_key(&new_reaction.reaction_type_())
                    && current.total_count() == new_reaction.total_count()
                    && current.is_chosen() == new_reaction.is_chosen()
            })
    })
}

fn extract_reply_count(interaction_info: Option<&tdlib::types::MessageInteractionInfo>) -> u32 {
    interaction_info
        .and_then(|interaction_info| interaction_info.reply_info.as_ref())
        .map(|reply_info| reply_info.reply_count)
        .unwrap_or(0) as u32
}

fn extract_view_count(interaction_info: Option<&tdlib::types::MessageInteractionInfo>) -> i32 {
    interaction_info
        .map(|interaction_info| interaction_info.view_count)
        .unwrap_or(0)
}
