use std::collections::hash_map::Entry;

use glib::clone;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::utils;

impl super::ClientStateSession {
    /// Downloads a file of the specified id. This will only return when the file
    /// downloading has completed or has failed.
    pub(crate) async fn download_file(
        &self,
        file_id: i32,
    ) -> Result<tdlib::types::File, tdlib::types::Error> {
        let client_id = self.client_().id();
        let result = tdlib::functions::download_file(file_id, 5, 0, 0, true, client_id).await;

        result.map(|data| {
            let tdlib::enums::File::File(file) = data;
            file
        })
    }

    /// Downloads a file of the specified id and calls a closure every time there's an update
    /// about the progress or when the download has completed.
    pub(crate) fn download_file_with_updates<F: Fn(tdlib::types::File) + 'static>(
        &self,
        file_id: i32,
        f: F,
    ) {
        let (sender, receiver) = async_channel::unbounded::<tdlib::types::File>();

        glib::spawn_future_local(async move {
            while let Ok(file) = receiver.recv().await {
                if !file.local.is_downloading_active {
                    break;
                }
                f(file);
            }
        });

        let mut downloading_files = self.imp().downloading_files.borrow_mut();
        match downloading_files.entry(file_id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().push(sender);
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![sender]);

                let client_id = self.client_().id();
                utils::spawn(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    async move {
                        let result =
                            tdlib::functions::download_file(file_id, 5, 0, 0, false, client_id)
                                .await;
                        match result {
                            Ok(tdlib::enums::File::File(file)) => {
                                obj.handle_file_update(file);
                            }
                            Err(e) => {
                                log::warn!("Error downloading a file: {:?}", e);
                            }
                        }
                    }
                ));
            }
        }
    }

    pub(crate) fn cancel_download_file(&self, file_id: i32) {
        let client_id = self.client_().id();
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::cancel_download_file(file_id, false, client_id).await
            {
                log::warn!("Error canceling a file: {:?}", e);
            }
        });
    }

    pub(super) fn handle_file_update(&self, file: tdlib::types::File) {
        let mut downloading_files = self.imp().downloading_files.borrow_mut();
        if let Entry::Occupied(mut entry) = downloading_files.entry(file.id) {
            // Keep only the senders with which it was possible to send successfully.
            // It is indeed possible that the object that created the sender and receiver and
            // attached it to the default main context has been disposed in the meantime.
            // This is problematic if it is now tried to upgrade a weak reference of this object in
            // the receiver closure.
            // It will either panic directly if `@default-panic` is used or the sender will return
            // an error in the `SyncSender::send()` function if
            // `default-return glib::Continue(false)` is used. In the latter case, the Receiver
            // will be detached from the main context, which will cause the sending to fail.
            entry.get_mut();
            // .retain(|sender| sender.send(file.clone()).is_ok());

            if !file.local.is_downloading_active || entry.get().is_empty() {
                entry.remove();
            }
        }
    }
}
