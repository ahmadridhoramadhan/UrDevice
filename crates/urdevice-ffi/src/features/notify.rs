//! Notification mirroring (#26): bindings for the Android app.
//!
//! Slot for the notify feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `notify_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
