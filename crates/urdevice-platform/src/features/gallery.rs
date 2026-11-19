//! Android gallery (#17): desktop OS adapters.
//!
//! Slot for the gallery feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`gallery/mod.rs`, `gallery/linux.rs`, ...).
