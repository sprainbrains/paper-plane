use std::cell::OnceCell;
use std::cell::RefCell;

use adw::subclass::prelude::*;
use glib::clone;
use gtk::CompositeTemplate;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::model;
use crate::ui;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, CompositeTemplate)]
    #[template(resource = "/app/drey/paper-plane/ui/session/content/send_media_window.ui")]
    pub(crate) struct SendMediaWindow {
        pub(super) chat: glib::WeakRef<model::Chat>,
        pub(super) paths: RefCell<Vec<String>>,
        pub(super) reply_to: OnceCell<i64>,
        pub(super) emoji_chooser: RefCell<Option<gtk::EmojiChooser>>,
        #[template_child]
        pub(super) toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub(super) picture: TemplateChild<gtk::Picture>,
        #[template_child]
        pub(super) file_count_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub(super) caption_entry: TemplateChild<ui::MessageEntry>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SendMediaWindow {
        const NAME: &'static str = "PaplSendMediaWindow";
        type Type = super::SendMediaWindow;
        type ParentType = adw::Window;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();

            klass.install_action_async(
                "send-media-window.send-message",
                None,
                |widget, _, _| async move {
                    widget.send_message(false).await;
                },
            );
            klass.install_action_async(
                "send-media-window.send-as-file",
                None,
                |widget, _, _| async move {
                    widget.send_message(true).await;
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SendMediaWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            self.caption_entry.connect_activate(clone!(
                #[weak]
                obj,
                move |_| {
                    obj.activate_action("send-media-window.send-message", None)
                        .unwrap()
                }
            ));

            self.caption_entry.connect_emoji_button_press(clone!(
                #[weak]
                obj,
                move |_, button| {
                    obj.show_emoji_chooser(&button);
                }
            ));
        }

        fn dispose(&self) {
            if let Some(emoji_chooser) = self.emoji_chooser.take() {
                emoji_chooser.unparent();
            }
        }
    }

    impl WidgetImpl for SendMediaWindow {}
    impl WindowImpl for SendMediaWindow {}
    impl AdwWindowImpl for SendMediaWindow {}

    #[gtk::template_callbacks]
    impl SendMediaWindow {
        #[template_callback]
        fn on_key_pressed(
            &self,
            key: gdk::Key,
            _: u32,
            modifier: gdk::ModifierType,
            _: &gtk::EventControllerKey,
        ) -> glib::Propagation {
            if key == gdk::Key::Escape
                || (key == gdk::Key::w && modifier == gdk::ModifierType::CONTROL_MASK)
            {
                self.obj().close();
            }

            glib::Propagation::Proceed
        }
    }
}

glib::wrapper! {
    pub(crate) struct SendMediaWindow(ObjectSubclass<imp::SendMediaWindow>)
        @extends gtk::Widget, gtk::Window, adw::Window,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl SendMediaWindow {
    pub(crate) fn new(
        parent: &gtk::Window,
        chat: &model::Chat,
        paths: Vec<String>,
        reply_to: i64,
    ) -> Self {
        let obj: Self = glib::Object::builder()
            .property("transient-for", parent)
            .build();
        let imp = obj.imp();

        // The first image previews the pick; the badge tells the rest will
        // travel with it as one album.
        if let Some(path) = paths.first() {
            imp.picture.set_filename(Some(path));
        }
        if paths.len() > 1 {
            imp.file_count_label.set_label(&paths.len().to_string());
            imp.file_count_label.set_visible(true);
        }
        imp.caption_entry.set_chat(Some(chat.clone()));

        imp.chat.set(Some(chat));
        imp.paths.replace(paths);
        imp.reply_to.set(reply_to).unwrap();

        obj
    }

    fn show_emoji_chooser(&self, parent: &impl IsA<gtk::Widget>) {
        let imp = self.imp();
        let mut emoji_chooser = imp.emoji_chooser.borrow_mut();
        if emoji_chooser.is_none() {
            let chooser = gtk::EmojiChooser::new();
            chooser.set_parent(parent);
            chooser.connect_emoji_picked(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_, emoji| {
                    obj.imp().caption_entry.insert_at_cursor(emoji);
                }
            ));
            chooser.connect_hide(clone!(
                #[weak(rename_to = obj)]
                self,
                move |_| {
                    obj.imp().caption_entry.grab_focus();
                }
            ));
            *emoji_chooser = Some(chooser);
        }
        emoji_chooser.as_ref().unwrap().popup();
    }

    async fn send_message(&self, send_as_file: bool) {
        use tdlib::enums::*;
        use tdlib::types::*;

        let imp = self.imp();

        let Some(chat) = imp.chat.upgrade() else {
            return;
        };

        // Decoding and sending run in the background now; lock the buttons so
        // the same album cannot be fired off twice meanwhile.
        self.action_set_enabled("send-media-window.send-message", false);
        self.action_set_enabled("send-media-window.send-as-file", false);

        let paths = imp.paths.borrow().clone();
        let caption = imp.caption_entry.as_markdown().await;

        // Decode every file off the main thread: a phone-sized photo takes a
        // noticeable moment, and decoding them inline froze the whole window.
        let built = gio::spawn_blocking(move || {
            let mut contents: Vec<InputMessageContent> = Vec::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                let file = InputFile::Local(InputFileLocal { path: path.clone() });
                // Fall back to sending as a document when the file couldn't be
                // decoded as an image, even if the user chose "send as photo".
                let paintable = (!send_as_file)
                    .then(|| utils::decode_image_from_path(path).ok())
                    .flatten();

                // The caption belongs to the album as a whole, so it rides on
                // the first member only (the album's primary).
                let caption = (index == 0).then(|| caption.clone()).flatten();

                contents.push(match paintable {
                    Some(paintable) => InputMessageContent::InputMessagePhoto(InputMessagePhoto {
                        photo: tdlib::types::InputPhoto {
                            photo: file,
                            thumbnail: None,
                            video: None,
                            added_sticker_file_ids: vec![],
                            width: paintable.intrinsic_width(),
                            height: paintable.intrinsic_height(),
                        },
                        caption,
                        show_caption_above_media: false,
                        self_destruct_type: None,
                        has_spoiler: false,
                    }),
                    None => InputMessageContent::InputMessageDocument(InputMessageDocument {
                        document: tdlib::types::InputDocument {
                            document: file,
                            thumbnail: None,
                            disable_content_type_detection: true,
                        },
                        caption,
                    }),
                });
            }
            contents
        })
        .await;

        let contents = match built {
            Ok(contents) => contents,
            Err(e) => {
                log::warn!("Media decode task failed: {e:?}");
                self.restore_send_actions();
                return;
            }
        };

        let chat_id = chat.id();
        let client_id = chat.session_().client_().id();

        let reply_to_message_id = *imp.reply_to.get().unwrap();
        let reply_to = if reply_to_message_id != 0 {
            Some(InputMessageReplyTo::Message(InputMessageReplyToMessage {
                message_id: reply_to_message_id,
                ..Default::default()
            }))
        } else {
            None
        };

        // Several files go out as one album so every client groups them into a
        // single bubble; a lone file keeps the plain send path. Upload progress
        // belongs to the chat bubble, so the window closes as soon as TDLib has
        // accepted the message.
        let result = if contents.len() == 1 {
            tdlib::functions::send_message(
                chat_id,
                None,
                reply_to,
                None,
                contents.into_iter().next().unwrap(),
                client_id,
            )
            .await
            .map(|_| ())
        } else {
            tdlib::functions::send_message_album(chat_id, None, reply_to, None, contents, client_id)
                .await
                .map(|_| ())
        };

        match result {
            Ok(()) => self.close(),
            Err(e) => {
                imp.toast_overlay.add_toast(
                    adw::Toast::builder()
                        .title(e.message)
                        .timeout(3)
                        .priority(adw::ToastPriority::High)
                        .build(),
                );
                self.restore_send_actions();
            }
        }
    }

    fn restore_send_actions(&self) {
        self.action_set_enabled("send-media-window.send-message", true);
        self.action_set_enabled("send-media-window.send-as-file", true);
    }
}
