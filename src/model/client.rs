use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;

use glib::Properties;
use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::APPLICATION_OPTS;
use crate::config;
use crate::model;
use crate::types::ClientId;
use crate::utils;

#[derive(Debug, Default)]
struct AuthorizationUpdateTracker(Cell<u64>);

impl AuthorizationUpdateTracker {
    fn next(&self) -> u64 {
        let id = self.0.get().wrapping_add(1);
        self.0.set(id);
        id
    }

    fn is_current(&self, id: u64) -> bool {
        self.0.get() == id
    }
}

/// A struct for storing information about a session's database.
#[derive(Clone, Debug)]
pub(crate) struct DatabaseInfo {
    // The base name of the database directory.
    pub(crate) directory_base_name: String,
    // Whether this database uses a test dc.
    pub(crate) use_test_dc: bool,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::Client)]
    pub(crate) struct Client {
        pub(crate) queued_updates: RefCell<VecDeque<tdlib::enums::Update>>,
        pub(super) authorization_updates: AuthorizationUpdateTracker,
        #[property(get, set, construct_only)]
        pub(super) client_manager: glib::WeakRef<model::ClientManager>,
        #[property(get, set, construct_only)]
        pub(super) remove_if_auth: OnceCell<bool>,
        #[property(get, set, construct_only)]
        pub(super) id: OnceCell<ClientId>,
        #[property(get, set, construct_only)]
        pub(super) database_info: OnceCell<model::BoxedDatabaseInfo>,
        #[property(get)]
        pub(super) state: RefCell<Option<glib::Object>>,
        #[property(get, set)]
        pub(super) active: Cell<bool>,
        #[property(get)]
        pub(super) connection_online: Cell<bool>,
        pub(super) proxies: OnceCell<model::ProxyManager>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Client {
        const NAME: &'static str = "Client";
        type Type = super::Client;
    }

    impl ObjectImpl for Client {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }
        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }
        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();

            utils::spawn(clone!(
                #[weak]
                obj,
                async move {
                    obj.init().await;
                }
            ));
            obj.set_state(model::ClientStateAuth::from(obj).upcast());
        }
    }
}

glib::wrapper! { pub(crate) struct Client(ObjectSubclass<imp::Client>); }

impl Client {
    pub(crate) fn new(
        client_manager: &model::ClientManager,
        remove_if_auth: bool,
        id: ClientId,
        database_info: model::DatabaseInfo,
    ) -> Self {
        glib::Object::builder()
            .property("client-manager", client_manager)
            .property("remove-if-auth", remove_if_auth)
            .property("id", id)
            .property("database-info", model::BoxedDatabaseInfo(database_info))
            .build()
    }

    weak_getter!(client_manager_, client_manager, model::ClientManager);

    async fn init(&self) {
        if let Err(e) = tdlib::functions::set_log_verbosity_level(
            if log::log_enabled!(log::Level::Trace) {
                5
            } else if log::log_enabled!(log::Level::Debug) {
                4
            } else if log::log_enabled!(log::Level::Info) {
                3
            } else if log::log_enabled!(log::Level::Warn) {
                2
            } else {
                0
            },
            self.id(),
        )
        .await
        {
            log::warn!("Error setting the tdlib log level: {e:?}");
        }
    }

    pub(crate) fn is_logged_in(&self) -> bool {
        self.state()
            .and_downcast::<model::ClientStateSession>()
            .is_some()
    }

    pub(crate) async fn send_tdlib_parameters(&self) -> Result<(), tdlib::types::Error> {
        let system_language_code = sys_locale::get_locale().unwrap_or_else(|| "en_US".to_string());

        let database_info = &self.database_info().0;

        let database_directory = utils::data_dir()
            .join(&database_info.directory_base_name)
            .to_string_lossy()
            .into_owned();

        let application_opts = APPLICATION_OPTS.get().unwrap();

        tdlib::functions::set_tdlib_parameters(
            database_info.use_test_dc,
            database_directory,
            String::new(),
            String::new(),
            true,
            true,
            true,
            true,
            application_opts.client_id,
            application_opts.client_secret.to_string(),
            system_language_code,
            "Desktop".into(),
            String::new(),
            config::VERSION.into(),
            self.id(),
        )
        .await
    }

    fn set_state(&self, state: glib::Object) {
        if self.state().as_ref() == Some(&state) {
            return;
        }
        self.imp().state.replace(Some(state));
        self.notify_state();
    }

    pub(crate) async fn set_online(&self, online: bool) {
        if let Err(e) = tdlib::functions::set_option(
            "online".to_string(),
            Some(tdlib::enums::OptionValue::Boolean(
                tdlib::types::OptionValueBoolean { value: online },
            )),
            self.id(),
        )
        .await
        {
            log::error!(
                "Could not set online state of client '{}' to '{}': {}",
                self.id(),
                online,
                e.message
            );
        }
    }

    pub(crate) async fn log_out(&self) -> Result<(), tdlib::types::Error> {
        match tdlib::functions::log_out(self.id()).await {
            Ok(_) => Ok(()),
            Err(e) => {
                log::error!(
                    "Failed to log out client with id={}: {}",
                    self.id(),
                    e.message
                );
                Err(e)
            }
        }
    }

    pub(crate) fn remove(&self) {
        let database_path = utils::data_dir().join(self.database_info().0.directory_base_name);
        if fs::metadata(&database_path).is_ok()
            && let Err(e) = fs::remove_dir_all(&database_path)
        {
            log::error!("Error on on removing database directory {database_path:?}: {e}");
        }

        self.client_manager_().remove_client(self);
    }

    pub(crate) fn handle_update(&self, update: tdlib::enums::Update) {
        use tdlib::enums::AuthorizationState;
        use tdlib::enums::Update;

        match update {
            Update::AuthorizationState(state) => {
                let authorization_update_id = self.imp().authorization_updates.next();

                match state.authorization_state {
                    AuthorizationState::WaitTdlibParameters => {
                        utils::spawn(clone!(
                            #[weak(rename_to = obj)]
                            self,
                            async move {
                                if let Err(e) = obj.send_tdlib_parameters().await {
                                    log::error!("Failed sensing tdlib parameters: {e:?}");
                                }
                            }
                        ));
                    }
                    AuthorizationState::Ready => {
                        utils::spawn(clone!(
                            #[weak(rename_to = obj)]
                            self,
                            async move {
                                let me = match tdlib::functions::get_me(obj.id()).await {
                                    Ok(tdlib::enums::User::User(me)) => me,
                                    Err(e) => {
                                        log::error!("Failed to get current user: {e:?}");
                                        return;
                                    }
                                };

                                if !obj
                                    .imp()
                                    .authorization_updates
                                    .is_current(authorization_update_id)
                                    || obj
                                        .state()
                                        .and_downcast::<model::ClientStateAuth>()
                                        .is_none()
                                {
                                    return;
                                }

                                let state = model::ClientStateSession::new(&obj, me);
                                obj.set_state(state.clone().upcast());
                                loop {
                                    let update = obj.imp().queued_updates.borrow_mut().pop_front();
                                    let Some(update) = update else {
                                        break;
                                    };
                                    obj.handle_session_update(&state, update);
                                }

                                obj.client_manager_().on_client_logged_in(&obj);
                            }
                        ));
                    }
                    AuthorizationState::Closing => {
                        self.set_state(model::ClientStateLoggingOut::default().upcast());
                    }
                    AuthorizationState::LoggingOut => {
                        log::info!("Logging out client {}", self.id());
                    }
                    AuthorizationState::Closed => self.remove(),
                    _ => {
                        if let Some(auth) = self.state().and_downcast::<model::ClientStateAuth>() {
                            auth.handle_update(state);
                        }
                    }
                }
            }
            Update::ConnectionState(state) => {
                let online = matches!(state.state, tdlib::enums::ConnectionState::Ready);
                self.set_connection_online(online);
            }
            _ => match self.state().and_downcast::<model::ClientStateSession>() {
                Some(state) => self.handle_session_update(&state, update),
                None => self.imp().queued_updates.borrow_mut().push_back(update),
            },
        }
    }

    /// The client's proxy manager, created on first use.
    pub(crate) fn proxies(&self) -> model::ProxyManager {
        self.imp()
            .proxies
            .get_or_init(|| model::ProxyManager::new(self))
            .clone()
    }

    fn set_connection_online(&self, online: bool) {
        let imp = self.imp();
        if imp.connection_online.get() == online {
            return;
        }
        imp.connection_online.set(online);
        self.notify_connection_online();
        // Only touch the manager if it already exists: connection updates
        // arrive long before any proxy UI.
        if let Some(manager) = imp.proxies.get() {
            manager.set_connection_online(online);
        }
    }

    fn handle_session_update(
        &self,
        session: &model::ClientStateSession,
        update: tdlib::enums::Update,
    ) {
        if let tdlib::enums::Update::NotificationGroup(group) = update {
            self.client_manager_()
                .on_update_notification_group(group, session);
        } else {
            session.handle_update(update);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_authorization_update_invalidates_ready_completion() {
        let updates = AuthorizationUpdateTracker::default();
        let first_ready = updates.next();
        let second_ready = updates.next();

        assert!(!updates.is_current(first_ready));
        assert!(updates.is_current(second_ready));
    }
}
