//! Moving file bytes over one stream pair (design §9.2): the sender reads
//! and hashes on a blocking thread; the receiver writes `*.urdevice-part`
//! files, hashes while writing, keeps a resume sidecar, and renames a file
//! only after its SHA-256 matched (R8.4, R8.5). Used by file transfers and by
//! gallery downloads.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ring::digest::{Context, SHA256};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use super::path::{self, PART_SUFFIX, RESUME_SUFFIX};
use super::wire::{self, AckFrame, CHUNK, DataFrame, DataReader};

/// Opens a file to send. Called again for a retry or a resume.
pub type Opener = Arc<dyn Fn() -> io::Result<File> + Send + Sync>;

/// One file to send.
#[derive(Clone)]
pub struct Source {
    /// Path relative to the transfer root, `/` between folders.
    pub rel_path: String,
    /// Size in bytes.
    pub size: u64,
    /// Opens it.
    pub open: Opener,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No names in debug output: they are user content.
        f.debug_struct("Source")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Bytes moved so far, read by the UI.
#[derive(Debug, Default)]
pub struct Meter(AtomicU64);

impl Meter {
    /// Bytes so far.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
    /// Set the count.
    pub fn set(&self, v: u64) {
        self.0.store(v, Ordering::Relaxed);
    }
    fn add(&self, v: u64) {
        self.0.fetch_add(v, Ordering::Relaxed);
    }
    fn sub(&self, v: u64) {
        let _ = self
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| {
                Some(x.saturating_sub(v))
            });
    }
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_owned())
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

enum Chunk {
    Data(u64, Vec<u8>),
    End([u8; 32]),
}

/// Read `src` from `start` in chunks, hashing the whole file (the part before
/// `start` too, so the hash covers what the receiver already has).
fn produce(src: &Source, start: u64, tx: &mpsc::Sender<Chunk>) -> io::Result<()> {
    let mut f = (src.open)()?;
    let mut ctx = Context::new(&SHA256);
    let mut buf = vec![0u8; CHUNK];
    let mut pos = 0u64;
    while pos < start {
        let want = (start - pos).min(CHUNK as u64) as usize;
        f.read_exact(&mut buf[..want])?;
        ctx.update(&buf[..want]);
        pos += want as u64;
    }
    f.seek(SeekFrom::Start(start))?;
    while pos < src.size {
        let want = (src.size - pos).min(CHUNK as u64) as usize;
        let mut chunk = vec![0u8; want];
        f.read_exact(&mut chunk)?;
        ctx.update(&chunk);
        if tx.blocking_send(Chunk::Data(pos, chunk)).is_err() {
            return Ok(());
        }
        pos += want as u64;
    }
    let mut h = [0u8; 32];
    h.copy_from_slice(ctx.finish().as_ref());
    let _ = tx.blocking_send(Chunk::End(h));
    Ok(())
}

async fn send_one<W: AsyncWrite + Unpin>(
    w: &mut W,
    index: u32,
    src: &Source,
    start: u64,
) -> io::Result<()> {
    let (tx, mut rx) = mpsc::channel(4);
    let src2 = src.clone();
    let reader = tokio::task::spawn_blocking(move || produce(&src2, start, &tx));
    let mut ended = false;
    while let Some(c) = rx.recv().await {
        match c {
            Chunk::Data(off, bytes) => wire::write_data(w, index, off, &bytes).await?,
            Chunk::End(h) => {
                wire::write_end(w, index, &h).await?;
                ended = true;
            }
        }
    }
    let read = reader.await.map_err(|_| bad("reader stopped"))?;
    if !ended {
        // The file could not be read (gone, shorter than offered). An end
        // frame with a zero hash makes the receiver fail just this file.
        tracing::debug!(index, error = ?read.err().map(|e| e.kind()), "file not readable");
        wire::write_end(w, index, &[0u8; 32]).await?;
    }
    Ok(())
}

/// Send `files`, file `i` from `starts[i]`, and wait for the receiver's
/// verdict on each. A file whose hash did not match is sent once more from
/// the start (design §14). Returns the status of each file
/// ([`wire::OK`] or [`wire::FAILED`]).
pub async fn send<W, R>(
    w: &mut W,
    r: &mut R,
    files: &[Source],
    starts: &[u64],
    meter: &Meter,
) -> io::Result<Vec<u8>>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
{
    let n = files.len();
    let mut acked: Vec<u64> = (0..n)
        .map(|i| starts.get(i).copied().unwrap_or(0))
        .collect();
    meter.set(acked.iter().sum());
    let (retry_tx, mut retry_rx) = mpsc::unbounded_channel::<u32>();

    let writer = async {
        for (i, src) in files.iter().enumerate() {
            let start = starts.get(i).copied().unwrap_or(0).min(src.size);
            send_one(w, i as u32, src, start).await?;
        }
        while let Some(i) = retry_rx.recv().await {
            if let Some(src) = files.get(i as usize) {
                send_one(w, i, src, 0).await?;
            }
        }
        w.shutdown().await?;
        io::Result::Ok(())
    };

    let reader = async {
        let mut status: Vec<Option<u8>> = vec![None; n];
        let mut retried = vec![false; n];
        let mut left = n;
        let mut retry_tx = Some(retry_tx);
        if left == 0 {
            retry_tx = None;
        }
        while left > 0 {
            let Some(f) = wire::read_ack(r).await? else {
                return Err(bad("receiver ended early"));
            };
            match f {
                AckFrame::Ack { index, offset } => {
                    let i = index as usize;
                    let Some(file) = files.get(i) else {
                        return Err(bad("ack for unknown file"));
                    };
                    let offset = offset.min(file.size);
                    if offset > acked[i] {
                        meter.add(offset - acked[i]);
                        acked[i] = offset;
                    }
                }
                AckFrame::Result { index, status: s } => {
                    let i = index as usize;
                    if i >= n || status[i].is_some() {
                        return Err(bad("bad result"));
                    }
                    if s == wire::RETRY && !retried[i] {
                        retried[i] = true;
                        meter.sub(acked[i]);
                        acked[i] = 0;
                        if let Some(tx) = &retry_tx {
                            let _ = tx.send(index);
                        }
                        continue;
                    }
                    let s = if s == wire::OK {
                        wire::OK
                    } else {
                        wire::FAILED
                    };
                    if s == wire::OK && acked[i] < files[i].size {
                        meter.add(files[i].size - acked[i]);
                        acked[i] = files[i].size;
                    }
                    status[i] = Some(s);
                    left -= 1;
                }
            }
            if left == 0 {
                retry_tx = None;
            }
        }
        drop(retry_tx);
        io::Result::Ok(
            status
                .into_iter()
                .map(|s| s.unwrap_or(wire::FAILED))
                .collect(),
        )
    };

    let ((), status) = tokio::try_join!(writer, reader)?;
    Ok(status)
}

// ---------------------------------------------------------------------------
// Receiving
// ---------------------------------------------------------------------------

/// Where one incoming file goes.
#[derive(Debug, Clone)]
pub struct Target {
    /// The folder, already created and checked to be inside the root.
    pub dir: PathBuf,
    /// The cleaned final name; a collision becomes `name (1).ext`.
    pub name: String,
    /// The part file in `dir`, already created.
    pub part: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Bytes already there from an earlier attempt.
    pub start: u64,
    /// Resume key: the sender and its id for this file.
    pub key: ResumeKey,
}

/// Identifies a part file across attempts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeKey {
    /// The sending device (hex Device ID).
    pub peer: String,
    /// Transfer id (files) or item id (gallery).
    pub id: u64,
}

fn sidecar_path(part: &Path) -> PathBuf {
    let mut s = part.as_os_str().to_owned();
    s.push(".resume");
    PathBuf::from(s)
}

fn write_sidecar(part: &Path, key: &ResumeKey, size: u64, offset: u64) -> io::Result<()> {
    let body = format!(
        "urdevice-resume 1\n{}\n{}\n{size}\n{offset}\n",
        key.peer, key.id
    );
    std::fs::write(sidecar_path(part), body)
}

/// The verified offset in a sidecar, if it belongs to `key` and `size`.
fn read_sidecar(part: &Path, key: &ResumeKey, size: u64) -> Option<u64> {
    let side = sidecar_path(part);
    let meta = std::fs::symlink_metadata(&side).ok()?;
    if !meta.file_type().is_file() || meta.len() > 256 {
        return None;
    }
    let text = std::fs::read_to_string(&side).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "urdevice-resume 1" {
        return None;
    }
    let peer = lines.next()?;
    let id: u64 = lines.next()?.parse().ok()?;
    let sz: u64 = lines.next()?.parse().ok()?;
    let off: u64 = lines.next()?.parse().ok()?;
    (peer == key.peer && id == key.id && sz == size && off <= size).then_some(off)
}

fn remove_part(part: &Path) {
    let _ = std::fs::remove_file(part);
    let _ = std::fs::remove_file(sidecar_path(part));
}

/// Why a plan was refused.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    /// A name could not be cleaned.
    #[error("invalid name: {0}")]
    Name(#[from] path::PathError),
    /// The download folder or a folder in it could not be used.
    #[error("storage: {0}")]
    Storage(#[from] io::Error),
}

/// Plan where `files` (cleaned `rel_path`, size, resume id) go under `root`:
/// create the folders, find part files to resume, and reserve new ones. On
/// an error nothing new is left behind except empty folders.
pub fn plan(
    root: &Path,
    peer: &str,
    files: &[(String, u64, u64)],
) -> Result<Vec<Target>, PlanError> {
    let mut cleaned = Vec::with_capacity(files.len());
    for (rel, size, id) in files {
        cleaned.push((path::clean_rel_path(rel)?, *size, *id));
    }
    let mut out: Vec<Target> = Vec::with_capacity(files.len());
    let mut used: HashSet<String> = HashSet::new();
    let result = (|| {
        for (comps, size, id) in cleaned {
            let (name, dirs) = comps.split_last().ok_or(path::PathError::Empty)?;
            let dir = path::ensure_dirs(root, dirs)?;
            let key = ResumeKey {
                peer: peer.to_owned(),
                id,
            };
            let lower = |p: &Path| p.to_string_lossy().to_lowercase();
            // An earlier attempt of the same file?
            let mut resume = None;
            for n in 0..64 {
                let cand = if n == 0 {
                    name.clone()
                } else {
                    let (s, e) = path::split_ext(name);
                    format!("{s} ({n}){e}")
                };
                let part = dir.join(format!("{cand}{PART_SUFFIX}"));
                if !path::taken(&part) {
                    break;
                }
                if used.contains(&lower(&part)) {
                    continue;
                }
                if let Some(off) = read_sidecar(&part, &key, size)
                    && let Ok(m) = std::fs::symlink_metadata(&part)
                    && m.file_type().is_file()
                {
                    resume = Some((part, off.min(m.len())));
                    break;
                }
            }
            let (part, start) = match resume {
                Some(r) => r,
                None => {
                    let free = |n: &str| {
                        let p = dir.join(format!("{n}{PART_SUFFIX}"));
                        !path::taken(&p)
                            && !path::taken(&dir.join(format!("{n}{RESUME_SUFFIX}")))
                            && !used.contains(&lower(&p))
                    };
                    let n = path::unique_name(name, free)?;
                    let part = dir.join(format!("{n}{PART_SUFFIX}"));
                    OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&part)?;
                    write_sidecar(&part, &key, size, 0)?;
                    (part, 0)
                }
            };
            used.insert(lower(&part));
            out.push(Target {
                dir,
                name: name.clone(),
                part,
                size,
                start,
                key,
            });
        }
        Ok::<(), PlanError>(())
    })();
    match result {
        Ok(()) => Ok(out),
        Err(e) => {
            for t in &out {
                if t.start == 0 {
                    remove_part(&t.part);
                }
            }
            Err(e)
        }
    }
}

/// Delete the part files of a plan (cancel).
pub fn discard(targets: &[Target]) {
    for t in targets {
        remove_part(&t.part);
    }
}

/// Called on the writer thread when a file was verified and moved to `path`.
/// Returns where the file is now, if the platform moved it again (Android:
/// MediaStore Downloads).
pub type OnSaved = Arc<dyn Fn(usize, &Path) -> Option<String> + Send + Sync>;

/// What happened to each file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Verified and saved here.
    Saved(String),
    /// Failed (hash, size, or storage).
    Failed,
}

enum Cmd {
    Data(u32, u64, Vec<u8>),
    End(u32, [u8; 32]),
}

enum Ev {
    Ack(u32, u64),
    Result(u32, u8, Option<String>),
    Violation(&'static str),
}

struct Slot {
    t: Target,
    written: u64,
    retried: bool,
    done: bool,
    file: Option<File>,
    hash: Option<Context>,
    unsynced: u64,
}

/// Sync the sidecar every this many bytes.
const SYNC_EVERY: u64 = 16 * 1024 * 1024;

impl Slot {
    /// Open the part file and hash what it already holds.
    fn open(&mut self) -> io::Result<()> {
        let m = std::fs::symlink_metadata(&self.t.part)?;
        if !m.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "part is not a file",
            ));
        }
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.t.part)?;
        f.set_len(self.written)?;
        let mut ctx = Context::new(&SHA256);
        let mut buf = vec![0u8; CHUNK];
        let mut left = self.written;
        while left > 0 {
            let k = left.min(CHUNK as u64) as usize;
            f.read_exact(&mut buf[..k])?;
            ctx.update(&buf[..k]);
            left -= k as u64;
        }
        f.seek(SeekFrom::Start(self.written))?;
        self.file = Some(f);
        self.hash = Some(ctx);
        Ok(())
    }

    fn checkpoint(&mut self) {
        if let Some(f) = &self.file {
            let _ = f.sync_data();
        }
        let _ = write_sidecar(&self.t.part, &self.t.key, self.t.size, self.written);
        self.unsynced = 0;
    }
}

fn worker(
    targets: Vec<Target>,
    mut rx: mpsc::Receiver<Cmd>,
    ev: mpsc::UnboundedSender<Ev>,
    meter: Arc<Meter>,
    discard_parts: Arc<AtomicBool>,
    on_saved: OnSaved,
) {
    let mut slots: Vec<Slot> = targets
        .into_iter()
        .map(|t| Slot {
            written: t.start,
            t,
            retried: false,
            done: false,
            file: None,
            hash: None,
            unsynced: 0,
        })
        .collect();
    meter.set(slots.iter().map(|s| s.written).sum());
    let result = run_worker(&mut slots, &mut rx, &ev, &meter, &on_saved);
    // The stream ended, the transfer stopped, or the sender broke the rules.
    let discard = discard_parts.load(Ordering::Acquire);
    for s in slots.iter_mut().filter(|s| !s.done) {
        if discard {
            s.file = None;
            remove_part(&s.t.part);
        } else if s.file.is_some() {
            s.checkpoint();
        }
    }
    if let Err(what) = result {
        let _ = ev.send(Ev::Violation(what));
    }
}

fn run_worker(
    slots: &mut [Slot],
    rx: &mut mpsc::Receiver<Cmd>,
    ev: &mpsc::UnboundedSender<Ev>,
    meter: &Meter,
    on_saved: &OnSaved,
) -> Result<(), &'static str> {
    let mut current: Option<usize> = None;
    while let Some(cmd) = rx.blocking_recv() {
        match cmd {
            Cmd::Data(index, offset, bytes) => {
                let i = index as usize;
                let Some(s) = slots.get_mut(i) else {
                    return Err("unknown file");
                };
                if s.done
                    || current.is_some_and(|c| c != i)
                    || offset != s.written
                    || offset.saturating_add(bytes.len() as u64) > s.t.size
                {
                    return Err("out of order");
                }
                current = Some(i);
                if s.file.is_none()
                    && let Err(e) = s.open()
                {
                    tracing::warn!(index, kind = ?e.kind(), "part file");
                    return Err("storage");
                }
                let (Some(f), Some(h)) = (s.file.as_mut(), s.hash.as_mut()) else {
                    return Err("storage");
                };
                if let Err(e) = f.write_all(&bytes) {
                    tracing::warn!(index, kind = ?e.kind(), "write failed");
                    return Err("storage");
                }
                h.update(&bytes);
                s.written += bytes.len() as u64;
                s.unsynced += bytes.len() as u64;
                meter.add(bytes.len() as u64);
                if s.unsynced >= SYNC_EVERY {
                    s.checkpoint();
                }
                let _ = ev.send(Ev::Ack(index, s.written));
            }
            Cmd::End(index, sha) => {
                let i = index as usize;
                let Some(s) = slots.get_mut(i) else {
                    return Err("unknown file");
                };
                if s.done || current.is_some_and(|c| c != i) {
                    return Err("out of order");
                }
                current = None;
                if s.file.is_none()
                    && let Err(e) = s.open()
                {
                    tracing::warn!(index, kind = ?e.kind(), "part file");
                    return Err("storage");
                }
                let file = s.file.take();
                let ok = s.written == s.t.size
                    && s.hash
                        .take()
                        .is_some_and(|h| h.finish().as_ref() == sha.as_slice());
                if !ok {
                    drop(file);
                    let full = s.written == s.t.size;
                    meter.sub(s.written);
                    s.written = 0;
                    if full && !s.retried {
                        // Hash mismatch: start this file over once (design §14).
                        s.retried = true;
                        let _ = std::fs::OpenOptions::new()
                            .write(true)
                            .open(&s.t.part)
                            .and_then(|f| f.set_len(0));
                        let _ = ev.send(Ev::Result(index, wire::RETRY, None));
                    } else {
                        remove_part(&s.t.part);
                        s.done = true;
                        tracing::info!(index, "file failed its check and was discarded");
                        let _ = ev.send(Ev::Result(index, wire::FAILED, None));
                    }
                    continue;
                }
                if let Some(f) = &file {
                    let _ = f.sync_all();
                }
                drop(file);
                s.done = true;
                match path::place(&s.t.part, &s.t.dir, &s.t.name) {
                    Ok(dest) => {
                        let _ = std::fs::remove_file(sidecar_path(&s.t.part));
                        let shown =
                            on_saved(i, &dest).unwrap_or_else(|| dest.display().to_string());
                        let _ = ev.send(Ev::Result(index, wire::OK, Some(shown)));
                    }
                    Err(e) => {
                        tracing::warn!(index, kind = ?e.kind(), "could not save file");
                        remove_part(&s.t.part);
                        let _ = ev.send(Ev::Result(index, wire::FAILED, None));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Receive into `targets` (from [`plan`]), acknowledging on `w`. Returns the
/// outcome of each file once every file is final. If the future is dropped
/// or the stream fails, part files stay for a resume unless `discard` is set
/// first.
pub async fn receive<R, W>(
    r: &mut R,
    w: &mut W,
    targets: Vec<Target>,
    meter: Arc<Meter>,
    discard: Arc<AtomicBool>,
    on_saved: OnSaved,
) -> io::Result<Vec<Outcome>>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let n = targets.len();
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(8);
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    tokio::task::spawn_blocking(move || worker(targets, cmd_rx, ev_tx, meter, discard, on_saved));

    let reader = async move {
        let mut frames = DataReader::default();
        while let Some(f) = frames.next(r).await? {
            let cmd = match f {
                DataFrame::Data {
                    index,
                    offset,
                    bytes,
                } => Cmd::Data(index, offset, bytes.to_vec()),
                DataFrame::End { index, sha256 } => Cmd::End(index, sha256),
            };
            if cmd_tx.send(cmd).await.is_err() {
                break;
            }
        }
        // The sender finished. If files are still open it gave up: closing
        // the channel lets the worker keep the parts and end, and `acks`
        // then reports the early end.
        drop(cmd_tx);
        std::future::pending::<()>().await;
        io::Result::Ok(())
    };

    let acks = async {
        let mut out: Vec<Option<Outcome>> = vec![None; n];
        let mut left = n;
        while left > 0 {
            let Some(e) = ev_rx.recv().await else {
                return Err(bad("sender ended early"));
            };
            let frame = match e {
                Ev::Ack(index, offset) => AckFrame::Ack { index, offset },
                Ev::Result(index, status, saved) => {
                    if status != wire::RETRY {
                        out[index as usize] = Some(match saved {
                            Some(p) => Outcome::Saved(p),
                            None => Outcome::Failed,
                        });
                        left -= 1;
                    }
                    AckFrame::Result { index, status }
                }
                Ev::Violation(what) => return Err(bad(what)),
            };
            w.write_all(&wire::encode_ack(frame)).await?;
        }
        w.shutdown().await?;
        Ok(out
            .into_iter()
            .map(|o| o.unwrap_or(Outcome::Failed))
            .collect())
    };

    tokio::select! {
        biased;
        r = acks => r,
        r = reader => r.map(|()| Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "urdevice-engine-{tag}-{}-{}",
            std::process::id(),
            crate::crypto::random_u64().unwrap()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn data(len: usize, seed: u8) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn source(dir: &Path, rel: &str, bytes: &[u8]) -> Source {
        let p = dir.join(rel.replace('/', "_"));
        std::fs::write(&p, bytes).unwrap();
        Source {
            rel_path: rel.into(),
            size: bytes.len() as u64,
            open: Arc::new(move || File::open(&p)),
        }
    }

    fn sha(b: &[u8]) -> [u8; 32] {
        let mut h = [0u8; 32];
        h.copy_from_slice(ring::digest::digest(&SHA256, b).as_ref());
        h
    }

    fn no_saved() -> OnSaved {
        Arc::new(|_, _| None)
    }

    /// Run sender and receiver against each other over in-memory pipes.
    fn transfer(srcs: &[Source], starts: &[u64], targets: Vec<Target>) -> (Vec<u8>, Vec<Outcome>) {
        rt().block_on(async {
            let (mut data_w, mut data_r) = tokio::io::duplex(256 * 1024);
            let (mut ack_w, mut ack_r) = tokio::io::duplex(64 * 1024);
            let meter = Meter::default();
            let rmeter = Arc::new(Meter::default());
            let (s, r) = tokio::join!(
                send(&mut data_w, &mut ack_r, srcs, starts, &meter),
                receive(
                    &mut data_r,
                    &mut ack_w,
                    targets,
                    rmeter.clone(),
                    Arc::default(),
                    no_saved()
                ),
            );
            let total: u64 = srcs.iter().map(|s| s.size).sum();
            assert_eq!(meter.get(), total);
            (s.unwrap(), r.unwrap())
        })
    }

    fn entries(srcs: &[Source], id: u64) -> Vec<(String, u64, u64)> {
        srcs.iter()
            .map(|s| (s.rel_path.clone(), s.size, id))
            .collect()
    }

    fn parts_left(dir: &Path) -> usize {
        fn walk(d: &Path, n: &mut usize) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, n);
                } else if p.to_string_lossy().contains(".urdevice-part") {
                    *n += 1;
                }
            }
        }
        let mut n = 0;
        walk(dir, &mut n);
        n
    }

    #[test]
    fn files_arrive_verified_in_their_folders() {
        let src = temp("src");
        let dst = temp("dst");
        let big = data(2 * CHUNK + 12345, 1);
        let srcs = vec![
            source(&src, "a.txt", b"hello"),
            source(&src, "Folder/sub/big.bin", &big),
            source(&src, "Folder/empty", b""),
        ];
        let targets = plan(&dst, "peer", &entries(&srcs, 1)).unwrap();
        let (status, out) = transfer(&srcs, &[0, 0, 0], targets);
        assert_eq!(status, vec![wire::OK; 3]);
        assert!(out.iter().all(|o| matches!(o, Outcome::Saved(_))));
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"hello");
        assert_eq!(std::fs::read(dst.join("Folder/sub/big.bin")).unwrap(), big);
        assert_eq!(std::fs::read(dst.join("Folder/empty")).unwrap(), b"");
        assert_eq!(parts_left(&dst), 0);
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn existing_files_are_never_overwritten() {
        let src = temp("src");
        let dst = temp("dst");
        std::fs::write(dst.join("a.txt"), b"keep").unwrap();
        let srcs = vec![
            source(&src, "a.txt", b"new"),
            source(&src, "A.TXT", b"other"),
        ];
        let targets = plan(&dst, "peer", &entries(&srcs, 1)).unwrap();
        let (_, out) = transfer(&srcs, &[0, 0], targets);
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"keep");
        let Outcome::Saved(p) = &out[0] else { panic!() };
        assert!(p.ends_with("a (1).txt"), "{p}");
        assert!(matches!(&out[1], Outcome::Saved(_)));
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn a_second_attempt_resumes_from_the_sidecar() {
        let src = temp("src");
        let dst = temp("dst");
        let big = data(3 * CHUNK + 7, 2);
        let srcs = vec![source(&src, "big.bin", &big)];
        let first = plan(&dst, "peer", &entries(&srcs, 9)).unwrap();
        assert_eq!(first[0].start, 0);
        // An interrupted attempt left 1.5 chunks and a sidecar.
        let have = CHUNK as u64 + CHUNK as u64 / 2;
        std::fs::write(&first[0].part, &big[..have as usize]).unwrap();
        write_sidecar(&first[0].part, &first[0].key, big.len() as u64, have).unwrap();
        // Another sender or another transfer id does not match it.
        let other = plan(&dst, "peer", &entries(&srcs, 10)).unwrap();
        assert_eq!(other[0].start, 0);
        assert_ne!(other[0].part, first[0].part);
        discard(&other);
        let again = plan(&dst, "peer", &entries(&srcs, 9)).unwrap();
        assert_eq!(again[0].part, first[0].part);
        assert_eq!(again[0].start, have);
        let (status, _) = transfer(&srcs, &[have], again);
        assert_eq!(status, vec![wire::OK]);
        assert_eq!(std::fs::read(dst.join("big.bin")).unwrap(), big);
        assert_eq!(parts_left(&dst), 0);
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    /// Drive a receiver with hand-written frames and collect its acks.
    fn drive(targets: Vec<Target>, frames: Vec<u8>) -> (io::Result<Vec<Outcome>>, Vec<AckFrame>) {
        rt().block_on(async {
            let (mut ack_w, mut ack_r) = tokio::io::duplex(64 * 1024);
            let mut input: &[u8] = &frames;
            let r = receive(
                &mut input,
                &mut ack_w,
                targets,
                Arc::default(),
                Arc::default(),
                no_saved(),
            )
            .await;
            drop(ack_w);
            let mut acks = Vec::new();
            while let Ok(Some(a)) = wire::read_ack(&mut ack_r).await {
                acks.push(a);
            }
            (r, acks)
        })
    }

    async fn frame_file(out: &mut Vec<u8>, index: u32, bytes: &[u8], hash: [u8; 32]) {
        wire::write_data(out, index, 0, bytes).await.unwrap();
        wire::write_end(out, index, &hash).await.unwrap();
    }

    #[test]
    fn a_bad_hash_is_retried_once_then_discarded() {
        let dst = temp("dst");
        let body = b"payload";
        let mk = || plan(&dst, "peer", &[("x.bin".into(), body.len() as u64, 1)]).unwrap();
        // Wrong, then right: saved after one retry.
        let mut f = Vec::new();
        rt().block_on(async {
            frame_file(&mut f, 0, body, [0; 32]).await;
            frame_file(&mut f, 0, body, sha(body)).await;
        });
        let (r, acks) = drive(mk(), f);
        assert!(matches!(r.unwrap()[0], Outcome::Saved(_)));
        assert!(acks.contains(&AckFrame::Result {
            index: 0,
            status: wire::RETRY
        }));
        assert!(acks.contains(&AckFrame::Result {
            index: 0,
            status: wire::OK
        }));
        assert_eq!(std::fs::read(dst.join("x.bin")).unwrap(), body);
        // Wrong twice: failed and nothing left behind.
        let mut f = Vec::new();
        rt().block_on(async {
            frame_file(&mut f, 0, body, [0; 32]).await;
            frame_file(&mut f, 0, body, [1; 32]).await;
        });
        let (r, acks) = drive(mk(), f);
        assert_eq!(r.unwrap(), vec![Outcome::Failed]);
        assert!(acks.contains(&AckFrame::Result {
            index: 0,
            status: wire::FAILED
        }));
        assert!(!dst.join("x (1).bin").exists());
        assert_eq!(parts_left(&dst), 0);
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn frames_that_break_the_order_end_the_transfer() {
        let dst = temp("dst");
        let mk = || plan(&dst, "peer", &[("a".into(), 4, 1), ("b".into(), 4, 1)]).unwrap();
        let cases: Vec<Vec<u8>> = rt().block_on(async {
            let mut gap = Vec::new();
            wire::write_data(&mut gap, 0, 1, b"abc").await.unwrap();
            let mut past_end = Vec::new();
            wire::write_data(&mut past_end, 0, 0, b"abcde")
                .await
                .unwrap();
            let mut interleave = Vec::new();
            wire::write_data(&mut interleave, 0, 0, b"ab")
                .await
                .unwrap();
            wire::write_data(&mut interleave, 1, 0, b"ab")
                .await
                .unwrap();
            let mut unknown = Vec::new();
            wire::write_data(&mut unknown, 7, 0, b"ab").await.unwrap();
            vec![gap, past_end, interleave, unknown]
        });
        for f in cases {
            let (r, _) = drive(mk(), f);
            assert!(r.is_err());
        }
        // The sender stopping early is an error too; the parts stay for a resume.
        let (r, _) = drive(mk(), Vec::new());
        assert!(r.is_err());
        assert_eq!(parts_left(&dst), 2 * 2);
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn plans_refuse_bad_names_and_leave_nothing() {
        let dst = temp("dst");
        let r = plan(&dst, "p", &[("ok.txt".into(), 1, 1), ("../x".into(), 1, 1)]);
        assert!(matches!(r, Err(PlanError::Name(_))));
        assert_eq!(parts_left(&dst), 0);
        assert!(!dst.parent().unwrap().join("x").exists());
        std::fs::remove_dir_all(&dst).unwrap();
    }
}
