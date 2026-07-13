use std::ops::Deref;

use gtk::gio;

use crate::config;

#[derive(Debug)]
pub(crate) struct PaperPlaneSettings(gio::Settings);

impl Default for PaperPlaneSettings {
    fn default() -> Self {
        Self(gio::Settings::new(config::APP_ID))
    }
}

impl Deref for PaperPlaneSettings {
    type Target = gio::Settings;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
