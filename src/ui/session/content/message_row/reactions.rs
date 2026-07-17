use adw::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use std::cell::RefCell;

    use glib::Properties;

    use super::*;

    #[derive(Debug, Default, CompositeTemplate, Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/reactions.ui")]
    #[properties(wrapper_type = super::MessageReactions)]
    pub(crate) struct MessageReactions {
        #[property(get, set = Self::set_message, explicit_notify, nullable)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) items_changed_handler: RefCell<Option<(gio::ListStore, glib::SignalHandlerId)>>,
        #[template_child]
        pub(super) flow_box: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub(super) indicators_bin: TemplateChild<adw::Bin>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageReactions {
        const NAME: &'static str = "PaplMessageReactions";
        type Type = super::MessageReactions;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageReactions {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }
        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }
        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn dispose(&self) {
            self.disconnect_items_changed_handler();
            self.flow_box.unparent();
            self.indicators_bin.unparent();
        }
    }

    impl WidgetImpl for MessageReactions {}

    impl MessageReactions {
        fn disconnect_items_changed_handler(&self) {
            if let Some((reactions, handler)) = self.items_changed_handler.take() {
                reactions.disconnect(handler);
            }
        }

        fn set_message(&self, message: Option<model::Message>) {
            let obj = self.obj();
            let old_message = self.message.upgrade();
            if old_message == message {
                return;
            }

            // Drop the previous items-changed subscription.
            self.disconnect_items_changed_handler();

            if let Some(ref message) = message {
                let reactions = message.interaction_info().reactions();

                self.flow_box.bind_model(
                    Some(&reactions),
                    clone!(
                        #[weak]
                        obj,
                        #[upgrade_or_else]
                        || gtk::Label::new(None).upcast(),
                        move |item| {
                            let reaction = item.downcast_ref::<model::MessageReaction>().unwrap();
                            obj.imp().create_chip(reaction)
                        }
                    ),
                );

                let handler = reactions.connect_items_changed(clone!(
                    #[weak]
                    obj,
                    move |store, _, _, _| {
                        obj.set_visible(store.n_items() > 0);
                        // Reactions appearing/disappearing moves the timestamp on/off the
                        // reaction row, so let the bubble re-place its indicators.
                        if let Some(bubble) = utils::ancestor::<_, ui::MessageBubble>(&obj) {
                            bubble.update_indicators_position();
                        }
                    }
                ));
                obj.set_visible(reactions.n_items() > 0);
                self.items_changed_handler
                    .replace(Some((reactions, handler)));
            } else {
                self.flow_box
                    .bind_model(gio::ListModel::NONE, |_| gtk::Label::new(None).upcast());
                obj.set_visible(false);
            }

            self.message.set(message.as_ref());
            obj.notify_message();
        }

        fn create_chip(&self, reaction: &model::MessageReaction) -> gtk::Widget {
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .build();
            content.append(&self.create_emoji_widget(reaction));
            content.append(&gtk::Label::new(Some(&reaction.total_count().to_string())));

            let button = gtk::Button::builder().child(&content).build();
            button.add_css_class("reaction-chip");
            if reaction.is_chosen() {
                button.add_css_class("chosen");
            }

            // Paid reactions use a separate star-accumulation API
            // (addPendingPaidMessageReaction), not add/removeMessageReaction; until that is
            // supported, leave the chip non-interactive rather than issue a doomed toggle.
            if matches!(reaction.reaction_type_(), tdlib::enums::ReactionType::Paid) {
                button.set_sensitive(false);
            } else {
                let obj = self.obj();
                button.connect_clicked(clone!(
                    #[weak]
                    obj,
                    #[weak]
                    reaction,
                    move |_| {
                        if let Some(message) = obj.message() {
                            super::toggle_reaction(&message, reaction.reaction_type_());
                        }
                    }
                ));
            }

            button.upcast()
        }

        /// The badge of a chip: a static Apple-style sticker for standard emoji (or a
        /// one-shot animation when the user just added this reaction), the animated
        /// sticker for custom emoji, or a star for paid reactions.
        fn create_emoji_widget(&self, reaction: &model::MessageReaction) -> gtk::Widget {
            let Some(message) = self.obj().message() else {
                return gtk::Label::new(Some(&reaction.label())).upcast();
            };
            let session = message.chat_().session_();
            let presentation = if reaction.take_should_animate() {
                super::ReactionPresentation::AnimatedOnce
            } else {
                super::ReactionPresentation::Static
            };
            super::reaction_icon_widget(reaction.reaction_type_(), &session, 18, presentation)
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageReactions(ObjectSubclass<imp::MessageReactions>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageReactions {
    /// Hosts (or clears) the message indicators in the trailing slot of the reaction
    /// row, so the timestamp lands in the bottom-right corner alongside the reactions.
    pub(crate) fn set_indicators(&self, indicators: Option<&ui::MessageIndicators>) {
        let bin = &self.imp().indicators_bin;
        bin.set_child(indicators.map(|indicators| indicators.upcast_ref::<gtk::Widget>()));
        bin.set_visible(indicators.is_some());
    }
}

/// How a reaction's emoji is rendered.
#[derive(Clone, Copy)]
pub(crate) enum ReactionPresentation {
    /// The resting badge on a message: a still icon.
    Static,
    /// The reaction picker: a small looping animation, like tdesktop's selector.
    Animated,
    /// A just-added message chip: plays the emoji animation once, then settles to
    /// the still icon.
    AnimatedOnce,
    /// A crisp font glyph — for a small control (the hover quick-react button) that
    /// starts hidden, where a lazily-loaded sticker would render blank then soft. Only
    /// takes effect for standard Unicode-emoji reactions; a custom-emoji default still
    /// lazy-loads a sticker on the hover button (there is no font glyph for it).
    Glyph,
}

/// Builds the visual for a single reaction: the standard emoji rendered as an
/// Apple-style sticker (still or looping per `presentation`), the animated sticker
/// for custom emoji, or a star for paid reactions. `size` is the longer-side size of
/// the sticker, in pixels. Shared by the message chips, hover button, and picker bar.
pub(crate) fn reaction_icon_widget(
    reaction_type: tdlib::enums::ReactionType,
    session: &model::ClientStateSession,
    size: i32,
    presentation: ReactionPresentation,
) -> gtk::Widget {
    let custom_emoji_id = match reaction_type {
        tdlib::enums::ReactionType::Emoji(data) => {
            return match presentation {
                ReactionPresentation::Glyph => gtk::Label::new(Some(&data.emoji)).upcast(),
                _ => emoji_reaction_widget(data.emoji, session, size, presentation),
            };
        }
        tdlib::enums::ReactionType::Paid => {
            return gtk::Label::new(Some("⭐")).upcast();
        }
        tdlib::enums::ReactionType::CustomEmoji(data) => data.custom_emoji_id,
    };

    let sticker: ui::Sticker = glib::Object::new();
    sticker.set_longer_side_size(size);

    let client_id = session.client_().id();
    utils::spawn(clone!(
        #[weak]
        sticker,
        #[weak]
        session,
        async move {
            if let Ok(tdlib::enums::Stickers::Stickers(result)) =
                tdlib::functions::get_custom_emoji_stickers(vec![custom_emoji_id], client_id).await
                && let Some(custom_emoji) = result.stickers.into_iter().next()
            {
                sticker.update_sticker(custom_emoji, true, session);
            }
        }
    ));

    sticker.upcast()
}

/// Builds the visual for a standard-emoji reaction: the emoji glyph as a
/// placeholder (shown instantly, avoiding an empty gap while resolving), swapped
/// for the reaction's Apple-style sticker once `getEmojiReaction` resolves. Per
/// `presentation`: the still `static_icon`, the looping `select_animation` (picker),
/// or a one-shot animation that settles back to the still icon (a just-added chip).
/// Falls back to the glyph if the emoji has no active reaction.
fn emoji_reaction_widget(
    emoji: String,
    session: &model::ClientStateSession,
    size: i32,
    presentation: ReactionPresentation,
) -> gtk::Widget {
    let bin = adw::Bin::builder()
        .child(&gtk::Label::new(Some(&emoji)))
        .build();

    utils::spawn(clone!(
        #[weak]
        bin,
        #[weak]
        session,
        async move {
            let Some(reaction) = session.resolve_emoji_reaction(emoji).await else {
                return;
            };
            let sticker: ui::Sticker = glib::Object::new();
            sticker.set_longer_side_size(size);

            match presentation {
                // `Glyph` is handled before this function (returns a plain label), so it
                // never reaches here; render the still icon if it somehow does.
                ReactionPresentation::Static | ReactionPresentation::Glyph => {
                    sticker.update_sticker(reaction.static_icon.clone(), false, session);
                }
                ReactionPresentation::Animated => {
                    sticker.update_sticker(reaction.select_animation.clone(), true, session);
                }
                ReactionPresentation::AnimatedOnce => {
                    // Play the select animation once, then swap to the still icon so it
                    // doesn't replay when the chip scrolls back into view.
                    let static_icon = reaction.static_icon.clone();
                    sticker.connect_animation_finished(clone!(
                        #[weak]
                        session,
                        move |sticker| {
                            sticker.update_sticker(static_icon.clone(), false, session);
                        }
                    ));
                    sticker.update_sticker(reaction.select_animation.clone(), false, session);
                }
            }
            bin.set_child(Some(&sticker));
        }
    ));

    bin.upcast()
}

/// Toggles the current user's reaction of `reaction_type` on `message`: it is added
/// if not currently chosen, otherwise removed. The local reaction list is updated
/// optimistically so the chip appears/disappears immediately; the authoritative
/// server state replaces it when `updateMessageInteractionInfo` arrives.
pub(crate) fn toggle_reaction(message: &model::Message, reaction_type: tdlib::enums::ReactionType) {
    let now_chosen = message
        .interaction_info()
        .toggle_reaction_optimistic(&reaction_type);

    let chat = message.chat_();
    let client_id = chat.session_().client_().id();
    let chat_id = chat.id();
    let message_id = message.id();

    utils::spawn(async move {
        let result = if now_chosen {
            tdlib::functions::add_message_reaction(
                chat_id,
                message_id,
                reaction_type,
                false,
                true,
                client_id,
            )
            .await
        } else {
            tdlib::functions::remove_message_reaction(chat_id, message_id, reaction_type, client_id)
                .await
        };

        if let Err(e) = result {
            log::warn!("Failed to toggle a message reaction: {e:?}");
        }
    });
}
