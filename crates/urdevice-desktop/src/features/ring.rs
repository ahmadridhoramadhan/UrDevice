//! Find my device (#27): the desktop side.
//!
//! The view is `ui/features/ring.slint` (global `RingFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Ring;

impl Feature for Ring {
    fn id(&self) -> &'static str {
        "ring"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Ring,
            key: "ring.v1",
        }]
    }
}
