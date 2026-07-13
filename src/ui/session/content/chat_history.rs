use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use futures::Future;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;

use crate::expressions;
use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_history.ui")]
    pub(crate) struct ChatHistory {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) chat_handler: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) model: RefCell<Option<model::ChatHistoryModel>>,
        pub(super) message_menu: OnceCell<ui::MessageMenu>,
        pub(super) is_auto_scrolling: Cell<bool>,
        pub(super) is_loading_messages: Cell<bool>,
        pub(super) sticky: Cell<bool>,
        pub(super) viewed_message_ids: RefCell<HashSet<i64>>,
        pub(super) viewed_message_ids_changed: Cell<bool>,
        #[template_child]
        pub(super) window_title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) background: TemplateChild<ui::Background>,
        #[template_child]
        pub(super) scrolled_window: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) list_view: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) chat_action_bar: TemplateChild<ui::ChatActionBar>,
        #[template_child]
        pub(super) pinned_message_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) pinned_message_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) unpin_button: TemplateChild<gtk::Button>,
        pub(super) pinned_message_id: Cell<i64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatHistory {
        const NAME: &'static str = "PaplChatHistory";
        type Type = super::ChatHistory;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action("chat-history.view-info", None, move |widget, _, _| {
                widget.open_info_dialog();
            });
            klass.install_action("chat-history.scroll-down", None, move |widget, _, _| {
                widget.scroll_down();
            });
            klass.install_action(
                "chat-history.reply",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.imp().chat_action_bar.reply_to_message_id(message_id);
                },
            );
            klass.install_action(
                "chat-history.edit",
                Some(glib::VariantTy::INT64),
                move |widget, _, variant| {
                    let message_id = variant.and_then(|v| v.get()).unwrap();
                    widget.imp().chat_action_bar.edit_message_id(message_id);
                },
            );
            klass.install_action_async(
                "chat-history.leave-chat",
                None,
                |widget, _, _| async move {
                    widget.show_leave_chat_dialog().await;
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatHistory {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecBoolean::builder("sticky")
                        .read_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "chat" => {
                    let chat = value.get().unwrap();
                    obj.set_chat(chat);
                }
                "sticky" => obj.set_sticky(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "chat" => obj.chat().to_value(),
                "sticky" => obj.sticky().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.setup_expressions();

            self.unpin_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.unpin_message();
                }
            ));

            // Clicking the pinned-message bar reveals the message in the history.
            let pinned_gesture = gtk::GestureClick::new();
            pinned_gesture.connect_released(clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    obj.scroll_to_pinned();
                }
            ));
            self.pinned_message_bar.add_controller(pinned_gesture);

            let adj = self.list_view.vadjustment().unwrap();
            adj.connect_value_changed(clone!(
                #[weak]
                obj,
                move |adj| {
                    obj.view_messages();

                    let imp = obj.imp();

                    if imp.is_loading_messages.get() {
                        return;
                    }

                    if imp.is_auto_scrolling.get() {
                        if adj.value() + adj.page_size() >= adj.upper() {
                            imp.is_auto_scrolling.set(false);
                            obj.set_sticky(true);
                        }
                    } else {
                        obj.set_sticky(adj.value() + adj.page_size() >= adj.upper());

                        if adj.value() >= adj.page_size() * 2.0
                            && adj.upper() > adj.page_size() * 2.0
                        {
                            return;
                        }

                        if let Some(model) = imp.model.borrow().as_ref() {
                            imp.is_loading_messages.set(true);

                            utils::spawn(clone!(
                                #[weak]
                                obj,
                                #[weak]
                                model,
                                async move {
                                    obj.imp().is_loading_messages.set(false);

                                    if let Err(model::ChatHistoryError::Tdlib(e)) =
                                        model.load_older_messages(2).await
                                    {
                                        log::warn!("Couldn't load more chat messages: {e:?}");
                                    }
                                }
                            ));
                        }
                    }
                }
            ));

            adj.connect_upper_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    if obj.sticky() || obj.imp().is_auto_scrolling.get() {
                        obj.scroll_down();
                    }
                }
            ));
        }

        fn dispose(&self) {
            if let Some(chat) = self.obj().chat() {
                perform_chat_action(&chat, tdlib::functions::close_chat);
            }
        }
    }

    impl WidgetImpl for ChatHistory {
        fn direction_changed(&self, previous_direction: gtk::TextDirection) {
            let obj = self.obj();

            if obj.direction() == previous_direction {
                return;
            }

            if let Some(menu) = self.message_menu.get() {
                menu.set_halign(if obj.direction() == gtk::TextDirection::Rtl {
                    gtk::Align::End
                } else {
                    gtk::Align::Start
                });
            }
        }
    }

    impl BinImpl for ChatHistory {}
}

glib::wrapper! {
    pub(crate) struct ChatHistory(ObjectSubclass<imp::ChatHistory>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ChatHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistory {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn setup_expressions(&self) {
        let chat_expression = Self::this_expression("chat");

        // Chat title
        expressions::chat_display_name(&chat_expression).bind(
            &*self.imp().window_title,
            "title",
            Some(self),
        );
    }

    fn open_info_dialog(&self) {
        if let Some(chat) = self.chat() {
            ui::ChatInfoWindow::new(&self.parent_window(), &chat).present();
        }
    }

    async fn show_leave_chat_dialog(&self) {
        if let Some(chat) = self.chat() {
            let dialog = ui::destructive_confirmation_dialog(
                &gettext("Leave chat?"),
                &gettext("Do you want to leave this chat?"),
                ("no", &gettext("_No")),
                ("yes", &gettext("_Yes")),
            );
            dialog.set_close_response("no");

            if dialog.choose_future(Some(self)).await == "yes" {
                match tdlib::functions::leave_chat(chat.id(), chat.session_().client_().id()).await
                {
                    Ok(_) => {
                        // Unselect recently left chat
                        if let Some(sidebar) = utils::ancestor::<_, ui::Sidebar>(self) {
                            sidebar.set_selected_chat(Option::<model::Chat>::None);
                        }
                    }
                    Err(e) => log::warn!("Failed to leave chat: {e:?}"),
                }
            }
        }
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.root()?.downcast().ok()
    }

    fn request_sponsored_message(&self, chat: &model::Chat, list: &gio::ListStore) {
        utils::spawn(clone!(
            #[weak]
            chat,
            #[weak]
            list,
            async move {
                match model::SponsoredMessage::request(&chat).await {
                    Ok(sponsored_message) => {
                        if let Some(sponsored_message) = sponsored_message {
                            list.append(&sponsored_message);
                        }
                    }
                    Err(e) => {
                        if e.code != 404 {
                            log::warn!("Failed to request a SponsoredMessage: {e:?}");
                        }
                    }
                }
            }
        ));
    }

    pub(crate) fn add_to_viewed_message_ids(&self, message_id: i64) {
        let imp = self.imp();
        if imp.viewed_message_ids.borrow_mut().insert(message_id) {
            imp.viewed_message_ids_changed.set(true);
        }
    }

    pub(crate) fn remove_from_viewed_message_ids(&self, message_id: i64) {
        let imp = self.imp();
        if imp.viewed_message_ids.borrow_mut().remove(&message_id) {
            imp.viewed_message_ids_changed.set(true);
        }
    }

    pub(crate) fn message_menu(&self) -> &ui::MessageMenu {
        self.imp().message_menu.get_or_init(|| {
            let menu = ui::MessageMenu::new();

            menu.set_halign(if self.direction() == gtk::TextDirection::Rtl {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });

            menu
        })
    }

    pub(crate) fn handle_paste_action(&self) {
        self.imp().chat_action_bar.handle_paste_action();
    }

    pub(crate) fn insert_emoji(&self, emoji: &str) {
        self.imp().chat_action_bar.insert_emoji(emoji);
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        self.imp()
            .chat_action_bar
            .insert_custom_emoji(emoji, custom_emoji_id);
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    pub(crate) fn set_chat(&self, chat: Option<&model::Chat>) {
        let old_chat = self.chat();
        if chat == old_chat.as_ref() {
            return;
        }

        let imp = self.imp();

        // Reset the pinned-message bar; it's re-populated below if the new chat
        // has a pinned message.
        imp.pinned_message_bar.set_visible(false);

        if let Some(chat) = old_chat {
            chat.disconnect(imp.chat_handler.replace(None).unwrap());
            perform_chat_action(chat.as_ref(), tdlib::functions::close_chat);
        }

        if let Some(chat) = chat {
            self.fetch_pinned_message(chat);

            self.action_set_enabled(
                "chat-history.leave-chat",
                match chat.chat_type() {
                    model::ChatType::BasicGroup(data) => {
                        data.status().0 != tdlib::enums::ChatMemberStatus::Left
                    }
                    model::ChatType::Supergroup(data) => {
                        data.status().0 != tdlib::enums::ChatMemberStatus::Left
                    }
                    _ => false,
                },
            );

            let model = model::ChatHistoryModel::new(chat);

            // Request sponsored message, if needed
            let list_view_model: gio::ListModel = if matches!(chat.chat_type(), model::ChatType::Supergroup(supergroup) if supergroup.is_channel())
            {
                let list = gio::ListStore::new::<gio::ListModel>();

                // We need to create a list here so that we can append the sponsored message
                // to the chat history in the GtkListView using a GtkFlattenListModel
                let sponsored_message_list = gio::ListStore::new::<model::SponsoredMessage>();
                list.append(&sponsored_message_list);
                self.request_sponsored_message(chat, &sponsored_message_list);

                list.append(&model);

                gtk::FlattenListModel::new(Some(list)).upcast()
            } else {
                model.clone().upcast()
            };

            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                model,
                async move {
                    let imp = obj.imp();

                    imp.is_loading_messages.set(true);

                    let scrollbar = imp.scrolled_window.vscrollbar();
                    scrollbar.set_visible(false);

                    let adj = imp.list_view.vadjustment().unwrap();
                    adj.set_value(0.0);

                    while adj.value() == 0.0 {
                        match model.load_older_messages(2).await {
                            Ok(can_load_more) => {
                                if !can_load_more {
                                    break;
                                }
                            }
                            Err(e) => {
                                log::warn!("Couldn't load initial history messages: {e}");
                                break;
                            }
                        }
                    }

                    scrollbar.set_visible(true);

                    imp.is_loading_messages.set(false);
                    obj.set_sticky(true);

                    obj.view_messages();
                }
            ));

            let handler = chat.connect_new_message(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, msg| {
                    if msg.is_outgoing() {
                        obj.imp().background.animate();
                        // Reveal the message the user just sent: stick to the
                        // bottom and scroll down, even if the view wasn't already
                        // pinned there (otherwise an own message lands below the
                        // fold and only shows up after reloading the chat).
                        obj.set_sticky(true);
                        obj.scroll_down();
                    }
                }
            ));
            imp.chat_handler.replace(Some(handler));

            let selection = gtk::NoSelection::new(Some(list_view_model));
            imp.list_view.set_model(Some(&selection));

            imp.model.replace(Some(model));

            perform_chat_action(chat, tdlib::functions::open_chat);
        }

        imp.chat.set(chat);
        self.notify("chat");
    }

    /// Scrolls the history to the pinned message, if it is currently loaded.
    fn scroll_to_pinned(&self) {
        let imp = self.imp();
        let message_id = imp.pinned_message_id.get();

        let Some(model) = imp.list_view.model() else {
            return;
        };

        for pos in 0..model.n_items() {
            if let Some(item) = model.item(pos).and_downcast::<model::ChatHistoryItem>()
                && let Some(message) = item.message()
                && message.id() == message_id
            {
                imp.list_view
                    .scroll_to(pos, gtk::ListScrollFlags::FOCUS, None);
                return;
            }
        }

        log::debug!("Pinned message {message_id} is not in the loaded history");
    }

    /// Unpins the message currently shown in the pinned-message bar.
    fn unpin_message(&self) {
        let imp = self.imp();
        let Some(chat) = self.chat() else {
            return;
        };
        let message_id = imp.pinned_message_id.get();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();

        imp.pinned_message_bar.set_visible(false);

        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::unpin_chat_message(chat_id, message_id, client_id).await
            {
                log::warn!("Failed to unpin a message: {e:?}");
            }
        });
    }

    /// Fetches the chat's pinned message (if any) and shows a preview of it in
    /// the bar below the header. TDLib returns an error when nothing is pinned,
    /// in which case the bar stays hidden.
    fn fetch_pinned_message(&self, chat: &model::Chat) {
        let chat = chat.clone();
        let client_id = chat.session_().client_().id();
        let chat_id = chat.id();

        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Ok(tdlib::enums::Message::Message(td_message)) =
                    tdlib::functions::get_chat_pinned_message(chat_id, client_id).await
                {
                    // Ignore the result if the user has since switched chats.
                    if obj.chat().as_ref() != Some(&chat) {
                        return;
                    }

                    let message = model::Message::new(&chat, td_message);
                    let imp = obj.imp();
                    imp.pinned_message_id.set(message.id());
                    imp.pinned_message_label
                        .set_label(&strings::message_content(&message));
                    imp.pinned_message_bar.set_visible(true);
                }
            }
        ));
    }

    pub(crate) fn sticky(&self) -> bool {
        self.imp().sticky.get()
    }

    fn set_sticky(&self, sticky: bool) {
        if self.sticky() == sticky {
            return;
        }

        self.imp().sticky.set(sticky);
        self.notify("sticky");
    }

    fn scroll_down(&self) {
        let imp = self.imp();

        imp.is_auto_scrolling.set(true);

        imp.scrolled_window
            .emit_by_name::<bool>("scroll-child", &[&gtk::ScrollType::End, &false]);
    }

    pub(crate) fn view_messages(&self) {
        let imp = self.imp();

        if imp.viewed_message_ids_changed.get() {
            imp.viewed_message_ids_changed.set(false);

            let chat = self.chat().unwrap();
            let chat_id = chat.id();
            let client_id = chat.session_().client_().id();
            let viewed_message_ids =
                Vec::from_iter(imp.viewed_message_ids.borrow().iter().copied());

            utils::spawn(async move {
                tdlib::functions::view_messages(chat_id, viewed_message_ids, None, true, client_id)
                    .await
                    .unwrap();
            });
        }
    }
}

fn perform_chat_action<F, Fut>(chat: &model::Chat, op: F)
where
    F: Fn(i64, i32) -> Fut + 'static,
    Fut: Future<Output = Result<(), tdlib::types::Error>>,
{
    utils::spawn(clone!(
        #[weak]
        chat,
        async move {
            op(chat.id(), chat.session_().client_().id()).await.unwrap();
        }
    ));
}
