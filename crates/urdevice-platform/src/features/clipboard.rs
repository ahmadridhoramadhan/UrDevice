//! Clipboard sharing (#16): desktop OS adapters.
//!
//! Slot for the clipboard feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`clipboard/mod.rs`, `clipboard/linux.rs`, ...).
