// TODO: This has been added because of the gettext macros. Remove this
// when the macros are either fixed or removed (check #94).
#![allow(clippy::format_push_string)]
#![warn(clippy::uninlined_format_args)]

/// Like `gettext`, but interpolates positional `{}` placeholders with the given
/// arguments, in order.
///
/// This replaces the `gettext!` format macro that was provided by `gettext-rs`
/// up to 0.7.0 and removed in later releases. Defined at the crate root so it is
/// textually in scope (as bare `gettext!`) for all modules below without a `use`
/// that would collide with the `gettextrs::gettext` function import.
macro_rules! gettext {
    ($msgid:expr $(,)?) => {
        ::gettextrs::gettext($msgid)
    };
    ($msgid:expr, $($arg:expr),+ $(,)?) => {{
        let mut msg = ::gettextrs::gettext($msgid);
        $(
            msg = msg.replacen("{}", &::std::string::ToString::to_string(&$arg), 1);
        )+
        msg
    }};
}

mod application;
mod automation;
mod ui;
#[rustfmt::skip]
#[allow(clippy::all)]
mod config;
mod expressions;
mod i18n;
mod model;
mod strings;
mod types;
mod utils;

use std::borrow::Cow;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;

use gettextrs::LocaleCategory;
use gettextrs::gettext;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use temp_dir::TempDir;

use self::application::Application;

pub(crate) static APPLICATION_OPTS: OnceLock<ApplicationOptions> = OnceLock::new();
pub(crate) static TEMP_DIR: OnceLock<PathBuf> = OnceLock::new();

fn main() -> glib::ExitCode {
    let app = setup_cli(Application::default());

    // Command line handling
    app.connect_handle_local_options(|_, dict| {
        if dict.contains("version") {
            // Print version ...
            println!("paper-plane {}", config::VERSION);
            // ... and exit application.
            std::ops::ControlFlow::Break(glib::ExitCode::from(1))
        } else {
            adw::init().expect("Failed to init GTK/libadwaita");
            ui::init();

            // Prepare i18n
            gettextrs::setlocale(LocaleCategory::LcAll, "");
            gettextrs::bindtextdomain(config::GETTEXT_PACKAGE, config::LOCALEDIR)
                .expect("Unable to bind the text domain");
            gettextrs::textdomain(config::GETTEXT_PACKAGE)
                .expect("Unable to switch to the text domain");

            glib::set_application_name("Paper Plane");

            gio::resources_register(
                &gio::Resource::load(config::RESOURCES_FILE)
                    .expect("Could not load gresource file"),
            );
            gio::resources_register(
                &gio::Resource::load(config::UI_RESOURCES_FILE)
                    .expect("Could not load UI gresource file"),
            );

            let log_level = match dict.lookup::<String>("log-level") {
                Ok(Some(level)) => match log::Level::from_str(&level) {
                    Ok(level) => level,
                    Err(e) => {
                        eprintln!("Invalid log level '{level}': {e}");
                        return std::ops::ControlFlow::Break(glib::ExitCode::from(1));
                    }
                },
                // Standard log levels if not specified by user
                Ok(None) => log::Level::Warn,
                Err(e) => {
                    eprintln!("Could not read log-level option: {e}");
                    return std::ops::ControlFlow::Break(glib::ExitCode::from(1));
                }
            };

            let mut application_opts = ApplicationOptions::default();

            env_logger::Builder::new()
                .filter_level(log_level.to_level_filter())
                .init();

            if dict.contains("test-dc") {
                application_opts.test_dc = true;
            }

            let client_id = match dict.lookup("client-id") {
                Ok(client_id) => client_id,
                Err(e) => {
                    log::error!("Could not read client-id option: {e}");
                    return std::ops::ControlFlow::Break(glib::ExitCode::from(1));
                }
            };
            let client_secret = match dict.lookup("client-secret") {
                Ok(client_secret) => client_secret,
                Err(e) => {
                    log::error!("Could not read client-secret option: {e}");
                    return std::ops::ControlFlow::Break(glib::ExitCode::from(1));
                }
            };

            match (client_id, client_secret) {
                (Some(client_id), Some(client_secret)) => {
                    application_opts.client_id = client_id;
                    application_opts.client_secret = Cow::Owned(client_secret);
                }
                (None, None) => (),
                _ => {
                    log::error!("Both client-id and client-secret must be set together");
                    return std::ops::ControlFlow::Break(glib::ExitCode::from(1));
                }
            };

            APPLICATION_OPTS.set(application_opts).unwrap();

            std::ops::ControlFlow::Continue(())
        }
    });

    // Create temp directory.
    // This value must live during the entire execution of the app.
    let temp_dir = TempDir::with_prefix("paper-plane");
    match &temp_dir {
        Ok(temp_dir) => {
            TEMP_DIR.set(temp_dir.path().to_path_buf()).unwrap();
        }
        Err(e) => {
            log::warn!("Error creating temp directory: {e:?}");
        }
    }

    app.run()
}

/// Global options for the application
#[derive(Debug)]
pub(crate) struct ApplicationOptions {
    pub(crate) data_dir: PathBuf,
    pub(crate) test_dc: bool,
    pub(crate) client_id: i32,
    pub(crate) client_secret: Cow<'static, str>,
}
impl Default for ApplicationOptions {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from(glib::user_data_dir().to_str().unwrap()).join("paper-plane"),
            test_dc: Default::default(),
            client_id: config::TG_API_ID,
            client_secret: Cow::Borrowed(config::TG_API_HASH),
        }
    }
}

fn setup_cli<A: IsA<gio::Application>>(app: A) -> A {
    app.add_main_option(
        "version",
        b'v'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        &gettext("Prints application version"),
        None,
    );

    app.add_main_option(
        "log-level",
        b'l'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        &gettext("Specify the minimum log level"),
        Some("error|warn|info|debug|trace"),
    );

    app.add_main_option(
        "client-id",
        b'c'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::Int,
        &gettext("Override the builtin client id"),
        None,
    );

    app.add_main_option(
        "client-secret",
        b's'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::String,
        &gettext("Override the builtin client secret"),
        None,
    );

    app.add_main_option(
        "test-dc",
        b't'.into(),
        glib::OptionFlags::NONE,
        glib::OptionArg::None,
        &gettext("Whether to use a test data center on first account"),
        None,
    );

    app
}
