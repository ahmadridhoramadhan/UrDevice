//! Phone or tablet as a second display (#31): the desktop side.
//!
//! The view is `ui/features/display.slint` (global `DisplayFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct Display;

impl Feature for Display {
    fn id(&self) -> &'static str {
        "display"
    }
}
