use std::cell::Cell;
use std::cell::RefCell;
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
use crate::utils;

const CELL_WIDTH: i32 = 104;
const CELL_HEIGHT: i32 = 88;

/// Number of live GIF `MediaFile` pipelines — a resource-usage gauge; bounded by
/// [`MAX_CONCURRENT_GIFS`].
static LIVE_GIFS: AtomicUsize = AtomicUsize::new(0);

/// Maximum number of GIFs downloading and playing (each a `gtk::MediaFile` /
/// GStreamer pipeline) at once. Sized to cover a viewport (~3 columns × a few rows)
/// so every visible GIF plays, while still bounding live pipelines — unbounded
/// playback opens a file descriptor + GWakeup pipe per pipeline and floods the TDLib
/// download queue, exhausting the fd limit ("Too many open files").
const MAX_CONCURRENT_GIFS: usize = 16;

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
        pub(super) media: RefCell<Option<gtk::MediaFile>>,
        /// The playback slot held while this cell's GIF is downloading/playing.
        pub(super) permit: RefCell<Option<GifPermit>>,
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

    /// Bind the cell to a saved animation: show its static minithumbnail immediately
    /// and, since a bound cell is on-screen, begin muted looped playback in the
    /// background. Any previous playback is torn down first.
    pub(crate) fn set_animation(
        &self,
        animation: tdlib::types::Animation,
        session: &model::ClientStateSession,
    ) {
        let imp = self.imp();
        imp.session.set(Some(session));
        imp.generation.set(imp.generation.get().wrapping_add(1));
        let generation = imp.generation.get();
        self.stop_media();

        let Some(picture) = imp.picture.borrow().clone() else {
            return;
        };
        if let Some(mini) = &animation.minithumbnail
            && let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(
                glib::base64_decode(&mini.data),
            ))
        {
            picture.set_paintable(Some(&texture));
        } else {
            picture.set_paintable(gdk::Paintable::NONE);
        }

        let file = animation.animation;
        utils::spawn(clone!(
            #[weak(rename_to = obj)]
            self,
            #[weak]
            session,
            async move {
                // Wait for a playback slot before downloading anything, so at most
                // MAX_CONCURRENT_GIFS download and play at a time.
                let permit = GifPermit::acquire().await;
                if obj.imp().generation.get() != generation {
                    return;
                }
                let Some(path) = gif_path(file, &session).await else {
                    return;
                };
                if obj.imp().generation.get() != generation {
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

    /// Tear down playback when the cell scrolls off-screen (unbind) so off-screen
    /// GIFs cost nothing. The static minithumbnail is restored on the next bind.
    pub(crate) fn clear(&self) {
        let imp = self.imp();
        imp.generation.set(imp.generation.get().wrapping_add(1));
        self.stop_media();
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

async fn gif_path(
    file: tdlib::types::File,
    session: &model::ClientStateSession,
) -> Option<String> {
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
