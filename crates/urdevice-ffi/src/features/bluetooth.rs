//! Bluetooth fallback (#23): bindings for the Android app.
//!
//! The phone runs the GATT server in Kotlin (`host/BluetoothHost.kt`) and
//! moves raw packets: writes from the desktop go in through
//! [`UrDeviceNode::bluetooth_link_data`], packets for the desktop come out
//! through [`BluetoothRadioFfi::send`] as notifications. The core does the
//! framing, the Noise handshake, and the session; Kotlin never sees
//! plaintext. The core also decides when to advertise.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::mpsc;
use urdevice_core::Capability;
use urdevice_core::services::bluetooth::{BlePeripheral, RadioLink, allowed_capability, frame};

use crate::UrDeviceNode;

/// The phone's GATT server.
#[uniffi::export(with_foreign)]
pub trait BluetoothRadioFfi: Send + Sync {
    /// Start or stop advertising the UrDevice service.
    fn set_advertising(&self, on: bool);
    /// Send one packet to the central of `link` as a notification. Queue it
    /// and return at once; return false when the link is gone.
    fn send(&self, link: u64, packet: Vec<u8>) -> bool;
    /// Disconnect `link`.
    fn disconnect(&self, link: u64);
}

struct Peripheral(Arc<dyn BluetoothRadioFfi>);

impl BlePeripheral for Peripheral {
    fn set_advertising(&self, on: bool) {
        self.0.set_advertising(on);
    }
}

fn links() -> &'static Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>> {
    static LINKS: OnceLock<Mutex<HashMap<u64, mpsc::Sender<Vec<u8>>>>> = OnceLock::new();
    LINKS.get_or_init(Mutex::default)
}

fn radio_slot() -> &'static Mutex<Option<Arc<dyn BluetoothRadioFfi>>> {
    static RADIO: OnceLock<Mutex<Option<Arc<dyn BluetoothRadioFfi>>>> = OnceLock::new();
    RADIO.get_or_init(Mutex::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// True when a feature tile for `capability` works over Bluetooth LE; the
/// others are greyed out with "Not available over Bluetooth" (R6.4).
#[uniffi::export]
pub fn bluetooth_allows(capability: String) -> bool {
    capability
        .parse::<Capability>()
        .is_ok_and(allowed_capability)
}

#[uniffi::export]
impl UrDeviceNode {
    /// Register the GATT server. Call once after the core started.
    pub fn bluetooth_set_radio(&self, radio: Arc<dyn BluetoothRadioFfi>) {
        *lock(radio_slot()) = Some(radio.clone());
        self.node.ble_set_peripheral(Arc::new(Peripheral(radio)));
    }

    /// A central wrote its first packet on a new link. `mtu` is the
    /// negotiated ATT MTU.
    pub fn bluetooth_link_opened(&self, link: u64, mtu: u32) {
        let Some(radio) = lock(radio_slot()).clone() else {
            return;
        };
        let packet = usize::try_from(mtu).unwrap_or(0).saturating_sub(3);
        let (core_tx, mut to_radio) = mpsc::channel::<Vec<u8>>(64);
        let (from_radio, core_rx) = mpsc::channel::<Vec<u8>>(64);
        if let Some(old) = lock(links()).insert(link, from_radio) {
            drop(old);
        }
        // Core to radio. A plain thread: Kotlin's send returns at once.
        let spawned = std::thread::Builder::new()
            .name("urdevice-ble".into())
            .spawn(move || {
                while let Some(p) = to_radio.blocking_recv() {
                    if !radio.send(link, p) {
                        break;
                    }
                }
                lock(links()).remove(&link);
                radio.disconnect(link);
            });
        if spawned.is_err() {
            lock(links()).remove(&link);
            return;
        }
        self.node.ble_incoming(RadioLink {
            tx: core_tx,
            rx: core_rx,
            packet,
        });
    }

    /// A packet the central wrote. Oversized packets end the link.
    pub fn bluetooth_link_data(&self, link: u64, packet: Vec<u8>) {
        let tx = lock(links()).get(&link).cloned();
        let Some(tx) = tx else { return };
        if packet.len() > frame::MAX_PACKET || tx.blocking_send(packet).is_err() {
            lock(links()).remove(&link);
        }
    }

    /// The central disconnected.
    pub fn bluetooth_link_closed(&self, link: u64) {
        lock(links()).remove(&link);
    }
}
