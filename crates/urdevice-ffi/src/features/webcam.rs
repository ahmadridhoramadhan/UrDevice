//! Phone as webcam and microphone (#28): bindings for the Android app.
//!
//! **Camera host.** Kotlin opens the camera (CameraX) and encodes with MediaCodec
//! ([`CameraHostFfi`]). A camera stream is the same as a screen capture to the core, so
//! Kotlin hands encoded frames back with [`UrDeviceNode::screen_push_frame`] and
//! reports the end of a stream with [`UrDeviceNode::screen_capture_ended`]; the session
//! numbers are shared with the screen host. A phone is not a viewer of other devices'
//! cameras, so there is no listener here.
//!
//! **Microphone host.** Kotlin records with `AudioRecord` ([`MicHostFfi`]). Each recording is a
//! numbered session; Kotlin hands 16-bit PCM chunks back with [`UrDeviceNode::mic_push_chunk`] and
//! reports the end of a recording with [`UrDeviceNode::mic_capture_ended`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use urdevice_core::services::Grant;
use urdevice_core::services::mic::{Mic, MicCapture, MicHost, MicSink};
use urdevice_core::services::screen::{Capture, Display, FrameSink, ScreenHost, VideoTarget};
use urdevice_core::services::webcam::Camera;
use urdevice_core::{CoreError, Result};

use super::screen::{FfiDisplay, FfiVideoTarget, SESSIONS, lock};
use crate::{FfiError, UrDeviceNode};

/// The phone's cameras, implemented in Kotlin. Called from core worker threads; `start`
/// may block while the camera opens.
#[uniffi::export(with_foreign)]
pub trait CameraHostFfi: Send + Sync {
    /// The cameras that can be streamed. `primary` marks the back camera.
    fn cameras(&self) -> Vec<FfiDisplay>;
    /// Open camera `camera_id` and start stream `session`. Returns an empty string on
    /// success, else `no_display` (no such camera), `consent_declined` (the CAMERA
    /// permission was refused), or `capture_failed`.
    fn start(&self, session: u64, camera_id: u32, target: FfiVideoTarget) -> String;
    /// Make the next frame a keyframe.
    fn request_keyframe(&self, session: u64);
    /// The adaptive ladder moved.
    fn set_target(&self, session: u64, target: FfiVideoTarget);
    /// Switch the torch and set the zoom (ratio times 100). Ignore what the camera cannot do.
    fn control(&self, session: u64, flash: bool, zoom_x100: u32);
    /// Stop stream `session` and close the camera.
    fn stop(&self, session: u64);
}

struct HostAdapter(Arc<dyn CameraHostFfi>);

struct KotlinCamera {
    host: Arc<dyn CameraHostFfi>,
    session: u64,
}

impl Capture for KotlinCamera {
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

    fn control(&self, flash: bool, zoom_x100: u32) {
        self.host.control(self.session, flash, zoom_x100);
    }
}

impl ScreenHost for HostAdapter {
    fn displays(&self) -> Vec<Display> {
        self.0.cameras().into_iter().map(Into::into).collect()
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
        Ok(Arc::new(KotlinCamera {
            host: self.0.clone(),
            session,
        }))
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Give the core this phone's cameras.
    pub fn camera_set_host(&self, host: Arc<dyn CameraHostFfi>) -> Result<(), FfiError> {
        let camera = self.node.service::<Camera>().ok_or(FfiError::NotFound)?;
        camera.screen().host.set(Arc::new(HostAdapter(host)));
        Ok(())
    }
}

/// The phone's microphone, implemented in Kotlin. Called from core worker threads; `start` may
/// block while the permission dialog is open.
#[uniffi::export(with_foreign)]
pub trait MicHostFfi: Send + Sync {
    /// Ask for the RECORD_AUDIO permission when it is missing and start recording `session`
    /// (48 kHz, mono, 16-bit). Returns an empty string on success, else `consent_declined` or
    /// `capture_failed`.
    fn start(&self, session: u64) -> String;
    /// Stop recording `session` and release the microphone.
    fn stop(&self, session: u64);
}

/// Sinks of the running recordings, by session number.
#[derive(Default)]
struct MicSessions {
    next: AtomicU64,
    sinks: Mutex<HashMap<u64, Arc<dyn MicSink>>>,
}

static MIC_SESSIONS: std::sync::LazyLock<MicSessions> =
    std::sync::LazyLock::new(MicSessions::default);

struct MicAdapter(Arc<dyn MicHostFfi>);

struct KotlinMic {
    host: Arc<dyn MicHostFfi>,
    session: u64,
}

impl MicCapture for KotlinMic {
    fn stop(&self) {
        lock(&MIC_SESSIONS.sinks).remove(&self.session);
        self.host.stop(self.session);
    }
}

impl MicHost for MicAdapter {
    fn start(&self, _grant: &Grant, sink: Arc<dyn MicSink>) -> Result<Arc<dyn MicCapture>> {
        let session = MIC_SESSIONS.next.fetch_add(1, Ordering::Relaxed) + 1;
        lock(&MIC_SESSIONS.sinks).insert(session, sink);
        let r = self.0.start(session);
        if !r.is_empty() {
            lock(&MIC_SESSIONS.sinks).remove(&session);
            let reason = if r == "consent_declined" {
                r
            } else {
                "capture_failed".into()
            };
            return Err(CoreError::Refused(reason));
        }
        Ok(Arc::new(KotlinMic {
            host: self.0.clone(),
            session,
        }))
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Give the core this phone's microphone.
    pub fn mic_set_host(&self, host: Arc<dyn MicHostFfi>) -> Result<(), FfiError> {
        let mic = self.node.service::<Mic>().ok_or(FfiError::NotFound)?;
        mic.host.set(Arc::new(MicAdapter(host)));
        Ok(())
    }

    /// False while the network is behind: the recorder may skip this chunk.
    pub fn mic_ready(&self, session: u64) -> bool {
        lock(&MIC_SESSIONS.sinks)
            .get(&session)
            .is_some_and(|s| s.ready())
    }

    /// One chunk of 16-bit little-endian PCM (mono, 48 kHz) of recording `session`. Dropped when
    /// the session ended or the network is behind.
    pub fn mic_push_chunk(&self, session: u64, pcm: Vec<u8>) {
        let sink = lock(&MIC_SESSIONS.sinks).get(&session).cloned();
        if let Some(s) = sink {
            s.push(pcm);
        }
    }

    /// Recording `session` ended on its own (the permission was taken away, another app took the
    /// microphone): `stopped` or `capture_failed`.
    pub fn mic_capture_ended(&self, session: u64, reason: String) {
        let sink = lock(&MIC_SESSIONS.sinks).remove(&session);
        if let Some(s) = sink {
            s.failed(&reason);
        }
    }
}
