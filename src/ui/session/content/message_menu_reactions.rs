use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

/// How many quick reactions to offer before the "open full picker" chevron.
const MAX_QUICK_REACTIONS: usize = 7;
/// Longer-side size (px) of a reaction icon in the bar.
const REACTION_ICON_SIZE: i32 = 32;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct MessageMenuReactions {
        pub(super) message: RefCell<Option<model::Message>>,
        /// Bumped on every `load_for` so stale async availability responses (from a
        /// previously bound message) are discarded — the bar is a shared singleton
        /// re-parented per message.
        pub(super) generation: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageMenuReactions {
        const NAME: &'static str = "PaplMessageMenuReactions";
        type Type = super::MessageMenuReactions;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for MessageMenuReactions {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Horizontal);
            obj.set_spacing(2);
            obj.add_css_class("reaction-bar");
        }
    }

    impl WidgetImpl for MessageMenuReactions {}
    impl BoxImpl for MessageMenuReactions {}
}

glib::wrapper! {
    pub(crate) struct MessageMenuReactions(ObjectSubclass<imp::MessageMenuReactions>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl MessageMenuReactions {
    /// Populates the bar with quick reactions for `message`: the server-side
    /// favorite reaction first, then the chat's top and recent reactions, capped
    /// to [`MAX_QUICK_REACTIONS`], followed by a chevron opening the full picker.
    /// Hides itself entirely when the message can carry no reactions.
    pub(crate) fn load_for(&self, message: &model::Message) {
        let imp = self.imp();
        imp.message.replace(Some(message.clone()));

        let generation = imp.generation.get().wrapping_add(1);
        imp.generation.set(generation);

        while let Some(child) = self.first_child() {
            self.remove(&child);
        }

        let chat = message.chat_();
        let session = chat.session_();
        let client_id = session.client_().id();
        let chat_id = chat.id();
        let message_id = message.id();
        let favorite = session.default_reaction_type();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                let available = match tdlib::functions::get_message_available_reactions(
                    chat_id, message_id, 8, client_id,
                )
                .await
                {
                    Ok(tdlib::enums::AvailableReactions::AvailableReactions(available)) => {
                        available
                    }
                    Err(e) => {
                        log::warn!("Failed to get available reactions: {e:?}");
                        obj.set_visible(false);
                        return;
                    }
                };

                if obj.imp().generation.get() != generation {
                    return;
                }

                // Only offer the favorite reaction when the chat actually allows it:
                // either it is among the message's available reactions, or it is a custom
                // emoji and the chat permits those. Otherwise clicking it would issue an
                // addMessageReaction the server rejects.
                let allowed: HashSet<String> = available
                    .top_reactions
                    .iter()
                    .chain(available.recent_reactions.iter())
                    .map(|ar| model::reaction_type_key(&ar.r#type))
                    .collect();
                let favorite = favorite.map(|b| b.0).filter(|reaction_type| {
                    allowed.contains(&model::reaction_type_key(reaction_type))
                        || (available.allow_custom_emoji
                            && matches!(reaction_type, tdlib::enums::ReactionType::CustomEmoji(_)))
                });

                let mut seen: HashSet<String> = HashSet::new();
                let mut types: Vec<tdlib::enums::ReactionType> = Vec::new();
                let ordered = favorite
                    .into_iter()
                    .chain(available.top_reactions.into_iter().map(|ar| ar.r#type))
                    .chain(available.recent_reactions.into_iter().map(|ar| ar.r#type));
                for reaction_type in ordered {
                    if types.len() >= MAX_QUICK_REACTIONS {
                        break;
                    }
                    if seen.insert(model::reaction_type_key(&reaction_type)) {
                        types.push(reaction_type);
                    }
                }

                if types.is_empty() && !available.allow_custom_emoji {
                    obj.set_visible(false);
                    return;
                }
                obj.set_visible(true);

                for reaction_type in types {
                    let icon = super::message_row::reaction_icon_widget(
                        reaction_type.clone(),
                        &session,
                        REACTION_ICON_SIZE,
                        super::message_row::ReactionPresentation::Animated,
                    );
                    let button = gtk::Button::builder().child(&icon).build();
                    button.add_css_class("flat");
                    button.add_css_class("reaction-bar-item");
                    button.connect_clicked(clone!(
                        #[weak]
                        obj,
                        move |_| {
                            if let Some(message) = obj.imp().message.borrow().clone() {
                                super::message_row::toggle_reaction(&message, reaction_type.clone());
                            }
                            obj.close_menu();
                        }
                    ));
                    obj.append(&button);
                }

                let chevron = gtk::Button::from_icon_name("pan-down-symbolic");
                chevron.add_css_class("flat");
                chevron.add_css_class("reaction-bar-expand");
                chevron.connect_clicked(clone!(
                    #[weak]
                    obj,
                    move |_| obj.open_full_picker()
                ));
                obj.append(&chevron);
            }
        ));
    }

    /// Closes the enclosing context menu, if any.
    fn close_menu(&self) {
        if let Some(menu) = self
            .ancestor(ui::MessageMenu::static_type())
            .and_downcast::<ui::MessageMenu>()
        {
            menu.popdown();
        }
    }

    /// Dismisses the context menu and opens the full reaction picker (the content
    /// picker in reaction mode) anchored where the menu was.
    fn open_full_picker(&self) {
        let Some(message) = self.imp().message.borrow().clone() else {
            return;
        };
        let chat = message.chat_();
        let session = chat.session_();
        let client_id = session.client_().id();
        let chat_id = chat.id();
        let message_id = message.id();

        // Detach from the menu and re-anchor the picker exactly where the menu was,
        // so it opens in place rather than jumping.
        let menu = self
            .ancestor(ui::MessageMenu::static_type())
            .and_downcast::<ui::MessageMenu>();
        let placement = menu.as_ref().map(|menu| {
            (
                menu.parent(),
                menu.pointing_to(),
                menu.halign(),
                menu.position(),
            )
        });
        // Capture the history now (the picker popover detaches from the tree on close) and
        // hand off so the menu's close keeps the message spotlight during the picker.
        let chat_history = menu
            .as_ref()
            .and_then(|menu| utils::ancestor::<_, ui::ChatHistory>(menu));
        if let Some(menu) = &menu {
            menu.begin_picker_handoff();
            menu.popdown();
        }
        let Some((Some(anchor), pointing, halign, position)) = placement else {
            return;
        };
        // Set while returning to the menu via Back, so the picker's close keeps the
        // spotlight for the re-shown menu instead of clearing it.
        let going_back = Rc::new(Cell::new(false));

        // If the chat restricts reactions to a specific set, offer only those emoji;
        // otherwise (`All`) show the full keyboard + custom emoji.
        let allowed_emoji = match chat.available_reactions() {
            Some(tdlib::enums::ChatAvailableReactions::Some(some)) => Some(
                some.reactions
                    .into_iter()
                    .filter_map(|reaction| match reaction {
                        tdlib::enums::ReactionType::Emoji(emoji) => Some(emoji.emoji),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        };

        let picker: ui::ContentPicker = glib::Object::builder()
            .property("session", &session)
            .property("chat", &chat)
            .build();
        picker.set_reaction_mode(chat_id, message_id, allowed_emoji);
        picker.set_size_request(300, 360);
        // The `session` property is set after construction, so the emoji page built in
        // `constructed()` had no session and skipped custom-emoji packs. Refresh now
        // that the session is available so custom-emoji reactions appear (the sidebar
        // and compose popup rely on the same refresh).
        picker.refresh();

        // Reveal the picker with a downward "unfold" so it grows open from the
        // reaction bar's position instead of just appearing.
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SwingDown)
            .transition_duration(250)
            .reveal_child(false)
            .child(&picker)
            .build();

        let popover = gtk::Popover::builder().has_arrow(false).build();
        popover.add_css_class("reaction-picker-popover");
        popover.set_halign(halign);
        popover.set_position(position);
        popover.set_child(Some(&revealer));
        popover.set_parent(&anchor);
        if let (true, rect) = pointing {
            popover.set_pointing_to(Some(&rect));
        }
        popover.connect_closed(clone!(
            #[strong]
            going_back,
            #[strong]
            chat_history,
            move |popover| {
                popover.unparent();
                // Dismissed for good (not via Back): drop the message spotlight.
                if !going_back.replace(false)
                    && let Some(chat_history) = &chat_history
                {
                    chat_history.clear_menu_highlight();
                }
            }
        ));
        // Kick off the reveal once the popover is on screen.
        popover.connect_map(clone!(
            #[weak]
            revealer,
            move |_| revealer.set_reveal_child(true)
        ));

        // Back returns to the message menu it was opened from (still parented to the row).
        if let Some(menu) = menu {
            picker.add_reaction_back_button(clone!(
                #[weak]
                popover,
                #[strong]
                going_back,
                move || {
                    // Keep the spotlight for the menu we're returning to.
                    going_back.set(true);
                    popover.popdown();
                    menu.popup();
                }
            ));
        }

        picker.connect_content_picked(clone!(
            #[weak]
            popover,
            move |_| popover.popdown()
        ));
        picker.connect_reaction_picked(clone!(
            #[weak]
            popover,
            move |_, boxed| {
                let reaction_type = boxed.0;
                utils::spawn(async move {
                    if let Err(e) = tdlib::functions::add_message_reaction(
                        chat_id,
                        message_id,
                        reaction_type,
                        false,
                        true,
                        client_id,
                    )
                    .await
                    {
                        log::warn!("Failed to add a reaction from the picker: {e:?}");
                    }
                });
                popover.popdown();
            }
        ));

        popover.popup();
    }
}

