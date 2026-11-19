//! Headless device for testing clipboard sharing between real machines.
//!
//! It runs a core node with the desktop clipboard adapter, pairs by code
//! confirmation with whichever device it sees (confirming its own code), and
//! prints what happens, never clipboard content.
//!
//!   cargo run -p urdevice-platform --example clipboard_probe -- <name> <data-dir> <port> [peer-name] [seconds]
//!
//! With `peer-name` it starts pairing with that nearby device; without it, it
//! waits for the other device to start. After pairing it stays up for
//! `seconds` (default 120) so you can copy on one device and paste on the
//! other. This is a development tool, not part of the app.

use std::sync::Arc;
use std::time::{Duration, Instant};

use urdevice_core::node::{EventSink, PeerStatus};
use urdevice_core::secret::MemorySecretStore;
use urdevice_core::services::clipboard::{ClipEvent, Clipboard, ClipboardEvents};
use urdevice_core::{Node, NodeConfig, NodeEvent};

struct Events;

impl EventSink for Events {
    fn on_event(&self, e: NodeEvent) {
        if e != NodeEvent::Changed {
            println!("event {e:?}");
        }
    }
}

struct Clips;

impl ClipboardEvents for Clips {
    fn on_event(&self, e: &ClipEvent) {
        // No content: kinds, types, and sizes only.
        println!("clip {e:?}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: clipboard_probe <name> <data-dir> <port> [peer-name] [seconds]");
        return;
    }
    let (name, dir, port) = (
        args[1].clone(),
        std::path::PathBuf::from(&args[2]),
        &args[3],
    );
    let target = args.get(4).filter(|s| !s.is_empty()).cloned();
    let secs: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(120);
    let platform = urdevice_platform::platform_id();
    let mut cfg = NodeConfig::new(
        dir,
        platform,
        &name,
        Arc::new(MemorySecretStore::new()),
        Arc::new(Events),
    );
    cfg.quic_port = port.parse().unwrap_or(0);
    let node = Node::start(cfg).expect("node starts");
    node.complete_onboarding(&name, &[]).expect("onboarding");
    let svc = node.service::<Clipboard>().expect("clipboard service");
    svc.events.set(Arc::new(Clips));
    match urdevice_platform::features::clipboard::attach(&svc) {
        Ok(a) => println!("clipboard adapter: {}", a.name()),
        Err(e) => println!("clipboard adapter failed: {e}"),
    }
    node.open_pair_screen();
    println!("me {name} {}", node.snapshot().me.id);

    let end = Instant::now() + Duration::from_secs(secs);
    let mut asked = false;
    let mut connected = false;
    while Instant::now() < end {
        let s = node.snapshot();
        if let (Some(t), false) = (&target, asked)
            && let Some(n) = s.nearby.iter().find(|n| &n.name == t)
        {
            println!("pairing with {} at {}", n.name, n.address);
            let _ = node.pair_nearby(&n.key);
            asked = true;
        }
        for c in &s.code_pairings {
            if !c.confirmed_here {
                println!("code {} with {}", c.code, c.name);
                let _ = node.confirm_code(&c.key, true);
            }
        }
        let now = s
            .peers
            .iter()
            .any(|p| p.status == PeerStatus::Connected && !p.remote_permissions.is_empty());
        if now && !connected {
            println!("CONNECTED");
            node.close_pair_screen();
        }
        connected = now;
        std::thread::sleep(Duration::from_millis(200));
    }
}
