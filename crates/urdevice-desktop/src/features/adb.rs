//! Remote DevTools and ADB (#35): the desktop side.
//!
//! The view is `ui/features/adb.slint` (global `AdbFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Adb;

impl Feature for Adb {
    fn id(&self) -> &'static str {
        "adb"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Adb,
            key: "adb.v1",
        }]
    }
}
