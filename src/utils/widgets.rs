use gtk::glib;
use gtk::prelude::*;

pub(crate) fn show_toast<W: IsA<gtk::Widget>>(widget: &W, title: impl Into<glib::GString>) {
    let title = title.into();
    let Some(overlay) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    else {
        log::warn!("Could not show toast without a ToastOverlay ancestor: {title}");
        return;
    };

    overlay.add_toast(
        adw::Toast::builder()
            .title(title)
            .timeout(3)
            .priority(adw::ToastPriority::High)
            .build(),
    );
}

pub(crate) fn ancestor<W: IsA<gtk::Widget>, T: IsA<gtk::Widget>>(widget: &W) -> Option<T> {
    widget.ancestor(T::static_type()).and_downcast::<T>()
}

/// Returns the first descendant of `widget` (depth-first) that is a `T`.
pub(crate) fn descendant<W: IsA<gtk::Widget>, T: IsA<gtk::Widget>>(widget: &W) -> Option<T> {
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Ok(found) = widget.clone().downcast::<T>() {
            return Some(found);
        }
        if let Some(found) = descendant::<_, T>(&widget) {
            return Some(found);
        }
        child = widget.next_sibling();
    }
    None
}

/// Applies `f` to every descendant of `widget` (depth-first) that is a `T`.
pub(crate) fn for_each_descendant<W: IsA<gtk::Widget>, T: IsA<gtk::Widget>>(
    widget: &W,
    f: &mut impl FnMut(&T),
) {
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if let Some(found) = widget.downcast_ref::<T>() {
            f(found);
        }
        for_each_descendant::<_, T>(&widget, f);
        child = widget.next_sibling();
    }
}
