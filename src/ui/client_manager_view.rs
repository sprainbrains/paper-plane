use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;

use glib::Properties;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::APPLICATION_OPTS;
use crate::Application;
use crate::model;
use crate::types::ChatId;
use crate::types::ClientId;
use crate::ui;
use crate::utils;

/// What a freshly-initialized window should show.
#[derive(Debug, Clone)]
pub(crate) enum InitialView {
    /// Restore the last-used account (the normal primary-window behavior).
    RestoreLast,
    /// Show a specific account.
    Account(model::Client),
    /// Show a specific account with a chat pre-selected.
    Chat(model::Client, ChatId),
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::ClientManagerView)]
    #[template(resource = "/app/drey/paper-plane/ui/client_manager_view.ui")]
    pub(crate) struct ClientManagerView {
        pub(super) settings: utils::PaperPlaneSettings,
        /// The shared client manager, injected by the window via `init`.
        pub(super) client_manager: OnceCell<model::ClientManager>,
        /// The client from `client_manager` we currently show to the user
        #[property(get, set = Self::set_active_client, explicit_notify)]
        pub(super) active_client: glib::WeakRef<model::Client>,
        #[property(get)]
        pub(super) last_used_session: RefCell<Option<String>>,
        /// A chat to pre-select on the next active client. Consumed only once that client's
        /// `Session` exists (built synchronously for an already-logged-in client); if no
        /// session is present the request is left in place rather than silently dropped.
        pub(super) pending_chat: Cell<Option<ChatId>>,
        /// The `notify::chat` handler on the current session's `Content`, and the
        /// `(client, chat)` pair last reported to the application registry. Both are
        /// re-wired whenever the active client changes.
        pub(super) content_handler:
            RefCell<Option<(glib::WeakRef<ui::Content>, glib::SignalHandlerId)>>,
        pub(super) tracked_chat: Cell<Option<(ClientId, ChatId)>>,
        /// Handler on the shared client manager. The manager outlives this view, so the
        /// handler must be disconnected on dispose or it leaks and can fire on a
        /// destroyed view.
        pub(super) client_removed_handler: RefCell<Option<glib::SignalHandlerId>>,
        #[template_child]
        pub(super) animated_bin: TemplateChild<ui::AnimatedBin>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ClientManagerView {
        const NAME: &'static str = "PaplClientManagerView";
        type Type = super::ClientManagerView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ClientManagerView {
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
            if let Some((manager, handler)) = self
                .client_manager
                .get()
                .zip(self.client_removed_handler.borrow_mut().take())
            {
                manager.disconnect(handler);
            }
            self.clear_chat_tracking();
            self.animated_bin.unparent();
        }
    }

    impl WidgetImpl for ClientManagerView {}

    #[gtk::template_callbacks]
    impl ClientManagerView {
        fn set_active_client(&self, client: &model::Client) {
            let obj = &*self.obj();

            let old_active_client = obj.active_client();
            if old_active_client.as_ref() == Some(client) {
                return;
            }

            if let Some(client) = old_active_client {
                client.set_active(false);
                utils::spawn(async move {
                    client.set_online(false).await;
                });
            }

            self.active_client.set(Some(client));
            obj.notify_active_client();
        }

        #[template_callback]
        fn on_notify_active_client(&self) {
            let obj = &*self.obj();

            // Stop tracking the previous session's open chat.
            self.clear_chat_tracking();

            let Some(client) = obj.active_client() else {
                return;
            };

            client.set_active(true);
            obj.set_active_client_online();

            if client.is_logged_in()
                && let Err(e) = self.settings.set_string(
                    "last-used-session",
                    &client.database_info().0.directory_base_name,
                )
            {
                log::warn!("Could not save setting 'last-used-session': {e}");
            }

            let client_view = ui::ClientView::from(&client);
            self.animated_bin.set_child(&client_view);

            // For an already-logged-in client the `Session` (and `Content`) is built
            // synchronously by the state expression, so the chat can be selected and the
            // registry wired up right away.
            if let Some(session) = utils::descendant::<_, ui::Session>(&client_view)
                && let Some(chat_id) = self.pending_chat.take()
            {
                session.select_chat(chat_id);
            }

            if let Some(content) = utils::descendant::<_, ui::Content>(&client_view) {
                self.track_content(&content);
            }
        }

        /// Connects to `content`'s open-chat changes and reports them to the app registry.
        fn track_content(&self, content: &ui::Content) {
            let obj = self.obj();

            obj.report_content_chat(content);

            let handler = content.connect_notify_local(
                Some("chat"),
                clone!(
                    #[weak(rename_to = view)]
                    obj,
                    move |content, _| {
                        view.report_content_chat(content);
                    }
                ),
            );
            self.content_handler
                .replace(Some((content.downgrade(), handler)));
        }

        /// Disconnects the current content handler and unregisters the tracked chat.
        pub(super) fn clear_chat_tracking(&self) {
            if let Some((content, handler)) = self.content_handler.borrow_mut().take()
                && let Some(content) = content.upgrade()
            {
                content.disconnect(handler);
            }

            if let Some(old) = self.tracked_chat.take()
                && let Some(window) = self.obj().root().and_downcast::<ui::Window>()
                && let Some(app) = window.application().and_downcast::<Application>()
            {
                app.set_window_chat(&window, Some(old), None);
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct ClientManagerView(ObjectSubclass<imp::ClientManagerView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ClientManagerView {
    /// Injects the shared client manager and shows the requested initial view. Called by
    /// the window once, after construction (a template child can't reach the shared
    /// manager during its own `constructed`).
    pub(crate) fn init(&self, manager: &model::ClientManager, initial: InitialView) {
        let imp = self.imp();

        imp.client_manager
            .set(manager.clone())
            .expect("Client manager already set.");

        let handler = manager.connect_client_removed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, client| {
                if Some(client) == obj.active_client().as_ref() {
                    obj.restore_last_session()
                }
            }
        ));
        imp.client_removed_handler.replace(Some(handler));

        match initial {
            InitialView::RestoreLast => self.restore_last_session(),
            InitialView::Account(client) => self.set_active_client(&client),
            InitialView::Chat(client, chat_id) => {
                self.imp().pending_chat.set(Some(chat_id));
                self.set_active_client(&client);
            }
        }
    }

    pub(crate) fn client_manager(&self) -> model::ClientManager {
        self.imp().client_manager.get().unwrap().clone()
    }

    pub(crate) fn add_new_client(&self, use_test_dc: bool) {
        self.set_active_client(self.client_manager().add_new_client(use_test_dc));
    }

    fn restore_last_session(&self) {
        let imp = self.imp();
        let manager = imp.client_manager.get().unwrap();

        let active_client = manager.first_client().and_then(|first_client| {
            let directory_base_name = imp.settings.string("last-used-session");
            manager
                .client_by_directory_base_name(directory_base_name.as_str())
                .or(Some(first_client))
        });

        match active_client {
            Some(client) => self.set_active_client(&client),
            None => self.add_new_client(APPLICATION_OPTS.get().unwrap().test_dc),
        }
    }

    /// Sets the online status for the active logged in client. This will be called from the
    /// application `Window` when its active state has changed.
    pub(crate) fn set_active_client_online(&self) {
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Some(client) = obj.active_client()
                    && let Some(window) = obj.root().and_downcast::<gtk::Window>()
                {
                    client.set_online(window.is_active()).await;
                }
            }
        ));
    }

    /// Reports `content`'s current open chat to the application registry, replacing the
    /// previously tracked pair for this window.
    fn report_content_chat(&self, content: &ui::Content) {
        let imp = self.imp();

        let new = content
            .chat()
            .map(|chat| (chat.session_().client_().id(), chat.id()));
        let old = imp.tracked_chat.replace(new);
        if old == new {
            return;
        }

        if let Some(window) = self.root().and_downcast::<ui::Window>()
            && let Some(app) = window.application().and_downcast::<Application>()
        {
            app.set_window_chat(&window, old, new);
        }
    }

    pub(crate) fn select_chat(&self, client_id: ClientId, chat_id: ChatId) {
        if self
            .active_client()
            .filter(|client| client.id() == client_id)
            .is_some()
            && let Some(session) = utils::descendant::<_, ui::Session>(self)
        {
            session.select_chat(chat_id);
        }
    }
}
