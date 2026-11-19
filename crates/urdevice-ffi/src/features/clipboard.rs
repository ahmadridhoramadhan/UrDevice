//! Clipboard sharing (#16): bindings for the Android app.
//!
//! Slot for the clipboard feature. Add `#[uniffi::export] impl UrDeviceNode`
//! methods named `clipboard_*`, and `#[uniffi::export(with_foreign)]` traits for
//! Kotlin host adapters and listeners; see `docs/dev/adding-a-feature.md`.
