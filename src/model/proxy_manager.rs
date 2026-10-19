// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

//! The per-client proxy list: TDLib-backed storage, statuses, and the
//! active-proxy bookkeeping. Auto-switching lives here too (next task).
//!

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashSet;
use std::time::Duration;

use glib::Properties;
use glib::clone;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::model::proxies::{
    MergeOp, ProxyEntry, ProxyItem, ProxyStatus, entry_from_added, next_proxy_id, plan_merge,
    should_rotate,
};
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = crate::model::proxy_manager::ProxyManager)]
    pub(crate) struct ProxyManager {
        #[property(get, set, construct_only)]
        pub(super) client: glib::WeakRef<model::Client>,
        #[property(get)]
        pub(super) connection_online: Cell<bool>,
        /// The currently enabled proxy's id, 0 when none is enabled.
        #[property(get)]
        pub(super) active_proxy_id: Cell<i64>,
        pub(super) list: RefCell<Vec<ProxyItem>>,
        pub(super) checking: RefCell<HashSet<i64>>,
        pub(super) rotation_source: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProxyManager {
        const NAME: &'static str = "ProxyManager";
        type Type = crate::model::proxy_manager::ProxyManager;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ProxyManager {
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
            if let Some(source) = self.rotation_source.borrow_mut().take() {
                source.remove();
            }
        }
    }

    impl ListModelImpl for ProxyManager {
        fn item_type(&self) -> glib::Type {
            ProxyItem::static_type()
        }

        fn n_items(&self) -> u32 {
            self.list.borrow().len() as u32
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            self.list
                .borrow()
                .get(position as usize)
                .map(|item| item.upcast_ref::<glib::Object>())
                .cloned()
        }
    }
}

glib::wrapper! {
    pub(crate) struct ProxyManager(ObjectSubclass<imp::ProxyManager>)
        @implements gio::ListModel;
}

impl ProxyManager {
    pub(crate) fn new(client: &model::Client) -> Self {
        glib::Object::builder().property("client", client).build()
    }

    weak_getter!(client_, client, model::Client);

    pub(crate) fn set_connection_online(&self, online: bool) {
        let imp = self.imp();
        if imp.connection_online.get() == online {
            return;
        }
        imp.connection_online.set(online);
        self.notify_connection_online();
        if online {
            self.disarm_rotation();
        } else {
            self.arm_rotation();
        }
    }

    pub(crate) fn set_active(&self, id: i64) {
        let imp = self.imp();
        if imp.active_proxy_id.get() == id {
            return;
        }
        imp.active_proxy_id.set(id);
        self.notify_active_proxy_id();
        self.arm_rotation();
    }

    /// Stops the auto-switch deadline timer, if armed.
    pub(crate) fn disarm_rotation(&self) {
        if let Some(source) = self.imp().rotation_source.borrow_mut().take() {
            source.remove();
        }
    }

    /// Arms the auto-switch deadline timer when rotation applies: offline,
    /// an active proxy, at least two candidates and a positive deadline.
    /// Called on every state change that could flip the gate; when the gate
    /// is false any armed timer is removed, so a user who just disabled the
    /// proxy is never rotated back.
    pub(crate) fn arm_rotation(&self) {
        let imp = self.imp();
        let deadline = utils::PaperPlaneSettings::default().int("proxy-auto-switch-deadline");
        let count = imp.list.borrow().len();
        if !should_rotate(
            count,
            deadline,
            imp.connection_online.get(),
            imp.active_proxy_id.get() != 0,
        ) {
            if let Some(source) = imp.rotation_source.borrow_mut().take() {
                source.remove();
            }
            return;
        }
        if imp.rotation_source.borrow().is_some() {
            return; // one deadline window at a time
        }
        let source = glib::timeout_add_local_once(
            Duration::from_secs(deadline as u64),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    obj.imp().rotation_source.borrow_mut().take();
                    obj.rotate_now();
                }
            ),
        );
        *imp.rotation_source.borrow_mut() = Some(source);
    }

    fn rotate_now(&self) {
        if self.active_proxy_id() == 0 {
            return; // the user disabled the proxy while the timer was pending
        }
        let ids: Vec<i64> = self.imp().list.borrow().iter().map(ProxyItem::id).collect();
        let Some(next) = next_proxy_id(&ids, self.active_proxy_id()) else {
            return;
        };
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Err(e) = obj.enable(next).await {
                    log::warn!("Proxy auto-switch failed: {e:?}");
                }
                // Still offline after the switch: try the next candidate
                // after another deadline window.
                if !obj.connection_online() {
                    obj.arm_rotation();
                }
            }
        ));
    }

    pub(crate) async fn refresh(&self) {
        let client_id = self.client_().id();
        match tdlib::functions::get_proxies(client_id).await {
            Ok(tdlib::enums::AddedProxies::AddedProxies(list)) => {
                let entries = list.proxies.iter().map(entry_from_added).collect();
                self.apply_entries(entries);
            }
            Err(e) => log::warn!("GetProxies failed: {e:?}"),
        }
    }

    pub(crate) async fn add(
        &self,
        entry: &ProxyEntry,
        enable: bool,
    ) -> Result<i64, tdlib::types::Error> {
        let client_id = self.client_().id();
        let added = tdlib::functions::add_proxy(
            entry.tdlib_proxy(),
            enable,
            entry.comment.clone(),
            client_id,
        )
        .await?;
        self.refresh().await;
        Ok(match added {
            tdlib::enums::AddedProxy::AddedProxy(added) => i64::from(added.id),
        })
    }

    pub(crate) async fn edit(
        &self,
        entry: &ProxyEntry,
        enable: bool,
    ) -> Result<(), tdlib::types::Error> {
        let client_id = self.client_().id();
        tdlib::functions::edit_proxy(
            entry.id as i32,
            entry.tdlib_proxy(),
            enable,
            entry.comment.clone(),
            client_id,
        )
        .await?;
        self.refresh().await;
        Ok(())
    }

    pub(crate) async fn enable(&self, id: i64) -> Result<(), tdlib::types::Error> {
        let client_id = self.client_().id();
        tdlib::functions::enable_proxy(id as i32, client_id).await?;
        self.refresh().await;
        Ok(())
    }

    pub(crate) async fn disable(&self) -> Result<(), tdlib::types::Error> {
        tdlib::functions::disable_proxy(self.client_().id()).await?;
        self.refresh().await;
        Ok(())
    }

    pub(crate) async fn remove(&self, id: i64) -> Result<(), tdlib::types::Error> {
        tdlib::functions::remove_proxy(id as i32, self.client_().id()).await?;
        self.refresh().await;
        Ok(())
    }

    /// Probes one proxy with `testProxy` and records its status and latency.
    pub(crate) async fn check(&self, item: &ProxyItem) {
        let id = item.id();
        if !self.imp().checking.borrow_mut().insert(id) {
            return; // a check is already running for this proxy
        }
        item.set_status(ProxyStatus::Checking);

        let client_id = self.client_().id();
        let start = glib::monotonic_time();
        let result = tdlib::functions::test_proxy(item.tdlib_proxy(), 0, 5.0, client_id).await;
        let latency_ms = ((glib::monotonic_time() - start) / 1000) as i32;

        item.set_status(if result.is_ok() {
            ProxyStatus::Ok(latency_ms)
        } else {
            ProxyStatus::Failed
        });
        self.imp().checking.borrow_mut().remove(&id);
    }

    /// Probes every proxy in turn so the list shows fresh latencies when it
    /// opens. Sequential on purpose: parallel probes would skew each other.
    pub(crate) async fn check_all(&self) {
        let items: Vec<ProxyItem> = self.imp().list.borrow().clone();
        for item in items {
            self.check(&item).await;
        }
    }

    /// Replaces storage from an incoming snapshot, preserving item identity
    /// and emitting one exactly-ranged `items-changed`.
    fn apply_entries(&self, entries: Vec<ProxyEntry>) {
        let old: Vec<ProxyItem> = std::mem::take(&mut *self.imp().list.borrow_mut());

        // Field updates ride on the planned merge so both stay honest.
        let existing_ids: Vec<i64> = old.iter().map(ProxyItem::id).collect();
        for op in plan_merge(&existing_ids, &entries) {
            if let MergeOp::Update(index, entry) = op {
                old[index].apply(&entry);
            }
        }

        let mut new_list: Vec<ProxyItem> = Vec::with_capacity(entries.len());
        for entry in &entries {
            if let Some(existing) = old.iter().find(|item| item.id() == entry.id) {
                new_list.push(existing.clone());
            } else {
                new_list.push(ProxyItem::new(entry));
            }
        }

        // Cover only the changed range: trim the identical prefix and suffix.
        let mut prefix = 0;
        while prefix < old.len() && prefix < new_list.len() && old[prefix] == new_list[prefix] {
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix < old.len() - prefix
            && suffix < new_list.len() - prefix
            && old[old.len() - 1 - suffix] == new_list[new_list.len() - 1 - suffix]
        {
            suffix += 1;
        }

        let removed = (old.len() - prefix - suffix) as u32;
        let added = (new_list.len() - prefix - suffix) as u32;
        *self.imp().list.borrow_mut() = new_list;
        if removed > 0 || added > 0 {
            self.items_changed(prefix as u32, removed, added);
        }

        self.set_active(
            entries
                .iter()
                .find(|entry| entry.enabled)
                .map(|entry| entry.id)
                .unwrap_or(0),
        );
        // The candidate count may have just reached two.
        self.arm_rotation();
    }
}
