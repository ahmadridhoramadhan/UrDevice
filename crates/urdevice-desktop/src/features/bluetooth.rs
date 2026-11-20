//! Bluetooth fallback (#23): the desktop side.
//!
//! Bluetooth is an automatic fallback, not a setting. This module gives the
//! core the `btleplug` GATT central and fills the banner the device view
//! shows while a session runs on Bluetooth LE (`ui/features/bluetooth.slint`,
//! global `BluetoothFeature`). The device view greys out the other tiles
//! itself (`app.rs`, R6.4).

use slint::ComponentHandle;
use urdevice_core::node::{PeerStatus, Snapshot};
use urdevice_core::services::bluetooth::allowed_capability;

use super::{Feature, FeatureCtx};

pub(crate) struct Bluetooth;

impl Feature for Bluetooth {
    fn id(&self) -> &'static str {
        "bluetooth"
    }

    fn start(&self, ctx: &FeatureCtx) {
        ctx.node
            .ble_set_central(urdevice_platform::features::bluetooth::central());
    }

    fn refresh(&self, ctx: &FeatureCtx, snap: &Snapshot) {
        let Some(w) = ctx.window.upgrade() else {
            return;
        };
        let g = w.global::<crate::ui::BluetoothFeature>();
        let selected = w.get_selected_peer().to_string();
        let peer = snap
            .peers
            .iter()
            .find(|p| p.id == selected && p.status == PeerStatus::Connected && p.path == "ble");
        let Some(peer) = peer else {
            if g.get_on_ble() {
                g.set_on_ble(false);
            }
            return;
        };
        let labels = w.global::<crate::ui::Labels>();
        let mut names: Vec<String> = Vec::new();
        for p in &peer.remote_permissions {
            if allowed_capability(p.capability) {
                continue;
            }
            let name = labels
                .invoke_cap_label(p.capability.as_str().into())
                .to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        g.set_peer_id(peer.id.clone().into());
        g.set_peer_name(peer.name.clone().into());
        g.set_unavailable(names.join(", ").into());
        g.set_on_ble(true);
    }
}
