// Copyright 2020 - developers of the `grammers` project.
// Copyright 2021 - developers of the `tdlib-rs` project.
// Copyright 2024 - developers of the `tgt` and `tdlib-rs` projects.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may be copied, modified, or distributed
// except according to those terms.
//
// Switched from the deprecated global `td_create_client_id`/`td_send`/`td_receive`
// API to the per-instance `td_json_client_*` API: the global instance is destroyed
// by static destructors at process exit, which aborts inside TDLib when any
// connection was still alive. Per-instance clients can be destroyed cleanly while
// the process is running. TDLib assigns no cross-instance client ids, so this
// crate mints its own and stamps them onto responses as `@client_id`.
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn td_json_client_create() -> *mut c_void;
    fn td_json_client_destroy(client: *mut c_void);
    fn td_json_client_send(client: *mut c_void, request: *const c_char);
    fn td_json_client_receive(client: *mut c_void, timeout: c_double) -> *const c_char;
}

/// Opaque `TdJson` handle. TDLib serialises access internally, so the only
/// hazard is destroying a handle while another thread polls it; the maps
/// below guard against that.
#[derive(Copy, Clone)]
struct Handle(*mut c_void);

unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

/// Live clients, keyed by a crate-assigned id. `receive` polls each in turn
/// under the read lock; `destroy_client` takes the write lock, so a handle is
/// never polled after it has been retired.
static HANDLES: RwLock<BTreeMap<c_int, Handle>> = RwLock::new(BTreeMap::new());

/// Handles retired while the receive loop may still be running. TDLib's own
/// destructor pumps the "client released" response off the handle, and a
/// concurrent poller could steal it, so the actual destruction is deferred to
/// `destroy_pending_clients`, which must be called only after the receive
/// loop has stopped.
static PENDING_DESTROY: Mutex<Vec<Handle>> = Mutex::new(Vec::new());

static NEXT_CLIENT_ID: AtomicI32 = AtomicI32::new(1);

/// Time one handle is given per poll; also the worst-case wait a retiring
/// thread spends blocked behind the receive loop.
const POLL_INTERVAL: f64 = 0.05;

pub(crate) fn create_client() -> i32 {
    let handle = Handle(unsafe { td_json_client_create() });
    let id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    HANDLES.write().unwrap().insert(id, handle);
    id
}

/// Retires a client. The handle is removed from polling immediately and
/// destroyed later by `destroy_pending_clients`.
pub(crate) fn destroy_client(client_id: i32) {
    if let Some(handle) = HANDLES.write().unwrap().remove(&client_id) {
        PENDING_DESTROY.lock().unwrap().push(handle);
    }
}

/// Destroys every retired handle. Call only when no thread is polling
/// clients any more (i.e. after the receive loop has been joined).
pub(crate) fn destroy_pending_clients() {
    let pending = std::mem::take(&mut *PENDING_DESTROY.lock().unwrap());
    for handle in pending {
        unsafe { td_json_client_destroy(handle.0) };
    }
}

pub(crate) fn send(client_id: i32, request: String) {
    let guard = HANDLES.read().unwrap();
    let Some(&handle) = guard.get(&client_id) else {
        log::warn!("Dropping request sent to unknown client id {client_id}");
        return;
    };
    let cstring = CString::new(request).unwrap();
    unsafe { td_json_client_send(handle.0, cstring.as_ptr()) };
}

/// Polls every live client round-robin until one answers or `timeout`
/// elapses. Returns the raw response together with the crate-assigned id of
/// the client that produced it (the per-instance API omits `@client_id`).
pub(crate) fn receive(timeout: f64) -> Option<(String, i32)> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    loop {
        let response = {
            let guard = HANDLES.read().unwrap();
            if guard.is_empty() {
                return None;
            }
            guard.iter().find_map(|(&id, &handle)| {
                let response = unsafe {
                    td_json_client_receive(handle.0, POLL_INTERVAL)
                        .as_ref()
                        .map(|r| CStr::from_ptr(r).to_string_lossy().into_owned())
                };
                response.map(|text| (text, id))
            })
        };
        if response.is_some() {
            return response;
        }
        if Instant::now() >= deadline {
            return None;
        }
    }
}
