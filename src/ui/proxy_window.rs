// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

use std::cell::Cell;
use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;

use crate::model;
use crate::ui;
use crate::utils;

/// Deadlines offered for automatic proxy switching, in seconds.
const DEADLINES: &[i32] = &[0, 5, 10, 15, 30, 60];

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/proxy_window.ui")]
    pub(crate) struct ProxyWindow {
        pub(super) client: OnceCell<model::Client>,
        pub(super) manager: OnceCell<model::ProxyManager>,
        /// Set while the code syncs the radio rows from the model, so the
        /// resulting activation handlers do not echo back into TDLib.
        pub(super) syncing: Cell<bool>,
        #[template_child]
        pub(super) none_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub(super) none_check: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) use_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub(super) use_check: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) deadline_combo_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) proxy_list: TemplateChild<gtk::ListBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProxyWindow {
        const NAME: &'static str = "PaplProxyWindow";
        type Type = super::ProxyWindow;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action("proxy-window.add", None, move |widget, _, _| {
                // The editor dialog arrives with the next task on this branch.
                utils::show_toast(widget, gettext("Adding proxies is not available yet"));
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ProxyWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let Some(client) = self.client.get() else {
                return;
            };
            let manager = client.proxies();
            let _ = self.manager.set(manager.clone());

            let obj = &*self.obj();

            self.proxy_list.bind_model(
                Some(manager.upcast_ref::<gtk::gio::ListModel>()),
                clone!(
                    #[weak]
                    manager,
                    #[upgrade_or_panic]
                    move |item| {
                        let item = item.downcast_ref::<model::ProxyItem>().unwrap();
                        let row = ui::ProxyRow::new();
                        row.bind(item, &manager);
                        row.upcast()
                    }
                ),
            );

            // Radio rows mirror the active proxy; user activation drives TDLib.
            manager.connect_active_proxy_id_notify(clone!(
                #[weak]
                obj,
                move |_| obj.sync_radio_rows()
            ));
            self.none_row.connect_activated(clone!(
                #[weak]
                obj,
                move |_| obj.on_none_activated()
            ));
            self.use_row.connect_activated(clone!(
                #[weak]
                obj,
                move |_| obj.on_use_activated()
            ));

            // Deadline combo ↔ gsettings.
            let strings = gtk::StringList::new(&[
                &gettext("Off"),
                &gettext("5 seconds"),
                &gettext("10 seconds"),
                &gettext("15 seconds"),
                &gettext("30 seconds"),
                &gettext("60 seconds"),
            ]);
            self.deadline_combo_row.set_model(Some(&strings));
            let settings = utils::PaperPlaneSettings::default();
            let current = settings.int("proxy-auto-switch-deadline");
            if let Some(index) = DEADLINES.iter().position(|d| *d == current) {
                self.deadline_combo_row.set_selected(index as u32);
            }
            self.deadline_combo_row.connect_selected_notify(clone!(
                #[weak]
                obj,
                move |combo| {
                    let Some(&deadline) = DEADLINES.get(combo.selected() as usize) else {
                        return;
                    };
                    let settings = utils::PaperPlaneSettings::default();
                    settings
                        .set_int("proxy-auto-switch-deadline", deadline)
                        .unwrap();
                    if let Some(manager) = obj.imp().manager.get().cloned() {
                        if deadline == 0 {
                            manager.disarm_rotation();
                        } else {
                            manager.arm_rotation();
                        }
                    }
                }
            ));

            utils::spawn(clone!(
                #[weak]
                manager,
                #[weak]
                obj,
                async move {
                    manager.refresh().await;
                    obj.sync_radio_rows();
                }
            ));

            obj.sync_radio_rows();
        }
    }

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
        // `new` is the only constructor; the cells are always fresh here.
        let _ = window.imp().client.set(client.clone());
        window.present(Some(parent));
        window
    }

    /// Whether the "Use Proxy" radio is selected. The editor task enables
    /// newly added proxies when this is true.
    // `allow(dead_code)`: consumed by the editor task with the rest of the
    // branch's temporary allows.
    #[allow(dead_code)]
    pub(crate) fn use_proxy_active(&self) -> bool {
        let imp = self.imp();
        imp.use_check.is_visible()
    }

    /// Mirrors the model's active proxy into the radio rows.
    fn sync_radio_rows(&self) {
        let imp = self.imp();
        let Some(manager) = imp.manager.get() else {
            return;
        };
        let active = manager.active_proxy_id() != 0;
        imp.use_check.set_visible(active);
        imp.none_check.set_visible(!active);
    }

    fn on_none_activated(&self) {
        let imp = self.imp();
        if imp.syncing.get() || imp.none_check.is_visible() {
            return;
        }
        let Some(manager) = imp.manager.get() else {
            return;
        };
        if manager.active_proxy_id() == 0 {
            return;
        }
        imp.syncing.set(true);
        utils::spawn(clone!(
            #[weak]
            manager,
            #[weak(rename_to = as_this)]
            self,
            async move {
                if let Err(e) = manager.disable().await {
                    log::warn!("DisableProxy failed: {e:?}");
                }
                as_this.imp().syncing.set(false);
            }
        ));
    }

    fn on_use_activated(&self) {
        let imp = self.imp();
        if imp.syncing.get() || imp.use_check.is_visible() {
            return;
        }
        let Some(manager) = imp.manager.get() else {
            return;
        };
        if manager.active_proxy_id() != 0 {
            return;
        }
        // Nothing to enable yet: snap back with an explanation.
        let Some(first) = manager.item(0).and_downcast::<model::ProxyItem>() else {
            utils::show_toast(self, gettext("Add a proxy first"));
            return;
        };
        let id = first.id();
        imp.syncing.set(true);
        utils::spawn(clone!(
            #[weak]
            manager,
            #[weak(rename_to = as_this)]
            self,
            async move {
                if let Err(e) = manager.enable(id).await {
                    log::warn!("EnableProxy failed: {e:?}");
                }
                as_this.imp().syncing.set(false);
            }
        ));
    }
}
