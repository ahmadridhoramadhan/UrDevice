//! Phone or tablet as a second display (#31): bindings for the Android app.
//!
//! Slot for the display feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `display_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
