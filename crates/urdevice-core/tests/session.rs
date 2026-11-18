//! Two or three in-process nodes on loopback (design §16, task T1.12).

#![allow(clippy::unwrap_used)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use urdevice_core::discovery::DiscoveryConfig;
use urdevice_core::node::{EventSink, PeerStatus, PermissionView, Snapshot};
use urdevice_core::secret::MemorySecretStore;
use urdevice_core::{Capability, Mode, Node, NodeConfig, NodeEvent};

#[derive(Default)]
struct Events(Mutex<Vec<NodeEvent>>);

impl EventSink for Events {
    fn on_event(&self, e: NodeEvent) {
        if e != NodeEvent::Changed {
            self.0.lock().unwrap().push(e);
        }
    }
}

impl Events {
    fn find(&self, f: impl Fn(&NodeEvent) -> bool) -> Option<NodeEvent> {
        self.0.lock().unwrap().iter().find(|e| f(e)).cloned()
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

struct Dev {
    node: Option<Node>,
    events: Arc<Events>,
    secrets: Arc<MemorySecretStore>,
    dir: PathBuf,
    port: u16,
    name: String,
    ask_timeout: Duration,
    id: String,
}

fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

impl Dev {
    fn new(name: &str) -> Self {
        Self::with_timeout(name, Duration::from_secs(30))
    }

    fn with_timeout(name: &str, ask_timeout: Duration) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "urdevice-test-{name}-{}-{}",
            std::process::id(),
            urdevice_core::crypto::random_u64().unwrap()
        ));
        let mut d = Self {
            node: None,
            events: Arc::new(Events::default()),
            secrets: Arc::new(MemorySecretStore::new()),
            dir,
            port: free_port(),
            name: name.into(),
            ask_timeout,
            id: String::new(),
        };
        d.start();
        d.node().complete_onboarding(name, &[]).unwrap();
        d
    }

    fn start(&mut self) {
        let mut cfg = NodeConfig::new(
            self.dir.clone(),
            "linux",
            &self.name,
            self.secrets.clone(),
            self.events.clone(),
        );
        cfg.quic_port = self.port;
        cfg.bind_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        cfg.discovery = DiscoveryConfig {
            udp_port: 0,
            mdns: false,
        };
        cfg.invite_addrs = Some(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
        cfg.ask_timeout = self.ask_timeout;
        self.node = Some(Node::start(cfg).unwrap());
        self.id = self.snap().me.id;
    }

    fn stop(&mut self) {
        self.node = None;
    }

    fn node(&self) -> &Node {
        self.node.as_ref().unwrap()
    }

    fn snap(&self) -> Snapshot {
        self.node().snapshot()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port)
    }

    /// Make `other` show up as if it announced itself.
    fn see(&self, other: &Dev) {
        let id = other.id();
        self.node()
            .add_discovered(&id[..16], &other.name, "linux", &[other.addr()]);
    }

    fn status_of(&self, other: &Dev) -> Option<PeerStatus> {
        let id = other.id();
        self.snap()
            .peers
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.status)
    }

    fn connected_to(&self, other: &Dev) -> bool {
        self.status_of(other) == Some(PeerStatus::Connected)
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        self.node = None;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_until(what: &str, secs: u64, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn qr_pair(host: &Dev, scanner: &Dev) {
    host.node().open_pair_screen();
    let uri = host.snap().pair_screen.unwrap().qr_uri;
    scanner.node().scan_qr(&uri).unwrap();
    wait_until("QR pairing", 10, || {
        host.connected_to(scanner) && scanner.connected_to(host)
    });
    host.node().close_pair_screen();
}

fn remote_mode(viewer: &Dev, host: &Dev, cap: Capability) -> Option<Mode> {
    let id = host.id();
    viewer
        .snap()
        .peers
        .into_iter()
        .find(|p| p.id == id)?
        .remote_permissions
        .into_iter()
        .find(|p| p.capability == cap)
        .map(|p| p.mode)
}

#[test]
fn qr_pairing_then_permissions() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    qr_pair(&a, &b);

    // Defaults from onboarding reach the peer (R5.7).
    wait_until("PermissionState", 2, || {
        remote_mode(&b, &a, Capability::Input) == Some(Mode::Ask)
    });

    // Deny shows as off on the other device within 1 s.
    a.node()
        .set_permission(Capability::Input, Mode::Deny, false)
        .unwrap();
    wait_until("Deny pushed", 1, || {
        remote_mode(&b, &a, Capability::Input) == Some(Mode::Deny)
    });
    b.node()
        .request_activity(&a.id(), Capability::Input)
        .unwrap();
    wait_until("denied reply", 5, || {
        b.events
            .find(|e| matches!(e, NodeEvent::ActivityResult { granted: false, reason, .. } if reason == "denied"))
            .is_some()
    });

    // Always Ask: the host prompts, the user allows, an activity with Stop starts.
    a.node()
        .set_permission(Capability::Input, Mode::Ask, false)
        .unwrap();
    b.events.clear();
    b.node()
        .request_activity(&a.id(), Capability::Input)
        .unwrap();
    wait_until("prompt", 5, || {
        a.events
            .find(|e| matches!(e, NodeEvent::Prompt(_)))
            .is_some()
    });
    let Some(NodeEvent::Prompt(p)) = a.events.find(|e| matches!(e, NodeEvent::Prompt(_))) else {
        unreachable!()
    };
    assert_eq!(p.device_name, "Pixel");
    assert_eq!(p.capability, "input.v1");
    a.node().answer_prompt(p.id, true);
    wait_until("granted", 5, || {
        b.events
            .find(|e| matches!(e, NodeEvent::ActivityResult { granted: true, .. }))
            .is_some()
    });
    let act = a.snap().activities;
    assert_eq!(act.len(), 1);
    assert!(act[0].incoming);
    wait_until("controller activity", 2, || b.snap().activities.len() == 1);

    // Stop ends it on both sides (R5.5).
    a.node().stop_activity(act[0].id);
    wait_until("stopped", 2, || b.snap().activities.is_empty());
    assert!(a.snap().activities.is_empty());

    // Allow starts with no prompt; setting Deny revokes it.
    a.node()
        .set_permission(Capability::Input, Mode::Allow, false)
        .unwrap();
    b.node()
        .request_activity(&a.id(), Capability::Input)
        .unwrap();
    wait_until("allowed activity", 5, || a.snap().activities.len() == 1);
    a.node()
        .set_permission(Capability::Input, Mode::Deny, false)
        .unwrap();
    wait_until("revoked", 2, || {
        a.snap().activities.is_empty() && b.snap().activities.is_empty()
    });

    let log = a.node().activity_log(10).unwrap();
    assert!(log.iter().any(|e| e.decision == "denied"));
    assert!(log.iter().any(|e| e.decision == "allowed"));
}

#[test]
fn always_ask_times_out_as_deny() {
    let a = Dev::with_timeout("Laptop", Duration::from_secs(1));
    let b = Dev::new("Pixel");
    qr_pair(&a, &b);
    b.node()
        .request_activity(&a.id(), Capability::Screen)
        .unwrap();
    wait_until("timeout reply", 5, || {
        b.events
            .find(|e| matches!(e, NodeEvent::ActivityResult { granted: false, reason, .. } if reason == "timeout"))
            .is_some()
    });
    assert!(
        a.events
            .find(|e| matches!(e, NodeEvent::PromptClosed { .. }))
            .is_some()
    );
    assert!(a.snap().activities.is_empty());
}

#[test]
fn terminal_allow_needs_confirmation() {
    let a = Dev::new("Laptop");
    assert!(
        a.node()
            .set_permission(Capability::Terminal, Mode::Allow, false)
            .is_err()
    );
    a.node()
        .set_permission(Capability::Terminal, Mode::Allow, true)
        .unwrap();
    // Onboarding can't turn it on.
    a.node()
        .complete_onboarding(
            "Laptop",
            &[PermissionView {
                capability: Capability::FsWrite,
                mode: Mode::Allow,
            }],
        )
        .unwrap();
    let fs = a
        .snap()
        .permissions
        .into_iter()
        .find(|p| p.capability == Capability::FsWrite)
        .unwrap();
    assert_eq!(fs.mode, Mode::Deny);
}

#[test]
fn qr_token_is_single_use_and_wrong_key_is_refused() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    let c = Dev::new("Tablet");
    a.node().open_pair_screen();
    let uri = a.snap().pair_screen.unwrap().qr_uri;
    b.node().scan_qr(&uri).unwrap();
    wait_until("first scan", 10, || a.connected_to(&b));
    // The same QR code again: the token was used up (R3.2).
    c.node().scan_qr(&uri).unwrap();
    wait_until("replay refused", 10, || {
        c.events
            .find(|e| matches!(e, NodeEvent::PairingFinished { ok: false, .. }))
            .is_some()
    });
    assert_eq!(a.snap().peers.len(), 1);
    assert!(c.snap().peers.is_empty());

    // A QR code naming C's key but pointing at A: TLS refuses A's key (R3.3).
    a.node().open_pair_screen();
    let fresh = a.snap().pair_screen.unwrap().qr_uri;
    let forged = forge_device_id(&fresh, &c.id());
    b.events.clear();
    b.node().scan_qr(&forged).unwrap();
    wait_until("wrong fingerprint refused", 10, || {
        b.events
            .find(|e| matches!(e, NodeEvent::PairingFinished { ok: false, reason, .. } if reason == "unreachable"))
            .is_some()
    });
}

fn forge_device_id(uri: &str, id_hex: &str) -> String {
    use prost::Message;
    let b64 = uri.strip_prefix(urdevice_core::pairing::QR_PREFIX).unwrap();
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(b64.as_bytes())
        .unwrap();
    let mut inv = urdevice_proto::PairInvite::decode(bytes.as_slice()).unwrap();
    inv.device_id = data_encoding::HEXLOWER.decode(id_hex.as_bytes()).unwrap();
    urdevice_core::pairing::invite_uri(&inv)
}

#[test]
fn lockout_after_five_failures() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    a.node().open_pair_screen();
    for i in 0..5 {
        // Each scan of a stale code burns the fresh token and counts as a failure.
        let uri = a.snap().pair_screen.unwrap().qr_uri;
        let stale = tamper_token(&uri);
        b.events.clear();
        b.node().scan_qr(&stale).unwrap();
        wait_until(&format!("failure {i}"), 10, || {
            b.events
                .find(|e| matches!(e, NodeEvent::PairingFinished { ok: false, .. }))
                .is_some()
        });
    }
    wait_until("locked", 3, || {
        a.snap().pair_screen.unwrap().locked_secs > 0
    });
    assert!(
        a.events
            .find(|e| matches!(e, NodeEvent::Notice { code, .. } if code == "pairing_locked"))
            .is_some()
    );
    assert!(a.snap().pair_screen.unwrap().qr_uri.is_empty());
    assert!(a.snap().peers.is_empty());
}

fn tamper_token(uri: &str) -> String {
    use prost::Message;
    let b64 = uri.strip_prefix(urdevice_core::pairing::QR_PREFIX).unwrap();
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(b64.as_bytes())
        .unwrap();
    let mut inv = urdevice_proto::PairInvite::decode(bytes.as_slice()).unwrap();
    inv.token[0] ^= 0xff;
    urdevice_core::pairing::invite_uri(&inv)
}

#[test]
fn code_pairing_needs_both_confirmations() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    a.node().open_pair_screen();
    b.node().open_pair_screen();
    b.see(&a);
    wait_until("nearby", 3, || !b.snap().nearby.is_empty());
    let key = b.snap().nearby[0].key.clone();
    b.node().pair_nearby(&key).unwrap();
    wait_until("codes shown", 10, || {
        !a.snap().code_pairings.is_empty() && !b.snap().code_pairings.is_empty()
    });
    let ca = a.snap().code_pairings[0].clone();
    let cb = b.snap().code_pairings[0].clone();
    assert_eq!(ca.code, cb.code);
    assert_eq!(ca.code.len(), 6);
    assert!(ca.incoming && !cb.incoming);

    // One side alone is not enough.
    a.node().confirm_code(&ca.key, true).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(a.snap().peers.is_empty());
    assert!(a.snap().code_pairings[0].confirmed_here);

    b.node().confirm_code(&cb.key, true).unwrap();
    wait_until("paired and connected", 10, || {
        a.connected_to(&b) && b.connected_to(&a)
    });
}

#[test]
fn code_pairing_cancel_stores_nothing() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    a.node().open_pair_screen();
    b.node().open_pair_screen();
    b.see(&a);
    wait_until("nearby", 3, || !b.snap().nearby.is_empty());
    b.node().pair_nearby(&b.snap().nearby[0].key).unwrap();
    wait_until("codes shown", 10, || !a.snap().code_pairings.is_empty());
    let ca = a.snap().code_pairings[0].clone();
    a.node().confirm_code(&ca.key, false).unwrap();
    wait_until("cancel seen", 10, || {
        b.events
            .find(|e| matches!(e, NodeEvent::PairingFinished { ok: false, .. }))
            .is_some()
    });
    assert!(a.snap().peers.is_empty() && b.snap().peers.is_empty());
}

#[test]
fn pairing_refused_when_pair_screen_is_closed() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    a.node().open_pair_screen();
    let uri = a.snap().pair_screen.unwrap().qr_uri;
    a.node().close_pair_screen();
    b.node().scan_qr(&uri).unwrap();
    wait_until("refused", 10, || {
        b.events
            .find(|e| matches!(e, NodeEvent::PairingFinished { ok: false, .. }))
            .is_some()
    });
    assert!(a.snap().peers.is_empty());
}

#[test]
fn disconnect_and_reconnect() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    qr_pair(&a, &b);
    b.node().disconnect(&a.id()).unwrap();
    wait_until("both disconnected", 5, || {
        matches!(a.status_of(&b), Some(PeerStatus::Disconnected { .. }))
            && matches!(b.status_of(&a), Some(PeerStatus::Disconnected { .. }))
    });
    assert_eq!(
        a.status_of(&b),
        Some(PeerStatus::Disconnected { by: "Pixel".into() })
    );
    // No automatic reconnect while disconnected (R4.7).
    a.see(&b);
    b.see(&a);
    std::thread::sleep(Duration::from_secs(3));
    assert!(!a.connected_to(&b));
    // Reconnect on the other device than the one that disconnected.
    a.node().reconnect(&b.id()).unwrap();
    wait_until("reconnected", 10, || {
        a.connected_to(&b) && b.connected_to(&a)
    });
}

#[test]
fn restart_reconnects_on_its_own() {
    let mut a = Dev::new("Laptop");
    let mut b = Dev::new("Pixel");
    qr_pair(&a, &b);
    let rtt = a.snap().peers[0].rtt_ms;
    assert!(rtt.is_some());
    a.stop();
    b.stop();
    a.start();
    b.start();
    // Pairings survive the restart, and the stored addresses are enough.
    wait_until("reconnect after restart", 15, || {
        a.connected_to(&b) && b.connected_to(&a)
    });
}

#[test]
fn reconnects_after_the_peer_comes_back() {
    let a = Dev::new("Laptop");
    let mut b = Dev::new("Pixel");
    qr_pair(&a, &b);
    b.stop();
    wait_until("offline", 35, || {
        a.status_of(&b) == Some(PeerStatus::Offline)
    });
    b.start();
    wait_until("back", 15, || a.connected_to(&b));
}

#[test]
fn unpair_closes_and_blocks() {
    let a = Dev::new("Laptop");
    let mut b = Dev::new("Pixel");
    qr_pair(&a, &b);
    a.node().forget(&b.id()).unwrap();
    wait_until("both forgot", 5, || {
        a.snap().peers.is_empty() && b.snap().peers.is_empty()
    });
    assert!(
        b.events
            .find(|e| matches!(e, NodeEvent::Notice { code, .. } if code == "unpaired_by"))
            .is_some()
    );

    // B pairs again, goes offline, and A forgets it while it is away. B still
    // trusts A, but A refuses B from now on.
    qr_pair(&a, &b);
    b.stop();
    a.node().forget(&b.id()).unwrap();
    b.start();
    std::thread::sleep(Duration::from_secs(4));
    assert!(a.snap().peers.is_empty());
    assert!(!b.connected_to(&a));
    assert_eq!(b.status_of(&a), Some(PeerStatus::Offline));
}

#[test]
fn battery_reaches_the_peer() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    b.node().set_battery(Some((64, true)));
    qr_pair(&a, &b);
    wait_until("battery", 3, || {
        a.snap()
            .peers
            .first()
            .is_some_and(|p| p.battery_pct == Some(64) && p.charging)
    });
}

#[test]
fn reset_identity_forgets_everyone() {
    let a = Dev::new("Laptop");
    let b = Dev::new("Pixel");
    qr_pair(&a, &b);
    let old = a.id();
    a.node().reset_identity().unwrap();
    assert_ne!(a.snap().me.id, old);
    assert!(a.snap().peers.is_empty());
}
