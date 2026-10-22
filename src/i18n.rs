use gettextrs::gettext;
use gettextrs::ngettext;

use crate::utils;

// Module taken from Fractal: https://gitlab.gnome.org/GNOME/fractal/-/blob/main/src/i18n.rs

/// Like `gettext`, but replaces named variables with the given dictionary.
///
/// The expected format to replace is `{name}`, where `name` is the first string
/// in the dictionary entry tuple.
pub(crate) fn gettext_f(msgid: &str, args: &[(&str, &str)]) -> String {
    let s = gettext(msgid);
    utils::freplace(s, args)
}

/// Like `ngettext`, but replaces named variables with the given dictionary.
///
/// The expected format to replace is `{name}`, where `name` is the first string
/// in the dictionary entry tuple.
pub fn ngettext_f(msgid: &str, msgid_plural: &str, n: u32, args: &[(&str, &str)]) -> String {
    let s = ngettext(msgid, msgid_plural, n);
    utils::freplace(s, args)
}

/// Marks a string literal for extraction by `xgettext` without translating it
/// at the call site. For tables that store msgids and translate them later
/// with `gettext()`; registered as a keyword in `po/meson.build`.
pub(crate) const fn gettext_noop(s: &'static str) -> &'static str {
    s
}
