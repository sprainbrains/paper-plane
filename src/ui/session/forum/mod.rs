use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use glib::clone;
use gtk::glib;

use crate::model;
use crate::ui;
use crate::utils;

/// The six icon colors TDLib allows for forum topics (api.tl, createForumTopic).
const TOPIC_COLORS: [i32; 6] = [0x6FB9F0, 0xFFD67E, 0xCB86DB, 0x8EEE98, 0xFF93B2, 0xFB6F5F];

/// Whether `topic` may be renamed/reconfigured by the current user: its
/// creator, or a holder of the manage-topics right (Creator has all rights).
pub(crate) fn can_manage_topic(chat: &model::Chat, topic: &model::Topic) -> bool {
    let me = chat.session_().me_();
    if matches!(topic.creator(),
        Some(tdlib::enums::MessageSender::User(user)) if user.user_id == me.id())
    {
        return true;
    }
    match chat.chat_type().supergroup().map(|s| s.status().0) {
        Some(tdlib::enums::ChatMemberStatus::Creator(_)) => true,
        Some(tdlib::enums::ChatMemberStatus::Administrator(admin)) => {
            admin.rights.can_manage_topics
        }
        _ => false,
    }
}

/// Opens the create/edit topic dialog: a name field, the six TDLib colors and
/// the default icon row.
/// shortcut: only the default icon grid (no "any custom emoji" for Premium),
/// and edit mode hides the color row — `editForumTopic` has no color
/// parameter; upgrade when the emoji browser and TDLib allow both.
pub(crate) fn show_topic_dialog(
    parent: &gtk::Window,
    chat: &model::Chat,
    edit: Option<&model::Topic>,
) {
    let client_id = chat.session_().client_().id();
    let session = chat.session_();
    let chat = chat.clone();
    let edit_topic = edit.cloned();
    let is_edit = edit_topic.is_some();

    let window = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(380)
        .title(if is_edit {
            gettext("Edit Topic")
        } else {
            gettext("New Topic")
        })
        .build();

    // Selection state shared by the rows and the save button.
    let selected_color = Rc::new(Cell::new(
        edit_topic
            .as_ref()
            .map(|t| t.icon_color())
            .unwrap_or(TOPIC_COLORS[0]),
    ));
    let selected_emoji = Rc::new(Cell::new(
        edit_topic
            .as_ref()
            .map(|t| t.icon_custom_emoji_id())
            .unwrap_or(0),
    ));

    let name_row = adw::EntryRow::builder()
        .title(gettext("Name"))
        .text(edit_topic.as_ref().map(|t| t.name()).unwrap_or_default())
        .build();

    let color_box = gtk::Box::builder().spacing(8).homogeneous(true).build();
    if !is_edit {
        for color in TOPIC_COLORS {
            let button = gtk::ToggleButton::builder()
                .css_classes(["topic-swatch", &format!("topic-icon-color-{color:06X}")])
                .active(selected_color.get() == color)
                .build();
            button.connect_toggled(clone!(
                #[strong]
                selected_color,
                move |b| {
                    if b.is_active() {
                        selected_color.set(color);
                    }
                }
            ));
            color_box.append(&button);
        }
    } else {
        color_box.set_visible(false);
    }

    let icon_box = gtk::Box::builder().spacing(8).build();
    // The "no custom emoji" option (plain colored square).
    let none_button = gtk::ToggleButton::builder()
        .label("#")
        .css_classes(["topic-icon-choice"])
        .active(selected_emoji.get() == 0)
        .build();
    icon_box.append(&none_button);
    let default_emoji = selected_emoji.clone();
    none_button.connect_toggled(move |b| {
        if b.is_active() {
            default_emoji.set(0);
        }
    });

    let emoji_for_icons = selected_emoji.clone();
    utils::spawn(clone!(
        #[weak]
        icon_box,
        #[weak]
        none_button,
        #[weak]
        session,
        #[strong]
        emoji_for_icons,
        async move {
            let Ok(tdlib::enums::Stickers::Stickers(icons)) =
                tdlib::functions::get_forum_topic_default_icons(client_id).await
            else {
                return;
            };
            for sticker in icons.stickers.into_iter() {
                let tdlib::enums::StickerFullType::CustomEmoji(full_type) = &sticker.full_type
                else {
                    continue;
                };
                let emoji_id = full_type.custom_emoji_id;
                let Ok(texture) =
                    ui::load_picker_preview(&sticker, 48, &session, Rc::new(Cell::new(true))).await
                else {
                    continue;
                };
                let button = gtk::ToggleButton::builder()
                    .css_classes(["topic-icon-choice"])
                    .active(emoji_for_icons.get() == emoji_id)
                    .build();
                button.set_group(Some(&none_button));
                button.set_child(Some(&gtk::Image::builder().paintable(&texture).build()));
                let cell = emoji_for_icons.clone();
                button.connect_toggled(move |b| {
                    if b.is_active() {
                        cell.set(emoji_id);
                    }
                });
                icon_box.append(&button);
            }
        }
    ));

    let save = gtk::Button::builder()
        .label(if is_edit {
            gettext("Save")
        } else {
            gettext("Create")
        })
        .css_classes(["suggested-action", "pill"])
        .hexpand(true)
        .build();
    let cancel = gtk::Button::builder()
        .label(gettext("Cancel"))
        .css_classes(["flat"])
        .build();
    cancel.connect_clicked(clone!(
        #[weak]
        window,
        move |_| window.close()
    ));

    let chat_for_save = chat.clone();
    let name_row_for_save = name_row.clone();
    save.connect_clicked(move |button| {
        let name = name_row_for_save.text().trim().to_string();
        if name.is_empty() {
            return;
        }
        let window = button.root().and_downcast::<gtk::Window>();
        let chat = chat_for_save.clone();
        let color = selected_color.get();
        let emoji_id = selected_emoji.get();
        let edit_id = edit_topic.as_ref().map(|t| t.forum_topic_id());
        glib::spawn_future_local(async move {
            let result = match edit_id {
                None => tdlib::functions::create_forum_topic(
                    chat.id(),
                    name,
                    false,
                    tdlib::types::ForumTopicIcon {
                        color,
                        custom_emoji_id: emoji_id,
                    },
                    client_id,
                )
                .await
                .map(|_| ()),
                Some(id) => {
                    tdlib::functions::edit_forum_topic(
                        chat.id(),
                        id,
                        name,
                        true,
                        emoji_id,
                        client_id,
                    )
                    .await
                }
            };
            match result {
                Ok(()) => {
                    if let Some(window) = window {
                        window.close();
                    }
                }
                Err(e) => {
                    if let Some(window) = window {
                        utils::show_toast(&window, e.message);
                    }
                }
            }
        });
    });

    let buttons = gtk::Box::builder().spacing(8).margin_top(8).build();
    buttons.append(&cancel);
    buttons.append(&save);

    let form = adw::PreferencesGroup::builder().build();
    form.add(&name_row);

    let content_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();
    content_box.append(&form);
    content_box.append(&color_box);
    content_box.append(&icon_box);
    content_box.append(&buttons);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content_box));
    window.set_content(Some(&toolbar));
    window.present();
}
