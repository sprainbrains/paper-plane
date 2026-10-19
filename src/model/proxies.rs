// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

//! Proxy model: item, kinds, and pure merge/rotation planning.
//!
//! `allow(dead_code)`: the manager and UI that consume this land in the
//! next tasks on this branch; drop the attribute once they do.
#![allow(dead_code)]

use std::cell::Cell;
use std::cell::RefCell;

use glib::Properties;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::utils;

/// The protocol a proxy speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, glib::Enum)]
#[enum_type(name = "PaplProxyKind")]
pub(crate) enum ProxyKind {
    #[default]
    Mtproto,
    Socks5,
    Http,
}

impl ProxyKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::Mtproto => "MTProto",
            Self::Socks5 => "SOCKS5",
            Self::Http => "HTTP",
        }
    }
}

impl From<utils::proxy_link::ProxyKindTag> for ProxyKind {
    fn from(tag: utils::proxy_link::ProxyKindTag) -> Self {
        match tag {
            utils::proxy_link::ProxyKindTag::Mtproto => Self::Mtproto,
            utils::proxy_link::ProxyKindTag::Socks5 => Self::Socks5,
            utils::proxy_link::ProxyKindTag::Http => Self::Http,
        }
    }
}

/// Liveness of a proxy as last observed by the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyStatus {
    Unknown,
    Checking,
    Ok(i32),
    Failed,
}

/// Plain data snapshot of one TDLib proxy entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyEntry {
    pub(crate) id: i64,
    pub(crate) server: String,
    pub(crate) port: i32,
    pub(crate) kind: ProxyKind,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) secret: String,
    pub(crate) comment: String,
    pub(crate) enabled: bool,
}

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default)]
    #[properties(wrapper_type = super::ProxyItem)]
    pub(crate) struct ProxyItem {
        #[property(get, set, construct_only)]
        pub(super) id: Cell<i64>,
        #[property(get)]
        pub(super) server: RefCell<String>,
        #[property(get)]
        pub(super) port: Cell<i32>,
        /// Protocol tag for display ("MTProto"/"SOCKS5"/"HTTP"); the Rust
        /// enum itself lives in `kind` below (the Properties derive has no
        /// enum support in this glib-rs version).
        #[property(get)]
        pub(super) kind_tag: RefCell<String>,
        pub(super) kind: Cell<ProxyKind>,
        #[property(get)]
        pub(super) username: RefCell<String>,
        #[property(get)]
        pub(super) password: RefCell<String>,
        #[property(get)]
        pub(super) secret: RefCell<String>,
        #[property(get)]
        pub(super) comment: RefCell<String>,
        #[property(get)]
        pub(super) enabled: Cell<bool>,
        #[property(get)]
        pub(super) latency_ms: Cell<i32>,
        /// 0 unknown, 1 checking, 2 ok, 3 failed.
        #[property(get)]
        pub(super) status_code: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProxyItem {
        const NAME: &'static str = "ProxyItem";
        type Type = super::ProxyItem;
    }

    impl ObjectImpl for ProxyItem {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }
}

glib::wrapper! {
    pub(crate) struct ProxyItem(ObjectSubclass<imp::ProxyItem>);
}

impl ProxyItem {
    pub(crate) fn new(entry: &ProxyEntry) -> Self {
        let obj: Self = glib::Object::builder().property("id", entry.id).build();
        obj.apply(entry);
        obj
    }

    /// Applies an incoming snapshot, notifying only what changed.
    pub(crate) fn apply(&self, entry: &ProxyEntry) {
        let imp = self.imp();

        if imp.server.borrow().as_str() != entry.server {
            *imp.server.borrow_mut() = entry.server.clone();
            self.notify_server();
        }
        if imp.port.get() != entry.port {
            imp.port.set(entry.port);
            self.notify_port();
        }
        let tag = entry.kind.tag().to_string();
        if imp.kind.get() != entry.kind {
            imp.kind.set(entry.kind);
        }
        if *imp.kind_tag.borrow() != tag {
            *imp.kind_tag.borrow_mut() = tag;
            self.notify_kind_tag();
        }
        if imp.username.borrow().as_str() != entry.username {
            *imp.username.borrow_mut() = entry.username.clone();
            self.notify_username();
        }
        if imp.password.borrow().as_str() != entry.password {
            *imp.password.borrow_mut() = entry.password.clone();
            self.notify_password();
        }
        if imp.secret.borrow().as_str() != entry.secret {
            *imp.secret.borrow_mut() = entry.secret.clone();
            self.notify_secret();
        }
        if imp.comment.borrow().as_str() != entry.comment {
            *imp.comment.borrow_mut() = entry.comment.clone();
            self.notify_comment();
        }
        if imp.enabled.get() != entry.enabled {
            imp.enabled.set(entry.enabled);
            self.notify_enabled();
        }
    }

    pub(crate) fn entry(&self) -> ProxyEntry {
        let imp = self.imp();
        ProxyEntry {
            id: imp.id.get(),
            server: imp.server.borrow().clone(),
            port: imp.port.get(),
            kind: imp.kind.get(),
            username: imp.username.borrow().clone(),
            password: imp.password.borrow().clone(),
            secret: imp.secret.borrow().clone(),
            comment: imp.comment.borrow().clone(),
            enabled: imp.enabled.get(),
        }
    }

    pub(crate) fn set_status(&self, status: ProxyStatus) {
        let (code, latency) = match status {
            ProxyStatus::Unknown => (0, 0),
            ProxyStatus::Checking => (1, 0),
            ProxyStatus::Ok(ms) => (2, ms),
            ProxyStatus::Failed => (3, 0),
        };
        let imp = self.imp();
        if imp.status_code.get() != code {
            imp.status_code.set(code);
            self.notify_status_code();
        }
        if imp.latency_ms.get() != latency {
            imp.latency_ms.set(latency);
            self.notify_latency_ms();
        }
    }

    pub(crate) fn status(&self) -> ProxyStatus {
        match self.imp().status_code.get() {
            1 => ProxyStatus::Checking,
            2 => ProxyStatus::Ok(self.imp().latency_ms.get()),
            3 => ProxyStatus::Failed,
            _ => ProxyStatus::Unknown,
        }
    }

    /// The TDLib request payload for this proxy.
    pub(crate) fn tdlib_proxy(&self) -> tdlib::types::Proxy {
        let imp = self.imp();
        let r#type = match imp.kind.get() {
            ProxyKind::Socks5 => tdlib::enums::ProxyType::Socks5(tdlib::types::ProxyTypeSocks5 {
                username: imp.username.borrow().clone(),
                password: imp.password.borrow().clone(),
            }),
            ProxyKind::Http => tdlib::enums::ProxyType::Http(tdlib::types::ProxyTypeHttp {
                username: imp.username.borrow().clone(),
                password: imp.password.borrow().clone(),
                http_only: false,
            }),
            ProxyKind::Mtproto => {
                tdlib::enums::ProxyType::Mtproto(tdlib::types::ProxyTypeMtproto {
                    secret: imp.secret.borrow().clone(),
                })
            }
        };
        tdlib::types::Proxy {
            server: imp.server.borrow().clone(),
            port: imp.port.get(),
            r#type,
        }
    }
}

/// One structural change from an existing proxy list to the incoming one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeOp {
    /// Apply fresh fields to the existing item at this pre-merge index.
    Update(usize, ProxyEntry),
    /// New proxy, appended in incoming order.
    Insert(ProxyEntry),
    /// Existing index whose id is gone.
    Remove(usize),
}

pub(crate) fn entry_from_added(added: &tdlib::types::AddedProxy) -> ProxyEntry {
    use tdlib::enums::ProxyType;

    let (kind, username, password, secret) = match &added.proxy.r#type {
        ProxyType::Socks5(t) => (
            ProxyKind::Socks5,
            t.username.clone(),
            t.password.clone(),
            String::new(),
        ),
        ProxyType::Http(t) => (
            ProxyKind::Http,
            t.username.clone(),
            t.password.clone(),
            String::new(),
        ),
        ProxyType::Mtproto(t) => (
            ProxyKind::Mtproto,
            String::new(),
            String::new(),
            t.secret.clone(),
        ),
    };

    ProxyEntry {
        id: i64::from(added.id),
        server: added.proxy.server.clone(),
        port: added.proxy.port,
        kind,
        username,
        password,
        secret,
        comment: added.comment.clone(),
        enabled: added.is_enabled,
    }
}

/// Plans the transition from the list with `existing_ids` to `incoming`.
/// All indices refer to the pre-merge existing list.
pub(crate) fn plan_merge(existing_ids: &[i64], incoming: &[ProxyEntry]) -> Vec<MergeOp> {
    let mut ops = Vec::new();

    for (index, id) in existing_ids.iter().enumerate() {
        match incoming.iter().position(|entry| entry.id == *id) {
            Some(_) => {}
            None => ops.push(MergeOp::Remove(index)),
        }
    }

    for e in incoming {
        match existing_ids.iter().position(|id| *id == e.id) {
            Some(index) => ops.push(MergeOp::Update(index, e.clone())),
            None => ops.push(MergeOp::Insert(e.clone())),
        }
    }

    ops
}

/// The id to enable when rotation should move away from `current`.
/// `None` when there is nothing to rotate to; an unknown `current`
/// (e.g. the active proxy left the list) starts from the first entry.
pub(crate) fn next_proxy_id(ids: &[i64], current: i64) -> Option<i64> {
    if ids.len() < 2 {
        return None;
    }
    Some(match ids.iter().position(|id| *id == current) {
        Some(index) => ids[(index + 1) % ids.len()],
        None => ids[0],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added_mtproto(id: i32, enabled: bool, comment: &str) -> tdlib::types::AddedProxy {
        tdlib::types::AddedProxy {
            id,
            last_used_date: 0,
            is_enabled: enabled,
            comment: comment.to_string(),
            proxy: tdlib::types::Proxy {
                server: "1.2.3.4".to_string(),
                port: 443,
                r#type: tdlib::enums::ProxyType::Mtproto(tdlib::types::ProxyTypeMtproto {
                    secret: "ee01".to_string(),
                }),
            },
        }
    }

    fn entry(id: i64) -> ProxyEntry {
        ProxyEntry {
            id,
            server: "1.2.3.4".into(),
            port: 443,
            kind: ProxyKind::Mtproto,
            username: String::new(),
            password: String::new(),
            secret: "ee01".into(),
            comment: String::new(),
            enabled: false,
        }
    }

    #[test]
    fn entry_from_added_maps_mtproto() {
        let e = entry_from_added(&added_mtproto(7, true, "home"));
        assert_eq!(e.id, 7);
        assert_eq!(e.server, "1.2.3.4");
        assert_eq!(e.port, 443);
        assert_eq!(e.kind, ProxyKind::Mtproto);
        assert_eq!(e.secret, "ee01");
        assert_eq!(e.comment, "home");
        assert!(e.enabled);
    }

    #[test]
    fn entry_from_added_maps_socks5_credentials() {
        let mut added = added_mtproto(8, false, "");
        added.proxy.r#type = tdlib::enums::ProxyType::Socks5(tdlib::types::ProxyTypeSocks5 {
            username: "u".into(),
            password: "p".into(),
        });
        let e = entry_from_added(&added);
        assert_eq!(e.kind, ProxyKind::Socks5);
        assert_eq!(e.username, "u");
        assert_eq!(e.password, "p");
        assert_eq!(e.secret, "");
    }

    #[test]
    fn plan_merge_removes_updates_and_inserts_in_incoming_order() {
        // existing [10, 11]; incoming [11, 12]: 10 goes away, 11 stays, 12 is new.
        let ops = plan_merge(&[10, 11], &[entry(11), entry(12)]);
        assert_eq!(
            ops,
            vec![
                MergeOp::Remove(0),
                MergeOp::Update(1, entry(11)),
                MergeOp::Insert(entry(12)),
            ]
        );
    }

    #[test]
    fn plan_merge_empty_incoming_removes_all() {
        let ops = plan_merge(&[1, 2], &[]);
        assert_eq!(ops, vec![MergeOp::Remove(0), MergeOp::Remove(1)]);
    }

    #[test]
    fn next_proxy_id_rotates_and_guards() {
        assert_eq!(next_proxy_id(&[], 1), None);
        assert_eq!(next_proxy_id(&[5], 5), None);
        assert_eq!(next_proxy_id(&[5, 9], 5), Some(9));
        assert_eq!(next_proxy_id(&[5, 9], 9), Some(5));
        assert_eq!(next_proxy_id(&[5, 9], 42), Some(5));
    }
}
