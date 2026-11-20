//! Localhost sharing and Expose on LAN (#22, R13): bindings for the Android app.
//!
//! The core does all the work (listeners, QUIC streams, LAN exposure). The
//! Android viewer's loopback listener runs inside the app process, which the
//! foreground service keeps alive, so Chrome on the phone can open
//! `http://localhost:<port>` (design §11). Kotlin reads the state with
//! `tunnel_*` getters and redraws on the node's `Changed` event.

use std::sync::Arc;

use urdevice_core::services::tunnel::{LanMode, SharedPortView, Tunnel, TunnelStatus, TunnelView};

use crate::{FfiError, UrDeviceNode, device_id};

/// Expose on LAN choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiLanMode {
    /// Only paired devices, through their tunnels.
    Off,
    /// Open on the LAN for one hour.
    OneHour,
    /// Open on the LAN until turned off, also after a restart.
    Always,
}

impl From<LanMode> for FfiLanMode {
    fn from(m: LanMode) -> Self {
        match m {
            LanMode::Off => Self::Off,
            LanMode::OneHour => Self::OneHour,
            LanMode::Always => Self::Always,
        }
    }
}

impl From<FfiLanMode> for LanMode {
    fn from(m: FfiLanMode) -> Self {
        match m {
            FfiLanMode::Off => Self::Off,
            FfiLanMode::OneHour => Self::OneHour,
            FfiLanMode::Always => Self::Always,
        }
    }
}

/// A port this device shares.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiSharedPort {
    pub port: u16,
    pub label: String,
    pub lan: FfiLanMode,
    /// Unix seconds when "1 hour" ends; 0 otherwise.
    pub lan_until: u64,
    /// True while the LAN listener is bound.
    pub lan_listening: bool,
    /// Open LAN connections.
    pub lan_connections: u32,
}

impl From<SharedPortView> for FfiSharedPort {
    fn from(p: SharedPortView) -> Self {
        Self {
            port: p.port,
            label: p.label,
            lan: p.lan.into(),
            lan_until: p.lan_until,
            lan_listening: p.lan_listening,
            lan_connections: p.lan_connections,
        }
    }
}

/// A port a paired device shares with this one.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiRemotePort {
    pub port: u16,
    pub label: String,
}

/// One tunnel, outgoing (this device views) or incoming (this device hosts).
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiTunnel {
    pub id: u64,
    pub device_id: String,
    pub device_name: String,
    pub incoming: bool,
    pub remote_port: u16,
    /// The loopback port on this device (viewer, once open), or 0.
    pub local_port: u16,
    /// `opening`, `open`, or `failed`.
    pub status: String,
    /// For `failed`: `denied`, `declined`, `timeout`, `offline`,
    /// `unsupported`, `bind`, or `refused_port`.
    pub reason: String,
    pub connections: u32,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

impl From<TunnelView> for FfiTunnel {
    fn from(t: TunnelView) -> Self {
        let (status, reason) = match t.status {
            TunnelStatus::Opening => ("opening", String::new()),
            TunnelStatus::Open => ("open", String::new()),
            TunnelStatus::Failed(r) => ("failed", r),
        };
        Self {
            id: t.id,
            device_id: t.peer.hex(),
            device_name: t.peer_name,
            incoming: t.incoming,
            remote_port: t.remote_port,
            local_port: t.local_port.unwrap_or(0),
            status: status.into(),
            reason,
            connections: t.connections,
            bytes_up: t.bytes_up,
            bytes_down: t.bytes_down,
        }
    }
}

impl UrDeviceNode {
    fn tunnel(&self) -> Result<Arc<Tunnel>, FfiError> {
        self.node.service::<Tunnel>().ok_or(FfiError::NotFound)
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Share a loopback port (1-65535) with every paired device, or change
    /// its label.
    pub fn tunnel_share(&self, port: u16, label: String) -> Result<(), FfiError> {
        Ok(self.tunnel()?.share(port, &label)?)
    }

    /// Stop sharing a port; ends its tunnels and its LAN exposure.
    pub fn tunnel_unshare(&self, port: u16) -> Result<(), FfiError> {
        Ok(self.tunnel()?.unshare(port)?)
    }

    /// Expose on LAN. `Always` needs `confirmed` (the extra confirmation).
    pub fn tunnel_set_lan(
        &self,
        port: u16,
        mode: FfiLanMode,
        confirmed: bool,
    ) -> Result<(), FfiError> {
        Ok(self.tunnel()?.set_lan(port, mode.into(), confirmed)?)
    }

    /// The ports this device shares.
    pub fn tunnel_shared(&self) -> Vec<FfiSharedPort> {
        self.tunnel()
            .map(|t| t.shared().into_iter().map(Into::into).collect())
            .unwrap_or_default()
    }

    /// Ports open on the LAN now, for the persistent indicator.
    pub fn tunnel_lan_exposed(&self) -> Vec<u16> {
        self.tunnel().map(|t| t.lan_exposed()).unwrap_or_default()
    }

    /// The ports `device` shares with this device.
    pub fn tunnel_remote_ports(&self, device: String) -> Result<Vec<FfiRemotePort>, FfiError> {
        Ok(self
            .tunnel()?
            .remote_ports(device_id(&device)?)
            .into_iter()
            .map(|r| FfiRemotePort {
                port: r.port,
                label: r.label,
            })
            .collect())
    }

    /// Start using `device`'s port. Returns the tunnel id; `tunnel_list`
    /// shows the local port once it listens.
    pub fn tunnel_open(&self, device: String, port: u16) -> Result<u64, FfiError> {
        Ok(self.tunnel()?.open(device_id(&device)?, port)?)
    }

    /// Stop a tunnel on either side.
    pub fn tunnel_stop(&self, id: u64) {
        if let Ok(t) = self.tunnel() {
            t.stop(id);
        }
    }

    /// Every tunnel, outgoing first.
    pub fn tunnel_list(&self) -> Vec<FfiTunnel> {
        self.tunnel()
            .map(|t| t.tunnels().into_iter().map(Into::into).collect())
            .unwrap_or_default()
    }
}
