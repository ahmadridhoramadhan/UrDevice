//! Android gallery (#17): the desktop side (R8.7).
//!
//! The view is `ui/features/gallery.slint` (global `GalleryFeature`): pages
//! of thumbnails from the phone, selection, and download into the download
//! folder through the Files transfer list. See `docs/dev/adding-a-feature.md`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, VecModel};
use urdevice_core::Capability;
use urdevice_core::identity::DeviceId;
use urdevice_core::node::PeerView;
use urdevice_core::services::gallery::{Gallery as GalleryService, GalleryEvents};
use urdevice_platform::features::files::{DesktopFiles, open_folder};
use urdevice_proto::features::gallery::{GalleryItem, GalleryPage};

use super::files::human;
use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{GalleryFeature, GalleryTile, MainWindow};

pub(crate) struct Gallery;

/// Largest thumbnail side decoded.
const MAX_THUMB_SIDE: usize = 512;

/// Decode a JPEG thumbnail from the phone. Untrusted: size-limited, and a
/// bad one is just not shown.
fn thumb(jpeg: &[u8]) -> Option<Image> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    if jpeg.is_empty() {
        return None;
    }
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(MAX_THUMB_SIDE)
        .set_max_height(MAX_THUMB_SIDE);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(jpeg), options);
    let rgb = d.decode().ok()?;
    let (w, h) = d.dimensions()?;
    if rgb.len() != w.checked_mul(h)?.checked_mul(3)? {
        return None;
    }
    let buf = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(
        &rgb,
        u32::try_from(w).ok()?,
        u32::try_from(h).ok()?,
    );
    Some(Image::from_rgb8(buf))
}

fn duration(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// When it was taken, in local time. Empty when the phone did not say.
fn taken(ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%-d %b %Y, %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

#[derive(Default)]
struct State {
    peer: Option<DeviceId>,
    items: Vec<GalleryItem>,
    selected: Vec<bool>,
    next: u64,
}

struct Ui {
    svc: Arc<GalleryService>,
    window: slint::Weak<MainWindow>,
    tiles: Rc<VecModel<GalleryTile>>,
    st: RefCell<State>,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

fn with_ui(f: impl FnOnce(&Rc<Ui>)) {
    if let Some(ui) = UI.with(|u| u.borrow().clone()) {
        f(&ui);
    }
}

impl Ui {
    fn global(&self, f: impl FnOnce(&GalleryFeature<'_>)) {
        if let Some(w) = self.window.upgrade() {
            f(&w.global::<GalleryFeature>());
        }
    }

    fn count(&self) {
        let n = self.st.borrow().selected.iter().filter(|s| **s).count() as i32;
        self.global(|g| g.set_selected(n));
    }

    fn load(&self, cursor: u64) {
        let Some(peer) = self.st.borrow().peer else {
            return;
        };
        self.global(|g| {
            g.set_loading(true);
            g.set_error("".into());
        });
        if self.svc.browse(peer, cursor).is_err() {
            self.global(|g| {
                g.set_loading(false);
                g.set_error("failed".into());
            });
        }
    }

    fn reset(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.items.clear();
            st.selected.clear();
            st.next = 0;
        }
        self.tiles.set_vec(Vec::new());
        self.global(|g| {
            g.set_more(false);
            g.set_queued(0);
            g.set_detail(-1);
        });
        self.count();
    }

    fn page(&self, peer: DeviceId, page: GalleryPage) {
        let mut st = self.st.borrow_mut();
        if st.peer != Some(peer) {
            return;
        }
        let error = page.error.clone();
        self.global(|g| {
            g.set_loading(false);
            g.set_error(error.into());
        });
        if !page.error.is_empty() || page.cursor != st.items.len() as u64 {
            return;
        }
        for it in page.items {
            let img = thumb(&it.thumb);
            self.tiles.push(GalleryTile {
                index: st.items.len() as i32,
                has_thumb: img.is_some(),
                thumb: img.unwrap_or_default(),
                name: it.name.clone().into(),
                size_text: human(it.size).into(),
                duration: duration(it.duration_ms).into(),
                selected: false,
                video: it.mime.starts_with("video/"),
                mime: it.mime.clone().into(),
                date_text: taken(it.taken_ms).into(),
            });
            st.items.push(it);
            st.selected.push(false);
        }
        st.next = page.next_cursor;
        let more = page.next_cursor != 0;
        self.global(|g| g.set_more(more));
    }

    fn set_selected(&self, i: usize, on: bool) {
        let mut st = self.st.borrow_mut();
        if let Some(s) = st.selected.get_mut(i) {
            *s = on;
        }
        if let Some(mut t) = self.tiles.row_data(i) {
            t.selected = on;
            self.tiles.set_row_data(i, t);
        }
    }
}

/// Pages arrive on core threads.
struct Events;

impl GalleryEvents for Events {
    fn page(&self, peer: DeviceId, page: &GalleryPage) {
        let page = page.clone();
        let _ = slint::invoke_from_event_loop(move || with_ui(|ui| ui.page(peer, page)));
    }
}

impl Feature for Gallery {
    fn id(&self) -> &'static str {
        "gallery"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Gallery,
            key: "gallery.v1",
        }]
    }

    fn has_view(&self) -> bool {
        true
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(svc) = ctx.node.service::<GalleryService>() else {
            return;
        };
        svc.events.set(Arc::new(Events));
        let tiles = Rc::new(VecModel::default());
        let ui = Rc::new(Ui {
            svc,
            window: ctx.window.clone(),
            tiles: tiles.clone(),
            st: RefCell::default(),
        });
        UI.with(|u| *u.borrow_mut() = Some(ui));
        let g = w.global::<GalleryFeature>();
        g.set_items(ModelRc::from(tiles));
        g.on_reload(|| {
            with_ui(|ui| {
                ui.reset();
                ui.load(0);
            });
        });
        g.on_load_more(|| {
            with_ui(|ui| {
                let next = ui.st.borrow().next;
                if next != 0 {
                    ui.load(next);
                }
            });
        });
        g.on_toggle(|i| {
            with_ui(|ui| {
                let i = i as usize;
                let on = !ui.st.borrow().selected.get(i).copied().unwrap_or(true);
                ui.set_selected(i, on);
                ui.count();
            });
        });
        g.on_select_all(|on| {
            with_ui(|ui| {
                let n = ui.st.borrow().items.len();
                for i in 0..n {
                    ui.set_selected(i, on);
                }
                ui.count();
            });
        });
        g.on_download(|| {
            with_ui(|ui| {
                let (peer, picked) = {
                    let st = ui.st.borrow();
                    let picked: Vec<GalleryItem> = st
                        .items
                        .iter()
                        .zip(&st.selected)
                        .filter(|(_, s)| **s)
                        .map(|(it, _)| it.clone())
                        .collect();
                    (st.peer, picked)
                };
                let Some(peer) = peer else { return };
                let n = picked.len();
                match ui.svc.download(peer, picked) {
                    Ok(_) => ui.global(|g| {
                        g.set_queued(n as i32);
                        g.invoke_select_all(false);
                    }),
                    Err(e) => tracing::warn!(error = %e, "gallery download not started"),
                }
            });
        });
        g.on_download_one(|i| {
            with_ui(|ui| {
                let (peer, item) = {
                    let st = ui.st.borrow();
                    (
                        st.peer,
                        usize::try_from(i)
                            .ok()
                            .and_then(|i| st.items.get(i))
                            .cloned(),
                    )
                };
                let (Some(peer), Some(item)) = (peer, item) else {
                    return;
                };
                match ui.svc.download(peer, vec![item]) {
                    Ok(_) => ui.global(|g| g.set_queued(1)),
                    Err(e) => tracing::warn!(error = %e, "gallery download not started"),
                }
            });
        });
        g.on_open_folder(|| {
            let host = DesktopFiles::new(&urdevice_platform::data_dir());
            open_folder(&host.dir());
        });
    }

    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        w.global::<GalleryFeature>()
            .set_peer_id(peer.id.clone().into());
        let Some(id) = DeviceId::from_hex(&peer.id) else {
            return;
        };
        with_ui(|ui| {
            let same = ui.st.borrow().peer == Some(id);
            if same && !ui.st.borrow().items.is_empty() {
                return;
            }
            if !same && let Some(old) = ui.st.borrow().peer {
                ui.svc.close(old);
            }
            ui.st.borrow_mut().peer = Some(id);
            ui.reset();
            ui.load(0);
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn durations_read_well() {
        assert_eq!(super::duration(0), "");
        assert_eq!(super::duration(65_000), "1:05");
        assert_eq!(super::duration(3_725_000), "1:02:05");
    }

    #[test]
    fn dates_are_empty_when_unknown() {
        assert_eq!(super::taken(0), "");
        assert_eq!(super::taken(-5), "");
        assert!(!super::taken(1_700_000_000_000).is_empty());
    }

    #[test]
    fn bad_thumbs_are_skipped() {
        assert!(super::thumb(b"").is_none());
        assert!(super::thumb(b"not a jpeg").is_none());
    }
}
