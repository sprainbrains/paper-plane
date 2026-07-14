use glib::Properties;
use glib::clone;
use glib::closure;
use glib::subclass::InitializingObject;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, Default, CompositeTemplate, Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/login/other_device.ui")]
    #[properties(wrapper_type = super::OtherDevice)]
    pub(crate) struct OtherDevice {
        #[property(get, set)]
        pub(super) model: glib::WeakRef<model::ClientStateAuthOtherDevice>,
        pub(super) manager_handler: RefCell<Option<(model::ClientManager, glib::SignalHandlerId)>>,
        #[template_child]
        pub(super) image: TemplateChild<gtk::Image>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OtherDevice {
        const NAME: &'static str = "PaplLoginOtherDevice";
        type Type = super::OtherDevice;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action(
                "login.other-device.use-phone-number",
                None,
                |widget, _, _| {
                    widget.use_phone_number();
                },
            );
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for OtherDevice {
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
            self.obj().setup_expressions();
        }

        fn dispose(&self) {
            self.disconnect_manager_handler();
            self.dispose_template();
        }
    }

    impl WidgetImpl for OtherDevice {
        fn unroot(&self) {
            self.parent_unroot();

            if let Some(model) = self.obj().model() {
                let client = model.auth_().client_();
                utils::spawn(async move {
                    _ = client.log_out().await;
                });
            }
        }
    }

    #[gtk::template_callbacks]
    impl OtherDevice {
        fn disconnect_manager_handler(&self) {
            if let Some((manager, handler)) = self.manager_handler.take() {
                manager.disconnect(handler);
            }
        }

        #[template_callback]
        fn on_notify_model(&self) {
            let obj = &*self.obj();

            self.disconnect_manager_handler();

            if let Some(model) = obj.model() {
                let client_manager = model.auth_().client_().client_manager_();

                obj.action_set_enabled("login.exit", !client_manager.sessions().is_empty());
                let handler = client_manager.connect_items_changed(clone!(
                    #[weak]
                    obj,
                    move |client_manager, _, _, _| {
                        obj.action_set_enabled("login.exit", !client_manager.sessions().is_empty());
                    }
                ));
                self.manager_handler
                    .replace(Some((client_manager, handler)));
            }
        }
    }
}

glib::wrapper! {
    pub(crate) struct OtherDevice(ObjectSubclass<imp::OtherDevice>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl From<&model::ClientStateAuthOtherDevice> for OtherDevice {
    fn from(model: &model::ClientStateAuthOtherDevice) -> Self {
        glib::Object::builder().property("model", model).build()
    }
}

impl OtherDevice {
    pub(crate) fn use_phone_number(&self) {
        if let Some(model) = self.model() {
            let client = model.auth_().client_();

            if let Some(view) = utils::ancestor::<_, ui::ClientManagerView>(self) {
                view.add_new_client(client.database_info().0.use_test_dc);
            }

            utils::spawn(async move {
                // We actually need to logout to stop tdlib sending us new links.
                // https://github.com/tdlib/td/issues/1645
                _ = client.log_out().await;
            });
        }
    }

    fn setup_expressions(&self) {
        Self::this_expression("model")
            .chain_property::<model::ClientStateAuthOtherDevice>("data")
            .chain_closure::<gdk::MemoryTexture>(closure!(
                |obj: Self, data: model::BoxedAuthorizationStateWaitOtherDeviceConfirmation| {
                    let size = obj.imp().image.pixel_size() as usize;
                    let bytes_per_pixel = 3;

                    let data_luma = qrcode_generator::to_image_from_str(
                        data.0.link,
                        qrcode_generator::QrCodeEcc::Low,
                        size,
                    )
                    .unwrap();

                    let bytes = glib::Bytes::from_owned(
                        // gdk::Texture only knows 3 byte color spaces, thus convert Luma.
                        data_luma
                            .into_iter()
                            .flat_map(|p| (0..bytes_per_pixel).map(move |_| p))
                            .collect::<Vec<_>>(),
                    );

                    gdk::MemoryTexture::new(
                        size as i32,
                        size as i32,
                        gdk::MemoryFormat::R8g8b8,
                        &bytes,
                        size * bytes_per_pixel,
                    )
                }
            ))
            .bind(&*self.imp().image, "paintable", Some(self));
    }
}
