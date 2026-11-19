//! Remote file browser and search (#36): the desktop side.
//!
//! The view is `ui/features/file_browser.slint` (global `FileBrowserFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct FileBrowser;

impl Feature for FileBrowser {
    fn id(&self) -> &'static str {
        "file_browser"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::FsBrowse,
            key: "fs.browse.v1",
        }]
    }
}
