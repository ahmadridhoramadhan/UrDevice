//! The connection a session runs on: QUIC (over the LAN or Bluetooth PAN)
//! or the Bluetooth LE channel (design §6, §8).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use tokio::sync::{mpsc, watch};

/// The path label in the snapshot (R4.8, R6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PathKind {
    /// Wi-Fi or wired LAN.
    Lan,
    /// QUIC over a Bluetooth PAN interface.
    BtPan,
    /// The Bluetooth LE control channel.
    Ble,
}

impl PathKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "wifi",
            Self::BtPan => "bt-pan",
            Self::Ble => "ble",
        }
    }
}

static NEXT_BLE_ID: AtomicUsize = AtomicUsize::new(1);

/// The node's handle on a running Bluetooth LE session. The session task
/// owns the radio and the Noise state; this side only queues and closes.
pub(super) struct BleConn {
    id: usize,
    /// Datagrams to send (input events). Full means dropped, as on QUIC.
    pub(super) datagrams: mpsc::Sender<Vec<u8>>,
    rtt_ms: AtomicU32,
    close: watch::Sender<Option<u32>>,
}

impl BleConn {
    pub(super) fn new(
        datagrams: mpsc::Sender<Vec<u8>>,
    ) -> (Arc<Self>, watch::Receiver<Option<u32>>) {
        let (close, closed) = watch::channel(None);
        let conn = Arc::new(Self {
            id: NEXT_BLE_ID.fetch_add(1, Ordering::Relaxed),
            datagrams,
            rtt_ms: AtomicU32::new(0),
            close,
        });
        (conn, closed)
    }

    pub(super) fn set_rtt(&self, ms: u32) {
        self.rtt_ms.store(ms, Ordering::Relaxed);
    }
}

/// A session's connection.
#[derive(Clone)]
pub(super) enum Link {
    Quic(quinn::Connection),
    Ble(Arc<BleConn>),
}

impl Link {
    pub(super) fn close(&self, code: u32, reason: &[u8]) {
        match self {
            Self::Quic(c) => c.close(code.into(), reason),
            Self::Ble(b) => {
                b.close.send_replace(Some(code));
            }
        }
    }

    /// True when both are the same connection.
    pub(super) fn same(&self, other: &Link) -> bool {
        match (self, other) {
            (Self::Quic(a), Self::Quic(b)) => a.stable_id() == b.stable_id(),
            (Self::Ble(a), Self::Ble(b)) => a.id == b.id,
            _ => false,
        }
    }

    pub(super) fn rtt_ms(&self) -> u32 {
        match self {
            Self::Quic(c) => u32::try_from(c.rtt().as_millis()).unwrap_or(u32::MAX),
            Self::Ble(b) => b.rtt_ms.load(Ordering::Relaxed),
        }
    }

    pub(super) fn remote_address(&self) -> Option<SocketAddr> {
        self.quic().map(quinn::Connection::remote_address)
    }

    pub(super) fn quic(&self) -> Option<&quinn::Connection> {
        match self {
            Self::Quic(c) => Some(c),
            Self::Ble(_) => None,
        }
    }

    pub(super) fn is_ble(&self) -> bool {
        matches!(self, Self::Ble(_))
    }
}
