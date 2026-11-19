//! Shared UrDevice core.
//!
//! The core holds everything that is the same on every device: identity,
//! trust store, discovery, transport, pairing, sessions, and permissions. It
//! talks to the operating system only through the traits in [`platform`] and
//! [`secret`]. Desktop adapters live in `urdevice-platform`; on Android the
//! same traits become UniFFI callback interfaces implemented in Kotlin
//! (design §9).

pub mod crypto;
pub mod discovery;
pub mod error;
pub mod identity;
pub mod logging;
pub mod node;
pub mod pairing;
pub mod permission;
pub mod platform;
pub mod qr;
pub mod secret;
pub mod services;
pub mod store;
pub mod transport;

pub use error::{CoreError, Result};
pub use node::{Node, NodeConfig, NodeEvent, Snapshot};
pub use permission::{Capability, Mode};

/// The version of the UrDevice core, taken from the crate version.
///
/// Every app shows this value so the desktop and Android builds can be compared.
pub fn core_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_version_matches_crate_version() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
        assert!(!core_version().is_empty());
    }
}
