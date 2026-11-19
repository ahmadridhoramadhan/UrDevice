//! Screen sharing (#21): the desktop side.
//!
//! The view is `ui/features/screen.slint` (global `ScreenFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Screen;

impl Feature for Screen {
    fn id(&self) -> &'static str {
        "screen"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Screen,
            key: "screen.h264.v1",
        }]
    }
}
