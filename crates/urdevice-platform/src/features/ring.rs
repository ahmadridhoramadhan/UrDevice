//! Find my device (#27): desktop OS adapters.
//!
//! Slot for the ring feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`ring/mod.rs`, `ring/linux.rs`, ...).
