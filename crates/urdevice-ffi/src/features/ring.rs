//! Find my device (#27): bindings for the Android app.
//!
//! Slot for the ring feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `ring_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
