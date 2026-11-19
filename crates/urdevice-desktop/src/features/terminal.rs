//! Remote terminal (#32): the desktop side.
//!
//! The view is `ui/features/terminal.slint` (global `TerminalFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct Terminal;

impl Feature for Terminal {
    fn id(&self) -> &'static str {
        "terminal"
    }
}
