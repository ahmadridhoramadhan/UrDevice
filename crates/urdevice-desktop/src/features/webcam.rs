//! Phone as webcam and microphone (#28): the desktop side.
//!
//! The camera tile opens a window of its own (`CameraWindow` in `ui/features/webcam.slint`). The
//! window asks the phone for an activity (the phone's user answers Always Ask), lists the phone's
//! cameras, starts the back one, decodes the H.264 stream on a thread of its own, and draws the
//! pictures. Camera, flash, and zoom are buttons in the window; Stop ends the activity. The
//! camera is the screen service with another id (`urdevice_core::services::webcam`), so the
//! stream, the keyframe requests, and the limits are the ones of screen sharing.
//!
//! The decoded picture is turned to the angle the user chose and shown in the window. It also goes
//! to the virtual camera (`urdevice_platform::features::webcam`), which other apps can pick as a
//! webcam; where the OS has none the window says so and the camera shows here only (R17.3).
//!
//! Frames are never stored or logged.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use urdevice_core::Capability;
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerStatus, PeerView, Snapshot};
use urdevice_core::services::screen::{Display, Screen, ScreenEvents, VideoFrame};
use urdevice_core::services::webcam::Camera;
use urdevice_media::{H264Decoder, RgbaFrame};
use urdevice_platform::features::webcam::{
    Rotation, VirtualCamera, VirtualCameraError, open_virtual_camera, rotate_rgba,
};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::CameraWindow;

pub(crate) struct Webcam;

/// Frames waiting for the decoder. When it is behind, frames are dropped until the next keyframe.
const DECODE_QUEUE: usize = 16;

/// What the window asks the phone for: 720p at 30 frames a second, the least R17.5 asks for. A
/// camera sends full motion all the time (a screen is mostly still), so 1080p at 8 Mbps was more
/// than a Wi-Fi link carried smoothly; this keeps the picture even. The phone lowers the bit rate
/// when the link is slow.
const MAX_WIDTH: u32 = 1280;
const MAX_HEIGHT: u32 = 720;
const FPS: u32 = 30;

/// The states `CameraWindow.state` knows.
mod state {
    pub const IDLE: i32 = 0;
    pub const ASKING: i32 = 1;
    pub const STARTING: i32 = 2;
    pub const SHOWING: i32 = 3;
    pub const STOPPED: i32 = 4;
}

/// The reasons `CameraWindow.message-kind` knows.
fn message_for(reason: &str) -> i32 {
    match reason {
        "declined" => 1,
        "denied" => 2,
        "offline" => 3,
        "unsupported" | "no_host" | "no_display" => 4,
        "consent_declined" => 5,
        "stopped" => 7,
        "timeout" => 8,
        _ => 6,
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The one open camera window. Slint windows live on the UI thread.
struct View {
    win: CameraWindow,
    peer: DeviceId,
    name: String,
    activity: Option<u64>,
    cameras: Vec<Display>,
    index: usize,
    flash: bool,
    zoom: u32,
    /// The size of the last picture, for the window's shape.
    shape: (usize, usize),
}

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    static SVC: RefCell<Option<Arc<Screen>>> = const { RefCell::new(None) };
    static DEC: RefCell<Option<Arc<Dec>>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut View) -> R) -> Option<R> {
    VIEW.with(|v| v.borrow_mut().as_mut().map(f))
}

fn service() -> Option<Arc<Screen>> {
    SVC.with(|s| s.borrow().clone())
}

impl View {
    fn set_state(&self, state: i32, message: i32) {
        self.win.set_state(state);
        self.win.set_message_kind(message);
        if state != state::SHOWING {
            self.win.set_has_frame(false);
        }
    }

    fn show_cameras(&self) {
        let names: Vec<SharedString> = self
            .cameras
            .iter()
            .map(|c| SharedString::from(c.name.as_str()))
            .collect();
        self.win.set_cameras(ModelRc::new(VecModel::from(names)));
        self.win
            .set_camera_index(i32::try_from(self.index).unwrap_or(0));
        // The flash is on the back camera, which the phone marks as primary.
        let back = self.cameras.get(self.index).is_some_and(|c| c.primary);
        self.win.set_flash_available(back);
        self.win.set_flash(self.flash);
        self.win.set_zoom(i32::try_from(self.zoom).unwrap_or(100));
    }

    /// Start the stream of camera `index`. A new stream starts with the flash off and no zoom.
    fn start_camera(&mut self, index: usize) {
        let (Some(svc), Some(activity), Some(cam)) =
            (service(), self.activity, self.cameras.get(index))
        else {
            return;
        };
        self.index = index;
        self.flash = false;
        self.zoom = 100;
        self.show_cameras();
        self.set_state(state::STARTING, 0);
        let _ = svc.start(self.peer, activity, cam.id, MAX_WIDTH, MAX_HEIGHT, FPS);
    }

    fn control(&self) {
        if let (Some(svc), Some(activity)) = (service(), self.activity) {
            let _ = svc.control(self.peer, activity, self.flash, self.zoom);
        }
    }
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// The values of `CameraWindow.vcam-state`.
mod vcam {
    pub const OFF: i32 = 0;
    pub const ON: i32 = 1;
    pub const NOT_SET_UP: i32 = 2;
    pub const NOT_AVAILABLE: i32 = 3;
    pub const FAILED: i32 = 4;
}

enum Item {
    /// A new stream began: start the decoder over and open the virtual camera.
    Reset,
    /// The session ended: close the virtual camera.
    Stop,
    Frame(DeviceId, u64, VideoFrame),
}

/// What the core threads and the decoder thread share.
struct Dec {
    tx: SyncSender<Item>,
    /// The activity being shown; frames of any other are dropped.
    current: AtomicU64,
    /// The queue overflowed: drop until the next keyframe.
    drop_until_key: AtomicBool,
    /// The phone and activity the picture comes from, for keyframe retries.
    asking: Mutex<Option<(DeviceId, u64)>>,
    /// A picture is waiting for the UI thread.
    pending: Mutex<Option<SharedPixelBuffer<Rgba8Pixel>>>,
    posted: AtomicBool,
    /// Frames that arrived, were dropped because the decoder was behind, and are queued now
    /// (for the log and for skipping work on a frame a newer one replaces).
    rx_frames: AtomicU64,
    dropped_frames: AtomicU64,
    queued: AtomicI64,
    /// The turn the user chose, in quarter turns clockwise (0 to 3).
    quarter_turns: AtomicI32,
    /// What the window says about the virtual camera (`vcam::*`).
    vcam_state: AtomicI32,
    svc: Weak<Screen>,
}

/// Timing of the viewer, written to the log every two seconds: how many pictures arrive, are
/// dropped and are shown, how long a decode takes, and the longest wait between two pictures.
struct Stats {
    since: Instant,
    last_shown: Option<Instant>,
    decoded: u32,
    decode_total: Duration,
    decode_max: Duration,
    gap_max: Duration,
    last_rx: u64,
    last_dropped: u64,
}

impl Stats {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            last_shown: None,
            decoded: 0,
            decode_total: Duration::ZERO,
            decode_max: Duration::ZERO,
            gap_max: Duration::ZERO,
            last_rx: 0,
            last_dropped: 0,
        }
    }

    fn decoded(&mut self, took: Duration) {
        self.decoded += 1;
        self.decode_total += took;
        self.decode_max = self.decode_max.max(took);
        let now = Instant::now();
        if let Some(prev) = self.last_shown.replace(now) {
            self.gap_max = self.gap_max.max(now - prev);
        }
    }

    fn report(&mut self, d: &Dec) {
        let spent = self.since.elapsed();
        if spent < Duration::from_secs(2) {
            return;
        }
        let (rx, dropped) = (
            d.rx_frames.load(Ordering::Relaxed),
            d.dropped_frames.load(Ordering::Relaxed),
        );
        if self.decoded > 0 || rx > self.last_rx {
            tracing::info!(
                received = rx - self.last_rx,
                dropped = dropped - self.last_dropped,
                decoded = self.decoded,
                decode_avg_ms = self.decode_total.as_millis() as u32 / self.decoded.max(1),
                decode_max_ms = self.decode_max.as_millis() as u32,
                longest_gap_ms = self.gap_max.as_millis() as u32,
                secs = spent.as_secs_f32(),
                "camera viewer"
            );
        }
        *self = Self {
            last_rx: rx,
            last_dropped: dropped,
            ..Self::new()
        };
    }
}

impl Dec {
    fn want_keyframe(&self, peer: DeviceId, activity: u64) {
        if let Some(s) = self.svc.upgrade() {
            let _ = s.request_keyframe(peer, activity);
        }
    }

    fn rotation(&self) -> Rotation {
        Rotation::from_degrees(
            u32::try_from(self.quarter_turns.load(Ordering::Relaxed)).unwrap_or(0) * 90,
        )
    }

    /// Tell the window what became of the virtual camera.
    fn set_vcam(&self, state: i32) {
        if self.vcam_state.swap(state, Ordering::Relaxed) != state {
            on_ui(move || {
                with(|v| v.win.set_vcam_state(state));
            });
        }
    }

    fn publish(self: &Arc<Self>, data: &[u8], width: u32, height: u32) {
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(data, width, height);
        *lock(&self.pending) = Some(buf);
        if self.posted.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.clone();
        let _ = slint::invoke_from_event_loop(move || {
            me.posted.store(false, Ordering::Release);
            let Some(buf) = lock(&me.pending).take() else {
                return;
            };
            with(|v| {
                let (w, h) = (buf.width() as usize, buf.height() as usize);
                v.win.set_frame(Image::from_rgba8(buf));
                if v.shape != (w, h) && w > 0 && h > 0 {
                    v.shape = (w, h);
                    v.win.set_aspect(w as f32 / h as f32);
                }
                v.win.set_has_frame(true);
                if v.win.get_state() != state::SHOWING {
                    v.set_state(state::SHOWING, 0);
                }
            });
        });
    }
}

fn decode_loop(d: &Arc<Dec>, rx: &Receiver<Item>) {
    let mut dec = H264Decoder::new().ok();
    let mut pic = RgbaFrame::default();
    let mut waiting_key = true;
    let mut retries = 0u32;
    let mut stats = Stats::new();
    let mut camera: Option<Box<dyn VirtualCamera>> = None;
    loop {
        stats.report(d);
        let item = match rx.recv_timeout(Duration::from_millis(700)) {
            Ok(item) => item,
            Err(RecvTimeoutError::Timeout) => {
                // Nothing came and no picture shows yet: the first keyframe may have been lost.
                if waiting_key && d.current.load(Ordering::Relaxed) != 0 && retries < 12 {
                    retries += 1;
                    if let Some((peer, activity)) = *lock(&d.asking) {
                        d.want_keyframe(peer, activity);
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match item {
            Item::Reset => {
                dec = H264Decoder::new().ok();
                waiting_key = true;
                retries = 0;
                if camera.is_none() {
                    match open_virtual_camera() {
                        Ok(c) => {
                            camera = Some(c);
                            d.set_vcam(vcam::ON);
                        }
                        Err(VirtualCameraError::NotAvailable) => d.set_vcam(vcam::NOT_AVAILABLE),
                        Err(VirtualCameraError::NotSetUp) => d.set_vcam(vcam::NOT_SET_UP),
                        Err(e) => {
                            tracing::info!(error = %e, "no virtual camera");
                            d.set_vcam(vcam::FAILED);
                        }
                    }
                }
            }
            Item::Stop => {
                camera = None;
                d.set_vcam(vcam::OFF);
            }
            Item::Frame(peer, activity, f) => {
                // Newer frames wait behind this one: it is decoded, but not converted or shown.
                let behind = d.queued.fetch_sub(1, Ordering::Relaxed) > 1;
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
                let began = Instant::now();
                let decoded = decoder.decode_with(&f.data, &mut pic, !behind);
                stats.decoded(began.elapsed());
                match decoded {
                    Ok(true) => {
                        // Turned to the user's angle: the window and the virtual camera show the
                        // same picture.
                        let turned = rotate_rgba(&pic.data, pic.width, pic.height, d.rotation())
                            .filter(|_| d.rotation() != Rotation::None);
                        let (data, w, h) = match &turned {
                            Some((v, w, h)) => (v.as_slice(), *w, *h),
                            None => (pic.data.as_slice(), pic.width, pic.height),
                        };
                        d.publish(data, w, h);
                        if let Some(cam) = camera.as_mut()
                            && cam.write(data, w, h).is_err()
                        {
                            camera = None;
                            d.set_vcam(vcam::FAILED);
                        }
                    }
                    Ok(false) => {}
                    Err(_) => {
                        waiting_key = true;
                        d.want_keyframe(peer, activity);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Core events
// ---------------------------------------------------------------------------

/// What the core tells the window. Called on core threads: hand work to the UI thread.
struct Events {
    dec: Arc<Dec>,
}

fn on_ui(f: impl FnOnce() + Send + 'static) {
    let _ = slint::invoke_from_event_loop(f);
}

impl ScreenEvents for Events {
    fn opened(&self, peer: DeviceId, activity: u64) {
        on_ui(move || {
            let mine = with(|v| {
                if v.peer != peer {
                    return false;
                }
                v.activity = Some(activity);
                v.set_state(state::STARTING, 0);
                true
            });
            if mine == Some(true)
                && let Some(svc) = service()
            {
                let _ = svc.list_displays(peer, activity);
            }
        });
    }

    fn displays(&self, peer: DeviceId, activity: u64, displays: Vec<Display>) {
        on_ui(move || {
            with(|v| {
                if v.peer != peer || v.activity != Some(activity) {
                    return;
                }
                if displays.is_empty() {
                    v.set_state(state::STOPPED, message_for("no_display"));
                    return;
                }
                // The back camera (primary) first.
                let index = displays.iter().position(|d| d.primary).unwrap_or(0);
                v.cameras = displays;
                v.start_camera(index);
            });
        });
    }

    fn stream_started(&self, peer: DeviceId, activity: u64, _display_id: u32) {
        *lock(&self.dec.asking) = Some((peer, activity));
        self.dec.current.store(activity, Ordering::Relaxed);
        self.dec.drop_until_key.store(false, Ordering::Relaxed);
        let _ = self.dec.tx.try_send(Item::Reset);
    }

    fn frame(&self, peer: DeviceId, activity: u64, frame: VideoFrame) {
        self.dec.rx_frames.fetch_add(1, Ordering::Relaxed);
        if self.dec.drop_until_key.load(Ordering::Relaxed) {
            if !frame.keyframe {
                self.dec.dropped_frames.fetch_add(1, Ordering::Relaxed);
                return;
            }
            self.dec.drop_until_key.store(false, Ordering::Relaxed);
        }
        match self.dec.tx.try_send(Item::Frame(peer, activity, frame)) {
            Ok(()) => {
                self.dec.queued.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                // The decoder is behind: drop until a keyframe, and ask for one.
                self.dec.dropped_frames.fetch_add(1, Ordering::Relaxed);
                self.dec.drop_until_key.store(true, Ordering::Relaxed);
                self.dec.want_keyframe(peer, activity);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn error(&self, peer: DeviceId, activity: u64, reason: &str) {
        let message = message_for(reason);
        on_ui(move || {
            with(|v| {
                // Activity 0 is a refused request; any other is the running session.
                if v.peer == peer && (activity == 0 || v.activity == Some(activity)) {
                    v.set_state(state::STOPPED, message);
                }
            });
        });
    }

    fn ended(&self, peer: DeviceId, activity: u64) {
        self.dec.current.store(0, Ordering::Relaxed);
        let _ = self.dec.tx.try_send(Item::Stop);
        on_ui(move || {
            with(|v| {
                if v.peer == peer && v.activity == Some(activity) {
                    v.activity = None;
                    v.set_state(state::STOPPED, message_for("stopped"));
                }
            });
        });
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// Stop the stream and close the window.
fn close() {
    let old = VIEW.with(|v| v.borrow_mut().take());
    if let Some(v) = old {
        if let (Some(svc), Some(activity)) = (service(), v.activity) {
            svc.stop(v.peer, activity);
        }
        let _ = v.win.hide();
    }
    DEC.with(|d| {
        if let Some(d) = d.borrow().as_ref() {
            d.current.store(0, Ordering::Relaxed);
            let _ = d.tx.try_send(Item::Stop);
        }
    });
}

/// Connect the window's callbacks. Each looks the window up again, so none holds the window and
/// there is no reference cycle.
fn wire(win: &CameraWindow) {
    win.on_pick_camera(|i| {
        with(|v| {
            if let Ok(i) = usize::try_from(i) {
                v.start_camera(i);
            }
        });
    });
    win.on_set_flash(|on| {
        with(|v| {
            v.flash = on;
            v.win.set_flash(on);
            v.control();
        });
    });
    win.on_set_zoom(|z| {
        with(|v| {
            v.zoom = u32::try_from(z).unwrap_or(100).clamp(100, 1000);
            v.win.set_zoom(i32::try_from(v.zoom).unwrap_or(100));
            v.control();
        });
    });
    win.on_rotate(|| {
        DEC.with(|d| {
            if let Some(d) = d.borrow().as_ref() {
                let next = (d.quarter_turns.load(Ordering::Relaxed) + 1) % 4;
                d.quarter_turns.store(next, Ordering::Relaxed);
            }
        });
    });
    win.on_stop(close);
    win.window().on_close_requested(|| {
        // The close button ends the stream too: the phone stops using its camera.
        let _ = slint::invoke_from_event_loop(close);
        slint::CloseRequestResponse::HideWindow
    });
}

fn open_window(svc: &Arc<Screen>, peer: DeviceId, name: &str) {
    let same = VIEW.with(|v| v.borrow().as_ref().is_some_and(|x| x.peer == peer));
    if !same {
        // Another phone's window ends its stream first.
        close();
        let Ok(win) = CameraWindow::new() else {
            tracing::warn!("the camera window could not be created");
            return;
        };
        wire(&win);
        win.set_device_name(name.into());
        VIEW.with(|v| {
            *v.borrow_mut() = Some(View {
                win,
                peer,
                name: name.to_owned(),
                activity: None,
                cameras: Vec::new(),
                index: 0,
                flash: false,
                zoom: 100,
                shape: (0, 0),
            });
        });
    }
    let ask = with(|v| {
        let _ = v.win.show();
        // A window that stopped (or has not asked yet) asks the phone again.
        let idle = v.activity.is_none();
        if idle {
            v.win.set_device_name(v.name.as_str().into());
            v.set_state(state::ASKING, 0);
        }
        idle
    });
    if ask == Some(true) {
        let _ = svc.request_view(peer);
    }
}

impl Feature for Webcam {
    fn id(&self) -> &'static str {
        "webcam"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Camera,
            key: "camera.v1",
        }]
    }

    // The tile opens a window of its own, not a page.
    fn direct(&self) -> bool {
        true
    }

    fn start(&self, ctx: &FeatureCtx) {
        let Some(camera) = ctx.node.service::<Camera>() else {
            return;
        };
        let svc = camera.screen().clone();
        let (tx, rx) = sync_channel(DECODE_QUEUE);
        let dec = Arc::new(Dec {
            tx,
            current: AtomicU64::new(0),
            drop_until_key: AtomicBool::new(false),
            asking: Mutex::new(None),
            pending: Mutex::new(None),
            posted: AtomicBool::new(false),
            rx_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            queued: AtomicI64::new(0),
            quarter_turns: AtomicI32::new(0),
            vcam_state: AtomicI32::new(vcam::OFF),
            svc: Arc::downgrade(&svc),
        });
        let worker = dec.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("camera-decoder".into())
            .spawn(move || decode_loop(&worker, &rx))
        {
            tracing::warn!(error = %e, "no camera decoder thread");
        }
        svc.events.set(Arc::new(Events { dec: dec.clone() }));
        SVC.with(|s| *s.borrow_mut() = Some(svc));
        DEC.with(|d| *d.borrow_mut() = Some(dec));
    }

    fn refresh(&self, _ctx: &FeatureCtx, snap: &Snapshot) {
        // The phone went offline: the stream is gone.
        let gone = with(|v| {
            let hex = v.peer.hex();
            let online = snap
                .peers
                .iter()
                .any(|p| p.id == hex && p.status == PeerStatus::Connected);
            if !online && v.win.get_state() != state::STOPPED && v.win.get_state() != state::IDLE {
                v.activity = None;
                v.set_state(state::STOPPED, message_for("offline"));
            }
        });
        let _ = gone;
    }

    fn open(&self, _ctx: &FeatureCtx, peer: &PeerView, tile: &str) {
        if tile != "camera.v1" {
            return;
        }
        let (Some(id), Some(svc)) = (DeviceId::from_hex(&peer.id), service()) else {
            return;
        };
        open_window(&svc, id, &peer.name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_map_to_the_messages_of_the_window() {
        assert_eq!(message_for("declined"), 1);
        assert_eq!(message_for("denied"), 2);
        assert_eq!(message_for("consent_declined"), 5);
        assert_eq!(message_for("something new"), 6);
    }
}
