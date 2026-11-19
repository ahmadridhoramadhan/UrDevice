//! Remote control: pointer, keyboard, text, and presenter (#18, #19): the desktop side.
//!
//! The view is `ui/features/input.slint` (global `InputFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Input;

impl Feature for Input {
    fn id(&self) -> &'static str {
        "input"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[
            TileSpec {
                cap: Capability::Input,
                key: "input.v1",
            },
            TileSpec {
                cap: Capability::Input,
                key: "keyboard",
            },
        ]
    }
}
