use gettextrs::gettext;
use gtk::glib;

use crate::i18n::ngettext_f;
use crate::model;

pub(crate) fn user_display_name(user: &model::User, use_full_name: bool) -> String {
    if let tdlib::enums::UserType::Deleted = user.user_type().0 {
        gettext("Deleted Account")
    } else if user.last_name().is_empty() || !use_full_name {
        user.first_name()
    } else if user.first_name().is_empty() {
        user.last_name()
    } else {
        user.first_name() + " " + &user.last_name()
    }
}

pub(crate) fn user_status(status: &tdlib::enums::UserStatus) -> String {
    use tdlib::enums::UserStatus::*;

    match status {
        Empty => gettext("last seen a long time ago"),
        Online(_) => gettext("online"),
        Offline(data) => {
            let now = glib::DateTime::now_local().unwrap();
            let was_online = glib::DateTime::from_unix_local(data.was_online as i64).unwrap();
            let time_span = now.difference(&was_online);

            // TODO: Add a way to update the string when time passes
            if time_span.as_days() > 1 {
                // Translators: This is an online status with the date
                was_online.format(&gettext("last seen %x")).unwrap().into()
            } else if now.day_of_week() != was_online.day_of_week() && now.hour() >= 1 {
                // Translators: This is an online status with the last seen time, without seconds
                // Here you may want to change to a 24-hours representation, based on your locale.
                // You can use this site to learn more: https://www.strfti.me/
                let format = gettext("last seen yesterday at %l:%M %p");
                was_online.format(&format).unwrap().into()
            } else if time_span.as_hours() > 0 {
                ngettext_f(
                    "last seen {num} hour ago",
                    "last seen {num} hours ago",
                    time_span.as_hours() as u32,
                    &[("num", &time_span.as_hours().to_string())],
                )
            } else if time_span.as_minutes() > 0 {
                ngettext_f(
                    "last seen {num} minute ago",
                    "last seen {num} minutes ago",
                    time_span.as_minutes() as u32,
                    &[("num", &time_span.as_minutes().to_string())],
                )
            } else {
                gettext("last seen just now")
            }
        }
        Recently(_) => gettext("last seen recently"),
        LastWeek(_) => gettext("last seen within a week"),
        LastMonth(_) => gettext("last seen within a month"),
    }
}
