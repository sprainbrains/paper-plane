// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::CompositeTemplate;
use gtk::glib;

use crate::model;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/proxy_window.ui")]
    pub(crate) struct ProxyWindow {
        pub(super) client: OnceCell<model::Client>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProxyWindow {
        const NAME: &'static str = "PaplProxyWindow";
        type Type = super::ProxyWindow;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ProxyWindow {}

    impl WidgetImpl for ProxyWindow {}

    impl AdwDialogImpl for ProxyWindow {}

    impl PreferencesDialogImpl for ProxyWindow {}
}

glib::wrapper! {
    pub(crate) struct ProxyWindow(ObjectSubclass<imp::ProxyWindow>)
        @extends gtk::Widget, adw::Dialog, adw::PreferencesDialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ProxyWindow {
    pub(crate) fn new(parent: &gtk::Window, client: &model::Client) -> Self {
        let window: Self = glib::Object::new();
        // `new` is the only constructor; the cell is always fresh here.
        let _ = window.imp().client.set(client.clone());
        window.present(Some(parent));
        window
    }

    // `allow(dead_code)`: consumed by the window content in the next task;
    // drop together with the `proxy_manager.rs` allows.
    #[allow(dead_code)]
    pub(crate) fn client_(&self) -> Option<model::Client> {
        self.imp().client.get().cloned()
    }
}
