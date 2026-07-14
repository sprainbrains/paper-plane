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
                let is_terminal = !file.local.is_downloading_active;
                f(file);

                if is_terminal {
                    break;
                }
            }
        });

        let should_request = {
            let mut downloading_files = self.imp().downloading_files.borrow_mut();
            match downloading_files.entry(file_id) {
                Entry::Occupied(mut entry) => {
                    entry.get_mut().push(sender);
                    false
                }
                Entry::Vacant(entry) => {
                    entry.insert(vec![sender]);
                    true
                }
            }
        };

        if should_request {
            let client_id = self.client_().id();
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    let result =
                        tdlib::functions::download_file(file_id, 5, 0, 0, false, client_id).await;
                    match result {
                        Ok(tdlib::enums::File::File(file)) => {
                            obj.handle_file_update(file);
                        }
                        Err(e) => {
                            obj.imp().downloading_files.borrow_mut().remove(&file_id);
                            log::warn!("Error downloading a file: {e:?}");
                        }
                    }
                }
            ));
        }
    }

    pub(crate) fn cancel_download_file(&self, file_id: i32) {
        let client_id = self.client_().id();
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::cancel_download_file(file_id, false, client_id).await
            {
                log::warn!("Error canceling a file: {e:?}");
            }
        });
    }

    pub(super) fn handle_file_update(&self, file: tdlib::types::File) {
        let Some(mut senders) = self.imp().downloading_files.borrow_mut().remove(&file.id) else {
            return;
        };

        senders.retain(|sender| sender.try_send(file.clone()).is_ok());

        if file.local.is_downloading_active && !senders.is_empty() {
            self.imp()
                .downloading_files
                .borrow_mut()
                .insert(file.id, senders);
        }
    }
}
