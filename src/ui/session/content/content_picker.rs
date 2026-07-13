use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Instant;

use futures::StreamExt;
use gettextrs::gettext;
use glib::clone;
use glib::subclass::Signal;
use gtk::CompositeTemplate;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use serde::Deserialize;
use serde::Serialize;

use crate::model;
use crate::ui;
use crate::utils;

const EMOJI_SECTIONS: &[(&str, &str, &str)] = &[
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

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/content_picker.ui")]
    pub(crate) struct ContentPicker {
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        pub(super) chat: glib::WeakRef<model::Chat>,
        /// Whether this picker composes a message (default) or picks a reaction.
        pub(super) mode: Cell<PickerMode>,
        /// `(chat_id, message_id)` targeted when in [`PickerMode::Reaction`].
        pub(super) reaction_target: Cell<Option<(i64, i64)>>,
        pub(super) stickers_loaded: Cell<bool>,
        pub(super) gifs_loaded: Cell<bool>,
        /// Whether the emoji page's full content (standard + custom emoji) is built,
        /// so reopening only refreshes the cheap "Recently Used" section instead of
        /// rebuilding — and re-downloading — every custom-emoji pack.
        pub(super) emoji_built: Cell<bool>,
        pub(super) sticker_search_active: Cell<bool>,
        pub(super) emoji_search_active: Cell<bool>,
        pub(super) sticker_generation: Cell<u64>,
        /// Bumped whenever the emoji page is rebuilt, so an in-flight custom-emoji
        /// load from a previous build stops appending to the model.
        pub(super) custom_emoji_generation: Cell<u64>,
        pub(super) sticker_scrolling: Rc<Cell<bool>>,
        pub(super) scroll_timeout: RefCell<Option<glib::SourceId>>,
        pub(super) emoji_model: OnceCell<gio::ListStore>,
        pub(super) sticker_model: OnceCell<gio::ListStore>,
        pub(super) gif_model: OnceCell<gio::ListStore>,
        pub(super) rail_covers: RefCell<Vec<RailCover>>,
        /// Maps each content section's name to its rail button, so the rail can
        /// highlight the clicked section.
        pub(super) rail_sections: RefCell<Vec<(String, glib::WeakRef<gtk::Button>)>>,
        pub(super) animations: RefCell<Vec<tdlib::types::Animation>>,
        #[template_child]
        pub(super) emoji_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) sticker_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) gif_tab: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub(super) search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub(super) content_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) emoji_page: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) sticker_page: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) gif_page: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub(super) emoji_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) sticker_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) gif_grid: TemplateChild<gtk::GridView>,
        #[template_child]
        pub(super) navigation_rail: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) navigation_rail_scroll: TemplateChild<gtk::ScrolledWindow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ContentPicker {
        const NAME: &'static str = "PaplContentPicker";
        type Type = super::ContentPicker;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_css_name("contentpicker");
        }
        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ContentPicker {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("content-picked").build(),
                    Signal::builder("reaction-picked")
                        .param_types([model::BoxedReactionType::static_type()])
                        .build(),
                ]
            })
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::ClientStateSession>("session").build(),
                    glib::ParamSpecObject::builder::<model::Chat>("chat").build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "session" => self.session.set(value.get().unwrap()),
                "chat" => self.chat.set(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            match pspec.name() {
                "session" => self.session.upgrade().to_value(),
                "chat" => self.chat.upgrade().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.setup_emoji_list();
            obj.setup_sticker_list();
            obj.setup_sticker_scrolling();
            obj.setup_gif_grid();
            obj.setup_rail_scrolling();
            obj.build_emoji_page();
            self.emoji_tab.connect_toggled(clone!(
                #[weak]
                obj,
                move |b| if b.is_active() {
                    obj.show_page(PickerPage::Emoji);
                }
            ));
            self.sticker_tab.connect_toggled(clone!(
                #[weak]
                obj,
                move |b| if b.is_active() {
                    obj.show_page(PickerPage::Stickers);
                }
            ));
            self.gif_tab.connect_toggled(clone!(
                #[weak]
                obj,
                move |b| if b.is_active() {
                    obj.show_page(PickerPage::Gifs);
                }
            ));
            self.search_entry.connect_search_changed(clone!(
                #[weak]
                obj,
                move |entry| obj.search(entry.text().as_str())
            ));
        }

        fn dispose(&self) {
            if let Some(source) = self.scroll_timeout.borrow_mut().take() {
                source.remove();
            }
            self.emoji_list.unparent();
            self.sticker_list.unparent();
            self.gif_grid.unparent();
            self.navigation_rail.unparent();
        }
    }

    impl WidgetImpl for ContentPicker {}
}

glib::wrapper! {
    pub(crate) struct ContentPicker(ObjectSubclass<imp::ContentPicker>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

#[derive(Debug, Clone, Copy)]
enum PickerPage {
    Emoji,
    Stickers,
    Gifs,
}

/// What a picked item does: insert into the message being composed (default), or
/// be reported as a chosen reaction via the `reaction-picked` signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PickerMode {
    #[default]
    Compose,
    Reaction,
}

/// A sticker-pack icon in the navigation rail whose cover thumbnail is fetched
/// lazily — only once the button scrolls into the rail's viewport — so opening the
/// sticker tab doesn't stampede the decode limiter with one decode per installed pack.
#[derive(Debug)]
struct RailCover {
    button: glib::WeakRef<gtk::Button>,
    picture: glib::WeakRef<gtk::Picture>,
    cover: Option<tdlib::types::Sticker>,
    thumbnail: Option<tdlib::types::File>,
    demand: Rc<Cell<bool>>,
    loaded: bool,
}

#[derive(Debug, Clone)]
enum StickerRow {
    Header { title: String, set_id: Option<i64> },
    Stickers(Vec<tdlib::types::Sticker>),
    Loading(String),
    Error { set_id: i64, title: String },
}

/// A row in the virtualized emoji `ListView`. The emoji page is a `GtkListView` so
/// only on-screen rows are ever built/decoded/animated — the standard-emoji wall of
/// buttons and every off-screen custom-emoji pack cost nothing until scrolled to.
#[derive(Debug, Clone)]
enum EmojiRow {
    /// A section heading; `name` is the rail key used to scroll to it.
    Header { title: String, name: String },
    /// A row of standard emoji characters.
    Emoji(Vec<String>),
    /// A row of custom-emoji stickers.
    CustomEmoji(Vec<tdlib::types::Sticker>),
}

const STICKER_SET_CACHE_VERSION: u32 = 1;

/// Emoji per row on the emoji page (standard and custom alike).
const EMOJI_ROW_LEN: usize = 8;

/// Layout of custom-emoji rows in the emoji page, chosen to match the standard
/// emoji grid's density rather than the larger sticker cells.
const CUSTOM_EMOJI_COLUMNS: i32 = 8;
const CUSTOM_EMOJI_CELL_SIZE: i32 = 44;
const CUSTOM_EMOJI_SIZE: i32 = 30;

/// glib qdata key under which each realized picker row stashes its section's rail key,
/// so the scroll handler can tell which section a visible row belongs to.
const RAIL_SECTION_KEY: &str = "papl-rail-section";

/// Records `section` (a rail key) on a freshly built picker row.
fn tag_rail_section(widget: &impl IsA<gtk::Widget>, section: String) {
    // Safe: only ever a `String` is stored under this key, and the row is a fresh widget
    // dropped on unbind, so the data is cleared along with it.
    unsafe { widget.set_data(RAIL_SECTION_KEY, section) };
}

/// Reads back the rail key stashed by [`tag_rail_section`], if any.
fn widget_rail_section(widget: &gtk::Widget) -> Option<String> {
    // Safe: the only writer (`tag_rail_section`) always stores a `String`.
    unsafe {
        widget
            .data::<String>(RAIL_SECTION_KEY)
            .map(|value| value.as_ref().clone())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct StickerSetSignature {
    title: String,
    name: String,
    size: i32,
    thumbnail_file_id: Option<i32>,
    cover_file_ids: Vec<i32>,
}

impl From<&tdlib::types::StickerSetInfo> for StickerSetSignature {
    fn from(info: &tdlib::types::StickerSetInfo) -> Self {
        Self {
            title: info.title.clone(),
            name: info.name.clone(),
            size: info.size,
            thumbnail_file_id: info.thumbnail.as_ref().map(|thumbnail| thumbnail.file.id),
            cover_file_ids: info
                .covers
                .iter()
                .map(|sticker| sticker.sticker.id)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedStickerSet {
    signature: StickerSetSignature,
    set: tdlib::types::StickerSet,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct StickerSetCache {
    version: u32,
    sets: HashMap<i64, CachedStickerSet>,
}

impl ContentPicker {
    fn emoji_model(&self) -> &gio::ListStore {
        self.imp().emoji_model.get().unwrap()
    }

    fn setup_emoji_list(&self) {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        self.imp().emoji_list.set_model(Some(&selection));
        self.imp().emoji_model.set(model).unwrap();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(setup_unselectable_item);
        factory.connect_bind(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let row = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
                let child: gtk::Widget = match &*row.borrow::<EmojiRow>() {
                    EmojiRow::Header { title, .. } => section_title(title).upcast(),
                    EmojiRow::Emoji(emojis) => obj.build_emoji_row(emojis),
                    EmojiRow::CustomEmoji(stickers) => obj.build_custom_emoji_row(stickers),
                };
                item.set_child(Some(&child));
                if let Some(section) = obj.emoji_section_name_at(item.position()) {
                    tag_rail_section(&child, section);
                }
            }
        ));
        factory.connect_unbind(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE);
        });
        self.imp().emoji_list.set_factory(Some(&factory));
    }

    /// A row of standard-emoji buttons (built on bind, so only visible rows exist).
    fn build_emoji_row(&self, emojis: &[String]) -> gtk::Widget {
        emoji_grid(self, emojis.iter().map(String::as_str)).upcast()
    }

    /// A row of custom-emoji stickers. The `ListView` only binds visible rows, so
    /// these load and animate only while on screen and are torn down on unbind.
    fn build_custom_emoji_row(&self, stickers: &[tdlib::types::Sticker]) -> gtk::Widget {
        let Some(session) = self.imp().session.upgrade() else {
            return gtk::Box::new(gtk::Orientation::Horizontal, 0).upcast();
        };
        ui::StickerPickerRow::new_sized(
            stickers,
            &session,
            self.imp().sticker_scrolling.clone(),
            CUSTOM_EMOJI_CELL_SIZE,
            CUSTOM_EMOJI_SIZE,
            CUSTOM_EMOJI_COLUMNS,
            // No outlines (dense); animated (frames come from the shared cache).
            false,
            true,
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |sticker| obj.activate_sticker(sticker)
            ),
        )
        .upcast()
    }

    fn sticker_model(&self) -> &gio::ListStore {
        self.imp().sticker_model.get().unwrap()
    }

    fn setup_sticker_list(&self) {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        self.imp().sticker_list.set_model(Some(&selection));
        self.imp().sticker_model.set(model).unwrap();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(setup_unselectable_item);
        factory.connect_bind(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let row = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
                log::debug!(
                    target: "paper_plane::content_picker",
                    "bind sticker row; position={}",
                    item.position(),
                );
                let child: gtk::Widget = match &*row.borrow::<StickerRow>() {
                    StickerRow::Header { title, .. } => section_title(title).upcast(),
                    StickerRow::Stickers(stickers) => obj.build_sticker_row(stickers).upcast(),
                    StickerRow::Loading(title) => status_row(title, &gettext("Loading…"), None),
                    StickerRow::Error { set_id, title } => {
                        let set_id = *set_id;
                        status_row(
                            title,
                            &gettext("Couldn't load this sticker pack"),
                            Some(Box::new(clone!(
                                #[weak]
                                obj,
                                move || obj.retry_sticker_set(set_id)
                            ))),
                        )
                    }
                };
                item.set_child(Some(&child));
                if let Some(section) = obj.sticker_section_name_at(item.position()) {
                    tag_rail_section(&child, section);
                }
            }
        ));
        factory.connect_unbind(|_, item| {
            item.downcast_ref::<gtk::ListItem>()
                .unwrap()
                .set_child(gtk::Widget::NONE);
        });
        self.imp().sticker_list.set_factory(Some(&factory));
    }

    fn build_sticker_row(&self, stickers: &[tdlib::types::Sticker]) -> gtk::Box {
        let session = self.imp().session.upgrade().unwrap();
        let preview = ui::StickerPickerRow::new(
            stickers,
            &session,
            self.imp().sticker_scrolling.clone(),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move |sticker| obj.activate_sticker(sticker)
            ),
        );
        let row = gtk::Box::builder().margin_start(12).margin_end(12).build();
        row.append(&preview);
        row
    }

    fn setup_sticker_scrolling(&self) {
        // Both the sticker list and the emoji page host `StickerPickerRow`s (the
        // emoji page now embeds custom-emoji packs), so pause preview loading and
        // animation while either is scrolling. Scrolling also moves the active rail
        // entry to the section now at the top of the viewport.
        for (page, list) in [
            (self.imp().emoji_page.get(), self.imp().emoji_list.get()),
            (self.imp().sticker_page.get(), self.imp().sticker_list.get()),
        ] {
            let adjustment = page.vadjustment();
            adjustment.connect_value_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                page,
                #[weak]
                list,
                move |_| {
                    obj.mark_scrolling();
                    obj.update_active_rail_for(&list, &page);
                }
            ));
            // `changed` fires when the content is (re)allocated — after a pack loads or
            // the page is first shown — so the highlight is correct without scrolling.
            adjustment.connect_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                #[weak]
                page,
                #[weak]
                list,
                move |_| obj.update_active_rail_for(&list, &page)
            ));
        }
    }

    /// The section rail key for the row at `position`: its own name if it is a header,
    /// else the nearest header above it. Used to tag rows so the rail follows scroll.
    fn emoji_section_name_at(&self, position: u32) -> Option<String> {
        let model = self.emoji_model();
        (0..=position).rev().find_map(|p| {
            let item = model.item(p).and_downcast::<glib::BoxedAnyObject>()?;
            match &*item.borrow::<EmojiRow>() {
                EmojiRow::Header { name, .. } => Some(name.clone()),
                _ => None,
            }
        })
    }

    fn sticker_section_name_at(&self, position: u32) -> Option<String> {
        let model = self.sticker_model();
        (0..=position).rev().find_map(|p| {
            let item = model.item(p).and_downcast::<glib::BoxedAnyObject>()?;
            match &*item.borrow::<StickerRow>() {
                StickerRow::Header { title, .. } => Some(title.clone()),
                _ => None,
            }
        })
    }

    /// Highlights the rail entry for the section whose row occupies the top of the
    /// visible page, so the active rail icon follows the scroll position.
    fn update_active_rail_for(&self, list: &gtk::ListView, page: &gtk::ScrolledWindow) {
        // A hidden page (e.g. stickers loading in the background) must not steal the
        // visible page's rail highlight.
        if !self
            .imp()
            .content_stack
            .visible_child()
            .is_some_and(|visible| &visible == page.upcast_ref::<gtk::Widget>())
        {
            return;
        }
        let viewport_height = page.height() as f32;
        let mut top_section: Option<(f32, String)> = None;
        for item_widget in utils::ChildIter::from(list.upcast_ref::<gtk::Widget>()) {
            let Some(bounds) = item_widget.compute_bounds(page) else {
                continue;
            };
            let top = bounds.y();
            let bottom = top + bounds.height();
            // Skip rows fully above the viewport top (overdraw) or fully below it.
            if bottom <= 1.0 || top >= viewport_height {
                continue;
            }
            if top_section.as_ref().is_none_or(|(t, _)| top < *t)
                && let Some(row) = item_widget.first_child()
                && let Some(section) = widget_rail_section(&row)
            {
                top_section = Some((top, section));
            }
        }
        if let Some((_, section)) = top_section {
            self.set_active_rail(&section);
        }
    }

    fn mark_scrolling(&self) {
        let imp = self.imp();
        imp.sticker_scrolling.set(true);
        if let Some(source) = imp.scroll_timeout.borrow_mut().take() {
            source.remove();
        }
        let source = glib::timeout_add_local_once(
            std::time::Duration::from_millis(150),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    let imp = obj.imp();
                    imp.scroll_timeout.borrow_mut().take();
                    imp.sticker_scrolling.set(false);
                    imp.sticker_list.queue_draw();
                    imp.emoji_list.queue_draw();
                }
            ),
        );
        *imp.scroll_timeout.borrow_mut() = Some(source);
    }

    fn gif_model(&self) -> &gio::ListStore {
        self.imp().gif_model.get().unwrap()
    }

    fn setup_gif_grid(&self) {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        self.imp().gif_grid.set_model(Some(&selection));
        self.imp().gif_model.set(model).unwrap();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            item.set_child(Some(&ui::GifCell::new()));
        });
        factory.connect_bind(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let Some(cell) = item.child().and_downcast::<ui::GifCell>() else {
                    return;
                };
                let Some(row) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                    return;
                };
                let Some(session) = obj.imp().session.upgrade() else {
                    return;
                };
                let animation = row.borrow::<tdlib::types::Animation>().clone();
                cell.set_animation(animation, &session);
            }
        ));
        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(cell) = item.child().and_downcast::<ui::GifCell>() {
                cell.clear();
            }
        });
        self.imp().gif_grid.set_factory(Some(&factory));

        self.imp().gif_grid.connect_activate(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, position| obj.activate_gif(position as usize)
        ));
    }

    pub(crate) fn refresh(&self) {
        // Refresh the Recently Used emoji section on each open, without paying for a
        // full rebuild (and custom-emoji re-download) on every open.
        if self.imp().emoji_tab.is_active() && !self.imp().emoji_search_active.get() {
            if self.imp().emoji_built.get() {
                self.refresh_recent_emoji();
            } else {
                self.build_emoji_page();
            }
        }
        log::info!(
            target: "paper_plane::content_picker",
            "picker ready; sticker and GIF content remains lazy"
        );
    }

    fn show_page(&self, page: PickerPage) {
        let started = Instant::now();
        let imp = self.imp();
        let child: gtk::Widget = match page {
            PickerPage::Emoji => imp.emoji_page.get().upcast(),
            PickerPage::Stickers => imp.sticker_page.get().upcast(),
            PickerPage::Gifs => imp.gif_page.get().upcast(),
        };
        imp.content_stack.set_visible_child(&child);
        self.rebuild_rail(page);
        match page {
            PickerPage::Emoji => {}
            PickerPage::Stickers if !imp.stickers_loaded.replace(true) => {
                self.load_sticker_content();
            }
            PickerPage::Gifs if !imp.gifs_loaded.replace(true) => self.load_gif_content(),
            _ => {}
        }
        self.search(imp.search_entry.text().as_str());
        log::info!(
            target: "paper_plane::content_picker",
            "page switched to {:?} in {:?}",
            page,
            started.elapsed(),
        );
    }

    fn build_emoji_page(&self) {
        let started = Instant::now();
        // Cancel any custom-emoji load still appending to the old model.
        self.imp()
            .custom_emoji_generation
            .set(self.imp().custom_emoji_generation.get().wrapping_add(1));

        let mut rows: Vec<glib::BoxedAnyObject> = Vec::new();
        let recent = utils::PaperPlaneSettings::default().strv("recent-emojis");
        if !recent.is_empty() {
            let values: Vec<String> = recent.iter().map(|value| value.to_string()).collect();
            push_emoji_section(&mut rows, &gettext("Recently Used"), "Recently-Used", &values);
        }
        for (title, _, emojis) in EMOJI_SECTIONS {
            if emojis.is_empty() {
                continue;
            }
            let values: Vec<String> = emojis.split_whitespace().map(str::to_owned).collect();
            push_emoji_section(&mut rows, &gettext(*title), &title.replace(' ', "-"), &values);
        }
        let model = self.emoji_model();
        model.remove_all();
        model.splice(0, 0, &rows);

        self.rebuild_rail(PickerPage::Emoji);
        // Append the user's custom-emoji packs after the standard emoji sections,
        // tdesktop-style — so custom emoji live in the emoji panel rather than a
        // separate tab. Only visible rows are built by the ListView.
        self.load_custom_emoji_sections();
        // Consider the page built only once a session is available to load custom
        // emoji; otherwise the next refresh rebuilds so they still appear.
        self.imp()
            .emoji_built
            .set(self.imp().session.upgrade().is_some());
        log::info!(
            target: "paper_plane::content_picker",
            "emoji page built in {:?}; recent_count={}",
            started.elapsed(),
            recent.len(),
        );
    }

    /// Updates just the "Recently Used" rows at the head of the emoji model,
    /// leaving the standard and custom-emoji sections untouched.
    fn refresh_recent_emoji(&self) {
        let model = self.emoji_model();
        // Count the existing "Recently Used" rows (header + emoji rows) at the head.
        let mut old_len = 0u32;
        if let Some(first) = model.item(0).and_downcast::<glib::BoxedAnyObject>()
            && matches!(&*first.borrow::<EmojiRow>(), EmojiRow::Header { name, .. } if name == "Recently-Used")
        {
            old_len = 1;
            while let Some(item) = model.item(old_len).and_downcast::<glib::BoxedAnyObject>() {
                if matches!(&*item.borrow::<EmojiRow>(), EmojiRow::Header { .. }) {
                    break;
                }
                old_len += 1;
            }
        }
        let recent = utils::PaperPlaneSettings::default().strv("recent-emojis");
        let mut new_rows: Vec<glib::BoxedAnyObject> = Vec::new();
        if !recent.is_empty() {
            let values: Vec<String> = recent.iter().map(|value| value.to_string()).collect();
            push_emoji_section(&mut new_rows, &gettext("Recently Used"), "Recently-Used", &values);
        }
        model.splice(0, old_len, &new_rows);
    }

    /// Loads the installed custom-emoji sticker sets and appends each as a section
    /// of rows to the emoji model, with a matching pack icon in the navigation rail.
    fn load_custom_emoji_sections(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let client_id = session.client_().id();
        let generation = self.imp().custom_emoji_generation.get();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let Ok(tdlib::enums::StickerSets::StickerSets(data)) =
                    tdlib::functions::get_installed_sticker_sets(
                        tdlib::enums::StickerType::CustomEmoji,
                        client_id,
                    )
                    .await
                else {
                    return;
                };
                // Fetch several packs concurrently (each get_sticker_set is a TDLib
                // round-trip); append their (cheap) model rows one pack at a time,
                // yielding between packs. Widgets are only built for visible rows.
                let mut packs = futures::stream::iter(data.sets.into_iter().map(move |info| async move {
                    let set = tdlib::functions::get_sticker_set(info.id, client_id).await;
                    (info, set)
                }))
                .buffered(4);
                while let Some((info, set)) = packs.next().await {
                    if obj.imp().custom_emoji_generation.get() != generation {
                        return;
                    }
                    match set {
                        Ok(tdlib::enums::StickerSet::StickerSet(set)) => {
                            obj.append_custom_emoji_section(info.id, &set.title, set.stickers);
                            obj.append_emoji_rail_pack(&info);
                        }
                        Err(e) => log::warn!(
                            target: "paper_plane::content_picker",
                            "Failed to load custom emoji set {}: {e:?}",
                            info.id,
                        ),
                    }
                    glib::timeout_future(std::time::Duration::from_millis(0)).await;
                }
            }
        ));
    }

    /// Appends one custom-emoji pack (a header row + custom-emoji rows) to the emoji
    /// model.
    fn append_custom_emoji_section(
        &self,
        set_id: i64,
        title: &str,
        stickers: Vec<tdlib::types::Sticker>,
    ) {
        let model = self.emoji_model();
        model.append(&glib::BoxedAnyObject::new(EmojiRow::Header {
            title: title.to_owned(),
            name: custom_emoji_section_name(set_id),
        }));
        for chunk in stickers.chunks(EMOJI_ROW_LEN) {
            model.append(&glib::BoxedAnyObject::new(EmojiRow::CustomEmoji(
                chunk.to_vec(),
            )));
        }
    }

    /// Adds a custom-emoji pack's cover to the emoji navigation rail.
    fn append_emoji_rail_pack(&self, info: &tdlib::types::StickerSetInfo) {
        let picture = gtk::Picture::builder()
            .width_request(24)
            .height_request(24)
            .content_fit(gtk::ContentFit::Contain)
            .build();
        let button = gtk::Button::builder().child(&picture).build();
        button.add_css_class("flat");
        button.add_css_class("sticker-pack-rail-item");
        button.set_tooltip_text(Some(&info.title));
        let set_id = info.id;
        let name = custom_emoji_section_name(set_id);
        self.register_rail_section(name.clone(), &button);
        button.connect_clicked(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                obj.set_active_rail(&name);
                obj.scroll_emoji_to_section(&name);
            }
        ));

        let cover = info.covers.first().cloned();
        let thumbnail = info.thumbnail.as_ref().and_then(|thumbnail| {
            matches!(
                thumbnail.format,
                tdlib::enums::ThumbnailFormat::Jpeg
                    | tdlib::enums::ThumbnailFormat::Png
                    | tdlib::enums::ThumbnailFormat::Webp
            )
            .then(|| thumbnail.file.clone())
        });
        if cover.is_some() || thumbnail.is_some() {
            self.imp().rail_covers.borrow_mut().push(RailCover {
                button: button.downgrade(),
                picture: picture.downgrade(),
                cover,
                thumbnail,
                demand: Rc::new(Cell::new(false)),
                loaded: false,
            });
        }
        self.imp().navigation_rail.append(&button);
        self.update_rail_covers();
    }

    /// Scrolls the emoji list to the header row whose section name matches `name`.
    fn scroll_emoji_to_section(&self, name: &str) {
        let model = self.emoji_model();
        for position in 0..model.n_items() {
            let Some(item) = model.item(position).and_downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            if matches!(&*item.borrow::<EmojiRow>(), EmojiRow::Header { name: n, .. } if n == name) {
                self.imp()
                    .emoji_list
                    .scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
                break;
            }
        }
    }

    fn rebuild_rail(&self, page: PickerPage) {
        clear_box(&self.imp().navigation_rail);
        self.imp().rail_covers.borrow_mut().clear();
        self.imp().rail_sections.borrow_mut().clear();
        match page {
            PickerPage::Emoji => {
                for (title, icon, emojis) in EMOJI_SECTIONS {
                    if emojis.is_empty() {
                        continue;
                    }
                    let button = gtk::Button::from_icon_name(icon);
                    button.set_tooltip_text(Some(&gettext(*title)));
                    button.add_css_class("flat");
                    let name = title.replace(' ', "-");
                    self.register_rail_section(name.clone(), &button);
                    button.connect_clicked(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        move |_| {
                            obj.set_active_rail(&name);
                            obj.scroll_emoji_to_section(&name);
                        }
                    ));
                    self.imp().navigation_rail.append(&button);
                }
            }
            PickerPage::Stickers => {
                for (icon, label) in [
                    ("starred-symbolic", "Favorites"),
                    ("document-open-recent-symbolic", "Recently Used"),
                ] {
                    let button = gtk::Button::from_icon_name(icon);
                    button.set_tooltip_text(Some(&gettext(label)));
                    button.add_css_class("flat");
                    let name = gettext(label);
                    self.register_rail_section(name.clone(), &button);
                    button.connect_clicked(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        #[strong]
                        name,
                        move |_| {
                            obj.set_active_rail(&name);
                            obj.scroll_to_sticker_section(None, &name);
                        }
                    ));
                    self.imp().navigation_rail.append(&button);
                }
            }
            PickerPage::Gifs => {
                let button = gtk::Button::from_icon_name("document-open-recent-symbolic");
                button.set_tooltip_text(Some(&gettext("Saved GIFs")));
                button.add_css_class("flat");
                // The GIF page has a single section, so it is always the active one.
                button.add_css_class("rail-item");
                button.add_css_class("active");
                self.imp().navigation_rail.append(&button);
            }
        }
        // Default the highlight to the first section until a scroll refines it (the GIF
        // page registers no sections and keeps its own always-active button).
        let first = self
            .imp()
            .rail_sections
            .borrow()
            .first()
            .map(|(name, _)| name.clone());
        if let Some(name) = first {
            self.set_active_rail(&name);
        }
    }

    fn build_sticker_pack_rail(&self, infos: &[(bool, tdlib::types::StickerSetInfo)]) {
        self.rebuild_rail(PickerPage::Stickers);
        for (_, info) in infos {
            let picture = gtk::Picture::builder()
                .width_request(24)
                .height_request(24)
                .content_fit(gtk::ContentFit::Contain)
                .build();
            let button = gtk::Button::builder().child(&picture).build();
            button.add_css_class("flat");
            button.add_css_class("sticker-pack-rail-item");
            button.set_tooltip_text(Some(&info.title));
            let set_id = info.id;
            let title = info.title.clone();
            self.register_rail_section(title.clone(), &button);
            button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.set_active_rail(&title);
                    obj.scroll_to_sticker_section(Some(set_id), &title);
                }
            ));

            let cover = info.covers.first().cloned();
            let thumbnail = info.thumbnail.as_ref().and_then(|thumbnail| {
                matches!(
                    thumbnail.format,
                    tdlib::enums::ThumbnailFormat::Jpeg
                        | tdlib::enums::ThumbnailFormat::Png
                        | tdlib::enums::ThumbnailFormat::Webp
                )
                .then(|| thumbnail.file.clone())
            });
            // Defer the cover decode until the icon is actually scrolled into the
            // rail's viewport (see update_rail_covers). Loading every pack cover up
            // front swamps the decode limiter and delays the visible grid previews.
            if cover.is_some() || thumbnail.is_some() {
                self.imp().rail_covers.borrow_mut().push(RailCover {
                    button: button.downgrade(),
                    picture: picture.downgrade(),
                    cover,
                    thumbnail,
                    demand: Rc::new(Cell::new(false)),
                    loaded: false,
                });
            }
            self.imp().navigation_rail.append(&button);
        }
    }

    fn setup_rail_scrolling(&self) {
        let rail_scroll = self.imp().navigation_rail_scroll.get();
        // The rail is a horizontal strip: never show scrollbars, and let a mouse
        // wheel (either axis) scroll it horizontally.
        rail_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll(clone!(
            #[weak(rename_to = obj)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, dx, dy| {
                let adjustment = obj.imp().navigation_rail_scroll.hadjustment();
                let step = adjustment.step_increment().max(40.0);
                let max = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
                let value = (adjustment.value() + (dx + dy) * step).clamp(adjustment.lower(), max);
                adjustment.set_value(value);
                glib::Propagation::Stop
            }
        ));
        rail_scroll.add_controller(scroll);

        let adjustment = rail_scroll.hadjustment();
        // `value_changed` fires while scrolling the rail; `changed` fires once the rail
        // is (re)allocated, which is when button positions first become known.
        adjustment.connect_value_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.update_rail_covers()
        ));
        adjustment.connect_changed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.update_rail_covers()
        ));
    }

    /// Registers a rail button as the entry for the content section named `name`,
    /// so scrolling can highlight it.
    fn register_rail_section(&self, name: String, button: &gtk::Button) {
        button.add_css_class("rail-item");
        self.imp()
            .rail_sections
            .borrow_mut()
            .push((name, button.downgrade()));
    }

    /// Marks `name`'s rail button active and clears the others.
    fn set_active_rail(&self, name: &str) {
        for (section, button) in self.imp().rail_sections.borrow().iter() {
            if let Some(button) = button.upgrade() {
                if section == name {
                    button.add_css_class("active");
                } else {
                    button.remove_css_class("active");
                }
            }
        }
    }

    fn update_rail_covers(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let adjustment = self.imp().navigation_rail_scroll.hadjustment();
        let start = adjustment.value();
        let page = if adjustment.page_size() > 0.0 {
            adjustment.page_size()
        } else {
            self.imp().navigation_rail_scroll.width() as f64
        };
        let end = start + page;
        // Preload roughly a viewport on either side so covers are ready by the time
        // the user scrolls to them.
        let margin = page;
        let preview_pixel_size = 24 * self.scale_factor().max(1);
        let rail = self.imp().navigation_rail.get();
        let mut covers = self.imp().rail_covers.borrow_mut();
        for entry in covers.iter_mut() {
            if entry.loaded {
                continue;
            }
            let Some(button) = entry.button.upgrade() else {
                continue;
            };
            // Position within the rail box; None until the button is laid out, in
            // which case a later `changed` signal will revisit it.
            let Some(bounds) = button.compute_bounds(&rail) else {
                continue;
            };
            if bounds.width() == 0.0 {
                continue;
            }
            let button_start = bounds.x() as f64;
            let button_end = button_start + bounds.width() as f64;
            if button_end < start - margin || button_start > end + margin {
                continue;
            }

            entry.loaded = true;
            entry.demand.set(true);
            let Some(picture) = entry.picture.upgrade() else {
                continue;
            };
            let demand = entry.demand.clone();
            let cover = entry.cover.clone();
            let thumbnail = entry.thumbnail.clone();
            utils::spawn(clone!(
                #[weak]
                picture,
                #[weak]
                session,
                async move {
                    let result = if let Some(cover) = cover {
                        ui::load_picker_preview(&cover, preview_pixel_size, &session, demand).await
                    } else if let Some(thumbnail) = thumbnail {
                        ui::load_picker_file_preview(thumbnail, preview_pixel_size, &session, demand)
                            .await
                    } else {
                        return;
                    };
                    if let Ok(texture) = result {
                        picture.set_paintable(Some(&texture));
                    }
                }
            ));
        }
    }

    fn scroll_to_sticker_section(&self, set_id: Option<i64>, title: &str) {
        for position in 0..self.sticker_model().n_items() {
            let Some(row) = self
                .sticker_model()
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
            else {
                continue;
            };
            let matches = match &*row.borrow::<StickerRow>() {
                StickerRow::Header {
                    title: row_title,
                    set_id: row_set_id,
                } => row_set_id == &set_id && (set_id.is_some() || row_title == title),
                _ => false,
            };
            if matches {
                self.imp()
                    .sticker_list
                    .scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
                return;
            }
        }
    }

    fn load_sticker_content(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let client_id = session.client_().id();
        let generation = self.next_sticker_generation();
        self.sticker_model().remove_all();
        log::info!(target: "paper_plane::content_picker", "TDLib sticker load started; client_id={client_id}");

        // The sticker tab shows only regular sticker packs; custom emoji now live in
        // the emoji page (see load_custom_emoji_sections).
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let total_started = Instant::now();
                let favorites_started = Instant::now();
                let recent_started = Instant::now();
                let sets_started = Instant::now();
                let (favorites, recent, sets) = futures::future::join3(
                    tdlib::functions::get_favorite_stickers(client_id),
                    tdlib::functions::get_recent_stickers(false, client_id),
                    tdlib::functions::get_installed_sticker_sets(
                        tdlib::enums::StickerType::Regular,
                        client_id,
                    ),
                )
                .await;
                log_tdlib_result("favorite stickers", favorites_started, &favorites);
                log_tdlib_result("recent stickers", recent_started, &recent);
                log_tdlib_result("installed sticker sets", sets_started, &sets);
                if !obj.sticker_generation_is(generation) {
                    return;
                }
                obj.populate_stickers(
                    generation,
                    favorites.ok(),
                    recent.ok(),
                    sets.ok(),
                    None,
                )
                .await;
                log::info!(
                    target: "paper_plane::content_picker",
                    "TDLib sticker load and widget population finished in {:?}; total_stickers={}",
                    total_started.elapsed(),
                    obj.sticker_model().n_items(),
                );
            }
        ));
    }

    fn load_gif_content(&self) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let client_id = session.client_().id();
        log::info!(target: "paper_plane::content_picker", "TDLib GIF load started; client_id={client_id}");
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let started = Instant::now();
                let animations = tdlib::functions::get_saved_animations(client_id).await;
                log_tdlib_result("saved GIFs", started, &animations);
                if let Ok(tdlib::enums::Animations::Animations(data)) = animations {
                    obj.populate_gifs(data.animations);
                }
            }
        ));
    }

    async fn populate_stickers(
        &self,
        generation: u64,
        favorites: Option<tdlib::enums::Stickers>,
        recent: Option<tdlib::enums::Stickers>,
        sets: Option<tdlib::enums::StickerSets>,
        emoji_sets: Option<tdlib::enums::StickerSets>,
    ) {
        let started = Instant::now();
        for (title, data) in [("Favorites", favorites), ("Recently Used", recent)] {
            if let Some(tdlib::enums::Stickers::Stickers(data)) = data {
                self.append_sticker_section(&gettext(title), data.stickers);
            }
        }
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let client_id = session.client_().id();
        let infos = [(false, sets), (true, emoji_sets)]
            .into_iter()
            .flat_map(|(custom, list)| match list {
                Some(tdlib::enums::StickerSets::StickerSets(data)) => data
                    .sets
                    .into_iter()
                    .map(move |info| (custom, info))
                    .collect(),
                None => Vec::new(),
            })
            .collect::<Vec<_>>();
        self.build_sticker_pack_rail(&infos);
        let cache_path = sticker_set_cache_path(&session);
        let mut previous_cache = load_sticker_set_cache(cache_path.clone()).await;
        let mut next_cache = StickerSetCache {
            version: STICKER_SET_CACHE_VERSION,
            sets: HashMap::new(),
        };
        let loading_position = self.sticker_model().n_items();
        if !infos.is_empty() {
            self.sticker_model()
                .append(&glib::BoxedAnyObject::new(StickerRow::Loading(gettext(
                    "Sticker packs",
                ))));
        }
        let mut packs = futures::stream::iter(infos.into_iter().map(|(custom, info)| {
            let signature = StickerSetSignature::from(&info);
            let cached = previous_cache.sets.remove(&info.id);
            async move {
                let started = Instant::now();
                if let Some(cached) = cached.as_ref()
                    && cached.signature == signature
                {
                    return (
                        custom,
                        info.id,
                        info.title,
                        started.elapsed(),
                        Ok((cached.clone(), true, None)),
                    );
                }
                match tdlib::functions::get_sticker_set(info.id, client_id).await {
                    Ok(tdlib::enums::StickerSet::StickerSet(set)) => (
                        custom,
                        info.id,
                        info.title,
                        started.elapsed(),
                        Ok((CachedStickerSet { signature, set }, false, None)),
                    ),
                    Err(error) => {
                        if let Some(cached) = cached {
                            (
                                custom,
                                info.id,
                                info.title,
                                started.elapsed(),
                                Ok((cached, true, Some(error))),
                            )
                        } else {
                            (custom, info.id, info.title, started.elapsed(), Err(error))
                        }
                    }
                }
            }
        }))
        .buffered(4);
        let mut pack_count = 0;
        let mut loading_visible = true;
        while let Some((custom, set_id, title, elapsed, result)) = packs.next().await {
            if !self.sticker_generation_is(generation) {
                return;
            }
            while self.imp().sticker_scrolling.get() {
                glib::timeout_future(std::time::Duration::from_millis(50)).await;
                if !self.sticker_generation_is(generation) {
                    return;
                }
            }
            let mutation_started = Instant::now();
            if loading_visible && self.sticker_model().n_items() > loading_position {
                self.sticker_model().remove(loading_position);
                loading_visible = false;
            }
            match result {
                Ok((cached, cache_hit, refresh_error)) => {
                    let sticker_count = cached.set.stickers.len();
                    self.append_sticker_set_section(
                        set_id,
                        &cached.set.title,
                        cached.set.stickers.clone(),
                    );
                    next_cache.sets.insert(set_id, cached.clone());
                    pack_count += 1;
                    log::info!(
                        target: "paper_plane::content_picker",
                        "pack populated; id={set_id} custom={custom} cached={cache_hit} stickers={sticker_count} elapsed={elapsed:?} title={:?}",
                        cached.set.title,
                    );
                    if let Some(error) = refresh_error {
                        log::warn!(
                            target: "paper_plane::content_picker",
                            "using stale cached sticker pack; id={set_id} error={error:?}"
                        );
                    }
                }
                Err(error) => {
                    self.sticker_model()
                        .append(&glib::BoxedAnyObject::new(StickerRow::Error {
                            set_id,
                            title,
                        }));
                    log::warn!(
                        target: "paper_plane::content_picker",
                        "failed to load sticker pack; id={set_id} custom={custom} elapsed={elapsed:?} error={error:?}",
                    );
                }
            }
            let mutation_elapsed = mutation_started.elapsed();
            if mutation_elapsed.as_millis() > 16 {
                log::warn!(
                    target: "paper_plane::content_picker",
                    "slow sticker model mutation; elapsed={mutation_elapsed:?}"
                );
            }
            glib::timeout_future(std::time::Duration::from_millis(1)).await;
        }
        save_sticker_set_cache(cache_path, next_cache).await;
        log::info!(
            target: "paper_plane::content_picker",
            "sticker population finished in {:?}; packs={} widgets={}",
            started.elapsed(),
            pack_count,
            self.sticker_model().n_items(),
        );
    }

    fn next_sticker_generation(&self) -> u64 {
        let generation = self.imp().sticker_generation.get().wrapping_add(1);
        self.imp().sticker_generation.set(generation);
        generation
    }

    fn sticker_generation_is(&self, generation: u64) -> bool {
        self.imp().sticker_generation.get() == generation
    }

    fn retry_sticker_set(&self, set_id: i64) {
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let generation = self.imp().sticker_generation.get();
        let client_id = session.client_().id();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                if let Ok(tdlib::enums::StickerSet::StickerSet(set)) =
                    tdlib::functions::get_sticker_set(set_id, client_id).await
                    && obj.sticker_generation_is(generation)
                    && let Some(position) = obj.sticker_set_error_position(set_id)
                {
                    obj.sticker_model().remove(position);
                    obj.insert_sticker_section(position, Some(set_id), &set.title, set.stickers);
                }
            }
        ));
    }

    fn sticker_set_error_position(&self, set_id: i64) -> Option<u32> {
        for position in 0..self.sticker_model().n_items() {
            let Some(row) = self
                .sticker_model()
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
            else {
                continue;
            };
            if matches!(&*row.borrow::<StickerRow>(), StickerRow::Error { set_id: id, .. } if *id == set_id)
            {
                return Some(position);
            }
        }
        None
    }

    fn append_sticker_section(&self, title: &str, stickers: Vec<tdlib::types::Sticker>) {
        self.insert_sticker_section(self.sticker_model().n_items(), None, title, stickers);
    }

    fn append_sticker_set_section(
        &self,
        set_id: i64,
        title: &str,
        stickers: Vec<tdlib::types::Sticker>,
    ) {
        self.insert_sticker_section(
            self.sticker_model().n_items(),
            Some(set_id),
            title,
            stickers,
        );
    }

    fn insert_sticker_section(
        &self,
        position: u32,
        set_id: Option<i64>,
        title: &str,
        stickers: Vec<tdlib::types::Sticker>,
    ) {
        if stickers.is_empty() {
            return;
        }
        let started = Instant::now();
        let sticker_count = stickers.len();
        let mut rows = vec![glib::BoxedAnyObject::new(StickerRow::Header {
            title: title.to_string(),
            set_id,
        })];
        for row in stickers.chunks(5) {
            rows.push(glib::BoxedAnyObject::new(StickerRow::Stickers(
                row.to_vec(),
            )));
        }
        self.sticker_model().splice(position, 0, &rows);
        let elapsed = started.elapsed();
        log::info!(
            target: "paper_plane::content_picker",
            "sticker section widgets created; title={title:?} count={sticker_count} elapsed={elapsed:?}"
        );
        if elapsed.as_millis() > 100 {
            log::warn!(
                target: "paper_plane::content_picker",
                "slow main-thread sticker section; title={title:?} count={sticker_count} elapsed={elapsed:?}"
            );
        }
    }

    fn activate_sticker(&self, sticker: tdlib::types::Sticker) {
        self.emit_by_name::<()>("content-picked", &[]);
        if self.imp().mode.get() == PickerMode::Reaction {
            // In reaction mode the sticker tab only shows custom emoji.
            if let tdlib::enums::StickerFullType::CustomEmoji(custom) = sticker.full_type {
                let reaction_type =
                    model::BoxedReactionType(tdlib::enums::ReactionType::CustomEmoji(
                        tdlib::types::ReactionTypeCustomEmoji {
                            custom_emoji_id: custom.custom_emoji_id,
                        },
                    ));
                self.emit_by_name::<()>("reaction-picked", &[&reaction_type]);
            }
            return;
        }
        if let tdlib::enums::StickerFullType::CustomEmoji(custom) = sticker.full_type {
            utils::ancestor::<_, ui::Session>(self)
                .insert_custom_emoji(&sticker.emoji, custom.custom_emoji_id);
            return;
        }
        let Some(chat) = self.imp().chat.upgrade() else {
            return;
        };
        let content = tdlib::enums::InputMessageContent::InputMessageSticker(
            tdlib::types::InputMessageSticker {
                sticker: tdlib::enums::InputFile::Id(tdlib::types::InputFileId {
                    id: sticker.sticker.id,
                }),
                thumbnail: None,
                width: sticker.width,
                height: sticker.height,
                emoji: sticker.emoji,
            },
        );
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::send_message(
                chat.id(),
                None,
                None,
                None,
                content,
                chat.session_().client_().id(),
            )
            .await
            {
                log::warn!("Failed to send sticker: {e:?}");
            }
        });
    }

    fn populate_gifs(&self, animations: Vec<tdlib::types::Animation>) {
        let started = Instant::now();
        let animation_count = animations.len();
        self.gif_model().remove_all();
        let rows: Vec<glib::BoxedAnyObject> = animations
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        *self.imp().animations.borrow_mut() = animations;
        self.gif_model().splice(0, 0, &rows);
        log::info!(
            target: "paper_plane::content_picker",
            "GIF model populated; count={} elapsed={:?}",
            animation_count,
            started.elapsed(),
        );
    }

    fn activate_gif(&self, index: usize) {
        let Some(animation) = self.imp().animations.borrow().get(index).cloned() else {
            return;
        };
        let Some(chat) = self.imp().chat.upgrade() else {
            return;
        };
        self.emit_by_name::<()>("content-picked", &[]);
        let content = tdlib::enums::InputMessageContent::InputMessageAnimation(
            tdlib::types::InputMessageAnimation {
                animation: tdlib::enums::InputFile::Id(tdlib::types::InputFileId {
                    id: animation.animation.id,
                }),
                thumbnail: None,
                added_sticker_file_ids: vec![],
                duration: animation.duration,
                width: animation.width,
                height: animation.height,
                caption: None,
                show_caption_above_media: false,
                has_spoiler: false,
            },
        );
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::send_message(
                chat.id(),
                None,
                None,
                None,
                content,
                chat.session_().client_().id(),
            )
            .await
            {
                log::warn!("Failed to send GIF: {e:?}");
            }
        });
    }

    fn search(&self, query: &str) {
        let query = query.trim().to_lowercase();
        log::debug!(
            target: "paper_plane::content_picker",
            "search changed; query_len={} emoji_tab={} sticker_tab={} gif_tab={}",
            query.chars().count(),
            self.imp().emoji_tab.is_active(),
            self.imp().sticker_tab.is_active(),
            self.imp().gif_tab.is_active(),
        );
        if self.imp().emoji_tab.is_active() {
            if query.is_empty() {
                // Only rebuild the (large) static emoji page when we're actually
                // returning from a search; tab switches and redundant clears skip it.
                if self.imp().emoji_search_active.replace(false) {
                    self.build_emoji_page();
                }
            } else if let Some(session) = self.imp().session.upgrade() {
                self.imp().emoji_search_active.set(true);
                let client_id = session.client_().id();
                utils::spawn(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    async move {
                        if let Ok(tdlib::enums::EmojiKeywords::EmojiKeywords(data)) =
                            tdlib::functions::search_emojis(query, vec![], client_id).await
                        {
                            let mut emojis = Vec::new();
                            for item in data.emoji_keywords {
                                if !emojis.contains(&item.emoji) {
                                    emojis.push(item.emoji);
                                }
                            }
                            let mut rows: Vec<glib::BoxedAnyObject> = Vec::new();
                            push_emoji_section(
                                &mut rows,
                                &gettext("Search Results"),
                                "Search-Results",
                                &emojis,
                            );
                            let model = obj.emoji_model();
                            model.remove_all();
                            model.splice(0, 0, &rows);
                        }
                    }
                ));
            }
        } else if self.imp().sticker_tab.is_active() && query.is_empty() {
            if self.imp().sticker_search_active.replace(false) {
                log::info!(
                    target: "paper_plane::content_picker",
                    "sticker search cleared; restoring installed content"
                );
                self.load_sticker_content();
            }
        } else if self.imp().sticker_tab.is_active() {
            self.imp().sticker_search_active.set(true);
            let generation = self.next_sticker_generation();
            let Some(session) = self.imp().session.upgrade() else {
                return;
            };
            let client_id = session.client_().id();
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    if let Ok(tdlib::enums::Stickers::Stickers(data)) =
                        tdlib::functions::search_stickers(
                            tdlib::enums::StickerType::Regular,
                            String::new(),
                            query,
                            vec![],
                            0,
                            100,
                            client_id,
                        )
                        .await
                        && obj.sticker_generation_is(generation)
                    {
                        obj.sticker_model().remove_all();
                        obj.append_sticker_section(&gettext("Search Results"), data.stickers);
                    }
                }
            ));
        }
    }
}

fn log_tdlib_result<T>(label: &str, started: Instant, result: &Result<T, tdlib::types::Error>) {
    match result {
        Ok(_) => log::info!(
            target: "paper_plane::content_picker",
            "TDLib request finished; request={label:?} elapsed={:?}",
            started.elapsed(),
        ),
        Err(error) => log::warn!(
            target: "paper_plane::content_picker",
            "TDLib request failed; request={label:?} elapsed={:?} error={error:?}",
            started.elapsed(),
        ),
    }
}

/// The section name of a custom-emoji pack in the emoji model.
fn custom_emoji_section_name(set_id: i64) -> String {
    format!("custom-emoji-{set_id}")
}

/// Pushes a section (header row + chunked emoji rows) onto the emoji model rows.
fn push_emoji_section(
    rows: &mut Vec<glib::BoxedAnyObject>,
    title: &str,
    name: &str,
    emojis: &[String],
) {
    rows.push(glib::BoxedAnyObject::new(EmojiRow::Header {
        title: title.to_owned(),
        name: name.to_owned(),
    }));
    // The whole section is one grid row (rendered as an 8-column GtkGrid), so every
    // line has a consistent 8 columns. Standard emoji are cheap text labels, and the
    // ListView still only realizes the visible sections.
    if !emojis.is_empty() {
        rows.push(glib::BoxedAnyObject::new(EmojiRow::Emoji(emojis.to_vec())));
    }
}

fn sticker_set_cache_path(session: &model::ClientStateSession) -> PathBuf {
    utils::data_dir()
        .join(session.client_().database_info().0.directory_base_name)
        .join("sticker-sets-v1.json")
}

async fn load_sticker_set_cache(path: PathBuf) -> StickerSetCache {
    let started = Instant::now();
    let result = gio::spawn_blocking(move || {
        let data = fs::read(path)?;
        serde_json::from_slice::<StickerSetCache>(&data)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
    .await;
    match result {
        Ok(Ok(cache)) if cache.version == STICKER_SET_CACHE_VERSION => {
            log::info!(
                target: "paper_plane::content_picker",
                "sticker set cache loaded; sets={} elapsed={:?}",
                cache.sets.len(),
                started.elapsed(),
            );
            cache
        }
        Ok(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            log::warn!(
                target: "paper_plane::content_picker",
                "failed to load sticker set cache: {error}"
            );
            StickerSetCache::default()
        }
        _ => StickerSetCache::default(),
    }
}

async fn save_sticker_set_cache(path: PathBuf, cache: StickerSetCache) {
    let set_count = cache.sets.len();
    let started = Instant::now();
    let result = gio::spawn_blocking(move || {
        let data = serde_json::to_vec(&cache)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, data)?;
        fs::rename(temporary, path)
    })
    .await;
    match result {
        Ok(Ok(())) => log::info!(
            target: "paper_plane::content_picker",
            "sticker set cache saved; sets={set_count} elapsed={:?}",
            started.elapsed(),
        ),
        Ok(Err(error)) => log::warn!(
            target: "paper_plane::content_picker",
            "failed to save sticker set cache: {error}"
        ),
        Err(_) => log::warn!(
            target: "paper_plane::content_picker",
            "sticker set cache writer failed"
        ),
    }
}

fn section_title(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .build()
}

fn status_row(title: &str, message: &str, retry: Option<Box<dyn Fn()>>) -> gtk::Widget {
    let row = gtk::Box::builder()
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    labels.append(&section_title(title));
    labels.append(
        &gtk::Label::builder()
            .label(message)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build(),
    );
    row.append(&labels);
    if let Some(retry) = retry {
        let button = gtk::Button::with_label(&gettext("Retry"));
        button.set_valign(gtk::Align::Center);
        button.connect_clicked(move |_| retry());
        row.append(&button);
    }
    row.upcast()
}

/// A fixed `EMOJI_ROW_LEN`-column grid of emoji buttons. A column-homogeneous
/// `GtkGrid` guarantees exactly that many per line (unlike a `FlowBox`, whose
/// homogeneous natural width dropped the 8th column to a new line).
fn emoji_grid<'a>(picker: &ContentPicker, emojis: impl Iterator<Item = &'a str>) -> gtk::Grid {
    let grid = gtk::Grid::builder().column_homogeneous(true).build();
    for (i, emoji) in emojis.enumerate() {
        let button = gtk::Button::with_label(emoji);
        button.add_css_class("flat");
        button.add_css_class("emoji-picker-item");
        let value = emoji.to_string();
        button.connect_clicked(clone!(
            #[weak]
            picker,
            move |_| {
                if picker.imp().mode.get() == PickerMode::Reaction {
                    let reaction_type = model::BoxedReactionType(tdlib::enums::ReactionType::Emoji(
                        tdlib::types::ReactionTypeEmoji {
                            emoji: value.clone(),
                        },
                    ));
                    picker.emit_by_name::<()>("reaction-picked", &[&reaction_type]);
                } else {
                    picker.remember_emoji(&value);
                    utils::ancestor::<_, ui::Session>(&picker).insert_emoji(&value);
                }
                picker.emit_by_name::<()>("content-picked", &[]);
            }
        ));
        let column = (i % EMOJI_ROW_LEN) as i32;
        let row = (i / EMOJI_ROW_LEN) as i32;
        grid.attach(&button, column, row, 1, 1);
    }
    grid
}

/// Factory setup that makes a list item non-selectable and non-activatable, so the
/// `ListView` draws no row hover/selection background under the emoji/sticker cells.
fn setup_unselectable_item(_: &gtk::SignalListItemFactory, item: &glib::Object) {
    if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
        item.set_activatable(false);
        item.set_selectable(false);
    }
}

impl ContentPicker {
    pub(crate) fn connect_content_picked<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("content-picked", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);
            None
        })
    }

    /// Switches this picker into reaction mode targeting `(chat_id, message_id)`:
    /// picking an emoji or custom emoji emits `reaction-picked` instead of editing
    /// the message entry. The reaction picker is a single emoji + custom-emoji panel
    /// (tdesktop-style), so the tab bar, sticker tab, and GIF tab are hidden.
    pub(crate) fn set_reaction_mode(&self, chat_id: i64, message_id: i64) {
        let imp = self.imp();
        imp.mode.set(PickerMode::Reaction);
        imp.reaction_target.set(Some((chat_id, message_id)));
        imp.gif_tab.set_visible(false);
        imp.sticker_tab.set_visible(false);
        // Hide the whole tab bar — only the emoji (+ custom emoji) page remains.
        if let Some(tabs) = imp.emoji_tab.parent() {
            tabs.set_visible(false);
        }
    }

    pub(crate) fn connect_reaction_picked<F: Fn(&Self, model::BoxedReactionType) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("reaction-picked", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let reaction_type = values[1].get::<model::BoxedReactionType>().unwrap();
            f(&obj, reaction_type);
            None
        })
    }

    fn remember_emoji(&self, emoji: &str) {
        let settings = utils::PaperPlaneSettings::default();
        let mut recent: Vec<String> = settings
            .strv("recent-emojis")
            .iter()
            .map(ToString::to_string)
            .collect();
        recent.retain(|e| e != emoji);
        recent.insert(0, emoji.to_string());
        recent.truncate(32);
        let refs: Vec<&str> = recent.iter().map(String::as_str).collect();
        let _ = settings.set_strv("recent-emojis", refs);
    }
}
fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
