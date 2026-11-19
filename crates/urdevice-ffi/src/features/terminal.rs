//! Remote terminal (#32): bindings for the Android app.
//!
//! Slot for the terminal feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `terminal_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
