//! Notification mirroring (#26): desktop OS adapters.
//!
//! Slot for the notify feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`notify/mod.rs`, `notify/linux.rs`, ...).
