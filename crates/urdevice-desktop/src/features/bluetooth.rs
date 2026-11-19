//! Bluetooth fallback (#23): the desktop side.
//!
//! The view is `ui/features/bluetooth.slint` (global `BluetoothFeature`). See
//! `docs/dev/adding-a-feature.md`.

use super::Feature;

pub(crate) struct Bluetooth;

impl Feature for Bluetooth {
    fn id(&self) -> &'static str {
        "bluetooth"
    }
}
