//! SMS and calls from the desktop (#29): bindings for the Android app.
//!
//! Slot for the telephony feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `telephony_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
