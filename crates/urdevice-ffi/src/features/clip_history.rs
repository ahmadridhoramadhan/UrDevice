//! Shared clipboard history (#37): bindings for the Android app.
//!
//! Slot for the clip_history feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `clip_history_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
