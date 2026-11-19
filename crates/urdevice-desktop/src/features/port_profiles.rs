//! Port forwarding profiles (#33): the desktop side.
//!
//! The view is `ui/features/port_profiles.slint` (global `PortProfilesFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct PortProfiles;

impl Feature for PortProfiles {
    fn id(&self) -> &'static str {
        "port_profiles"
    }
}
