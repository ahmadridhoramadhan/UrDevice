//! Shared clipboard history (#37): desktop OS adapters.
//!
//! Slot for the clip_history feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`clip_history/mod.rs`, `clip_history/linux.rs`, ...).
