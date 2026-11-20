//! Screen sharing (#21, design §10): the desktop side.
//!
//! **Host.** [`DesktopHost`] gives the core service this computer's capture
//! (`urdevice_platform::features::screen`) and an `urdevice_media` encoder.
//! Capture hands frames to a one-frame mailbox; an encoder thread takes the
//! newest one at the target frame rate, scales and encodes it, and pushes it
//! into the core's sink. When the sink says the network is behind, capture
//! skips frames before they are copied (design §10). The indicator and Stop
//! for an incoming session are the app's activity banner (R5.4).
//!
//! **Viewer.** The view (`ui/features/screen.slint`, global
//! `ScreenFeature`) asks for an activity, lists the host's displays, and
//! starts the stream. A decoder thread turns frames into a Slint image. When
//! its queue overflows or a frame does not decode, it drops frames up to the
//! next keyframe and asks the host for one.
//!
//! Frames are never stored or logged.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerView, Snapshot};
use urdevice_core::services::Grant;
use urdevice_core::services::screen::{
    Capture, Display, EncodedFrame, FrameSink, Screen as ScreenService, ScreenEvents, ScreenHost,
    VideoFrame, VideoTarget,
};
use urdevice_core::{Capability, CoreError};
use urdevice_media::{AutoEncoder, Backend, BgraFrame, EncoderSettings, H264Decoder, RgbaFrame};
use urdevice_platform::features::screen::{self as capture, CaptureHandle, Frame, FrameHandler};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::{MainWindow, ScreenFeature};

pub(crate) struct Screen;

/// Remote control from the viewing window (R12.5) needs the input service
/// (#18, feature `input`), which is not on `main` yet. Until it is, the view
/// is view-only and [`pointer`] is the hook to fill in.
const INPUT_FORWARDING: bool = false;

/// Frames waiting for the decoder before the viewer starts dropping.
const DECODE_QUEUE: usize = 4;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

// ---------------------------------------------------------------------------
// Host: capture -> encoder thread -> core sink
// ---------------------------------------------------------------------------

struct DesktopHost;

impl ScreenHost for DesktopHost {
    fn displays(&self) -> Vec<Display> {
        capture::displays()
    }

    fn start(
        &self,
        grant: &Grant,
        display_id: u32,
        target: VideoTarget,
        sink: Arc<dyn FrameSink>,
    ) -> urdevice_core::Result<Arc<dyn Capture>> {
        tracing::info!(
            activity = grant.activity(),
            display = display_id,
            "sharing this screen"
        );
        let shared = Arc::new(Pipe {
            sink,
            mailbox: Mutex::new(None),
            cv: Condvar::new(),
            stopped: AtomicBool::new(false),
            keyframe: AtomicBool::new(true),
            target: Mutex::new(target),
            epoch: Instant::now(),
            due_ns: AtomicU64::new(0),
        });
        let handle = capture::start(display_id, target.fps, shared.clone())?;
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("screen-encoder".into())
            .spawn(move || encode_loop(&worker))
            .map_err(|_| CoreError::Refused("capture_failed".into()))?;
        Ok(Arc::new(Pipeline {
            pipe: shared,
            handle: Mutex::new(Some(handle)),
        }))
    }
}

/// State shared by the capture back-end, the encoder thread, and the core.
struct Pipe {
    sink: Arc<dyn FrameSink>,
    mailbox: Mutex<Option<Frame>>,
    cv: Condvar,
    stopped: AtomicBool,
    keyframe: AtomicBool,
    target: Mutex<VideoTarget>,
    epoch: Instant,
    /// Nanoseconds since `epoch` before which capture skips frames.
    due_ns: AtomicU64,
}

impl Pipe {
    fn now_ns(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

impl FrameHandler for Pipe {
    fn wants(&self) -> bool {
        !self.stopped.load(Ordering::Relaxed)
            && self.sink.ready()
            && self.now_ns() >= self.due_ns.load(Ordering::Relaxed)
    }

    fn frame(&self, frame: Frame) {
        let fps = lock(&self.target).fps.max(1);
        let every = 1_000_000_000 / u64::from(fps);
        self.due_ns.store(self.now_ns() + every, Ordering::Relaxed);
        *lock(&self.mailbox) = Some(frame);
        self.cv.notify_one();
    }

    fn ended(&self, reason: &'static str) {
        self.sink.failed(reason);
    }
}

fn encode_loop(p: &Pipe) {
    let mut encoder: Option<AutoEncoder> = None;
    while !p.stopped.load(Ordering::Relaxed) {
        let frame = {
            let mut g = lock(&p.mailbox);
            if g.is_none() {
                g = match p.cv.wait_timeout(g, Duration::from_millis(200)) {
                    Ok((g, _)) => g,
                    Err(e) => e.into_inner().0,
                };
            }
            g.take()
        };
        let Some(frame) = frame else { continue };
        let target = *lock(&p.target);
        let (w, h) = target.fit(frame.width, frame.height);
        let settings = EncoderSettings {
            width: w,
            height: h,
            fps: target.fps,
            bitrate_bps: target.bitrate_bps,
        };
        let ready = match &mut encoder {
            Some(e) => e.reconfigure(settings).is_ok(),
            None => match AutoEncoder::open(settings, Backend::Auto) {
                Ok(e) => {
                    tracing::info!(
                        encoder = e.name(),
                        width = w,
                        height = h,
                        "screen encoder open"
                    );
                    encoder = Some(e);
                    true
                }
                Err(e) => {
                    tracing::warn!(error = %e, "no H.264 encoder");
                    false
                }
            },
        };
        let Some(enc) = encoder.as_mut().filter(|_| ready) else {
            p.sink.failed("capture_failed");
            return;
        };
        let bgra = BgraFrame {
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            data: frame.data,
        };
        let pts_us = u64::try_from(p.epoch.elapsed().as_micros()).unwrap_or(u64::MAX);
        let key = p.keyframe.swap(false, Ordering::Relaxed);
        match enc.encode(&bgra, pts_us, key) {
            Ok(units) => {
                for u in units {
                    p.sink.push(EncodedFrame {
                        pts_us: u.pts_us,
                        keyframe: u.keyframe,
                        data: u.data,
                        width: w,
                        height: h,
                    });
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "screen encode failed");
                p.sink.failed("capture_failed");
                return;
            }
        }
    }
}

struct Pipeline {
    pipe: Arc<Pipe>,
    handle: Mutex<Option<CaptureHandle>>,
}

impl Capture for Pipeline {
    fn request_keyframe(&self) {
        self.pipe.keyframe.store(true, Ordering::Relaxed);
    }

    fn set_target(&self, target: VideoTarget) {
        *lock(&self.pipe.target) = target;
    }

    fn stop(&self) {
        self.pipe.stopped.store(true, Ordering::Relaxed);
        self.pipe.cv.notify_all();
        // Dropping the handle stops the OS capture.
        drop(lock(&self.handle).take());
        tracing::info!("screen sharing stopped");
    }
}

// ---------------------------------------------------------------------------
// Viewer: core events -> decoder thread -> Slint image
// ---------------------------------------------------------------------------

/// The message kinds `ui/features/screen.slint` knows.
mod kind {
    pub const NONE: i32 = 0;
    pub const DECLINED: i32 = 1;
    pub const DENIED: i32 = 2;
    pub const OFFLINE: i32 = 3;
    pub const NO_HOST: i32 = 4;
    pub const CONSENT: i32 = 5;
    pub const FAILED: i32 = 6;
    pub const ENDED: i32 = 7;
    pub const TIMEOUT: i32 = 8;
}

fn message_for(reason: &str) -> i32 {
    match reason {
        "declined" => kind::DECLINED,
        "denied" | "ask" | "no_activity" => kind::DENIED,
        "offline" => kind::OFFLINE,
        "no_host" | "unsupported" | "no_display" => kind::NO_HOST,
        "consent_declined" => kind::CONSENT,
        "timeout" => kind::TIMEOUT,
        "stopped" => kind::ENDED,
        _ => kind::FAILED,
    }
}

enum DecItem {
    Reset,
    Frame(DeviceId, u64, VideoFrame),
}

/// What the core threads and the decoder thread share.
struct Decode {
    tx: SyncSender<DecItem>,
    /// The activity being shown; frames of any other are dropped.
    current: AtomicU64,
    /// The queue overflowed: drop until the next keyframe.
    drop_until_key: AtomicBool,
    /// A picture is waiting for the UI thread.
    pending: Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    posted: AtomicBool,
    svc: Weak<ScreenService>,
    window: slint::Weak<MainWindow>,
}

impl Decode {
    fn want_keyframe(&self, peer: DeviceId, activity: u64) {
        if let Some(s) = self.svc.upgrade() {
            let _ = s.request_keyframe(peer, activity);
        }
    }

    fn publish(self: &Arc<Self>, pic: &RgbaFrame) {
        let buf =
            SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&pic.data, pic.width, pic.height);
        *lock(&self.pending) = Some(buf);
        if self.posted.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        let _ = self.window.upgrade_in_event_loop(move |w| {
            me.posted.store(false, Ordering::Release);
            let Some(buf) = lock(&me.pending).take() else {
                return;
            };
            let g = w.global::<ScreenFeature>();
            g.set_frame_width(buf.width() as i32);
            g.set_frame_height(buf.height() as i32);
            g.set_frame(Image::from_rgba8(buf));
            if g.get_state() == 2 {
                g.set_state(3);
            }
        });
    }
}

fn decode_loop(d: &Arc<Decode>, rx: &Receiver<DecItem>) {
    let mut dec = H264Decoder::new().ok();
    let mut pic = RgbaFrame::default();
    let mut waiting_key = true;
    while let Ok(item) = rx.recv() {
        match item {
            DecItem::Reset => {
                dec = H264Decoder::new().ok();
                waiting_key = true;
            }
            DecItem::Frame(peer, activity, f) => {
                if activity != d.current.load(Ordering::Relaxed) {
                    continue;
                }
                if waiting_key && !f.keyframe {
                    continue;
                }
                waiting_key = false;
                let Some(decoder) = dec.as_mut() else {
                    continue;
                };
                match decoder.decode(&f.data, &mut pic) {
                    Ok(true) => d.publish(&pic),
                    Ok(false) => {}
                    Err(e) => {
                        tracing::debug!(error = %e, "frame did not decode; asking for a keyframe");
                        waiting_key = true;
                        d.want_keyframe(peer, activity);
                    }
                }
            }
        }
    }
}

struct Events {
    window: slint::Weak<MainWindow>,
    dec: Arc<Decode>,
}

impl Events {
    fn ui(&self, f: impl FnOnce(&MainWindow) + Send + 'static) {
        let _ = self.window.upgrade_in_event_loop(move |w| f(&w));
    }
}

impl ScreenEvents for Events {
    fn opened(&self, peer: DeviceId, activity: u64) {
        self.ui(move |w| on_opened(w, peer, activity));
    }

    fn displays(&self, peer: DeviceId, activity: u64, displays: Vec<Display>) {
        self.ui(move |w| on_displays(w, peer, activity, displays));
    }

    fn stream_started(&self, _peer: DeviceId, activity: u64, _display_id: u32) {
        self.dec.current.store(activity, Ordering::Relaxed);
        self.dec.drop_until_key.store(false, Ordering::Relaxed);
        let _ = self.dec.tx.try_send(DecItem::Reset);
    }

    fn frame(&self, peer: DeviceId, activity: u64, frame: VideoFrame) {
        if self.dec.drop_until_key.load(Ordering::Relaxed) {
            if !frame.keyframe {
                return;
            }
            self.dec.drop_until_key.store(false, Ordering::Relaxed);
        }
        match self.dec.tx.try_send(DecItem::Frame(peer, activity, frame)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dec.drop_until_key.store(true, Ordering::Relaxed);
                self.dec.want_keyframe(peer, activity);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn error(&self, peer: DeviceId, activity: u64, reason: &str) {
        let m = message_for(reason);
        self.ui(move |w| on_error(w, peer, activity, m));
    }

    fn ended(&self, peer: DeviceId, activity: u64) {
        self.ui(move |w| on_error(w, peer, activity, kind::ENDED));
    }
}

/// What the UI thread keeps.
#[derive(Default)]
struct Ui {
    svc: Option<Arc<ScreenService>>,
    dec: Option<Arc<Decode>>,
    peer: Option<DeviceId>,
    activity: Option<u64>,
    displays: Vec<Display>,
    started: bool,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui::default());
}

fn reset(w: &MainWindow, message: i32) {
    UI.with(|u| {
        let mut u = u.borrow_mut();
        u.activity = None;
        u.displays.clear();
        u.started = false;
        if let Some(d) = &u.dec {
            d.current.store(0, Ordering::Relaxed);
        }
    });
    let g = w.global::<ScreenFeature>();
    g.set_state(0);
    g.set_message_kind(message);
    g.set_displays(ModelRc::default());
    g.set_frame(Image::default());
    g.set_frame_width(0);
    g.set_frame_height(0);
}

fn on_opened(w: &MainWindow, peer: DeviceId, activity: u64) {
    let asking = w.global::<ScreenFeature>().get_state() == 1;
    let svc = UI.with(|u| {
        let mut u = u.borrow_mut();
        if !asking || u.peer != Some(peer) || u.activity.is_some() {
            return None;
        }
        u.activity = Some(activity);
        u.svc.clone()
    });
    let Some(svc) = svc else {
        // The view moved on: end the session nobody watches.
        if let Some(s) = UI.with(|u| u.borrow().svc.clone()) {
            s.stop(peer, activity);
        }
        return;
    };
    w.global::<ScreenFeature>().set_state(2);
    if let Err(e) = svc.list_displays(peer, activity) {
        tracing::debug!(error = %e, "could not ask for displays");
    }
}

fn on_displays(w: &MainWindow, peer: DeviceId, activity: u64, displays: Vec<Display>) {
    let start = UI.with(|u| {
        let mut u = u.borrow_mut();
        if u.peer != Some(peer) || u.activity != Some(activity) {
            return None;
        }
        u.displays = displays.clone();
        let first = !u.started;
        u.started = true;
        let index = displays.iter().position(|d| d.primary).unwrap_or(0);
        Some((first, index, u.svc.clone()))
    });
    let Some((first, index, svc)) = start else {
        return;
    };
    let g = w.global::<ScreenFeature>();
    let names: Vec<SharedString> = displays
        .iter()
        .map(|d| {
            if d.width > 0 && d.height > 0 {
                format!("{} ({}×{})", d.name, d.width, d.height).into()
            } else {
                d.name.clone().into()
            }
        })
        .collect();
    g.set_displays(ModelRc::new(VecModel::from(names)));
    if first && let (Some(d), Some(svc)) = (displays.get(index), svc) {
        g.set_display_index(index as i32);
        let _ = svc.start(peer, activity, d.id, 0, 0, 0);
    }
    if displays.is_empty() {
        reset(w, kind::NO_HOST);
        if let Some(s) = UI.with(|u| u.borrow().svc.clone()) {
            s.stop(peer, activity);
        }
    }
}

fn on_error(w: &MainWindow, peer: DeviceId, activity: u64, message: i32) {
    let ours = UI.with(|u| {
        let u = u.borrow();
        u.peer == Some(peer) && (activity == 0 || u.activity == Some(activity))
    });
    let state = w.global::<ScreenFeature>().get_state();
    if !ours || state == 0 {
        return;
    }
    // A request refused before any activity only matters while asking.
    if activity == 0 && state != 1 {
        return;
    }
    let act = UI.with(|u| u.borrow().activity);
    reset(w, message);
    if let (Some(a), Some(s)) = (act, UI.with(|u| u.borrow().svc.clone())) {
        s.stop(peer, a);
    }
}

/// Pointer on the picture (R12.5). `x`, `y` are fractions of the host's
/// display. Remote control needs the input service (#18): when it is on
/// `main`, request an `input.v1` activity for the peer and send `Abs`
/// pointer events with these coordinates here.
fn pointer(_x: f32, _y: f32, _event: i32, _button: i32) {
    // TODO(#18): forward to the input service once it exists.
}

fn wire(w: &MainWindow) {
    let g = w.global::<ScreenFeature>();
    let weak = w.as_weak();
    g.on_start(move || {
        let Some(w) = weak.upgrade() else { return };
        let peer = DeviceId::from_hex(&w.global::<ScreenFeature>().get_peer_id());
        let Some(peer) = peer else { return };
        let svc = UI.with(|u| {
            let mut u = u.borrow_mut();
            u.peer = Some(peer);
            u.activity = None;
            u.started = false;
            u.svc.clone()
        });
        let Some(svc) = svc else { return };
        let g = w.global::<ScreenFeature>();
        g.set_message_kind(kind::NONE);
        g.set_state(1);
        if svc.request_view(peer).is_err() {
            reset(&w, kind::OFFLINE);
        }
    });
    let weak = w.as_weak();
    g.on_stop(move || {
        let Some(w) = weak.upgrade() else { return };
        let (peer, act, svc) = UI.with(|u| {
            let u = u.borrow();
            (u.peer, u.activity, u.svc.clone())
        });
        reset(&w, kind::NONE);
        if let (Some(p), Some(a), Some(s)) = (peer, act, svc) {
            s.stop(p, a);
        }
    });
    let weak = w.as_weak();
    g.on_pick_display(move |i| {
        let Some(w) = weak.upgrade() else { return };
        let pick = UI.with(|u| {
            let u = u.borrow();
            let d = u.displays.get(usize::try_from(i).ok()?)?.id;
            Some((u.peer?, u.activity?, d, u.svc.clone()?))
        });
        let Some((peer, act, display, svc)) = pick else {
            return;
        };
        let g = w.global::<ScreenFeature>();
        g.set_display_index(i);
        g.set_state(2);
        let _ = svc.start(peer, act, display, 0, 0, 0);
    });
    g.on_pointer(pointer);
}

impl Feature for Screen {
    fn id(&self) -> &'static str {
        "screen"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Screen,
            key: "screen.h264.v1",
        }]
    }

    fn has_view(&self) -> bool {
        true
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(svc) = ctx.node.service::<ScreenService>() else {
            return;
        };
        svc.host.set(Arc::new(DesktopHost));
        let (tx, rx) = sync_channel(DECODE_QUEUE);
        let dec = Arc::new(Decode {
            tx,
            current: AtomicU64::new(0),
            drop_until_key: AtomicBool::new(false),
            pending: Mutex::new(None),
            posted: AtomicBool::new(false),
            svc: Arc::downgrade(&svc),
            window: ctx.window.clone(),
        });
        let worker = dec.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("screen-decoder".into())
            .spawn(move || decode_loop(&worker, &rx))
        {
            tracing::warn!(error = %e, "no screen decoder thread");
        }
        svc.events.set(Arc::new(Events {
            window: ctx.window.clone(),
            dec: dec.clone(),
        }));
        UI.with(|u| {
            let mut u = u.borrow_mut();
            u.svc = Some(svc);
            u.dec = Some(dec);
        });
        wire(&w);
    }

    fn refresh(&self, ctx: &FeatureCtx, snap: &Snapshot) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        // The view's peer went offline: the session is gone.
        let peer = UI.with(|u| u.borrow().peer);
        if let Some(p) = peer
            && w.global::<ScreenFeature>().get_state() != 0
        {
            let hex = p.hex();
            let online = snap
                .peers
                .iter()
                .any(|x| x.id == hex && x.status == urdevice_core::node::PeerStatus::Connected);
            if !online {
                reset(&w, kind::OFFLINE);
            }
        }
        w.global::<ScreenFeature>()
            .set_input_allowed(INPUT_FORWARDING);
    }

    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let Some(id) = DeviceId::from_hex(&peer.id) else {
            return;
        };
        let (old_peer, old_act, svc) = UI.with(|u| {
            let u = u.borrow();
            (u.peer, u.activity, u.svc.clone())
        });
        if old_peer != Some(id) {
            // Leaving another device's screen ends that session.
            if let (Some(p), Some(a), Some(s)) = (old_peer, old_act, svc) {
                s.stop(p, a);
            }
            reset(&w, kind::NONE);
            UI.with(|u| u.borrow_mut().peer = Some(id));
        }
        w.global::<ScreenFeature>()
            .set_peer_id(peer.id.clone().into());
    }
}
