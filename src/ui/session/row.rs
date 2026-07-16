use std::cell::OnceCell;

use adw::prelude::*;
use gettextrs::gettext;
use glib::Properties;
use glib::clone;
use glib::closure;
use glib::subclass::InitializingObject;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::expressions;
use crate::i18n::gettext_f;
use crate::model;
use crate::strings;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, Properties)]
    #[properties(wrapper_type = super::Row)]
    #[template(resource = "/app/drey/paper-plane/ui/session/row.ui")]
    pub(crate) struct Row {
        #[property(get, set)]
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        pub(super) menu: OnceCell<gtk::PopoverMenu>,
        #[template_child]
        pub(super) display_name_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) username_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) unread_count_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) spinner: TemplateChild<gtk::Spinner>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Row {
        const NAME: &'static str = "PaplSessionRow";
        type Type = super::Row;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();

            klass.install_action_async(
                "session-row.log-out-client",
                None,
                |widget, _, _| async move {
                    widget.log_out().await;
                },
            );
            klass.install_action("session-row.open-in-window", None, |widget, _, _| {
                widget.open_account_in_window();
            });
        }

        fn instance_init(obj: &InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Row {
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
            obj.setup_expressions();

            // Right-click opens the account context menu.
            let right_click = gtk::GestureClick::new();
            right_click.set_button(gdk::BUTTON_SECONDARY);
            right_click.connect_released(clone!(
                #[weak(rename_to = imp)]
                self,
                move |_, _, x, y| {
                    imp.show_menu(x as i32, y as i32);
                }
            ));
            obj.add_controller(right_click);

            // Ctrl+click opens the account in a separate window. Capture phase so it beats
            // the list view's single-click activation; a plain click is denied and falls
            // through to normal account switching.
            let ctrl_click = gtk::GestureClick::new();
            ctrl_click.set_button(gdk::BUTTON_PRIMARY);
            ctrl_click.set_propagation_phase(gtk::PropagationPhase::Capture);
            ctrl_click.connect_pressed(clone!(
                #[weak]
                obj,
                move |gesture, _, _, _| {
                    if gesture
                        .current_event_state()
                        .contains(gdk::ModifierType::CONTROL_MASK)
                    {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        obj.open_account_in_window();
                    } else {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                    }
                }
            ));
            obj.add_controller(ctrl_click);
        }

        fn dispose(&self) {
            if let Some(menu) = self.menu.get() {
                menu.unparent();
            }
            self.dispose_template();
            // `dispose_template` leaves the template's root box parented on this widget
            // subclass; unparent whatever remains so list recycling doesn't finalize the
            // row with live children.
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Row {}

    impl Row {
        fn menu(&self) -> &gtk::PopoverMenu {
            self.menu.get_or_init(|| {
                gtk::Builder::from_resource("/app/drey/paper-plane/ui/session/row_menu.ui")
                    .object::<gtk::PopoverMenu>("menu")
                    .unwrap()
            })
        }

        fn show_menu(&self, x: i32, y: i32) {
            let menu = self.menu();
            menu.set_pointing_to(Some(&gdk::Rectangle::new(x, y, 0, 0)));
            menu.unparent();
            menu.set_parent(&*self.obj());
            menu.popup();
        }
    }
}

glib::wrapper! {
    pub(crate) struct Row(ObjectSubclass<imp::Row>)
        @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Row {
    pub(crate) fn new(session: &model::ClientStateSession) -> Self {
        glib::Object::builder().property("session", session).build()
    }

    /// Opens this account in a new separate window. Accounts may be shown in several
    /// windows at once, so there is no duplicate check.
    fn open_account_in_window(&self) {
        let Some(session) = self.session() else {
            return;
        };
        let client = session.client_();

        let Some(window) = utils::ancestor::<_, ui::Window>(self) else {
            return;
        };
        let Some(app) = window.application().and_downcast::<crate::Application>() else {
            return;
        };
        app.present_window_for_account(&client);
    }

    pub(crate) async fn log_out(&self) {
        if let Some(session) = self.session() {
            let dialog = ui::destructive_confirmation_dialog(
                &gettext_f(
                    "Log out <i>{display_name}</i>",
                    &[(
                        "display_name",
                        &strings::user_display_name(&session.me_(), true),
                    )],
                ),
                &gettext("Are you sure you want to log out?"),
                ("cancel", &gettext("_Cancel")),
                ("log-out", &gettext("_Log out")),
            );
            dialog.set_heading_use_markup(true);

            if dialog.choose_future(Some(self)).await == "log-out" {
                let imp = self.imp();

                imp.stack.set_visible_child(&imp.spinner.get());
                imp.spinner.set_spinning(true);

                self.action_set_enabled("session-row.log-out-client", false);
                if let Err(e) = session.client_().log_out().await {
                    utils::show_toast(
                        self,
                        gettext_f("Failed to log out: {error}", &[("error", &e.message)]),
                    );
                    self.action_set_enabled("session-row.log-out-client", true);
                }

                imp.stack.set_visible_child_name("button");
                imp.spinner.set_spinning(false);
            }
        }
    }

    fn setup_expressions(&self) {
        let imp = self.imp();
        let me_expr =
            Self::this_expression("session").chain_property::<model::ClientStateSession>("me");

        // Bind the name
        expressions::user_display_name(&me_expr).bind(
            &*imp.display_name_label,
            "label",
            Some(self),
        );

        // Bind the username
        let username_expr = me_expr.chain_property::<model::User>("username");
        username_expr
            .chain_closure::<String>(closure!(|_: Self, username: String| {
                format!("@{username}")
            }))
            .bind(&*imp.username_label, "label", Some(self));

        let username_not_empty_expr =
            username_expr.chain_closure::<bool>(closure!(|_: Self, username: String| {
                !username.is_empty()
            }));

        username_not_empty_expr
            .chain_closure::<f64>(closure!(|_: Self, not_empty: bool| {
                if not_empty { 1.0 } else { 0.5 }
            }))
            .bind(&*imp.display_name_label, "yalign", Some(self));

        username_not_empty_expr.bind(&*imp.username_label, "visible", Some(self));
    }
}
