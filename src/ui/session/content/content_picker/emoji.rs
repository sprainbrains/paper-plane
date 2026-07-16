use glib::clone;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::ContentPicker;
use super::PickerMode;
use crate::model;
use crate::ui;
use crate::utils;

pub(super) const EMOJI_SECTIONS: &[(&str, &str, &str)] = &[
    ("Recently Used", "document-open-recent-symbolic", ""),
    (
        "Smileys & People",
        "emoji-people-symbolic",
        "😀 😃 😄 😁 😆 😅 😂 🤣 😊 😇 🙂 🙃 😉 😌 😍 🥰 😘 😗 😙 😚 😋 😛 😝 😜 🤪 🤨 🧐 🤓 😎 🤩 🥳 😏 😒 😞 😔 😟 😕 🙁 ☹️ 😣 😖 😫 😩 🥺 😢 😭 😤 😠 😡 🤬 🤯 😳 🥵 🥶 😱 😨 😰 😥 😓 🤗 🤔 🫣 🤭 🫢 🫡 🤫 🫠 🤥 😶 🫥 😐 🫤 😑 😬 🙄 😯 😦 😧 😮 😲 🥱 😴 🤤 😪 😵 🤐 🥴 🤢 🤮 🤧 😷 🤒 🤕",
    ),
    (
        "Animals & Nature",
        "emoji-nature-symbolic",
        "🐶 🐱 🐭 🐹 🐰 🦊 🐻 🐼 🐻‍❄️ 🐨 🐯 🦁 🐮 🐷 🐸 🐵 🙈 🙉 🙊 🐒 🐔 🐧 🐦 🐤 🦆 🦅 🦉 🦇 🐺 🐗 🐴 🦄 🐝 🪱 🐛 🦋 🐌 🐞 🐜 🪰 🪲 🪳 🦟 🦗 🕷️ 🦂 🐢 🐍 🦎 🐙 🦑 🦀 🐠 🐟 🐬 🐳 🌵 🎄 🌲 🌳 🌴 🪴 🌱 🌿 ☘️ 🍀 🎍 🪺 🍁 🍄 🐚",
    ),
    (
        "Food & Drink",
        "emoji-food-symbolic",
        "🍏 🍎 🍐 🍊 🍋 🍌 🍉 🍇 🍓 🫐 🍈 🍒 🍑 🥭 🍍 🥥 🥝 🍅 🍆 🥑 🥦 🥬 🥒 🌶️ 🫑 🌽 🥕 🫒 🧄 🧅 🥔 🍠 🥐 🥯 🍞 🥖 🥨 🧀 🥚 🍳 🧈 🥞 🧇 🥓 🥩 🍗 🍖 🌭 🍔 🍟 🍕 🥪 🥙 🧆 🌮 🌯 🥗 🍝 🍜 🍣 🍱 🥟 🍤 🍚 🍦 🍩 🍪 🎂 🍰 🧁 🍫 🍬 ☕️ 🫖 🧃 🥤 🍺 🍷",
    ),
    (
        "Activities",
        "emoji-activities-symbolic",
        "⚽️ 🏀 🏈 ⚾️ 🥎 🎾 🏐 🏉 🥏 🎱 🪀 🏓 🏸 🏒 🏑 🥍 🏏 🪃 🥅 ⛳️ 🪁 🏹 🎣 🤿 🥊 🥋 🎽 🛹 🛼 🛷 ⛸️ 🥌 🎿 ⛷️ 🏂 🪂 🏋️ 🤼 🤸 ⛹️ 🤺 🤾 🏌️ 🏇 🧘 🏄 🏊 🤽 🚣 🧗 🚵 🚴 🏆 🥇 🥈 🥉 🎮 🕹️ 🎲 ♟️ 🎯 🎳 🎨 🎭 🎤 🎧 🎼 🎹 🥁 🎷 🎺 🎸 🎻",
    ),
    (
        "Travel & Places",
        "emoji-travel-symbolic",
        "🚗 🚕 🚙 🚌 🚎 🏎️ 🚓 🚑 🚒 🚐 🛻 🚚 🚛 🚜 🛴 🚲 🛵 🏍️ 🚨 🚔 🚍 🚘 🚖 🚡 🚠 🚟 🚃 🚋 🚞 🚝 🚄 🚅 🚈 🚂 🚆 🚇 🚊 ✈️ 🛫 🛬 🛩️ 🚀 🛸 🚁 ⛵️ 🚤 🛥️ 🛳️ ⛴️ 🚢 ⚓️ ⛽️ 🚧 🚦 🗺️ 🗿 🗽 🗼 🏰 🏯 🏟️ 🎡 🎢 ⛲️ 🏖️ 🏝️ 🏜️ 🌋 ⛰️ 🏕️ 🌅 🌄 🌠 🎇 🌇 🌆 🏙️ 🌃 🌌",
    ),
    (
        "Objects",
        "emoji-objects-symbolic",
        "⌚️ 📱 💻 ⌨️ 🖥️ 🖨️ 🖱️ 🕹️ 🗜️ 💽 💾 💿 📀 📼 📷 📸 📹 🎥 📽️ 🎞️ 📞 ☎️ 📟 📠 📺 📻 🎙️ ⏱️ ⏰ ⌛️ 🔋 🔌 💡 🔦 🕯️ 🧯 🛢️ 💸 💵 💶 💷 💰 💳 💎 ⚖️ 🪜 🧰 🪛 🔧 🔨 ⚒️ 🛠️ ⛏️ 🪚 🔩 ⚙️ 🧱 ⛓️ 🧲 🔫 💣 🧨 🪓 🔪 🗡️ ⚔️ 🛡️ 🚬 ⚰️ 🪦 🔮 📿 🧿 💈 ⚗️ 🔭 🔬 🕳️ 🩹 🩺 💊 💉 🧬 🦠 🧹 🧺 🧻 🚽 🚿 🛁 🪥 🧼 🪒",
    ),
    (
        "Symbols",
        "emoji-symbols-symbolic",
        "❤️ 🧡 💛 💚 💙 💜 🖤 🤍 🤎 💔 ❤️‍🔥 ❤️‍🩹 ❣️ 💕 💞 💓 💗 💖 💘 💝 💟 ☮️ ✝️ ☪️ 🕉️ ☸️ ✡️ 🔯 🕎 ☯️ ☦️ 🛐 ⛎ ♈️ ♉️ ♊️ ♋️ ♌️ ♍️ ♎️ ♏️ ♐️ ♑️ ♒️ ♓️ 🆔 ⚛️ ☢️ ☣️ 📴 📳 🈶 🈚️ 🈸 🈺 🈷️ ✴️ 🆚 💮 🉐 ㊙️ ㊗️ 🈴 🈵 🈹 🈲 🅰️ 🅱️ 🆎 🆑 🅾️ 🆘 ❌ ⭕️ 🛑 ⛔️ 📛 🚫 💯 💢 ♨️ 🚷 🚯 🚳 🚱 🔞 📵 🚭 ❗️ ❓ ‼️ ⁉️ 🔅 🔆 〽️ ⚠️ 🚸 🔱 ⚜️ 🔰 ♻️ ✅ ❇️ ✳️ ❎ 🌐 💠 Ⓜ️ 🌀 💤",
    ),
    (
        "Flags",
        "emoji-flags-symbolic",
        "🏁 🚩 🎌 🏴 🏳️ 🏳️‍🌈 🏳️‍⚧️ 🇺🇳 🇪🇺 🇩🇪 🇫🇷 🇬🇧 🇮🇹 🇪🇸 🇵🇱 🇺🇦 🇳🇱 🇧🇪 🇨🇭 🇦🇹 🇸🇪 🇳🇴 🇩🇰 🇫🇮 🇮🇸 🇮🇪 🇵🇹 🇬🇷 🇨🇿 🇸🇰 🇭🇺 🇷🇴 🇧🇬 🇭🇷 🇷🇸 🇸🇮 🇪🇪 🇱🇻 🇱🇹 🇺🇸 🇨🇦 🇲🇽 🇧🇷 🇦🇷 🇨🇱 🇨🇴 🇯🇵 🇰🇷 🇨🇳 🇮🇳 🇦🇺 🇳🇿 🇿🇦 🇪🇬 🇹🇷 🇮🇱 🇸🇦 🇦🇪",
    ),
];

pub(super) const CUSTOM_EMOJI_COLUMNS: i32 = 8;
pub(super) const CUSTOM_EMOJI_CELL_SIZE: i32 = 44;
pub(super) const CUSTOM_EMOJI_SIZE: i32 = 30;

#[derive(Debug, Clone)]
pub(super) enum EmojiRow {
    Header { title: String, name: String },
    Emoji(Vec<String>),
    CustomEmoji(Vec<tdlib::types::Sticker>),
}

pub(super) fn custom_emoji_section_name(set_id: i64) -> String {
    format!("custom-emoji-{set_id}")
}

pub(super) fn push_emoji_section(
    rows: &mut Vec<glib::BoxedAnyObject>,
    title: &str,
    name: &str,
    emojis: &[String],
) {
    rows.push(glib::BoxedAnyObject::new(EmojiRow::Header {
        title: title.to_owned(),
        name: name.to_owned(),
    }));
    if !emojis.is_empty() {
        rows.push(glib::BoxedAnyObject::new(EmojiRow::Emoji(emojis.to_vec())));
    }
}

pub(super) fn emoji_grid<'a>(
    picker: &ContentPicker,
    emojis: impl Iterator<Item = &'a str>,
) -> gtk::FlowBox {
    // A FlowBox fits as many equal-width cells per line as the panel width allows and
    // reflows on its own as the picker is resized — no manual column bookkeeping.
    let grid = gtk::FlowBox::builder()
        .homogeneous(true)
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(4)
        .max_children_per_line(64)
        .build();
    for emoji in emojis {
        let button = gtk::Button::with_label(emoji);
        button.add_css_class("flat");
        button.add_css_class("emoji-picker-item");
        let value = emoji.to_string();
        button.connect_clicked(clone!(
            #[weak]
            picker,
            move |_| {
                if picker.imp().mode.get() == PickerMode::Reaction {
                    let reaction_type = model::BoxedReactionType(
                        tdlib::enums::ReactionType::Emoji(tdlib::types::ReactionTypeEmoji {
                            emoji: value.clone(),
                        }),
                    );
                    picker.emit_by_name::<()>("reaction-picked", &[&reaction_type]);
                } else {
                    picker.remember_emoji(&value);
                    if let Some(session) = utils::ancestor::<_, ui::Session>(&picker) {
                        session.insert_emoji(&value);
                    }
                }
                picker.emit_by_name::<()>("content-picked", &[]);
            }
        ));
        grid.append(&button);
    }
    grid
}
