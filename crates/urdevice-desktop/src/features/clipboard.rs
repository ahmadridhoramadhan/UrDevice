//! Clipboard sharing (#16): the desktop side.
//!
//! The view is `ui/features/clipboard.slint` (global `ClipboardFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Clipboard;

impl Feature for Clipboard {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Clipboard,
            key: "clipboard.v1",
        }]
    }
}
