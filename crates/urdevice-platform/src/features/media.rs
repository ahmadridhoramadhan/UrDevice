//! Media control (#20): desktop OS adapters.
//!
//! Slot for the media feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`media/mod.rs`, `media/linux.rs`, ...).
