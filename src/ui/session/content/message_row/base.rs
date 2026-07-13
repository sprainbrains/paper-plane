use std::cell::RefCell;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::ui;
use crate::utils;

#[derive(Debug, Default)]
pub(super) struct MessageHandler(RefCell<Option<glib::SignalHandlerId>>);

impl MessageHandler {
    pub(super) fn disconnect(&self, message: &glib::WeakRef<crate::model::Message>) {
        if let Some(handler) = self.0.take()
            && let Some(message) = message.upgrade()
        {
            message.disconnect(handler);
        }
    }

    pub(super) fn set(&self, handler: glib::SignalHandlerId) {
        self.0.replace(Some(handler));
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/base.ui")]
    pub(crate) struct MessageBase {}

    #[glib::object_subclass]
    impl ObjectSubclass for MessageBase {
        const NAME: &'static str = "PaplMessageBase";
        const ABSTRACT: bool = true;
        type Type = super::MessageBase;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl MessageBase {
        #[template_callback]
        fn on_long_pressed(&self, x: f64, y: f64) {
            // On touch, press-and-hold opens the message menu (which offers "Select" to
            // start multi-selection); selection dragging itself is mouse-only.
            self.show_message_menu(x as i32, y as i32);
        }

        fn show_message_menu(&self, x: i32, y: i32) {
            let obj = &*self.obj();
            let Some(chat_history) = utils::ancestor::<_, ui::ChatHistory>(obj) else {
                return;
            };
            let menu = chat_history.message_menu();

            // Recompute row visibility and the reactions bar for this message.
            let Some(row) = utils::ancestor::<_, ui::MessageRow>(obj) else {
                return;
            };
            menu.rebind(&row);

            menu.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 0, 0)));
            menu.unparent();
            menu.set_parent(obj);
            menu.popup();
        }

        /// Whether the point (in this widget's coordinates) is over selected bubble text —
        /// where a right click should reach the label's own text menu rather than ours.
        fn point_over_selected_text(&self, x: f64, y: f64) -> bool {
            let mut widget = self.obj().pick(x, y, gtk::PickFlags::DEFAULT);
            while let Some(w) = widget {
                if w.downcast_ref::<gtk::Label>().is_some_and(|label| {
                    label.is_selectable() && label.selection_bounds().is_some()
                }) {
                    return true;
                }
                widget = w.parent();
            }
            false
        }
    }

    impl ObjectImpl for MessageBase {
        fn constructed(&self) {
            self.parent_constructed();

            // Right click opens the message menu. Handled in the capture phase (before the
            // selectable text label's own menu) so plain text and empty bubble areas get
            // the message menu; only a click on already-selected text falls through to the
            // label's text menu.
            let click = gtk::GestureClick::new();
            click.set_button(3);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(clone!(
                #[weak(rename_to = imp)]
                self,
                move |gesture, _, x, y| {
                    if imp.point_over_selected_text(x, y) {
                        return;
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    imp.show_message_menu(x as i32, y as i32);
                }
            ));
            self.obj().add_controller(click);
        }

        fn dispose(&self) {
            let mut child = self.obj().first_child();
            while let Some(child_) = child {
                child = child_.next_sibling();
                child_.unparent();
            }
        }
    }

    impl WidgetImpl for MessageBase {}
}

glib::wrapper! {
    pub(crate) struct MessageBase(ObjectSubclass<imp::MessageBase>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

pub(crate) trait MessageBaseExt:
    glib::object::IsClass + IsA<glib::Object> + IsA<gtk::Widget> + IsA<MessageBase>
{
    type Message: IsA<glib::Object>;

    fn new(message: &Self::Message) -> Self {
        glib::Object::builder().property("message", message).build()
    }

    fn set_message(&self, message: &Self::Message);
}

pub(crate) trait MessageBaseImpl: WidgetImpl + ObjectImpl + 'static {}

unsafe impl<T: MessageBaseImpl> IsSubclassable<T> for MessageBase {
    fn class_init(class: &mut glib::Class<Self>) {
        Self::parent_class_init::<T>(class.upcast_ref_mut());
    }
}
