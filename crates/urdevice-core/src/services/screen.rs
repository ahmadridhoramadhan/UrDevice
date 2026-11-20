//! Screen sharing (#21, design §10, R12).
//!
//! Service id: [`ServiceId::SCREEN`], capability `screen.h264.v1` (Release 1,
//! default Always Ask, watched: every session is an activity, so the host
//! shows the indicator and Stop, R5.4).
//!
//! **Viewer (controller).** The UI calls [`Screen::open`], which asks the
//! host for an activity (the host allows, prompts, or refuses). With the
//! activity it calls [`Screen::list_displays`] and [`Screen::start`]. The host
//! opens one unidirectional video stream per start; the service reads it with
//! size limits and hands each [`VideoFrame`] to [`ScreenEvents::frame`]. The
//! UI decodes and draws it and never acts on it. When the decoder loses sync
//! it calls [`Screen::request_keyframe`]. [`Screen::stop`] ends the activity.
//!
//! **Host.** A [`Start`] that passed the permission check reaches the
//! [`ScreenHost`] with its [`Grant`]. The host adapter captures and encodes
//! (desktop: `urdevice-platform` + `urdevice-media`; Android: MediaProjection
//! and MediaCodec) and pushes encoded frames into a [`FrameSink`]. The sink
//! keeps at most [`MAX_QUEUED_FRAMES`] frames waiting for the network: a frame
//! that does not fit is dropped and the encoder is asked for a keyframe, and
//! every frame up to that keyframe is dropped too, so the viewer never gets a
//! broken reference chain (design §10, sender-side frame dropping). Every
//! [`ADAPT_EVERY`] the session feeds the drop counts to a [`Ladder`], which
//! moves between bitrate and resolution steps (R12.4).
//!
//! Frames are never stored or logged; tracing carries ids and sizes only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, mpsc};
use urdevice_proto::features::screen::{
    self as wire, FRAME_PREFIX, ListDisplays, MAX_DISPLAYS, RequestKeyframe, STREAM_VIDEO,
    Screen as WireScreen, ScreenError, StreamInfo, message::Msg,
};

use super::{
    Access, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Slot, Stream,
    stream_code,
};
use crate::identity::DeviceId;
use crate::permission::Capability;
use crate::{CoreError, Result};

pub use urdevice_proto::features::screen::{Display, Start, VideoFrame};

/// Release 2 capabilities this module ships. Screen is a Release 1
/// capability, which is always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// Most encoded frames waiting for the network per session. Design §10:
/// drop when the send buffer holds more than about 2 frames.
pub const MAX_QUEUED_FRAMES: usize = 2;

/// How often a session feeds its counters to the [`Ladder`].
pub const ADAPT_EVERY: Duration = Duration::from_millis(500);

/// Fewest milliseconds between two keyframes a viewer can ask for. A
/// keyframe costs the host CPU and bandwidth; a hostile viewer must not be
/// able to ask for one per packet.
pub const KEYFRAME_MIN_GAP: Duration = Duration::from_millis(250);

/// Frame rate a host uses when the viewer does not ask for one.
pub const DEFAULT_FPS: u32 = 30;

/// Most video sessions a host runs at once, over all viewers.
pub const MAX_HOST_SESSIONS: usize = 4;

/// What the encoder should aim for. The adapter scales the display to fit
/// inside `max_width` x `max_height`, keeping its aspect ratio
/// ([`VideoTarget::fit`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoTarget {
    /// Largest width to encode.
    pub max_width: u32,
    /// Largest height to encode.
    pub max_height: u32,
    /// Frames per second.
    pub fps: u32,
    /// Target bitrate, bits per second (constant bitrate, design §10).
    pub bitrate_bps: u32,
}

impl VideoTarget {
    /// The encode size for a `width` x `height` source: scaled down (never
    /// up) to fit the target, keeping the aspect ratio, rounded down to even
    /// numbers as H.264 with 4:2:0 chroma needs. Never below 16 x 16.
    pub fn fit(&self, width: u32, height: u32) -> (u32, u32) {
        let (w, h) = (width.max(2), height.max(2));
        let mw = self.max_width.max(16);
        let mh = self.max_height.max(16);
        let scale = f64::min(1.0, f64::min(mw as f64 / w as f64, mh as f64 / h as f64));
        let sw = ((w as f64 * scale) as u32).max(16) & !1;
        let sh = ((h as f64 * scale) as u32).max(16) & !1;
        (sw, sh)
    }
}

/// One step of the adaptive ladder (design §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// Box the picture is scaled into.
    pub width: u32,
    /// Box height.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
    /// Bits per second.
    pub bitrate_bps: u32,
}

/// The ladder, best step first: 1080p30 at 8 Mbps, 720p30 at 4 Mbps, 540p24
/// at 2 Mbps.
pub const STEPS: [Step; 3] = [
    Step {
        width: 1920,
        height: 1080,
        fps: 30,
        bitrate_bps: 8_000_000,
    },
    Step {
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_bps: 4_000_000,
    },
    Step {
        width: 960,
        height: 540,
        fps: 24,
        bitrate_bps: 2_000_000,
    },
];

/// Share of frames dropped in one window that moves the ladder down.
const DOWN_RATIO: f64 = 0.10;
/// Time without drops before the ladder moves up again.
const UP_AFTER: Duration = Duration::from_secs(5);
/// Time after a change before the ladder moves down again, so the encoder's
/// new rate shows in the counters first.
const SETTLE: Duration = Duration::from_secs(1);

/// The adaptive bitrate and resolution ladder (R12.4). Pure logic: feed it
/// the frames sent and dropped since the last call.
#[derive(Debug, Clone)]
pub struct Ladder {
    step: usize,
    /// Viewer limits applied to every step.
    max_width: u32,
    max_height: u32,
    max_fps: u32,
    stable_since: Instant,
    changed_at: Instant,
}

impl Ladder {
    /// Start at the best step, capped by what the viewer asked for (0 = no
    /// limit).
    pub fn new(now: Instant, max_width: u32, max_height: u32, max_fps: u32) -> Self {
        Self {
            step: 0,
            max_width,
            max_height,
            max_fps,
            stable_since: now,
            changed_at: now,
        }
    }

    /// Index of the current step in [`STEPS`].
    pub fn index(&self) -> usize {
        self.step
    }

    /// The current target.
    pub fn target(&self) -> VideoTarget {
        let s = STEPS[self.step];
        let cap = |v: u32, max: u32| if max == 0 { v } else { v.min(max) };
        VideoTarget {
            max_width: cap(s.width, self.max_width),
            max_height: cap(s.height, self.max_height),
            fps: cap(s.fps, self.max_fps).max(1),
            bitrate_bps: s.bitrate_bps,
        }
    }

    /// Feed one window's counters. Returns the new target when the step
    /// changed.
    pub fn observe(&mut self, now: Instant, sent: u64, dropped: u64) -> Option<VideoTarget> {
        let total = sent + dropped;
        let ratio = if total == 0 {
            0.0
        } else {
            dropped as f64 / total as f64
        };
        if dropped > 0 {
            self.stable_since = now;
        }
        if ratio >= DOWN_RATIO
            && self.step + 1 < STEPS.len()
            && now.duration_since(self.changed_at) >= SETTLE
        {
            self.step += 1;
            self.changed_at = now;
            return Some(self.target());
        }
        if dropped == 0 && self.step > 0 && now.duration_since(self.stable_since) >= UP_AFTER {
            self.step -= 1;
            self.changed_at = now;
            self.stable_since = now;
            return Some(self.target());
        }
        None
    }
}

/// An encoded frame from a host adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Capture time in microseconds, from any fixed start.
    pub pts_us: u64,
    /// True for an IDR frame with SPS and PPS in front of it.
    pub keyframe: bool,
    /// H.264 Annex B NAL units.
    pub data: Vec<u8>,
    /// Encoded width.
    pub width: u32,
    /// Encoded height.
    pub height: u32,
}

/// Where a host adapter puts encoded frames. Made by the service.
pub trait FrameSink: Send + Sync {
    /// False while the network is behind: skip capturing or encoding this
    /// frame (cheaper than encoding a frame that is dropped).
    fn ready(&self) -> bool;
    /// Hand over one encoded frame. Never blocks; drops when full.
    fn push(&self, frame: EncodedFrame);
    /// Capture stopped on its own (the OS revoked it, the display went
    /// away). `reason` is one of the [`ScreenError`] reasons.
    fn failed(&self, reason: &str);
}

/// A running capture, returned by [`ScreenHost::start`].
pub trait Capture: Send + Sync {
    /// Make the next frame a keyframe.
    fn request_keyframe(&self);
    /// Change size, frame rate, or bitrate (the ladder moved).
    fn set_target(&self, target: VideoTarget);
    /// Stop capturing and release the OS resources. Called once.
    fn stop(&self);
}

/// What the host OS provides. Desktop: `urdevice-desktop` glues
/// `urdevice-platform` capture to an `urdevice-media` encoder. Android:
/// Kotlin through the FFI.
///
/// Both methods may block (a portal or system consent dialog); the service
/// calls them on the blocking pool.
pub trait ScreenHost: Send + Sync {
    /// The displays that can be shared.
    fn displays(&self) -> Vec<Display>;

    /// Start capturing `display_id`. `grant` is proof the permission engine
    /// allowed this peer; an adapter never captures without one. Errors use
    /// [`CoreError::Refused`] with a [`ScreenError`] reason
    /// (`consent_declined`, `no_display`, `capture_failed`).
    fn start(
        &self,
        grant: &Grant,
        display_id: u32,
        target: VideoTarget,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Arc<dyn Capture>>;
}

/// What the viewer UI listens to. Called from core threads; hand work to
/// the UI thread. Never act on the device because of these.
pub trait ScreenEvents: Send + Sync {
    /// [`Screen::request_view`] got an activity: the host allowed it.
    fn opened(&self, peer: DeviceId, activity: u64);
    /// The host's display list.
    fn displays(&self, peer: DeviceId, activity: u64, displays: Vec<Display>);
    /// A new video stream began (first start or a display switch): reset the
    /// decoder.
    fn stream_started(&self, peer: DeviceId, activity: u64, display_id: u32);
    /// One frame. Keep the queue behind this bounded; when it overflows,
    /// drop frames and call [`Screen::request_keyframe`].
    fn frame(&self, peer: DeviceId, activity: u64, frame: VideoFrame);
    /// The host reported an error, or refused a message.
    fn error(&self, peer: DeviceId, activity: u64, reason: &str);
    /// The session ended (Stop on either device, Deny, disconnect).
    fn ended(&self, peer: DeviceId, activity: u64);
}

// ---------------------------------------------------------------------------
// Host side
// ---------------------------------------------------------------------------

/// Counters and queue of one host session, shared with the adapter.
struct Sink {
    tx: mpsc::Sender<EncodedFrame>,
    need_key: AtomicBool,
    closed: AtomicBool,
    sent: AtomicU64,
    dropped: AtomicU64,
    /// Wakes the session task: a keyframe is needed, or capture failed.
    kick: Notify,
    failure: Mutex<Option<String>>,
}

impl FrameSink for Sink {
    fn ready(&self) -> bool {
        !self.closed.load(Ordering::Relaxed) && self.tx.capacity() > 0
    }

    fn push(&self, frame: EncodedFrame) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        if self.need_key.load(Ordering::Relaxed) && !frame.keyframe {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let key = frame.keyframe;
        match self.tx.try_send(frame) {
            Ok(()) => {
                if key {
                    self.need_key.store(false, Ordering::Relaxed);
                }
                self.sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                self.need_key.store(true, Ordering::Relaxed);
                self.kick.notify_one();
            }
        }
    }

    fn failed(&self, reason: &str) {
        *lock(&self.failure) = Some(reason.chars().take(wire::MAX_REASON).collect());
        self.kick.notify_one();
    }
}

struct HostSession {
    /// Distinguishes a display switch from the session it replaced.
    generation: u64,
    stop: Arc<Notify>,
    capture: Option<Arc<dyn Capture>>,
    last_keyframe: Option<Instant>,
}

// ---------------------------------------------------------------------------
// Viewer side
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Viewing {
    /// Bumped for each new video stream; older readers stop delivering.
    generation: u64,
}

#[derive(Default)]
struct State {
    host: HashMap<(DeviceId, u64), HostSession>,
    next_generation: u64,
    viewing: HashMap<(DeviceId, u64), Viewing>,
}

/// The screen service. UI code reaches it with `node.service::<Screen>()`.
#[derive(Default)]
pub struct Screen {
    ctx: OnceLock<ServiceCtx>,
    /// The capture adapter, on devices that can be viewed.
    pub host: Slot<dyn ScreenHost>,
    /// The viewer UI.
    pub events: Slot<dyn ScreenEvents>,
    state: Arc<Mutex<State>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

impl Screen {
    fn ctx(&self) -> Result<&ServiceCtx> {
        self.ctx.get().ok_or(CoreError::Stopped)
    }

    /// Ask `peer` to share its screen. Resolves when the host allowed it
    /// (possibly after its user answered a prompt); returns the activity id
    /// for the other calls.
    pub async fn open(&self, peer: DeviceId) -> Result<u64> {
        let ctx = self.ctx()?.clone();
        let activity = ctx.request(peer).await?;
        lock(&self.state)
            .viewing
            .insert((peer, activity), Viewing::default());
        Ok(activity)
    }

    /// Like [`Screen::open`], for UIs without an async runtime: the result
    /// arrives in [`ScreenEvents::opened`], or in [`ScreenEvents::error`]
    /// with activity 0 and `denied`, `declined`, `timeout`, `unsupported`,
    /// or `offline`.
    pub fn request_view(self: &Arc<Self>, peer: DeviceId) -> Result<()> {
        let ctx = self.ctx()?.clone();
        let this = self.clone();
        ctx.spawn(async move {
            let r = this.open(peer).await;
            let Some(e) = this.events() else { return };
            match r {
                Ok(activity) => e.opened(peer, activity),
                Err(CoreError::Refused(reason)) => e.error(peer, 0, &reason),
                Err(_) => e.error(peer, 0, "offline"),
            }
        });
        Ok(())
    }

    /// Ask the host for its displays; the answer arrives in
    /// [`ScreenEvents::displays`].
    pub fn list_displays(&self, peer: DeviceId, activity: u64) -> Result<()> {
        self.send(peer, activity, Msg::ListDisplays(ListDisplays {}))
    }

    /// Start (or switch) the video stream. 0 means no limit / host default.
    pub fn start(
        &self,
        peer: DeviceId,
        activity: u64,
        display_id: u32,
        max_width: u32,
        max_height: u32,
        max_fps: u32,
    ) -> Result<()> {
        let mut s = Start {
            display_id,
            max_width,
            max_height,
            max_fps,
        };
        s.sanitize();
        self.send(peer, activity, Msg::Start(s))
    }

    /// Ask for a keyframe after the decoder lost sync or dropped frames.
    pub fn request_keyframe(&self, peer: DeviceId, activity: u64) -> Result<()> {
        self.send(peer, activity, Msg::RequestKeyframe(RequestKeyframe {}))
    }

    /// End the session on both devices.
    pub fn stop(&self, peer: DeviceId, activity: u64) {
        if let Ok(ctx) = self.ctx() {
            ctx.stop(peer, activity);
        }
    }

    /// True while this device shares its screen with anyone.
    pub fn is_sharing(&self) -> bool {
        !lock(&self.state).host.is_empty()
    }

    fn send(&self, peer: DeviceId, activity: u64, msg: Msg) -> Result<()> {
        if !lock(&self.state).viewing.contains_key(&(peer, activity)) {
            return Err(CoreError::InvalidInput("no such screen session".into()));
        }
        self.ctx()?
            .send(peer, Direction::ToHost, activity, &WireScreen::from(msg))
    }

    fn events(&self) -> Option<Arc<dyn ScreenEvents>> {
        self.events.get()
    }

    // --- host -------------------------------------------------------------

    fn reply_error(ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        let msg = WireScreen::from(Msg::Error(ScreenError {
            reason: reason.into(),
        }));
        let _ = ctx.send(peer, Direction::ToController, activity, &msg);
    }

    fn host_list(&self, ctx: &ServiceCtx, grant: Grant) {
        let Some(host) = self.host.get() else {
            Self::reply_error(ctx, grant.peer(), grant.activity(), "no_host");
            return;
        };
        let ctx2 = ctx.clone();
        ctx.spawn(async move {
            let list = tokio::task::spawn_blocking(move || host.displays())
                .await
                .unwrap_or_default();
            let mut d = wire::Displays { displays: list };
            d.displays.truncate(MAX_DISPLAYS);
            d.sanitize();
            let _ = ctx2.send(
                grant.peer(),
                Direction::ToController,
                grant.activity(),
                &WireScreen::from(Msg::Displays(d)),
            );
        });
    }

    fn host_start(&self, ctx: &ServiceCtx, grant: Grant, mut start: Start) {
        start.sanitize();
        let (peer, activity) = (grant.peer(), grant.activity());
        let Some(host) = self.host.get() else {
            Self::reply_error(ctx, peer, activity, "no_host");
            return;
        };
        // One stream per activity: a new Start replaces the old one.
        let generation = {
            let mut st = lock(&self.state);
            if let Some(old) = st.host.remove(&(peer, activity)) {
                // Its task stops the capture and finishes the old stream.
                old.stop.notify_one();
            }
            if st.host.len() >= MAX_HOST_SESSIONS {
                drop(st);
                Self::reply_error(ctx, peer, activity, "capture_failed");
                return;
            }
            st.next_generation += 1;
            let generation = st.next_generation;
            st.host.insert(
                (peer, activity),
                HostSession {
                    generation,
                    stop: Arc::new(Notify::new()),
                    capture: None,
                    last_keyframe: None,
                },
            );
            generation
        };
        let stop = lock(&self.state)
            .host
            .get(&(peer, activity))
            .map(|s| s.stop.clone());
        let Some(stop) = stop else { return };
        tracing::info!(
            activity,
            display = start.display_id,
            "screen session starting"
        );
        let state = self.state.clone();
        let ctx2 = ctx.clone();
        ctx.spawn(run_host_session(
            ctx2, state, host, grant, start, generation, stop,
        ));
    }

    fn host_keyframe(&self, grant: &Grant) {
        let mut st = lock(&self.state);
        let Some(s) = st.host.get_mut(&(grant.peer(), grant.activity())) else {
            return;
        };
        let now = Instant::now();
        if s.last_keyframe
            .is_some_and(|t| now.duration_since(t) < KEYFRAME_MIN_GAP)
        {
            return;
        }
        s.last_keyframe = Some(now);
        if let Some(c) = s.capture.clone() {
            drop(st);
            c.request_keyframe();
        }
    }

    // --- viewer -----------------------------------------------------------

    fn viewer_stream(&self, ctx: &ServiceCtx, mut stream: Stream, activity: u64) {
        let peer = stream.peer;
        let generation = {
            let mut st = lock(&self.state);
            match st.viewing.get_mut(&(peer, activity)) {
                Some(v) if stream.kind == STREAM_VIDEO => {
                    v.generation += 1;
                    Some(v.generation)
                }
                _ => None,
            }
        };
        let Some(generation) = generation else {
            let _ = stream.recv.stop(stream_code::UNSUPPORTED.into());
            return;
        };
        let display_id = prost::Message::decode(stream.meta.as_slice())
            .map(|i: StreamInfo| i.display_id)
            .unwrap_or_default();
        if let Some(e) = self.events() {
            e.stream_started(peer, activity, display_id);
        }
        let state = self.state.clone();
        let events = self.events.get();
        ctx.spawn(async move {
            let mut recv = stream.recv;
            loop {
                let frame = match read_frame(&mut recv).await {
                    Ok(Some(f)) => f,
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!(activity, error = %e, "video stream ended");
                        let _ = recv.stop(stream_code::CANCELLED.into());
                        break;
                    }
                };
                let current = lock(&state)
                    .viewing
                    .get(&(peer, activity))
                    .is_some_and(|v| v.generation == generation);
                if !current {
                    let _ = recv.stop(stream_code::CANCELLED.into());
                    break;
                }
                if let Some(e) = &events {
                    e.frame(peer, activity, frame);
                }
            }
        });
    }
}

/// Read one length-prefixed frame. `Ok(None)` at a clean end of stream.
async fn read_frame(recv: &mut quinn::RecvStream) -> Result<Option<VideoFrame>> {
    let mut prefix = [0u8; FRAME_PREFIX];
    match recv.read_exact(&mut prefix).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(e) => return Err(CoreError::Network(e.to_string())),
    }
    let len = wire::frame_len(prefix).map_err(|e| CoreError::Protocol(e.to_string()))?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body)
        .await
        .map_err(|e| CoreError::Network(e.to_string()))?;
    wire::decode_frame(&body)
        .map(Some)
        .map_err(|e| CoreError::Protocol(e.to_string()))
}

/// One host session: open the stream, start capture, write frames, adapt,
/// and clean up on Stop or any failure.
async fn run_host_session(
    ctx: ServiceCtx,
    state: Arc<Mutex<State>>,
    host: Arc<dyn ScreenHost>,
    grant: Grant,
    start: Start,
    generation: u64,
    stop: Arc<Notify>,
) {
    let (peer, activity) = (grant.peer(), grant.activity());
    let current = |st: &State| {
        st.host
            .get(&(peer, activity))
            .is_some_and(|s| s.generation == generation)
    };
    let meta = prost::Message::encode_to_vec(&StreamInfo {
        display_id: start.display_id,
        codec: "h264".into(),
    });
    let mut send = match ctx
        .open_uni(peer, Direction::ToController, activity, STREAM_VIDEO, meta)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(activity, error = %e, "screen stream did not open");
            remove_if_current(&state, peer, activity, generation);
            return;
        }
    };
    let (tx, mut rx) = mpsc::channel(MAX_QUEUED_FRAMES);
    let sink = Arc::new(Sink {
        tx,
        need_key: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        sent: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        kick: Notify::new(),
        failure: Mutex::new(None),
    });
    let mut ladder = Ladder::new(
        Instant::now(),
        start.max_width,
        start.max_height,
        start.max_fps,
    );
    let target = ladder.target();
    let started = {
        let (host, grant, sink) = (host.clone(), grant.clone(), sink.clone());
        let display = start.display_id;
        tokio::task::spawn_blocking(move || host.start(&grant, display, target, sink)).await
    };
    let capture = match started {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            let reason = match e {
                CoreError::Refused(r) => r,
                _ => "capture_failed".into(),
            };
            tracing::info!(activity, reason = %reason, "screen capture did not start");
            Screen::reply_error(&ctx, peer, activity, &reason);
            let _ = send.finish();
            remove_if_current(&state, peer, activity, generation);
            return;
        }
        Err(_) => {
            Screen::reply_error(&ctx, peer, activity, "capture_failed");
            let _ = send.finish();
            remove_if_current(&state, peer, activity, generation);
            return;
        }
    };
    {
        let mut st = lock(&state);
        if !current(&st) {
            // Stopped while the OS dialog was open.
            drop(st);
            sink.closed.store(true, Ordering::Relaxed);
            capture.stop();
            let _ = send.finish();
            return;
        }
        if let Some(s) = st.host.get_mut(&(peer, activity)) {
            s.capture = Some(capture.clone());
        }
    }
    tracing::info!(activity, "screen session running");

    let mut tick = tokio::time::interval(ADAPT_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_key_kick: Option<Instant> = None;
    let mut written: u64 = 0;
    let end_reason: Option<String> = loop {
        tokio::select! {
            _ = stop.notified() => break None,
            frame = rx.recv() => {
                let Some(f) = frame else { break Some("capture_failed".into()) };
                let bytes = wire::encode_frame(&VideoFrame {
                    pts_us: f.pts_us,
                    keyframe: f.keyframe,
                    width: if f.keyframe { f.width } else { 0 },
                    height: if f.keyframe { f.height } else { 0 },
                    data: f.data,
                });
                if let Err(e) = send.write_all(&bytes).await {
                    tracing::debug!(activity, error = %e, "viewer closed the video stream");
                    break None;
                }
                written += bytes.len() as u64;
            }
            _ = sink.kick.notified() => {
                if let Some(r) = lock(&sink.failure).take() {
                    break Some(r);
                }
                let now = Instant::now();
                if sink.need_key.load(Ordering::Relaxed)
                    && last_key_kick.is_none_or(|t| now.duration_since(t) >= KEYFRAME_MIN_GAP)
                {
                    last_key_kick = Some(now);
                    capture.request_keyframe();
                }
            }
            _ = tick.tick() => {
                let sent = sink.sent.swap(0, Ordering::Relaxed);
                let dropped = sink.dropped.swap(0, Ordering::Relaxed);
                if let Some(t) = ladder.observe(Instant::now(), sent, dropped) {
                    tracing::info!(activity, step = ladder.index(), bitrate = t.bitrate_bps, "screen ladder moved");
                    capture.set_target(t);
                }
                // A keyframe request lost while the encoder was busy.
                if sink.need_key.load(Ordering::Relaxed) {
                    capture.request_keyframe();
                }
            }
        }
    };
    sink.closed.store(true, Ordering::Relaxed);
    capture.stop();
    let _ = send.finish();
    tracing::info!(activity, bytes = written, "screen session ended");
    let still_current = {
        let st = lock(&state);
        current(&st)
    };
    remove_if_current(&state, peer, activity, generation);
    if let Some(reason) = end_reason
        && still_current
    {
        Screen::reply_error(&ctx, peer, activity, &reason);
        // Capture died on its own: end the activity so the indicator goes.
        ctx.stop(peer, activity);
    }
}

fn remove_if_current(state: &Mutex<State>, peer: DeviceId, activity: u64, generation: u64) {
    let mut st = lock(state);
    if st
        .host
        .get(&(peer, activity))
        .is_some_and(|s| s.generation == generation)
    {
        st.host.remove(&(peer, activity));
    }
}

impl Service for Screen {
    fn id(&self) -> ServiceId {
        ServiceId::SCREEN
    }

    fn capability(&self) -> Capability {
        Capability::Screen
    }

    fn start(&self, ctx: ServiceCtx) {
        let _ = self.ctx.set(ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        let body: WireScreen = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad Screen message".into()))?;
        let Some(m) = body.msg else { return Ok(()) };
        match msg.access {
            Access::Host(grant) => {
                // Watched: the node only lets traffic with an activity through.
                if grant.activity() == 0 {
                    return Ok(());
                }
                match m {
                    Msg::ListDisplays(_) => self.host_list(ctx, grant),
                    Msg::Start(s) => self.host_start(ctx, grant, s),
                    Msg::RequestKeyframe(_) => self.host_keyframe(&grant),
                    // Host-to-viewer messages sent the wrong way.
                    Msg::Displays(_) | Msg::Error(_) => {}
                }
            }
            Access::Controller { activity } => {
                let peer = msg.peer;
                if !lock(&self.state).viewing.contains_key(&(peer, activity)) {
                    return Ok(());
                }
                let Some(e) = self.events() else {
                    return Ok(());
                };
                match m {
                    Msg::Displays(mut d) => {
                        d.sanitize();
                        e.displays(peer, activity, d.displays);
                    }
                    Msg::Error(mut err) => {
                        err.sanitize();
                        e.error(peer, activity, &err.reason);
                    }
                    Msg::ListDisplays(_) | Msg::Start(_) | Msg::RequestKeyframe(_) => {}
                }
            }
        }
        Ok(())
    }

    fn on_stream(&self, ctx: &ServiceCtx, mut stream: Stream) {
        match stream.access.clone() {
            Access::Controller { activity } => self.viewer_stream(ctx, stream, activity),
            Access::Host(_) => {
                // Viewers never open streams to a host.
                let _ = stream.recv.stop(stream_code::UNSUPPORTED.into());
                if let Some(s) = stream.send.as_mut() {
                    let _ = s.reset(stream_code::UNSUPPORTED.into());
                }
            }
        }
    }

    fn on_refused(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        if let Some(e) = self.events() {
            e.error(peer, activity, reason);
        }
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        let (host, viewed) = {
            let mut st = lock(&self.state);
            (
                st.host.remove(&(peer, activity)),
                st.viewing.remove(&(peer, activity)).is_some(),
            )
        };
        if let Some(h) = host {
            // The session task stops the capture and finishes the stream.
            h.stop.notify_one();
        }
        if viewed && let Some(e) = self.events() {
            e.ended(peer, activity);
        }
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if connected {
            return;
        }
        let ids: Vec<u64> = {
            let st = lock(&self.state);
            st.host
                .keys()
                .chain(st.viewing.keys())
                .filter(|(p, _)| *p == peer)
                .map(|(_, a)| *a)
                .collect()
        };
        for a in ids {
            self.on_activity_end(ctx, peer, a);
        }
    }
}

pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Screen::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_and_even_sizes() {
        let t = VideoTarget {
            max_width: 1280,
            max_height: 720,
            fps: 30,
            bitrate_bps: 1,
        };
        assert_eq!(t.fit(1920, 1080), (1280, 720));
        assert_eq!(t.fit(2560, 1600), (1152, 720));
        assert_eq!(t.fit(800, 600), (800, 600));
        assert_eq!(t.fit(1081, 2401), (324, 720));
        let (w, h) = t.fit(0, 0);
        assert!(w >= 16 && h >= 16 && w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn ladder_moves_down_on_drops_and_up_when_stable() {
        let t0 = Instant::now();
        let mut l = Ladder::new(t0, 0, 0, 0);
        assert_eq!(l.target().bitrate_bps, 8_000_000);
        // Too early after start: settle first.
        assert_eq!(l.observe(t0 + Duration::from_millis(500), 10, 5), None);
        let t = l.observe(t0 + Duration::from_secs(1), 10, 5).unwrap();
        assert_eq!((t.max_height, t.bitrate_bps), (720, 4_000_000));
        let t = l.observe(t0 + Duration::from_secs(2), 10, 5).unwrap();
        assert_eq!((t.max_height, t.fps), (540, 24));
        // Bottom of the ladder.
        assert_eq!(l.observe(t0 + Duration::from_secs(3), 10, 5), None);
        // A few drops below the ratio do not move it, but reset stability.
        assert_eq!(l.observe(t0 + Duration::from_secs(4), 100, 1), None);
        assert_eq!(l.observe(t0 + Duration::from_secs(8), 15, 0), None);
        let t = l.observe(t0 + Duration::from_secs(9), 15, 0).unwrap();
        assert_eq!(t.max_height, 720);
        assert_eq!(l.observe(t0 + Duration::from_secs(10), 15, 0), None);
        let t = l.observe(t0 + Duration::from_secs(14), 15, 0).unwrap();
        assert_eq!(t.max_height, 1080);
        assert_eq!(l.observe(t0 + Duration::from_secs(30), 15, 0), None);
    }

    #[test]
    fn ladder_respects_viewer_limits() {
        let l = Ladder::new(Instant::now(), 800, 0, 15);
        let t = l.target();
        assert_eq!((t.max_width, t.max_height, t.fps), (800, 1080, 15));
    }

    fn sink(cap: usize) -> (Arc<Sink>, mpsc::Receiver<EncodedFrame>) {
        let (tx, rx) = mpsc::channel(cap);
        (
            Arc::new(Sink {
                tx,
                need_key: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                sent: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                kick: Notify::new(),
                failure: Mutex::new(None),
            }),
            rx,
        )
    }

    fn frame(key: bool) -> EncodedFrame {
        EncodedFrame {
            pts_us: 0,
            keyframe: key,
            data: vec![1],
            width: 2,
            height: 2,
        }
    }

    #[test]
    fn sink_drops_until_keyframe_after_overflow() {
        let (s, mut rx) = sink(2);
        s.push(frame(true));
        s.push(frame(false));
        assert!(!s.ready());
        s.push(frame(false)); // full: dropped, keyframe needed
        assert!(s.need_key.load(Ordering::Relaxed));
        rx.try_recv().unwrap();
        rx.try_recv().unwrap();
        s.push(frame(false)); // still waiting for a keyframe
        assert!(rx.try_recv().is_err());
        s.push(frame(true));
        assert!(rx.try_recv().unwrap().keyframe);
        s.push(frame(false));
        assert!(rx.try_recv().is_ok());
        assert_eq!(s.dropped.load(Ordering::Relaxed), 2);
        assert_eq!(s.sent.load(Ordering::Relaxed), 4);
        s.closed.store(true, Ordering::Relaxed);
        s.push(frame(true));
        assert!(rx.try_recv().is_err());
    }
}
