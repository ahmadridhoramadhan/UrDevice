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
use urdevice_core::services::gallery::{Filter, Gallery as GalleryService, GalleryEvents, Query};
use urdevice_platform::features::files::{DesktopFiles, open_folder};
use urdevice_proto::features::gallery::{GalleryAlbum, GalleryAlbums, GalleryItem, GalleryPage};

use super::files::human;
use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{GalleryFeature, GalleryTile, MainWindow};

pub(crate) struct Gallery;

/// Largest thumbnail side decoded.
const MAX_THUMB_SIDE: usize = 512;
/// Largest side of a large preview decoded.
const MAX_PREVIEW_SIDE: usize = 2048;
/// Large previews kept, newest last.
const PREVIEW_CACHE: usize = 6;

/// Decode a JPEG from the phone. Untrusted: size-limited, and a bad one is
/// just not shown.
fn decode(jpeg: &[u8], max_side: usize) -> Option<SharedPixelBuffer<Rgb8Pixel>> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    if jpeg.is_empty() {
        return None;
    }
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(max_side)
        .set_max_height(max_side);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(jpeg), options);
    let rgb = d.decode().ok()?;
    let (w, h) = d.dimensions()?;
    if rgb.len() != w.checked_mul(h)?.checked_mul(3)? {
        return None;
    }
    Some(SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(
        &rgb,
        u32::try_from(w).ok()?,
        u32::try_from(h).ok()?,
    ))
}

fn thumb(jpeg: &[u8]) -> Option<Image> {
    decode(jpeg, MAX_THUMB_SIDE).map(Image::from_rgb8)
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
    /// 0 all, 1 photos, 2 videos.
    kind: u32,
    album: Option<i64>,
    albums: Vec<GalleryAlbum>,
    /// The album list is asked for once per device, after the first page (so the phone
    /// is asked for approval only once).
    albums_asked: bool,
    /// Counts the lists asked for; a page for an older one is dropped.
    generation: u64,
    /// The item whose large preview is wanted, and the one being fetched.
    wanted: Option<u64>,
    inflight: Option<u64>,
    /// Large previews, newest last.
    previews: Vec<(u64, Image)>,
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
        let (peer, query) = {
            let st = self.st.borrow();
            let Some(peer) = st.peer else {
                return;
            };
            let filter = Filter {
                kind: st.kind,
                album: st.album,
            };
            (
                peer,
                Query {
                    filter,
                    tag: st.generation,
                },
            )
        };
        self.global(|g| {
            g.set_loading(true);
            g.set_error("".into());
        });
        if self.svc.browse_with(peer, cursor, query).is_err() {
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
            st.generation += 1;
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
        // An answer to a list that is no longer shown. Refusals carry no tag.
        if page.tag != st.generation && !(page.tag == 0 && !page.error.is_empty()) {
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
        if !st.albums_asked {
            st.albums_asked = true;
            let _ = self.svc.albums(peer);
        }
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

impl Ui {
    /// Show another set of pictures: start again from the newest.
    fn refilter(&self) {
        self.reset();
        self.load(0);
    }

    fn albums(&self, peer: DeviceId, a: GalleryAlbums) {
        {
            let mut st = self.st.borrow_mut();
            if st.peer != Some(peer) || !a.error.is_empty() {
                return;
            }
            st.albums = a.albums;
        }
        let st = self.st.borrow();
        let mut names = vec![slint::SharedString::from("All albums")];
        names.extend(
            st.albums
                .iter()
                .map(|al| slint::SharedString::from(format!("{} ({})", al.name, al.count))),
        );
        let index = st
            .album
            .and_then(|id| st.albums.iter().position(|al| al.id == id))
            .map_or(0, |i| i + 1);
        drop(st);
        self.global(|g| {
            g.set_album_names(ModelRc::new(VecModel::from(names)));
            g.set_album_index(index as i32);
        });
    }

    /// The sharper picture of item `index` is wanted. One is fetched at a time; the newest
    /// wish wins.
    fn want_preview(&self, index: usize) {
        let (id, cached) = {
            let mut st = self.st.borrow_mut();
            let Some(id) = st.items.get(index).map(|i| i.id) else {
                return;
            };
            st.wanted = Some(id);
            let cached = st
                .previews
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, p)| p.clone());
            (id, cached)
        };
        match cached {
            Some(img) => self.global(|g| {
                g.set_preview(img);
                g.set_preview_index(index as i32);
                g.set_preview_state(1);
            }),
            None => {
                self.global(|g| {
                    g.set_preview_index(index as i32);
                    g.set_preview_state(0);
                });
                self.fetch_wanted(id);
            }
        }
    }

    fn fetch_wanted(&self, id: u64) {
        let peer = {
            let mut st = self.st.borrow_mut();
            if st.inflight.is_some() {
                return;
            }
            let Some(peer) = st.peer else { return };
            st.inflight = Some(id);
            peer
        };
        if self.svc.preview(peer, id).is_err() {
            self.preview_done(peer, id, None);
        }
    }

    fn preview_done(&self, peer: DeviceId, id: u64, buf: Option<SharedPixelBuffer<Rgb8Pixel>>) {
        let next = {
            let mut st = self.st.borrow_mut();
            if st.peer != Some(peer) {
                return;
            }
            if st.inflight == Some(id) {
                st.inflight = None;
            }
            let img = buf.map(Image::from_rgb8);
            if let Some(img) = &img {
                st.previews.retain(|(i, _)| *i != id);
                st.previews.push((id, img.clone()));
                if st.previews.len() > PREVIEW_CACHE {
                    st.previews.remove(0);
                }
            }
            let index = st.items.iter().position(|i| i.id == id);
            if st.wanted == Some(id)
                && let Some(index) = index
            {
                let ok = img.is_some();
                drop(st);
                self.global(|g| {
                    if let Some(img) = img {
                        g.set_preview(img);
                    }
                    g.set_preview_index(index as i32);
                    g.set_preview_state(if ok { 1 } else { 2 });
                });
                None
            } else {
                // The user moved on: fetch what they look at now, unless it is already here.
                st.wanted
                    .filter(|w| !st.previews.iter().any(|(i, _)| i == w))
            }
        };
        if let Some(w) = next {
            self.fetch_wanted(w);
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

    fn albums(&self, peer: DeviceId, albums: &GalleryAlbums) {
        let albums = albums.clone();
        let _ = slint::invoke_from_event_loop(move || with_ui(|ui| ui.albums(peer, albums)));
    }

    fn preview(&self, peer: DeviceId, id: u64, jpeg: Option<Vec<u8>>) {
        // Decode here, off the window thread; only the finished pixels cross over.
        let buf = jpeg.and_then(|j| decode(&j, MAX_PREVIEW_SIDE));
        let _ = slint::invoke_from_event_loop(move || {
            with_ui(|ui| ui.preview_done(peer, id, buf));
        });
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
        g.on_set_kind(|k| {
            with_ui(|ui| {
                let k = u32::try_from(k).unwrap_or(0).min(2);
                if ui.st.borrow().kind == k {
                    return;
                }
                ui.st.borrow_mut().kind = k;
                ui.global(|g| g.set_kind(k as i32));
                ui.refilter();
            });
        });
        g.on_pick_album(|i| {
            with_ui(|ui| {
                let album = usize::try_from(i)
                    .ok()
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| ui.st.borrow().albums.get(i).map(|a| a.id));
                if ui.st.borrow().album == album {
                    return;
                }
                ui.st.borrow_mut().album = album;
                ui.refilter();
            });
        });
        g.on_want_preview(|i| {
            with_ui(|ui| {
                if let Ok(i) = usize::try_from(i) {
                    ui.want_preview(i);
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
            {
                let mut st = ui.st.borrow_mut();
                st.peer = Some(id);
                st.kind = 0;
                st.album = None;
                st.albums.clear();
                st.albums_asked = false;
                st.previews.clear();
                st.wanted = None;
                st.inflight = None;
            }
            ui.global(|g| {
                g.set_kind(0);
                g.set_album_index(0);
                g.set_album_names(ModelRc::new(VecModel::from(vec![
                    slint::SharedString::from("All albums"),
                ])));
                g.set_preview_index(-1);
            });
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
