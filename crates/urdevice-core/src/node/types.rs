//! Plain data the node hands to the UI (desktop directly, Android through UniFFI).

use crate::permission::{Capability, Mode};

/// Receives events from the node. Called from core threads; implementations
/// must hand the work to their UI thread and return quickly.
pub trait EventSink: Send + Sync {
    /// Handle one event.
    fn on_event(&self, event: NodeEvent);
}

/// Something the UI should react to. For anything else, re-read
/// [`crate::Node::snapshot`] after [`NodeEvent::Changed`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeEvent {
    /// The snapshot changed.
    Changed,
    /// A pairing attempt ended (R3.6).
    PairingFinished {
        /// Name of the other device, when known.
        name: String,
        /// True when the devices are now paired.
        ok: bool,
        /// Short reason when it failed: `cancelled`, `timeout`, `bad_proof`,
        /// `expired`, `locked`, `unreachable`, `rejected`, `version`.
        reason: String,
    },
    /// Ask the user (Always Ask, R5.3). Answer with [`crate::Node::answer_prompt`].
    Prompt(PromptView),
    /// A prompt was answered or timed out; hide it.
    PromptClosed {
        /// The prompt id.
        id: u64,
    },
    /// The host answered a request this device made.
    ActivityResult {
        /// Device ID (hex) of the host.
        device_id: String,
        /// Host name.
        device_name: String,
        /// Capability string.
        capability: String,
        /// True when the host allowed it.
        granted: bool,
        /// Reason when refused: `denied`, `timeout`, `declined`, `unsupported`, `offline`.
        reason: String,
    },
    /// A one-line notice for the user.
    Notice {
        /// `unpaired_by`, `disconnected_by`, `update_required`, `pairing_locked`, or `stopped_by`.
        code: String,
        /// The device it concerns.
        device_name: String,
    },
}

/// An Always Ask prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptView {
    /// Prompt id.
    pub id: u64,
    /// Name of the device asking.
    pub device_name: String,
    /// Device ID (hex) of the device asking.
    pub device_id: String,
    /// Capability string.
    pub capability: String,
    /// Seconds until it is denied on its own.
    pub timeout_secs: u32,
}

/// Connection state of a paired device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerStatus {
    /// A session is open.
    Connected,
    /// Not connected, will connect when seen.
    Offline,
    /// The user chose Disconnect on one side (R4.7).
    Disconnected {
        /// Name of the device whose user disconnected.
        by: String,
    },
}

/// One entry of a permission list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionView {
    /// The capability.
    pub capability: Capability,
    /// Its mode.
    pub mode: Mode,
}

/// A paired device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerView {
    /// Device ID, hex.
    pub id: String,
    /// Readable fingerprint.
    pub fingerprint: String,
    /// Display name.
    pub name: String,
    /// Platform.
    pub platform: String,
    /// Status.
    pub status: PeerStatus,
    /// Unix seconds, 0 if never.
    pub last_seen: u64,
    /// IP address in use or last known.
    pub address: String,
    /// `wifi` today; `bt-pan` and `ble` come with the Bluetooth fallback (R4.8).
    pub path: String,
    /// Round-trip time in milliseconds while connected.
    pub rtt_ms: Option<u32>,
    /// Battery level of the peer, when it has one (R4.9).
    pub battery_pct: Option<u8>,
    /// True while the peer charges.
    pub charging: bool,
    /// The peer's permission settings, for drawing tiles only (R5.7).
    pub remote_permissions: Vec<PermissionView>,
}

/// A device on the network that is not paired (R2.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearbyView {
    /// Short ID, hex. Pass it to [`crate::Node::pair_nearby`].
    pub key: String,
    /// Display name.
    pub name: String,
    /// Platform.
    pub platform: String,
    /// IP address.
    pub address: String,
}

/// A code-confirmation pairing in progress (R3.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodePairingView {
    /// Device ID, hex. Pass it to [`crate::Node::confirm_code`].
    pub key: String,
    /// Other device's name.
    pub name: String,
    /// Other device's platform.
    pub platform: String,
    /// The 6-digit code.
    pub code: String,
    /// True when this side tapped "Codes match".
    pub confirmed_here: bool,
    /// True when the other device started it.
    pub incoming: bool,
}

/// The Pair screen state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairScreenView {
    /// QR text, empty while locked.
    pub qr_uri: String,
    /// Seconds the current QR code stays valid (R3.11).
    pub expires_in_secs: u32,
    /// Seconds pairing stays locked (R3.7), 0 when open.
    pub locked_secs: u32,
    /// True when hidden mode blocks pairing (R2.7).
    pub blocked_by_hidden: bool,
}

/// A running activity: input or screen sessions on this device, or ones this
/// device started on another device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityView {
    /// Activity id. Pass it to [`crate::Node::stop_activity`].
    pub id: u64,
    /// Other device's ID, hex.
    pub device_id: String,
    /// Other device's name.
    pub device_name: String,
    /// Capability string.
    pub capability: String,
    /// True when the other device acts on this one (show the indicator, R5.4).
    pub incoming: bool,
    /// Unix seconds.
    pub started_at: u64,
}

/// This device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalView {
    /// Display name.
    pub name: String,
    /// Device ID, hex.
    pub id: String,
    /// Readable fingerprint (R1.4).
    pub fingerprint: String,
    /// Platform.
    pub platform: String,
    /// IPv4 addresses.
    pub addresses: Vec<String>,
    /// QUIC port.
    pub port: u16,
    /// Hidden mode (R2.7).
    pub hidden: bool,
    /// True after onboarding.
    pub onboarded: bool,
    /// Battery level, if known.
    pub battery_pct: Option<u8>,
    /// True while charging.
    pub charging: bool,
}

/// Everything the UI draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// This device.
    pub me: LocalView,
    /// This device's permission settings.
    pub permissions: Vec<PermissionView>,
    /// Paired devices.
    pub peers: Vec<PeerView>,
    /// Unpaired devices on the network.
    pub nearby: Vec<NearbyView>,
    /// Pair screen, while open.
    pub pair_screen: Option<PairScreenView>,
    /// Code pairings in progress.
    pub code_pairings: Vec<CodePairingView>,
    /// Running activities.
    pub activities: Vec<ActivityView>,
}

/// An activity-log row for the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityLogView {
    /// Device name.
    pub device_name: String,
    /// Capability string.
    pub capability: String,
    /// `allowed`, `denied`, `timeout`, `declined`, or `stopped`.
    pub decision: String,
    /// Unix seconds.
    pub ts: u64,
}
