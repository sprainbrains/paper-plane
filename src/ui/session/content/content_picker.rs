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
        pub(super) stickers_loaded: Cell<bool>,
        pub(super) gifs_loaded: Cell<bool>,
        pub(super) sticker_search_active: Cell<bool>,
        pub(super) sticker_generation: Cell<u64>,
        pub(super) sticker_scrolling: Rc<Cell<bool>>,
        pub(super) scroll_timeout: RefCell<Option<glib::SourceId>>,
        pub(super) sticker_model: OnceCell<gio::ListStore>,
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
        pub(super) emoji_sections: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) sticker_list: TemplateChild<gtk::ListView>,
        #[template_child]
        pub(super) gif_grid: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub(super) navigation_rail: TemplateChild<gtk::Box>,
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
            SIGNALS.get_or_init(|| vec![Signal::builder("content-picked").build()])
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
            obj.setup_sticker_list();
            obj.setup_sticker_scrolling();
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
            self.emoji_sections.unparent();
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

#[derive(Debug, Clone)]
enum StickerRow {
    Header { title: String, set_id: Option<i64> },
    Stickers(Vec<tdlib::types::Sticker>),
    Loading(String),
    Error { set_id: i64, title: String },
}

const STICKER_SET_CACHE_VERSION: u32 = 1;

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
    fn sticker_model(&self) -> &gio::ListStore {
        self.imp().sticker_model.get().unwrap()
    }

    fn setup_sticker_list(&self) {
        let model = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::NoSelection::new(Some(model.clone()));
        self.imp().sticker_list.set_model(Some(&selection));
        self.imp().sticker_model.set(model).unwrap();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().unwrap();
                let row = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
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
        self.imp()
            .sticker_page
            .vadjustment()
            .connect_value_changed(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    let imp = obj.imp();
                    imp.sticker_scrolling.set(true);
                    if let Some(source) = imp.scroll_timeout.borrow_mut().take() {
                        source.remove();
                    }
                    let source = glib::timeout_add_local_once(
                        std::time::Duration::from_millis(150),
                        clone!(
                            #[weak]
                            obj,
                            move || {
                                let imp = obj.imp();
                                imp.scroll_timeout.borrow_mut().take();
                                imp.sticker_scrolling.set(false);
                                imp.sticker_list.queue_draw();
                            }
                        ),
                    );
                    *imp.scroll_timeout.borrow_mut() = Some(source);
                }
            ));
    }

    pub(crate) fn refresh(&self) {
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
        clear_box(&self.imp().emoji_sections);
        let settings = utils::PaperPlaneSettings::default();
        let recent = settings.strv("recent-emojis");
        if !recent.is_empty() {
            self.imp()
                .emoji_sections
                .append(&section_title(&gettext("Recently Used")));
            let values: Vec<&str> = recent.iter().map(|value| value.as_str()).collect();
            let grid = emoji_grid(self, values.into_iter());
            grid.set_widget_name("Recently-Used");
            self.imp().emoji_sections.append(&grid);
        }
        for (title, _, emojis) in EMOJI_SECTIONS {
            if emojis.is_empty() {
                continue;
            }
            self.imp()
                .emoji_sections
                .append(&section_title(&gettext(*title)));
            let grid = emoji_grid(self, emojis.split_whitespace());
            grid.set_widget_name(&title.replace(' ', "-"));
            self.imp().emoji_sections.append(&grid);
        }
        self.rebuild_rail(PickerPage::Emoji);
        log::info!(
            target: "paper_plane::content_picker",
            "emoji page built in {:?}; recent_count={}",
            started.elapsed(),
            recent.len(),
        );
    }

    fn rebuild_rail(&self, page: PickerPage) {
        clear_box(&self.imp().navigation_rail);
        match page {
            PickerPage::Emoji => {
                for (title, icon, emojis) in EMOJI_SECTIONS {
                    if emojis.is_empty() {
                        continue;
                    }
                    let button = gtk::Button::from_icon_name(icon);
                    button.set_tooltip_text(Some(&gettext(*title)));
                    button.add_css_class("flat");
                    let page = self.imp().emoji_page.get();
                    let sections = self.imp().emoji_sections.get();
                    let name = title.replace(' ', "-");
                    button.connect_clicked(move |_| {
                        if let Some(child) = sections.first_child().and_then(|mut c| {
                            loop {
                                if c.widget_name() == name {
                                    break Some(c);
                                }
                                match c.next_sibling() {
                                    Some(next) => c = next,
                                    None => break None,
                                }
                            }
                        }) && let Some(point) =
                            child.compute_point(&sections, &gtk::graphene::Point::zero())
                        {
                            page.vadjustment().set_value(point.y() as f64);
                        }
                    });
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
                    button.connect_clicked(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        #[strong]
                        label,
                        move |_| obj.scroll_to_sticker_section(None, &gettext(label))
                    ));
                    self.imp().navigation_rail.append(&button);
                }
            }
            PickerPage::Gifs => {
                let button = gtk::Button::from_icon_name("document-open-recent-symbolic");
                button.set_tooltip_text(Some(&gettext("Saved GIFs")));
                button.add_css_class("flat");
                self.imp().navigation_rail.append(&button);
            }
        }
    }

    fn build_sticker_pack_rail(&self, infos: &[(bool, tdlib::types::StickerSetInfo)]) {
        self.rebuild_rail(PickerPage::Stickers);
        let Some(session) = self.imp().session.upgrade() else {
            return;
        };
        let preview_pixel_size = 24 * self.scale_factor().max(1);
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
            button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| obj.scroll_to_sticker_section(Some(set_id), &title)
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
                let demand = Rc::new(Cell::new(true));
                utils::spawn(clone!(
                    #[weak]
                    picture,
                    #[weak]
                    session,
                    async move {
                        let result = if let Some(cover) = cover {
                            ui::load_picker_preview(&cover, preview_pixel_size, &session, demand)
                                .await
                        } else {
                            ui::load_picker_file_preview(
                                thumbnail.unwrap(),
                                preview_pixel_size,
                                &session,
                                demand,
                            )
                            .await
                        };
                        if let Ok(texture) = result {
                            picture.set_paintable(Some(&texture));
                        }
                    }
                ));
            }
            self.imp().navigation_rail.append(&button);
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
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let total_started = Instant::now();
                let favorites_started = Instant::now();
                let recent_started = Instant::now();
                let sets_started = Instant::now();
                let emoji_sets_started = Instant::now();
                let (favorites, recent, sets, emoji_sets) = futures::future::join4(
                    tdlib::functions::get_favorite_stickers(client_id),
                    tdlib::functions::get_recent_stickers(false, client_id),
                    tdlib::functions::get_installed_sticker_sets(
                        tdlib::enums::StickerType::Regular,
                        client_id,
                    ),
                    tdlib::functions::get_installed_sticker_sets(
                        tdlib::enums::StickerType::CustomEmoji,
                        client_id,
                    ),
                )
                .await;
                log_tdlib_result("favorite stickers", favorites_started, &favorites);
                log_tdlib_result("recent stickers", recent_started, &recent);
                log_tdlib_result("installed sticker sets", sets_started, &sets);
                log_tdlib_result(
                    "installed custom emoji sets",
                    emoji_sets_started,
                    &emoji_sets,
                );
                if !obj.sticker_generation_is(generation) {
                    return;
                }
                obj.populate_stickers(
                    generation,
                    favorites.ok(),
                    recent.ok(),
                    sets.ok(),
                    emoji_sets.ok(),
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
        clear_flowbox(&self.imp().gif_grid);
        *self.imp().animations.borrow_mut() = animations;
        for (index, animation) in self.imp().animations.borrow().iter().enumerate() {
            let picture = gtk::Picture::builder()
                .width_request(104)
                .height_request(88)
                .content_fit(gtk::ContentFit::Cover)
                .build();
            if let Some(mini) = &animation.minithumbnail
                && let Ok(texture) = gtk::gdk::Texture::from_bytes(&glib::Bytes::from_owned(
                    glib::base64_decode(&mini.data),
                ))
            {
                picture.set_paintable(Some(&texture));
            }
            let button = gtk::Button::builder().child(&picture).build();
            button.add_css_class("flat");
            button.add_css_class("gif-picker-item");
            button.connect_clicked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| obj.activate_gif(index)
            ));
            self.imp().gif_grid.insert(&button, -1);
        }
        log::info!(
            target: "paper_plane::content_picker",
            "GIF widgets populated; count={} elapsed={:?}",
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
                self.build_emoji_page();
            } else if let Some(session) = self.imp().session.upgrade() {
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
                            clear_box(&obj.imp().emoji_sections);
                            obj.imp()
                                .emoji_sections
                                .append(&section_title(&gettext("Search Results")));
                            let values: Vec<&str> = emojis.iter().map(String::as_str).collect();
                            obj.imp()
                                .emoji_sections
                                .append(&emoji_grid(&obj, values.into_iter()));
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

fn emoji_grid<'a>(picker: &ContentPicker, emojis: impl Iterator<Item = &'a str>) -> gtk::FlowBox {
    let grid = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .max_children_per_line(8)
        .min_children_per_line(6)
        .row_spacing(2)
        .column_spacing(2)
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
                picker.remember_emoji(&value);
                utils::ancestor::<_, ui::Session>(&picker).insert_emoji(&value);
                picker.emit_by_name::<()>("content-picked", &[]);
            }
        ));
        grid.insert(&button, -1);
    }
    grid
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
fn clear_flowbox(container: &gtk::FlowBox) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
