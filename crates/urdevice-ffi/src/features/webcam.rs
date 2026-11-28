//! Phone as webcam and microphone (#28): bindings for the Android app.
//!
//! **Camera host.** Kotlin opens the camera (CameraX) and encodes with MediaCodec
//! ([`CameraHostFfi`]). A camera stream is the same as a screen capture to the core, so
//! Kotlin hands encoded frames back with [`UrDeviceNode::screen_push_frame`] and
//! reports the end of a stream with [`UrDeviceNode::screen_capture_ended`]; the session
//! numbers are shared with the screen host. A phone is not a viewer of other devices'
//! cameras, so there is no listener here.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use urdevice_core::services::Grant;
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
