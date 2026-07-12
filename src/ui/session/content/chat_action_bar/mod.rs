use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::OnceLock;

use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::expressions;
use crate::i18n::gettext_f;
use crate::model;
use crate::strings;
use crate::types::MessageId;
use crate::ui;
use crate::utils;

mod composer_actions;

const PHOTO_MIME_TYPES: &[&str] = &["image/png", "image/jpeg"];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum ChatActionBarState {
    #[default]
    Composing,
    Replying(i64),
    Editing(i64),
}

impl ChatActionBarState {
    fn replying(&self) -> i64 {
        match self {
            Self::Replying(message_id) => *message_id,
            _ => 0,
        }
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_action_bar.ui")]
    pub(crate) struct ChatActionBar {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) chat_action_in_cooldown: Cell<bool>,
        pub(super) state: Cell<ChatActionBarState>,
        pub(super) chat_signal_group: OnceCell<glib::SignalGroup>,
        pub(super) basic_group_signal_group: OnceCell<glib::SignalGroup>,
        pub(super) supergroup_signal_group: OnceCell<glib::SignalGroup>,
        pub(super) bindings: RefCell<Vec<gtk::ExpressionWatch>>,
        #[template_child]
        pub(super) top_bar_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub(super) top_bar_image: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) top_bar_title_label: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) top_bar_message_label: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) message_entry: TemplateChild<ui::MessageEntry>,
        #[template_child]
        pub(super) send_message_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) select_file_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) restriction_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) mute_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) action_bar_stack: TemplateChild<gtk::Stack>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatActionBar {
        const NAME: &'static str = "PaplChatActionBar";
        type Type = super::ChatActionBar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action(
                "chat-action-bar.cancel-action",
                None,
                move |widget, _, _| {
                    widget.cancel_action();
                },
            );
            klass.install_action_async(
                "chat-action-bar.select-file",
                None,
                |widget, _, _| async move {
                    widget.select_file().await;
                },
            );
            klass.install_action_async(
                "chat-action-bar.send-message",
                None,
                |widget, _, _| async move {
                    if let ChatActionBarState::Editing(_) = widget.imp().state.get() {
                        widget.edit_message().await;
                    } else {
                        widget.send_text_message().await;
                    }
                },
            );
            klass.install_action_async(
                "chat-action-bar.join-chat",
                None,
                |widget, _, _| async move {
                    let chat = widget.chat().unwrap();
                    match tdlib::functions::join_chat(chat.id(), chat.session_().client_().id())
                        .await
                    {
                        Ok(_) => {
                            let sidebar = utils::ancestor::<_, ui::Sidebar>(&widget);
                            // Select chat recently joined by the user
                            sidebar.set_selected_chat(Some(&chat));
                        }
                        Err(e) => {
                            log::warn!("Failed to join chat: {e:?}");
                            utils::show_toast(
                                &widget,
                                gettext_f("Failed to join chat: {error}", &[("error", &e.message)]),
                            );
                        }
                    }
                },
            );
            klass.install_action_async(
                "chat-action-bar.toggle-mute",
                None,
                |widget, _, _| async move {
                    widget.toggle_mute().await;
                },
            );
            klass.install_action_async(
                "chat-action-bar.unblock-chat",
                None,
                |widget, _, _| async move {
                    let chat = widget.chat().unwrap();
                    if let model::ChatType::Private(user) = chat.chat_type() {
                        let message_sender =
                            tdlib::enums::MessageSender::User(tdlib::types::MessageSenderUser {
                                user_id: user.id(),
                            });
                        let result = tdlib::functions::set_message_sender_block_list(
                            message_sender,
                            None,
                            chat.session_().client_().id(),
                        )
                        .await;
                        if let Err(e) = result {
                            log::warn!("Failed to unblock user: {:?}", e);
                        }
                    }
                },
            )
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatActionBar {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "chat" => self.obj().set_chat(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "chat" => self.obj().chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.layout_manager()
                .and_then(|l| l.downcast::<gtk::BoxLayout>().ok())
                .unwrap()
                .set_orientation(gtk::Orientation::Vertical);

            self.message_entry.connect_formatted_text_notify(clone!(
                #[weak]
                obj,
                move |message_entry, _| {
                    // Enable the send-message action only when the message entry contains
                    // at least one non-whitespace character
                    let should_enable = message_entry
                        .formatted_text()
                        .map(|f| f.0.text.contains(|c: char| !c.is_whitespace()))
                        .unwrap_or_default();
                    obj.action_set_enabled("chat-action-bar.send-message", should_enable);

                    // Send typing action
                    utils::spawn(clone!(
                        #[weak]
                        obj,
                        async move {
                            obj.send_chat_action(tdlib::enums::ChatAction::Typing).await;
                        }
                    ));
                }
            ));

            self.message_entry.connect_paste_clipboard(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.handle_paste_action();
                }
            ));

            self.message_entry.connect_emoji_button_press(clone!(
                #[weak]
                obj,
                move |_, _| {
                    utils::ancestor::<_, ui::Session>(&obj).toggle_content_picker_sidebar();
                }
            ));
            self.message_entry.connect_emoji_button_hover(clone!(
                #[weak]
                obj,
                move |_, button| {
                    utils::ancestor::<_, ui::Session>(&obj).show_content_picker_popup(&button);
                }
            ));
            self.message_entry.connect_emoji_button_leave(clone!(
                #[weak]
                obj,
                move |_| {
                    utils::ancestor::<_, ui::Session>(&obj).schedule_content_picker_close();
                }
            ));

            // The message entry is always empty at this point, so disable the
            // send-message action
            obj.action_set_enabled("chat-action-bar.send-message", false);

            self.message_entry.connect_activate(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.activate_action("chat-action-bar.send-message", None)
                        .unwrap()
                }
            ));

            obj.create_signal_groups();
        }

        fn dispose(&self) {
            self.top_bar_revealer.unparent();
            self.action_bar_stack.unparent();
        }
    }

    impl WidgetImpl for ChatActionBar {}
}

glib::wrapper! {
    pub(crate) struct ChatActionBar(ObjectSubclass<imp::ChatActionBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ChatActionBar {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatActionBar {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn create_signal_groups(&self) {
        let imp = self.imp();

        let chat_signal_group = glib::SignalGroup::new::<model::Chat>();
        chat_signal_group.connect_notify_local(
            Some("notification-settings"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_stack_page();
                }
            ),
        );
        chat_signal_group.connect_notify_local(
            Some("block-list"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_stack_page();
                }
            ),
        );
        imp.chat_signal_group.set(chat_signal_group).unwrap();

        let basic_group_signal_group = glib::SignalGroup::new::<model::BasicGroup>();
        basic_group_signal_group.connect_notify_local(
            Some("status"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_stack_page();
                }
            ),
        );
        imp.basic_group_signal_group
            .set(basic_group_signal_group)
            .unwrap();

        let supergroup_signal_group = glib::SignalGroup::new::<model::Supergroup>();
        supergroup_signal_group.connect_notify_local(
            Some("status"),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, _| {
                    obj.update_stack_page();
                }
            ),
        );
        imp.supergroup_signal_group
            .set(supergroup_signal_group)
            .unwrap();
    }

    fn cancel_action(&self) {
        use ChatActionBarState::*;

        if let Editing(_) = self.imp().state.get() {
            // If we were editing, go back to the previous state by loading the
            // draft message if we have one, otherwise just reset everything
            // and go back to the "Composing" state.
            if let Some(draft_message) = self.chat().and_then(|c| c.draft_message()) {
                self.load_draft_message(draft_message);
            } else {
                self.reset();
            }
        } else {
            // We were probably replying, so just go back to "Composing" state
            self.set_state(Composing);
        }
    }

    fn set_state(&self, state: ChatActionBarState) {
        let imp = self.imp();
        if imp.state.get() == state {
            return;
        }

        // If we were editing, reset the message entry
        if let ChatActionBarState::Editing(_) = imp.state.get() {
            self.imp().message_entry.set_formatted_text(None);
        }

        // If the new state is "Editing", save the current
        // message composition state as draft message first
        if let ChatActionBarState::Editing(_) = state {
            utils::block_on(async move {
                self.save_message_as_draft().await;
            });
        }

        imp.state.set(state);

        self.update_top_bar();
        self.update_send_button();

        if let ChatActionBarState::Editing(message_id) = state {
            self.load_message_to_edit(message_id);
        }
    }

    fn update_top_bar(&self) {
        use ChatActionBarState::*;
        let imp = self.imp();

        match imp.state.get() {
            Composing => {
                imp.top_bar_title_label.set_text(None);
                imp.top_bar_message_label.set_text(None);
                imp.top_bar_revealer.set_reveal_child(false);
            }
            Replying(message_id) => {
                // TODO: Use TDLib to retrieve the message if we don't have it locally
                if let Some(message) = self.chat().and_then(|c| c.message(message_id)) {
                    // TODO: Make these labels auto update
                    imp.top_bar_title_label
                        .set_text(Some(&strings::message_sender(&message.sender(), true)));
                    imp.top_bar_message_label
                        .set_text(Some(&strings::message_content(&message)));
                } else {
                    imp.top_bar_title_label.set_text(Some(&gettext("Unknown")));
                    imp.top_bar_message_label
                        .set_text(Some(&gettext("Deleted Message")));
                }

                imp.top_bar_image
                    .set_icon_name(Some("mail-reply-sender-symbolic"));
                imp.top_bar_revealer.set_reveal_child(true);
            }
            Editing(message_id) => {
                // TODO: Use TDLib to retrieve the message if we don't have it locally
                if let Some(message) = self.chat().and_then(|c| c.message(message_id)) {
                    imp.top_bar_title_label
                        .set_text(Some(&gettext("Edit Message")));
                    imp.top_bar_message_label
                        .set_text(Some(&strings::message_content(&message)));
                } else {
                    imp.top_bar_title_label.set_text(Some(&gettext("Unknown")));
                    imp.top_bar_message_label
                        .set_text(Some(&gettext("Deleted Message")));
                }

                imp.top_bar_image.set_icon_name(Some("edit-symbolic"));
                imp.top_bar_revealer.set_reveal_child(true);
            }
        }
    }

    fn update_send_button(&self) {
        use ChatActionBarState::*;
        let imp = self.imp();

        match imp.state.get() {
            Editing(_) => {
                imp.send_message_button.set_icon_name("done-symbolic");
            }
            _ => {
                imp.send_message_button.set_icon_name("go-up-symbolic");
            }
        }
    }

    fn load_message_to_edit(&self, id: MessageId) {
        if let Some(chat) = self.chat() {
            let client_id = chat.session_().client_().id();

            if let Some(message) = chat.message(id) {
                let text = match message.content().0 {
                    tdlib::enums::MessageContent::MessageText(data) => Some(data.text),
                    other => message_caption(other),
                };

                if let Some(text) = text {
                    utils::block_on(async move {
                        let tdlib::enums::FormattedText::FormattedText(markdown_text) =
                            tdlib::functions::get_markdown_text(text, client_id)
                                .await
                                .unwrap();

                        self.imp()
                            .message_entry
                            .set_formatted_text(Some(model::BoxedFormattedText(markdown_text)));
                    });
                }
            }
        }
    }

    fn reset(&self) {
        self.set_state(ChatActionBarState::Composing);
        self.imp().message_entry.set_formatted_text(None);
    }

    async fn compose_text_message(&self) -> Option<tdlib::enums::InputMessageContent> {
        if let Some(formatted_text) = self.imp().message_entry.as_markdown().await {
            let content = tdlib::types::InputMessageText {
                text: formatted_text,
                link_preview_options: None,
                clear_draft: true,
            };

            Some(tdlib::enums::InputMessageContent::InputMessageText(content))
        } else {
            None
        }
    }

    pub(crate) fn insert_emoji(&self, emoji: &str) {
        self.imp().message_entry.insert_at_cursor(emoji);
        self.imp().message_entry.grab_focus();
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        self.imp()
            .message_entry
            .insert_custom_emoji(emoji, custom_emoji_id);
        self.imp().message_entry.grab_focus();
    }

    async fn select_file(&self) {
        let dialog = gtk::FileDialog::new();
        let filter = gtk::FileFilter::new();
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        let parent = self.root().and_downcast::<gtk::Window>().unwrap();

        filter.set_name(Some(&gettext("Images")));
        for mime in PHOTO_MIME_TYPES {
            filter.add_mime_type(mime);
        }

        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        if let Ok(file) = dialog.open_future(Some(&parent)).await {
            let path = file.path().unwrap().to_str().unwrap().to_string();
            let chat = self.chat().unwrap();

            ui::SendMediaWindow::new(&parent, &chat, path, self.imp().state.get().replying())
                .present();
        }
    }

    async fn edit_message(&self) {
        if let Some(chat) = self.chat()
            && let ChatActionBarState::Editing(message_id) = self.imp().state.get()
        {
            let client_id = chat.session_().client_().id();
            let chat_id = chat.id();

            // Media messages edit their caption; everything else edits its text.
            let is_caption = chat
                .message(message_id)
                .map(|message| {
                    !matches!(
                        message.content().0,
                        tdlib::enums::MessageContent::MessageText(_)
                    )
                })
                .unwrap_or(false);

            let result = if is_caption {
                let caption = self.imp().message_entry.as_markdown().await;
                tdlib::functions::edit_message_caption(
                    chat_id, message_id, caption, false, client_id,
                )
                .await
            } else if let Some(message) = self.compose_text_message().await {
                tdlib::functions::edit_message_text(chat_id, message_id, message, client_id).await
            } else {
                return;
            };

            if let Err(e) = result {
                log::warn!("Error editing a message: {:?}", e);
            }

            self.cancel_action();
        }
    }

    async fn send_text_message(&self) {
        if let Some(chat) = self.chat()
            && let Some(message) = self.compose_text_message().await
        {
            let client_id = chat.session_().client_().id();
            let chat_id = chat.id();
            let message_id = match self.imp().state.get() {
                ChatActionBarState::Replying(id) => id,
                _ => 0,
            };
            let reply_to = if message_id != 0 {
                Some(tdlib::enums::InputMessageReplyTo::Message(
                    tdlib::types::InputMessageReplyToMessage {
                        message_id,
                        ..Default::default()
                    },
                ))
            } else {
                None
            };

            // Send the message
            let result =
                tdlib::functions::send_message(chat_id, None, reply_to, None, message, client_id)
                    .await;
            if let Err(e) = result {
                log::warn!("Error sending a message: {:?}", e);
            }

            self.reset();
        }
    }

    fn is_chat_muted(&self) -> bool {
        let chat = self.chat().unwrap();
        let notifications = chat.notification_settings().0;
        if notifications.use_default_mute_for {
            chat.session_()
                .channel_chats_notification_settings()
                .0
                .mute_for
                != 0
        } else {
            notifications.mute_for != 0
        }
    }

    async fn toggle_mute(&self) {
        let chat = self.chat().unwrap();
        let mut notifications = chat.clone().notification_settings().0;
        let default = chat.session_().channel_chats_notification_settings().0;
        if default.mute_for == notifications.mute_for {
            notifications.use_default_mute_for = false;
            if notifications.mute_for == 0 {
                let now = glib::DateTime::now_utc().unwrap().to_unix() as i32;
                notifications.mute_for = i32::MAX - now
            } else {
                notifications.mute_for = 0
            }
        } else {
            notifications.use_default_mute_for = true;
        }

        let result = tdlib::functions::set_chat_notification_settings(
            chat.id(),
            notifications.clone(),
            chat.session_().client_().id(),
        )
        .await;
        if let Err(e) = result {
            log::warn!("Failed to unmute/mute chat: {:?}", e);
        }
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    fn set_chat(&self, chat: Option<&model::Chat>) {
        if self.chat().as_ref() == chat {
            return;
        }

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                obj.save_message_as_draft().await;
            }
        ));

        let imp = self.imp();
        let mut bindings = imp.bindings.borrow_mut();
        while let Some(binding) = bindings.pop() {
            binding.unwatch();
        }

        if let Some(chat) = chat {
            if let Some(draft_message) = chat.draft_message() {
                self.load_draft_message(draft_message);
            } else {
                self.reset();
            }

            imp.chat_action_in_cooldown.set(false);

            // Handle restriction_label caption
            let restriction_label_binding = expressions::restriction_expression(chat).bind(
                &*imp.restriction_label,
                "label",
                Some(chat),
            );

            bindings.push(restriction_label_binding);
        }

        imp.chat.set(chat);

        self.update_stack_page();
        self.update_signal_groups();

        self.notify("chat");
    }

    pub(crate) fn reply_to_message_id(&self, id: MessageId) {
        self.set_state(ChatActionBarState::Replying(id));
    }

    pub(crate) fn edit_message_id(&self, id: MessageId) {
        self.set_state(ChatActionBarState::Editing(id));
    }

    fn update_stack_page(&self) {
        if let Some(chat) = self.chat() {
            let imp = self.imp();

            match chat.chat_type() {
                model::ChatType::Private(user) => {
                    let is_deleted = matches!(user.user_type().0, tdlib::enums::UserType::Deleted);
                    let is_blocked = chat.is_blocked();
                    if is_deleted {
                        // TODO: Add delete chat button
                        imp.action_bar_stack.set_visible_child_name("entry");
                    } else if is_blocked {
                        imp.action_bar_stack.set_visible_child_name("unblock");
                    } else {
                        imp.action_bar_stack.set_visible_child_name("entry");
                    }
                }
                model::ChatType::Secret(secret) => {
                    let is_closed = matches!(secret.state(), model::SecretChatState::Closed);
                    let is_blocked = chat.is_blocked();
                    if is_closed {
                        // TODO: Add delete chat button
                        imp.action_bar_stack.set_visible_child_name("entry");
                    } else if is_blocked {
                        imp.action_bar_stack.set_visible_child_name("unblock");
                    } else {
                        imp.action_bar_stack.set_visible_child_name("entry");
                    }
                }
                model::ChatType::Supergroup(data) if data.is_channel() => match data.status().0 {
                    tdlib::enums::ChatMemberStatus::Creator(_)
                    | tdlib::enums::ChatMemberStatus::Administrator(_) => {
                        imp.action_bar_stack.set_visible_child_name("entry");
                    }
                    tdlib::enums::ChatMemberStatus::Left => {
                        imp.action_bar_stack.set_visible_child_name("join");
                    }
                    _ => {
                        imp.action_bar_stack.set_visible_child_name("mute");

                        if self.is_chat_muted() {
                            imp.mute_button.set_label(&gettext("Unmute"));
                        } else {
                            imp.mute_button.set_label(&gettext("Mute"));
                        }
                    }
                },
                model::ChatType::Supergroup(data) => match data.status().0 {
                    tdlib::enums::ChatMemberStatus::Restricted(data)
                        if !data.permissions.can_send_basic_messages =>
                    {
                        imp.action_bar_stack.set_visible_child_name("restricted");
                    }
                    tdlib::enums::ChatMemberStatus::Left => {
                        imp.action_bar_stack.set_visible_child_name("join");
                    }
                    _ => {
                        imp.action_bar_stack.set_visible_child_name("entry");
                    }
                },
                model::ChatType::BasicGroup(data) => match data.status().0 {
                    tdlib::enums::ChatMemberStatus::Restricted(_) => {
                        imp.action_bar_stack.set_visible_child_name("restricted");
                    }
                    tdlib::enums::ChatMemberStatus::Left => {
                        imp.action_bar_stack.set_visible_child_name("join");
                    }
                    _ => {
                        imp.action_bar_stack.set_visible_child_name("entry");
                    }
                },
            }
        }
    }

    fn update_signal_groups(&self) {
        let imp = self.imp();

        let chat = self.chat();
        imp.chat_signal_group
            .get()
            .unwrap()
            .set_target(chat.as_ref());

        let basic_group = chat
            .as_ref()
            .and_then(|c| c.chat_type().basic_group().cloned());
        imp.basic_group_signal_group
            .get()
            .unwrap()
            .set_target(basic_group.as_ref());

        let supergroup = chat.and_then(|c| c.chat_type().supergroup().cloned());
        imp.supergroup_signal_group
            .get()
            .unwrap()
            .set_target(supergroup.as_ref());
    }
}

/// Extracts the editable caption from a captioned media message, if any.
fn message_caption(content: tdlib::enums::MessageContent) -> Option<tdlib::types::FormattedText> {
    use tdlib::enums::MessageContent::*;

    match content {
        MessagePhoto(data) => Some(data.caption),
        MessageVideo(data) => Some(data.caption),
        MessageAnimation(data) => Some(data.caption),
        MessageDocument(data) => Some(data.caption),
        MessageAudio(data) => Some(data.caption),
        MessageVoiceNote(data) => Some(data.caption),
        _ => None,
    }
}
