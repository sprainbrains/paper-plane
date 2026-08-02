use std::cell::Cell;
use std::cell::RefCell;

use glib::Properties;
use glib::clone;
use glib::closure;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::gsk;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::model;

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, CompositeTemplate)]
    #[properties(wrapper_type = super::Icon)]
    #[template(resource = "/app/drey/paper-plane/ui/session/sidebar/chat_folder/icon.ui")]
    pub(crate) struct Icon {
        pub(super) unread_label_width: Cell<i32>,
        pub(super) unread_label_height: Cell<i32>,
        /// The model + its `unread-chat-count` notify handler, live only while the icon is
        /// mapped so it can never fire `set_label` on a detached (being-torn-down) widget.
        pub(super) unread_handler: RefCell<Option<(model::ChatList, glib::SignalHandlerId)>>,
        #[property(get, set)]
        pub(super) chat_list: glib::WeakRef<model::ChatList>,
        #[template_child]
        pub(super) icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub(super) unread_mask_bin: TemplateChild<adw::Bin>,
        #[template_child]
        pub(super) unread_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Icon {
        const NAME: &'static str = "PaplSidebarChatFolderIcon";
        type Type = super::Icon;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.set_css_name("chatfoldericon");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Icon {
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

            let obj = &*self.obj();

            let chat_list_expr = Self::Type::this_expression("chat-list");

            gtk::ClosureExpression::new::<String>(
                [
                    &chat_list_expr.chain_property::<model::ChatList>("list-type"),
                    &chat_list_expr.chain_property::<model::ChatList>("icon"),
                ],
                closure!(
                    |_: Self::Type, list_type: model::BoxedChatListType, icon: &str| {
                        use tdlib::enums::ChatList;

                        match list_type.0 {
                            ChatList::Main => "all-chats-symbolic",
                            ChatList::Archive => "",
                            _ => super::super::folder_icon_symbolic(icon),
                        }
                    }
                ),
            )
            .bind(&self.icon.get(), "icon-name", Some(obj));

            // Drive the unread badge through a manual notify handler that is connected
            // only while the icon is mapped (see `refresh_unread_binding`). A
            // `GtkExpression` bind here keeps firing `set_label` — and thus
            // `queue_resize` up the ancestor chain — during folder-row teardown, after an
            // ancestor has been freed, which is a use-after-free crash.
            obj.connect_map(|obj| obj.imp().refresh_unread_binding());
            obj.connect_unmap(|obj| obj.imp().refresh_unread_binding());
            obj.connect_chat_list_notify(|obj| obj.imp().refresh_unread_binding());
        }

        fn dispose(&self) {
            if let Some((model, handler)) = self.unread_handler.borrow_mut().take() {
                model.disconnect(handler);
            }
            self.dispose_template();
        }
    }

    impl Icon {
        /// Reconnects the `unread-chat-count` notify handler to the current chat list,
        /// but only while the icon is mapped. Called on map, unmap, and whenever the
        /// `chat-list` changes, so the handler exists exactly when the icon is in a live
        /// widget tree — never during teardown.
        pub(super) fn refresh_unread_binding(&self) {
            if let Some((model, handler)) = self.unread_handler.borrow_mut().take() {
                model.disconnect(handler);
            }

            let obj = self.obj();
            if !obj.is_mapped() {
                return;
            }
            self.update_unread();

            if let Some(chat_list) = obj.chat_list() {
                let handler = chat_list.connect_unread_chat_count_notify(clone!(
                    #[weak]
                    obj,
                    move |_| obj.imp().update_unread()
                ));
                *self.unread_handler.borrow_mut() = Some((chat_list, handler));
            }
        }

        fn update_unread(&self) {
            let count = self
                .obj()
                .chat_list()
                .map_or(0, |list| list.unread_chat_count());
            self.unread_label.set_label(&count.to_string());
            self.unread_label.set_visible(count > 0);
            // The badge mask/label in `snapshot` reads the count live.
            self.obj().queue_draw();
        }
    }

    impl WidgetImpl for Icon {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (min1, nat1, min_baseline1, nat_baseline1) =
                self.icon.measure(orientation, for_size);
            let (min2, nat2, min_baseline2, nat_baseline2) =
                self.unread_mask_bin.measure(orientation, for_size);
            let (min3, nat3, min_baseline3, nat_baseline3) =
                self.unread_label.measure(orientation, for_size);

            match orientation {
                gtk::Orientation::Horizontal => self.unread_label_width.set(min3),
                _ => self.unread_label_height.set(min3),
            }

            (
                min1.max(min2).max(min3),
                nat1.max(nat2).max(nat3),
                min_baseline1.max(min_baseline2).max(min_baseline3),
                nat_baseline1.max(nat_baseline2).max(nat_baseline3),
            )
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let unread_label_width = self.unread_label_width.get();
            let unread_label_height = self.unread_label_height.get();

            self.icon
                .size_allocate(&gdk::Rectangle::new(0, 0, width, height), baseline);

            self.unread_mask_bin.size_allocate(
                &gdk::Rectangle::new(
                    match self.obj().direction() {
                        gtk::TextDirection::Ltr => width - unread_label_width - 2,
                        _ => 0,
                    },
                    0,
                    unread_label_width + 2,
                    unread_label_height + 2,
                ),
                baseline,
            );

            self.unread_label
                .size_allocate(&gdk::Rectangle::new(0, 0, width, height), baseline);
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = &*self.obj();

            if obj
                .chat_list()
                .filter(|chat_list| chat_list.unread_chat_count() > 0)
                .is_none()
            {
                obj.snapshot_child(&*self.icon, snapshot);
                return;
            }

            snapshot.push_mask(gsk::MaskMode::InvertedAlpha);

            obj.snapshot_child(&*self.unread_mask_bin, snapshot);
            snapshot.pop();

            obj.snapshot_child(&*self.icon, snapshot);
            snapshot.pop();

            obj.snapshot_child(&*self.unread_label, snapshot);
        }
    }
}

glib::wrapper! {
    pub(crate) struct Icon(ObjectSubclass<imp::Icon>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
