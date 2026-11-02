use std::sync::OnceLock;

use adw::subclass::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

use crate::model;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/event_row.ui")]
    pub(crate) struct EventRow {
        #[template_child]
        pub(super) label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) thumbnail: TemplateChild<gtk::Picture>,
        /// Whose profile a tapped sender-name link opens (0 = no link).
        pub(super) link_user_id: std::cell::Cell<i64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EventRow {
        const NAME: &'static str = "PaplEventRow";
        type Type = super::EventRow;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for EventRow {
        fn constructed(&self) {
            self.parent_constructed();
            let weak = self.obj().downgrade();
            self.label.connect_activate_link(move |_, _| {
                let Some(row) = weak.upgrade() else {
                    return glib::Propagation::Stop;
                };
                let id = row.imp().link_user_id.get();
                if id != 0
                    && let Some(session) = crate::utils::ancestor::<_, crate::ui::Session>(&row)
                {
                    session.show_chat_info(id);
                }
                glib::Propagation::Stop
            });
        }

        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: OnceLock<Vec<glib::ParamSpec>> = OnceLock::new();
            PROPERTIES.get_or_init(|| vec![glib::ParamSpecString::builder("label").build()])
        }

        fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            let obj = self.obj();

            match pspec.name() {
                "label" => obj.set_label(value.get().unwrap()),
                _ => unimplemented!(),
            }
        }

        fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            let obj = self.obj();

            match pspec.name() {
                "label" => obj.label().to_value(),
                _ => unimplemented!(),
            }
        }
    }

    impl WidgetImpl for EventRow {}
    impl BinImpl for EventRow {}
}

glib::wrapper! {
    pub(crate) struct EventRow(ObjectSubclass<imp::EventRow>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for EventRow {
    fn default() -> Self {
        Self::new()
    }
}

impl EventRow {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn label(&self) -> String {
        self.imp().label.text().to_string()
    }

    pub(crate) fn set_label(&self, label: &str) {
        self.imp().link_user_id.set(0);
        self.imp().label.set_label(label);
    }

    /// Renders `text` with the sender's display name as a link that opens
    /// their profile (tdesktop). `name` must be the exact text `text` uses.
    pub(crate) fn set_label_with_link(&self, text: &str, name: &str, user: &model::User) {
        let escaped = glib::markup_escape_text(text).to_string();
        let name_escaped = glib::markup_escape_text(name);
        let link = format!("<a href=\"user\">{name_escaped}</a>");
        let markup = escaped.replacen(name_escaped.as_str(), &link, 1);
        self.imp().link_user_id.set(user.id());
        self.imp().label.set_markup(&markup);
    }

    /// Shows a small preview built from an inline minithumbnail, e.g. the new
    /// chat photo of a "changed the group photo" service message. Pass `None` to
    /// hide the preview.
    pub(crate) fn set_minithumbnail(&self, minithumbnail: Option<&tdlib::types::Minithumbnail>) {
        let thumbnail = &self.imp().thumbnail;
        let texture = minithumbnail.and_then(|m| {
            gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(&m.data))).ok()
        });
        thumbnail.set_paintable(texture.as_ref());
        thumbnail.set_visible(texture.is_some());
    }
}
