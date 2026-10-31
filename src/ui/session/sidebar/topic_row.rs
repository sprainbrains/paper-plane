use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use glib::Properties;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::row::timestamp_text;
use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

/// Texture target for the topic icon emoji (26 CSS px at up to 2x scale).
const TOPIC_EMOJI_SIZE: i32 = 52;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::TopicRow)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/topic_row.ui")]
    pub(crate) struct TopicRow {
        /// The topic this row currently renders.
        pub(super) topic: RefCell<Option<model::Topic>>,
        /// The forum the bound topic belongs to (mute calls need its id).
        pub(super) chat: RefCell<Option<model::Chat>>,
        /// This row's context menu (mute/unmute).
        pub(super) menu: std::cell::OnceCell<gtk::PopoverMenu>,
        /// `changed` handler on the bound topic, disconnected on rebind.
        pub(super) changed_handler: RefCell<Option<(model::Topic, glib::SignalHandlerId)>>,
        /// Cleared on unmap: cancels in-flight emoji loads (AGENTS.md rule —
        /// per-row work is gated on map/unmap, never on bind alone).
        pub(super) demand: Rc<Cell<bool>>,
        /// Emoji id the visible texture (or failed load) belongs to.
        pub(super) emoji_loaded_for: Cell<i64>,
        /// The `topic-icon-color-…` class currently applied to the icon box.
        pub(super) color_class: RefCell<Option<String>>,
        #[template_child]
        pub(super) icon_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) icon_fallback: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) icon_emoji: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) closed_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) muted_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) pin_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) timestamp_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) preview_label: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) unread_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TopicRow {
        const NAME: &'static str = "PaplSidebarTopicRow";
        type Type = super::TopicRow;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.install_action("topic-row.mute", None, move |row, _, _| {
                row.toggle_mute(true);
            });
            klass.install_action("topic-row.unmute", None, move |row, _, _| {
                row.toggle_mute(false);
            });
            klass.install_action("topic-row.edit", None, move |row, _, _| {
                row.open_edit_dialog();
            });
            klass.install_action("topic-row.close", None, move |row, _, _| {
                row.set_closed(true);
            });
            klass.install_action("topic-row.reopen", None, move |row, _, _| {
                row.set_closed(false);
            });
            klass.install_action("topic-row.pin", None, move |row, _, _| {
                row.set_pinned(true);
            });
            klass.install_action("topic-row.unpin", None, move |row, _, _| {
                row.set_pinned(false);
            });
            klass.install_action("topic-row.delete", None, move |row, _, _| {
                row.confirm_delete();
            });
            klass.install_action("topic-row.hide-general", None, move |row, _, _| {
                row.hide_general();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TopicRow {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.connect_map(clone!(
                #[weak(rename_to = row)]
                obj,
                move |_| row.start_row_work()
            ));
            obj.connect_unmap(clone!(
                #[weak(rename_to = row)]
                obj,
                move |_| row.stop_row_work()
            ));

            let right_click = gtk::GestureClick::new();
            right_click.set_button(gdk::BUTTON_SECONDARY);
            right_click.connect_released(clone!(
                #[weak(rename_to = row)]
                obj,
                move |_, _, x, y| row.show_menu(x, y)
            ));
            obj.add_controller(right_click);
        }

        fn dispose(&self) {
            self.dispose_template();
            if let Some(menu) = self.menu.get() {
                menu.unparent();
            }
            if let Some((topic, id)) = self.changed_handler.take() {
                topic.disconnect(id);
            }
        }
    }

    impl WidgetImpl for TopicRow {}
}

glib::wrapper! {
    pub(crate) struct TopicRow(ObjectSubclass<imp::TopicRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl TopicRow {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Binds (or rebinds) the row to a topic of `chat`; re-rendering then
    /// follows the topic's `changed` signal.
    pub(crate) fn set_topic(&self, topic: Option<&model::Topic>, chat: Option<&model::Chat>) {
        let imp = self.imp();
        if let Some((watched, id)) = imp.changed_handler.take() {
            watched.disconnect(id);
        }
        imp.emoji_loaded_for.set(0);
        imp.topic.replace(topic.cloned());
        imp.chat.replace(chat.cloned());
        if let Some(topic) = topic {
            let weak = self.downgrade();
            let id = topic.connect_local("changed", false, move |_| {
                if let Some(row) = weak.upgrade() {
                    row.update_all();
                }
                None
            });
            imp.changed_handler.replace(Some((topic.clone(), id)));
        }
        self.update_all();
        // Only mapped rows decode; a bound-but-unmapped row starts on `map`.
        if self.is_mapped() {
            self.start_row_work();
        }
    }

    fn start_row_work(&self) {
        self.imp().demand.set(true);
        self.load_emoji();
    }

    fn stop_row_work(&self) {
        self.imp().demand.set(false);
    }

    fn update_all(&self) {
        let imp = self.imp();
        let Some(topic) = imp.topic.borrow().clone() else {
            return;
        };

        imp.name_label.set_text(&topic.name());
        imp.apply_icon_color(topic.icon_color());

        let emoji_id = topic.icon_custom_emoji_id();
        if emoji_id == 0 {
            imp.icon_fallback.set_visible(true);
            imp.icon_emoji.set_visible(false);
        } else if imp.emoji_loaded_for.get() == emoji_id {
            imp.icon_fallback.set_visible(false);
            imp.icon_emoji.set_visible(true);
        }

        let last_message = topic.last_message();
        if let Some(message) = last_message.as_ref() {
            imp.timestamp_label
                .set_label(&timestamp_text(message.date() as i64));
            imp.timestamp_label.set_visible(true);
            let sender = strings::message_sender(&message.sender(), true);
            imp.preview_label.set_text(Some(&format!(
                "{sender}: {}",
                strings::message_content(message)
            )));
            imp.preview_label.set_visible(true);
        } else {
            imp.timestamp_label.set_visible(false);
            imp.preview_label.set_visible(false);
        }

        imp.closed_icon.set_visible(topic.is_closed());
        imp.muted_icon.set_visible(topic.is_muted());
        imp.pin_icon.set_visible(topic.is_pinned());

        let unread = topic.unread_count();
        imp.unread_label.set_visible(unread > 0);
        if unread > 0 {
            imp.unread_label.set_label(&unread.to_string());
        }

        self.action_set_enabled("topic-row.mute", !topic.is_muted());
        self.action_set_enabled("topic-row.unmute", topic.is_muted());
        let can_manage = imp
            .chat
            .borrow()
            .as_ref()
            .is_some_and(|chat| crate::ui::session::forum::can_manage_topic(chat, &topic));
        self.action_set_enabled("topic-row.edit", can_manage);
        self.action_set_enabled("topic-row.close", can_manage && !topic.is_closed());
        self.action_set_enabled("topic-row.reopen", can_manage && topic.is_closed());
        self.action_set_enabled("topic-row.pin", can_manage && !topic.is_pinned());
        self.action_set_enabled("topic-row.unpin", can_manage && topic.is_pinned());
        self.action_set_enabled("topic-row.delete", can_manage);
        // Hiding General is a per-user view option, open to any member.
        self.action_set_enabled("topic-row.hide-general", topic.is_general());
    }

    /// Shared runner for the topic management calls: log failures as toasts.
    fn run_manage(
        &self,
        call: impl std::future::Future<Output = Result<(), tdlib::types::Error>> + 'static,
    ) {
        let widget = self.clone();
        utils::spawn(async move {
            if let Err(e) = call.await {
                utils::show_toast(&widget, e.message);
            }
        });
    }

    fn bound(&self) -> Option<(model::Topic, model::Chat)> {
        let imp = self.imp();
        Some((imp.topic.borrow().clone()?, imp.chat.borrow().clone()?))
    }

    fn set_closed(&self, is_closed: bool) {
        let Some((topic, chat)) = self.bound() else {
            return;
        };
        let client_id = chat.session_().client_().id();
        self.run_manage(tdlib::functions::toggle_forum_topic_is_closed(
            chat.id(),
            topic.forum_topic_id(),
            is_closed,
            client_id,
        ));
    }

    fn set_pinned(&self, is_pinned: bool) {
        let Some((topic, chat)) = self.bound() else {
            return;
        };
        let client_id = chat.session_().client_().id();
        self.run_manage(tdlib::functions::toggle_forum_topic_is_pinned(
            chat.id(),
            topic.forum_topic_id(),
            is_pinned,
            client_id,
        ));
    }

    fn hide_general(&self) {
        let Some((_, chat)) = self.bound() else {
            return;
        };
        let client_id = chat.session_().client_().id();
        self.run_manage(tdlib::functions::toggle_general_forum_topic_is_hidden(
            chat.id(),
            true,
            client_id,
        ));
    }

    /// Deletion is destructive and irreversible for everyone: confirm first.
    fn confirm_delete(&self) {
        let Some((topic, chat)) = self.bound() else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Delete Topic"))
            .body(gettext(
                "Are you sure you want to delete this topic for everyone?",
            ))
            .build();
        dialog.add_response("cancel", &gettext("_Cancel"));
        dialog.add_response("delete", &gettext("_Delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.choose(Some(self), gtk::gio::Cancellable::NONE, move |response| {
            if response != "delete" {
                return;
            }
            let client_id = chat.session_().client_().id();
            utils::spawn(async move {
                if let Err(e) = tdlib::functions::delete_forum_topic(
                    chat.id(),
                    topic.forum_topic_id(),
                    client_id,
                )
                .await
                {
                    log::warn!("Failed to delete a forum topic: {e:?}");
                }
            });
        });
    }

    /// Opens the rename/re-icon dialog for this topic.
    fn open_edit_dialog(&self) {
        let imp = self.imp();
        let (Some(topic), Some(chat)) = (imp.topic.borrow().clone(), imp.chat.borrow().clone())
        else {
            return;
        };
        let Some(window) = self.root().and_downcast::<gtk::Window>() else {
            return;
        };
        crate::ui::session::forum::show_topic_dialog(&window, &chat, Some(&topic));
    }

    fn show_menu(&self, x: f64, y: f64) {
        let imp = self.imp();
        let menu = imp.menu.get_or_init(|| {
            gtk::Builder::from_resource(
                "/app/drey/paper-plane/ui/session/sidebar/topic_row_menu.ui",
            )
            .object::<gtk::PopoverMenu>("menu")
            .unwrap()
        });
        menu.set_parent(self);
        menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 0, 0)));
        menu.popup();
    }

    /// Mutes or unmutes this topic (tdesktop's per-topic bell). The other
    /// notification fields are kept.
    fn toggle_mute(&self, mute: bool) {
        let imp = self.imp();
        let (Some(topic), Some(chat)) = (imp.topic.borrow().clone(), imp.chat.borrow().clone())
        else {
            return;
        };
        let Some(mut settings) = topic.notification_settings() else {
            return;
        };
        // Explicit value, never the chat default: TDLib does not propagate a
        // `use_default_mute_for` change to the server for topics.
        settings.use_default_mute_for = false;
        settings.mute_for = if mute {
            let now = glib::DateTime::now_utc().unwrap().to_unix() as i32;
            i32::MAX - now
        } else {
            0
        };
        let client_id = chat.session_().client_().id();
        utils::spawn(async move {
            if let Err(e) = tdlib::functions::set_forum_topic_notification_settings(
                chat.id(),
                topic.forum_topic_id(),
                settings,
                client_id,
            )
            .await
            {
                log::warn!("Failed to toggle a topic mute: {e:?}");
            }
        });
    }
}

impl imp::TopicRow {
    /// Pastel squircle background in the topic's TDLib color. TDLib restricts
    /// topic colors to a fixed six-value palette, so the styles ship as
    /// static classes and the row just swaps one.
    fn apply_icon_color(&self, color: i32) {
        let class = format!("topic-icon-color-{color:06X}");
        let current = self.color_class.replace(Some(class.clone()));
        if current.as_deref() != Some(class.as_str()) {
            if let Some(old) = current {
                self.icon_box.remove_css_class(&old);
            }
            self.icon_box.add_css_class(&class);
        }
    }
}

impl TopicRow {
    /// Decodes the topic's custom-emoji icon while the row is demanded
    /// (mapped); a recycled row re-uses the texture when the id is unchanged.
    fn load_emoji(&self) {
        let imp = self.imp();
        let Some(topic) = imp.topic.borrow().clone() else {
            return;
        };
        let emoji_id = topic.icon_custom_emoji_id();
        if emoji_id == 0 || imp.emoji_loaded_for.get() == emoji_id {
            return;
        }
        let Some(chat) = imp.chat.borrow().clone() else {
            return;
        };
        let session = chat.session_();
        let demand = imp.demand.clone();
        demand.set(self.is_mapped());
        if !demand.get() {
            return;
        }
        imp.emoji_loaded_for.set(emoji_id);

        utils::spawn(clone!(
            #[weak(rename_to = row)]
            self,
            async move {
                let client_id = session.client_().id();
                let Ok(tdlib::enums::Stickers::Stickers(data)) =
                    tdlib::functions::get_custom_emoji_stickers(vec![emoji_id], client_id).await
                else {
                    return;
                };
                let Some(sticker) = data.stickers.into_iter().next() else {
                    return;
                };
                let Ok(texture) =
                    ui::load_picker_preview(&sticker, TOPIC_EMOJI_SIZE, &session, demand.clone())
                        .await
                else {
                    return;
                };
                // The row may have been recycled to another topic meanwhile.
                let imp = row.imp();
                if !demand.get()
                    || imp.emoji_loaded_for.get() != emoji_id
                    || imp.topic.borrow().as_ref() != Some(&topic)
                {
                    return;
                }
                imp.icon_emoji.set_paintable(Some(&texture));
                imp.icon_emoji.set_visible(true);
                imp.icon_fallback.set_visible(false);
            }
        ));
    }
}
