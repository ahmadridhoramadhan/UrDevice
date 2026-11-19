//! Use another device as a speaker (#40): desktop OS adapters.
//!
//! Slot for the speaker feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`speaker/mod.rs`, `speaker/linux.rs`, ...).
