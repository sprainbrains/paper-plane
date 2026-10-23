use std::cell::Cell;
use std::cell::OnceCell;
use std::cell::RefCell;
use std::sync::OnceLock;

use glib::clone;
use glib::subclass::Signal;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

/// How many candidates the mention popup lists at most.
const MENTION_SEARCH_LIMIT: i32 = 20;

/// Beyond this length a `@token` is no longer treated as a mention query.
const MENTION_QUERY_MAX_CHARS: usize = 64;

/// Finds an `@mention` token being typed at the very end of
/// `text_before_cursor`.
///
/// Returns the byte index of the `@` plus the query after it. The `@` must
/// start a word (text start or after whitespace) and the rest of the word may
/// not contain whitespace or a second `@`, so `mail@example.com` and URLs
/// never trigger the picker. An empty query (`@` typed alone) is valid and
/// lists the default set of mentionable members.
fn mention_prefix(text_before_cursor: &str) -> Option<(usize, &str)> {
    let mut token_start = 0;
    for (index, ch) in text_before_cursor.char_indices() {
        if ch.is_whitespace() {
            token_start = index + ch.len_utf8();
        }
    }
    let token = &text_before_cursor[token_start..];
    let query = token.strip_prefix('@')?;
    if query.contains('@') || query.chars().count() > MENTION_QUERY_MAX_CHARS {
        return None;
    }
    Some((token_start, query))
}

/// Converts a UTF-16 code-unit offset (TDLib entity offsets) to a character
/// index (GtkTextBuffer offsets), saturating at the end of `text`.
fn char_index_of_utf16(text: &str, units: i32) -> i32 {
    let mut seen = 0i32;
    for (index, ch) in text.chars().enumerate() {
        if seen >= units {
            return index as i32;
        }
        seen += ch.len_utf16() as i32;
    }
    text.chars().count() as i32
}

mod imp {
    use super::*;

    /// A custom emoji inserted into the buffer. The two marks bracket the emoji's plain
    /// text (start with right gravity, end with left gravity) so the range tracks the
    /// glyph as surrounding text is edited; the entity offset/length is re-derived from
    /// the marks on every buffer change instead of being lost.
    #[derive(Debug)]
    pub(crate) struct CustomEmojiAnchor {
        pub(super) start: gtk::TextMark,
        pub(super) end: gtk::TextMark,
        pub(super) custom_emoji_id: i64,
    }

    /// An @mention inserted into the buffer. Tracked exactly like a custom
    /// emoji anchor: the marks keep the MentionName entity's offsets correct
    /// while the surrounding text is edited, and a link-colored tag paints the
    /// span.
    #[derive(Debug)]
    pub(crate) struct MentionAnchor {
        pub(super) start: gtk::TextMark,
        pub(super) end: gtk::TextMark,
        pub(super) user_id: i64,
    }

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/components/message_entry.ui")]
    pub(crate) struct MessageEntry {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) formatted_text: RefCell<Option<model::BoxedFormattedText>>,
        pub(super) custom_emojis: RefCell<Vec<CustomEmojiAnchor>>,
        pub(super) mention_anchors: RefCell<Vec<MentionAnchor>>,
        /// Popover listing mention candidates; parented to the text view so it
        /// floats above the composer.
        pub(super) mention_popup: OnceCell<ui::MentionPopup>,
        /// Link color applied to anchored mention spans (created on demand).
        pub(super) mention_tag: OnceCell<gtk::TextTag>,
        /// Bumped on every buffer change; an in-flight member search whose
        /// generation no longer matches is dropped when it replies.
        pub(super) mention_generation: Cell<u64>,
        pub(super) mention_debounce: RefCell<Option<glib::SourceId>>,
        /// Character offset of the `@` the open popup was searching for, used
        /// to replace exactly that token when a candidate is chosen.
        pub(super) mention_token_start: Cell<i32>,
        #[template_child]
        pub(super) overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub(super) placeholder: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) emoji_button: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) text_view: TemplateChild<gtk::TextView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageEntry {
        const NAME: &'static str = "PaplMessageEntry";
        type Type = super::MessageEntry;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageEntry {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    Signal::builder("activate").build(),
                    Signal::builder("paste-clipboard").build(),
                    Signal::builder("emoji-button-press")
                        .param_types([gtk::Image::static_type()])
                        .build(),
                    Signal::builder("emoji-button-hover")
                        .param_types([gtk::Image::static_type()])
                        .build(),
                    Signal::builder("emoji-button-leave").build(),
                ]
            })
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecBoxed::builder::<model::BoxedFormattedText>("formatted-text")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecString::builder("placeholder-text")
                        .explicit_notify()
                        .build(),
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .explicit_notify()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "formatted-text" => obj.set_formatted_text(value.get().unwrap()),
                "placeholder-text" => obj.set_placeholder_text(value.get().unwrap()),
                "chat" => obj.set_chat(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "formatted-text" => obj.formatted_text().to_value(),
                "placeholder-text" => obj.placeholder_text().to_value(),
                "chat" => obj.chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            self.placeholder.connect_text_notify(clone!(
                #[weak]
                obj,
                move |_| obj.notify("placeholder-text")
            ));

            // Handle the enter key to emit the "activate" signal if neither the "ctrl" nor the
            // "shift" modifier are pressed at the same time.
            let key_events = gtk::EventControllerKey::new();
            key_events.connect_key_pressed(clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, modifier| {
                    // While the mention popup is open its navigation wins:
                    // Enter picks a candidate instead of sending.
                    if obj.mention_popup_key(key) {
                        return glib::Propagation::Stop;
                    }
                    if !modifier.contains(gdk::ModifierType::CONTROL_MASK)
                        && !modifier.contains(gdk::ModifierType::SHIFT_MASK)
                        && (key == gdk::Key::Return || key == gdk::Key::KP_Enter)
                    {
                        obj.emit_by_name::<()>("activate", &[]);
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
            ));
            self.text_view.add_controller(key_events);

            let popup = ui::MentionPopup::new();
            // Parent to the overlay, not the text view: GtkTextView tracks its
            // own child list, and a foreign widget (the popover) parented
            // directly to it sends GTK into a warning/resize loop when the
            // view is destroyed with the popup open (e.g. a closed send-media
            // window). The overlay hosts children properly, and the popup
            // still positions above the whole entry.
            popup.set_parent(&*self.overlay);
            popup.set_position(gtk::PositionType::Top);
            popup.connect_candidate_activated(clone!(
                #[weak]
                obj,
                move |_, user| obj.insert_mention(user)
            ));
            self.mention_popup.set(popup).ok();

            let press = gtk::GestureClick::new();
            press.connect_pressed(clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    obj.emit_by_name::<()>("emoji-button-press", &[&*obj.imp().emoji_button]);
                }
            ));
            self.emoji_button.add_controller(press);
            let hover = gtk::EventControllerMotion::new();
            hover.connect_enter(clone!(
                #[weak]
                obj,
                move |_, _, _| {
                    obj.emit_by_name::<()>("emoji-button-hover", &[&*obj.imp().emoji_button]);
                }
            ));
            hover.connect_leave(clone!(
                #[weak]
                obj,
                move |_| obj.emit_by_name::<()>("emoji-button-leave", &[])
            ));
            self.emoji_button.add_controller(hover);

            self.text_view.buffer().connect_changed(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.text_buffer_changed();
                }
            ));

            self.text_view.connect_paste_clipboard(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.emit_by_name::<()>("paste-clipboard", &[]);
                }
            ));
        }

        fn dispose(&self) {
            self.overlay.unparent();
        }
    }

    impl WidgetImpl for MessageEntry {
        fn grab_focus(&self) -> bool {
            self.text_view.grab_focus()
        }
    }

    #[gtk::template_callbacks]
    impl MessageEntry {
        #[template_callback]
        fn on_gesture_click_pressed(gesture_click: &gtk::GestureClick) {
            gesture_click.set_state(gtk::EventSequenceState::Claimed);
        }
    }
}

glib::wrapper! {
    pub(crate) struct MessageEntry(ObjectSubclass<imp::MessageEntry>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageEntry {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    fn text_buffer_changed(&self) {
        let imp = self.imp();
        let buffer = imp.text_view.buffer();
        let text: String = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .into();

        if text.is_empty() {
            self.clear_anchors();
            imp.formatted_text.replace(None);
            imp.placeholder.set_visible(true);
        } else {
            let entities = self.rebuild_anchor_entities();
            imp.formatted_text.replace(Some(model::BoxedFormattedText(
                tdlib::types::FormattedText { text, entities },
            )));

            imp.placeholder.set_visible(false);
        }

        self.notify("formatted-text");
        self.schedule_mention_update();
    }

    /// Re-derives every mark-anchored entity (custom emojis and @mentions)
    /// from the tracking marks against the buffer's current contents, dropping
    /// any whose text has been deleted (the marks collapsed).
    fn rebuild_anchor_entities(&self) -> Vec<tdlib::types::TextEntity> {
        let buffer = self.imp().text_view.buffer();
        let start = buffer.start_iter();
        let mut entities = Vec::new();

        self.imp().custom_emojis.borrow_mut().retain(|anchor| {
            let anchor_start = buffer.iter_at_mark(&anchor.start);
            let anchor_end = buffer.iter_at_mark(&anchor.end);
            if anchor_start.offset() >= anchor_end.offset() {
                buffer.delete_mark(&anchor.start);
                buffer.delete_mark(&anchor.end);
                return false;
            }

            let offset = buffer
                .text(&start, &anchor_start, true)
                .encode_utf16()
                .count() as i32;
            let length = buffer
                .text(&anchor_start, &anchor_end, true)
                .encode_utf16()
                .count() as i32;
            entities.push(tdlib::types::TextEntity {
                offset,
                length,
                r#type: tdlib::enums::TextEntityType::CustomEmoji(
                    tdlib::types::TextEntityTypeCustomEmoji {
                        custom_emoji_id: anchor.custom_emoji_id,
                    },
                ),
            });
            true
        });

        self.imp().mention_anchors.borrow_mut().retain(|anchor| {
            let anchor_start = buffer.iter_at_mark(&anchor.start);
            let anchor_end = buffer.iter_at_mark(&anchor.end);
            if anchor_start.offset() >= anchor_end.offset() {
                buffer.delete_mark(&anchor.start);
                buffer.delete_mark(&anchor.end);
                return false;
            }

            let offset = buffer
                .text(&start, &anchor_start, true)
                .encode_utf16()
                .count() as i32;
            let length = buffer
                .text(&anchor_start, &anchor_end, true)
                .encode_utf16()
                .count() as i32;
            entities.push(tdlib::types::TextEntity {
                offset,
                length,
                r#type: tdlib::enums::TextEntityType::MentionName(
                    tdlib::types::TextEntityTypeMentionName {
                        user_id: anchor.user_id,
                    },
                ),
            });
            true
        });

        entities.sort_by_key(|entity| entity.offset);
        entities
    }

    fn clear_anchors(&self) {
        let buffer = self.imp().text_view.buffer();
        for anchor in self.imp().custom_emojis.borrow_mut().drain(..) {
            buffer.delete_mark(&anchor.start);
            buffer.delete_mark(&anchor.end);
        }
        for anchor in self.imp().mention_anchors.borrow_mut().drain(..) {
            buffer.delete_mark(&anchor.start);
            buffer.delete_mark(&anchor.end);
        }
    }

    /// Insert text inside the message entry at the cursor position,
    /// deleting eventual selected text
    pub(crate) fn insert_at_cursor(&self, text: &str) {
        let buffer = self.imp().text_view.buffer();
        buffer.begin_user_action();
        buffer.delete_selection(true, true);
        buffer.insert_at_cursor(text);
        buffer.end_user_action();
    }

    pub(crate) fn insert_custom_emoji(&self, emoji: &str, custom_emoji_id: i64) {
        let buffer = self.imp().text_view.buffer();

        buffer.begin_user_action();
        buffer.delete_selection(true, true);
        let start_offset = buffer.cursor_position();
        buffer.insert_at_cursor(emoji);
        buffer.end_user_action();

        let start_iter = buffer.iter_at_offset(start_offset);
        let end_iter = buffer.iter_at_offset(start_offset + emoji.chars().count() as i32);
        // Right gravity on start, left gravity on end: boundary edits land outside the glyph.
        let start = buffer.create_mark(None, &start_iter, false);
        let end = buffer.create_mark(None, &end_iter, true);
        self.imp()
            .custom_emojis
            .borrow_mut()
            .push(imp::CustomEmojiAnchor {
                start,
                end,
                custom_emoji_id,
            });

        self.text_buffer_changed();
    }

    pub(crate) fn formatted_text(&self) -> Option<model::BoxedFormattedText> {
        self.imp().formatted_text.borrow().clone()
    }

    pub(crate) fn set_formatted_text(&self, formatted_text: Option<model::BoxedFormattedText>) {
        if self.formatted_text() == formatted_text {
            return;
        }

        let (text, mentions) = match formatted_text {
            Some(model::BoxedFormattedText(tdlib::types::FormattedText { text, entities })) => {
                let mentions: Vec<(i32, i32, i64)> = entities
                    .iter()
                    .filter_map(|entity| match &entity.r#type {
                        tdlib::enums::TextEntityType::MentionName(data) => Some((
                            char_index_of_utf16(&text, entity.offset),
                            char_index_of_utf16(&text, entity.offset + entity.length),
                            data.user_id,
                        )),
                        _ => None,
                    })
                    .collect();
                (text, mentions)
            }
            None => (String::new(), Vec::new()),
        };

        self.clear_anchors();
        let buffer = self.imp().text_view.buffer();
        buffer.set_text(&text);

        // Re-anchor mention entities carried by drafts so they survive further
        // edits and still go out as real mentions when the message is sent.
        if !mentions.is_empty() {
            let tag = self.mention_tag();
            for (start_offset, end_offset, user_id) in mentions {
                if start_offset >= end_offset {
                    continue;
                }
                let start_iter = buffer.iter_at_offset(start_offset);
                let end_iter = buffer.iter_at_offset(end_offset);
                let start = buffer.create_mark(None, &start_iter, false);
                let end = buffer.create_mark(None, &end_iter, true);
                buffer.apply_tag(&tag, &start_iter, &end_iter);
                self.imp()
                    .mention_anchors
                    .borrow_mut()
                    .push(imp::MentionAnchor {
                        start,
                        end,
                        user_id,
                    });
            }
            // `set_text` fired a buffer change before the anchors existed;
            // re-derive the formatted text so it carries the entities.
            self.text_buffer_changed();
        }
    }

    /// Link color for mention spans: the theme's accent color (`accent_color_rgba`
    /// falls back to the classic link blue when accent colors are off).
    /// Registered once per buffer.
    fn mention_tag(&self) -> gtk::TextTag {
        let imp = self.imp();
        if let Some(tag) = imp.mention_tag.get() {
            return tag.clone();
        }
        let color = adw::StyleManager::default().accent_color_rgba();
        let tag = gtk::TextTag::builder()
            .name("papl-mention")
            .foreground_rgba(&color)
            .build();
        imp.text_view.buffer().tag_table().add(&tag);
        imp.mention_tag.set(tag.clone()).ok();
        tag
    }

    /// Routes a key press to the mention popup while it is open; returns true
    /// when the key was consumed, so Enter selects a candidate instead of
    /// sending the message.
    fn mention_popup_key(&self, key: gdk::Key) -> bool {
        let Some(popup) = self
            .imp()
            .mention_popup
            .get()
            .filter(|popup| popup.is_visible())
        else {
            return false;
        };
        match key {
            gdk::Key::Up => popup.move_selected(-1),
            gdk::Key::Down => popup.move_selected(1),
            gdk::Key::Tab | gdk::Key::Return | gdk::Key::KP_Enter => popup.activate_selected(),
            gdk::Key::Escape => self.close_mention_popup(),
            _ => return false,
        }
        true
    }

    fn close_mention_popup(&self) {
        if let Some(popup) = self.imp().mention_popup.get() {
            popup.popdown();
        }
    }

    /// Restarts the debounce and bumps the generation on every buffer change:
    /// an in-flight member search whose generation no longer matches is
    /// dropped when it replies, so a late answer can never fill the popup for
    /// an already-edited token.
    fn schedule_mention_update(&self) {
        let imp = self.imp();
        imp.mention_generation
            .set(imp.mention_generation.get().wrapping_add(1));
        if let Some(source) = imp.mention_debounce.borrow_mut().take() {
            source.remove();
        }
        let source = glib::timeout_add_local_once(
            std::time::Duration::from_millis(150),
            clone!(
                #[weak(rename_to = obj)]
                self,
                move || {
                    obj.imp().mention_debounce.replace(None);
                    obj.mention_search_now();
                }
            ),
        );
        imp.mention_debounce.replace(Some(source));
    }

    /// If the cursor sits right after an `@token` in a chat with a member
    /// list, asks TDLib for mentionable members and shows the candidates.
    fn mention_search_now(&self) {
        let imp = self.imp();
        // Mentions only make sense where there is a member list to pick from
        // (groups, supergroups, channels); private and secret chats never get
        // the popup, and chats the user cannot write in have no entry at all.
        let Some(chat) = self.chat() else {
            self.close_mention_popup();
            return;
        };
        if !matches!(
            chat.chat_type(),
            model::ChatType::BasicGroup(_) | model::ChatType::Supergroup(_)
        ) {
            self.close_mention_popup();
            return;
        }

        let buffer = imp.text_view.buffer();
        let insert = buffer.iter_at_offset(buffer.cursor_position());
        let before_cursor: String = buffer.text(&buffer.start_iter(), &insert, false).into();
        let Some((token_byte, query)) = mention_prefix(&before_cursor) else {
            self.close_mention_popup();
            return;
        };
        imp.mention_token_start
            .set(before_cursor[..token_byte].chars().count() as i32);
        let generation = imp.mention_generation.get();

        let session = chat.session_();
        let client_id = session.client_().id();
        let chat_id = chat.id();
        let me = session.me_().id();
        let query = query.to_string();

        utils::spawn(clone!(
            #[weak(rename_to = as_this)]
            self,
            async move {
                // The mention filter is also the permission gate: it answers
                // only with members the current user may mention (in channels
                // that hide their member list, that is just admins and bots
                // for regular users; no access at all is an error).
                let filter = tdlib::enums::ChatMembersFilter::Mention(
                    tdlib::types::ChatMembersFilterMention { topic_id: None },
                );
                let result = tdlib::functions::search_chat_members(
                    chat_id,
                    query,
                    MENTION_SEARCH_LIMIT,
                    Some(filter),
                    client_id,
                )
                .await;
                let users: Vec<model::User> = match result {
                    Ok(tdlib::enums::ChatMembers::ChatMembers(members)) => members
                        .members
                        .into_iter()
                        .filter_map(|member| match member.member_id {
                            tdlib::enums::MessageSender::User(user) if user.user_id != me => {
                                Some(session.user_or_fetch(user.user_id))
                            }
                            _ => None,
                        })
                        .collect(),
                    Err(e) => {
                        log::debug!("mention search unavailable: {}", e.message);
                        Vec::new()
                    }
                };

                let imp = as_this.imp();
                if imp.mention_generation.get() != generation {
                    // The text changed while probing; that search owns the
                    // popup now.
                    return;
                }
                if users.is_empty() {
                    as_this.close_mention_popup();
                } else if let Some(popup) = imp.mention_popup.get() {
                    popup.set_candidates(&users);
                }
            }
        ));
    }

    /// Replaces the typed `@query` token with the candidate's display name,
    /// anchored as a MentionName entity (with link coloring) so it survives
    /// further edits and pings the user on send; the trailing space lets
    /// typing continue behind the mention.
    fn insert_mention(&self, user: &model::User) {
        let imp = self.imp();
        self.close_mention_popup();

        let name = strings::user_display_name(user, true);
        if name.trim().is_empty() {
            return;
        }
        let buffer = imp.text_view.buffer();
        let token_start = imp
            .mention_token_start
            .get()
            .clamp(0, buffer.cursor_position());
        let token_end = buffer.cursor_position();
        if token_start >= token_end {
            return;
        }

        buffer.begin_user_action();
        let mut start_iter = buffer.iter_at_offset(token_start);
        let mut end_iter = buffer.iter_at_offset(token_end);
        buffer.delete(&mut start_iter, &mut end_iter);
        buffer.insert(&mut start_iter, &format!("{name} "));
        let name_end = token_start + name.chars().count() as i32;
        let mark_start_iter = buffer.iter_at_offset(token_start);
        let mark_end_iter = buffer.iter_at_offset(name_end);
        // Same gravity as custom-emoji anchors: boundary edits land outside
        // the mention span.
        let start = buffer.create_mark(None, &mark_start_iter, false);
        let end = buffer.create_mark(None, &mark_end_iter, true);
        let tag = self.mention_tag();
        buffer.apply_tag(&tag, &mark_start_iter, &mark_end_iter);
        // Park the cursor after the trailing space, not at the mention start.
        let after = buffer.iter_at_offset(name_end + 1);
        buffer.place_cursor(&after);
        imp.mention_anchors.borrow_mut().push(imp::MentionAnchor {
            start,
            end,
            user_id: user.id(),
        });
        buffer.end_user_action();

        // The buffer-change signal fired before the anchor existed; publish
        // the entity now.
        self.text_buffer_changed();
        self.grab_focus();
    }

    /// Dump of the mention state for the Devel automation bridge.
    pub(crate) fn debug_mention_state(&self) -> String {
        let imp = self.imp();
        let popup = imp.mention_popup.get();
        let buffer = imp.text_view.buffer();
        let text: String = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .into();
        let mut out = format!(
            "popup_visible: {}\npopup_mapped: {}\nentry_mapped: {}\ncandidates: {}\nselected: {:?}\ntoken_start: {}\nbuffer: {text:?}\n",
            popup.is_some_and(|p| p.is_visible()),
            popup.map(|p| p.is_mapped()).unwrap_or_default(),
            imp.text_view.is_mapped(),
            popup.map(|p| p.candidate_count()).unwrap_or_default(),
            popup.and_then(|p| p.selected_index()),
            imp.mention_token_start.get(),
        );
        for anchor in imp.mention_anchors.borrow().iter() {
            let start = buffer.iter_at_mark(&anchor.start);
            let end = buffer.iter_at_mark(&anchor.end);
            let span: String = buffer.text(&start, &end, true).into();
            out.push_str(&format!(
                "anchor: user_id={} span={span:?}\n",
                anchor.user_id
            ));
        }
        if let Some(formatted) = imp.formatted_text.borrow().as_ref() {
            let entities: Vec<(i32, i32, &str)> = formatted
                .0
                .entities
                .iter()
                .map(|entity| {
                    let kind = match &entity.r#type {
                        tdlib::enums::TextEntityType::MentionName(_) => "mention-name",
                        tdlib::enums::TextEntityType::CustomEmoji(_) => "custom-emoji",
                        _ => "other",
                    };
                    (entity.offset, entity.length, kind)
                })
                .collect();
            out.push_str(&format!("entities: {entities:?}\n"));
        }
        out
    }

    /// Picks the highlighted candidate, exactly as Enter in the popup does.
    pub(crate) fn mention_activate_selected(&self) {
        if let Some(popup) = self.imp().mention_popup.get() {
            popup.activate_selected();
        }
    }

    /// The mention popup widget (for the automation bridge's screenshots).
    pub(crate) fn mention_popup(&self) -> Option<ui::MentionPopup> {
        self.imp().mention_popup.get().cloned()
    }

    pub(crate) async fn as_markdown(&self) -> Option<tdlib::types::FormattedText> {
        let text = self.imp().formatted_text.borrow().clone().map(|f| f.0)?;
        let client_id = self.chat().unwrap().session_().client_().id();

        tdlib::functions::parse_markdown(text.clone(), client_id)
            .await
            .map(|text| {
                let tdlib::enums::FormattedText::FormattedText(text) = text;
                text
            })
            .ok()
            .or(Some(text))
    }

    pub(crate) fn chat(&self) -> Option<model::Chat> {
        self.imp().chat.upgrade()
    }

    pub(crate) fn set_chat(&self, chat: Option<model::Chat>) {
        if self.chat() == chat {
            return;
        }

        self.imp().chat.set(chat.as_ref());
        self.notify("chat");
    }

    pub(crate) fn connect_formatted_text_notify<F: Fn(&Self, &glib::ParamSpec) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_notify_local(Some("formatted-text"), f)
    }

    pub(crate) fn placeholder_text(&self) -> Option<glib::GString> {
        self.imp().placeholder.text()
    }

    pub(crate) fn set_placeholder_text(&self, placeholder_text: Option<&str>) {
        self.imp().placeholder.set_text(placeholder_text);
    }

    pub(crate) fn connect_activate<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_local("activate", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);

            None
        })
    }

    pub(crate) fn connect_paste_clipboard<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("paste-clipboard", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);

            None
        })
    }

    pub(crate) fn connect_emoji_button_press<F: Fn(&Self, gtk::Image) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("emoji-button-press", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let button = values[1].get::<gtk::Image>().unwrap();
            f(&obj, button);

            None
        })
    }

    pub(crate) fn connect_emoji_button_hover<F: Fn(&Self, gtk::Image) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("emoji-button-hover", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            let button = values[1].get::<gtk::Image>().unwrap();
            f(&obj, button);
            None
        })
    }

    pub(crate) fn connect_emoji_button_leave<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("emoji-button-leave", true, move |values| {
            let obj = values[0].get::<Self>().unwrap();
            f(&obj);
            None
        })
    }
}

impl Default for MessageEntry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::char_index_of_utf16;
    use super::mention_prefix;

    #[test]
    fn bare_at_lists_default_candidates() {
        assert_eq!(mention_prefix("@"), Some((0, "")));
    }

    #[test]
    fn query_at_text_start() {
        assert_eq!(mention_prefix("@ivan"), Some((0, "ivan")));
    }

    #[test]
    fn query_after_space() {
        assert_eq!(mention_prefix("hi @iv"), Some((3, "iv")));
    }

    #[test]
    fn email_and_url_do_not_trigger() {
        assert_eq!(mention_prefix("mail@example"), None);
        assert_eq!(mention_prefix("https://t.me/chan@nel"), None);
    }

    #[test]
    fn second_at_in_token_is_not_a_query() {
        assert_eq!(mention_prefix("@a@b"), None);
    }

    #[test]
    fn whitespace_ends_the_query() {
        assert_eq!(mention_prefix("@ivan "), None);
        assert_eq!(mention_prefix("@ivan\n"), None);
    }

    #[test]
    fn multiline_last_token_wins() {
        assert_eq!(mention_prefix("@first\n@sec"), Some((7, "sec")));
    }

    #[test]
    fn empty_text_has_no_token() {
        assert_eq!(mention_prefix(""), None);
    }

    #[test]
    fn unicode_offsets_are_byte_accurate() {
        let text = "привет @Ив";
        let at = text.find('@').unwrap();
        assert_eq!(mention_prefix(text), Some((at, "Ив")));
    }

    #[test]
    fn long_queries_stop_triggering() {
        let too_long = format!("@{}", "a".repeat(65));
        assert_eq!(mention_prefix(&too_long), None);
        let at_limit = format!("@{}", "a".repeat(64));
        assert_eq!(mention_prefix(&at_limit), Some((0, &at_limit[1..])));
    }

    #[test]
    fn utf16_to_char_index() {
        assert_eq!(char_index_of_utf16("emoji 👍 test", 6), 6);
        // The thumbs-up spans two UTF-16 units, so unit 8 is the char after.
        assert_eq!(char_index_of_utf16("emoji 👍 test", 8), 7);
        assert_eq!(char_index_of_utf16("abc", 100), 3);
    }
}
