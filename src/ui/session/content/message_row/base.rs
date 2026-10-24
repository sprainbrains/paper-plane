use std::cell::RefCell;

use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::ui;
use crate::utils;

#[derive(Debug, Default)]
pub(super) struct MessageHandler(RefCell<Option<(crate::model::Message, glib::SignalHandlerId)>>);

impl MessageHandler {
    pub(super) fn disconnect(&self) {
        if let Some((message, handler)) = self.0.take() {
            message.disconnect(handler);
        }
    }

    pub(super) fn set(&self, message: &crate::model::Message, handler: glib::SignalHandlerId) {
        self.disconnect();
        self.0.replace(Some((message.clone(), handler)));
    }
}

impl Drop for MessageHandler {
    fn drop(&mut self) {
        if let Some((message, handler)) = self.0.get_mut().take() {
            message.disconnect(handler);
        }
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

            // Point at the whole bubble rather than the click point so GTK places the menu
            // beside/above the message instead of on top of it. Fall back to the click point
            // if the bubble's bounds can't be resolved in this widget's coordinates.
            //
            // A media album is often taller than the popover itself. GTK can then find no
            // side to open on (not enough room above or below the tall pointing rect) and
            // silently fails to show the menu at all. So cap the pointing rect to a modest
            // height and slide it to stay centred on the click: short messages keep the
            // whole-bubble behaviour unchanged, tall albums get the menu right at the cursor.
            const MAX_POINTING_HEIGHT: f32 = 200.0;
            let pointing = obj
                .compute_bounds(obj)
                .map(|bounds| {
                    let height = bounds.height().min(MAX_POINTING_HEIGHT);
                    let lowest_top = (bounds.y() + bounds.height() - height).max(bounds.y());
                    let top = (y as f32 - height / 2.0).clamp(bounds.y(), lowest_top);
                    gdk::Rectangle::new(
                        bounds.x() as i32,
                        top as i32,
                        bounds.width() as i32,
                        height as i32,
                    )
                })
                .unwrap_or_else(|| gdk::Rectangle::new(x, y, 0, 0));
            menu.set_pointing_to(Some(&pointing));
            menu.unparent();
            menu.set_parent(obj);

            chat_history.highlight_message_for_menu(&row);
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
                    // In multi-select mode the right-click offers the selection actions
                    // (Forward/Copy/Delete/Clear); otherwise the full message menu.
                    let obj = imp.obj();
                    let selecting = utils::ancestor::<_, ui::ChatHistory>(&*obj)
                        .is_some_and(|history| history.is_selection_mode());
                    if selecting {
                        if let Some(row) = utils::ancestor::<_, ui::MessageRow>(&*obj) {
                            let point = obj
                                .compute_point(&row, &graphene::Point::new(x as f32, y as f32))
                                .unwrap_or_else(|| graphene::Point::new(x as f32, y as f32));
                            row.show_selection_menu(point.x() as f64, point.y() as f64);
                        }
                    } else {
                        imp.show_message_menu(x as i32, y as i32);
                    }
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
