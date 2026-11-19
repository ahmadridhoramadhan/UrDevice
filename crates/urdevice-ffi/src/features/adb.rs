//! Remote DevTools and ADB (#35): bindings for the Android app.
//!
//! Slot for the adb feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `adb_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
