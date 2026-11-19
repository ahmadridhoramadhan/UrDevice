//! Remote file browser and search (#36): bindings for the Android app.
//!
//! Slot for the file_browser feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `file_browser_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
