//! Media control (#20): bindings for the Android app.
//!
//! Slot for the media feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `media_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
