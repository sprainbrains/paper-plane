mod album;
mod audio;
mod base;
mod blocks;
mod bubble;
mod document;
mod indicators;
mod label;
mod location;
mod media_picture;
mod photo;
mod poll;
mod reactions;
mod reply;
mod sticker;
mod text;
mod venue;
mod video;
mod video_note;
mod voice_note;

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::graphene;
use gtk::subclass::prelude::*;

pub(crate) use self::album::MessageAlbum;
pub(crate) use self::audio::MessageAudio;
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
pub(crate) use self::poll::MessagePoll;
pub(crate) use self::reactions::MessageReactions;
pub(crate) use self::reactions::ReactionPresentation;
pub(crate) use self::reactions::reaction_icon_widget;
pub(crate) use self::reactions::toggle_reaction;
pub(crate) use self::reply::MessageReply;
pub(crate) use self::sticker::MessageSticker;
pub(crate) use self::text::MessageText;
pub(crate) use self::venue::MessageVenue;
pub(crate) use self::video::MessageVideo;
pub(crate) use self::video_note::MessageVideoNote;
pub(crate) use self::voice_note::MessageVoiceNote;
use crate::config;
use crate::model;
use crate::strings;
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
        /// The trailing selection checkbox, shown (empty or checked) for every message
        /// while in multi-select mode.
        pub(super) selection_check: RefCell<Option<gtk::CheckButton>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "PaplMessageRow";
        type Type = super::Row;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();

            klass.install_action("message-row.reply", None, move |widget, _, _| {
                widget.reply()
            });
            klass.install_action("message-row.forward", None, move |widget, _, _| {
                widget.show_forward()
            });
            klass.install_action("message-row.select", None, move |widget, _, _| {
                widget.begin_selection()
            });
            klass.install_action("message-row.copy", None, move |widget, _, _| {
                widget.copy_text()
            });
            klass.install_action("message-row.copy-link", None, move |widget, _, _| {
                widget.copy_link()
            });
            klass.install_action("message-row.copy-debug", None, move |widget, _, _| {
                widget.copy_debug_entry()
            });
            klass.install_action("message-row.pin", None, move |widget, _, _| {
                widget.pin_message()
            });
            klass.install_action("message-row.unpin", None, move |widget, _, _| {
                widget.unpin_message()
            });
            klass.install_action("message-row.edit", None, move |widget, _, _| widget.edit());
            klass.install_action(
                "message-row.show-sticker-pack",
                None,
                move |widget, _, _| widget.show_sticker_pack(),
            );
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
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_selection_gestures();
        }

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

            if let Some(check) = self.selection_check.borrow().as_ref() {
                check.unparent();
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
        if let Some(message) = self.primary_message() {
            self.activate_action("chat-history.reply", Some(&message.id().to_variant()))
                .unwrap();
        }
    }

    /// Pins this message in its chat.
    fn pin_message(&self) {
        let Some(message) = self.primary_message() else {
            return;
        };
        let chat = message.chat_();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();
        let message_id = message.id();

        let chat_history = utils::ancestor::<_, ui::ChatHistory>(self);
        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::pin_chat_message(chat_id, message_id, false, false, client_id)
                    .await
            {
                log::warn!("Failed to pin a message: {e:?}");
            } else if let Some(chat_history) = chat_history {
                chat_history.refresh_pinned_messages();
            }
        });
    }

    /// Unpins this message in its chat (for everyone).
    fn unpin_message(&self) {
        let Some(message) = self.primary_message() else {
            return;
        };
        let chat = message.chat_();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();
        let message_id = message.id();

        let chat_history = utils::ancestor::<_, ui::ChatHistory>(self);
        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::unpin_chat_message(chat_id, message_id, client_id).await
            {
                log::warn!("Failed to unpin a message: {e:?}");
            } else if let Some(chat_history) = chat_history {
                chat_history.refresh_pinned_messages();
            }
        });
    }

    /// Copies the message's text (or media caption) to the clipboard.
    fn copy_text(&self) {
        if let Some(message) = self.primary_message()
            && let Some(text) = copyable_text(&message.content().0)
        {
            self.clipboard().set_text(&text);
        }
    }

    fn can_copy_text(&self) -> bool {
        self.primary_message()
            .and_then(|message| copyable_text(&message.content().0))
            .is_some()
    }

    /// Copies a public/shareable link to this message (supergroups and channels).
    fn copy_link(&self) {
        let Some(message) = self.primary_message() else {
            return;
        };
        let chat = message.chat_();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();
        let message_id = message.id();
        let clipboard = self.clipboard();

        utils::spawn(async move {
            match tdlib::functions::get_message_link(
                chat_id,
                message_id,
                0,
                0,
                String::new(),
                false,
                false,
                client_id,
            )
            .await
            {
                Ok(tdlib::enums::MessageLink::MessageLink(data)) => clipboard.set_text(&data.link),
                Err(e) => log::warn!("Failed to get a message link: {e:?}"),
            }
        });
    }

    fn can_copy_link(&self) -> bool {
        self.primary_message()
            .map(|message| matches!(message.chat_().chat_type(), model::ChatType::Supergroup(_)))
            .unwrap_or(false)
    }

    /// Development builds only: copies everything the client knows about this
    /// message — its full `get_message` result, Debug-formatted — to the
    /// clipboard. Useful for bug reports, adding support for currently
    /// unsupported message types, and writing tests.
    fn copy_debug_entry(&self) {
        let Some(message) = self.primary_message() else {
            return;
        };
        let chat = message.chat_();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();
        let message_id = message.id();
        let clipboard = self.clipboard();

        utils::spawn(async move {
            let text = match tdlib::functions::get_message(chat_id, message_id, client_id).await {
                Ok(tdlib::enums::Message::Message(td_message)) => format!("{td_message:#?}"),
                Err(e) => format!("get_message({chat_id}, {message_id}) failed: {e:?}"),
            };
            clipboard.set_text(&text);
        });
    }

    /// Opens a chat picker to forward this message to another chat.
    fn show_forward(&self) {
        let Some(message) = self.primary_message() else {
            return;
        };
        let Some(session) = utils::ancestor::<_, ui::Session>(self) else {
            return;
        };
        let parent = self.root().and_downcast::<gtk::Window>();

        let window = ui::ForwardWindow::new(
            parent.as_ref(),
            &session,
            message.chat_().id(),
            // Forward an album as a whole, exactly like deletion.
            match self.message().downcast_ref::<model::ChatHistoryItem>() {
                Some(item) => item.messages().iter().map(|m| m.id()).collect(),
                None => vec![message.id()],
            },
        );
        window.present();
    }

    /// Whether the given `message-row.*` context-menu action applies to this row's
    /// message. The redesigned [`ui::MessageMenu`] renders custom icon rows instead
    /// of a `GMenuModel`, so it recomputes each row's visibility from this at popup
    /// time (replacing the model's `hidden-when: "action-disabled"`). Mirrors the
    /// enable logic in [`Self::update_actions`].
    pub(crate) fn menu_action_enabled(&self, action: &str) -> bool {
        match action {
            "message-row.reply" => self.can_reply_to_message(),
            "message-row.edit" => self.can_edit_message(),
            "message-row.copy" => self.can_copy_text(),
            "message-row.copy-link" => self.can_copy_link(),
            "message-row.copy-debug" => debug_options_enabled(),
            "message-row.show-sticker-pack" => self
                .primary_message()
                .is_some_and(|message| sticker_message_set_id(&message) != 0),
            "message-row.delete" => self
                .primary_message()
                .is_some_and(|m| m.can_be_deleted_only_for_self()),
            "message-row.revoke-delete" => self
                .primary_message()
                .is_some_and(|m| m.can_be_deleted_for_all_users()),
            // Only location/venue message rows install `message-row.open`.
            "message-row.open" => matches!(
                self.primary_message().map(|m| m.content().0),
                Some(
                    tdlib::enums::MessageContent::MessageLocation(_)
                        | tdlib::enums::MessageContent::MessageLiveLocation(_)
                        | tdlib::enums::MessageContent::MessageVenue(_)
                )
            ),
            // Forward and select are always offered for real messages.
            "message-row.forward" | "message-row.select" => self.primary_message().is_some(),
            // Pin and Unpin are mutually exclusive, following the message's live state.
            "message-row.pin" => self
                .primary_message()
                .is_some_and(|message| !message.is_pinned()),
            "message-row.unpin" => self
                .primary_message()
                .is_some_and(|message| message.is_pinned()),
            _ => true,
        }
    }

    fn edit(&self) {
        if let Some(message) = self.primary_message() {
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

        let dialog = ui::destructive_confirmation_dialog(
            &gettext("Confirm Message Deletion"),
            &message,
            ("no", &gettext("_No")),
            ("yes", &gettext("_Yes")),
        );
        dialog.set_body_use_markup(true);

        dialog.choose(
            Some(self),
            gio::Cancellable::NONE,
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |response| {
                    if response != "yes" {
                        return;
                    }
                    // An album is deleted as a whole: every member goes with it.
                    let ids: Vec<i64> = match obj.message().downcast_ref::<model::ChatHistoryItem>()
                    {
                        Some(item) => item.messages().iter().map(|m| m.id()).collect(),
                        None => match obj.primary_message() {
                            Some(message) => vec![message.id()],
                            None => return,
                        },
                    };
                    let Some(primary) = obj.primary_message() else {
                        return;
                    };
                    let chat = primary.chat_();
                    let chat_id = chat.id();
                    let client_id = chat.session_().client_().id();
                    utils::spawn(async move {
                        if let Err(e) =
                            tdlib::functions::delete_messages(chat_id, ids, revoke, client_id).await
                        {
                            log::warn!("Error deleting a message (revoke = {revoke}): {e:?}");
                        }
                    });
                }
            ),
        );
    }

    pub(crate) fn message(&self) -> glib::Object {
        self.imp().message.borrow().clone().unwrap()
    }

    /// The message this row acts on: plain messages themselves, albums through
    /// their primary (first, caption-carrying) member.
    fn primary_message(&self) -> Option<model::Message> {
        resolve_primary_message(&self.message())
    }

    /// Wires the per-message click gesture: once in selection mode, a primary click
    /// toggles this message. The selection *drag* lives on the stable list widget (see
    /// `ChatHistory`), not here, so scrolling can't recycle it out from under the drag.
    fn setup_selection_gestures(&self) {
        let click = gtk::GestureClick::new();
        click.set_button(1);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_released(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, _, x, y| {
                let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(&obj) else {
                    return;
                };
                let Some(message) = obj.primary_message() else {
                    return;
                };
                let shift = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::SHIFT_MASK);

                if chat_history.is_selection_mode() {
                    // Blank space beside the bubble is inert; only a click that actually
                    // lands on the bubble (or the trailing checkbox) affects selection.
                    if !obj.click_hits_message(x, y) {
                        return;
                    }
                    // Shift-click extends the range from the anchor; a plain click toggles
                    // this message (so clicking a checked message — or its checkbox —
                    // deselects it). The selection actions live on the right-click menu.
                    if shift {
                        chat_history.extend_selection_to(message.id());
                    } else {
                        chat_history.toggle_message_selection(message.id());
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                } else if shift {
                    // Shift-click on a message enters selection mode with it selected.
                    chat_history.select_message(message.id());
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                }
            }
        ));
        self.add_controller(click);

        // Drag messages out of the list. In selection mode (starting on an already-
        // selected message) the whole selection is dragged: an in-app drop on a sidebar
        // chat row forwards it, an external drop exports a .txt transcript plus any
        // downloaded media files. Outside selection mode, a drag starting on a message's
        // downloaded media picture drags that media: an in-app chat drop re-sends it
        // (copy, no forward header), an external drop saves the file. The list's
        // sweep-select stands down for exactly these cases (see `ChatHistory`), so this
        // never competes with drag-to-select.
        let drag_source = gtk::DragSource::new();
        drag_source.set_actions(gdk::DragAction::COPY);
        drag_source.connect_prepare(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            None,
            move |_, x, y| {
                let chat_history = utils::ancestor::<_, ui::ChatHistory>(&obj)?;
                let message = obj.message().downcast::<model::Message>().ok()?;

                if chat_history.is_selection_mode() {
                    if !chat_history.is_message_selected(message.id()) {
                        return None;
                    }
                    let payload = chat_history.message_drag_payload()?;
                    let mut providers = vec![gdk::ContentProvider::for_value(&payload.to_value())];
                    let files = chat_history.selected_export_files();
                    if !files.is_empty() {
                        let file_list = gdk::FileList::from_array(&files);
                        providers.push(gdk::ContentProvider::for_value(&file_list.to_value()));
                    }
                    return Some(gdk::ContentProvider::new_union(&providers));
                }

                // Single-media drag: only when the press landed on the media picture.
                let media = obj
                    .imp()
                    .content
                    .borrow()
                    .clone()
                    .and_then(|content| utils::descendant::<_, ui::MediaPicture>(&content))?;
                let bounds = media.compute_bounds(&obj)?;
                if !bounds.contains_point(&graphene::Point::new(x as f32, y as f32)) {
                    return None;
                }
                let path = utils::message_media_local_path(&message.content().0)?;
                let history = glib::WeakRef::new();
                history.set(Some(&chat_history));
                let payload = ui::MessageDragPayload {
                    from_chat_id: message.chat_().id(),
                    message_ids: vec![message.id()],
                    send_copy: true,
                    history,
                };
                let file_list = gdk::FileList::from_array(&[gio::File::for_path(path)]);
                Some(gdk::ContentProvider::new_union(&[
                    gdk::ContentProvider::for_value(&payload.to_value()),
                    gdk::ContentProvider::for_value(&file_list.to_value()),
                ]))
            }
        ));
        drag_source.connect_drag_begin(clone!(
            #[weak(rename_to = obj)]
            self,
            move |source, _| {
                source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&obj))), 0, 0);
            }
        ));
        self.add_controller(drag_source);

        // The trailing selection checkbox: a passive indicator (clicks fall through to
        // the row's gesture). Hidden until selection mode; repositioned to the end after
        // the content in `set_message`.
        let check = gtk::CheckButton::builder()
            .valign(gtk::Align::Center)
            .can_target(false)
            .can_focus(false)
            .visible(false)
            .css_classes(["selection-check"])
            .build();
        check.set_parent(self);
        self.imp().selection_check.replace(Some(check));
    }

    /// Whether a click at `(x, y)` (in this row's coordinates) landed on the message
    /// bubble or the trailing selection checkbox, as opposed to the blank space that
    /// flanks the bubble (which must not affect selection).
    fn click_hits_message(&self, x: f64, y: f64) -> bool {
        let point = graphene::Point::new(x as f32, y as f32);
        let imp = self.imp();
        [
            imp.content.borrow().clone(),
            imp.selection_check
                .borrow()
                .clone()
                .map(|check| check.upcast::<gtk::Widget>()),
        ]
        .into_iter()
        .flatten()
        .any(|widget| {
            widget
                .compute_bounds(self)
                .is_some_and(|bounds| bounds.contains_point(&point))
        })
    }

    /// Pops a menu with the selection actions at the click point — a right-click while in
    /// multi-select mode offers what the selection bar does, right at hand.
    pub(crate) fn show_selection_menu(&self, x: f64, y: f64) {
        let menu = gio::Menu::new();
        menu.append(
            Some(&gettext("Forward")),
            Some("chat-history.selection-forward"),
        );
        menu.append(
            Some(&gettext("Copy Text")),
            Some("chat-history.selection-copy"),
        );
        menu.append(
            Some(&gettext("Delete")),
            Some("chat-history.selection-delete"),
        );
        menu.append(
            Some(&gettext("Clear Selection")),
            Some("chat-history.selection-clear"),
        );

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_parent(self);
        popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        popover.connect_closed(|popover| {
            // Unparent once the close is fully processed, not from within the signal.
            glib::idle_add_local_once(clone!(
                #[weak]
                popover,
                move || popover.unparent()
            ));
        });
        popover.popup();
    }

    /// Reflects multi-select state on this row: shows the trailing checkbox while in
    /// selection mode and marks it checked when this message is selected.
    pub(crate) fn set_selection_state(&self, in_selection_mode: bool, selected: bool) {
        if let Some(check) = self.imp().selection_check.borrow().as_ref() {
            check.set_visible(in_selection_mode);
            check.set_active(selected);
        }
        // In multi-select mode a click must toggle the message, so caret text selection is
        // suspended on every label (a rich message has several); otherwise the bubble text
        // stays selectable/copyable.
        if let Some(content) = self.imp().content.borrow().as_ref() {
            utils::for_each_descendant::<_, ui::MessageLabel>(content, &mut |label| {
                label.set_text_selectable(!in_selection_mode);
            });
        }
    }

    /// Applies message-combining visuals for a run of consecutive same-sender messages:
    /// the avatar is kept only on the last (bottom) message, the sender name only on the
    /// first (top) one, and the shared corners between grouped bubbles are tightened.
    pub(crate) fn apply_grouping(&self, is_group_start: bool, is_group_end: bool) {
        // The avatar (valign: End) belongs on the last message of the run; other rows
        // keep it allocated but transparent so their bubbles stay aligned under it.
        if let Some(avatar) = self.imp().avatar.borrow().as_ref() {
            avatar.set_opacity(if is_group_end { 1.0 } else { 0.0 });
        }

        for class in [
            "grouped-top",
            "grouped-middle",
            "grouped-bottom",
            "group-start",
        ] {
            self.remove_css_class(class);
        }
        // The first message of a run (and standalone messages) gets extra top spacing to
        // separate groups; messages continuing a run sit tight beneath the previous one.
        if is_group_start {
            self.add_css_class("group-start");
        }
        if !(is_group_start && is_group_end) {
            self.add_css_class(if is_group_start {
                "grouped-top"
            } else if is_group_end {
                "grouped-bottom"
            } else {
                "grouped-middle"
            });
        }

        if let Some(content) = self.imp().content.borrow().as_ref()
            && let Some(bubble) = utils::descendant::<_, ui::MessageBubble>(content)
        {
            if !is_group_start {
                bubble.hide_sender();
            }
            // Feed the run position to the bubble so it can draw its outline: the last
            // message of a run (and standalone messages) gets the tail shape.
            bubble.set_shape(is_group_start, is_group_end);
        }
    }

    pub(crate) fn set_message(&self, message: glib::Object) {
        let imp = self.imp();

        if imp.message.borrow().as_ref() == Some(&message) {
            return;
        }

        if let Some(message) = resolve_primary_message(&message) {
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
                        // The avatar is taller than a single-line bubble; without this it
                        // stretches short rows, making intra-group spacing uneven (see the
                        // .message-avatar rule in style.css).
                        avatar.add_css_class("message-avatar");

                        // Clicking the avatar reveals the sender's profile in a popover.
                        avatar.set_cursor_from_name(Some("pointer"));
                        let gesture = gtk::GestureClick::new();
                        gesture.set_button(1);
                        gesture.connect_released(clone!(
                            #[weak]
                            avatar,
                            move |gesture, _, _, _| {
                                gesture.set_state(gtk::EventSequenceState::Claimed);
                                show_avatar_profile(&avatar);
                            }
                        ));
                        avatar.add_controller(gesture);

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

        // Keep the selection checkbox at the trailing edge, after the content.
        if let Some(check) = imp.selection_check.borrow().as_ref() {
            check.insert_before(self, gtk::Widget::NONE);
        }

        imp.message.replace(Some(message));

        // TODO: Update actions when needed (e.g. chat permissions change)
        self.update_actions();

        self.notify("message");
    }

    /// Enters multi-select mode with this message selected (from the message menu).
    fn begin_selection(&self) {
        if let Some(message) = self.primary_message()
            && let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(self)
        {
            chat_history.select_message(message.id());
        }
    }

    fn show_sticker_pack(&self) {
        let Some(message) = self.primary_message() else {
            return;
        };
        let set_id = sticker_message_set_id(&message);
        if set_id == 0 {
            return;
        }
        if let Some(session) = utils::ancestor::<_, ui::Session>(self) {
            session.show_sticker_pack(set_id);
        }
    }

    fn can_reply_to_message(&self) -> bool {
        if let Some(message) = self.primary_message() {
            can_send_messages_in_chat(&message.chat_())
        } else {
            false
        }
    }

    fn can_edit_message(&self) -> bool {
        if let Some(message) = self.primary_message() {
            use tdlib::enums::MessageContent::*;

            // Text messages edit their text; captioned media edit their caption.
            let content = message.content();
            let is_editable = matches!(content.0, MessageText(_))
                || super::captioned_media_caption(&content.0).is_some();

            is_editable && message.can_be_edited() && can_send_messages_in_chat(&message.chat_())
        } else {
            false
        }
    }

    fn update_actions(&self) {
        self.action_set_enabled("message-row.reply", self.can_reply_to_message());
        self.action_set_enabled("message-row.edit", self.can_edit_message());
        self.action_set_enabled("message-row.copy", self.can_copy_text());
        self.action_set_enabled("message-row.copy-link", self.can_copy_link());
        // The debug-entry action is gated behind the Debug Options preference (and
        // always available in development builds).
        self.action_set_enabled("message-row.copy-debug", debug_options_enabled());
        self.action_set_enabled(
            "message-row.show-sticker-pack",
            self.primary_message()
                .is_some_and(|message| sticker_message_set_id(&message) != 0),
        );

        if let Some(message) = self.primary_message() {
            self.action_set_enabled("message-row.delete", message.can_be_deleted_only_for_self());
            self.action_set_enabled(
                "message-row.revoke-delete",
                message.can_be_deleted_for_all_users(),
            );
            // Pin and Unpin are mutually exclusive, following the message's live state.
            self.action_set_enabled("message-row.pin", !message.is_pinned());
            self.action_set_enabled("message-row.unpin", message.is_pinned());
        } else {
            self.action_set_enabled("message-row.delete", false);
            self.action_set_enabled("message-row.revoke-delete", false);
            self.action_set_enabled("message-row.pin", false);
            self.action_set_enabled("message-row.unpin", false);
        }
    }

    fn update_content(&self, message: glib::Object) {
        use tdlib::enums::MessageContent::*;

        let is_outgoing = if let Some(item) = message.downcast_ref::<model::ChatHistoryItem>() {
            // An album item renders as one grid bubble holding all its members.
            self.update_specific_content::<model::ChatHistoryItem, ui::MessageAlbum>(item);
            match item.message() {
                Some(primary) => match primary.chat_().chat_type() {
                    // Do not mark channel messages as outgoing
                    model::ChatType::Supergroup(data) if data.is_channel() => false,
                    _ => primary.is_outgoing(),
                },
                None => false,
            }
        } else if let Some(message_) = message.downcast_ref::<model::Message>() {
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
                MessageLocation(_) | MessageLiveLocation(_) => {
                    self.update_specific_content::<_, ui::MessageLocation>(message_);
                }
                MessagePhoto(_) => {
                    self.update_specific_content::<_, ui::MessagePhoto>(message_);
                }
                MessagePoll(_) => {
                    self.update_specific_content::<_, ui::MessagePoll>(message_);
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
                MessageVoiceNote(_) => {
                    self.update_specific_content::<_, ui::MessageVoiceNote>(message_);
                }
                MessageAudio(_) => {
                    self.update_specific_content::<_, ui::MessageAudio>(message_);
                }
                MessageVideoNote(_) => {
                    self.update_specific_content::<_, ui::MessageVideoNote>(message_);
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

/// Shows a small popover with the sender's profile (avatar, name, username/status,
/// phone) anchored to the clicked message avatar, plus a shortcut to open the chat.
/// Non-user avatars (e.g. forwarded channels) are left alone.
fn show_avatar_profile(avatar: &ui::Avatar) {
    let Some(item) = avatar.item() else {
        return;
    };
    let Some(user) = item.downcast_ref::<model::User>() else {
        return;
    };

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .width_request(220)
        .build();

    let big_avatar = ui::Avatar::new();
    big_avatar.set_size(96);
    big_avatar.set_halign(gtk::Align::Center);
    big_avatar.set_item(Some(item.clone()));
    content.append(&big_avatar);

    content.append(&profile_label(
        &strings::user_display_name(user, true),
        "title-4",
    ));

    let username = user.username();
    if !username.is_empty() {
        content.append(&profile_label(&format!("@{username}"), "dim-label"));
    } else {
        content.append(&profile_label(
            &strings::user_status(&user.status().0),
            "dim-label",
        ));
    }
    let phone = user.phone_number();
    if !phone.is_empty() {
        content.append(&profile_label(&format!("+{phone}"), "dim-label"));
    }

    let popover = gtk::Popover::builder().child(&content).build();
    popover.connect_closed(|popover| popover.unparent());

    let user_id = user.id();
    let button = gtk::Button::builder()
        .label(gettext("Send Message"))
        .css_classes(["pill", "suggested-action"])
        .halign(gtk::Align::Center)
        .margin_top(6)
        .build();
    button.connect_clicked(clone!(
        #[weak]
        avatar,
        #[weak]
        popover,
        move |_| {
            if let Some(session) = utils::ancestor::<_, ui::Session>(&avatar) {
                session.select_chat_with_return(user_id);
            }
            popover.popdown();
        }
    ));
    content.append(&button);

    popover.set_parent(avatar);
    popover.popup();
}

fn profile_label(text: &str, css_class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes([css_class])
        .build()
}

/// Whether developer/debug message actions (e.g. "Copy Debug Entry") are available:
/// on in development builds, or when the user enables Debug Options in preferences.
fn debug_options_enabled() -> bool {
    // Called on the hot message-menu/action-update path, so reuse one Settings.
    thread_local! {
        static SETTINGS: gio::Settings = gio::Settings::new(config::APP_ID);
    }
    config::PROFILE == "Devel"
        || SETTINGS.with(|settings| settings.boolean("debug-options-enabled"))
}

/// The sticker set id of a sticker message, or 0 if the message is not a sticker or
/// the sticker belongs to no set.
fn sticker_message_set_id(message: &model::Message) -> i64 {
    if let tdlib::enums::MessageContent::MessageSticker(data) = message.content().0 {
        data.sticker.set_id
    } else {
        0
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

/// The message's plain text or media caption, if non-empty (for the copy action).
/// The message a row object acts on: plain messages themselves, album items
/// through their primary (first, caption-carrying) member.
fn resolve_primary_message(obj: &glib::Object) -> Option<model::Message> {
    if let Some(message) = obj.downcast_ref::<model::Message>() {
        return Some(message.clone());
    }
    obj.downcast_ref::<model::ChatHistoryItem>()?
        .message()
        .cloned()
}

fn copyable_text(content: &tdlib::enums::MessageContent) -> Option<String> {
    use tdlib::enums::MessageContent::*;

    let text = match content {
        MessageText(data) => &data.text.text,
        other => &super::captioned_media_caption(other)?.text,
    };

    (!text.is_empty()).then(|| text.clone())
}
