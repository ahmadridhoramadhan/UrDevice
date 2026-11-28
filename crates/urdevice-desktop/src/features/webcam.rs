//! Phone as webcam and microphone (#28): the desktop side.
//!
//! The camera tile opens a window of its own (`CameraWindow` in `ui/features/webcam.slint`). The
//! window asks the phone for an activity (the phone's user answers Always Ask), lists the phone's
//! cameras, starts the back one, decodes the H.264 stream on a thread of its own, and draws the
//! pictures. Camera, flash, and zoom are buttons in the window; Stop ends the activity. The
//! camera is the screen service with another id (`urdevice_core::services::webcam`), so the
//! stream, the keyframe requests, and the limits are the ones of screen sharing.
//!
//! Frames are never stored or logged.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use urdevice_core::Capability;
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{PeerStatus, PeerView, Snapshot};
use urdevice_core::services::screen::{Display, Screen, ScreenEvents, VideoFrame};
use urdevice_core::services::webcam::Camera;
use urdevice_media::{H264Decoder, RgbaFrame};

use super::{Feature, FeatureCtx, TileSpec};
use crate::ui::CameraWindow;

pub(crate) struct Webcam;

/// Frames waiting for the decoder. When it is behind, frames are dropped until the next keyframe.
const DECODE_QUEUE: usize = 16;

/// What the window asks the phone for: up to 1080p at 30 frames a second (R17.5). The phone's
/// ladder lowers it when the link is slow.
const MAX_WIDTH: u32 = 1920;
const MAX_HEIGHT: u32 = 1080;
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

enum Item {
    Reset,
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
    svc: Weak<Screen>,
}

impl Dec {
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
    loop {
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
            }
            Item::Frame(peer, activity, f) => {
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
                match decoder.decode_with(&f.data, &mut pic, true) {
                    Ok(true) => d.publish(&pic),
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
        if self.dec.drop_until_key.load(Ordering::Relaxed) {
            if !frame.keyframe {
                return;
            }
            self.dec.drop_until_key.store(false, Ordering::Relaxed);
        }
        match self.dec.tx.try_send(Item::Frame(peer, activity, frame)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                // The decoder is behind: drop until a keyframe, and ask for one.
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
        &[
            TileSpec {
                cap: Capability::Camera,
                key: "camera.v1",
            },
            TileSpec {
                cap: Capability::Mic,
                key: "mic.v1",
            },
        ]
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
