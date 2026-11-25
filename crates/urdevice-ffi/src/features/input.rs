//! Remote control: pointer, keyboard, text, and presenter (#18, #19): bindings for the Android app.
//!
//! The phone is the **controller** here: the Mousepad, Presenter, and Keyboard
//! screens call the `input_*` methods, and [`InputListener`] reports the
//! session, what the host can do, and refusals. Events are queued in the core
//! and sent every 8 ms. Nothing here ever carries or logs typed text back.
//!
//! The phone is also a **host** (#19): [`InputHostFfi`] is the Kotlin side of
//! the accessibility service (taps, swipes, Back, Home, Recents) and the
//! UrDevice keyboard (text and keys). The core builds the gestures from the
//! controller's pointer events and keeps the permission checks; Kotlin only
//! runs what it is handed.

use std::sync::Arc;

use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::input::{
    Gesture, HostStatus, Input, InputEvents, Missing, PermStatus, TouchHost, TouchSink,
};
use urdevice_core::{CoreError, Result};

use crate::{FfiError, UrDeviceNode, device_id};

/// Told about the session, never about content.
#[uniffi::export(with_foreign)]
pub trait InputListener: Send + Sync {
    /// Runs on a core thread; post UI work to the main thread.
    fn on_event(&self, event: FfiInputEvent);
}

/// The phone's accessibility service and keyboard, in Kotlin (#19, R9.3,
/// R10.3). Called from the core's input worker, one call at a time. Each
/// action method returns false when Android refused it. Never log the text.
#[uniffi::export(with_foreign)]
pub trait InputHostFfi: Send + Sync {
    /// The UrDevice accessibility service is on and connected.
    fn gestures_ready(&self) -> bool;
    /// The UrDevice keyboard is on and is the keyboard in use.
    fn text_ready(&self) -> bool;
    /// Width of the display in pixels; 0 when unknown.
    fn display_width(&self) -> u32;
    /// Height of the display in pixels; 0 when unknown.
    fn display_height(&self) -> u32;
    /// Touch one point for `duration_ms`.
    fn tap(&self, x: u32, y: u32, duration_ms: u32) -> bool;
    /// Drag through the points `(xs[i], ys[i])` in `duration_ms`.
    fn swipe(&self, xs: Vec<u32>, ys: Vec<u32>, duration_ms: u32) -> bool;
    /// Global action: 0 Back, 1 Home, 2 Recents.
    fn nav(&self, action: u32) -> bool;
    /// Commit text in the focused field. No newlines.
    fn text(&self, text: String) -> bool;
    /// Send an Android key code to the focused field with `meta` state bits.
    fn key(&self, code: u32, down: bool, meta: u32) -> bool;
    /// Draw the pointer on the screen at `(x, y)` pixels, or remove it. Cosmetic.
    fn cursor(&self, x: u32, y: u32, shown: bool);
}

struct HostAdapter(Arc<dyn InputHostFfi>);

fn refused(what: &'static str) -> CoreError {
    CoreError::Io(std::io::Error::other(what))
}

impl TouchHost for HostAdapter {
    fn gestures_ready(&self) -> bool {
        self.0.gestures_ready()
    }

    fn text_ready(&self) -> bool {
        self.0.text_ready()
    }

    fn display_size(&self) -> (u32, u32) {
        (self.0.display_width(), self.0.display_height())
    }

    fn gesture(&self, _grant: &Grant, gesture: &Gesture) -> Result<()> {
        let ok = match gesture {
            Gesture::Tap { x, y, duration_ms } => self.0.tap(*x, *y, *duration_ms),
            Gesture::Swipe {
                points,
                duration_ms,
            } => self.0.swipe(
                points.iter().map(|p| p.0).collect(),
                points.iter().map(|p| p.1).collect(),
                *duration_ms,
            ),
        };
        ok.then_some(()).ok_or_else(|| refused("gesture refused"))
    }

    fn nav(&self, _grant: &Grant, action: u32) -> Result<()> {
        self.0
            .nav(action)
            .then_some(())
            .ok_or_else(|| refused("navigation action refused"))
    }

    fn text(&self, _grant: &Grant, text: &str) -> Result<()> {
        self.0
            .text(text.to_owned())
            .then_some(())
            .ok_or_else(|| refused("text refused"))
    }

    fn key(&self, _grant: &Grant, code: u32, down: bool, meta: u32) -> Result<()> {
        self.0
            .key(code, down, meta)
            .then_some(())
            .ok_or_else(|| refused("key refused"))
    }

    fn cursor(&self, _grant: &Grant, x: u32, y: u32, shown: bool) -> Result<()> {
        self.0.cursor(x, y, shown);
        Ok(())
    }
}

/// What happened to a remote-control session.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiInputEvent {
    /// `session` (the session started or ended), `status` (what the host can
    /// do), `refused`, or `host_missing` (this phone is being controlled but
    /// needs `accessibility_service` or `input_method` turned on; see
    /// `missing`).
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
    /// For `status`: the host's display width in pixels, 0 when unknown.
    pub width: u32,
    /// For `status`: the host's display height in pixels, 0 when unknown.
    pub height: u32,
    /// For `status` with `ready`: `input_method` when typing is not possible
    /// on the host, otherwise empty.
    pub text_missing: String,
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
            width: 0,
            height: 0,
            text_missing: String::new(),
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
        let (width, height) = s.display.unwrap_or((0, 0));
        self.0.on_event(FfiInputEvent {
            kind: "status".into(),
            device_id: peer.hex(),
            detail: detail.into(),
            missing: missing.into(),
            backend: s.backend.clone(),
            width,
            height,
            text_missing: s.text_missing.map_or("", Missing::as_str).into(),
        });
    }

    fn missing_permission(&self, peer: DeviceId, missing: Missing) {
        self.send("host_missing", peer, "", missing.as_str(), "");
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

    /// Give the core this phone's accessibility service and keyboard, so a
    /// paired device can control it once the user approves (#19).
    pub fn input_set_host(&self, host: Arc<dyn InputHostFfi>) {
        if let Ok(i) = self.input() {
            i.set_sink(Arc::new(TouchSink::new(Arc::new(HostAdapter(host)))));
        }
    }

    /// Run Back (0), Home (1), or Recents (2) on `device`. Android hosts only.
    pub fn input_nav(&self, device: String, action: u32) -> Result<(), FfiError> {
        Ok(self.input()?.nav(device_id(&device)?, action)?)
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

    /// Put the host's pointer at `x`, `y` (0.0 to 1.0 across and down its primary
    /// display). Not available on Wayland and `uinput` hosts.
    pub fn input_abs(&self, device: String, x: f32, y: f32) -> Result<(), FfiError> {
        let to = |v: f32| (v.clamp(0.0, 1.0) * 65535.0).round() as u32;
        Ok(self
            .input()?
            .pointer_abs(device_id(&device)?, to(x), to(y), 0)?)
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
