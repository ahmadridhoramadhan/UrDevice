//! Localhost sharing and Expose on LAN (#22): the desktop side.
//!
//! The view is `ui/features/tunnel.slint` (global `TunnelFeature`). See
//! `docs/dev/adding-a-feature.md`.

use urdevice_core::Capability;

use super::{Feature, TileSpec};

pub(crate) struct Tunnel;

impl Feature for Tunnel {
    fn id(&self) -> &'static str {
        "tunnel"
    }

    fn tiles(&self) -> &'static [TileSpec] {
        &[TileSpec {
            cap: Capability::Tunnel,
            key: "tunnel.v1",
        }]
    }
}
