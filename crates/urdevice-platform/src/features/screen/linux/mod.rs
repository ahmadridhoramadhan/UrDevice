//! Linux capture: the ScreenCast portal and PipeWire on Wayland, `GetImage`
//! on X11 (design §10).

use std::sync::Arc;

use urdevice_core::Result;

use super::{CaptureHandle, Display, FrameHandler};

mod pipewire;
mod pod;
mod portal;
mod x11;

/// True when this session should use the portal.
fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
}

pub(super) fn displays() -> Vec<Display> {
    if wayland() {
        // The portal dialog lets the host's user pick the monitor.
        return vec![Display {
            id: 0,
            name: "Screen (chosen on the shared device)".into(),
            width: 0,
            height: 0,
            primary: true,
        }];
    }
    x11::displays().unwrap_or_default()
}

pub(super) fn start(id: u32, fps: u32, handler: Arc<dyn FrameHandler>) -> Result<CaptureHandle> {
    if wayland() {
        portal::start(fps, handler)
    } else {
        x11::start(id, fps, handler)
    }
}
