use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gtk::gdk;
use gtk::gio;
use gtk::glib;

use crate::model;
use crate::ui;
use crate::utils;

const CELL_WIDTH: i32 = 104;
const CELL_HEIGHT: i32 = 88;

/// Number of live GIF `MediaFile` pipelines — a resource-usage gauge; bounded by
/// [`MAX_CONCURRENT_GIFS`].
static LIVE_GIFS: AtomicUsize = AtomicUsize::new(0);

/// Maximum number of GIFs playing (each a `gtk::MediaFile` / GStreamer pipeline) at
/// once. Playback is now hover-driven — only the cell under the pointer animates — so
/// this is just a safety cap against rapid hovering; the grid at rest holds zero
/// pipelines. Spinning up a pipeline per visible cell (the old behaviour) opened a file
/// descriptor + GWakeup pipe each and could exhaust the fd limit and abort the app.
const MAX_CONCURRENT_GIFS: usize = 4;

fn gif_limiter() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    static LIMITER: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
        OnceLock::new();
    LIMITER.get_or_init(|| {
        let (sender, receiver) = async_channel::bounded(MAX_CONCURRENT_GIFS);
        for _ in 0..MAX_CONCURRENT_GIFS {
            sender.try_send(()).unwrap();
        }
        (sender, receiver)
    })
}

/// Holds one GIF playback slot; releasing it (on drop) frees the slot.
struct GifPermit(&'static async_channel::Sender<()>);

impl GifPermit {
    async fn acquire() -> Self {
        let limiter = gif_limiter();
        limiter.1.recv().await.unwrap();
        Self(&limiter.0)
    }
}

impl Drop for GifPermit {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub(crate) struct GifCell {
        pub(super) picture: RefCell<Option<gtk::Picture>>,
        pub(super) session: glib::WeakRef<model::ClientStateSession>,
        /// The animation bound to this cell, kept so hovering can start playback of the
        /// full file on demand.
        pub(super) animation: RefCell<Option<tdlib::types::Animation>>,
        /// The decoded static preview texture, restored when playback stops.
        pub(super) preview: RefCell<Option<gdk::Texture>>,
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        /// The playback slot held while this cell's GIF is playing.
        pub(super) permit: RefCell<Option<GifPermit>>,
        /// Whether the pointer is currently over the cell — playback should run only
        /// while this is set, so a download that finishes after the pointer left is
        /// discarded.
        pub(super) wants_play: Cell<bool>,
        /// Live "this cell still wants its static preview" flag handed to the shared
        /// preview loader; cleared on unbind so stale decodes are dropped.
        pub(super) demand: RefCell<Rc<Cell<bool>>>,
        pub(super) generation: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GifCell {
        const NAME: &'static str = "PaplGifCell";
        type Type = super::GifCell;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_layout_manager_type::<gtk::BinLayout>();
            klass.set_css_name("gifcell");
        }
    }

    impl ObjectImpl for GifCell {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("gif-picker-item");
            let picture = gtk::Picture::builder()
                .width_request(CELL_WIDTH)
                .height_request(CELL_HEIGHT)
                .content_fit(gtk::ContentFit::Cover)
                .build();
            picture.set_parent(&*obj);
            *self.picture.borrow_mut() = Some(picture);

            // Animate only the hovered cell: the grid at rest runs no pipelines, so
            // opening the tab can never spin up a pipeline per visible GIF.
            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter(clone!(
                #[weak]
                obj,
                move |_, _, _| obj.start_playback()
            ));
            motion.connect_leave(clone!(
                #[weak]
                obj,
                move |_| obj.stop_playback()
            ));
            obj.add_controller(motion);
        }

        fn dispose(&self) {
            self.obj().stop_media();
            if let Some(picture) = self.picture.take() {
                picture.unparent();
            }
        }
    }

    impl WidgetImpl for GifCell {}
}

glib::wrapper! {
    pub(crate) struct GifCell(ObjectSubclass<imp::GifCell>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for GifCell {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GifCell {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Bind the cell to a saved animation: show its static minithumbnail immediately,
    /// then (once the grid has settled) replace it with a crisp, cached static preview.
    /// Nothing animates until the pointer hovers the cell. Any previous playback is torn
    /// down first.
    ///
    /// `scrolling` is the picker's shared "grid is scrolling" flag: while it is set, the
    /// preview download/decode is deferred so flinging through the grid stays cheap.
    pub(crate) fn set_animation(
        &self,
        animation: tdlib::types::Animation,
        session: &model::ClientStateSession,
        scrolling: Rc<Cell<bool>>,
    ) {
        let imp = self.imp();
        imp.session.set(Some(session));
        imp.generation.set(imp.generation.get().wrapping_add(1));
        let generation = imp.generation.get();
        imp.wants_play.set(false);
        self.stop_media();
        imp.preview.replace(None);
        // Retire the previous bind's preview request, then start a fresh one.
        imp.demand.borrow().set(false);
        let demand = Rc::new(Cell::new(true));
        imp.demand.replace(demand.clone());

        let Some(picture) = imp.picture.borrow().clone() else {
            return;
        };
        if let Some(mini) = &animation.minithumbnail
            && let Ok(texture) =
                gdk::Texture::from_bytes(&glib::Bytes::from_owned(glib::base64_decode(&mini.data)))
        {
            picture.set_paintable(Some(&texture));
        } else {
            picture.set_paintable(gdk::Paintable::NONE);
        }

        imp.animation.replace(Some(animation.clone()));

        // Load a crisp static preview from the animation's thumbnail (a cheap, cached,
        // rate-limited image decode — the same path the sticker previews use), rather
        // than a video pipeline. Only static image thumbnails are usable here.
        let Some(thumbnail) = animation.thumbnail.as_ref().and_then(|thumbnail| {
            matches!(
                thumbnail.format,
                tdlib::enums::ThumbnailFormat::Jpeg
                    | tdlib::enums::ThumbnailFormat::Png
                    | tdlib::enums::ThumbnailFormat::Webp
                    | tdlib::enums::ThumbnailFormat::Gif
            )
            .then(|| thumbnail.file.clone())
        }) else {
            return;
        };
        let preview_size = CELL_WIDTH.max(CELL_HEIGHT) * self.scale_factor().max(1);
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                // Defer the preview while the grid is scrolling; bail if recycled.
                while scrolling.get() {
                    glib::timeout_future(std::time::Duration::from_millis(50)).await;
                    if obj.imp().generation.get() != generation {
                        return;
                    }
                }
                let Ok(texture) =
                    ui::load_picker_file_preview(thumbnail, preview_size, &session, demand).await
                else {
                    return;
                };
                if obj.imp().generation.get() != generation {
                    return;
                }
                obj.imp().preview.replace(Some(texture.clone()));
                // Don't clobber a running hover animation with the static preview.
                if obj.imp().media.borrow().is_none()
                    && let Some(picture) = obj.imp().picture.borrow().as_ref()
                {
                    picture.set_paintable(Some(&texture));
                }
            }
        ));
    }

    /// Begin muted looped playback of the hovered cell's full animation.
    fn start_playback(&self) {
        let imp = self.imp();
        imp.wants_play.set(true);
        if imp.media.borrow().is_some() {
            return;
        }
        let Some(session) = imp.session.upgrade() else {
            return;
        };
        let Some(animation) = imp.animation.borrow().clone() else {
            return;
        };
        let generation = imp.generation.get();
        let file = animation.animation;
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                // Bound concurrent pipelines even under rapid hovering.
                let permit = GifPermit::acquire().await;
                if obj.imp().generation.get() != generation || !obj.imp().wants_play.get() {
                    return;
                }
                let Some(path) = gif_path(file, &session).await else {
                    return;
                };
                if obj.imp().generation.get() != generation || !obj.imp().wants_play.get() {
                    return;
                }
                let media = gtk::MediaFile::for_file(&gio::File::for_path(path));
                media.set_loop(true);
                media.set_muted(true);
                media.play();
                if let Some(picture) = obj.imp().picture.borrow().as_ref() {
                    picture.set_paintable(Some(&media));
                }
                *obj.imp().media.borrow_mut() = Some(media);
                let live = LIVE_GIFS.fetch_add(1, Ordering::Relaxed) + 1;
                log::debug!(target: "paper_plane::content_picker", "gif playing; live_gifs={live}");
                // Hold the slot until playback is torn down (stop_media).
                *obj.imp().permit.borrow_mut() = Some(permit);
            }
        ));
    }

    /// Stop the hover animation and restore the static preview.
    fn stop_playback(&self) {
        let imp = self.imp();
        imp.wants_play.set(false);
        self.stop_media();
        if let Some(picture) = imp.picture.borrow().as_ref() {
            if let Some(preview) = imp.preview.borrow().as_ref() {
                picture.set_paintable(Some(preview));
            }
        }
    }

    /// Tear down state when the cell scrolls off-screen (unbind) so off-screen cells
    /// cost nothing. The preview is restored on the next bind.
    pub(crate) fn clear(&self) {
        let imp = self.imp();
        imp.generation.set(imp.generation.get().wrapping_add(1));
        imp.wants_play.set(false);
        imp.demand.borrow().set(false);
        self.stop_media();
        imp.animation.replace(None);
        imp.preview.replace(None);
        if let Some(picture) = imp.picture.borrow().as_ref() {
            picture.set_paintable(gdk::Paintable::NONE);
        }
    }

    fn stop_media(&self) {
        if let Some(media) = self.imp().media.borrow_mut().take() {
            media.pause();
            LIVE_GIFS.fetch_sub(1, Ordering::Relaxed);
        }
        // Release the playback slot so another cell can start.
        self.imp().permit.borrow_mut().take();
    }
}

async fn gif_path(file: tdlib::types::File, session: &model::ClientStateSession) -> Option<String> {
    if file.local.is_downloading_completed {
        return Some(file.local.path);
    }
    match session.download_file(file.id).await {
        Ok(file) => Some(file.local.path),
        Err(error) => {
            log::warn!(target: "paper_plane::content_picker", "failed to download GIF {}: {error:?}", file.id);
            None
        }
    }
}
