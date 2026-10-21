use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashMap;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::config;
use crate::model;
use crate::strings;
use crate::types::ChatId;
use crate::types::ClientId;
use crate::ui;
use crate::ui::InitialView;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub(crate) struct Application {
        /// The single TDLib-backed client manager, shared by every window.
        pub(super) client_manager: OnceCell<model::ClientManager>,
        /// Which window currently displays a given chat, so a chat can't be opened in
        /// a second separate window. Updated as each window navigates.
        pub(super) open_chats: RefCell<HashMap<(ClientId, ChatId), glib::WeakRef<ui::Window>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Application {
        const NAME: &'static str = "PaplApplication";
        type Type = super::Application;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for Application {}

    impl ApplicationImpl for Application {
        fn activate(&self) {
            log::debug!("GtkApplication<Application>::activate");

            let obj = self.obj();

            if let Some(window) = obj.existing_window() {
                window.present();
                return;
            }

            let window = ui::Window::new(&obj);
            window.init_session(InitialView::RestoreLast);
            window.present();
        }

        fn startup(&self) {
            log::debug!("GtkApplication<Application>::startup");

            log::info!("Paper Plane ({})", config::APP_ID);
            log::info!("Version: {} ({})", config::VERSION, config::PROFILE);
            log::info!("Datadir: {}", config::PKGDATADIR);

            self.parent_startup();

            let obj = self.obj();

            // Set icons for shell
            gtk::Window::set_default_icon_name(config::APP_ID);

            // The single shared client manager owns the TDLib thread. It must be created
            // exactly once — a second one would spawn a second thread competing for the
            // global TDLib receive queue.
            let client_manager = model::ClientManager::default();
            client_manager.connect_update_notification_group(clone!(
                #[weak]
                obj,
                move |_, notification_group, session| {
                    obj.handle_notifications(notification_group, session);
                }
            ));
            self.client_manager
                .set(client_manager)
                .expect("Client manager already set.");

            obj.setup_gactions();
            obj.setup_accels();
            obj.load_color_scheme();
            obj.setup_automation();
        }

        fn shutdown(&self) {
            self.parent_shutdown();

            // Close the TDLib clients while libtdjson is still fully alive.
            // Exiting with live connections makes TDLib's static destructors
            // abort (CHECK in the session/pollable-fd teardown) or race the
            // receive thread, which is the intermittent crash on window close.
            if let Some(client_manager) = self.client_manager.get() {
                client_manager.shutdown();
            }
        }
    }

    impl GtkApplicationImpl for Application {}

    impl AdwApplicationImpl for Application {}
}

glib::wrapper! {
    pub(crate) struct Application(ObjectSubclass<imp::Application>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl Default for Application {
    fn default() -> Self {
        Self::new()
    }
}

impl Application {
    pub(crate) fn new() -> Self {
        glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("resource-base-path", "/app/drey/paper-plane/")
            .build()
    }

    /// The first live application window, if any. Derived from `windows()` so it never
    /// dangles the way a cached handle would once its window closes.
    pub(crate) fn existing_window(&self) -> Option<ui::Window> {
        self.windows()
            .into_iter()
            .find_map(|window| window.downcast::<ui::Window>().ok())
    }

    /// A window to use as a last-resort target for actions that have no better one.
    /// Panics only if the application is running with no window at all, which no action
    /// path can reach.
    pub(crate) fn main_window(&self) -> ui::Window {
        self.existing_window()
            .expect("the application has no window")
    }

    pub(crate) fn client_manager(&self) -> model::ClientManager {
        self.imp().client_manager.get().unwrap().clone()
    }

    /// Opens `chat_id` (belonging to `client`) in a new separate window, unless it is
    /// already displayed in another window — in which case that window is raised and a
    /// toast is shown on `current`.
    pub(crate) fn open_chat_in_window(
        &self,
        client: &model::Client,
        chat_id: ChatId,
        current: &ui::Window,
    ) {
        if let Some(window) = self.window_showing_chat(client.id(), chat_id) {
            if window != *current {
                utils::show_toast(current, gettext("Chat is already open in another window"));
                window.present();
            }
            return;
        }
        self.present_window_for_chat(client, chat_id);
    }

    pub(crate) fn present_window_for_chat(&self, client: &model::Client, chat_id: ChatId) {
        let window = ui::Window::new(self);
        window.init_session(InitialView::Chat(client.clone(), chat_id));
        window.present();
    }

    pub(crate) fn present_window_for_account(&self, client: &model::Client) {
        let window = ui::Window::new(self);
        window.init_session(InitialView::Account(client.clone()));
        window.present();
    }

    /// The window currently displaying `(client_id, chat_id)`, if any. Prunes the entry
    /// if the window has been destroyed.
    pub(crate) fn window_showing_chat(
        &self,
        client_id: ClientId,
        chat_id: ChatId,
    ) -> Option<ui::Window> {
        let mut open_chats = self.imp().open_chats.borrow_mut();
        let key = (client_id, chat_id);
        match open_chats.get(&key).and_then(glib::WeakRef::upgrade) {
            Some(window) => Some(window),
            None => {
                open_chats.remove(&key);
                None
            }
        }
    }

    /// Records that `window` moved from displaying `old` to displaying `new` (either may
    /// be `None`). Called whenever a window's open chat changes.
    pub(crate) fn set_window_chat(
        &self,
        window: &ui::Window,
        old: Option<(ClientId, ChatId)>,
        new: Option<(ClientId, ChatId)>,
    ) {
        let mut open_chats = self.imp().open_chats.borrow_mut();
        if let Some(old) = old {
            open_chats.remove(&old);
        }
        if let Some(new) = new {
            open_chats.insert(new, window.downgrade());
        }
    }

    /// Drops every registry entry pointing at `window` (called when it closes).
    pub(crate) fn unregister_window(&self, window: &ui::Window) {
        self.imp()
            .open_chats
            .borrow_mut()
            .retain(|_, w| w.upgrade().is_some_and(|w| &w != window));
    }

    fn setup_gactions(&self) {
        // Quit
        let action_quit = gio::SimpleAction::new("quit", None);
        action_quit.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| {
                // Close every window first so each saves its state.
                for window in app.windows() {
                    window.close();
                }
                app.quit();
            }
        ));
        self.add_action(&action_quit);

        // About
        let action_about = gio::SimpleAction::new("about", None);
        action_about.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| {
                app.show_about_dialog();
            }
        ));
        self.add_action(&action_about);

        // Select chat
        let action_select_chat =
            gio::SimpleAction::new("select-chat", Some(glib::VariantTy::new("(ix)").unwrap()));
        action_select_chat.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, data| {
                let (client_id, chat_id) = data.unwrap().get().unwrap();
                // Prefer a window already showing this chat; otherwise the primary one.
                let window = app
                    .window_showing_chat(client_id, chat_id)
                    .unwrap_or_else(|| app.main_window());
                window.present();
                window.select_chat(client_id, chat_id);
            }
        ));
        self.add_action(&action_select_chat);

        // New login on production server
        let action_new_login_production_server =
            gio::SimpleAction::new("new-login-production-server", None);
        action_new_login_production_server.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| {
                app.focused_window()
                    .client_manager_view()
                    .add_new_client(false);
            }
        ));
        self.add_action(&action_new_login_production_server);

        // New login on test server
        let action_new_login_test_server = gio::SimpleAction::new("new-login-test-server", None);
        action_new_login_test_server.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, _| {
                app.focused_window()
                    .client_manager_view()
                    .add_new_client(true);
            }
        ));
        self.add_action(&action_new_login_test_server);
    }

    /// The currently focused window, or the primary one if none is focused.
    fn focused_window(&self) -> ui::Window {
        self.active_window()
            .and_downcast::<ui::Window>()
            .unwrap_or_else(|| self.main_window())
    }

    // Sets up keyboard shortcuts
    fn setup_accels(&self) {
        self.set_accels_for_action("app.quit", &["<primary>q"]);
        self.set_accels_for_action("win.show-shortcuts", &["<primary>question"]);
    }

    fn load_color_scheme(&self) {
        let style_manager = adw::StyleManager::default();
        let settings = gio::Settings::new(config::APP_ID);
        match settings.string("color-scheme").as_ref() {
            "light" => style_manager.set_color_scheme(adw::ColorScheme::ForceLight),
            "dark" => style_manager.set_color_scheme(adw::ColorScheme::ForceDark),
            _ => style_manager.set_color_scheme(adw::ColorScheme::PreferLight),
        }
    }

    fn show_about_dialog(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Paper Plane")
            .application_icon(config::APP_ID)
            .version(config::VERSION)
            .website("https://github.com/paper-plane-developers/paper-plane")
            .issue_url("https://github.com/paper-plane-developers/paper-plane/issues")
            .support_url("https://t.me/paperplanechat")
            .developer_name(gettext("Paper Plane developers"))
            .copyright("© 2021–2023 Marco Melorio")
            .license_type(gtk::License::Gpl30)
            .developers([
                "Karol Lademan https://github.com/karl0d",
                "Marco Melorio (orig. author) https://github.com/melix99",
                "Marcus Behrendt https://github.com/marhkb",
                "Yuri Izmer https://github.com/yuraiz",
            ])
            .designers([
                "Marco Melorio https://github.com/melix99",
                "Yuri Izmer https://github.com/yuraiz",
            ])
            .artists([
                "Mateus Santos https://github.com/swyknox",
                "noëlle https://github.com/jannuary",
            ])
            .build();

        about.add_acknowledgement_section(
            Some(&gettext("Sponsors")),
            &["Alisson Lauffer", "Jordan Maris"],
        );
        about.add_legal_section(
            "Maps",
            Some(&gettext(
                "<span size=\"small\">Map data by \
                <a href=\"https://www.openstreetmap.org\">OpenStreetMap</a> \
                and contributors</span>",
            )),
            gtk::License::Custom,
            Some(&gettext(
                "OpenStreetMap® is open data, licensed under the \
                <a href=\"https://opendatacommons.org/licenses/odbl\">\
                Open Data Commons Open Database License </a> (ODbL) by the \
                <a href=\"https://osmfoundation.org\">OpenStreetMap Foundation</a> (OSMF).",
            )),
        );

        about.present(Some(&self.focused_window()));
    }

    fn handle_notifications(
        &self,
        notification_group: model::BoxedUpdateNotificationGroup,
        session: &model::ClientStateSession,
    ) {
        // Respect the user's notifications preference.
        if !gio::Settings::new(config::APP_ID).boolean("notifications-enabled") {
            return;
        }

        let chat = session.chat(notification_group.0.chat_id);

        for notification in notification_group.0.added_notifications {
            let notification_id = notification.id;

            let notification = match notification.r#type {
                tdlib::enums::NotificationType::NewMessage(data) => {
                    let message = chat.upsert_message(data.message);
                    let mut body = strings::message_content(&message);

                    // Add the sender's name to the body if the chat is a group
                    if matches!(chat.chat_type(), model::ChatType::BasicGroup(_))
                        || matches!(chat.chat_type(), model::ChatType::Supergroup(s) if !s.is_channel())
                    {
                        let sender_name = strings::message_sender(&message.sender(), true);
                        body.insert_str(0, &(sender_name + ": "));
                    }

                    let notification = gio::Notification::new(&chat.title());
                    notification.set_body(Some(&body));

                    Some(notification)
                }
                tdlib::enums::NotificationType::NewCall(_) => {
                    let body = gettext("Incoming call");
                    let notification = gio::Notification::new(&chat.title());
                    notification.set_body(Some(&body));

                    Some(notification)
                }
                _ => None,
            };

            if let Some(notification) = notification {
                notification.set_default_action_and_target_value(
                    "app.select-chat",
                    Some(&(session.client_().id(), chat.id()).to_variant()),
                );

                if let Some(avatar) = chat.avatar() {
                    let avatar_file = &avatar.0;
                    if avatar_file.local.is_downloading_completed {
                        match gdk::Texture::from_filename(&avatar_file.local.path) {
                            Ok(texture) => notification.set_icon(&texture),
                            Err(error) => {
                                log::warn!("Failed to load a notification avatar: {error:?}")
                            }
                        }
                        self.send_notification(Some(&notification_id.to_string()), &notification);
                    } else {
                        self.send_notification(Some(&notification_id.to_string()), &notification);

                        let file_id = avatar_file.id;
                        utils::spawn(clone!(
                            #[weak]
                            session,
                            #[weak(rename_to = app)]
                            self,
                            async move {
                                match session.download_file(file_id).await {
                                    Ok(file) => {
                                        match gdk::Texture::from_filename(file.local.path) {
                                            Ok(texture) => {
                                                notification.set_icon(&texture);
                                                app.send_notification(
                                                    Some(&notification_id.to_string()),
                                                    &notification,
                                                );
                                            }
                                            Err(error) => log::warn!(
                                                "Failed to load a notification avatar: {error:?}"
                                            ),
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("Failed to download an avatar: {e:?}");
                                    }
                                }
                            }
                        ));
                    }
                }
            }
        }

        for notification_id in notification_group.0.removed_notification_ids {
            self.withdraw_notification(&notification_id.to_string());
        }
    }
}
