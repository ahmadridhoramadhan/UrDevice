//! Bluetooth fallback end to end between two in-process nodes (#23, R6).
//!
//! The radio is an in-memory link with a small MTU, so the test runs the
//! real framing, Noise IK handshake, and session code of both sides.

#![allow(clippy::unwrap_used)]

mod harness;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use harness::{Dev, Pair, WAIT, block_on, holds_for, service, wait_until};
use urdevice_core::node::PeerStatus;
use urdevice_core::platform::BoxFuture;
use urdevice_core::services::bluetooth::{BleCentral, RadioLink};
use urdevice_core::services::clipboard::{ClipData, Clipboard, ClipboardHost};
use urdevice_core::services::{Direction, Grant, Service, ServiceCtx, ServiceId};
use urdevice_core::{Capability, CoreError};

/// A central whose "radio" hands the other end to the test.
#[derive(Default)]
struct Central {
    pending: Mutex<Option<RadioLink>>,
}

impl BleCentral for Central {
    fn connect(
        &self,
        _scan: Duration,
        _skip: Vec<String>,
    ) -> BoxFuture<'static, urdevice_core::Result<(String, RadioLink)>> {
        // 185 is a common negotiated ATT MTU: every message is split.
        let (mine, theirs) = RadioLink::pair(182);
        *self.pending.lock().unwrap() = Some(theirs);
        Box::pin(async move { Ok(("AA:BB:CC:DD:EE:FF".into(), mine)) })
    }
}

#[derive(Default)]
struct Fake(Mutex<Vec<ClipData>>);

impl ClipboardHost for Fake {
    fn write(&self, _grant: &Grant, data: &ClipData) -> urdevice_core::Result<()> {
        self.0.lock().unwrap().push(data.clone());
        Ok(())
    }
}

/// A service that is not allowed over Bluetooth LE.
#[derive(Default)]
struct Bulk {
    ctx: OnceLock<ServiceCtx>,
}

impl Service for Bulk {
    fn id(&self) -> ServiceId {
        ServiceId::TEST_FIRST
    }
    fn capability(&self) -> Capability {
        Capability::Files
    }
    fn start(&self, ctx: ServiceCtx) {
        let _ = self.ctx.set(ctx);
    }
}

fn path(d: &Dev, other: &Dev) -> Option<String> {
    let id = other.id().hex();
    d.snap()
        .peers
        .into_iter()
        .find(|p| p.id == id && p.status == PeerStatus::Connected)
        .map(|p| p.path)
}

/// Pair over QUIC, then drop QUIC and connect over the fake radio.
fn over_ble() -> (Pair, Arc<Central>) {
    let p = Pair::with(
        Dev::with("Laptop", "linux", vec![service(Arc::new(Bulk::default()))]),
        Dev::with("Phone", "android", vec![service(Arc::new(Bulk::default()))]),
    );
    assert_eq!(path(&p.a, &p.b).as_deref(), Some("wifi"));
    let central = Arc::new(Central::default());
    p.a.node.ble_set_central(central.clone());

    // No IP path any more: Disconnect also stops reconnecting over QUIC.
    p.a.node.disconnect(&p.b.id().hex()).unwrap();
    wait_until("disconnected", WAIT, || {
        !p.a.connected_to(&p.b) && !p.b.connected_to(&p.a)
    });

    p.a.node.ble_connect_now(&p.b.id().hex()).unwrap();
    wait_until("radio link", WAIT, || {
        central.pending.lock().unwrap().is_some()
    });
    let theirs = central.pending.lock().unwrap().take().unwrap();
    p.b.node.ble_incoming(theirs);
    wait_until("session over Bluetooth LE", WAIT, || {
        path(&p.a, &p.b).as_deref() == Some("ble") && path(&p.b, &p.a).as_deref() == Some("ble")
    });
    (p, central)
}

#[test]
fn a_session_runs_over_ble_with_only_the_allowed_features() {
    let (p, _central) = over_ble();

    // The control protocol works: both have the other's permissions.
    wait_until("permission state", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Clipboard).is_some()
    });

    // Clipboard text passes (R6.2).
    let host = Arc::new(Fake::default());
    p.b.node
        .service::<Clipboard>()
        .unwrap()
        .host
        .set(host.clone());
    let a_clip = p.a.node.service::<Clipboard>().unwrap();
    let _ = a_clip.send_manual(ClipData::text("over bluetooth"), false, None);
    wait_until("clip", WAIT, || host.0.lock().unwrap().len() == 1);
    assert_eq!(host.0.lock().unwrap()[0], ClipData::text("over bluetooth"));
    // Text of 64 KB still fits; an image never goes.
    let _ = a_clip.send_manual(ClipData::text("x".repeat(64 * 1024)), false, None);
    wait_until("64 KB clip", WAIT, || host.0.lock().unwrap().len() == 2);
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x10\0\0\0\x10".to_vec();
    png.resize(2000, 0);
    let _ = a_clip.send_manual(ClipData::png(png), false, None);
    holds_for("no image", 1000, || host.0.lock().unwrap().len() == 2);

    // Everything else is refused with FallbackPath (R6.4).
    let bulk = p.a.node.service::<Bulk>().unwrap();
    let ctx = bulk.ctx.get().unwrap();
    let peer = p.b.id();
    assert!(ctx.on_ble(peer));
    let r = ctx.send(peer, Direction::ToHost, 0, &urdevice_proto::Disconnect {});
    assert!(matches!(r, Err(CoreError::FallbackPath)), "{r:?}");
    let r = block_on(ctx.open_uni(peer, Direction::ToHost, 0, 1, vec![]));
    assert!(matches!(r, Err(CoreError::FallbackPath)), "{r:?}");
    let r = ctx.send_datagram(peer, Direction::ToHost, 0, &[1, 2, 3]);
    assert!(matches!(r, Err(CoreError::FallbackPath)), "{r:?}");
    assert_eq!(ctx.max_datagram_payload(peer), None);
}

#[test]
fn the_session_moves_back_to_the_lan() {
    let (p, _central) = over_ble();
    // The LAN comes back: the phone is announced again (R6.5).
    let b = p.b.snap();
    let short = &b.me.id[..16];
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), b.me.port);
    p.a.node.add_discovered(short, "Phone", "android", &[addr]);
    wait_until("back on QUIC", 15, || {
        path(&p.a, &p.b).as_deref() == Some("wifi") && path(&p.b, &p.a).as_deref() == Some("wifi")
    });
    holds_for("stays on QUIC", 500, || p.a.connected_to(&p.b));
}

#[test]
fn a_hostile_central_gets_nowhere() {
    let p = Pair::with(
        Dev::with("Laptop", "linux", vec![]),
        Dev::with("Phone", "android", vec![]),
    );
    // Garbage instead of a Noise handshake: the phone drops the link.
    let (mut mine, theirs) = RadioLink::pair(182);
    p.b.node.ble_incoming(theirs);
    block_on(async {
        mine.tx.send(vec![0, 0, 1, 9, 9, 9]).await.unwrap();
        let r = tokio::time::timeout(Duration::from_secs(12), mine.rx.recv()).await;
        assert!(
            matches!(r, Ok(None)),
            "the link should close without an answer"
        );
    });
    // A well-formed packet stream that never ends is cut at the limit.
    let (mut mine, theirs) = RadioLink::pair(512);
    p.b.node.ble_incoming(theirs);
    block_on(async {
        for seq in 0u16..200 {
            let mut pkt = seq.to_be_bytes().to_vec();
            pkt.push(0);
            pkt.extend_from_slice(&[7; 509]);
            if mine.tx.send(pkt).await.is_err() {
                break;
            }
        }
        let r = tokio::time::timeout(Duration::from_secs(12), mine.rx.recv()).await;
        assert!(matches!(r, Ok(None)));
    });
    // The QUIC session was never touched.
    assert!(p.a.connected_to(&p.b));
}
