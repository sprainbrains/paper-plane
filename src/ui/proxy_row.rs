// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/proxy_row.ui")]
    pub(crate) struct ProxyRow {
        #[template_child]
        pub(super) kind_image: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) selected_check: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) status_checking: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) status_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) edit_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) check_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) remove_button: TemplateChild<gtk::Button>,
        pub(super) item: RefCell<Option<model::ProxyItem>>,
        pub(super) manager: RefCell<Option<model::ProxyManager>>,
        pub(super) handlers: RefCell<Vec<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProxyRow {
        const NAME: &'static str = "PaplProxyRow";
        type Type = super::ProxyRow;
        type ParentType = adw::ActionRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ProxyRow {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();
            self.edit_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.on_edit_clicked()
            ));
            self.check_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.on_check_clicked()
            ));
            self.remove_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.on_remove_clicked()
            ));
            obj.connect_activated(clone!(
                #[weak]
                obj,
                move |_| obj.on_activated()
            ));
        }

        fn dispose(&self) {
            if let Some(item) = self.item.borrow().as_ref() {
                for handler in self.handlers.borrow_mut().drain(..) {
                    item.disconnect(handler);
                }
            }
        }
    }

    impl WidgetImpl for ProxyRow {}

    impl gtk::subclass::prelude::ListBoxRowImpl for ProxyRow {}

    impl adw::subclass::prelude::PreferencesRowImpl for ProxyRow {}

    impl ActionRowImpl for ProxyRow {}
}

glib::wrapper! {
    pub(crate) struct ProxyRow(ObjectSubclass<imp::ProxyRow>)
        @extends gtk::Widget, gtk::ListBoxRow, adw::PreferencesRow, adw::ActionRow,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Actionable;
}

impl ProxyRow {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Binds the row to an item and its manager, replacing any previous
    /// binding (AdwListBox recycles rows).
    pub(crate) fn bind(&self, item: &model::ProxyItem, manager: &model::ProxyManager) {
        let imp = self.imp();

        // Retarget: drop the old subscriptions first.
        if let Some(old) = imp.item.borrow().as_ref() {
            for handler in imp.handlers.borrow_mut().drain(..) {
                old.disconnect(handler);
            }
        }
        *imp.item.borrow_mut() = Some(item.clone());
        *imp.manager.borrow_mut() = Some(manager.clone());

        for pspec in [
            "status-code",
            "latency-ms",
            "enabled",
            "comment",
            "server",
            "port",
        ] {
            let handler = item.connect_notify_local(
                Some(pspec),
                clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move |_, _| obj.update()
                ),
            );
            imp.handlers.borrow_mut().push(handler);
        }

        self.update();
    }

    fn update(&self) {
        let imp = self.imp();
        let Some(item) = imp.item.borrow().clone() else {
            return;
        };

        let address = format!("{}:{}", item.server(), item.port());
        if item.comment().is_empty() {
            self.set_title(&address);
        } else {
            self.set_title(&item.comment());
        }
        self.set_subtitle(&format!("{} · {}", item.kind().tag(), address));
        imp.kind_image.set_icon_name(match item.kind() {
            model::ProxyKind::Mtproto => Some("network-workgroup-symbolic"),
            model::ProxyKind::Socks5 | model::ProxyKind::Http => Some("network-server-symbolic"),
        });

        imp.status_checking.set_visible(item.status_code() == 1);
        match item.status_code() {
            2 => {
                imp.status_label
                    .set_label(&gettext!("Available ({} ms)", item.latency_ms()));
                imp.status_label.set_visible(true);
                imp.status_label.add_css_class("success");
                imp.status_label.remove_css_class("error");
            }
            3 => {
                imp.status_label.set_label(&gettext!("Unavailable"));
                imp.status_label.set_visible(true);
                imp.status_label.add_css_class("error");
                imp.status_label.remove_css_class("success");
            }
            _ => imp.status_label.set_visible(false),
        }

        self.set_activatable(!item.enabled());
        imp.selected_check.set_visible(item.enabled());
        if item.enabled() {
            self.add_css_class("active-proxy");
        } else {
            self.remove_css_class("active-proxy");
        }
    }

    fn on_edit_clicked(&self) {
        let (Some(item), Some(manager)) = (
            self.imp().item.borrow().clone(),
            self.imp().manager.borrow().clone(),
        ) else {
            return;
        };
        let Some(window) = self.root().and_downcast::<gtk::Window>() else {
            return;
        };
        ui::ProxyEditor::for_edit(&window, &manager.client_(), &item);
    }

    fn on_check_clicked(&self) {
        let (Some(item), Some(manager)) = (
            self.imp().item.borrow().clone(),
            self.imp().manager.borrow().clone(),
        ) else {
            return;
        };
        utils::spawn(async move {
            manager.check(&item).await;
        });
    }

    fn on_remove_clicked(&self) {
        let (Some(item), Some(manager)) = (
            self.imp().item.borrow().clone(),
            self.imp().manager.borrow().clone(),
        ) else {
            return;
        };
        utils::spawn(async move {
            if let Err(e) = manager.remove(item.id()).await {
                log::warn!("RemoveProxy failed: {e:?}");
            }
        });
    }

    fn on_activated(&self) {
        let (Some(item), Some(manager)) = (
            self.imp().item.borrow().clone(),
            self.imp().manager.borrow().clone(),
        ) else {
            return;
        };
        if item.enabled() {
            return;
        }
        let id = item.id();
        log::debug!("proxy row activated: enabling id={id}");
        utils::spawn(async move {
            if let Err(e) = manager.enable(id).await {
                log::warn!("EnableProxy failed: {e:?}");
            }
        });
    }
}
