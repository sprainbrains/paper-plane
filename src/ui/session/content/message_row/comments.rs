use std::cell::Cell;

use gtk::CompositeTemplate;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::i18n::ngettext_f;
use crate::model;
use crate::ui;

/// The number of replier avatars shown in the bar (TDLib provides at most three).
const MAX_AVATARS: usize = 3;
/// Avatar edge, in pixels.
const AVATAR_SIZE: i32 = 22;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/message_row/comments.ui")]
    pub(crate) struct MessageCommentsBar {
        #[template_child]
        pub(super) avatars_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) count_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) icon: TemplateChild<gtk::Image>,
        /// Set on the anchor post's bar while its thread is open: a tree query
        /// cannot answer that at bind time (content widgets bind detached), so
        /// the thread view suppresses the bar explicitly.
        pub(super) suppressed: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageCommentsBar {
        const NAME: &'static str = "PaplMessageCommentsBar";
        type Type = super::MessageCommentsBar;
        type ParentType = gtk::Button;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MessageCommentsBar {
        fn constructed(&self) {
            self.parent_constructed();
            // The bar is decorative until tapped; it should not steal the bubble's
            // own click semantics beyond opening the thread.
            self.obj().set_hexpand(true);
            self.obj().set_visible(false);
        }
    }

    impl WidgetImpl for MessageCommentsBar {}

    impl ButtonImpl for MessageCommentsBar {}
}

glib::wrapper! {
    pub(crate) struct MessageCommentsBar(ObjectSubclass<imp::MessageCommentsBar>)
        @extends gtk::Widget, gtk::Button,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl MessageCommentsBar {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Permanently hides the bar on this bubble (used for the thread anchor post,
    /// where opening the thread from inside it is pointless).
    pub(crate) fn set_suppressed(&self, suppressed: bool) {
        self.imp().suppressed.set(suppressed);
        if suppressed {
            self.set_visible(false);
        }
    }

    /// Binds the bar to a message: shown only for posts in a channel that has a
    /// discussion thread. Populates the count text and replier avatars.
    pub(crate) fn set_message(&self, message: Option<&model::Message>) {
        let imp = self.imp();

        let Some(message) = message else {
            self.set_visible(false);
            return;
        };

        if imp.suppressed.get() {
            self.set_visible(false);
            return;
        }

        let is_channel = matches!(
            message.chat_().chat_type(),
            model::ChatType::Supergroup(data) if data.is_channel()
        );
        let interaction = message.interaction_info();
        if !is_channel || !interaction.has_thread() {
            self.set_visible(false);
            return;
        }

        let reply_count = interaction.reply_count();
        if reply_count == 0 {
            imp.count_label.set_label(&gettextrs::gettext("Comment"));
            imp.icon.set_visible(true);
            self.clear_avatars();
        } else {
            imp.count_label.set_label(&ngettext_f(
                "{n} comment",
                "{n} comments",
                reply_count,
                &[("n", &reply_count.to_string())],
            ));
            imp.icon.set_visible(false);
            self.update_avatars(&interaction, message);
        }

        self.set_visible(true);
    }

    fn clear_avatars(&self) {
        let imp = self.imp();
        while let Some(child) = imp.avatars_box.first_child() {
            imp.avatars_box.remove(&child);
        }
    }

    fn update_avatars(
        &self,
        interaction: &model::MessageInteractionInfo,
        message: &model::Message,
    ) {
        let imp = self.imp();
        self.clear_avatars();

        let session = message.chat_().session_();
        // Render newest-first but overlap visually left-to-right; reverse so the
        // most recent replier sits on top of the overlap.
        let repliers = interaction.recent_repliers();
        for (index, sender) in repliers.iter().take(MAX_AVATARS).rev().enumerate() {
            let sender = model::MessageSender::new(&session, sender);
            let avatar = ui::Avatar::new();
            avatar.set_size(AVATAR_SIZE);
            match &sender {
                model::MessageSender::User(user) => avatar.set_item(Some(user.clone().upcast())),
                model::MessageSender::Chat(chat) => avatar.set_item(Some(chat.clone().upcast())),
            }
            // Overlap avatars (tdesktop style): every non-leading one is pulled
            // back under its neighbour, which paints on top.
            if index > 0 {
                avatar.set_margin_end(-(AVATAR_SIZE / 3));
            }
            imp.avatars_box.append(&avatar);
        }
    }
}

impl Default for MessageCommentsBar {
    fn default() -> Self {
        Self::new()
    }
}
