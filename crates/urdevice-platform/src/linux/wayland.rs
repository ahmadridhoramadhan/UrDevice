//! Wayland back-end (RemoteDesktop/ScreenCast portals, `uinput` fallback). Stubs for now.

use crate::{Platform, stub};

pub(crate) fn platform() -> Platform {
    stub::platform("linux-wayland")
}
