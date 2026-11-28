//! Screen sharing (#21): bindings for the Android app.
//!
//! **Host.** Kotlin captures with MediaProjection and encodes with
//! MediaCodec ([`ScreenHostFfi`]). Each capture is a numbered session; Kotlin
//! hands encoded frames back with [`UrDeviceNode::screen_push_frame`] and
//! reports the end of a capture with [`UrDeviceNode::screen_capture_ended`].
//!
//! **Viewer.** Kotlin asks with [`UrDeviceNode::screen_request_view`] and
//! gets everything else through [`ScreenListener`]: the activity, the
//! display list, and the H.264 frames it feeds to MediaCodec.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::screen::{
    Capture, Display, EncodedFrame, FrameSink, Screen, ScreenEvents, ScreenHost, VideoFrame,
    VideoTarget,
};
use urdevice_core::{CoreError, Result};

use crate::{FfiError, UrDeviceNode, device_id};

/// One display on a host.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDisplay {
    pub id: u32,
    pub name: String,
    /// 0 when unknown until capture starts.
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl From<Display> for FfiDisplay {
    fn from(d: Display) -> Self {
        Self {
            id: d.id,
            name: d.name,
            width: d.width,
            height: d.height,
            primary: d.primary,
        }
    }
}

impl From<FfiDisplay> for Display {
    fn from(d: FfiDisplay) -> Self {
        Self {
            id: d.id,
            name: d.name,
            width: d.width,
            height: d.height,
            primary: d.primary,
        }
    }
}

/// What the encoder should aim for (design §10 ladder step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FfiVideoTarget {
    /// Scale the screen to fit inside this box, keeping its aspect ratio.
    pub max_width: u32,
    pub max_height: u32,
    pub fps: u32,
    /// Constant bitrate, bits per second.
    pub bitrate_bps: u32,
}

impl From<VideoTarget> for FfiVideoTarget {
    fn from(t: VideoTarget) -> Self {
        Self {
            max_width: t.max_width,
            max_height: t.max_height,
            fps: t.fps,
            bitrate_bps: t.bitrate_bps,
        }
    }
}

/// One encoded frame.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiVideoFrame {
    /// Capture time in microseconds.
    pub pts_us: u64,
    /// An IDR frame with SPS and PPS.
    pub keyframe: bool,
    /// H.264 Annex B.
    pub data: Vec<u8>,
    /// Set on keyframes, 0 otherwise.
    pub width: u32,
    pub height: u32,
}

/// The phone's screen capture, implemented in Kotlin. Called from core
/// worker threads; `start` may block while the system consent dialog is
/// open.
#[uniffi::export(with_foreign)]
pub trait ScreenHostFfi: Send + Sync {
    /// The displays that can be shared (one on a phone).
    fn displays(&self) -> Vec<FfiDisplay>;
    /// Ask for consent and start capture `session`. Returns an empty string
    /// on success, else `consent_declined`, `no_display`, or
    /// `capture_failed`.
    fn start(&self, session: u64, display_id: u32, target: FfiVideoTarget) -> String;
    /// Make the next frame a keyframe.
    fn request_keyframe(&self, session: u64);
    /// The adaptive ladder moved.
    fn set_target(&self, session: u64, target: FfiVideoTarget);
    /// Stop capture `session` and release MediaProjection.
    fn stop(&self, session: u64);
}

/// Viewer news from the core, on core threads: post to the main thread
/// (frames go straight to the decoder).
#[uniffi::export(with_foreign)]
pub trait ScreenListener: Send + Sync {
    /// The host allowed viewing: use `activity` for the other calls.
    fn on_opened(&self, device_id: String, activity: u64);
    /// The host's displays.
    fn on_displays(&self, device_id: String, activity: u64, displays: Vec<FfiDisplay>);
    /// A new video stream began: reset the decoder.
    fn on_stream_started(&self, device_id: String, activity: u64, display_id: u32);
    /// One H.264 access unit.
    fn on_frame(&self, device_id: String, activity: u64, frame: FfiVideoFrame);
    /// A refusal or a host error: `declined`, `denied`, `timeout`,
    /// `offline`, `unsupported`, `no_host`, `consent_declined`,
    /// `capture_failed`, `no_display`. `activity` is 0 when the request
    /// itself was refused.
    fn on_error(&self, device_id: String, activity: u64, reason: String);
    /// The session ended.
    fn on_ended(&self, device_id: String, activity: u64);
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Sinks of the running captures, by session number.
#[derive(Default)]
pub(crate) struct Sessions {
    pub(crate) next: AtomicU64,
    pub(crate) sinks: Mutex<HashMap<u64, Arc<dyn FrameSink>>>,
}

pub(crate) static SESSIONS: std::sync::LazyLock<Sessions> =
    std::sync::LazyLock::new(Sessions::default);

struct HostAdapter(Arc<dyn ScreenHostFfi>);

struct KotlinCapture {
    host: Arc<dyn ScreenHostFfi>,
    session: u64,
}

impl Capture for KotlinCapture {
    fn request_keyframe(&self) {
        self.host.request_keyframe(self.session);
    }

    fn set_target(&self, target: VideoTarget) {
        self.host.set_target(self.session, target.into());
    }

    fn stop(&self) {
        lock(&SESSIONS.sinks).remove(&self.session);
        self.host.stop(self.session);
    }
}

impl ScreenHost for HostAdapter {
    fn displays(&self) -> Vec<Display> {
        self.0.displays().into_iter().map(Into::into).collect()
    }

    fn start(
        &self,
        _grant: &Grant,
        display_id: u32,
        target: VideoTarget,
        sink: Arc<dyn FrameSink>,
    ) -> Result<Arc<dyn Capture>> {
        let session = SESSIONS.next.fetch_add(1, Ordering::Relaxed) + 1;
        lock(&SESSIONS.sinks).insert(session, sink);
        let r = self.0.start(session, display_id, target.into());
        if !r.is_empty() {
            lock(&SESSIONS.sinks).remove(&session);
            let reason = match r.as_str() {
                "consent_declined" | "no_display" => r,
                _ => "capture_failed".into(),
            };
            return Err(CoreError::Refused(reason));
        }
        Ok(Arc::new(KotlinCapture {
            host: self.0.clone(),
            session,
        }))
    }
}

struct EventsAdapter(Arc<dyn ScreenListener>);

impl ScreenEvents for EventsAdapter {
    fn opened(&self, peer: DeviceId, activity: u64) {
        self.0.on_opened(peer.hex(), activity);
    }

    fn displays(&self, peer: DeviceId, activity: u64, displays: Vec<Display>) {
        self.0.on_displays(
            peer.hex(),
            activity,
            displays.into_iter().map(Into::into).collect(),
        );
    }

    fn stream_started(&self, peer: DeviceId, activity: u64, display_id: u32) {
        self.0.on_stream_started(peer.hex(), activity, display_id);
    }

    fn frame(&self, peer: DeviceId, activity: u64, f: VideoFrame) {
        self.0.on_frame(
            peer.hex(),
            activity,
            FfiVideoFrame {
                pts_us: f.pts_us,
                keyframe: f.keyframe,
                data: f.data,
                width: f.width,
                height: f.height,
            },
        );
    }

    fn error(&self, peer: DeviceId, activity: u64, reason: &str) {
        self.0.on_error(peer.hex(), activity, reason.to_owned());
    }

    fn ended(&self, peer: DeviceId, activity: u64) {
        self.0.on_ended(peer.hex(), activity);
    }
}

impl UrDeviceNode {
    fn screen_service(&self) -> Result<Arc<Screen>, FfiError> {
        self.node.service::<Screen>().ok_or(FfiError::NotFound)
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Give the core this phone's screen capture.
    pub fn screen_set_host(&self, host: Arc<dyn ScreenHostFfi>) -> Result<(), FfiError> {
        self.screen_service()?.host.set(Arc::new(HostAdapter(host)));
        Ok(())
    }

    /// Listen for viewer events.
    pub fn screen_set_listener(&self, listener: Arc<dyn ScreenListener>) -> Result<(), FfiError> {
        self.screen_service()?
            .events
            .set(Arc::new(EventsAdapter(listener)));
        Ok(())
    }

    /// False while the network is behind: MediaCodec may skip this frame.
    pub fn screen_ready(&self, session: u64) -> bool {
        lock(&SESSIONS.sinks)
            .get(&session)
            .is_some_and(|s| s.ready())
    }

    /// One encoded frame of capture `session`. Dropped when the session
    /// ended or the network is behind.
    pub fn screen_push_frame(&self, session: u64, frame: FfiVideoFrame) {
        let sink = lock(&SESSIONS.sinks).get(&session).cloned();
        if let Some(s) = sink {
            s.push(EncodedFrame {
                pts_us: frame.pts_us,
                keyframe: frame.keyframe,
                data: frame.data,
                width: frame.width,
                height: frame.height,
            });
        }
    }

    /// Capture `session` ended on its own (the user stopped it from the
    /// system, the projection was revoked): `stopped` or `capture_failed`.
    pub fn screen_capture_ended(&self, session: u64, reason: String) {
        let sink = lock(&SESSIONS.sinks).remove(&session);
        if let Some(s) = sink {
            let r = if reason == "stopped" {
                "stopped"
            } else {
                "capture_failed"
            };
            s.failed(r);
        }
    }

    /// Ask `device` to share its screen; the answer arrives in
    /// [`ScreenListener::on_opened`] or [`ScreenListener::on_error`].
    pub fn screen_request_view(&self, device: String) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        Ok(self.screen_service()?.request_view(peer)?)
    }

    /// Ask for the host's displays.
    pub fn screen_list_displays(&self, device: String, activity: u64) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        Ok(self.screen_service()?.list_displays(peer, activity)?)
    }

    /// Start or switch the stream. 0 means no limit.
    pub fn screen_start(
        &self,
        device: String,
        activity: u64,
        display_id: u32,
        max_width: u32,
        max_height: u32,
        max_fps: u32,
    ) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        Ok(self
            .screen_service()?
            .start(peer, activity, display_id, max_width, max_height, max_fps)?)
    }

    /// The decoder lost sync.
    pub fn screen_request_keyframe(&self, device: String, activity: u64) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        Ok(self.screen_service()?.request_keyframe(peer, activity)?)
    }

    /// End a viewing session.
    pub fn screen_stop(&self, device: String, activity: u64) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        self.screen_service()?.stop(peer, activity);
        Ok(())
    }
}
