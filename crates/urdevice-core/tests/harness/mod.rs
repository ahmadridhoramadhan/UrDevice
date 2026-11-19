//! In-process test harness for feature services (design §16).
//!
//! [`Pair::new`] starts two nodes on loopback, pairs them with the QR flow,
//! and waits until both are connected and have each other's permission
//! settings. A feature test then drives its service on one node and checks
//! the other, end to end through QUIC, with no devices. Use it from a test
//! file with `mod harness;`. `tests/services.rs` shows the pattern.

#![allow(dead_code, clippy::unwrap_used)]

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use urdevice_core::discovery::DiscoveryConfig;
use urdevice_core::identity::DeviceId;
use urdevice_core::node::{EventSink, PeerStatus, PromptView, ServiceFactory, Snapshot};
use urdevice_core::secret::MemorySecretStore;
use urdevice_core::services::{Registry, Service};
use urdevice_core::{Capability, Mode, Node, NodeConfig, NodeEvent};

/// Budget for waits. `wait_until` returns as soon as the condition holds.
pub const WAIT: u64 = 15;

/// Every event except `Changed`.
#[derive(Default)]
pub struct Events(Mutex<Vec<NodeEvent>>);

impl EventSink for Events {
    fn on_event(&self, e: NodeEvent) {
        if e != NodeEvent::Changed {
            self.0.lock().unwrap().push(e);
        }
    }
}

impl Events {
    /// The first event that matches.
    pub fn find(&self, f: impl Fn(&NodeEvent) -> bool) -> Option<NodeEvent> {
        self.0.lock().unwrap().iter().find(|e| f(e)).cloned()
    }

    /// Forget the events seen so far.
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// One in-process device.
pub struct Dev {
    pub node: Node,
    pub events: Arc<Events>,
    pub name: String,
    dir: PathBuf,
}

/// Build the factory that adds `service` to a node, keeping its type so
/// [`Node::service`] finds it.
pub fn service<S: Service>(service: Arc<S>) -> ServiceFactory {
    Box::new(move |r: &mut Registry| r.add(service))
}

impl Dev {
    /// A Linux device with only the built-in services.
    pub fn new(name: &str) -> Self {
        Self::with(name, "linux", Vec::new())
    }

    /// A device on `platform` (`linux`, `android`, ...) with extra services.
    pub fn with(name: &str, platform: &str, services: Vec<ServiceFactory>) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "urdevice-feature-test-{name}-{}-{}",
            std::process::id(),
            urdevice_core::crypto::random_u64().unwrap()
        ));
        let events = Arc::new(Events::default());
        let mut cfg = NodeConfig::new(
            dir.clone(),
            platform,
            name,
            Arc::new(MemorySecretStore::new()),
            events.clone(),
        );
        cfg.quic_port = 0;
        cfg.bind_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        cfg.discovery = DiscoveryConfig {
            udp_port: 0,
            mdns: false,
        };
        cfg.invite_addrs = Some(vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]);
        cfg.ask_timeout = Duration::from_secs(10);
        cfg.services = services;
        let node = Node::start(cfg).unwrap();
        node.complete_onboarding(name, &[]).unwrap();
        Self {
            node,
            events,
            name: name.into(),
            dir,
        }
    }

    pub fn snap(&self) -> Snapshot {
        self.node.snapshot()
    }

    /// This device's ID.
    pub fn id(&self) -> DeviceId {
        DeviceId::from_hex(&self.snap().me.id).unwrap()
    }

    /// Change a permission setting here, confirming local-only ones.
    pub fn set(&self, cap: Capability, mode: Mode) {
        self.node.set_permission(cap, mode, true).unwrap();
    }

    pub fn connected_to(&self, other: &Dev) -> bool {
        let id = other.id().hex();
        self.snap()
            .peers
            .iter()
            .any(|p| p.id == id && p.status == PeerStatus::Connected)
    }

    /// The other device's setting for `cap`, as this device sees it.
    pub fn remote_mode(&self, host: &Dev, cap: Capability) -> Option<Mode> {
        let id = host.id().hex();
        self.snap()
            .peers
            .into_iter()
            .find(|p| p.id == id)?
            .remote_permissions
            .into_iter()
            .find(|p| p.capability == cap)
            .map(|p| p.mode)
    }

    /// Wait for the next Always Ask prompt on this device and answer it.
    pub fn answer_prompt(&self, allow: bool) -> PromptView {
        wait_until("prompt", WAIT, || {
            self.events
                .find(|e| matches!(e, NodeEvent::Prompt(_)))
                .is_some()
        });
        let Some(NodeEvent::Prompt(p)) = self.events.find(|e| matches!(e, NodeEvent::Prompt(_)))
        else {
            unreachable!()
        };
        self.events.clear();
        self.node.answer_prompt(p.id, allow);
        p
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Two paired, connected devices: `a` (Linux) and `b`.
pub struct Pair {
    pub a: Dev,
    pub b: Dev,
}

impl Pair {
    /// Two Linux devices with only the built-in services.
    pub fn new() -> Self {
        Self::with(Dev::new("Laptop"), Dev::new("Phone"))
    }

    /// Pair two devices you built with [`Dev::with`].
    pub fn with(a: Dev, b: Dev) -> Self {
        a.node.open_pair_screen();
        let uri = a.snap().pair_screen.unwrap().qr_uri;
        b.node.scan_qr(&uri).unwrap();
        wait_until("pairing", WAIT, || a.connected_to(&b) && b.connected_to(&a));
        a.node.close_pair_screen();
        // Both have the other's PermissionState once a default shows up.
        wait_until("permission state", WAIT, || {
            a.remote_mode(&b, Capability::Input).is_some()
                && b.remote_mode(&a, Capability::Input).is_some()
        });
        Self { a, b }
    }
}

/// Poll `f` every 20 ms until it holds; panic after `secs`.
pub fn wait_until(what: &str, secs: u64, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

/// Check that `f` holds for the whole of `ms` milliseconds.
pub fn holds_for(what: &str, ms: u64, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        assert!(f(), "{what} broke");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Run an async call (for example `ServiceCtx::request` or `open_bi`) from
/// a test thread.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}
