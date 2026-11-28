//! Phone as webcam and microphone (#28, design §18.1).
//!
//! Service ids: `ServiceId::CAMERA`, `ServiceId::MIC`.
//!
//! **Camera (`camera.v1`).** The phone's camera is the screen service with another id: a
//! camera is a "display", the picture is the video stream, and the viewer adds flash and
//! zoom ([`super::screen::Screen::control`]). [`Camera`] wraps a [`Screen`] made with
//! [`Screen::for_camera`], so `node.service::<Camera>()` finds it and the screen one stays
//! separate. The permission engine sees `camera.v1`: it is watched, so every stream is an
//! activity with the indicator and Stop on the phone (R5.4, R17.6), and it defaults to
//! Always Ask. Only Android hosts offer it (`Capability::android_only`).
//!
//! **Microphone (`mic.v1`, T11.5).** A service of its own, [`super::mic::Mic`]: the phone's
//! microphone is a stream of PCM chunks to the desktop, which plays it into a virtual source.

use std::sync::Arc;

use super::screen::Screen;
use super::{Message, Registry, Service, ServiceCtx, ServiceId, Stream};
use crate::Result;
use crate::identity::DeviceId;
use crate::permission::Capability;

/// Release 2 capabilities this module ships. Add each one when the feature
/// is done: the capability then shows in the settings and the handshake.
pub(crate) const SHIPPED: &[Capability] = &[Capability::Camera, Capability::Mic];

/// The phone-as-webcam service. UI code reaches it with `node.service::<Camera>()` and then
/// uses [`Camera::screen`] for the calls.
pub struct Camera {
    screen: Arc<Screen>,
}

impl Camera {
    fn new() -> Self {
        Self {
            screen: Arc::new(Screen::for_camera()),
        }
    }

    /// The service that does the work. Set its `host` (the phone's camera) or its `events`
    /// (the viewer) and call `request_view`, `start`, `control`, and `stop` on it.
    pub fn screen(&self) -> &Arc<Screen> {
        &self.screen
    }
}

impl Service for Camera {
    fn id(&self) -> ServiceId {
        ServiceId::CAMERA
    }

    fn capability(&self) -> Capability {
        Capability::Camera
    }

    fn start(&self, ctx: ServiceCtx) {
        // Not `self.screen.start`: `Screen` has its own `start` for the viewer.
        Service::start(self.screen.as_ref(), ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        self.screen.on_message(ctx, msg)
    }

    fn on_stream(&self, ctx: &ServiceCtx, stream: Stream) {
        self.screen.on_stream(ctx, stream);
    }

    fn on_refused(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64, reason: &str) {
        self.screen.on_refused(ctx, peer, activity, reason);
    }

    fn on_activity_end(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        self.screen.on_activity_end(ctx, peer, activity);
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        self.screen.on_peer(ctx, peer, connected);
    }
}

/// Add this feature's services to a starting node. `platform` is `linux`,
/// `windows`, `macos`, or `android`.
pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Camera::new()));
    super::mic::register(r, _platform);
}
