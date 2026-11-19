//! Bluetooth fallback (#23): bindings for the Android app.
//!
//! Slot for the bluetooth feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `bluetooth_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
