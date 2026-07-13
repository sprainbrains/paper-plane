use adw::prelude::*;

pub(crate) fn destructive_confirmation_dialog(
    heading: &str,
    body: &str,
    cancel_response: (&str, &str),
    destructive_response: (&str, &str),
) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(body)
        .build();

    dialog.add_responses(&[cancel_response, destructive_response]);
    dialog.set_default_response(Some(cancel_response.0));
    dialog.set_response_appearance(destructive_response.0, adw::ResponseAppearance::Destructive);
    dialog
}
