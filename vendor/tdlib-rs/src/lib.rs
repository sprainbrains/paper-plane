// Copyright 2020 - developers of the `grammers` project.
// Copyright 2021 - developers of the `tdlib-rs` project.
// Copyright 2024 - developers of the `tgt` and `tdlib-rs` projects.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
pub mod build;
mod generated;
mod observer;
mod tdjson;

pub use generated::{enums, functions, types};

use enums::Update;
use once_cell::sync::Lazy;
use serde_json::Value;
use std::sync::atomic::{AtomicU32, Ordering};

static EXTRA_COUNTER: AtomicU32 = AtomicU32::new(0);
static OBSERVER: Lazy<observer::Observer> = Lazy::new(observer::Observer::new);

/// Create a TdLib client returning its id. Note that to start receiving
/// updates for a client you need to send at least a request with it first.
pub fn create_client() -> i32 {
    tdjson::create_client()
}

/// Retire a client created by [`create_client`]: it stops receiving updates
/// and its responses are no longer polled. The underlying TDLib instance is
/// destroyed by [`destroy_pending_clients`].
pub fn destroy_client(client_id: i32) {
    tdjson::destroy_client(client_id)
}

/// Destroys the TDLib instances of all retired clients, blocking until each
/// is fully released. Must be called only after [`receive`] is no longer in
/// use (i.e. the receive loop has stopped), because TDLib's own teardown
/// needs to read the "client released" response off the instance itself.
pub fn destroy_pending_clients() {
    tdjson::destroy_pending_clients()
}

/// Receive a single update or response from TdLib. If it's an update, it
/// returns a tuple with the `Update` and the associated `client_id`.
/// Note that to start receiving updates for a client you need to send
/// at least a request with it first.
pub fn receive() -> Option<(Update, i32)> {
    if let Some((response_str, client_id)) = tdjson::receive(2.0) {
        let response: Value = serde_json::from_str(&response_str).unwrap();

        match response.get("@extra") {
            Some(_) => {
                OBSERVER.notify(response);
            }
            None => {
                match serde_json::from_value(response) {
                    Ok(update) => {
                        return Some((update, client_id));
                    }
                    Err(e) => {
                        log::warn!("Received an unknown response: {response_str}\nReason: {e}");
                    }
                }
            }
        }
    }

    None
}

pub(crate) async fn send_request(client_id: i32, mut request: Value) -> Value {
    let extra = EXTRA_COUNTER.fetch_add(1, Ordering::Relaxed);
    request["@extra"] = serde_json::to_value(extra).unwrap();

    let receiver = OBSERVER.subscribe(extra);
    tdjson::send(client_id, request.to_string());

    receiver.await.unwrap()
}
