//! UniFFI bindings for the Android app.
//!
//! Built for Android with `cargo-ndk`; the Gradle module `android/core-bindings`
//! runs the build and generates the Kotlin bindings (see `docs/development.md`).
//! The types here mirror `urdevice_core::node` as plain records, so the Kotlin
//! side gets data classes and never touches core internals.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use urdevice_core::node::{EventSink, PeerStatus, PermissionView};
use urdevice_core::{Capability, CoreError, Mode, Node, NodeConfig, NodeEvent};

mod features;
mod log;

uniffi::setup_scaffolding!();

/// The version of the UrDevice core.
#[uniffi::export]
pub fn core_version() -> String {
    urdevice_core::core_version().to_owned()
}

/// Send core logs to logcat (Android) or stderr. Logs are redacted (R16.2).
#[uniffi::export]
pub fn init_logging(filter: String) {
    log::init(&filter);
}

/// A QR matrix for drawing this device's pairing code.
#[derive(uniffi::Record)]
pub struct FfiQr {
    /// Modules per side.
    pub size: u32,
    /// Row-major, true for dark.
    pub modules: Vec<bool>,
}

/// Encode text as a QR matrix.
#[uniffi::export]
pub fn qr_matrix(text: String) -> Result<FfiQr, FfiError> {
    let m = urdevice_core::qr::encode(&text)?;
    Ok(FfiQr {
        size: m.size,
        modules: m.modules,
    })
}

/// Errors that cross into Kotlin.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FfiError {
    /// The device, prompt, or activity does not exist.
    #[error("not found")]
    NotFound,
    /// Bad input, for example a QR code that is not a UrDevice code.
    #[error("{detail}")]
    Invalid {
        /// Why. (Not `message`: that clashes with Kotlin's `Throwable.message`.)
        detail: String,
    },
    /// Pairing failed; `reason` is `expired`, `version`, and so on.
    #[error("pairing failed: {reason}")]
    Pairing {
        /// Short reason.
        reason: String,
    },
    /// Anything else.
    #[error("{detail}")]
    Failed {
        /// Description.
        detail: String,
    },
}

impl From<CoreError> for FfiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound => Self::NotFound,
            CoreError::InvalidInput(detail) => Self::Invalid { detail },
            CoreError::Pairing(reason) => Self::Pairing { reason },
            other => Self::Failed {
                detail: other.to_string(),
            },
        }
    }
}

/// Permission mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiMode {
    /// Start at once.
    Allow,
    /// Ask on this device.
    Ask,
    /// Refuse.
    Deny,
}

impl From<Mode> for FfiMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Allow => Self::Allow,
            Mode::Ask => Self::Ask,
            Mode::Deny => Self::Deny,
        }
    }
}

impl From<FfiMode> for Mode {
    fn from(m: FfiMode) -> Self {
        match m {
            FfiMode::Allow => Self::Allow,
            FfiMode::Ask => Self::Ask,
            FfiMode::Deny => Self::Deny,
        }
    }
}

/// One permission setting.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiPermission {
    /// Capability string, for example `input.v1`.
    pub capability: String,
    /// The mode.
    pub mode: FfiMode,
    /// True for settings only this device may change, with an extra confirmation.
    pub local_only: bool,
}

fn perm(p: &PermissionView) -> FfiPermission {
    FfiPermission {
        capability: p.capability.as_str().into(),
        mode: p.mode.into(),
        local_only: p.capability.local_only(),
    }
}

/// This device.
#[derive(uniffi::Record)]
pub struct FfiLocal {
    pub name: String,
    pub id: String,
    pub fingerprint: String,
    pub platform: String,
    pub addresses: Vec<String>,
    pub port: u16,
    pub hidden: bool,
    pub onboarded: bool,
    pub battery_pct: Option<u8>,
    pub charging: bool,
}

/// A paired device.
#[derive(uniffi::Record)]
pub struct FfiPeer {
    pub id: String,
    pub fingerprint: String,
    pub name: String,
    pub platform: String,
    /// `connected`, `offline`, or `disconnected`.
    pub status: String,
    /// Who chose Disconnect, when `status` is `disconnected`.
    pub disconnected_by: String,
    pub last_seen: u64,
    pub address: String,
    pub path: String,
    pub rtt_ms: Option<u32>,
    pub battery_pct: Option<u8>,
    pub charging: bool,
    pub remote_permissions: Vec<FfiPermission>,
}

/// An unpaired device on the network.
#[derive(uniffi::Record)]
pub struct FfiNearby {
    pub key: String,
    pub name: String,
    pub platform: String,
    pub address: String,
}

/// A code-confirmation pairing in progress.
#[derive(uniffi::Record)]
pub struct FfiCodePairing {
    pub key: String,
    pub name: String,
    pub platform: String,
    pub code: String,
    pub confirmed_here: bool,
    pub incoming: bool,
}

/// The Pair screen.
#[derive(uniffi::Record)]
pub struct FfiPairScreen {
    pub qr_uri: String,
    pub expires_in_secs: u32,
    pub locked_secs: u32,
    pub blocked_by_hidden: bool,
}

/// A running activity.
#[derive(uniffi::Record)]
pub struct FfiActivity {
    pub id: u64,
    pub device_id: String,
    pub device_name: String,
    pub capability: String,
    pub incoming: bool,
    pub started_at: u64,
}

/// Everything the UI draws.
#[derive(uniffi::Record)]
pub struct FfiSnapshot {
    pub me: FfiLocal,
    pub permissions: Vec<FfiPermission>,
    pub peers: Vec<FfiPeer>,
    pub nearby: Vec<FfiNearby>,
    pub pair_screen: Option<FfiPairScreen>,
    pub code_pairings: Vec<FfiCodePairing>,
    pub activities: Vec<FfiActivity>,
}

/// An activity-log row.
#[derive(uniffi::Record)]
pub struct FfiLogEntry {
    pub device_name: String,
    pub capability: String,
    pub decision: String,
    pub ts: u64,
}

/// Values for the app's own NsdManager registration.
#[derive(uniffi::Record)]
pub struct FfiMdns {
    pub instance: String,
    pub port: u16,
    pub txt: HashMap<String, String>,
}

/// An event from the core.
#[derive(uniffi::Enum)]
pub enum FfiEvent {
    Changed,
    PairingFinished {
        name: String,
        ok: bool,
        reason: String,
    },
    Prompt {
        id: u64,
        device_name: String,
        device_id: String,
        capability: String,
        timeout_secs: u32,
    },
    PromptClosed {
        id: u64,
    },
    ActivityResult {
        device_id: String,
        device_name: String,
        capability: String,
        granted: bool,
        reason: String,
    },
    Notice {
        code: String,
        device_name: String,
    },
}

impl From<NodeEvent> for FfiEvent {
    fn from(e: NodeEvent) -> Self {
        match e {
            NodeEvent::Changed => Self::Changed,
            NodeEvent::PairingFinished { name, ok, reason } => {
                Self::PairingFinished { name, ok, reason }
            }
            NodeEvent::Prompt(p) => Self::Prompt {
                id: p.id,
                device_name: p.device_name,
                device_id: p.device_id,
                capability: p.capability,
                timeout_secs: p.timeout_secs,
            },
            NodeEvent::PromptClosed { id } => Self::PromptClosed { id },
            NodeEvent::ActivityResult {
                device_id,
                device_name,
                capability,
                granted,
                reason,
            } => Self::ActivityResult {
                device_id,
                device_name,
                capability,
                granted,
                reason,
            },
            NodeEvent::Notice { code, device_name } => Self::Notice { code, device_name },
        }
    }
}

/// Secure storage implemented in Kotlin with an Android Keystore–wrapped key (R1.2).
#[uniffi::export(with_foreign)]
pub trait SecretStore: Send + Sync {
    /// Read a secret, `None` when missing.
    fn get(&self, name: String) -> Option<Vec<u8>>;
    /// Store a secret. Returns false on failure.
    fn set(&self, name: String, value: Vec<u8>) -> bool;
    /// Delete a secret.
    fn delete(&self, name: String);
}

/// Receives core events. Called on core threads.
#[uniffi::export(with_foreign)]
pub trait NodeListener: Send + Sync {
    /// One event.
    fn on_event(&self, event: FfiEvent);
}

struct SecretAdapter(Arc<dyn SecretStore>);

impl urdevice_core::secret::SecretStore for SecretAdapter {
    fn get(&self, name: &str) -> urdevice_core::Result<Option<Vec<u8>>> {
        Ok(self.0.get(name.to_owned()))
    }
    fn set(&self, name: &str, value: &[u8]) -> urdevice_core::Result<()> {
        if self.0.set(name.to_owned(), value.to_vec()) {
            Ok(())
        } else {
            Err(CoreError::Secret("Keystore write failed".into()))
        }
    }
    fn delete(&self, name: &str) -> urdevice_core::Result<()> {
        self.0.delete(name.to_owned());
        Ok(())
    }
}

struct ListenerAdapter(Arc<dyn NodeListener>);

impl EventSink for ListenerAdapter {
    fn on_event(&self, event: NodeEvent) {
        self.0.on_event(event.into());
    }
}

fn cap(s: &str) -> Result<Capability, FfiError> {
    s.parse().map_err(|_| FfiError::Invalid {
        detail: format!("unknown capability {s}"),
    })
}

/// Parse a Device ID (hex) passed from Kotlin. For the feature modules.
#[allow(dead_code)]
pub(crate) fn device_id(s: &str) -> Result<urdevice_core::identity::DeviceId, FfiError> {
    urdevice_core::identity::DeviceId::from_hex(s).ok_or(FfiError::NotFound)
}

/// A running core.
#[derive(uniffi::Object)]
pub struct UrDeviceNode {
    /// Feature modules (`features/<name>.rs`) reach the core through this.
    node: Node,
}

#[uniffi::export]
impl UrDeviceNode {
    /// Start the core. `data_dir` is app-private storage.
    #[uniffi::constructor]
    pub fn start(
        data_dir: String,
        platform: String,
        default_name: String,
        secrets: Arc<dyn SecretStore>,
        listener: Arc<dyn NodeListener>,
    ) -> Result<Arc<Self>, FfiError> {
        let mut cfg = NodeConfig::new(
            PathBuf::from(data_dir),
            &platform,
            &default_name,
            Arc::new(SecretAdapter(secrets)),
            Arc::new(ListenerAdapter(listener)),
        );
        // Android browses and registers mDNS through NsdManager (design §5).
        cfg.discovery.mdns = false;
        let node = Node::start(cfg)?;
        Ok(Arc::new(Self { node }))
    }

    /// Everything the UI draws.
    pub fn snapshot(&self) -> FfiSnapshot {
        let s = self.node.snapshot();
        FfiSnapshot {
            me: FfiLocal {
                name: s.me.name,
                id: s.me.id,
                fingerprint: s.me.fingerprint,
                platform: s.me.platform,
                addresses: s.me.addresses,
                port: s.me.port,
                hidden: s.me.hidden,
                onboarded: s.me.onboarded,
                battery_pct: s.me.battery_pct,
                charging: s.me.charging,
            },
            permissions: s.permissions.iter().map(perm).collect(),
            peers: s
                .peers
                .into_iter()
                .map(|p| {
                    let (status, by) = match p.status {
                        PeerStatus::Connected => ("connected", String::new()),
                        PeerStatus::Offline => ("offline", String::new()),
                        PeerStatus::Disconnected { by } => ("disconnected", by),
                    };
                    FfiPeer {
                        id: p.id,
                        fingerprint: p.fingerprint,
                        name: p.name,
                        platform: p.platform,
                        status: status.into(),
                        disconnected_by: by,
                        last_seen: p.last_seen,
                        address: p.address,
                        path: p.path,
                        rtt_ms: p.rtt_ms,
                        battery_pct: p.battery_pct,
                        charging: p.charging,
                        remote_permissions: p.remote_permissions.iter().map(perm).collect(),
                    }
                })
                .collect(),
            nearby: s
                .nearby
                .into_iter()
                .map(|n| FfiNearby {
                    key: n.key,
                    name: n.name,
                    platform: n.platform,
                    address: n.address,
                })
                .collect(),
            pair_screen: s.pair_screen.map(|p| FfiPairScreen {
                qr_uri: p.qr_uri,
                expires_in_secs: p.expires_in_secs,
                locked_secs: p.locked_secs,
                blocked_by_hidden: p.blocked_by_hidden,
            }),
            code_pairings: s
                .code_pairings
                .into_iter()
                .map(|c| FfiCodePairing {
                    key: c.key,
                    name: c.name,
                    platform: c.platform,
                    code: c.code,
                    confirmed_here: c.confirmed_here,
                    incoming: c.incoming,
                })
                .collect(),
            activities: s
                .activities
                .into_iter()
                .map(|a| FfiActivity {
                    id: a.id,
                    device_id: a.device_id,
                    device_name: a.device_name,
                    capability: a.capability,
                    incoming: a.incoming,
                    started_at: a.started_at,
                })
                .collect(),
        }
    }

    /// The activity log, newest first.
    pub fn activity_log(&self, limit: u32) -> Result<Vec<FfiLogEntry>, FfiError> {
        Ok(self
            .node
            .activity_log(limit)?
            .into_iter()
            .map(|e| FfiLogEntry {
                device_name: e.device_name,
                capability: e.capability,
                decision: e.decision,
                ts: e.ts,
            })
            .collect())
    }

    /// Save onboarding.
    pub fn complete_onboarding(
        &self,
        name: String,
        settings: Vec<FfiPermission>,
    ) -> Result<(), FfiError> {
        let settings: Vec<PermissionView> = settings
            .iter()
            .filter_map(|p| {
                Some(PermissionView {
                    capability: p.capability.parse().ok()?,
                    mode: p.mode.into(),
                })
            })
            .collect();
        Ok(self.node.complete_onboarding(&name, &settings)?)
    }

    /// Change a setting. `confirmed` is required to Allow terminal and file changes.
    pub fn set_permission(
        &self,
        capability: String,
        mode: FfiMode,
        confirmed: bool,
    ) -> Result<(), FfiError> {
        Ok(self
            .node
            .set_permission(cap(&capability)?, mode.into(), confirmed)?)
    }

    /// Rename.
    pub fn set_name(&self, name: String) -> Result<(), FfiError> {
        Ok(self.node.set_name(&name)?)
    }

    /// Hidden mode.
    pub fn set_hidden(&self, hidden: bool) -> Result<(), FfiError> {
        Ok(self.node.set_hidden(hidden)?)
    }

    /// The Pair screen opened.
    pub fn open_pair_screen(&self) {
        self.node.open_pair_screen();
    }

    /// The Pair screen closed.
    pub fn close_pair_screen(&self) {
        self.node.close_pair_screen();
    }

    /// A QR code was scanned.
    pub fn scan_qr(&self, text: String) -> Result<(), FfiError> {
        Ok(self.node.scan_qr(&text)?)
    }

    /// Start code pairing with a nearby device.
    pub fn pair_nearby(&self, key: String) -> Result<(), FfiError> {
        Ok(self.node.pair_nearby(&key)?)
    }

    /// "Codes match" or "Cancel".
    pub fn confirm_code(&self, key: String, matches: bool) -> Result<(), FfiError> {
        Ok(self.node.confirm_code(&key, matches)?)
    }

    /// Disconnect without unpairing.
    pub fn disconnect(&self, device: String) -> Result<(), FfiError> {
        Ok(self.node.disconnect(&device)?)
    }

    /// Reconnect after Disconnect.
    pub fn reconnect(&self, device: String) -> Result<(), FfiError> {
        Ok(self.node.reconnect(&device)?)
    }

    /// Forget a device.
    pub fn forget(&self, device: String) -> Result<(), FfiError> {
        Ok(self.node.forget(&device)?)
    }

    /// Ask a device to start an activity.
    pub fn request_activity(&self, device: String, capability: String) -> Result<(), FfiError> {
        Ok(self.node.request_activity(&device, cap(&capability)?)?)
    }

    /// Answer an Always Ask prompt.
    pub fn answer_prompt(&self, id: u64, allow: bool) {
        self.node.answer_prompt(id, allow);
    }

    /// Stop an activity.
    pub fn stop_activity(&self, id: u64) {
        self.node.stop_activity(id);
    }

    /// Battery from `BatteryManager`.
    pub fn set_battery(&self, pct: Option<u8>, charging: bool) {
        self.node.set_battery(pct.map(|p| (p, charging)));
    }

    /// A device found by NsdManager.
    pub fn add_discovered(
        &self,
        short_id: String,
        name: String,
        platform: String,
        addrs: Vec<String>,
        port: u16,
    ) {
        let addrs: Vec<SocketAddr> = addrs
            .iter()
            .filter_map(|a| a.parse::<IpAddr>().ok())
            .map(|ip| SocketAddr::new(ip, port))
            .collect();
        self.node
            .add_discovered(&short_id, &name, &platform, &addrs);
    }

    /// Values for NsdManager registration.
    pub fn mdns_record(&self) -> FfiMdns {
        let (instance, txt) = self.node.mdns_record();
        FfiMdns {
            instance,
            port: self.node.snapshot().me.port,
            txt: txt.into_iter().collect(),
        }
    }

    /// New identity, all pairings removed.
    pub fn reset_identity(&self) -> Result<(), FfiError> {
        Ok(self.node.reset_identity()?)
    }

    /// Text export.
    pub fn export_data(&self) -> Result<String, FfiError> {
        Ok(self.node.export_data()?)
    }

    /// Delete everything.
    pub fn delete_all_data(&self) -> Result<(), FfiError> {
        Ok(self.node.delete_all_data()?)
    }
}

/// Settings shown on Android, in UI order.
#[uniffi::export]
pub fn capabilities_for_android() -> Vec<String> {
    Capability::for_platform("android")
        .into_iter()
        .map(|c| c.as_str().to_owned())
        .collect()
}

/// Onboarding defaults (R5.2).
#[uniffi::export]
pub fn default_mode(capability: String) -> FfiMode {
    capability
        .parse::<Capability>()
        .map(|c| c.default_mode())
        .unwrap_or(Mode::Deny)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_core_version() {
        assert_eq!(super::core_version(), urdevice_core::core_version());
    }

    #[test]
    fn defaults_cross_the_boundary() {
        assert_eq!(default_mode("input.v1".into()), FfiMode::Ask);
        assert_eq!(default_mode("bogus".into()), FfiMode::Deny);
        assert!(capabilities_for_android().contains(&"gallery.v1".to_owned()));
        assert!(qr_matrix("hello".into()).unwrap().size >= 21);
    }
}
