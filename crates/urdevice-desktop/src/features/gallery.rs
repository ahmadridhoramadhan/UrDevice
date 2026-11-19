//! Android gallery (#17): the desktop side.
//!
//! The view is `ui/features/gallery.slint` (global `GalleryFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Gallery;

impl Feature for Gallery {
    fn id(&self) -> &'static str {
        "gallery"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Gallery,
            key: "gallery.v1",
        }]
    }
}
