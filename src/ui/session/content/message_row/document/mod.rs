mod file_status;
mod status_indicator;

use std::cell::Cell;
use std::cell::RefCell;

use file_status::FileStatus;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

pub(crate) use self::status_indicator::StatusIndicator;
use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/document/mod.ui")]
    #[properties(wrapper_type = super::MessageDocument)]
    pub(crate) struct MessageDocument {
        pub(super) bindings: RefCell<Vec<gtk::ExpressionWatch>>,
        pub(super) handler_id: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) status_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        pub(super) target_generation: Cell<u64>,
        pub(super) thumbnail_generation: Cell<u64>,
        pub(super) thumbnail_demand: Cell<bool>,
        pub(super) thumbnail_file_id: Cell<Option<i32>>,
        pub(super) thumbnail_request_id: Cell<Option<i32>>,
        #[property(
            type = Option<glib::Object>,
            get = Self::message,
            set = Self::set_message,
            explicit_notify,
            nullable
        )]
        pub(super) message: glib::WeakRef<model::Message>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) file_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) click: TemplateChild<gtk::GestureClick>,
        #[template_child]
        pub(super) file_thumbnail_picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) status_indicator: TemplateChild<ui::MessageDocumentStatusIndicator>,
        #[template_child]
        pub(super) file_name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) file_size_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageDocument {
        const NAME: &'static str = "PaplMessageDocument";
        type Type = super::MessageDocument;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageDocument {
        fn constructed(&self) {
            self.parent_constructed();

            self.obj().connect_map(|obj| obj.start_thumbnail_work());
            self.obj().connect_unmap(|obj| obj.stop_thumbnail_work());
        }

        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }

        fn dispose(&self) {
            self.thumbnail_demand.set(false);
            self.target_generation
                .set(self.target_generation.get().wrapping_add(1));
            self.thumbnail_generation
                .set(self.thumbnail_generation.get().wrapping_add(1));

            if let Some(handler_id) = self.handler_id.take()
                && let Some(message) = self.message.upgrade()
            {
                message.disconnect(handler_id);
            }
            if let Some(handler_id) = self.status_handler_id.take() {
                self.click.disconnect(handler_id);
            }

            let bindings = std::mem::take(&mut *self.bindings.borrow_mut());
            for binding in bindings {
                binding.unwatch();
            }
        }
    }

    impl MessageDocument {
        fn message(&self) -> Option<glib::Object> {
            self.message.upgrade().map(|message| message.upcast())
        }

        fn set_message(&self, message: Option<glib::Object>) {
            let message = message.unwrap().downcast::<model::Message>().unwrap();
            ui::MessageBaseExt::set_message(&*self.obj(), &message);
        }
    }

    impl WidgetImpl for MessageDocument {}
    impl ui::MessageBaseImpl for MessageDocument {}
}

glib::wrapper! {
    pub(crate) struct MessageDocument(ObjectSubclass<imp::MessageDocument>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessageDocument {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        if imp.message.upgrade().as_ref() == Some(message) {
            return;
        }

        imp.target_generation
            .set(imp.target_generation.get().wrapping_add(1));
        imp.thumbnail_generation
            .set(imp.thumbnail_generation.get().wrapping_add(1));
        imp.thumbnail_file_id.set(None);
        imp.thumbnail_request_id.set(None);

        if let Some(handler_id) = imp.status_handler_id.take() {
            imp.click.disconnect(handler_id);
        }
        if let Some(handler_id) = imp.handler_id.take()
            && let Some(previous_message) = imp.message.upgrade()
        {
            previous_message.disconnect(handler_id);
        }

        let bindings = std::mem::take(&mut *imp.bindings.borrow_mut());
        for binding in bindings {
            binding.unwatch();
        }

        imp.message.set(Some(message));
        imp.message_bubble.update_from_message(message, false);

        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                if obj.imp().message.upgrade().as_ref() == Some(message) {
                    obj.update_document(message);
                }
            }
        ));
        imp.handler_id.replace(Some(handler_id));
        self.update_document(message);

        self.notify("message");
    }
}

impl MessageDocument {
    fn update_document(&self, message: &model::Message) {
        if let tdlib::enums::MessageContent::MessageDocument(data) = message.content().0 {
            let imp = self.imp();

            let message_text = utils::parse_formatted_text(data.caption);
            imp.message_bubble.set_label(message_text);

            imp.file_name_label.set_label(&data.document.file_name);

            let session = message.chat_().session_();

            self.try_load_thumbnail(message);
            self.update_status(data.document.document, session);
        }
    }

    fn update_status(
        &self,
        file: tdlib::types::File,
        session: model::ClientStateSession,
    ) -> FileStatus {
        let status = FileStatus::from(&file);

        let size = file.size.max(file.expected_size) as u64;

        self.update_size_label(status, size);
        self.update_button(file, session, status);

        status
    }

    fn update_button(
        &self,
        file: tdlib::types::File,
        session: model::ClientStateSession,
        status: FileStatus,
    ) {
        let imp = self.imp();
        let click = &*imp.click;
        let file_id = file.id;

        imp.status_indicator.set_status(status);
        let handler_id = match status {
            FileStatus::Downloading(_) | FileStatus::Uploading(_) => {
                return;
            }
            FileStatus::CanBeDownloaded => {
                // Download file
                let generation = imp.target_generation.get();
                click.connect_released(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    #[weak]
                    session,
                    move |click, _, _, _| {
                        // TODO: Fix bug mentioned here
                        // https://github.com/paper-plane-developers/paper-plane/pull/372#discussion_r968841370
                        session.download_file_with_updates(
                            file_id,
                            clone!(
                                #[weak]
                                obj,
                                #[weak]
                                session,
                                move |file| {
                                    if obj.imp().target_generation.get() == generation {
                                        obj.update_status(file, session);
                                    }
                                }
                            ),
                        );

                        let imp = obj.imp();

                        imp.status_indicator
                            .set_status(FileStatus::Downloading(0.0));
                        let handler_id = click.connect_released(clone!(
                            #[weak]
                            session,
                            move |_, _, _, _| {
                                session.cancel_download_file(file_id);
                            }
                        ));
                        if let Some(handler_id) = imp.status_handler_id.replace(Some(handler_id)) {
                            click.disconnect(handler_id);
                        }
                    }
                ))
            }
            FileStatus::Downloaded => {
                // Open file
                if imp.file_thumbnail_picture.file().is_some() {
                    imp.status_indicator.set_visible(false);
                }
                let gio_file = gio::File::for_path(&file.local.path);
                click.connect_released(move |_, _, _, _| {
                    if let Err(err) = gio::AppInfo::launch_default_for_uri(
                        &gio_file.uri(),
                        gio::AppLaunchContext::NONE,
                    ) {
                        log::error!("Error: {err}");
                    }
                })
            }
        };

        if let Some(handler_id) = imp.status_handler_id.replace(Some(handler_id)) {
            click.disconnect(handler_id);
        }
    }

    fn update_size_label(&self, status: FileStatus, size: u64) {
        let size_label = &self.imp().file_size_label;

        match status {
            FileStatus::Downloading(progress) | FileStatus::Uploading(progress) => {
                let downloaded = glib::format_size((size as f64 * progress) as u64);
                let full_size = glib::format_size(size);

                size_label.set_label(&format!("{downloaded} / {full_size}"));
            }
            FileStatus::CanBeDownloaded | FileStatus::Downloaded => {
                size_label.set_label(&glib::format_size(size));
            }
        }
    }

    fn try_load_thumbnail(&self, message: &model::Message) {
        if let tdlib::enums::MessageContent::MessageDocument(data) = message.content().0 {
            let imp = self.imp();
            if let Some(thumbnail) = data.document.thumbnail {
                let file_id = thumbnail.file.id;
                if imp.thumbnail_file_id.replace(Some(file_id)) != Some(file_id) {
                    imp.thumbnail_generation
                        .set(imp.thumbnail_generation.get().wrapping_add(1));
                    imp.thumbnail_request_id.set(None);
                    imp.file_thumbnail_picture
                        .set_paintable(gdk::Paintable::NONE);
                }

                imp.status_indicator.set_masked(false);
                imp.file_thumbnail_picture.set_visible(true);
                imp.file_box.add_css_class("with-thumbnail");
                if !imp.thumbnail_demand.get() || !self.is_mapped() {
                    return;
                }

                if thumbnail.file.local.is_downloading_completed {
                    imp.thumbnail_request_id.set(None);
                    imp.file_thumbnail_picture
                        .set_filename(Some(&thumbnail.file.local.path));
                } else {
                    if let Some(minithumbnail) = data.document.minithumbnail
                        && let Ok(minithumbnail) = gdk::Texture::from_bytes(
                            &glib::Bytes::from_owned(glib::base64_decode(&minithumbnail.data)),
                        )
                    {
                        imp.file_thumbnail_picture
                            .set_paintable(Some(&minithumbnail));
                    }

                    if imp.thumbnail_request_id.replace(Some(file_id)) == Some(file_id) {
                        return;
                    }

                    let generation = imp.thumbnail_generation.get().wrapping_add(1);
                    imp.thumbnail_generation.set(generation);
                    let target_generation = imp.target_generation.get();
                    let session = message.chat_().session_();
                    utils::spawn(clone!(
                        #[weak(rename_to = obj)]
                        self,
                        async move {
                            let result = session.download_file(file_id).await;
                            let imp = obj.imp();
                            if !imp.thumbnail_demand.get()
                                || !obj.is_mapped()
                                || imp.target_generation.get() != target_generation
                                || imp.thumbnail_generation.get() != generation
                                || imp.thumbnail_file_id.get() != Some(file_id)
                            {
                                return;
                            }

                            imp.thumbnail_request_id.set(None);
                            if let Ok(file) = result {
                                obj.imp()
                                    .file_thumbnail_picture
                                    .set_filename(Some(&file.local.path));
                            }
                        }
                    ));
                }
            } else {
                imp.thumbnail_generation
                    .set(imp.thumbnail_generation.get().wrapping_add(1));
                imp.thumbnail_file_id.set(None);
                imp.thumbnail_request_id.set(None);
                imp.status_indicator.set_masked(true);
                imp.file_thumbnail_picture.set_visible(false);
                imp.file_box.remove_css_class("with-thumbnail");
                imp.file_thumbnail_picture
                    .set_paintable(gdk::Paintable::NONE);
            }
        }
    }

    fn start_thumbnail_work(&self) {
        self.imp().thumbnail_demand.set(true);
        if let Some(message) = self.imp().message.upgrade() {
            self.try_load_thumbnail(&message);
        }
    }

    fn stop_thumbnail_work(&self) {
        let imp = self.imp();
        imp.thumbnail_demand.set(false);
        imp.thumbnail_generation
            .set(imp.thumbnail_generation.get().wrapping_add(1));
        imp.thumbnail_request_id.set(None);
    }
}
