use std::cell::OnceCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;

use crate::expressions;
use crate::i18n::ngettext_f;
use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/chat_info_window.ui")]
    pub(crate) struct ChatInfoWindow {
        pub(super) chat: OnceCell<model::Chat>,
        #[template_child]
        pub(super) toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub(super) name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) subtitle_label: TemplateChild<gtk::Inscription>,
        #[template_child]
        pub(super) info_list: TemplateChild<gtk::ListBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ChatInfoWindow {
        const NAME: &'static str = "PaplChatInfoWindow";
        type Type = super::ChatInfoWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ChatInfoWindow {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| {
                vec![
                    glib::ParamSpecObject::builder::<model::Chat>("chat")
                        .construct_only()
                        .build(),
                ]
            })
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            match pspec.name() {
                "chat" => self.chat.set(value.get().unwrap()).unwrap(),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "chat" => obj.chat().to_value(),
                _ => unimplemented!(),
            }
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup_window();
        }
    }

    impl WidgetImpl for ChatInfoWindow {}
    impl WindowImpl for ChatInfoWindow {}
    impl AdwWindowImpl for ChatInfoWindow {}

    #[gtk::template_callbacks]
    impl ChatInfoWindow {
        #[template_callback]
        fn on_key_pressed(
            &self,
            key: gdk::Key,
            _: u32,
            modifier: gdk::ModifierType,
            _: &gtk::EventControllerKey,
        ) -> glib::Propagation {
            if key == gdk::Key::Escape
                || (key == gdk::Key::w && modifier == gdk::ModifierType::CONTROL_MASK)
            {
                self.obj().close();
            }

            glib::Propagation::Proceed
        }
    }
}

glib::wrapper! {
    pub(crate) struct ChatInfoWindow(ObjectSubclass<imp::ChatInfoWindow>)
        @extends gtk::Widget, gtk::Window, adw::Window,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl ChatInfoWindow {
    pub(crate) fn new(parent_window: &Option<gtk::Window>, chat: &model::Chat) -> Self {
        glib::Object::builder()
            .property("transient-for", parent_window)
            .property("chat", chat)
            .build()
    }

    fn setup_window(&self) {
        let imp = self.imp();
        let chat_expression = Self::this_expression("chat");

        // Bind the name
        expressions::chat_display_name(&chat_expression).bind(
            &*imp.name_label,
            "label",
            Some(self),
        );

        match self.chat().unwrap().chat_type() {
            model::ChatType::Private(user) => {
                self.setup_user_info(&user);
            }
            model::ChatType::BasicGroup(basic_group) => {
                self.setup_basic_group_info(&basic_group);
            }
            model::ChatType::Supergroup(supergroup) => {
                self.setup_supergroup_info(&supergroup);
            }
            model::ChatType::Secret(secret) => {
                self.setup_user_info(&secret.user_());
            }
        }
    }

    /// Adds an activatable "Chat with user" / "Open chat" row with a trailing
    /// chevron (the About-window menu style): closes this window and switches
    /// to the chat, remembering the current one for the header back arrow.
    fn add_open_chat_row(&self) {
        let Some(chat) = self.chat() else {
            return;
        };
        let chat_id = chat.id();
        let title = match chat.chat_type() {
            model::ChatType::Private(_) | model::ChatType::Secret(_) => gettext("Chat with user"),
            _ => gettext("Open chat"),
        };
        let row = adw::ActionRow::builder()
            .title(&title)
            .activatable(true)
            .build();
        row.add_css_class("property");
        row.add_suffix(&gtk::Image::from_icon_name("pan-end-symbolic"));
        row.connect_activated(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| {
                if let Some(parent) = obj.transient_for()
                    && let Some(session) = utils::descendant::<_, ui::Session>(&parent)
                {
                    session.select_chat_with_return(chat_id);
                }
                obj.close();
            }
        ));
        self.imp().info_list.append(&row);
    }

    fn setup_user_info(&self, user: &model::User) {
        let imp = self.imp();

        // Online status or bot label
        if let tdlib::enums::UserType::Bot(_) = user.user_type().0 {
            imp.subtitle_label.set_text(Some(&gettext("bot")));
        } else {
            model::User::this_expression("status")
                .chain_closure::<String>(closure!(
                    |_: glib::Object, status: model::BoxedUserStatus| {
                        strings::user_status(&status.0)
                    }
                ))
                .bind(&*imp.subtitle_label, "text", Some(user));
        }

        // Phone number
        if !user.phone_number().is_empty() {
            let row = new_property_row(&gettext("Mobile"), &format!("+{}", user.phone_number()));
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        // Username
        if !user.username().is_empty() {
            let row = new_property_row(&gettext("Username"), &format!("@{}", user.username()));
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.add_open_chat_row();
        self.update_info_list_visibility();
    }

    fn setup_basic_group_info(&self, basic_group: &model::BasicGroup) {
        let client_id = self.chat().unwrap().session_().client_().id();
        let basic_group_id = basic_group.id();
        let imp = self.imp();

        // Members number
        model::BasicGroup::this_expression("member-count")
            .chain_closure::<String>(closure!(|_: glib::Object, member_count: i32| {
                ngettext_f(
                    "{num} member",
                    "{num} members",
                    member_count as u32,
                    &[("num", &member_count.to_string())],
                )
            }))
            .bind(&*imp.subtitle_label, "text", Some(basic_group));

        self.add_open_chat_row();
        self.update_info_list_visibility();

        // Full info
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result =
                    tdlib::functions::get_basic_group_full_info(basic_group_id, client_id).await;
                match result {
                    Ok(tdlib::enums::BasicGroupFullInfo::BasicGroupFullInfo(full_info)) => {
                        obj.setup_basic_group_full_info(full_info);
                    }
                    Err(e) => {
                        log::warn!("Failed to get basic group full info: {e:?}");
                    }
                }
            }
        ));
    }

    fn setup_basic_group_full_info(&self, basic_group_full_info: tdlib::types::BasicGroupFullInfo) {
        let imp = self.imp();

        // Description
        if !basic_group_full_info.description.is_empty() {
            let row = new_property_row(&gettext("Description"), &basic_group_full_info.description);
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.update_info_list_visibility();
    }

    fn setup_supergroup_info(&self, supergroup: &model::Supergroup) {
        let client_id = self.chat().unwrap().session_().client_().id();
        let supergroup_id = supergroup.id();
        let imp = self.imp();

        // Members number
        model::Supergroup::this_expression("member-count")
            .chain_closure::<String>(closure!(|_: glib::Object, member_count: i32| {
                ngettext_f(
                    "{num} member",
                    "{num} members",
                    member_count as u32,
                    &[("num", &member_count.to_string())],
                )
            }))
            .bind(&*imp.subtitle_label, "text", Some(supergroup));

        // Link
        if !supergroup.username().is_empty() {
            let row = new_property_row(
                &gettext("Link"),
                &format!("https://t.me/{}", supergroup.username()),
            );
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.add_open_chat_row();
        self.update_info_list_visibility();

        // Full info
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            async move {
                let result =
                    tdlib::functions::get_supergroup_full_info(supergroup_id, client_id).await;
                match result {
                    Ok(tdlib::enums::SupergroupFullInfo::SupergroupFullInfo(full_info)) => {
                        obj.setup_supergroup_full_info(full_info);
                    }
                    Err(e) => {
                        log::warn!("Failed to get supergroup full info: {e:?}");
                    }
                }
            }
        ));
    }

    fn setup_supergroup_full_info(&self, supergroup_full_info: tdlib::types::SupergroupFullInfo) {
        let imp = self.imp();

        // Description
        if !supergroup_full_info.description.is_empty() {
            let row = new_property_row(&gettext("Description"), &supergroup_full_info.description);
            self.make_row_copyable(&row);
            imp.info_list.append(&row);
        }

        self.update_info_list_visibility();
    }

    fn update_info_list_visibility(&self) {
        let info_list = &self.imp().info_list;
        info_list.set_visible(info_list.first_child().is_some());
    }

    fn make_row_copyable(&self, action_row: &adw::ActionRow) {
        action_row.set_activatable(true);
        action_row.connect_activated(clone!(
            #[weak(rename_to = obj)]
            self,
            move |action_row| {
                // The value lives in the subtitle; the title is the field label.
                action_row
                    .clipboard()
                    .set_text(&action_row.subtitle().unwrap_or_default());

                let toast = adw::Toast::new(&gettext("Copied to clipboard"));
                obj.imp().toast_overlay.add_toast(toast);
            }
        ));
    }

    pub(crate) fn chat(&self) -> Option<&model::Chat> {
        self.imp().chat.get()
    }

    /// Snapshot of the info window's rows (title + whether it carries the
    /// trailing chevron) for the Devel automation bridge, so the "Chat with
    /// user"/"Open chat" row can be verified headlessly.
    pub(crate) fn debug_state(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let _ = writeln!(out, "chat: {:?}", self.chat().map(|c| (c.id(), c.title())));
        let mut child = self.imp().info_list.first_child();
        while let Some(widget) = child {
            if let Some(row) = widget.downcast_ref::<adw::ActionRow>() {
                let has_chevron = has_descendant_icon(row, "pan-end-symbolic");
                let _ = writeln!(
                    out,
                    "row title={:?} activatable={} chevron={}",
                    row.title(),
                    row.is_activatable(),
                    has_chevron
                );
            }
            child = widget.next_sibling();
        }
        out
    }
}

fn new_property_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();
    row.add_css_class("property");
    row
}

/// Whether `root`'s widget tree contains an image with this icon name. Used to
/// confirm the "Chat with user" row rendered its trailing chevron.
fn has_descendant_icon(root: &impl IsA<gtk::Widget>, icon_name: &str) -> bool {
    let mut stack = vec![root.as_ref().clone()];
    while let Some(widget) = stack.pop() {
        if let Some(image) = widget.downcast_ref::<gtk::Image>()
            && image.icon_name().as_deref() == Some(icon_name)
        {
            return true;
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            stack.push(c.clone());
            child = c.next_sibling();
        }
    }
    false
}
