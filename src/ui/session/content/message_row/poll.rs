use gettextrs::gettext;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::base::MessageHandler;
use crate::i18n::ngettext_f;
use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate, glib::Properties)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/poll.ui")]
    #[properties(wrapper_type = super::MessagePoll)]
    pub(crate) struct MessagePoll {
        #[property(get, set = Self::set_message, explicit_notify)]
        pub(super) message: glib::WeakRef<model::Message>,
        pub(super) handler: MessageHandler,
        #[template_child]
        pub(super) message_bubble: TemplateChild<ui::MessageBubble>,
        #[template_child]
        pub(super) question_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) options_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) votes_label: TemplateChild<gtk::Label>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessagePoll {
        const NAME: &'static str = "PaplMessagePoll";
        type Type = super::MessagePoll;
        type ParentType = ui::MessageBase;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessagePoll {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            self.derived_set_property(id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            self.derived_property(id, pspec)
        }
    }

    impl MessagePoll {
        fn set_message(&self, message: Option<model::Message>) {
            ui::MessageBaseExt::set_message(&*self.obj(), message.as_ref().unwrap());
        }
    }

    impl WidgetImpl for MessagePoll {}
    impl ui::MessageBaseImpl for MessagePoll {}
}

glib::wrapper! {
    pub(crate) struct MessagePoll(ObjectSubclass<imp::MessagePoll>)
        @extends gtk::Widget, ui::MessageBase,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ui::MessageBaseExt for MessagePoll {
    type Message = model::Message;

    fn set_message(&self, message: &Self::Message) {
        let imp = self.imp();

        let old_message = imp.message.upgrade();
        if old_message.as_ref() == Some(message) {
            return;
        }

        imp.handler.disconnect(&imp.message);

        imp.message_bubble.update_from_message(message, true);

        // Re-render when the poll updates (e.g. someone else votes).
        let handler_id = message.connect_content_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |message| {
                obj.update_poll(message);
            }
        ));
        imp.handler.set(handler_id);

        self.update_poll(message);

        imp.message.set(Some(message));
        self.notify("message");
    }
}

impl MessagePoll {
    fn update_poll(&self, message: &model::Message) {
        let imp = self.imp();

        let tdlib::enums::MessageContent::MessagePoll(data) = message.content().0 else {
            return;
        };
        let poll = data.poll;

        imp.question_label
            .set_label(&utils::parse_formatted_text(poll.question));

        imp.votes_label.set_label(&if poll.total_voter_count == 0 {
            gettext("No votes yet")
        } else {
            ngettext_f(
                "{count} vote",
                "{count} votes",
                poll.total_voter_count as u32,
                &[("count", &poll.total_voter_count.to_string())],
            )
        });

        // Show results once the user has voted or the poll is closed.
        let show_results = poll.is_closed || poll.options.iter().any(|option| option.is_chosen);

        // Rebuild the option rows.
        while let Some(child) = imp.options_box.first_child() {
            imp.options_box.remove(&child);
        }

        for (index, option) in poll.options.into_iter().enumerate() {
            let row = self.build_option(message, index as i32, &option, show_results);
            imp.options_box.append(&row);
        }
    }

    fn build_option(
        &self,
        message: &model::Message,
        index: i32,
        option: &tdlib::types::PollOption,
        show_results: bool,
    ) -> gtk::Widget {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();

        let top = gtk::Box::builder().spacing(6).build();
        let text = gtk::Label::builder()
            .label(utils::parse_formatted_text(option.text.clone()))
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .build();
        top.append(&text);

        if show_results {
            top.append(
                &gtk::Label::builder()
                    .label(format!("{}%", option.vote_percentage))
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
        }
        content.append(&top);

        if show_results {
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(option.vote_percentage as f64 / 100.0);
            if option.is_chosen {
                bar.add_css_class("accent");
            }
            content.append(&bar);
        }

        let button = gtk::Button::builder()
            .child(&content)
            .has_frame(false)
            .build();
        button.add_css_class("poll-option");

        // Voting is only possible before results are shown.
        button.set_sensitive(!show_results);
        button.connect_clicked(clone!(
            #[weak]
            message,
            move |_| {
                let chat = message.chat_();
                let client_id = chat.session_().client_().id();
                let chat_id = chat.id();
                let message_id = message.id();
                utils::spawn(async move {
                    if let Err(e) = tdlib::functions::set_poll_answer(
                        chat_id,
                        message_id,
                        vec![index],
                        client_id,
                    )
                    .await
                    {
                        log::warn!("Failed to answer a poll: {e:?}");
                    }
                });
            }
        ));

        button.upcast()
    }
}
