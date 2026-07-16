mod bar;
mod edit_window;
mod icon;
mod row;
mod selection;

pub(crate) use self::bar::Bar;
pub(crate) use self::edit_window::EditWindow;
pub(crate) use self::icon::Icon;
pub(crate) use self::row::Row;
pub(crate) use self::selection::Selection;

/// The TDLib chat-folder icon names offered in the editor's icon picker, each paired with
/// the symbolic icon Paper Plane renders for it (in both the picker and the sidebar). The
/// stored folder icon is the TDLib name; unknown names fall back to a plain folder.
pub(crate) const FOLDER_ICONS: &[(&str, &str)] = &[
    ("All", "all-chats-symbolic"),
    ("Unread", "mail-unread-symbolic"),
    ("Unmuted", "audio-volume-high-symbolic"),
    ("Groups", "system-users-symbolic"),
    ("Private", "avatar-default-symbolic"),
    ("Custom", "folder-symbolic"),
    ("Setup", "emblem-system-symbolic"),
    ("Crown", "starred-symbolic"),
    ("Game", "applications-games-symbolic"),
    ("Home", "user-home-symbolic"),
    ("Love", "emote-love-symbolic"),
    ("Travel", "emoji-travel-symbolic"),
    ("Airplane", "airplane-mode-symbolic"),
    ("Book", "user-bookmarks-symbolic"),
    ("Light", "weather-clear-symbolic"),
    ("Note", "document-edit-symbolic"),
    ("Palette", "applications-graphics-symbolic"),
    ("Mask", "face-smile-symbolic"),
];

/// The symbolic icon for a TDLib chat-folder icon name (defaults to a plain folder).
pub(crate) fn folder_icon_symbolic(name: &str) -> &'static str {
    FOLDER_ICONS
        .iter()
        .find(|(tdlib_name, _)| *tdlib_name == name)
        .map(|(_, symbolic)| *symbolic)
        .unwrap_or("folder-symbolic")
}
