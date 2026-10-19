// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;

use crate::model;
use crate::utils;

/// Kind order matching the editor's combo model.
const KINDS: [model::ProxyKind; 3] = [
    model::ProxyKind::Mtproto,
    model::ProxyKind::Socks5,
    model::ProxyKind::Http,
];

/// Whether the current form fields describe a usable proxy.
fn validate(server: &str, port: &str, kind: model::ProxyKind, secret: &str) -> bool {
    if server.trim().is_empty() {
        return false;
    }
    match port.parse::<u16>() {
        Ok(p) if p > 0 => {}
        _ => return false,
    }
    match kind {
        model::ProxyKind::Mtproto => {
            let secret = secret.trim();
            !secret.is_empty()
                && secret.len().is_multiple_of(2)
                && secret.bytes().all(|b| b.is_ascii_hexdigit())
        }
        model::ProxyKind::Socks5 | model::ProxyKind::Http => true,
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/proxy_editor.ui")]
    pub(crate) struct ProxyEditor {
        pub(super) client: OnceCell<model::Client>,
        /// The proxy being edited, `None` while adding.
        pub(super) editing: RefCell<Option<model::ProxyItem>>,
        /// Whether a saved proxy should become the active one.
        pub(super) enable_on_save: Cell<bool>,
        #[template_child]
        pub(super) title: TemplateChild<adw::WindowTitle>,
        #[template_child]
        pub(super) save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) kind_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub(super) server_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub(super) port_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub(super) name_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub(super) secret_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub(super) username_row: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub(super) password_row: TemplateChild<adw::EntryRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProxyEditor {
        const NAME: &'static str = "PaplProxyEditor";
        type Type = super::ProxyEditor;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ProxyEditor {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();
            self.kind_row.connect_selected_notify(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.update_field_visibility();
                    obj.update_save_sensitivity();
                }
            ));
            for row in [
                &self.server_row,
                &self.port_row,
                &self.secret_row,
                &self.username_row,
                &self.password_row,
            ] {
                row.connect_text_notify(clone!(
                    #[weak]
                    obj,
                    move |_| obj.update_save_sensitivity()
                ));
            }
            self.save_button.connect_clicked(clone!(
                #[weak]
                obj,
                move |_| obj.save()
            ));

            obj.update_field_visibility();
            obj.update_save_sensitivity();
        }
    }

    impl WidgetImpl for ProxyEditor {}

    impl AdwDialogImpl for ProxyEditor {}
}

glib::wrapper! {
    pub(crate) struct ProxyEditor(ObjectSubclass<imp::ProxyEditor>)
        @extends gtk::Widget, adw::Dialog,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ProxyEditor {
    /// Opens the editor to add a new proxy. The new proxy becomes active
    /// exactly when one already is (the window's "Use Proxy" state).
    pub(crate) fn for_add(parent: &gtk::Window, client: &model::Client) {
        let editor = Self::new_window(client);
        editor
            .imp()
            .enable_on_save
            .set(client.proxies().active_proxy_id() != 0);
        editor.present(Some(parent));
    }

    /// Opens the editor prefilled from a `t.me/proxy` link; saving activates it.
    // `allow(dead_code)`: the message-link task wires this next; remove with
    // the branch's other temporary allows.
    #[allow(dead_code)]
    pub(crate) fn for_link(
        parent: &gtk::Window,
        client: &model::Client,
        link: &utils::proxy_link::ProxyLink,
    ) {
        let editor = Self::new_window(client);
        {
            let imp = editor.imp();
            imp.enable_on_save.set(true);
            imp.title
                .set_title(&link.name.clone().unwrap_or_else(|| gettext("Add Proxy")));
            let kind = link.kind.into();
            imp.kind_row
                .set_selected(KINDS.iter().position(|k| *k == kind).unwrap_or(0) as u32);
            imp.server_row.set_text(&link.server);
            imp.port_row.set_text(&link.port.to_string());
            imp.secret_row.set_text(&link.secret);
            imp.username_row.set_text(&link.username);
            imp.password_row.set_text(&link.password);
            if let Some(name) = link.name.as_deref() {
                imp.name_row.set_text(name);
            }
        }
        editor.present(Some(parent));
    }

    /// Opens the editor prefilled from an existing proxy.
    pub(crate) fn for_edit(parent: &gtk::Window, client: &model::Client, item: &model::ProxyItem) {
        let editor = Self::new_window(client);
        {
            let imp = editor.imp();
            imp.editing.replace(Some(item.clone()));
            imp.enable_on_save.set(item.enabled());
            imp.title.set_title(&gettext("Edit Proxy"));
            imp.kind_row
                .set_selected(KINDS.iter().position(|k| *k == item.kind()).unwrap_or(0) as u32);
            imp.server_row.set_text(&item.server());
            imp.port_row.set_text(&item.port().to_string());
            imp.name_row.set_text(&item.comment());
            imp.secret_row.set_text(&item.secret());
            imp.username_row.set_text(&item.username());
            imp.password_row.set_text(&item.password());
        }
        editor.present(Some(parent));
    }

    fn new_window(client: &model::Client) -> Self {
        let editor: Self = glib::Object::new();
        let _ = editor.imp().client.set(client.clone());
        editor
    }

    fn update_field_visibility(&self) {
        let imp = self.imp();
        let kind = self.selected_kind();
        imp.secret_row
            .set_visible(kind == model::ProxyKind::Mtproto);
        let creds = kind != model::ProxyKind::Mtproto;
        imp.username_row.set_visible(creds);
        imp.password_row.set_visible(creds);
    }

    fn selected_kind(&self) -> model::ProxyKind {
        KINDS
            .get(self.imp().kind_row.selected() as usize)
            .copied()
            .unwrap_or(model::ProxyKind::Mtproto)
    }

    fn update_save_sensitivity(&self) {
        let imp = self.imp();
        imp.save_button.set_sensitive(validate(
            &imp.server_row.text(),
            &imp.port_row.text(),
            self.selected_kind(),
            &imp.secret_row.text(),
        ));
    }

    fn save(&self) {
        let imp = self.imp();
        let Some(client) = imp.client.get().cloned() else {
            return;
        };
        let manager = client.proxies();

        let entry = model::ProxyEntry {
            id: imp
                .editing
                .borrow()
                .as_ref()
                .map(|item| item.id())
                .unwrap_or(0),
            server: imp.server_row.text().trim().to_string(),
            port: imp.port_row.text().trim().parse().unwrap_or(0),
            kind: self.selected_kind(),
            username: imp.username_row.text().into(),
            password: imp.password_row.text().into(),
            secret: imp.secret_row.text().trim().to_string(),
            comment: imp.name_row.text().into(),
            enabled: false,
        };
        let enable = imp.enable_on_save.get();
        let editing = imp.editing.borrow().clone();

        utils::spawn(clone!(
            #[weak(rename_to = as_this)]
            self,
            async move {
                let result = match &editing {
                    Some(_) => manager.edit(&entry, enable).await,
                    None => manager.add(&entry, enable).await.map(|_| ()),
                };
                match result {
                    Ok(()) => {
                        as_this.close();
                    }
                    Err(e) => {
                        log::warn!("SaveProxy failed: {e:?}");
                        utils::show_toast(&as_this, gettext("Could not save the proxy"));
                    }
                }
            }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProxyKind;

    #[test]
    fn validate_requires_server_and_port() {
        assert!(!validate("", "443", ProxyKind::Mtproto, "ee01"));
        assert!(!validate(
            "proxy.example.com",
            "",
            ProxyKind::Mtproto,
            "ee01"
        ));
        assert!(!validate(
            "proxy.example.com",
            "abc",
            ProxyKind::Mtproto,
            "ee01"
        ));
        assert!(!validate(
            "proxy.example.com",
            "0",
            ProxyKind::Mtproto,
            "ee01"
        ));
        assert!(!validate(
            "proxy.example.com",
            "65536",
            ProxyKind::Mtproto,
            "ee01"
        ));
    }

    #[test]
    fn validate_mtproto_needs_even_hex_secret() {
        assert!(!validate("h", "443", ProxyKind::Mtproto, ""));
        assert!(!validate("h", "443", ProxyKind::Mtproto, "abc"));
        assert!(!validate("h", "443", ProxyKind::Mtproto, "zz01"));
        assert!(validate("h", "443", ProxyKind::Mtproto, "eeF0AB01"));
    }

    #[test]
    fn validate_socks5_and_http_need_no_credentials() {
        assert!(validate("h", "1080", ProxyKind::Socks5, ""));
        assert!(validate("h", "3128", ProxyKind::Http, ""));
        assert!(validate("h", "1080", ProxyKind::Socks5, "ignored"));
    }
}
