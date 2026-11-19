//! Android gallery (#17): bindings for the Android app.
//!
//! Slot for the gallery feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `gallery_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
