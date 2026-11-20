//! Bluetooth fallback (#23, design §8).
//!
//! Bluetooth is an automatic fallback, not a setting. The node handles both
//! paths itself:
//!
//! - **Bluetooth PAN** is an ordinary IP network, so the QUIC session simply
//!   moves to it (`node/paths.rs`): the node watches its interfaces and the
//!   session's traffic, dials the peer's other addresses when the path goes
//!   quiet, labels the path from the local interface, and moves back to the
//!   LAN when it returns (R6.1, R6.4, R6.5).
//! - **Bluetooth LE** is a control channel for when there is no IP path
//!   between a desktop and an Android phone (`node/ble.rs`, R6.2). The phone
//!   runs a GATT server, the desktop is the central. This module holds the
//!   parts both sides share: the radio interface ([`RadioLink`],
//!   [`BleCentral`], [`BlePeripheral`]), the packet framing ([`frame`]), the
//!   Noise IK channel ([`noise`]), and what may travel over it
//!   ([`allowed_service`]).
//!
//! The channel uses `ServiceId::BLUETOOTH` only for the empty path probes
//! the node sends as QUIC datagrams; the peer drops them, but their ACKs
//! show the path is alive.

use std::time::Duration;

use tokio::sync::mpsc;
use urdevice_proto::ServiceId;

use super::Registry;
use crate::Result;
use crate::permission::Capability;
use crate::platform::BoxFuture;

pub mod frame;
pub mod noise;

/// Release 2 capabilities this module ships. Release 1 capabilities are
/// always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// The UrDevice GATT service (design §8.2).
pub const GATT_SERVICE: &str = "6b1f0001-5d7a-4c1e-9b8a-2f6d1c3e8a10";
/// Central to peripheral: write without response.
pub const GATT_RX: &str = "6b1f0002-5d7a-4c1e-9b8a-2f6d1c3e8a10";
/// Peripheral to central: notify.
pub const GATT_TX: &str = "6b1f0003-5d7a-4c1e-9b8a-2f6d1c3e8a10";

/// Largest clipboard message over Bluetooth LE: 64 KB of text plus its
/// envelope (R6.2).
pub const MAX_CLIPBOARD: usize = 64 * 1024 + 512;
/// Largest datagram over Bluetooth LE, header included.
pub const MAX_DATAGRAM: usize = 256;

/// True when traffic of this service may use the Bluetooth LE channel:
/// input (pointer, keyboard, presenter), media control, and clipboard text
/// (R6.2). Everything else gets `fallback_path` (R6.4).
pub fn allowed_service(id: ServiceId) -> bool {
    matches!(
        id,
        ServiceId::CLIPBOARD | ServiceId::INPUT | ServiceId::MEDIA
    )
}

/// True when a feature tile for this capability works over Bluetooth LE.
/// The device view greys the others out with "Not available over
/// Bluetooth" (R6.4).
pub fn allowed_capability(cap: Capability) -> bool {
    matches!(
        cap,
        Capability::Clipboard | Capability::Input | Capability::Media
    )
}

/// True when a control message body of this service fits the channel.
pub fn allowed_message(id: ServiceId, body: &[u8]) -> bool {
    if !allowed_service(id) {
        return false;
    }
    if id != ServiceId::CLIPBOARD {
        return body.len() <= 16 * 1024;
    }
    if body.len() > MAX_CLIPBOARD {
        return false;
    }
    // Text only: an image never goes over Bluetooth LE.
    match <urdevice_proto::features::clipboard::Clip as prost::Message>::decode(body) {
        Ok(c) => c.mime != urdevice_proto::features::clipboard::mime::PNG,
        Err(_) => true,
    }
}

/// One Bluetooth LE connection as the radio sees it: raw GATT payloads in
/// each direction. Dropping `tx` asks the radio to disconnect; `rx` ends when
/// the radio lost the link.
#[derive(Debug)]
pub struct RadioLink {
    /// Packets to the other device, each at most `packet` bytes.
    pub tx: mpsc::Sender<Vec<u8>>,
    /// Packets from the other device.
    pub rx: mpsc::Receiver<Vec<u8>>,
    /// Usable bytes per packet: the ATT MTU minus 3.
    pub packet: usize,
}

impl RadioLink {
    /// Two ends of an in-memory link, for tests and for the FFI glue.
    pub fn pair(packet: usize) -> (Self, Self) {
        let (atx, brx) = mpsc::channel(64);
        let (btx, arx) = mpsc::channel(64);
        (
            Self {
                tx: atx,
                rx: arx,
                packet,
            },
            Self {
                tx: btx,
                rx: brx,
                packet,
            },
        )
    }
}

/// The GATT central on a desktop (`btleplug` in urdevice-platform).
pub trait BleCentral: Send + Sync {
    /// Scan for up to `scan` for a peripheral with [`GATT_SERVICE`], skipping
    /// the radio addresses in `skip`, then connect, subscribe to [`GATT_TX`],
    /// and return its address with the link.
    fn connect(
        &self,
        scan: Duration,
        skip: Vec<String>,
    ) -> BoxFuture<'static, Result<(String, RadioLink)>>;
}

/// The GATT server on a phone (Kotlin). The core tells it when to advertise:
/// only while a paired desktop has no session (battery, R15.4).
pub trait BlePeripheral: Send + Sync {
    /// Start or stop advertising [`GATT_SERVICE`].
    fn set_advertising(&self, on: bool);
}

/// Add this feature's services to a starting node. The fallback lives in the
/// node itself, so there is nothing to add.
pub(crate) fn register(_r: &mut Registry, _platform: &str) {}

#[cfg(test)]
mod tests {
    use urdevice_proto::features::clipboard::{Clip, mime};

    use super::*;

    #[test]
    fn only_low_bandwidth_features_are_allowed() {
        for id in [ServiceId::CLIPBOARD, ServiceId::INPUT, ServiceId::MEDIA] {
            assert!(allowed_service(id));
        }
        for id in [
            ServiceId::FILES,
            ServiceId::GALLERY,
            ServiceId::SCREEN,
            ServiceId::TUNNEL,
            ServiceId::NOTIFY,
            ServiceId::TEST_FIRST,
        ] {
            assert!(!allowed_service(id));
        }
        assert!(allowed_capability(Capability::Input));
        assert!(!allowed_capability(Capability::Files));
        assert!(!allowed_capability(Capability::Screen));
        assert!(!allowed_capability(Capability::Tunnel));
    }

    #[test]
    fn clipboard_text_up_to_64_kb() {
        let clip = |m: &str, n: usize| {
            prost::Message::encode_to_vec(&Clip {
                id: vec![1; 16],
                mime: m.into(),
                bytes: vec![b'a'; n],
                origin_device: vec![2; 32],
                alt_text: String::new(),
            })
        };
        assert!(allowed_message(
            ServiceId::CLIPBOARD,
            &clip(mime::TEXT, 64 * 1024)
        ));
        assert!(!allowed_message(
            ServiceId::CLIPBOARD,
            &clip(mime::TEXT, 65 * 1024)
        ));
        assert!(!allowed_message(
            ServiceId::CLIPBOARD,
            &clip(mime::PNG, 100)
        ));
        assert!(!allowed_message(ServiceId::FILES, &[]));
        assert!(allowed_message(ServiceId::INPUT, &[0; 64]));
    }
}
