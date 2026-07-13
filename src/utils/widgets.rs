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

#[allow(dead_code)]
pub(crate) struct ChildIter(Option<gtk::Widget>);

impl From<&gtk::Widget> for ChildIter {
    fn from(widget: &gtk::Widget) -> Self {
        Self(widget.first_child())
    }
}

impl Iterator for ChildIter {
    type Item = gtk::Widget;

    fn next(&mut self) -> Option<Self::Item> {
        let r = self.0.take();
        self.0 = r.as_ref().and_then(|widget| widget.next_sibling());
        r
    }
}
