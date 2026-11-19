//! Lock the desktop with the phone (#38): the desktop side.
//!
//! The view is `ui/features/proximity_lock.slint` (global `ProximityLockFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct ProximityLock;

impl Feature for ProximityLock {
    fn id(&self) -> &'static str {
        "proximity_lock"
    }
}
