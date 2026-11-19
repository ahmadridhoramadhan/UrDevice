//! Clipboard sharing (#16, design §9.1, R7).
//!
//! Copy on one paired device, paste on another. The service has two halves:
//!
//! - **Sending.** A platform watcher (desktop) or a user action (Android: the
//!   notification button, the Quick Settings tile, the share sheet) hands new
//!   content to [`Clipboard::local_changed`] or [`Clipboard::send_manual`].
//!   The service drops what must not leave the device (paused, per-device
//!   switch off, marked sensitive, too large, echo of received content) and
//!   delivers the rest to every connected peer that offers `clipboard.v1`.
//!   Items up to [`MAX_INLINE_BYTES`] go in a control message, larger ones
//!   over a QUIC stream.
//! - **Receiving.** The node has already checked the host's own Clipboard
//!   setting ([`Access::Host`]). The service checks the pause and per-device
//!   switches, validates the item as hostile input, remembers its hash so the
//!   watcher does not send it back (R7.4), and writes it through
//!   [`ClipboardHost`].
//!
//! Clipboard content is never logged, and no event carries it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::time::timeout;
use urdevice_proto::features::clipboard::{
    Clip, ClipHeader, MAX_ALT_TEXT_BYTES, MAX_CLIP_BYTES, MAX_INLINE_BYTES, STREAM_CLIP, mime,
};

use super::{
    Access, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Slot, Stream,
};
use crate::identity::DeviceId;
use crate::permission::{Capability, Mode};
use crate::{CoreError, Result};

/// Release 2 capabilities this module ships. Clipboard is Release 1, which
/// is always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Most items accepted from one peer in [`RATE_WINDOW`].
const RATE_LIMIT: usize = 20;
const RATE_WINDOW: Duration = Duration::from_secs(10);
/// How many item ids are remembered to drop repeats.
const SEEN_IDS: usize = 64;
/// How long a stream may take to deliver an item.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest image, in pixels, that is written to the clipboard. Stops a tiny
/// PNG that decodes to gigabytes when an app pastes it.
const MAX_PIXELS: u64 = 100_000_000;
/// How long a new item stays available for a retry after `ask`.
const RETRY_WINDOW: Duration = Duration::from_secs(5);

/// Clipboard content in one format, with the plain-text version of rich text.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipData {
    /// One of `text/plain`, `text/html`, `image/png`.
    pub mime: String,
    /// The content in that format. Text is UTF-8.
    pub bytes: Vec<u8>,
    /// Plain-text version of an HTML item, so apps that paste only text still
    /// get something.
    pub alt_text: Option<String>,
}

impl ClipData {
    /// Plain text.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            mime: mime::TEXT.into(),
            bytes: s.into().into_bytes(),
            alt_text: None,
        }
    }

    /// Rich text with its plain-text version.
    pub fn html(html: impl Into<String>, alt_text: Option<String>) -> Self {
        Self {
            mime: mime::HTML.into(),
            bytes: html.into().into_bytes(),
            alt_text,
        }
    }

    /// A PNG image.
    pub fn png(bytes: Vec<u8>) -> Self {
        Self {
            mime: mime::PNG.into(),
            bytes,
            alt_text: None,
        }
    }

    /// Hash used to recognise the same content again (echo prevention).
    /// Line endings are normalised for text, because Windows turns `\n`
    /// into `\r\n` on the way through its clipboard.
    fn digest(&self) -> [u8; 32] {
        let mut ctx = digest::Context::new(&digest::SHA256);
        ctx.update(self.mime.as_bytes());
        ctx.update(&[0]);
        if self.mime == mime::PNG {
            ctx.update(&self.bytes);
        } else {
            let mut prev_cr = false;
            let mut buf = Vec::with_capacity(self.bytes.len().min(64 * 1024));
            for &b in &self.bytes {
                // `\r\n` and a lone `\r` both count as `\n`.
                match (b, prev_cr) {
                    (b'\n', true) => {}
                    (b'\r', _) => buf.push(b'\n'),
                    _ => buf.push(b),
                }
                prev_cr = b == b'\r';
                if buf.len() >= 64 * 1024 {
                    ctx.update(&buf);
                    buf.clear();
                }
            }
            ctx.update(&buf);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(ctx.finish().as_ref());
        out
    }
}

// Never print content, not even in a debug log by accident.
impl std::fmt::Debug for ClipData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipData")
            .field("mime", &self.mime)
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Why content was not sent. Never carries the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// "Pause clipboard on all devices" is on (R7.6).
    Paused,
    /// The OS or the source app marked it sensitive (R7.3).
    Sensitive,
    /// Over 10 MB (R7.2).
    TooLarge,
    /// A format other than text, rich text, or PNG.
    Unsupported,
    /// No connected device takes clipboard content.
    NoDevice,
}

/// What the UI is told. Never carries clipboard content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipEvent {
    /// An item from `peer` was written to this device's clipboard.
    Received {
        /// The sender.
        peer: DeviceId,
        /// Its MIME type.
        mime: String,
        /// Its size in bytes.
        len: usize,
    },
    /// An item was delivered to `peer`.
    Sent {
        /// The receiver.
        peer: DeviceId,
        /// Its MIME type.
        mime: String,
        /// Its size in bytes.
        len: usize,
    },
    /// Content was not sent.
    Skipped(SkipReason),
    /// `peer` did not take an item: `denied`, `declined`, `timeout`, ...
    Refused {
        /// The device that refused.
        peer: DeviceId,
        /// A short reason.
        reason: String,
    },
    /// The pause or a per-device switch changed.
    SettingsChanged,
}

/// What the platform provides: write to the system clipboard.
///
/// Called from the node's threads, one item at a time. A back-end that can
/// block (Windows opens the clipboard) is fine: the service calls it from a
/// blocking task. The [`Grant`] proves the permission engine allowed the
/// write.
pub trait ClipboardHost: Send + Sync {
    /// Replace the clipboard content.
    fn write(&self, grant: &Grant, data: &ClipData) -> Result<()>;
}

/// What the UI listens to.
pub trait ClipboardEvents: Send + Sync {
    /// Something happened. Runs on a core thread; hand UI work to the UI
    /// thread.
    fn on_event(&self, event: &ClipEvent);
}

/// The result of handing content to the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submit {
    /// Delivery to this many devices started.
    Sending(usize),
    /// It was not sent, for this reason.
    Skipped(SkipReason),
    /// The content is what this device just received or sent. Not sent again.
    Echo,
}

struct Outgoing {
    id: [u8; 16],
    data: ClipData,
}

#[derive(Default)]
struct State {
    /// Hashes of what this device wrote (received from a peer), received, and
    /// sent last (R7.4). Written and received are the same set in practice;
    /// both are kept so a second item arriving first does not reopen the loop.
    last_written: Option<[u8; 32]>,
    last_received: Option<[u8; 32]>,
    last_sent: Option<[u8; 32]>,
    seen_ids: VecDeque<[u8; 16]>,
    rate: HashMap<DeviceId, VecDeque<Instant>>,
    /// The newest item and when it was handed over, for a retry after `ask`.
    latest: Option<(Arc<Outgoing>, Instant)>,
    /// Peers with a prompt in flight, so a burst of copies asks once.
    asking: HashSet<DeviceId>,
    /// Per-device switches read so far: `false` means off.
    peers: HashMap<DeviceId, bool>,
}

/// The clipboard service.
#[derive(Default)]
pub struct Clipboard {
    ctx: OnceLock<ServiceCtx>,
    /// Writes to the system clipboard. Set by the platform.
    pub host: Slot<dyn ClipboardHost>,
    /// Receives events for the UI.
    pub events: Slot<dyn ClipboardEvents>,
    paused: AtomicBool,
    state: Mutex<State>,
    /// This service as the node holds it, for tasks that outlive a call.
    me: OnceLock<std::sync::Weak<Clipboard>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn random_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    // A failing CSPRNG is a broken machine; an all-zero id only costs the
    // repeat filter.
    let _ = SystemRandom::new().fill(&mut id);
    id
}

/// True for the MIME types R7.2 allows.
fn mime_allowed(m: &str) -> bool {
    matches!(m, mime::TEXT | mime::HTML | mime::PNG)
}

/// Checks an incoming item before it reaches the clipboard. The item is
/// hostile: size, type, encoding, and for a PNG its header.
fn validate(data: &ClipData) -> Result<()> {
    if !mime_allowed(&data.mime) {
        return Err(CoreError::Protocol("clipboard type".into()));
    }
    if data.bytes.len() > MAX_CLIP_BYTES {
        return Err(CoreError::Protocol("clipboard item too large".into()));
    }
    if let Some(alt) = &data.alt_text
        && (alt.len() > MAX_ALT_TEXT_BYTES || data.mime != mime::HTML)
    {
        return Err(CoreError::Protocol("clipboard alt text".into()));
    }
    match data.mime.as_str() {
        mime::PNG => {
            let (w, h) = png_size(&data.bytes)
                .ok_or_else(|| CoreError::Protocol("clipboard image is not a PNG".into()))?;
            if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
                return Err(CoreError::Protocol("clipboard image size".into()));
            }
        }
        _ => {
            if std::str::from_utf8(&data.bytes).is_err() {
                return Err(CoreError::Protocol("clipboard text is not UTF-8".into()));
            }
        }
    }
    Ok(())
}

/// Width and height from the PNG header, or `None` when it is not a PNG.
pub fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    const SIG: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if b.len() < 33 || &b[..8] != SIG || &b[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(b[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(b[20..24].try_into().ok()?);
    Some((w, h))
}

impl Clipboard {
    /// A new service. [`register`] and tests use this, not `default()`, so
    /// the service can hand itself to its background tasks.
    pub fn new() -> Arc<Self> {
        let me = Arc::new(Self::default());
        let _ = me.me.set(Arc::downgrade(&me));
        me
    }

    fn arc(&self) -> Option<Arc<Self>> {
        self.me.get()?.upgrade()
    }

    fn ctx(&self) -> Result<&ServiceCtx> {
        self.ctx.get().ok_or(CoreError::Stopped)
    }

    /// The service's node handle, so tests can send hostile messages as this
    /// service would.
    #[doc(hidden)]
    pub fn test_ctx(&self) -> Result<&ServiceCtx> {
        self.ctx()
    }

    fn emit(&self, e: ClipEvent) {
        if let Some(events) = self.events.get() {
            events.on_event(&e);
        }
    }

    // -- settings (R7.6) ------------------------------------------------------

    /// True while "Pause clipboard on all devices" is on. Nothing is sent
    /// and nothing received is written.
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Turn "Pause clipboard on all devices" on or off.
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.ctx()?
            .set_setting("paused", if paused { "1" } else { "0" })?;
        self.paused.store(paused, Ordering::Relaxed);
        self.emit(ClipEvent::SettingsChanged);
        self.ctx()?.changed();
        Ok(())
    }

    /// True when clipboard sync with `peer` is on (the default).
    pub fn peer_enabled(&self, peer: DeviceId) -> bool {
        if let Some(on) = lock(&self.state).peers.get(&peer) {
            return *on;
        }
        let on = self
            .ctx
            .get()
            .and_then(|c| c.setting(&format!("peer.{}", peer.hex())))
            .is_none_or(|v| v != "0");
        lock(&self.state).peers.insert(peer, on);
        on
    }

    /// Turn clipboard sync with one device on or off, in both directions.
    pub fn set_peer_enabled(&self, peer: DeviceId, enabled: bool) -> Result<()> {
        self.ctx()?.set_setting(
            &format!("peer.{}", peer.hex()),
            if enabled { "1" } else { "0" },
        )?;
        lock(&self.state).peers.insert(peer, enabled);
        self.emit(ClipEvent::SettingsChanged);
        self.ctx()?.changed();
        Ok(())
    }

    /// Whether items to and from `peer` may flow at all: not paused, switch on.
    fn open_for(&self, peer: DeviceId) -> bool {
        !self.paused() && self.peer_enabled(peer)
    }

    /// Connected devices this one may send clipboard content to.
    pub fn targets(&self) -> Vec<DeviceId> {
        let Ok(ctx) = self.ctx() else {
            return Vec::new();
        };
        ctx.connected()
            .into_iter()
            .filter(|p| ctx.peer_offers(*p) && self.open_for(*p))
            .filter(|p| ctx.remote_mode(*p) != Some(Mode::Deny))
            .collect()
    }

    // -- sending ---------------------------------------------------------------

    /// New content appeared on this device's clipboard (desktop watcher).
    /// `sensitive` is the OS or source app's own mark (R7.3). Content that
    /// this device just received is not sent back (R7.4).
    pub fn local_changed(&self, data: ClipData, sensitive: bool) -> Submit {
        self.submit(data, sensitive, None, true)
    }

    /// The user asked to send content now (Android notification action, Quick
    /// Settings tile, share sheet). `peer` is one device, or `None` for every
    /// connected device. The sensitive mark is still honoured.
    pub fn send_manual(&self, data: ClipData, sensitive: bool, peer: Option<DeviceId>) -> Submit {
        self.submit(data, sensitive, peer, false)
    }

    /// Hand content over to send. `automatic` content (a change noticed
    /// without a user action) is checked against what this device just
    /// received or sent, so it is not echoed (R7.4). `only` limits delivery
    /// to one device.
    pub fn submit(
        &self,
        data: ClipData,
        sensitive: bool,
        only: Option<DeviceId>,
        automatic: bool,
    ) -> Submit {
        let skip = |r: SkipReason| {
            self.emit(ClipEvent::Skipped(r));
            Submit::Skipped(r)
        };
        if self.paused() {
            return skip(SkipReason::Paused);
        }
        if sensitive {
            return skip(SkipReason::Sensitive);
        }
        if !mime_allowed(&data.mime) || data.bytes.is_empty() {
            return skip(SkipReason::Unsupported);
        }
        if data.bytes.len() > MAX_CLIP_BYTES {
            return skip(SkipReason::TooLarge);
        }
        if data.mime == mime::PNG && png_size(&data.bytes).is_none() {
            return skip(SkipReason::Unsupported);
        }
        let hash = data.digest();
        {
            let mut st = lock(&self.state);
            if automatic && [st.last_written, st.last_received, st.last_sent].contains(&Some(hash))
            {
                return Submit::Echo;
            }
            st.last_sent = Some(hash);
        }
        let mut targets = self.targets();
        if let Some(p) = only {
            targets.retain(|t| *t == p);
        }
        if targets.is_empty() {
            return skip(SkipReason::NoDevice);
        }
        let item = Arc::new(Outgoing {
            id: random_id(),
            data,
        });
        lock(&self.state).latest = Some((item.clone(), Instant::now()));
        for peer in &targets {
            self.deliver(*peer, item.clone(), false);
        }
        Submit::Sending(targets.len())
    }

    /// Deliver one item to one peer in the background. Under Always Ask the
    /// peer's user approves first; `force_ask` does that even when the last
    /// `PermissionState` said Allow.
    fn deliver(&self, peer: DeviceId, item: Arc<Outgoing>, force_ask: bool) {
        let (Ok(ctx), Some(me)) = (self.ctx(), self.arc()) else {
            return;
        };
        let ask = force_ask || ctx.remote_mode(peer) == Some(Mode::Ask);
        if ask && !lock(&self.state).asking.insert(peer) {
            // A prompt is already open on that device. It gets the newest
            // item once approved (see `send_item`).
            return;
        }
        let task_ctx = ctx.clone();
        ctx.spawn(async move {
            let result = send_item(&me, &task_ctx, peer, item, ask).await;
            if ask {
                lock(&me.state).asking.remove(&peer);
            }
            me.emit(match result {
                Ok(sent) => ClipEvent::Sent {
                    peer,
                    mime: sent.data.mime.clone(),
                    len: sent.data.bytes.len(),
                },
                Err(reason) => ClipEvent::Refused { peer, reason },
            });
        });
    }

    // -- receiving ---------------------------------------------------------------

    /// Check the pause, the per-device switch, the rate, and the repeat
    /// filter. False means drop the item quietly.
    fn admit(&self, peer: DeviceId, id: &[u8]) -> bool {
        if !self.open_for(peer) {
            return false;
        }
        let Ok(id) = <[u8; 16]>::try_from(id) else {
            return false;
        };
        let now = Instant::now();
        let mut st = lock(&self.state);
        let q = st.rate.entry(peer).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) > RATE_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= RATE_LIMIT {
            return false;
        }
        q.push_back(now);
        if st.seen_ids.contains(&id) {
            return false;
        }
        st.seen_ids.push_back(id);
        if st.seen_ids.len() > SEEN_IDS {
            st.seen_ids.pop_front();
        }
        true
    }

    /// Write a validated item from `peer` to the clipboard.
    async fn apply(self: Arc<Self>, peer: DeviceId, grant: Grant, data: ClipData) {
        let Some(host) = self.host.get() else { return };
        let hash = data.digest();
        {
            // Remember it before the write, so the watcher's event for our own
            // write finds it.
            let mut st = lock(&self.state);
            st.last_written = Some(hash);
            st.last_received = Some(hash);
        }
        let (mime, len) = (data.mime.clone(), data.bytes.len());
        let r = tokio::task::spawn_blocking(move || host.write(&grant, &data)).await;
        match r {
            Ok(Ok(())) => self.emit(ClipEvent::Received { peer, mime, len }),
            Ok(Err(e)) => tracing::debug!(error = %e, "clipboard write failed"),
            Err(_) => tracing::debug!("clipboard write task failed"),
        }
    }
}

/// Send `item` to `peer`, asking its user first when `ask` is set. Returns
/// the item that was sent (after an approval that took a while, the newest
/// one), or the short reason shown to the user.
async fn send_item(
    me: &Clipboard,
    ctx: &ServiceCtx,
    peer: DeviceId,
    item: Arc<Outgoing>,
    ask: bool,
) -> std::result::Result<Arc<Outgoing>, String> {
    let origin = ctx.this_device().map(|d| d.0.to_vec()).unwrap_or_default();
    let (activity, item) = if ask {
        let activity = ctx.request(peer).await.map_err(|e| match e {
            CoreError::Refused(r) => r,
            _ => "offline".to_owned(),
        })?;
        // The user may have copied something newer while the prompt was open.
        let newest = lock(&me.state).latest.as_ref().map(|(i, _)| i.clone());
        (activity, newest.unwrap_or(item))
    } else {
        (0, item)
    };
    let result = if item.data.bytes.len() <= MAX_INLINE_BYTES {
        let clip = Clip {
            id: item.id.to_vec(),
            mime: item.data.mime.clone(),
            bytes: item.data.bytes.clone(),
            origin_device: origin,
            alt_text: item.data.alt_text.clone().unwrap_or_default(),
        };
        ctx.send(peer, Direction::ToHost, activity, &clip)
            .map_err(|_| "offline".to_owned())
    } else {
        send_stream(ctx, peer, activity, &item, origin).await
    };
    if activity != 0 {
        // One approval covers one item.
        ctx.stop(peer, activity);
    }
    result.map(|()| item)
}

async fn send_stream(
    ctx: &ServiceCtx,
    peer: DeviceId,
    activity: u64,
    item: &Outgoing,
    origin: Vec<u8>,
) -> std::result::Result<(), String> {
    let header = ClipHeader {
        id: item.id.to_vec(),
        mime: item.data.mime.clone(),
        size: item.data.bytes.len() as u64,
        sha256: digest::digest(&digest::SHA256, &item.data.bytes)
            .as_ref()
            .to_vec(),
        origin_device: origin,
    };
    let mut send = ctx
        .open_uni(
            peer,
            Direction::ToHost,
            activity,
            STREAM_CLIP,
            prost::Message::encode_to_vec(&header),
        )
        .await
        .map_err(|_| "offline".to_owned())?;
    let run = async {
        send.write_all(&item.data.bytes).await.map_err(|_| ())?;
        send.finish().map_err(|_| ())?;
        // Wait until the peer has read it all, or reset the stream.
        match send.stopped().await {
            Ok(None) => Ok(()),
            _ => Err(()),
        }
    };
    match timeout(STREAM_TIMEOUT, run).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(())) => Err("refused".to_owned()),
        Err(_) => {
            let _ = send.reset(super::stream_code::CANCELLED.into());
            Err("timeout".to_owned())
        }
    }
}

impl Service for Clipboard {
    fn id(&self) -> ServiceId {
        ServiceId::CLIPBOARD
    }

    fn capability(&self) -> Capability {
        Capability::Clipboard
    }

    fn start(&self, ctx: ServiceCtx) {
        if ctx.setting("paused").as_deref() == Some("1") {
            self.paused.store(true, Ordering::Relaxed);
        }
        let _ = self.ctx.set(ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        // Untrusted: decode with limits; a malformed body closes the session.
        let clip: Clip = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad Clip".into()))?;
        // Only content sent to this device as the host is ever written.
        let Access::Host(grant) = msg.access else {
            return Ok(());
        };
        if clip.bytes.len() > MAX_INLINE_BYTES || clip.id.len() != 16 {
            return Err(CoreError::Protocol("bad Clip".into()));
        }
        let data = ClipData {
            mime: clip.mime,
            bytes: clip.bytes,
            alt_text: (!clip.alt_text.is_empty()).then_some(clip.alt_text),
        };
        if let Err(e) = validate(&data) {
            // Not a violation: a newer peer may send a type this build does
            // not know. It is simply not written.
            tracing::debug!(peer = %msg.peer.short_hex(), error = %e, "clipboard item dropped");
            return Ok(());
        }
        if !self.admit(msg.peer, &clip.id) {
            return Ok(());
        }
        let Some(this) = self.arc() else {
            return Ok(());
        };
        ctx.spawn(this.apply(msg.peer, grant, data));
        Ok(())
    }

    fn on_stream(&self, ctx: &ServiceCtx, stream: Stream) {
        let Stream {
            peer,
            access,
            kind,
            meta,
            mut recv,
            ..
        } = stream;
        let Access::Host(grant) = access else { return };
        let refuse = |recv: &mut quinn::RecvStream| {
            let _ = recv.stop(super::stream_code::REFUSED.into());
        };
        let header = prost::Message::decode(meta.as_slice())
            .ok()
            .filter(|_| kind == STREAM_CLIP);
        let Some(header): Option<ClipHeader> = header else {
            return refuse(&mut recv);
        };
        let size = usize::try_from(header.size).unwrap_or(usize::MAX);
        let sha = <[u8; 32]>::try_from(header.sha256.as_slice()).ok();
        let (Some(sha), Some(this)) = (sha, self.arc()) else {
            return refuse(&mut recv);
        };
        if size == 0
            || size > MAX_CLIP_BYTES
            || !mime_allowed(&header.mime)
            || !self.admit(peer, &header.id)
        {
            return refuse(&mut recv);
        }
        let mime = header.mime;
        ctx.spawn(async move {
            let body = match timeout(STREAM_TIMEOUT, recv.read_to_end(size)).await {
                Ok(Ok(b)) if b.len() == size => b,
                _ => {
                    let _ = recv.stop(super::stream_code::CANCELLED.into());
                    return;
                }
            };
            if digest::digest(&digest::SHA256, &body).as_ref() != sha {
                tracing::debug!(peer = %peer.short_hex(), "clipboard stream failed its hash");
                return;
            }
            let data = ClipData {
                mime,
                bytes: body,
                alt_text: None,
            };
            if validate(&data).is_ok() {
                this.apply(peer, grant, data).await;
            }
        });
    }

    fn on_refused(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        // The host's setting is Always Ask but our copy said Allow (it changed
        // a moment ago): ask once and send the newest item.
        if reason == "ask" && activity == 0 {
            let item = {
                let st = lock(&self.state);
                st.latest
                    .as_ref()
                    .filter(|(_, at)| at.elapsed() < RETRY_WINDOW)
                    .map(|(i, _)| i.clone())
            };
            if let Some(item) = item {
                self.deliver(peer, item, true);
            }
            return;
        }
        self.emit(ClipEvent::Refused {
            peer,
            reason: reason.to_owned(),
        });
    }

    fn on_peer(&self, _ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if !connected {
            lock(&self.state).asking.remove(&peer);
        }
    }
}

pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Clipboard::new());
}
