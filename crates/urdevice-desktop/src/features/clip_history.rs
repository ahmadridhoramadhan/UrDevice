//! Shared clipboard history (#37): the desktop side.
//!
//! The view is `ui/features/clip_history.slint` (global `ClipHistoryFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct ClipHistory;

impl Feature for ClipHistory {
    fn id(&self) -> &'static str {
        "clip_history"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::ClipHistory,
            key: "cliphist.v1",
        }]
    }
}
