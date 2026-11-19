//! SMS and calls from the desktop (#29): the desktop side.
//!
//! The view is `ui/features/telephony.slint` (global `TelephonyFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Telephony;

impl Feature for Telephony {
    fn id(&self) -> &'static str {
        "telephony"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Telephony,
            key: "telephony.v1",
        }]
    }
}
