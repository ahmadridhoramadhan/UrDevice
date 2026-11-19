//! Shared mouse and keyboard across desktops (#30): the desktop side.
//!
//! The view is `ui/features/kvm.slint` (global `KvmFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Kvm;

impl Feature for Kvm {
    fn id(&self) -> &'static str {
        "kvm"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Kvm,
            key: "kvm.v1",
        }]
    }
}
