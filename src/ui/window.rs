use adw::prelude::AdwDialogExt;
use adw::subclass::prelude::*;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::Application;
use crate::config;
use crate::model;
use crate::types::ChatId;
use crate::types::ClientId;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/window.ui")]
    pub(crate) struct Window {
        pub(super) settings: utils::PaperPlaneSettings,
        #[template_child]
        pub(super) client_manager_view: TemplateChild<ui::ClientManagerView>,
        #[template_child]
        pub(super) media_viewer: TemplateChild<ui::MediaViewer>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "PaplWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
            klass.install_action("win.show-shortcuts", None, |window, _, _| {
                window.show_shortcuts();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();

            // Devel profile
            if config::PROFILE == "Devel" {
                obj.add_css_class("devel");
            }

            // Load latest window state
            obj.load_window_size();
        }
    }

    impl WidgetImpl for Window {}

    impl WindowImpl for Window {
        // Save window state on delete event
        fn close_request(&self) -> glib::Propagation {
            let obj = self.obj();

            if let Err(err) = obj.save_window_size() {
                log::warn!("Failed to save window state, {err}");
            }

            // Drop this window's entries from the cross-window open-chat registry.
            if let Some(app) = obj.application().and_downcast::<Application>() {
                app.unregister_window(&obj);
            }

            // Pass close request on to the parent
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}

    #[gtk::template_callbacks]
    impl Window {
        #[template_callback]
        fn on_key_pressed(
            &self,
            key: gdk::Key,
            _: u32,
            modifier: gdk::ModifierType,
            _: &gtk::EventControllerKey,
        ) -> glib::Propagation {
            if key == gdk::Key::w && modifier == gdk::ModifierType::CONTROL_MASK {
                self.obj().close();
            }

            glib::Propagation::Proceed
        }

        #[template_callback]
        fn on_notify_is_active(&self) {
            self.obj().client_manager_view().set_active_client_online();
        }
    }
}

glib::wrapper! {
    pub(crate) struct Window(ObjectSubclass<imp::Window>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager, gio::ActionGroup, gio::ActionMap;
}

impl Window {
    fn show_shortcuts(&self) {
        let builder = gtk::Builder::from_resource("/app/drey/paper-plane/ui/shortcuts.ui");
        let dialog = builder
            .object::<adw::Dialog>("shortcuts_dialog")
            .expect("shortcuts resource must contain an AdwShortcutsDialog");
        dialog.present(Some(self));
    }

    pub(crate) fn new(app: &Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    /// Wires this window's client-manager view to the shared manager and shows the
    /// requested initial account/chat. Must be called once, right after `new`.
    pub(crate) fn init_session(&self, initial: ui::InitialView) {
        let app = self.application().and_downcast::<Application>().unwrap();
        self.client_manager_view().init(&app.client_manager(), initial);
    }

    pub(crate) fn select_chat(&self, client_id: ClientId, chat_id: ChatId) {
        self.client_manager_view().select_chat(client_id, chat_id);
    }

    pub(crate) fn client_manager_view(&self) -> &ui::ClientManagerView {
        &self.imp().client_manager_view
    }

    pub(crate) fn open_media_viewer(&self, message: &model::Message) {
        self.imp().media_viewer.open(message);
    }

    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let imp = self.imp();

        let (width, height) = self.default_size();
        imp.settings.set_int("window-width", width)?;
        imp.settings.set_int("window-height", height)?;

        imp.settings
            .set_boolean("is-maximized", self.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let imp = self.imp();

        let width = imp.settings.int("window-width");
        let height = imp.settings.int("window-height");
        self.set_default_size(width, height);

        let is_maximized = imp.settings.boolean("is-maximized");
        if is_maximized {
            self.maximize();
        }
    }
}
