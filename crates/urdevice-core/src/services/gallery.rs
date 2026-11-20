//! Android gallery (#17, design §9.2, R8.7).
//!
//! Service id `ServiceId::GALLERY`, capability `gallery.v1`, offered by
//! Android hosts only. A desktop (the controller) asks for one activity per
//! browsing session, pages through the phone's photos and videos with
//! 256-px thumbnails, and downloads selected items. Downloads reuse the file
//! transfer machinery ([`super::files::engine`]): part files, SHA-256, resume
//! sidecars, and the transfer list.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::watch;
use urdevice_proto::features::gallery::{
    FetchItem, GalleryFetch, GalleryItem, GalleryList, GalleryMsg, GalleryPage, STREAM_FETCH,
    gallery_msg,
};

use super::files::{Files, engine};
use super::{Access, Direction, Message, Registry, Service, ServiceCtx, ServiceId, Slot, Stream};
use crate::identity::DeviceId;
use crate::permission::Capability;
use crate::{CoreError, Result};

/// Release 2 capabilities this module ships. Release 1 capabilities are
/// always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Most items in one page.
pub const MAX_PAGE: u32 = 60;
/// Most items in one download stream.
pub const MAX_FETCH: usize = 200;
/// Largest thumbnail sent; bigger ones are left out.
pub const MAX_THUMB: usize = 64 * 1024;
/// Encoded page budget, under the 1 MiB control limit.
const PAGE_BUDGET: usize = 900 * 1024;
/// Bit set in a gallery item's resume id, so it never matches a transfer id
/// of the same sender by accident.
const RESUME_BIT: u64 = 1 << 63;

/// One page from the platform.
#[derive(Debug, Clone, Default)]
pub struct HostPage {
    /// Items, newest first.
    pub items: Vec<GalleryItem>,
    /// True when there are more after these.
    pub more: bool,
}

/// What the phone provides (Kotlin through the FFI: MediaStore).
pub trait GalleryHost: Send + Sync {
    /// Items `cursor..cursor + limit`, newest first. Return
    /// `CoreError::Refused("permission")` when the app may not read media.
    fn page(&self, cursor: u64, limit: u32) -> Result<HostPage>;
    /// Open item `id` for reading; returns the file and its size.
    fn open(&self, id: u64) -> Result<(File, u64)>;
}

/// What the browsing UI listens to. Called on core threads.
pub trait GalleryEvents: Send + Sync {
    /// A page arrived from `peer`, or an error (`page.error`): `permission`,
    /// `failed`, `busy`, or a refusal reason (`denied`, `declined`,
    /// `timeout`, `offline`, ...).
    fn page(&self, peer: DeviceId, page: &GalleryPage);
}

/// The gallery service. The UI reaches it with `node.service::<Gallery>()`.
#[derive(Default)]
pub struct Gallery {
    ctx: OnceLock<ServiceCtx>,
    files: OnceLock<Weak<Files>>,
    /// The phone's MediaStore adapter.
    pub host: Slot<dyn GalleryHost>,
    /// The browsing UI.
    pub events: Slot<dyn GalleryEvents>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Controller: the activity of the open browsing session per host.
    sessions: HashMap<DeviceId, u64>,
    /// Host: peers with a page being built.
    listing: HashSet<DeviceId>,
    /// Host: running downloads per activity, stopped when it ends.
    fetches: HashMap<(DeviceId, u64), watch::Sender<bool>>,
}

impl std::fmt::Debug for Gallery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gallery").finish_non_exhaustive()
    }
}

fn lock(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn error_page(cursor: u64, reason: &str) -> GalleryPage {
    GalleryPage {
        cursor,
        items: Vec::new(),
        next_cursor: 0,
        error: reason.into(),
    }
}

/// The activity of the browsing session with `peer`, asking for one if
/// there is none (one approval per session, design §7.4).
async fn session(ctx: &ServiceCtx, state: &Mutex<State>, peer: DeviceId) -> Result<u64> {
    if let Some(a) = lock(state).sessions.get(&peer).copied() {
        return Ok(a);
    }
    let a = ctx.request(peer).await?;
    lock(state).sessions.insert(peer, a);
    Ok(a)
}

/// Keep a host page under the control limit and the per-item limits.
fn bounded_page(cursor: u64, host: HostPage) -> GalleryPage {
    let mut page = GalleryPage {
        cursor,
        items: Vec::new(),
        next_cursor: 0,
        error: String::new(),
    };
    let mut used = 0usize;
    let total = host.items.len();
    for mut it in host.items {
        if it.thumb.len() > MAX_THUMB {
            it.thumb.clear();
        }
        truncate(&mut it.name, 255);
        truncate(&mut it.mime, 100);
        let len = prost::Message::encoded_len(&it) + 8;
        if used + len > PAGE_BUDGET {
            break;
        }
        used += len;
        page.items.push(it);
    }
    let taken = page.items.len();
    if host.more || taken < total {
        page.next_cursor = cursor + taken.max(1) as u64;
    }
    page
}

fn truncate(s: &mut String, max: usize) {
    if s.len() > max {
        let mut k = max;
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        s.truncate(k);
    }
}

impl Gallery {
    fn ctx(&self) -> Result<&ServiceCtx> {
        self.ctx.get().ok_or(CoreError::Stopped)
    }

    fn emit(&self, peer: DeviceId, page: &GalleryPage) {
        if let Some(e) = self.events.get() {
            e.page(peer, page);
        }
    }

    /// Ask `peer` for the page at `cursor` (0 for the newest). The answer
    /// arrives through [`GalleryEvents::page`].
    pub fn browse(&self, peer: DeviceId, cursor: u64) -> Result<()> {
        let ctx = self.ctx()?.clone();
        let state = self.state.clone();
        let events = self.events.get();
        ctx.clone().spawn(async move {
            let report = |reason: &str| {
                if let Some(e) = &events {
                    e.page(peer, &error_page(cursor, reason));
                }
            };
            let activity = match session(&ctx, &state, peer).await {
                Ok(a) => a,
                Err(CoreError::Refused(r)) => return report(&r),
                Err(_) => return report("failed"),
            };
            let msg = GalleryMsg::from(gallery_msg::Msg::List(GalleryList {
                cursor,
                limit: MAX_PAGE,
            }));
            if ctx.send(peer, Direction::ToHost, activity, &msg).is_err() {
                report("offline");
            }
        });
        Ok(())
    }

    /// Download items from `peer` into the download folder. Progress shows
    /// in the Files transfer list. Returns the transfer handles (one per
    /// [`MAX_FETCH`] items).
    pub fn download(&self, peer: DeviceId, items: Vec<GalleryItem>) -> Result<Vec<u64>> {
        let ctx = self.ctx()?.clone();
        let files = self
            .files
            .get()
            .and_then(Weak::upgrade)
            .ok_or(CoreError::Stopped)?;
        let mut handles = Vec::new();
        for batch in items.chunks(MAX_FETCH) {
            let entries: Vec<(String, u64, u64)> = batch
                .iter()
                .map(|i| {
                    let fallback = format!("item_{}", i.id);
                    let name = super::files::path::file_name_only(&i.name, &fallback);
                    (name, i.size, i.id | RESUME_BIT)
                })
                .collect();
            let wanted: Vec<(u64, u64)> = batch.iter().map(|i| (i.id, i.size)).collect();
            let ctx2 = ctx.clone();
            let state = self.state.clone();
            let open: super::files::OpenDownload = Box::new(move |starts: Vec<u64>| {
                Box::pin(async move {
                    let activity = session(&ctx2, &state, peer).await?;
                    let meta = GalleryFetch {
                        items: wanted
                            .iter()
                            .zip(starts)
                            .map(|((id, size), offset)| FetchItem {
                                id: *id,
                                offset,
                                size: *size,
                            })
                            .collect(),
                    };
                    let meta = prost::Message::encode_to_vec(&meta);
                    ctx2.open_bi(peer, Direction::ToHost, activity, STREAM_FETCH, meta)
                        .await
                })
            });
            handles.push(files.start_download(peer, entries, open)?);
        }
        Ok(handles)
    }

    /// End the browsing session with `peer`.
    pub fn close(&self, peer: DeviceId) {
        let a = lock(&self.state).sessions.remove(&peer);
        if let (Some(a), Ok(ctx)) = (a, self.ctx()) {
            ctx.stop(peer, a);
        }
    }

    fn on_list(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64, l: GalleryList) {
        let reply = move |ctx: &ServiceCtx, page: GalleryPage| {
            let msg = GalleryMsg::from(gallery_msg::Msg::Page(page));
            let _ = ctx.send(peer, Direction::ToController, activity, &msg);
        };
        let Some(host) = self.host.get() else {
            return reply(ctx, error_page(l.cursor, "failed"));
        };
        if !lock(&self.state).listing.insert(peer) {
            return reply(ctx, error_page(l.cursor, "busy"));
        }
        let state = self.state.clone();
        let ctx2 = ctx.clone();
        ctx.spawn(async move {
            let cursor = l.cursor;
            let limit = l.limit.clamp(1, MAX_PAGE);
            let r = tokio::task::spawn_blocking(move || host.page(cursor, limit)).await;
            lock(&state).listing.remove(&peer);
            let page = match r {
                Ok(Ok(p)) => bounded_page(cursor, p),
                Ok(Err(CoreError::Refused(r))) if r == "permission" => {
                    error_page(cursor, "permission")
                }
                _ => error_page(cursor, "failed"),
            };
            reply(&ctx2, page);
        });
    }

    fn on_fetch(&self, ctx: &ServiceCtx, activity: u64, stream: Stream) {
        let Stream {
            peer,
            meta,
            send,
            mut recv,
            ..
        } = stream;
        let Some(mut send) = send else { return };
        let refuse = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream| {
            let _ = send.reset(super::stream_code::REFUSED.into());
            let _ = recv.stop(super::stream_code::REFUSED.into());
        };
        let fetch: GalleryFetch = match prost::Message::decode(meta.as_slice()) {
            Ok(f) => f,
            Err(_) => return refuse(&mut send, &mut recv),
        };
        let Some(host) = self.host.get() else {
            return refuse(&mut send, &mut recv);
        };
        if fetch.items.is_empty() || fetch.items.len() > MAX_FETCH {
            return refuse(&mut send, &mut recv);
        }
        let mut stop = {
            let mut st = lock(&self.state);
            st.fetches
                .entry((peer, activity))
                .or_insert_with(|| watch::channel(false).0)
                .subscribe()
        };
        ctx.spawn(async move {
            let items = fetch.items;
            let h = host.clone();
            let opened = tokio::task::spawn_blocking(move || {
                items
                    .iter()
                    .map(|it| {
                        let id = it.id;
                        let actual = h.open(id).ok().map(|(_, size)| size);
                        let h2 = h.clone();
                        let open: engine::Opener = if actual == Some(it.size) {
                            Arc::new(move || {
                                h2.open(id).map(|(f, _)| f).map_err(|e| {
                                    std::io::Error::other(e.to_string())
                                })
                            })
                        } else {
                            // Gone or changed since it was listed: this item fails.
                            Arc::new(|| Err(std::io::Error::other("item changed")))
                        };
                        (
                            engine::Source {
                                rel_path: String::new(),
                                size: it.size,
                                open,
                            },
                            it.offset.min(it.size),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .await;
            let Ok(opened) = opened else { return };
            let (sources, starts): (Vec<_>, Vec<_>) = opened.into_iter().unzip();
            let meter = engine::Meter::default();
            tokio::select! {
                r = engine::send(&mut send, &mut recv, &sources, &starts, &meter) => {
                    if let Err(e) = r {
                        tracing::debug!(peer = %peer.short_hex(), kind = ?e.kind(), "gallery download stopped");
                    }
                }
                _ = stop.wait_for(|s| *s) => {
                    let _ = send.reset(super::stream_code::CANCELLED.into());
                    let _ = recv.stop(super::stream_code::CANCELLED.into());
                }
            }
        });
    }
}

impl Service for Gallery {
    fn id(&self) -> ServiceId {
        ServiceId::GALLERY
    }

    fn capability(&self) -> Capability {
        Capability::Gallery
    }

    fn start(&self, ctx: ServiceCtx) {
        let _ = self.ctx.set(ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        let m: GalleryMsg = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad GalleryMsg".into()))?;
        match (msg.access, m.msg) {
            (Access::Host(grant), Some(gallery_msg::Msg::List(l))) => {
                self.on_list(ctx, msg.peer, grant.activity(), l);
            }
            (Access::Controller { .. }, Some(gallery_msg::Msg::Page(mut p))) => {
                // Shown only; still keep it to the limits.
                p.items.retain(|i| i.thumb.len() <= MAX_THUMB);
                p.items.truncate(MAX_PAGE as usize);
                truncate(&mut p.error, 16);
                self.emit(msg.peer, &p);
            }
            (_, None) => return Err(CoreError::Protocol("empty GalleryMsg".into())),
            _ => {}
        }
        Ok(())
    }

    fn on_stream(&self, ctx: &ServiceCtx, mut stream: Stream) {
        match (&stream.access, stream.kind) {
            (Access::Host(grant), STREAM_FETCH) => {
                let a = grant.activity();
                self.on_fetch(ctx, a, stream);
            }
            _ => {
                if let Some(s) = stream.send.as_mut() {
                    let _ = s.reset(super::stream_code::REFUSED.into());
                }
                let _ = stream.recv.stop(super::stream_code::REFUSED.into());
            }
        }
    }

    fn on_refused(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        let had = {
            let mut st = lock(&self.state);
            let had = st.sessions.get(&peer) == Some(&activity);
            if had {
                st.sessions.remove(&peer);
            }
            had
        };
        let _ = ctx;
        if had || activity == 0 {
            self.emit(peer, &error_page(0, reason));
        }
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        let mut st = lock(&self.state);
        if st.sessions.get(&peer) == Some(&activity) {
            st.sessions.remove(&peer);
        }
        if let Some(tx) = st.fetches.remove(&(peer, activity)) {
            tx.send_replace(true);
        }
    }

    fn on_peer(&self, _ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if !connected {
            let mut st = lock(&self.state);
            st.sessions.remove(&peer);
            st.listing.remove(&peer);
            st.fetches.retain(|(p, _), tx| {
                if *p == peer {
                    tx.send_replace(true);
                }
                *p != peer
            });
        }
    }
}

/// Add this feature's services to a starting node. `platform` is `linux`,
/// `windows`, `macos`, or `android`. Every platform registers it: desktops
/// browse, Android hosts also serve (the node only accepts host traffic on
/// Android, because only Android offers `gallery.v1`).
pub(crate) fn register(r: &mut Registry, _platform: &str) {
    let g = Arc::new(Gallery::default());
    if let Some(files) = r.typed::<Files>() {
        let _ = g.files.set(Arc::downgrade(&files));
    }
    r.add(g);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn item(id: u64, thumb: usize) -> GalleryItem {
        GalleryItem {
            id,
            name: "x".repeat(300),
            mime: "image/jpeg".into(),
            size: 1,
            taken_ms: 0,
            duration_ms: 0,
            thumb: vec![0; thumb],
        }
    }

    #[test]
    fn pages_stay_under_the_control_limit() {
        let host = HostPage {
            items: (0..60).map(|i| item(i, MAX_THUMB)).collect(),
            more: false,
        };
        let p = bounded_page(10, host);
        let len = prost::Message::encoded_len(&p);
        assert!(len < urdevice_proto::MAX_FRAME, "{len}");
        assert!(p.items.len() < 60);
        assert_eq!(p.next_cursor, 10 + p.items.len() as u64);
        assert!(p.items.iter().all(|i| i.name.len() <= 255));
    }

    #[test]
    fn big_thumbs_are_dropped_and_the_end_has_no_cursor() {
        let host = HostPage {
            items: vec![item(1, MAX_THUMB + 1), item(2, 10)],
            more: false,
        };
        let p = bounded_page(0, host);
        assert_eq!(p.items.len(), 2);
        assert!(p.items[0].thumb.is_empty());
        assert_eq!(p.next_cursor, 0);
    }
}
