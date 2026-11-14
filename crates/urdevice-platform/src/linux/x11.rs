//! X11 back-end (XTest, XFixes). Stubs for now.

use crate::{Platform, stub};

pub(crate) fn platform() -> Platform {
    stub::platform("linux-x11")
}
