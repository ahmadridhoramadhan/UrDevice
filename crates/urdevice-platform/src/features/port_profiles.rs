//! Port forwarding profiles (#33): desktop OS adapters.
//!
//! Slot for the port_profiles feature's Windows, macOS, and Linux back-ends. Pick the
//! OS with `#[cfg(target_os = ...)]` inside this module, or turn it into a
//! directory (`port_profiles/mod.rs`, `port_profiles/linux.rs`, ...).
