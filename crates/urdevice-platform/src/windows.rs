//! Windows adapters (Win32 / WinRT). Stubs for now.

use crate::{Platform, stub};

pub(crate) fn platform() -> Platform {
    stub::platform("windows")
}
