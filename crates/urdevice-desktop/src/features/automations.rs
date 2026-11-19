//! Automations (#39): the desktop side.
//!
//! The view is `ui/features/automations.slint` (global `AutomationsFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct Automations;

impl Feature for Automations {
    fn id(&self) -> &'static str {
        "automations"
    }
}
