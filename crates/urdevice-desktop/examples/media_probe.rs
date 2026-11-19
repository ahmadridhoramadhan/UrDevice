//! Headless media test node: pairs, subscribes, and controls media from stdin
//! commands, with this machine's real media host adapter. A dev tool for
//! device tests (#20); not part of the app.
//!
//!   URDEVICE_DATA_DIR=/tmp/probe URDEVICE_PORT=47950 URDEVICE_NO_KEYRING=1 \
//!     cargo run -p urdevice-desktop --example media_probe < commands
//!
//! Commands: snap | qr | scan <uri> | pair <key> | confirm <key> | on <dev> |
//! off <dev> | state <dev> | host | cmd <dev> <session> pp|next|prev|seek <ms> |
//! vol <dev> set <0..1>|step <n>|mute <0|1> | mode <allow|ask|deny> | sleep <s> | quit

use std::io::BufRead;
use std::sync::Arc;
use std::time::Duration;

use urdevice_core::identity::DeviceId;
use urdevice_core::node::EventSink;
use urdevice_core::services::media::{CmdKind, Media, MediaEvents, VolumeChange};
use urdevice_core::{Capability, Mode, Node, NodeConfig, NodeEvent};

struct Sink(Arc<Node>);

impl EventSink for Sink {
    fn on_event(&self, e: NodeEvent) {
        match e {
            NodeEvent::Changed => {}
            NodeEvent::Prompt(p) => {
                println!("EVENT prompt {} -> allowing", p.id);
                self.0.answer_prompt(p.id, true);
            }
            other => println!("EVENT {other:?}"),
        }
    }
}

struct Events;

impl MediaEvents for Events {
    fn updated(&self, peer: DeviceId) {
        println!("MEDIA updated {}", peer.hex());
    }

    fn refused(&self, peer: DeviceId, reason: &str) {
        println!("MEDIA refused {} {reason}", peer.hex());
    }
}

fn main() {
    let data_dir = urdevice_platform::data_dir();
    let secrets = Arc::new(urdevice_platform::DesktopSecretStore::new(&data_dir));
    // The sink needs the node, and the node needs the sink.
    let cell: Arc<std::sync::OnceLock<Arc<Node>>> = Arc::default();
    struct Late(Arc<std::sync::OnceLock<Arc<Node>>>);
    impl EventSink for Late {
        fn on_event(&self, e: NodeEvent) {
            if let Some(n) = self.0.get() {
                Sink(n.clone()).on_event(e);
            }
        }
    }
    let mut cfg = NodeConfig::new(
        data_dir,
        urdevice_platform::platform_id(),
        &urdevice_platform::device_name(),
        secrets,
        Arc::new(Late(cell.clone())),
    );
    if let Some(port) = std::env::var("URDEVICE_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
    {
        cfg.quic_port = port;
    }
    let node = Arc::new(Node::start(cfg).expect("node starts"));
    let _ = cell.set(node.clone());
    if !node.snapshot().me.onboarded {
        node.complete_onboarding(&urdevice_platform::device_name(), &[])
            .expect("onboarding");
    }
    let svc = node.service::<Media>().expect("media service");
    let host = urdevice_platform::features::media::host();
    svc.set_host(host.clone());
    svc.set_events(Arc::new(Events));
    println!("READY id={}", node.snapshot().me.id);

    let dev = |s: &str| DeviceId::from_hex(s).expect("device id");
    for line in command_lines() {
        let a: Vec<&str> = line.split_whitespace().collect();
        let r: Result<(), String> = (|| match a.as_slice() {
            [] => Ok(()),
            ["quit"] => std::process::exit(0),
            ["sleep", s] => {
                std::thread::sleep(Duration::from_secs_f32(s.parse().map_err(|_| "num")?));
                Ok(())
            }
            ["snap"] => {
                let s = node.snapshot();
                for p in &s.peers {
                    println!("PEER {} {} {} {:?}", p.id, p.name, p.platform, p.status);
                }
                for n in &s.nearby {
                    println!("NEARBY {} {} {} {}", n.key, n.name, n.platform, n.address);
                }
                for c in &s.code_pairings {
                    println!(
                        "CODE {} {} {} confirmed_here={}",
                        c.key, c.name, c.code, c.confirmed_here
                    );
                }
                Ok(())
            }
            ["qr"] => {
                node.open_pair_screen();
                std::thread::sleep(Duration::from_millis(300));
                let q = node
                    .snapshot()
                    .pair_screen
                    .map(|p| p.qr_uri)
                    .unwrap_or_default();
                println!("QR {q}");
                Ok(())
            }
            ["scan", uri] => node.scan_qr(uri).map_err(|e| e.to_string()),
            ["pair", k] => node.pair_nearby(k).map_err(|e| e.to_string()),
            ["confirm", k] => node.confirm_code(k, true).map_err(|e| e.to_string()),
            ["mode", m] => {
                let mode = match *m {
                    "allow" => Mode::Allow,
                    "ask" => Mode::Ask,
                    _ => Mode::Deny,
                };
                node.set_permission(Capability::Media, mode, true)
                    .map_err(|e| e.to_string())
            }
            ["on", d] => svc.subscribe(dev(d), true).map_err(|e| e.to_string()),
            ["off", d] => svc.subscribe(dev(d), false).map_err(|e| e.to_string()),
            ["host"] => {
                print_state("HOST", &host.state());
                Ok(())
            }
            ["state", d] => {
                match svc.state_of(dev(d)) {
                    Some(st) => print_state("STATE", &st),
                    None => println!("STATE none"),
                }
                Ok(())
            }
            ["cmd", d, sid, what @ ..] => {
                let (kind, ms) = match what {
                    ["pp"] => (CmdKind::PlayPause, 0),
                    ["next"] => (CmdKind::Next, 0),
                    ["prev"] => (CmdKind::Prev, 0),
                    ["seek", ms] => (CmdKind::Seek, ms.parse().map_err(|_| "ms")?),
                    _ => return Err("cmd".into()),
                };
                svc.command(dev(d), sid, kind, ms)
                    .map_err(|e| e.to_string())
            }
            ["vol", d, what @ ..] => {
                let c = match what {
                    ["set", v] => VolumeChange::Set(v.parse().map_err(|_| "num")?),
                    ["step", n] => VolumeChange::Step(n.parse().map_err(|_| "num")?),
                    ["mute", m] => VolumeChange::Mute(*m == "1"),
                    _ => return Err("vol".into()),
                };
                svc.volume(dev(d), c).map_err(|e| e.to_string())
            }
            _ => Err("unknown command".into()),
        })();
        match r {
            Ok(()) => println!("OK {line}"),
            Err(e) => println!("ERR {line}: {e}"),
        }
    }
}

/// Commands from the file named by `URDEVICE_PROBE_CMDS` (read as it grows,
/// for a probe that starts without a terminal), else from stdin.
fn command_lines() -> Box<dyn Iterator<Item = String>> {
    let Some(path) = std::env::var_os("URDEVICE_PROBE_CMDS") else {
        return Box::new(std::io::stdin().lock().lines().map_while(Result::ok));
    };
    let mut done = 0usize;
    let mut pending: std::collections::VecDeque<String> = Default::default();
    Box::new(std::iter::from_fn(move || {
        loop {
            if let Some(l) = pending.pop_front() {
                return Some(l);
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                let lines: Vec<&str> = text.lines().collect();
                // Only complete lines: the writer may still be appending.
                let complete = if text.ends_with('\n') {
                    lines.len()
                } else {
                    lines.len().saturating_sub(1)
                };
                for l in lines.iter().take(complete).skip(done) {
                    pending.push_back((*l).to_owned());
                }
                done = done.max(complete);
            }
            if pending.is_empty() {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }))
}

fn print_state(tag: &str, st: &urdevice_core::services::media::MediaState) {
    println!(
        "{tag} volume={:.2} muted={} can_volume={} unavailable='{}' sessions={}",
        st.volume,
        st.muted,
        st.can_volume,
        st.unavailable,
        st.sessions.len()
    );
    for s in &st.sessions {
        println!(
            "{tag}   id={} app={} title='{}' artist='{}' state={:?} pos={:?} dur={:?} seek={} next={} prev={} pp={} art={}B",
            s.id,
            s.app_name,
            s.title,
            s.artist,
            s.play_state(),
            s.position_ms,
            s.duration_ms,
            s.can_seek,
            s.can_next,
            s.can_prev,
            s.can_play_pause,
            s.artwork_png.len()
        );
    }
}
