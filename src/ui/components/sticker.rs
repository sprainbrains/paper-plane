use std::cell::Cell;
use std::cell::RefCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use super::sticker_cache;
use crate::model;
use crate::utils;

mod imp {
    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::Sticker)]
    pub(crate) struct Sticker {
        pub(super) file_id: Cell<i32>,
        pub(super) generation: Cell<u64>,
        pub(super) demand: Cell<bool>,
        pub(super) aspect_ratio: Cell<f64>,
        pub(super) child: RefCell<Option<gtk::Widget>>,
        pub(super) sticker: RefCell<Option<tdlib::types::Sticker>>,
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        pub(super) looped: Cell<bool>,

        #[property(get, set = Self::set_longer_side_size)]
        pub(super) longer_side_size: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sticker {
        const NAME: &'static str = "PaplSticker";
        type Type = super::Sticker;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Sticker {
        fn properties() -> &'static [glib::ParamSpec] {
            Self::derived_properties()
        }

        fn set_property(&self, id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
            Self::derived_set_property(self, id, value, pspec)
        }

        fn property(&self, id: usize, pspec: &glib::ParamSpec) -> glib::Value {
            Self::derived_property(self, id, pspec)
        }

        fn dispose(&self) {
            self.generation.set(self.generation.get().wrapping_add(1));
            if let Some(child) = self.child.replace(None) {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Sticker {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if let Some(child) = &*self.child.borrow() {
                let _ = child.measure(orientation, for_size);
            }
            let size = self.longer_side_size.get();
            let aspect_ratio = self.aspect_ratio.get();
            let measured = if orientation == gtk::Orientation::Horizontal {
                if aspect_ratio >= 1.0 {
                    size
                } else {
                    (size as f64 * aspect_ratio) as i32
                }
            } else if aspect_ratio >= 1.0 {
                (size as f64 / aspect_ratio) as i32
            } else {
                size
            }
            .max(1);
            (measured, measured, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = &*self.child.borrow() {
                child.allocate(width, height, baseline, None);
            }
        }

        fn map(&self) {
            self.parent_map();
            self.demand.set(true);
            self.obj().start_loading();
            self.obj().resume_animation();
        }

        fn unmap(&self) {
            self.demand.set(false);
            self.generation.set(self.generation.get().wrapping_add(1));
            self.obj().pause_animation();
            self.parent_unmap();
        }
    }

    impl Sticker {
        fn set_longer_side_size(&self, size: i32) {
            self.longer_side_size.set(size);
            self.obj().queue_resize();
        }
    }
}

glib::wrapper! {
    pub(crate) struct Sticker(ObjectSubclass<imp::Sticker>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sticker {
    pub(crate) fn update_sticker(
        &self,
        sticker: tdlib::types::Sticker,
        looped: bool,
        session: model::ClientStateSession,
    ) {
        self.begin_update(&sticker);
        self.imp().sticker.replace(Some(sticker));
        self.imp().session.set(Some(&session));
        self.imp().looped.set(looped);
        if self.is_mapped() {
            self.imp().demand.set(true);
            self.start_loading();
        }
    }

    fn start_loading(&self) {
        let imp = self.imp();
        if imp.child.borrow().is_some() || !imp.demand.get() {
            return;
        }
        let Some(sticker) = imp.sticker.borrow().clone() else {
            return;
        };
        let Some(session) = imp.session.upgrade() else {
            return;
        };
        imp.generation.set(imp.generation.get().wrapping_add(1));
        let file = sticker.sticker;
        let file_id = file.id;
        let format = sticker.format;
        let generation = imp.generation.get();
        let looped = imp.looped.get();
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                if let Some(path) = sticker_cache::download_sticker_path(file, &session).await {
                    obj.install_full_media(path, file_id, generation, looped, format)
                        .await;
                }
            }
        ));
    }

    fn begin_update(&self, sticker: &tdlib::types::Sticker) {
        let imp = self.imp();
        imp.file_id.set(sticker.sticker.id);
        imp.generation.set(imp.generation.get().wrapping_add(1));
        imp.aspect_ratio
            .set(sticker.width as f64 / sticker.height.max(1) as f64);
        self.set_child(None);
    }

    async fn install_full_media(
        &self,
        path: String,
        file_id: i32,
        generation: u64,
        looped: bool,
        format: tdlib::enums::StickerFormat,
    ) {
        if !self.is_current(file_id, generation) {
            return;
        }
        let widget: gtk::Widget = match format {
            tdlib::enums::StickerFormat::Tgs => {
                let animation = rlt::Animation::from_filename(&path);
                animation.set_loop(looped);
                animation.use_cache(looped);
                // Only animate while on-screen; map()/unmap() drive play/pause.
                if self.is_mapped() {
                    animation.play();
                }
                animation.upcast()
            }
            tdlib::enums::StickerFormat::Webp => {
                let target_size = self.longer_side_size().max(1) * self.scale_factor().max(1);
                let key = sticker_cache::PreviewKey {
                    file_id,
                    size: target_size,
                };
                // Reuse the decoded texture across ListView recycling so scrolling
                // back over a sticker never re-decodes.
                let cached = sticker_cache::cached_texture(key);
                let texture = if let Some(texture) = cached {
                    texture
                } else {
                    let permit = sticker_cache::acquire_decode().await;
                    if !self.is_current(file_id, generation) {
                        return;
                    }
                    let result = gio::spawn_blocking(move || {
                        utils::decode_image_from_path_at_size(&path, Some(target_size as u32))
                    })
                    .await;
                    drop(permit);
                    match result {
                        Ok(Ok(texture)) => {
                            let texture = texture.upcast::<gdk::Texture>();
                            sticker_cache::cache_texture(key, texture.clone());
                            texture
                        }
                        Ok(Err(error)) => {
                            log::warn!(target: "paper_plane::sticker", "failed to decode sticker {file_id}: {error}");
                            return;
                        }
                        Err(_) => {
                            log::warn!(target: "paper_plane::sticker", "sticker decode task failed for {file_id}");
                            return;
                        }
                    }
                };
                let picture = gtk::Picture::new();
                picture.set_paintable(Some(&texture));
                picture.upcast()
            }
            tdlib::enums::StickerFormat::Webm => {
                let media = gtk::MediaFile::for_file(&gio::File::for_path(path));
                media.set_loop(looped);
                media.set_muted(true);
                if self.is_mapped() {
                    media.play();
                }
                let picture = gtk::Picture::for_paintable(&media);
                picture.set_content_fit(gtk::ContentFit::Contain);
                picture.set_can_shrink(true);
                picture.upcast()
            }
        };
        if self.is_current(file_id, generation) {
            self.set_child(Some(widget));
        }
    }

    fn is_current(&self, file_id: i32, generation: u64) -> bool {
        let imp = self.imp();
        imp.demand.get()
            && self.is_mapped()
            && imp.file_id.get() == file_id
            && imp.generation.get() == generation
    }

    pub(crate) fn play_animation(&self) {
        if !self.is_mapped() {
            return;
        }
        if let Some(animation) = &*self.imp().child.borrow()
            && let Some(animation) = animation.downcast_ref::<rlt::Animation>()
            && !animation.is_playing()
        {
            animation.play();
        }
    }

    /// Resume playback when the sticker scrolls back into view.
    fn resume_animation(&self) {
        let child = self.imp().child.borrow();
        let Some(child) = child.as_ref() else {
            return;
        };
        if let Some(animation) = child.downcast_ref::<rlt::Animation>() {
            if !animation.is_playing() {
                animation.play();
            }
        } else if let Some(picture) = child.downcast_ref::<gtk::Picture>()
            && let Some(media) = picture.paintable().and_downcast::<gtk::MediaFile>()
        {
            media.play();
        }
    }

    /// Pause playback (keeping the last frame on screen) while off-screen so a chat
    /// full of animated stickers doesn't keep every decoder running.
    fn pause_animation(&self) {
        let child = self.imp().child.borrow();
        let Some(child) = child.as_ref() else {
            return;
        };
        if let Some(animation) = child.downcast_ref::<rlt::Animation>() {
            animation.pause();
        } else if let Some(picture) = child.downcast_ref::<gtk::Picture>()
            && let Some(media) = picture.paintable().and_downcast::<gtk::MediaFile>()
        {
            media.pause();
        }
    }

    fn set_child(&self, child: Option<gtk::Widget>) {
        let imp = self.imp();
        if let Some(ref child) = child {
            child.set_parent(self);
        }
        if let Some(old) = imp.child.replace(child) {
            old.unparent();
        }
    }
}
