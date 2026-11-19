//! File transfer (#17): the desktop side.
//!
//! The view is `ui/features/files.slint` (global `FilesFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Files;

impl Feature for Files {
    fn id(&self) -> &'static str {
        "files"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Files,
            key: "files.v1",
        }]
    }
}
