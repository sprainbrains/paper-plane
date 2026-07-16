use std::cell::Cell;
use std::cell::RefCell;

use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::base::MessageHandler;
use crate::model;
use crate::types::MessageId;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/photo.ui")]
    #[properties(wrapper_type = super::MessagePhoto)]
    pub(crate) struct MessagePhoto {
        pub(super) binding: RefCell<Option<gtk::ExpressionWatch>>,
        pub(super) handler: MessageHandler,
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) generation: Cell<u64>,
        pub(super) demand: Cell<bool>,
        pub(super) loaded_file_id: Cell<Option<i32>>,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) picture: TemplateChild<ui::MediaPicture>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessagePhoto {
        const NAME: &'static str = "PaplMessagePhoto";
        type Type = super::MessagePhoto;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessagePhoto {
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

            self.obj().connect_scale_factor_notify(|obj| {
                if let Some(message) = obj.imp().message.upgrade() {
                    obj.update_photo(&message);
                }
            });

            // A single primary click opens the photo in the fullscreen viewer. The row's
            // capture-phase selection gesture runs first and only claims in selection
            // mode, so a normal click reaches this bubble-phase gesture; the `n_press`
            // guard leaves double-click free for the row's reply gesture.
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(clone!(
                #[weak(rename_to = obj)]
                self,
                move |gesture, n_press, _, _| {
                    if n_press != 1 {
                        return;
                    }
                    let Some(message) = obj.message.upgrade() else {
                        return;
                    };
                    if let Some(window) = utils::ancestor::<_, ui::Window>(&*obj.obj()) {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        window.open_media_viewer(&message);
                    }
                }
            ));
            self.picture.add_controller(gesture);
        }
    }

    impl MessagePhoto {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessagePhoto {
        fn map(&self) {
            self.parent_map();
            self.demand.set(true);
            if let Some(message) = self.message.upgrade() {
                self.obj().update_photo(&message);
            }
        }

        fn unmap(&self) {
            self.demand.set(false);
            self.generation.set(self.generation.get().wrapping_add(1));
            self.parent_unmap();
        }
    }
    impl ui::MessageBaseImpl for MessagePhoto {}
}

glib::wrapper! {
    pub(crate) struct MessagePhoto(ObjectSubclass<imp::MessagePhoto>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessagePhoto {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        let old_message = imp.message.upgrade();
        if old_message.as_ref() == Some(message) {
            return;
        }

        if let Some(binding) = imp.binding.take() {
            binding.unwatch();
        }

        imp.handler.disconnect();

        imp.message.set(Some(message));

        imp.message_bubble.update_from_message(message, true);

        // Setup caption expression
        let caption_binding = model::Message::this_expression("content")
            .chain_closure::<String>(closure!(
                |_: model::Message, content: model::BoxedMessageContent| {
                    // Content can change in place (e.g. a TTL photo becoming
                    // `MessageExpiredPhoto`), so tolerate a non-photo variant.
                    if let tdlib::enums::MessageContent::MessagePhoto(data) = content.0 {
                        utils::parse_formatted_text(data.caption)
                    } else {
                        String::new()
                    }
                }
            ))
            .bind(&*imp.message_bubble, "label", Some(message));
        imp.binding.replace(Some(caption_binding));

        // Load photo
        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                obj.update_photo(message);
            }
        ));
        imp.handler.set(message, handler_id);
        self.update_photo(message);

        self.notify("message");
    }
}

impl MessagePhoto {
    fn message_id(&self) -> Option<MessageId> {
        self.imp().message.upgrade().map(|message| message.id())
    }

    fn update_photo(&self, message: &model::Message) {
        if let tdlib::enums::MessageContent::MessagePhoto(mut data) = message.content().0 {
            let imp = self.imp();
            imp.generation.set(imp.generation.get().wrapping_add(1));
            let generation = imp.generation.get();
            // Choose the right photo size based on the screen scale factor.
            // See https://core.telegram.org/api/files#image-thumbnail-types for more
            // information about photo sizes.
            let photo_size = if self.scale_factor() > 2 {
                data.photo.sizes.pop()
            } else {
                let type_ = if self.scale_factor() > 1 { "y" } else { "x" };

                match data.photo.sizes.iter().position(|s| s.r#type == type_) {
                    Some(pos) => Some(data.photo.sizes.swap_remove(pos)),
                    None => data.photo.sizes.pop(),
                }
            };
            let Some(photo_size) = photo_size else {
                imp.picture.set_aspect_ratio(1.0);
                log::warn!("Photo message {} has no available sizes", message.id());
                return;
            };

            if photo_size.width <= 0 || photo_size.height <= 0 {
                imp.picture.set_aspect_ratio(1.0);
                log::warn!(
                    "Photo message {} has invalid dimensions {}x{}",
                    message.id(),
                    photo_size.width,
                    photo_size.height
                );
                return;
            }

            imp.picture
                .set_aspect_ratio(photo_size.width as f64 / photo_size.height as f64);

            if imp.loaded_file_id.get() == Some(photo_size.photo.id)
                && imp.picture.paintable().is_some()
            {
                return;
            }
            imp.loaded_file_id.set(None);
            imp.picture.set_paintable(gdk::Paintable::NONE);

            if photo_size.photo.local.is_downloading_completed {
                if imp.demand.get() {
                    self.load_photo(photo_size.photo.local.path, photo_size.photo.id, generation);
                }
            } else {
                if imp.demand.get() {
                    imp.picture.set_paintable(
                        data.photo
                            .minithumbnail
                            .and_then(|m| {
                                gdk::Texture::from_bytes(&glib::Bytes::from_owned(
                                    glib::base64_decode(&m.data),
                                ))
                                .ok()
                            })
                            .as_ref(),
                    );
                }

                if !imp.demand.get() {
                    return;
                }
                let file_id = photo_size.photo.id;
                let session = message.chat_().session_();
                utils::spawn(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    #[weak]
                    session,
                    async move {
                        obj.download_photo(file_id, generation, &session).await;
                    }
                ));
            }
        }
    }

    async fn download_photo(
        &self,
        file_id: i32,
        generation: u64,
        session: &model::ClientStateSession,
    ) {
        match session.download_file(file_id).await {
            Ok(file) => {
                self.load_photo(file.local.path, file_id, generation);
            }
            Err(e) => {
                log::warn!("Failed to download a photo: {e:?}");
            }
        }
    }

    fn load_photo(&self, path: String, file_id: i32, generation: u64) {
        if let Some(message_id) = self.message_id() {
            utils::spawn(clone!(
                #[weak(rename_to = obj)]
                self,
                async move {
                    let result =
                        match gio::spawn_blocking(move || utils::decode_image_from_path(&path))
                            .await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                log::warn!("Photo decoder task failed: {error:?}");
                                return;
                            }
                        };

                    // Check if the current message id is the same as the one at
                    // the time of the request. It may be changed because of the
                    // ListView recycling while decoding the image. It may also
                    // that the message has been already removed from the history
                    // and the WeakRef is None (after successful sent)
                    let imp = obj.imp();
                    if imp.demand.get()
                        && obj.is_mapped()
                        && imp.generation.get() == generation
                        && obj.message_id().filter(|id| *id == message_id).is_some()
                        && obj.has_photo_file_id(file_id)
                    {
                        match result {
                            Ok(texture) => {
                                obj.imp().picture.set_paintable(Some(&texture));
                                obj.imp().loaded_file_id.set(Some(file_id));
                            }
                            Err(e) => {
                                log::warn!("Error decoding a photo: {e:?}");
                            }
                        }
                    }
                }
            ));
        }
    }

    fn has_photo_file_id(&self, file_id: i32) -> bool {
        let Some(message) = self.imp().message.upgrade() else {
            return false;
        };
        let tdlib::enums::MessageContent::MessagePhoto(data) = message.content().0 else {
            return false;
        };
        data.photo
            .sizes
            .into_iter()
            .any(|size| size.photo.id == file_id)
    }
}
