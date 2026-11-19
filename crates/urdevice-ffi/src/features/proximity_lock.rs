//! Lock the desktop with the phone (#38): bindings for the Android app.
//!
//! Slot for the proximity_lock feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `proximity_lock_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
