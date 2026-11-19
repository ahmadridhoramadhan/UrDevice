//! Remote file browser and search (#36): desktop OS adapters.
//!
//! Slot for the file_browser feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`file_browser/mod.rs`, `file_browser/linux.rs`, ...).
