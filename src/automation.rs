//! Development-only UI automation bridge.
//!
//! This module registers a set of `GAction`s on the [`Application`] that let an
//! external driver observe and drive the running client without a mouse — the
//! foundation for a screenshot-and-verify development loop. It is **gated at
//! runtime** on `config::PROFILE == "Devel"`: on release builds
//! [`Application::setup_automation`] is a no-op and no actions are registered.
//!
//! Because `GApplication` automatically exports its action group on the session
//! bus via the `org.gtk.Actions` interface, these actions are invokable with
//! `gdbus` at the object path derived from the application id
//! (`/app/drey/PaperPlane/Devel`). For example:
//!
//! ```sh
//! # Take a screenshot of the current window.
//! gdbus call --session --dest app.drey.PaperPlane.Devel \
//!   --object-path /app/drey/PaperPlane/Devel \
//!   --method org.gtk.Actions.Activate auto-screenshot '[]' '{}'
//!
//! # Log in on the test data center, then submit the code.
//! gdbus call ... --method org.gtk.Actions.Activate auto-new-test-client '[]' '{}'
//! gdbus call ... --method org.gtk.Actions.Activate auto-submit-phone "[<'9996612345'>]" '{}'
//! gdbus call ... --method org.gtk.Actions.Activate auto-submit-code  "[<'11111'>]" '{}'
//!
//! # Open a chat, send a message, react to one.
//! gdbus call ... --method org.gtk.Actions.Activate auto-open-chat "[<int64 123>]" '{}'
//! gdbus call ... --method org.gtk.Actions.Activate auto-send-text "[<(int64 123, 'hi')>]" '{}'
//! gdbus call ... --method org.gtk.Actions.Activate auto-react "[<(int64 123, int64 456, '👍')>]" '{}'
//! ```
//!
//! Results that must be read back (screenshots, message dumps, the chat list)
//! are written under [`output_dir`], which lives in the user cache dir. Inside
//! the flatpak sandbox that maps, on the host, to
//! `~/.var/app/app.drey.PaperPlane.Devel/cache/automation/`.

use adw::prelude::*;
use glib::clone;
use gtk::gio;
use gtk::glib;

use crate::application::Application;
use crate::config;
use crate::model;
use crate::types::ChatId;
use crate::types::ClientId;
use crate::types::MessageId;
use crate::utils;

/// Directory where automation artifacts (screenshots, dumps) are written.
///
/// Created on demand; on the host this resolves to
/// `~/.var/app/app.drey.PaperPlane.Devel/cache/automation/`.
pub(crate) fn output_dir() -> std::path::PathBuf {
    let dir = glib::user_cache_dir().join("automation");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!(
            "Could not create automation output dir {}: {e:?}",
            dir.display()
        );
    }
    dir
}

impl Application {
    /// Registers the development-only automation actions. No-op on non-`Devel`
    /// builds.
    pub(crate) fn setup_automation(&self) {
        if config::PROFILE != "Devel" {
            return;
        }

        log::info!(
            "Development automation bridge enabled; artifacts written to {}",
            output_dir().display()
        );

        // Observation.
        self.add_auto_action("auto-screenshot", None, |app, _| app.auto_screenshot());
        self.add_auto_action("auto-list-chats", None, |app, _| app.auto_list_chats());
        self.add_auto_action(
            "auto-list-messages",
            Some(glib::VariantTy::INT64),
            |app, p| app.auto_list_messages(p.unwrap().get::<i64>().unwrap()),
        );
        self.add_auto_action(
            "auto-describe-message",
            Some(glib::VariantTy::new("(xx)").unwrap()),
            |app, p| {
                let (chat_id, message_id) = p.unwrap().get::<(i64, i64)>().unwrap();
                app.auto_describe_message(chat_id, message_id);
            },
        );

        // Login (test data center).
        self.add_auto_action("auto-new-test-client", None, |app, _| {
            app.main_window().client_manager_view().add_new_client(true);
        });
        self.add_auto_action(
            "auto-submit-phone",
            Some(glib::VariantTy::STRING),
            |app, p| {
                app.auto_submit_phone(p.unwrap().get::<String>().unwrap());
            },
        );
        self.add_auto_action(
            "auto-submit-code",
            Some(glib::VariantTy::STRING),
            |app, p| {
                app.auto_submit_code(p.unwrap().get::<String>().unwrap());
            },
        );
        self.add_auto_action(
            "auto-submit-password",
            Some(glib::VariantTy::STRING),
            |app, p| {
                app.auto_submit_password(p.unwrap().get::<String>().unwrap());
            },
        );

        // Navigation & write actions.
        self.add_auto_action("auto-open-chat", Some(glib::VariantTy::INT64), |app, p| {
            app.auto_open_chat(p.unwrap().get::<i64>().unwrap());
        });
        self.add_auto_action(
            "auto-send-text",
            Some(glib::VariantTy::new("(xs)").unwrap()),
            |app, p| {
                let (chat_id, text) = p.unwrap().get::<(i64, String)>().unwrap();
                app.auto_send_text(chat_id, text);
            },
        );
        self.add_auto_action(
            "auto-react",
            Some(glib::VariantTy::new("(xxs)").unwrap()),
            |app, p| {
                let (chat_id, message_id, emoji) = p.unwrap().get::<(i64, i64, String)>().unwrap();
                app.auto_react(chat_id, message_id, emoji);
            },
        );
    }

    /// Adds a `SimpleAction` that forwards to `handler`, receiving the
    /// application and the (optional) action target.
    fn add_auto_action(
        &self,
        name: &str,
        parameter_type: Option<&glib::VariantTy>,
        handler: impl Fn(&Application, Option<&glib::Variant>) + 'static,
    ) {
        let action = gio::SimpleAction::new(name, parameter_type);
        action.connect_activate(clone!(
            #[weak(rename_to = app)]
            self,
            move |_, param| handler(&app, param)
        ));
        self.add_action(&action);
    }

    /// The id of the currently active client, if any.
    fn active_client_id(&self) -> Option<ClientId> {
        self.main_window()
            .client_manager_view()
            .active_client()
            .map(|client| client.id())
    }

    /// The currently active logged-in session, if any.
    fn active_session(&self) -> Option<model::ClientStateSession> {
        self.main_window()
            .client_manager_view()
            .active_client()?
            .state()
            .and_downcast::<model::ClientStateSession>()
    }

    /// Renders the current window to `<output_dir>/screenshot.png`.
    fn auto_screenshot(&self) {
        let window = self.main_window();
        let widget = window.upcast_ref::<gtk::Widget>();

        let width = widget.width();
        let height = widget.height();
        if width <= 0 || height <= 0 {
            log::warn!("auto-screenshot: window not realized yet ({width}x{height})");
            return;
        }

        let Some(renderer) = widget.native().and_then(|native| native.renderer()) else {
            log::warn!("auto-screenshot: no renderer available");
            return;
        };

        let paintable = gtk::WidgetPaintable::new(Some(widget));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, width as f64, height as f64);
        let Some(node) = snapshot.to_node() else {
            log::warn!("auto-screenshot: nothing to render");
            return;
        };

        let texture = renderer.render_texture(&node, None);
        let bytes = texture.save_to_png_bytes();

        let path = output_dir().join("screenshot.png");
        match std::fs::write(&path, bytes.as_ref()) {
            Ok(()) => log::info!("auto-screenshot: wrote {}", path.display()),
            Err(e) => log::warn!("auto-screenshot: failed to write {}: {e:?}", path.display()),
        }
    }

    /// Writes the loaded main chat list (`<id>\t<title>` per line) to
    /// `<output_dir>/chats.txt`.
    fn auto_list_chats(&self) {
        let Some(session) = self.active_session() else {
            log::warn!("auto-list-chats: no active session");
            return;
        };

        let list = session.main_chat_list();
        let mut out = String::new();
        for i in 0..list.n_items() {
            if let Some(item) = list.item(i).and_downcast::<model::ChatListItem>() {
                let chat = item.chat_();
                out.push_str(&format!("{}\t{}\n", chat.id(), chat.title()));
            }
        }

        let path = output_dir().join("chats.txt");
        if let Err(e) = std::fs::write(&path, out) {
            log::warn!("auto-list-chats: failed to write {}: {e:?}", path.display());
        }
    }

    /// Writes the newest messages of a chat (`<id>\t<content-preview>` per line,
    /// newest first) to `<output_dir>/messages.txt`, so a driver can pick a
    /// message id to act on.
    fn auto_list_messages(&self, chat_id: ChatId) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-list-messages: no active client");
            return;
        };

        utils::spawn(async move {
            let out = match tdlib::functions::get_chat_history(chat_id, 0, 0, 30, false, client_id)
                .await
            {
                Ok(tdlib::enums::Messages::Messages(data)) => data
                    .messages
                    .into_iter()
                    .flatten()
                    .map(|m| {
                        let mut preview = format!("{:?}", m.content);
                        preview.truncate(80);
                        format!("{}\t{}", m.id, preview)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(e) => format!("get_chat_history({chat_id}) failed: {e:?}"),
            };
            let path = output_dir().join("messages.txt");
            if let Err(e) = std::fs::write(&path, out) {
                log::warn!(
                    "auto-list-messages: failed to write {}: {e:?}",
                    path.display()
                );
            }
        });
    }

    /// Dumps everything the client knows about a message (its full `get_message`
    /// result, `Debug`-formatted) to `<output_dir>/message.txt`.
    fn auto_describe_message(&self, chat_id: ChatId, message_id: MessageId) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-describe-message: no active client");
            return;
        };

        utils::spawn(async move {
            let text = match tdlib::functions::get_message(chat_id, message_id, client_id).await {
                Ok(tdlib::enums::Message::Message(message)) => format!("{message:#?}"),
                Err(e) => format!("get_message({chat_id}, {message_id}) failed: {e:?}"),
            };
            let path = output_dir().join("message.txt");
            if let Err(e) = std::fs::write(&path, text) {
                log::warn!(
                    "auto-describe-message: failed to write {}: {e:?}",
                    path.display()
                );
            }
        });
    }

    fn auto_submit_phone(&self, phone_number: String) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-submit-phone: no active client");
            return;
        };
        utils::spawn(async move {
            let settings = tdlib::types::PhoneNumberAuthenticationSettings {
                allow_flash_call: true,
                allow_missed_call: true,
                ..Default::default()
            };
            if let Err(e) = tdlib::functions::set_authentication_phone_number(
                phone_number,
                Some(settings),
                client_id,
            )
            .await
            {
                log::warn!("auto-submit-phone failed: {e:?}");
            }
        });
    }

    fn auto_submit_code(&self, code: String) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-submit-code: no active client");
            return;
        };
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::check_authentication_code(code, client_id).await {
                log::warn!("auto-submit-code failed: {e:?}");
            }
        });
    }

    fn auto_submit_password(&self, password: String) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-submit-password: no active client");
            return;
        };
        utils::spawn(async move {
            if let Err(e) =
                tdlib::functions::check_authentication_password(password, client_id).await
            {
                log::warn!("auto-submit-password failed: {e:?}");
            }
        });
    }

    fn auto_open_chat(&self, chat_id: ChatId) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-open-chat: no active client");
            return;
        };
        self.main_window().select_chat(client_id, chat_id);
    }

    fn auto_send_text(&self, chat_id: ChatId, text: String) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-send-text: no active client");
            return;
        };
        utils::spawn(async move {
            let content = tdlib::enums::InputMessageContent::InputMessageText(
                tdlib::types::InputMessageText {
                    text: tdlib::types::FormattedText {
                        text,
                        entities: vec![],
                    },
                    link_preview_options: None,
                    clear_draft: false,
                },
            );
            if let Err(e) =
                tdlib::functions::send_message(chat_id, None, None, None, content, client_id).await
            {
                log::warn!("auto-send-text failed: {e:?}");
            }
        });
    }

    fn auto_react(&self, chat_id: ChatId, message_id: MessageId, emoji: String) {
        let Some(client_id) = self.active_client_id() else {
            log::warn!("auto-react: no active client");
            return;
        };
        utils::spawn(async move {
            let reaction_type =
                tdlib::enums::ReactionType::Emoji(tdlib::types::ReactionTypeEmoji { emoji });
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
                log::warn!("auto-react failed: {e:?}");
            }
        });
    }
}
