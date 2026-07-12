mod base;
mod bubble;
mod document;
mod indicators;
mod label;
mod location;
mod media_picture;
mod photo;
mod reactions;
mod reply;
mod sticker;
mod text;
mod venue;
mod video;

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;
use gtk::subclass::prelude::*;

pub(crate) use self::base::MessageBase;
pub(crate) use self::base::MessageBaseExt;
pub(crate) use self::base::MessageBaseImpl;
pub(crate) use self::bubble::MessageBubble;
pub(crate) use self::document::MessageDocument;
pub(crate) use self::document::StatusIndicator as MessageDocumentStatusIndicator;
pub(crate) use self::indicators::MessageIndicators;
pub(crate) use self::label::MessageLabel;
pub(crate) use self::location::MessageLocation;
pub(crate) use self::media_picture::MediaPicture;
pub(crate) use self::photo::MessagePhoto;
pub(crate) use self::reactions::MessageReactions;
pub(crate) use self::reply::MessageReply;
pub(crate) use self::sticker::MessageSticker;
pub(crate) use self::text::MessageText;
pub(crate) use self::venue::MessageVenue;
pub(crate) use self::video::MessageVideo;
use crate::model;
use crate::ui;
use crate::utils;

const AVATAR_SIZE: i32 = 32;
const SPACING: i32 = 6;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/mod.ui")]
    pub(crate) struct Row {
        /// A `model::Message` or `SponsoredMessage`
        pub(super) message: RefCell<Option<glib::Object>>,
        pub(super) content: RefCell<Option<gtk::Widget>>,
        pub(super) avatar: RefCell<Option<ui::Avatar>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "PaplMessageRow";
        type Type = super::Row;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();

            klass.install_action("message-row.react", None, move |widget, _, _| {
                widget.show_reaction_chooser()
            });
            klass.install_action("message-row.reply", None, move |widget, _, _| {
                widget.reply()
            });
            klass.install_action("message-row.edit", None, move |widget, _, _| widget.edit());
            klass.install_action("message-row.revoke-delete", None, move |widget, _, _| {
                widget.show_delete_dialog(true)
            });
            klass.install_action("message-row.delete", None, move |widget, _, _| {
                widget.show_delete_dialog(false)
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Row {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<glib::Object>("message")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "message" => obj.set_message(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "message" => obj.message().to_value(),
                _ => unimplemented!(),
            }
        }

        fn dispose(&self) {
            if let Some(avatar) = self.avatar.borrow().as_ref() {
                avatar.unparent();
            }

            if let Some(content) = self.content.borrow().as_ref() {
                content.unparent();
            }
        }
    }

    impl WidgetImpl for Row {}
}

glib::wrapper! {
    pub(crate) struct Row(ObjectSubclass<imp::Row>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

#[gtk::template_callbacks]
impl Row {
    pub(crate) fn new(message: &glib::Object) -> Self {
        let layout_manager = gtk::BoxLayout::builder().spacing(SPACING).build();
        glib::Object::builder()
            .property("layout-manager", layout_manager)
            .property("message", message)
            .build()
    }

    #[template_callback]
    fn on_released(&self, n_press: i32, _x: f64, _y: f64) {
        if n_press == 2 && self.can_reply_to_message() {
            self.reply();
        }
    }

    fn reply(&self) {
        if let Ok(message) = self.message().downcast::<model::Message>() {
            self.activate_action("chat-history.reply", Some(&message.id().to_variant()))
                .unwrap();
        }
    }

    /// Opens a popover offering the reactions available for this message; picking
    /// one adds it via `add_message_reaction`.
    fn show_reaction_chooser(&self) {
        let Ok(message) = self.message().downcast::<model::Message>() else {
            return;
        };
        let chat = message.chat_();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();
        let message_id = message.id();

        let flow_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .max_children_per_line(8)
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .max_content_height(220)
            .propagate_natural_height(true)
            .propagate_natural_width(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&flow_box)
            .build();
        let popover = gtk::Popover::builder().child(&scrolled).build();
        popover.set_parent(self);
        popover.connect_closed(|popover| popover.unparent());

        utils::spawn(clone!(
            #[weak]
            flow_box,
            #[weak]
            popover,
            async move {
                match tdlib::functions::get_message_available_reactions(
                    chat_id, message_id, 8, client_id,
                )
                .await
                {
                    Ok(tdlib::enums::AvailableReactions::AvailableReactions(available)) => {
                        for available_reaction in available.top_reactions {
                            if let tdlib::enums::ReactionType::Emoji(ref emoji) =
                                available_reaction.r#type
                            {
                                let button = gtk::Button::builder()
                                    .label(&emoji.emoji)
                                    .has_frame(false)
                                    .build();
                                let reaction_type = available_reaction.r#type.clone();
                                button.connect_clicked(clone!(
                                    #[weak]
                                    popover,
                                    move |_| {
                                        let reaction_type = reaction_type.clone();
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
                                                log::warn!("Failed to add a reaction: {e:?}");
                                            }
                                        });
                                        popover.popdown();
                                    }
                                ));
                                flow_box.append(&button);
                            }
                        }
                    }
                    Err(e) => log::warn!("Failed to get available reactions: {e:?}"),
                }
            }
        ));

        popover.popup();
    }

    fn edit(&self) {
        if let Ok(message) = self.message().downcast::<model::Message>() {
            self.activate_action("chat-history.edit", Some(&message.id().to_variant()))
                .unwrap();
        }
    }

    fn show_delete_dialog(&self, revoke: bool) {
        let message = if revoke {
            gettext("Do you want to delete this message for <b>everyone</b>?")
        } else {
            gettext("Do you want to delete this message?")
        };

        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Confirm Message Deletion"))
            .body_use_markup(true)
            .body(message)
            .build();

        dialog.add_responses(&[("no", &gettext("_No")), ("yes", &gettext("_Yes"))]);
        dialog.set_default_response(Some("no"));
        dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);

        dialog.choose(
            Some(self),
            gio::Cancellable::NONE,
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |response| {
                    if response == "yes"
                        && let Ok(message) = obj.message().downcast::<model::Message>()
                    {
                        utils::spawn(async move {
                            if let Err(e) = message.delete(revoke).await {
                                log::warn!(
                                    "Error deleting a message (revoke = {}): {:?}",
                                    revoke,
                                    e
                                );
                            }
                        });
                    }
                }
            ),
        );
    }

    pub(crate) fn message(&self) -> glib::Object {
        self.imp().message.borrow().clone().unwrap()
    }

    pub(crate) fn set_message(&self, message: glib::Object) {
        let imp = self.imp();

        if imp.message.borrow().as_ref() == Some(&message) {
            return;
        }

        if let Some(message) = message.downcast_ref::<model::Message>() {
            let show_avatar = if message.is_outgoing() {
                false
            } else if message.chat_().is_own_chat() {
                message.forward_info().is_some()
            } else {
                match message.chat_().chat_type() {
                    model::ChatType::BasicGroup(_) => true,
                    model::ChatType::Supergroup(supergroup) => !supergroup.is_channel(),
                    _ => false,
                }
            };

            if show_avatar {
                let avatar = {
                    let mut avatar_borrow = imp.avatar.borrow_mut();
                    if let Some(avatar) = avatar_borrow.clone() {
                        avatar
                    } else {
                        let avatar = ui::Avatar::new();
                        avatar.set_size(AVATAR_SIZE);
                        avatar.set_valign(gtk::Align::End);

                        // Insert at the beginning
                        avatar.insert_after(self, gtk::Widget::NONE);

                        *avatar_borrow = Some(avatar.clone());
                        avatar
                    }
                };

                if message.chat_().is_own_chat() {
                    match message.forward_info().unwrap().origin() {
                        model::MessageForwardOrigin::User(user) => {
                            avatar.set_custom_text(None);
                            avatar.set_item(Some(user.upcast()));
                        }
                        model::MessageForwardOrigin::Chat { chat, .. }
                        | model::MessageForwardOrigin::Channel { chat, .. } => {
                            avatar.set_custom_text(None);
                            avatar.set_item(Some(chat.upcast()));
                        }
                        model::MessageForwardOrigin::HiddenUser { sender_name }
                        | model::MessageForwardOrigin::MessageImport { sender_name } => {
                            avatar.set_item(None);
                            avatar.set_custom_text(Some(&sender_name));
                        }
                    }
                } else {
                    let avatar_item = match message.sender() {
                        model::MessageSender::User(user) => user.upcast(),
                        model::MessageSender::Chat(chat) => chat.upcast(),
                    };
                    avatar.set_custom_text(None);
                    avatar.set_item(Some(avatar_item));
                }
            } else {
                if let Some(avatar) = imp.avatar.borrow().as_ref() {
                    avatar.unparent();
                }
                imp.avatar.replace(None);
            }
        }

        self.update_content(message.clone());

        imp.message.replace(Some(message));

        // TODO: Update actions when needed (e.g. chat permissions change)
        self.update_actions();

        self.notify("message");
    }

    fn can_reply_to_message(&self) -> bool {
        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            can_send_messages_in_chat(&message.chat_())
        } else {
            false
        }
    }

    fn can_edit_message(&self) -> bool {
        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            let is_text_message = matches!(
                message.content().0,
                tdlib::enums::MessageContent::MessageText(_)
            );

            // TODO: Support more message types in the future
            is_text_message
                && message.can_be_edited()
                && can_send_messages_in_chat(&message.chat_())
        } else {
            false
        }
    }

    fn update_actions(&self) {
        self.action_set_enabled("message-row.reply", self.can_reply_to_message());
        self.action_set_enabled("message-row.edit", self.can_edit_message());

        if let Some(message) = self.message().downcast_ref::<model::Message>() {
            self.action_set_enabled("message-row.delete", message.can_be_deleted_only_for_self());
            self.action_set_enabled(
                "message-row.revoke-delete",
                message.can_be_deleted_for_all_users(),
            );
        } else {
            self.action_set_enabled("message-row.delete", false);
            self.action_set_enabled("message-row.revoke-delete", false);
        }
    }

    fn update_content(&self, message: glib::Object) {
        use tdlib::enums::MessageContent::*;

        let is_outgoing = if let Some(message_) = message.downcast_ref::<model::Message>() {
            // Do not mark channel messages as outgoing
            let is_outgoing = match message_.chat_().chat_type() {
                model::ChatType::Supergroup(data) if data.is_channel() => false,
                _ => message_.is_outgoing(),
            };

            match message_.content().0 {
                // Videos are display-only until clicked (issue #410): the full
                // file is downloaded on demand, not eagerly, so opening a chat
                // full of videos no longer floods the download queue.
                MessageAnimation(_) | MessageVideo(_) => {
                    self.update_specific_content::<_, ui::MessageVideo>(message_);
                }
                MessageAnimatedEmoji(data)
                    if data
                        .animated_emoji
                        .sticker
                        .clone()
                        .map(|s| {
                            matches!(
                                s.format,
                                tdlib::enums::StickerFormat::Webp
                                    | tdlib::enums::StickerFormat::Tgs
                            )
                        })
                        .unwrap_or_default() =>
                {
                    self.update_specific_content::<_, ui::MessageSticker>(message_);
                }
                MessageLocation(_) => {
                    self.update_specific_content::<_, ui::MessageLocation>(message_);
                }
                MessagePhoto(_) => {
                    self.update_specific_content::<_, ui::MessagePhoto>(message_);
                }
                MessageSticker(data)
                    if matches!(
                        data.sticker.format,
                        tdlib::enums::StickerFormat::Webp | tdlib::enums::StickerFormat::Tgs
                    ) =>
                {
                    self.update_specific_content::<_, ui::MessageSticker>(message_);
                }
                MessageDocument(_) => {
                    self.update_specific_content::<_, ui::MessageDocument>(message_);
                }
                MessageVenue(_) => {
                    self.update_specific_content::<_, ui::MessageVenue>(message_);
                }
                _ => {
                    self.update_specific_content::<_, ui::MessageText>(&message);
                }
            }

            is_outgoing
        } else {
            self.update_specific_content::<_, ui::MessageText>(&message);
            false
        };

        let content_ref = self.imp().content.borrow();
        let content = content_ref.as_ref().unwrap();

        if is_outgoing {
            content.set_halign(gtk::Align::End);
        } else {
            content.set_halign(gtk::Align::Start);
        }
    }

    fn update_specific_content<M, B>(&self, message: &M)
    where
        B: MessageBaseExt<Message = M>,
    {
        let mut content_ref = self.imp().content.borrow_mut();
        match content_ref.as_ref().and_then(|c| c.downcast_ref::<B>()) {
            Some(content) => {
                content.set_message(message);
            }
            None => {
                if let Some(old_content) = &*content_ref {
                    old_content.unparent();
                }

                let content = B::new(message);
                content.set_hexpand(true);
                content.set_valign(gtk::Align::Start);

                // Insert at the end
                content.insert_before(self, gtk::Widget::NONE);

                *content_ref = Some(content.upcast());
            }
        }
    }
}

fn can_send_messages_in_chat(chat: &model::Chat) -> bool {
    use tdlib::enums::ChatMemberStatus::*;
    let member_status = match chat.chat_type() {
        model::ChatType::Supergroup(supergroup) => Some(supergroup.status()),
        model::ChatType::BasicGroup(supergroup) => Some(supergroup.status()),
        _ => None,
    };
    member_status
        .map(|s| match s.0 {
            Creator(_) => true,
            Administrator(_) => true,
            Member(_) => chat.permissions().0.can_send_basic_messages,
            Restricted(data) => {
                chat.permissions().0.can_send_basic_messages
                    && data.permissions.can_send_basic_messages
            }
            Left => false,
            Banned(_) => false,
        })
        .unwrap_or(true)
}
