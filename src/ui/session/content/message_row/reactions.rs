use glib::clone;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
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
        pub(super) message: RefCell<Option<model::Message>>,
        pub(super) items_changed_handler: RefCell<Option<glib::SignalHandlerId>>,
        #[template_child]
        pub(super) flow_box: TemplateChild<gtk::FlowBox>,
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
            self.flow_box.unparent();
        }
    }

    impl WidgetImpl for MessageReactions {}

    impl MessageReactions {
        fn set_message(&self, message: Option<model::Message>) {
            let obj = self.obj();
            if *self.message.borrow() == message {
                return;
            }

            // Drop the previous items-changed subscription.
            if let Some(handler) = self.items_changed_handler.take()
                && let Some(old) = self.message.borrow().as_ref()
            {
                old.interaction_info().reactions().disconnect(handler);
            }

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
                    }
                ));
                obj.set_visible(reactions.n_items() > 0);
                self.items_changed_handler.replace(Some(handler));
            } else {
                self.flow_box
                    .bind_model(gio::ListModel::NONE, |_| gtk::Label::new(None).upcast());
                obj.set_visible(false);
            }

            self.message.replace(message);
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

            let obj = self.obj();
            button.connect_clicked(clone!(
                #[weak]
                obj,
                #[weak]
                reaction,
                move |_| {
                    if let Some(message) = obj.message() {
                        super::toggle_reaction(&message, &reaction);
                    }
                }
            ));

            button.upcast()
        }

        /// The leading widget of a chip: the resolved custom-emoji sticker for
        /// custom-emoji reactions, or a plain label (the emoji / a star) otherwise.
        fn create_emoji_widget(&self, reaction: &model::MessageReaction) -> gtk::Widget {
            let Some(message) = self.obj().message() else {
                return gtk::Label::new(Some(&reaction.label())).upcast();
            };
            let session = message.chat_().session_();
            super::reaction_icon_widget(reaction.reaction_type_(), &session, 18)
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageReactions(ObjectSubclass<imp::MessageReactions>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Builds the visual for a single reaction: an animated custom-emoji sticker for
/// `CustomEmoji` reactions (resolved asynchronously), or a plain label (the emoji
/// character, or a star for paid reactions) otherwise. `size` is the longer-side
/// size of the sticker, in pixels. Shared by the message-bubble reaction chips and
/// the context-menu reactions bar.
pub(crate) fn reaction_icon_widget(
    reaction_type: tdlib::enums::ReactionType,
    session: &model::ClientStateSession,
    size: i32,
) -> gtk::Widget {
    let custom_emoji_id = match reaction_type {
        tdlib::enums::ReactionType::Emoji(data) => {
            return gtk::Label::new(Some(&data.emoji)).upcast();
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

/// Toggles the current user's reaction: removes it if already chosen, otherwise
/// adds it.
fn toggle_reaction(message: &model::Message, reaction: &model::MessageReaction) {
    let chat = message.chat_();
    let client_id = chat.session_().client_().id();
    let chat_id = chat.id();
    let message_id = message.id();
    let reaction_type = reaction.reaction_type_();
    let is_chosen = reaction.is_chosen();

    utils::spawn(async move {
        let result = if is_chosen {
            tdlib::functions::remove_message_reaction(chat_id, message_id, reaction_type, client_id)
                .await
        } else {
            tdlib::functions::add_message_reaction(
                chat_id,
                message_id,
                reaction_type,
                false,
                true,
                client_id,
            )
            .await
        };

        if let Err(e) = result {
            log::warn!("Failed to toggle a message reaction: {e:?}");
        }
    });
}
