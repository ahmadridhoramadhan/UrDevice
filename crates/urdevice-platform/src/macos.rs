//! macOS adapters (AppKit / CoreGraphics). Stubs for now.

use crate::{Platform, stub};

pub(crate) fn platform() -> Platform {
    stub::platform("macos")
}
