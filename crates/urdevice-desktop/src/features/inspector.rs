//! HTTP request inspector (#34): the desktop side.
//!
//! The view is `ui/features/inspector.slint` (global `InspectorFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct Inspector;

impl Feature for Inspector {
    fn id(&self) -> &'static str {
        "inspector"
    }
}
