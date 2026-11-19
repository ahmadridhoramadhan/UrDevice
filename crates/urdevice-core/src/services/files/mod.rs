//! File transfer (#17, design §9.2, R8).
//!
//! Service id `ServiceId::FILES`, capability `files.v1`. The sender is the
//! controller and the receiver the host: the receiver's Files setting
//! decides (Allow, Always Ask, Deny), and one approval covers one transfer.
//!
//! 1. The sender asks for an activity ([`ServiceCtx::request`]) and sends an
//!    [`Offer`] with the file list.
//! 2. The receiver cleans every name ([`path`]), creates the part files, and
//!    answers [`Accept`] with resume points, or [`Decline`].
//! 3. The sender opens one data stream and sends the bytes ([`wire`],
//!    [`engine`]). The receiver hashes while writing and renames a file only
//!    after its SHA-256 matched (R8.4).
//! 4. If the session drops, both keep their state; when the peer is back the
//!    sender offers the remaining files again under the same transfer id and
//!    the receiver resumes from its sidecars (R8.5).
//!
//! Either side can cancel; a [`Cancel`] deletes the part files.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, watch};
use urdevice_proto::features::files::{
    Accept, Cancel, Decline, FileEntry, FilesMsg, Offer, ResumeAt, STREAM_DATA, files_msg,
};

use super::{Access, Direction, Message, Registry, Service, ServiceCtx, ServiceId, Slot, Stream};
use crate::identity::DeviceId;
use crate::permission::Capability;
use crate::{CoreError, Result};

pub mod engine;
pub mod path;
pub mod wire;

pub use engine::{Opener, Source};

/// Release 2 capabilities this module ships. Release 1 capabilities are
/// always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Most files in one transfer.
pub const MAX_FILES: usize = 10_000;
/// Largest single file accepted (1 TiB).
pub const MAX_FILE_SIZE: u64 = 1 << 40;
/// Most incoming transfers running at once.
const MAX_INCOMING: usize = 8;
/// How long the sender waits for Accept (the receiver creates the folders).
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the receiver waits for the data stream after Accept.
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);
/// Automatic resume attempts after reconnects.
const MAX_RESUMES: u32 = 20;
/// Progress events at most this often (design §9.2: about every 100 ms).
const TICK: Duration = Duration::from_millis(100);

/// What the platform provides for receiving (desktop: urdevice-platform;
/// Android: Kotlin through the FFI).
pub trait FilesHost: Send + Sync {
    /// The download folder (R8.6). Created if missing.
    fn incoming_dir(&self) -> PathBuf;

    /// A file was verified and saved at `path`, in the cleaned folder
    /// `rel_dir` of the download folder (`/` between names, empty for the
    /// folder itself). Android moves it to MediaStore Downloads here and
    /// returns where it is now; desktops leave it and return `None`.
    fn saved(&self, path: &Path, rel_dir: &str) -> Option<String> {
        let _ = (path, rel_dir);
        None
    }
}

/// What the UI listens to. Called on core threads.
pub trait FilesEvents: Send + Sync {
    /// Something in [`Files::transfers`] changed. At most every 100 ms while
    /// bytes move.
    fn changed(&self);

    /// A peer started sending files to this device (R8.2).
    fn incoming(&self, transfer: &TransferView) {
        let _ = transfer;
    }
}

/// Where a transfer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TransferState {
    /// Waiting for the receiver's approval.
    Asking,
    /// Bytes are moving.
    Running,
    /// The connection dropped; it resumes when the device is back (R8.5).
    Interrupted,
    /// Every file was verified and saved.
    Done,
    /// Some files failed; see [`TransferView::failed`] and `reason`.
    Failed,
    /// Cancelled on either device.
    Cancelled,
    /// The receiver refused it (Deny, a declined prompt, or a bad offer).
    Declined,
}

impl TransferState {
    /// True once nothing more happens.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Declined
        )
    }
}

/// One transfer, for the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct TransferView {
    /// Local handle for [`Files::cancel`] and [`Files::retry`].
    pub id: u64,
    /// The other device.
    pub peer: DeviceId,
    /// Its name.
    pub peer_name: String,
    /// True when this device sends.
    pub outgoing: bool,
    /// File names (relative paths), in order. Shown only on this device.
    pub names: Vec<String>,
    /// Sum of the file sizes.
    pub total: u64,
    /// Bytes acknowledged so far.
    pub done: u64,
    /// Bytes per second over the last few seconds.
    pub speed: u64,
    /// Seconds left at the current speed.
    pub eta_secs: Option<u64>,
    /// Where it is.
    pub state: TransferState,
    /// Why it was declined or failed, or who cancelled: `denied`,
    /// `declined`, `timeout`, `busy`, `invalid`, `storage`, `failed`,
    /// `peer` (cancelled on the other device), or empty.
    pub reason: String,
    /// Where received files were saved.
    pub saved: Vec<String>,
    /// Files that failed their check (R8.4).
    pub failed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Run,
    Interrupt,
    Cancel,
}

type Opened = (quinn::SendStream, quinn::RecvStream);
/// Opens the stream of a gallery download once the resume offsets are
/// known. Used by the gallery service.
pub(crate) type OpenDownload = Box<
    dyn FnOnce(Vec<u64>) -> std::pin::Pin<Box<dyn Future<Output = Result<Opened>> + Send>> + Send,
>;

struct Transfer {
    handle: u64,
    id: u64,
    peer: DeviceId,
    peer_name: String,
    outgoing: bool,
    activity: u64,
    names: Vec<String>,
    total: u64,
    meter: Arc<engine::Meter>,
    state: TransferState,
    reason: String,
    signal: watch::Sender<Signal>,
    samples: VecDeque<(Instant, u64)>,
    /// Outgoing: what to send and what the receiver verified.
    sources: Vec<Source>,
    verified: Vec<bool>,
    resumes: u32,
    /// Incoming: the plan, for deleting part files on a late Cancel.
    targets: Vec<engine::Target>,
    stream_tx: Option<oneshot::Sender<Stream>>,
    /// Incoming: set before a cancel, so the writer deletes its part files.
    discard: Arc<AtomicBool>,
    saved: Vec<String>,
    failed: u32,
    /// Held by the task that runs the transfer.
    running: Arc<tokio::sync::Mutex<()>>,
}

impl Transfer {
    fn view(&self) -> TransferView {
        let speed = match (self.samples.front(), self.samples.back()) {
            (Some((t0, b0)), Some((t1, b1))) if t1 > t0 => {
                let dt = t1.duration_since(*t0).as_secs_f64();
                (b1.saturating_sub(*b0) as f64 / dt) as u64
            }
            _ => 0,
        };
        let done = self.meter.get().min(self.total);
        let eta_secs = (self.state == TransferState::Running && speed > 0)
            .then(|| self.total.saturating_sub(done).div_ceil(speed));
        TransferView {
            id: self.handle,
            peer: self.peer,
            peer_name: self.peer_name.clone(),
            outgoing: self.outgoing,
            names: self.names.clone(),
            total: self.total,
            done: if self.state == TransferState::Done {
                self.total
            } else {
                done
            },
            speed: if self.state == TransferState::Running {
                speed
            } else {
                0
            },
            eta_secs,
            state: self.state,
            reason: self.reason.clone(),
            saved: self.saved.clone(),
            failed: self.failed,
        }
    }
}

#[derive(Default)]
struct Table {
    list: Vec<Transfer>,
    accepts: HashMap<(DeviceId, u64), oneshot::Sender<std::result::Result<Accept, String>>>,
}

#[derive(Default)]
struct Shared {
    host: Slot<dyn FilesHost>,
    events: Slot<dyn FilesEvents>,
    table: Mutex<Table>,
    next: AtomicU64,
}

impl Shared {
    fn table(&self) -> std::sync::MutexGuard<'_, Table> {
        match self.table.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn changed(&self) {
        if let Some(e) = self.events.get() {
            e.changed();
        }
    }

    fn with<R>(&self, handle: u64, f: impl FnOnce(&mut Transfer) -> R) -> Option<R> {
        let mut t = self.table();
        t.list.iter_mut().find(|x| x.handle == handle).map(f)
    }

    fn set_state(&self, handle: u64, state: TransferState, reason: &str) {
        self.with(handle, |t| {
            t.state = state;
            t.reason = reason.to_owned();
            t.samples.clear();
        });
        self.changed();
    }

    fn new_transfer(&self, peer: DeviceId, peer_name: String, outgoing: bool, id: u64) -> Transfer {
        Transfer {
            handle: self.next.fetch_add(1, Ordering::Relaxed) + 1,
            id,
            peer,
            peer_name,
            outgoing,
            activity: 0,
            names: Vec::new(),
            total: 0,
            meter: Arc::default(),
            state: TransferState::Asking,
            reason: String::new(),
            signal: watch::channel(Signal::Run).0,
            samples: VecDeque::new(),
            sources: Vec::new(),
            verified: Vec::new(),
            resumes: 0,
            targets: Vec::new(),
            stream_tx: None,
            discard: Arc::default(),
            saved: Vec::new(),
            failed: 0,
            running: Arc::default(),
        }
    }

    /// Record progress samples of running transfers. True if any moved.
    fn tick(&self) -> bool {
        let now = Instant::now();
        let mut moved = false;
        for t in self.table().list.iter_mut() {
            if t.state != TransferState::Running {
                continue;
            }
            let b = t.meter.get();
            if t.samples.back().is_none_or(|(_, last)| *last != b) {
                moved = true;
            }
            t.samples.push_back((now, b));
            while t
                .samples
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(3))
            {
                t.samples.pop_front();
            }
        }
        moved
    }
}

/// Wait until the signal leaves `Run`.
async fn signalled(rx: &mut watch::Receiver<Signal>) -> Signal {
    match rx.wait_for(|s| *s != Signal::Run).await {
        Ok(s) => *s,
        Err(_) => Signal::Interrupt,
    }
}

fn send_msg(ctx: &ServiceCtx, peer: DeviceId, dir: Direction, activity: u64, m: files_msg::Msg) {
    if let Err(e) = ctx.send(peer, dir, activity, &FilesMsg::from(m)) {
        tracing::debug!(peer = %peer.short_hex(), error = %e, "files message not sent");
    }
}

/// The file transfer service. The UI reaches it with
/// `node.service::<Files>()`.
#[derive(Default)]
pub struct Files {
    ctx: OnceLock<ServiceCtx>,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files").finish_non_exhaustive()
    }
}

impl Files {
    fn ctx(&self) -> Result<&ServiceCtx> {
        self.ctx.get().ok_or(CoreError::Stopped)
    }

    /// Set the platform adapter for receiving.
    pub fn set_host(&self, host: Arc<dyn FilesHost>) {
        self.shared.host.set(host);
    }

    /// Set the UI listener.
    pub fn set_events(&self, events: Arc<dyn FilesEvents>) {
        self.shared.events.set(events);
    }

    /// Every transfer since the app started, newest first.
    pub fn transfers(&self) -> Vec<TransferView> {
        let t = self.shared.table();
        t.list.iter().rev().map(Transfer::view).collect()
    }

    /// Send files to `peer` (R8.1). Returns the transfer's handle; progress
    /// shows in [`Files::transfers`].
    pub fn send(&self, peer: DeviceId, files: Vec<Source>) -> Result<u64> {
        let ctx = self.ctx()?.clone();
        if files.is_empty() {
            return Err(CoreError::InvalidInput("nothing to send".into()));
        }
        if files.len() > MAX_FILES {
            return Err(CoreError::InvalidInput(format!(
                "more than {MAX_FILES} files"
            )));
        }
        let id = crate::crypto::random_u64()?;
        let mut t = self
            .shared
            .new_transfer(peer, ctx.peer_name(peer), true, id);
        t.names = files.iter().map(|f| f.rel_path.clone()).collect();
        t.total = files.iter().map(|f| f.size).sum();
        t.verified = vec![false; files.len()];
        t.sources = files;
        let handle = t.handle;
        self.shared.table().list.push(t);
        self.shared.changed();
        ctx.spawn(run_outgoing(ctx.clone(), self.shared.clone(), handle));
        Ok(handle)
    }

    /// Send files and folders from the local disk (desktop file picker and
    /// drag-and-drop). Folders are sent with their contents; links are
    /// skipped.
    pub fn send_paths(&self, peer: DeviceId, paths: &[PathBuf]) -> Result<u64> {
        let files = sources_from_paths(paths)?;
        self.send(peer, files)
    }

    /// Cancel a transfer on both devices.
    pub fn cancel(&self, handle: u64) {
        let late = self.shared.with(handle, |t| {
            if t.state.is_final() {
                return None;
            }
            if matches!(t.state, TransferState::Interrupted) {
                // No task is running: tell the peer and clean up here.
                t.state = TransferState::Cancelled;
                return Some((
                    t.peer,
                    t.id,
                    t.activity,
                    t.outgoing,
                    std::mem::take(&mut t.targets),
                ));
            }
            t.discard.store(true, Ordering::Release);
            t.signal.send_replace(Signal::Cancel);
            None
        });
        if let Some(Some((peer, id, activity, outgoing, targets))) = late {
            engine::discard(&targets);
            if let Ok(ctx) = self.ctx() {
                let dir = if outgoing {
                    Direction::ToHost
                } else {
                    Direction::ToController
                };
                send_msg(
                    ctx,
                    peer,
                    dir,
                    activity,
                    files_msg::Msg::Cancel(Cancel { transfer_id: id }),
                );
            }
        }
        self.shared.changed();
    }

    /// Try an interrupted or failed outgoing transfer again, sending only
    /// the files that were not verified.
    pub fn retry(&self, handle: u64) -> Result<()> {
        let ctx = self.ctx()?.clone();
        let ok = self
            .shared
            .with(handle, |t| {
                t.outgoing
                    && matches!(t.state, TransferState::Interrupted | TransferState::Failed)
                    && t.verified.iter().any(|v| !v)
            })
            .unwrap_or(false);
        if !ok {
            return Err(CoreError::NotFound);
        }
        ctx.spawn(run_outgoing(ctx.clone(), self.shared.clone(), handle));
        Ok(())
    }

    /// Forget finished transfers.
    pub fn clear_finished(&self) {
        self.shared.table().list.retain(|t| !t.state.is_final());
        self.shared.changed();
    }

    /// Receive a gallery download from `peer` into the download folder,
    /// shown as an incoming transfer. `items` are (name, size, resume id).
    pub(crate) fn start_download(
        &self,
        peer: DeviceId,
        items: Vec<(String, u64, u64)>,
        open: OpenDownload,
    ) -> Result<u64> {
        let ctx = self.ctx()?.clone();
        let mut t = self
            .shared
            .new_transfer(peer, ctx.peer_name(peer), false, 0);
        t.names = items.iter().map(|(n, _, _)| n.clone()).collect();
        t.total = items.iter().map(|(_, s, _)| *s).sum();
        t.state = TransferState::Running;
        let handle = t.handle;
        self.shared.table().list.push(t);
        self.shared.changed();
        ctx.spawn(run_download(
            ctx.clone(),
            self.shared.clone(),
            handle,
            items,
            open,
        ));
        Ok(handle)
    }

    fn on_offer(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64, offer: Offer) {
        let decline = |reason: &str| {
            send_msg(
                ctx,
                peer,
                Direction::ToController,
                activity,
                files_msg::Msg::Decline(Decline {
                    transfer_id: offer.transfer_id,
                    reason: reason.into(),
                }),
            );
        };
        let total = offer
            .files
            .iter()
            .try_fold(0u64, |acc, f| acc.checked_add(f.size));
        let valid = !offer.files.is_empty()
            && offer.files.len() <= MAX_FILES
            && offer
                .files
                .iter()
                .all(|f| f.size <= MAX_FILE_SIZE && f.rel_path.len() <= path::MAX_REL_PATH)
            && total == Some(offer.total_size);
        if !valid {
            return decline("invalid");
        }
        let mut table = self.shared.table();
        let busy = table
            .list
            .iter()
            .filter(|t| !t.outgoing && t.state == TransferState::Running)
            .count();
        let existing = table
            .list
            .iter_mut()
            .find(|t| !t.outgoing && t.peer == peer && t.id == offer.transfer_id && t.id != 0);
        let handle = match existing {
            Some(t) => {
                // A resume: stop the old attempt first; the new task waits
                // for it to let go of the part files.
                t.signal.send_replace(Signal::Interrupt);
                t.signal = watch::channel(Signal::Run).0;
                t.handle
            }
            None => {
                if busy >= MAX_INCOMING {
                    drop(table);
                    return decline("busy");
                }
                let mut t =
                    self.shared
                        .new_transfer(peer, ctx.peer_name(peer), false, offer.transfer_id);
                t.state = TransferState::Running;
                let h = t.handle;
                table.list.push(t);
                h
            }
        };
        if let Some(t) = table.list.iter_mut().find(|t| t.handle == handle) {
            t.activity = activity;
            t.names = offer.files.iter().map(|f| f.rel_path.clone()).collect();
            t.total = offer.total_size;
            t.state = TransferState::Running;
            t.reason.clear();
            t.failed = 0;
        }
        drop(table);
        self.shared.changed();
        ctx.spawn(run_incoming(
            ctx.clone(),
            self.shared.clone(),
            handle,
            offer,
        ));
    }
}

/// Files and folders on disk as [`Source`]s. Folders keep their name as the
/// first path component; links and special files are skipped.
pub fn sources_from_paths(paths: &[PathBuf]) -> Result<Vec<Source>> {
    fn opener(p: PathBuf) -> Opener {
        Arc::new(move || File::open(&p))
    }
    fn walk(dir: &Path, rel: &str, depth: usize, out: &mut Vec<Source>) -> Result<()> {
        if depth > path::MAX_DEPTH {
            return Ok(());
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let child = format!("{rel}/{name}");
            if ft.is_dir() {
                walk(&e.path(), &child, depth + 1, out)?;
            } else if ft.is_file() {
                let size = e.metadata()?.len();
                out.push(Source {
                    rel_path: child,
                    size,
                    open: opener(e.path()),
                });
            }
            if out.len() > MAX_FILES {
                return Err(CoreError::InvalidInput(format!(
                    "more than {MAX_FILES} files"
                )));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for p in paths {
        let meta = std::fs::symlink_metadata(p)?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        if meta.is_dir() {
            walk(p, &name, 1, &mut out)?;
        } else if meta.is_file() {
            out.push(Source {
                rel_path: name,
                size: meta.len(),
                open: opener(p.clone()),
            });
        }
    }
    if out.len() > MAX_FILES {
        return Err(CoreError::InvalidInput(format!(
            "more than {MAX_FILES} files"
        )));
    }
    Ok(out)
}

/// Tell the host where a verified file landed, with its cleaned folder
/// relative to the download folder.
fn saved_hook(host: Arc<dyn FilesHost>) -> engine::OnSaved {
    let root = host.incoming_dir().canonicalize().ok();
    Arc::new(move |_, p| {
        let rel_dir = root
            .as_ref()
            .and_then(|r| p.parent()?.strip_prefix(r).ok())
            .map(|d| {
                d.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .unwrap_or_default();
        host.saved(p, &rel_dir)
    })
}

/// Wait a moment, then say whether the session to `peer` is still up: an
/// error while it is up is a real failure, otherwise an interruption.
async fn still_connected(ctx: &ServiceCtx, peer: DeviceId) -> bool {
    tokio::time::sleep(Duration::from_millis(300)).await;
    ctx.connected().contains(&peer)
}

async fn run_outgoing(ctx: ServiceCtx, sh: Arc<Shared>, handle: u64) {
    let Some((peer, id, lock)) = sh.with(handle, |t| (t.peer, t.id, t.running.clone())) else {
        return;
    };
    let Ok(_guard) = lock.try_lock() else {
        return; // already running
    };
    let mut sig = {
        let Some(rx) = sh.with(handle, |t| {
            t.signal = watch::channel(Signal::Run).0;
            t.state = TransferState::Asking;
            t.reason.clear();
            t.signal.subscribe()
        }) else {
            return;
        };
        rx
    };
    sh.changed();

    // 1. Approval on the receiver (R8.2).
    let activity = tokio::select! {
        r = ctx.request(peer) => r,
        s = signalled(&mut sig) => {
            let st = if s == Signal::Cancel { TransferState::Cancelled } else { TransferState::Interrupted };
            return sh.set_state(handle, st, "");
        }
    };
    let activity = match activity {
        Ok(a) => a,
        Err(CoreError::Refused(r)) if r == "offline" => {
            return sh.set_state(handle, TransferState::Interrupted, "");
        }
        Err(CoreError::Refused(r)) => return sh.set_state(handle, TransferState::Declined, &r),
        Err(_) => return sh.set_state(handle, TransferState::Interrupted, ""),
    };

    // 2. Offer what is not verified yet.
    let Some((srcs, map)) = sh.with(handle, |t| {
        t.activity = activity;
        let map: Vec<usize> = (0..t.sources.len()).filter(|i| !t.verified[*i]).collect();
        let srcs: Vec<Source> = map.iter().map(|i| t.sources[*i].clone()).collect();
        (srcs, map)
    }) else {
        return;
    };
    let offer = Offer {
        transfer_id: id,
        files: srcs
            .iter()
            .map(|s| FileEntry {
                rel_path: s.rel_path.clone(),
                size: s.size,
            })
            .collect(),
        total_size: srcs.iter().map(|s| s.size).sum(),
    };
    let (tx, rx) = oneshot::channel();
    sh.table().accepts.insert((peer, id), tx);
    send_msg(
        &ctx,
        peer,
        Direction::ToHost,
        activity,
        files_msg::Msg::Offer(offer),
    );
    let answer = tokio::select! {
        r = tokio::time::timeout(ACCEPT_TIMEOUT, rx) => r,
        s = signalled(&mut sig) => {
            sh.table().accepts.remove(&(peer, id));
            return finish_outgoing(&ctx, &sh, handle, peer, id, activity, s, None).await;
        }
    };
    sh.table().accepts.remove(&(peer, id));
    let accept = match answer {
        Ok(Ok(Ok(a))) => a,
        Ok(Ok(Err(reason))) => {
            ctx.stop(peer, activity);
            return sh.set_state(handle, TransferState::Declined, &reason);
        }
        _ => {
            ctx.stop(peer, activity);
            let st = if still_connected(&ctx, peer).await {
                TransferState::Failed
            } else {
                TransferState::Interrupted
            };
            return sh.set_state(handle, st, "timeout");
        }
    };
    let mut starts = vec![0u64; srcs.len()];
    for r in &accept.resume {
        if let Some(s) = starts.get_mut(r.index as usize) {
            *s = r.offset.min(srcs[r.index as usize].size);
        }
    }

    // 3. The data.
    let meter = Arc::new(engine::Meter::default());
    sh.with(handle, |t| {
        // Bytes verified in earlier attempts count as done.
        let before: u64 = (0..t.sources.len())
            .filter(|i| t.verified[*i])
            .map(|i| t.sources[i].size)
            .sum();
        t.meter = Arc::new(engine::Meter::default());
        t.meter.set(before);
        t.state = TransferState::Running;
    });
    sh.changed();
    let stream = tokio::select! {
        s = ctx.open_bi(peer, Direction::ToHost, activity, STREAM_DATA, id.to_be_bytes().to_vec()) => s,
        s = signalled(&mut sig) => return finish_outgoing(&ctx, &sh, handle, peer, id, activity, s, None).await,
    };
    let (mut send, mut recv) = match stream {
        Ok(s) => s,
        Err(_) => {
            return finish_outgoing(
                &ctx,
                &sh,
                handle,
                peer,
                id,
                activity,
                Signal::Interrupt,
                None,
            )
            .await;
        }
    };
    // Mirror the per-attempt meter into the transfer's meter.
    let shown = sh.with(handle, |t| t.meter.clone());
    let base = shown.as_ref().map(|m| m.get()).unwrap_or(0);
    let mirror = {
        let meter = meter.clone();
        let shown = shown.clone();
        async move {
            loop {
                tokio::time::sleep(TICK).await;
                if let Some(s) = &shown {
                    s.set(base + meter.get());
                }
            }
        }
    };
    let result = tokio::select! {
        r = engine::send(&mut send, &mut recv, &srcs, &starts, &meter) => Ok(r),
        s = signalled(&mut sig) => Err(s),
        () = mirror => Err(Signal::Interrupt),
    };
    if let Some(s) = &shown {
        s.set(base + meter.get());
    }
    match result {
        Ok(Ok(status)) => {
            let failed = status.iter().filter(|s| **s != wire::OK).count() as u32;
            sh.with(handle, |t| {
                for (k, s) in status.iter().enumerate() {
                    if *s == wire::OK
                        && let Some(i) = map.get(k)
                    {
                        t.verified[*i] = true;
                    }
                }
                t.failed = failed;
            });
            ctx.stop(peer, activity);
            if failed == 0 {
                sh.set_state(handle, TransferState::Done, "");
            } else {
                sh.set_state(handle, TransferState::Failed, "failed");
            }
        }
        Ok(Err(e)) => {
            tracing::debug!(peer = %peer.short_hex(), kind = ?e.kind(), "send stopped");
            finish_outgoing(
                &ctx,
                &sh,
                handle,
                peer,
                id,
                activity,
                Signal::Interrupt,
                Some((send, recv)),
            )
            .await;
        }
        Err(s) => {
            finish_outgoing(&ctx, &sh, handle, peer, id, activity, s, Some((send, recv))).await
        }
    }
}

/// End an outgoing attempt that did not complete.
#[allow(clippy::too_many_arguments)]
async fn finish_outgoing(
    ctx: &ServiceCtx,
    sh: &Shared,
    handle: u64,
    peer: DeviceId,
    id: u64,
    activity: u64,
    s: Signal,
    streams: Option<Opened>,
) {
    let by_peer = sh.with(handle, |t| t.reason == "peer").unwrap_or(false);
    if let Some((mut send, mut recv)) = streams {
        let _ = send.reset(super::stream_code::CANCELLED.into());
        let _ = recv.stop(super::stream_code::CANCELLED.into());
    }
    if s == Signal::Cancel || by_peer {
        if !by_peer {
            send_msg(
                ctx,
                peer,
                Direction::ToHost,
                activity,
                files_msg::Msg::Cancel(Cancel { transfer_id: id }),
            );
        }
        ctx.stop(peer, activity);
        let reason = if by_peer { "peer" } else { "" };
        return sh.set_state(handle, TransferState::Cancelled, reason);
    }
    if still_connected(ctx, peer).await {
        // Cancelled by the receiver while we waited?
        if sh.with(handle, |t| t.reason == "peer").unwrap_or(false) {
            return sh.set_state(handle, TransferState::Cancelled, "peer");
        }
        ctx.stop(peer, activity);
        sh.set_state(handle, TransferState::Interrupted, "")
    } else {
        sh.set_state(handle, TransferState::Interrupted, "")
    }
}

async fn run_incoming(ctx: ServiceCtx, sh: Arc<Shared>, handle: u64, offer: Offer) {
    let Some((peer, id, activity, lock, mut sig)) = sh.with(handle, |t| {
        (
            t.peer,
            t.id,
            t.activity,
            t.running.clone(),
            t.signal.subscribe(),
        )
    }) else {
        return;
    };
    let decline = |reason: &str| {
        send_msg(
            &ctx,
            peer,
            Direction::ToController,
            activity,
            files_msg::Msg::Decline(Decline {
                transfer_id: id,
                reason: reason.into(),
            }),
        );
    };
    // Wait for an earlier attempt of the same transfer to let go.
    let _guard = match tokio::time::timeout(Duration::from_secs(10), lock.lock_owned()).await {
        Ok(g) => g,
        Err(_) => {
            decline("busy");
            return sh.set_state(handle, TransferState::Failed, "busy");
        }
    };
    let Some(host) = sh.host.get() else {
        decline("storage");
        return sh.set_state(handle, TransferState::Failed, "storage");
    };
    let root = host.incoming_dir();
    let peer_hex = peer.hex();
    let entries: Vec<(String, u64, u64)> = offer
        .files
        .iter()
        .map(|f| (f.rel_path.clone(), f.size, id))
        .collect();
    let planned =
        tokio::task::spawn_blocking(move || engine::plan(&root, &peer_hex, &entries)).await;
    let targets = match planned {
        Ok(Ok(t)) => t,
        Ok(Err(engine::PlanError::Name(e))) => {
            tracing::info!(peer = %peer.short_hex(), error = %e, "offer refused");
            decline("invalid");
            return sh.set_state(handle, TransferState::Declined, "invalid");
        }
        Ok(Err(engine::PlanError::Storage(e))) => {
            tracing::warn!(kind = ?e.kind(), "download folder");
            decline("storage");
            return sh.set_state(handle, TransferState::Failed, "storage");
        }
        Err(_) => {
            decline("storage");
            return sh.set_state(handle, TransferState::Failed, "storage");
        }
    };
    let resume: Vec<ResumeAt> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.start > 0)
        .map(|(i, t)| ResumeAt {
            index: i as u32,
            offset: t.start,
        })
        .collect();
    let (stx, srx) = oneshot::channel();
    let meter = Arc::new(engine::Meter::default());
    let discard = Arc::new(AtomicBool::new(false));
    let view = sh.with(handle, |t| {
        t.targets = targets.clone();
        t.stream_tx = Some(stx);
        t.meter = meter.clone();
        t.discard = discard.clone();
        t.view()
    });
    if let (Some(v), Some(e)) = (view, sh.events.get()) {
        e.incoming(&v);
    }
    send_msg(
        &ctx,
        peer,
        Direction::ToController,
        activity,
        files_msg::Msg::Accept(Accept {
            transfer_id: id,
            resume,
        }),
    );
    let stream = tokio::select! {
        r = tokio::time::timeout(STREAM_TIMEOUT, srx) => r.ok().and_then(|r| r.ok()),
        s = signalled(&mut sig) => {
            return finish_incoming(&ctx, &sh, handle, peer, id, activity, s, &targets).await;
        }
    };
    let Some(Stream {
        send: Some(mut send),
        mut recv,
        ..
    }) = stream
    else {
        return finish_incoming(
            &ctx,
            &sh,
            handle,
            peer,
            id,
            activity,
            Signal::Interrupt,
            &targets,
        )
        .await;
    };
    let on_saved = saved_hook(host.clone());
    let result = tokio::select! {
        biased;
        r = engine::receive(&mut recv, &mut send, targets.clone(), meter, discard.clone(), on_saved) => Ok(r),
        s = signalled(&mut sig) => Err(s),
    };
    match result {
        Ok(Ok(outcomes)) => {
            let failed = outcomes
                .iter()
                .filter(|o| **o == engine::Outcome::Failed)
                .count() as u32;
            sh.with(handle, |t| {
                t.saved = outcomes
                    .iter()
                    .filter_map(|o| match o {
                        engine::Outcome::Saved(p) => Some(p.clone()),
                        engine::Outcome::Failed => None,
                    })
                    .collect();
                t.failed = failed;
                t.targets.clear();
            });
            ctx.log(
                peer,
                if failed == 0 {
                    "received"
                } else {
                    "received with errors"
                },
            );
            if failed == 0 {
                sh.set_state(handle, TransferState::Done, "");
            } else {
                sh.set_state(handle, TransferState::Failed, "failed");
            }
        }
        Ok(Err(e)) => {
            tracing::debug!(peer = %peer.short_hex(), kind = ?e.kind(), "receive stopped");
            let _ = send.reset(super::stream_code::CANCELLED.into());
            finish_incoming(
                &ctx,
                &sh,
                handle,
                peer,
                id,
                activity,
                Signal::Interrupt,
                &targets,
            )
            .await;
        }
        Err(s) => {
            if s == Signal::Cancel {
                discard.store(true, Ordering::Release);
            }
            let _ = send.reset(super::stream_code::CANCELLED.into());
            let _ = recv.stop(super::stream_code::CANCELLED.into());
            finish_incoming(&ctx, &sh, handle, peer, id, activity, s, &targets).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_incoming(
    ctx: &ServiceCtx,
    sh: &Shared,
    handle: u64,
    peer: DeviceId,
    id: u64,
    activity: u64,
    s: Signal,
    targets: &[engine::Target],
) {
    let by_peer = sh.with(handle, |t| t.reason == "peer").unwrap_or(false);
    if s == Signal::Cancel || by_peer {
        // Give the writer a moment to close the files, then delete them.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let t = targets.to_vec();
        let _ = tokio::task::spawn_blocking(move || engine::discard(&t)).await;
        if !by_peer {
            send_msg(
                ctx,
                peer,
                Direction::ToController,
                activity,
                files_msg::Msg::Cancel(Cancel { transfer_id: id }),
            );
        }
        ctx.stop(peer, activity);
        sh.with(handle, |t| t.targets.clear());
        let reason = if by_peer { "peer" } else { "" };
        return sh.set_state(handle, TransferState::Cancelled, reason);
    }
    sh.set_state(handle, TransferState::Interrupted, "");
}

async fn run_download(
    ctx: ServiceCtx,
    sh: Arc<Shared>,
    handle: u64,
    items: Vec<(String, u64, u64)>,
    open: OpenDownload,
) {
    let Some((peer, mut sig)) = sh.with(handle, |t| (t.peer, t.signal.subscribe())) else {
        return;
    };
    let Some(host) = sh.host.get() else {
        return sh.set_state(handle, TransferState::Failed, "storage");
    };
    let root = host.incoming_dir();
    let peer_hex = peer.hex();
    let entries = items.clone();
    let targets =
        match tokio::task::spawn_blocking(move || engine::plan(&root, &peer_hex, &entries)).await {
            Ok(Ok(t)) => t,
            Ok(Err(engine::PlanError::Name(_))) => {
                return sh.set_state(handle, TransferState::Failed, "invalid");
            }
            _ => return sh.set_state(handle, TransferState::Failed, "storage"),
        };
    let starts: Vec<u64> = targets.iter().map(|t| t.start).collect();
    let meter = Arc::new(engine::Meter::default());
    let discard = Arc::new(AtomicBool::new(false));
    sh.with(handle, |t| {
        t.targets = targets.clone();
        t.meter = meter.clone();
        t.discard = discard.clone();
    });
    let opened = tokio::select! {
        r = open(starts) => r,
        _ = signalled(&mut sig) => {
            engine::discard(&targets);
            return sh.set_state(handle, TransferState::Cancelled, "");
        }
    };
    let (mut send, mut recv) = match opened {
        Ok(s) => s,
        Err(CoreError::Refused(r)) => {
            engine::discard(&targets);
            return sh.set_state(handle, TransferState::Declined, &r);
        }
        Err(_) => return sh.set_state(handle, TransferState::Interrupted, ""),
    };
    let on_saved = saved_hook(host.clone());
    let result = tokio::select! {
        biased;
        r = engine::receive(&mut recv, &mut send, targets.clone(), meter, discard.clone(), on_saved) => Ok(r),
        s = signalled(&mut sig) => Err(s),
    };
    match result {
        Ok(Ok(outcomes)) => {
            let failed = outcomes
                .iter()
                .filter(|o| **o == engine::Outcome::Failed)
                .count() as u32;
            sh.with(handle, |t| {
                t.saved = outcomes
                    .iter()
                    .filter_map(|o| match o {
                        engine::Outcome::Saved(p) => Some(p.clone()),
                        engine::Outcome::Failed => None,
                    })
                    .collect();
                t.failed = failed;
                t.targets.clear();
            });
            let st = if failed == 0 {
                TransferState::Done
            } else {
                TransferState::Failed
            };
            sh.set_state(handle, st, if failed == 0 { "" } else { "failed" });
        }
        Ok(Err(_)) => {
            let _ = send.reset(super::stream_code::CANCELLED.into());
            sh.set_state(handle, TransferState::Interrupted, "");
        }
        Err(s) => {
            if s == Signal::Cancel {
                discard.store(true, Ordering::Release);
            }
            let _ = send.reset(super::stream_code::CANCELLED.into());
            let _ = recv.stop(super::stream_code::CANCELLED.into());
            if s == Signal::Cancel {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let t = targets.clone();
                let _ = tokio::task::spawn_blocking(move || engine::discard(&t)).await;
                sh.set_state(handle, TransferState::Cancelled, "");
            } else {
                sh.set_state(handle, TransferState::Interrupted, "");
            }
        }
    }
    let _ = ctx;
}

impl Service for Files {
    fn id(&self) -> ServiceId {
        ServiceId::FILES
    }

    fn capability(&self) -> Capability {
        Capability::Files
    }

    fn start(&self, ctx: ServiceCtx) {
        let weak: Weak<Shared> = Arc::downgrade(&self.shared);
        ctx.spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let Some(sh) = weak.upgrade() else { return };
                if sh.tick() {
                    sh.changed();
                }
            }
        });
        let _ = self.ctx.set(ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        let m: FilesMsg = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad FilesMsg".into()))?;
        let Some(m) = m.msg else {
            return Err(CoreError::Protocol("empty FilesMsg".into()));
        };
        let peer = msg.peer;
        match (msg.access, m) {
            (Access::Host(grant), files_msg::Msg::Offer(o)) => {
                self.on_offer(ctx, peer, grant.activity(), o);
            }
            (Access::Host(_), files_msg::Msg::Cancel(c)) => {
                self.peer_cancelled(ctx, peer, c.transfer_id, false);
            }
            (Access::Controller { .. }, files_msg::Msg::Accept(a)) => {
                if let Some(tx) = self.shared.table().accepts.remove(&(peer, a.transfer_id)) {
                    let _ = tx.send(Ok(a));
                }
            }
            (Access::Controller { .. }, files_msg::Msg::Decline(d)) => {
                if let Some(tx) = self.shared.table().accepts.remove(&(peer, d.transfer_id)) {
                    let reason: String = d.reason.chars().take(16).collect();
                    let _ = tx.send(Err(reason));
                }
            }
            (Access::Controller { .. }, files_msg::Msg::Cancel(c)) => {
                self.peer_cancelled(ctx, peer, c.transfer_id, true);
            }
            // Anything else is sent the wrong way; ignore it.
            _ => {}
        }
        Ok(())
    }

    fn on_stream(&self, _ctx: &ServiceCtx, mut stream: Stream) {
        let Access::Host(grant) = &stream.access else {
            return;
        };
        let id = <[u8; 8]>::try_from(stream.meta.as_slice()).map(u64::from_be_bytes);
        let tx = match (stream.kind, id) {
            (STREAM_DATA, Ok(id)) => {
                let mut table = self.shared.table();
                table
                    .list
                    .iter_mut()
                    .find(|t| {
                        !t.outgoing
                            && t.peer == stream.peer
                            && t.id == id
                            && t.activity == grant.activity()
                    })
                    .and_then(|t| t.stream_tx.take())
            }
            _ => None,
        };
        match tx {
            Some(tx) => {
                let _ = tx.send(stream);
            }
            None => {
                if let Some(s) = stream.send.as_mut() {
                    let _ = s.reset(super::stream_code::REFUSED.into());
                }
                let _ = stream.recv.stop(super::stream_code::REFUSED.into());
            }
        }
    }

    fn on_refused(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        // An offer was refused by the gate (for example the setting became
        // Deny): fail the waiting sender.
        let mut table = self.shared.table();
        let ids: Vec<u64> = table
            .list
            .iter()
            .filter(|t| t.outgoing && t.peer == peer && t.activity == activity)
            .map(|t| t.id)
            .collect();
        for id in ids {
            if let Some(tx) = table.accepts.remove(&(peer, id)) {
                let _ = tx.send(Err(reason.chars().take(16).collect()));
            }
        }
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        let table = self.shared.table();
        for t in table.list.iter() {
            if t.peer == peer && t.activity == activity && !t.state.is_final() {
                t.signal.send_replace(Signal::Interrupt);
            }
        }
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if !connected {
            let table = self.shared.table();
            for t in table
                .list
                .iter()
                .filter(|t| t.peer == peer && !t.state.is_final())
            {
                t.signal.send_replace(Signal::Interrupt);
            }
            return;
        }
        // Resume outgoing transfers to this device (R8.5).
        let resume: Vec<u64> = {
            let mut table = self.shared.table();
            table
                .list
                .iter_mut()
                .filter(|t| {
                    t.outgoing
                        && t.peer == peer
                        && t.state == TransferState::Interrupted
                        && t.resumes < MAX_RESUMES
                })
                .map(|t| {
                    t.resumes += 1;
                    t.handle
                })
                .collect()
        };
        for h in resume {
            let ctx2 = ctx.clone();
            let sh = self.shared.clone();
            ctx.spawn(async move {
                // Let the session settle (permission state, both services).
                tokio::time::sleep(Duration::from_millis(500)).await;
                run_outgoing(ctx2, sh, h).await;
            });
        }
    }
}

impl Files {
    /// The peer cancelled transfer `id`. `outgoing` is true when this device
    /// is the sender.
    fn peer_cancelled(&self, ctx: &ServiceCtx, peer: DeviceId, id: u64, outgoing: bool) {
        let late = self.shared.with_where(
            |t| t.outgoing == outgoing && t.peer == peer && t.id == id && !t.state.is_final(),
            |t| {
                t.reason = "peer".into();
                if t.state == TransferState::Interrupted {
                    t.state = TransferState::Cancelled;
                    return Some(std::mem::take(&mut t.targets));
                }
                t.discard.store(true, Ordering::Release);
                t.signal.send_replace(Signal::Cancel);
                None
            },
        );
        if let Some(Some(targets)) = late {
            engine::discard(&targets);
        }
        let _ = ctx;
        self.shared.changed();
    }
}

impl Shared {
    fn with_where<R>(
        &self,
        pred: impl Fn(&Transfer) -> bool,
        f: impl FnOnce(&mut Transfer) -> R,
    ) -> Option<R> {
        let mut t = self.table();
        t.list.iter_mut().find(|x| pred(x)).map(f)
    }
}

/// Add this feature's services to a starting node. `platform` is `linux`,
/// `windows`, `macos`, or `android`.
pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Files::default()));
}
