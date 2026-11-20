//! Remote control: pointer, keyboard, text, and presenter (#18, #19): bindings for the Android app.
//!
//! The phone is the **controller** here: the Mousepad, Presenter, and Keyboard
//! screens call the `input_*` methods, and [`InputListener`] reports the
//! session, what the host can do, and refusals. Events are queued in the core
//! and sent every 8 ms. Nothing here ever carries or logs typed text back.

use std::sync::Arc;

use urdevice_core::identity::DeviceId;
use urdevice_core::services::input::{HostStatus, Input, InputEvents, PermStatus};

use crate::{FfiError, UrDeviceNode, device_id};

/// Told about the session, never about content.
#[uniffi::export(with_foreign)]
pub trait InputListener: Send + Sync {
    /// Runs on a core thread; post UI work to the main thread.
    fn on_event(&self, event: FfiInputEvent);
}

/// What happened to a remote-control session.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiInputEvent {
    /// `session` (the session started or ended), `status` (what the host can
    /// do), or `refused`.
    pub kind: String,
    /// The host (hex).
    pub device_id: String,
    /// For `session`: `start` or `end`. For `status`: `ready`, `unavailable`,
    /// or `missing`. For `refused`: the host's reason (`denied`, `declined`,
    /// `timeout`, `unsupported`, `offline`, `ask`, `no_activity`).
    pub detail: String,
    /// For `status` with `missing`: `accessibility`, `uinput`, or `portal`.
    pub missing: String,
    /// For `status`: the host's back-end name, display only.
    pub backend: String,
}

struct Listener(Arc<dyn InputListener>);

impl Listener {
    fn send(&self, kind: &str, peer: DeviceId, detail: &str, missing: &str, backend: &str) {
        self.0.on_event(FfiInputEvent {
            kind: kind.into(),
            device_id: peer.hex(),
            detail: detail.into(),
            missing: missing.into(),
            backend: backend.into(),
        });
    }
}

impl InputEvents for Listener {
    fn session(&self, peer: DeviceId, active: bool) {
        self.send(
            "session",
            peer,
            if active { "start" } else { "end" },
            "",
            "",
        );
    }

    fn host_status(&self, peer: DeviceId, s: &HostStatus) {
        let (detail, missing) = match s.status {
            PermStatus::Ready => ("ready", ""),
            PermStatus::Missing(m) => ("missing", m.as_str()),
            PermStatus::Unavailable => ("unavailable", ""),
        };
        self.send("status", peer, detail, missing, &s.backend);
    }

    fn refused(&self, peer: DeviceId, reason: &str) {
        self.send("refused", peer, reason, "", "");
    }
}

impl UrDeviceNode {
    fn input(&self) -> Result<Arc<Input>, FfiError> {
        self.node.service::<Input>().ok_or(FfiError::NotFound)
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Set the listener for remote-control events.
    pub fn input_set_listener(&self, listener: Arc<dyn InputListener>) {
        if let Ok(i) = self.input() {
            i.set_events(Arc::new(Listener(listener)));
        }
    }

    /// Start a remote-control session with `device`. The host's user approves
    /// it under Always Ask; the listener hears `session` or `refused`.
    pub fn input_connect(&self, device: String) -> Result<(), FfiError> {
        Ok(self.input()?.connect(device_id(&device)?)?)
    }

    /// End the session with `device`.
    pub fn input_end(&self, device: String) -> Result<(), FfiError> {
        Ok(self.input()?.end(device_id(&device)?)?)
    }

    /// True while a session with `device` is running.
    pub fn input_is_active(&self, device: String) -> bool {
        match (self.input(), device_id(&device)) {
            (Ok(i), Ok(d)) => i.is_active(d),
            _ => false,
        }
    }

    /// True while a session with `device` is waiting for the host's answer.
    pub fn input_is_connecting(&self, device: String) -> bool {
        match (self.input(), device_id(&device)) {
            (Ok(i), Ok(d)) => i.is_connecting(d),
            _ => false,
        }
    }

    /// Move the host's pointer by `dx`, `dy` pixels.
    pub fn input_move(&self, device: String, dx: i32, dy: i32) -> Result<(), FfiError> {
        Ok(self.input()?.pointer_move(device_id(&device)?, dx, dy)?)
    }

    /// Turn the host's wheel: 120 is one notch, down and right are positive.
    pub fn input_scroll(&self, device: String, dx: i32, dy: i32) -> Result<(), FfiError> {
        Ok(self.input()?.scroll(device_id(&device)?, dx, dy)?)
    }

    /// Press or release a mouse button: 0 left, 1 right, 2 middle.
    pub fn input_button(&self, device: String, button: u32, down: bool) -> Result<(), FfiError> {
        Ok(self.input()?.button(device_id(&device)?, button, down)?)
    }

    /// Press or release a key. `usage` is a USB HID keyboard usage;
    /// `modifiers` are 1 Ctrl, 2 Shift, 4 Alt, 8 Meta, held around the key.
    pub fn input_key(
        &self,
        device: String,
        usage: u32,
        down: bool,
        modifiers: u32,
    ) -> Result<(), FfiError> {
        Ok(self
            .input()?
            .key(device_id(&device)?, usage, down, modifiers)?)
    }

    /// Press and release a key with modifiers (a special key or a shortcut).
    pub fn input_tap_key(
        &self,
        device: String,
        usage: u32,
        modifiers: u32,
    ) -> Result<(), FfiError> {
        Ok(self
            .input()?
            .tap_key(device_id(&device)?, usage, modifiers)?)
    }

    /// Type Unicode text on the host.
    pub fn input_text(&self, device: String, text: String) -> Result<(), FfiError> {
        Ok(self.input()?.text(device_id(&device)?, &text)?)
    }

    /// Show the presenter's laser at `x`, `y` (0.0 to 1.0 across and down the
    /// host's display), or hide it with `on = false`.
    pub fn input_laser(&self, device: String, x: f32, y: f32, on: bool) -> Result<(), FfiError> {
        Ok(self.input()?.laser(device_id(&device)?, x, y, on)?)
    }
}
