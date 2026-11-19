//! Notification mirroring (#26): the desktop side.
//!
//! The view is `ui/features/notify.slint` (global `NotifyFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Notify;

impl Feature for Notify {
    fn id(&self) -> &'static str {
        "notify"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Notify,
            key: "notify.v1",
        }]
    }
}
