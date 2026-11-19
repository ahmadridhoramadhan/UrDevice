//! HTTP request inspector (#34): bindings for the Android app.
//!
//! Slot for the inspector feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `inspector_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
