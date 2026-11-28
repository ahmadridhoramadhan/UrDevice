//! Desktop feature screens (design §12). One Rust module here and one Slint
//! file in `ui/features/` per feature, so features never edit `app.rs` or
//! `main.slint`. See `docs/dev/adding-a-feature.md`.
//!
//! The device view asks every [`Feature`] for its tiles. A tile whose feature
//! has a view opens `ui/features/<id>.slint` through `FeatureView`; any other
//! tile only sends an activity request, as before the feature existed.

use std::sync::Arc;

use urdevice_core::node::{PeerView, Snapshot};
use urdevice_core::{Capability, Node};

use crate::ui::MainWindow;

mod list;

mod adb;
mod automations;
mod bluetooth;
mod clip_history;
mod clipboard;
mod display;
mod file_browser;
mod files;
mod gallery;
mod input;
mod inspector;
mod kvm;
mod media;
mod mic;
mod notify;
mod port_profiles;
mod proximity_lock;
mod ring;
mod screen;
mod speaker;
mod telephony;
mod terminal;
mod tunnel;
mod webcam;

pub(crate) use input::select_backend;
pub(crate) use list::all;

/// One tile on the paired-device view.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TileSpec {
    /// The host's capability the tile uses. The tile shows only when the
    /// host lists it, and follows its Allow / Always Ask / Deny.
    pub cap: Capability,
    /// Picks the label (`Labels.tile-label`) and the icon (`app::icon`).
    pub key: &'static str,
}

/// What a feature gets to work with.
// Read by the feature modules once they are built.
#[allow(dead_code)]
pub(crate) struct FeatureCtx {
    pub window: slint::Weak<MainWindow>,
    pub node: Arc<Node>,
}

/// One desktop feature. Every method has a default, so a stub is just an id.
/// All methods run on the Slint thread.
pub(crate) trait Feature {
    /// The feature id: the module name, and the `feature` value the Slint
    /// `FeatureView` switches on.
    fn id(&self) -> &'static str;

    /// Tiles on the paired-device view, in order.
    fn tiles(&self) -> &'static [TileSpec] {
        &[]
    }

    /// True when a tile opens this feature's view instead of sending a
    /// plain activity request.
    fn has_view(&self) -> bool {
        false
    }

    /// True when a tile opens a window of its own: the page does not change, and `open` is
    /// called at once (screen sharing).
    fn direct(&self) -> bool {
        false
    }

    /// Called once after the window and the node exist: wire the Slint
    /// global's callbacks, give the core service its desktop adapters
    /// (`urdevice_platform::features::<id>`), and set its listener.
    fn start(&self, ctx: &FeatureCtx) {
        let _ = ctx;
    }

    /// Called on every redraw with a fresh snapshot.
    fn refresh(&self, ctx: &FeatureCtx, snap: &Snapshot) {
        let _ = (ctx, snap);
    }

    /// For a tile that is a plain switch (no view, no settings icon): its state
    /// for `peer` (Device ID, hex). `None` for a normal tile.
    fn switch(&self, ctx: &FeatureCtx, peer: &str) -> Option<bool> {
        let _ = (ctx, peer);
        None
    }

    /// The user flipped the switch tile of `peer`.
    fn set_switch(&self, ctx: &FeatureCtx, peer: &str, on: bool) {
        let _ = (ctx, peer, on);
    }

    /// A tile opened the view for `peer`. `tile` is the [`TileSpec::key`].
    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, tile: &str) {
        let _ = (ctx, peer, tile);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn ids_are_unique_and_match_the_slint_switch() {
        let features = all();
        let ids: HashSet<_> = features.iter().map(|f| f.id()).collect();
        assert_eq!(ids.len(), features.len());
        let slint = include_str!("../../ui/features/features.slint");
        for id in ids {
            assert!(
                slint.contains(&format!("root.feature == \"{id}\"")),
                "{id} is missing in ui/features/features.slint"
            );
        }
    }

    #[test]
    fn release_1_tiles_keep_their_order() {
        let keys: Vec<_> = all()
            .iter()
            .flat_map(|f| f.tiles().iter())
            .filter(|t| Capability::RELEASE_1.contains(&t.cap))
            .map(|t| t.key)
            .collect();
        assert_eq!(
            keys,
            [
                "clipboard.v1",
                "files.v1",
                "gallery.v1",
                "input.v1",
                "screen.h264.v1",
                "tunnel.v1"
            ]
        );
    }
}
