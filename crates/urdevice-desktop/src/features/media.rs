//! Media control (#20): the desktop side.
//!
//! The view is `ui/features/media.slint` (global `MediaFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Media;

impl Feature for Media {
    fn id(&self) -> &'static str {
        "media"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Media,
            key: "media.v1",
        }]
    }
}
