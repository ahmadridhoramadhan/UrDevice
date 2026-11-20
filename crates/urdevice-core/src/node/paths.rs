//! Path selection for QUIC sessions: Bluetooth PAN and back (design §8.1).
//!
//! Once a second the node looks at its interfaces and at each session:
//!
//! - **Label (R4.8, R6.4).** The path is Bluetooth PAN when the session's
//!   local address sits on a Bluetooth interface, otherwise Wi-Fi/LAN.
//! - **Loss (R6.1).** A session that has received nothing for a second gets
//!   an empty probe datagram, which the peer drops but acknowledges. After
//!   [`QUIET_DEAD`] without a packet, or as soon as the session's local
//!   address disappears, the node dials the peer's other known addresses
//!   (its PAN address, for example). A new connection replaces the old
//!   session in place, so running activities carry on (`net::open_session`).
//!   Before this, a dead path was only noticed after the 30 s idle timeout.
//! - **Return (R6.5).** A session on PAN, or on the Bluetooth LE channel,
//!   dials the peer's LAN addresses once they show up again in discovery,
//!   at most every [`DIAL_EVERY`], so it moves back well within 10 s.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use urdevice_proto::{DatagramHeader, Direction, ServiceId};

use super::link::{Link, PathKind};
use super::{Inner, lock, net};
use crate::identity::DeviceId;

const TICK: Duration = Duration::from_secs(1);
/// Quiet this long: send a probe.
const PROBE_AFTER: Duration = Duration::from_secs(1);
/// Quiet this long (three probes unanswered): look for another path, and
/// let the Bluetooth LE channel take over (`ble.rs`).
pub(super) const QUIET_DEAD: Duration = Duration::from_millis(3_500);
/// Least time between two path dials to one peer.
const DIAL_EVERY: Duration = Duration::from_secs(4);
/// An address announced within this time is live.
pub(super) const FRESH: Duration = Duration::from_secs(8);

/// One local IPv4 interface.
#[derive(Clone, Debug)]
pub(super) struct Iface {
    pub ip: IpAddr,
    pub mask: Option<IpAddr>,
    pub bluetooth: bool,
}

pub(super) fn interfaces() -> Vec<Iface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            let mask = match &i.addr {
                if_addrs::IfAddr::V4(v4) => Some(IpAddr::V4(v4.netmask)),
                if_addrs::IfAddr::V6(_) => None,
            };
            let bluetooth = !i.is_loopback() && is_bluetooth_interface(&i.name);
            Iface {
                ip: i.ip(),
                mask,
                bluetooth,
            }
        })
        .collect()
}

/// True when an interface name says Bluetooth PAN: `bnep*` (BlueZ),
/// `bt-pan` (Android tethering), or a Windows adapter named "Bluetooth
/// Network Connection".
pub fn bluetooth_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("bnep") || n.starts_with("bt-pan") || n.contains("bluetooth")
}

/// True for a Bluetooth PAN interface. NetworkManager names the PAN
/// interface after the adapter's MAC (`enx…`), so on Linux the kernel's
/// device type decides, not the name alone (spike T0.5).
pub fn is_bluetooth_interface(name: &str) -> bool {
    if bluetooth_name(name) {
        return true;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        if name.contains('/') || name.contains("..") {
            return false;
        }
        if let Ok(uevent) = std::fs::read_to_string(format!("/sys/class/net/{name}/uevent")) {
            return uevent.lines().any(|l| l.trim() == "DEVTYPE=bluetooth");
        }
    }
    false
}

/// The path of a connection from its local address.
pub(super) fn classify(conn: &quinn::Connection) -> PathKind {
    match conn.local_ip() {
        Some(ip) => classify_ip(ip, &interfaces()),
        None => PathKind::Lan,
    }
}

pub(super) fn classify_ip(ip: IpAddr, ifs: &[Iface]) -> PathKind {
    if ifs.iter().any(|i| i.ip == ip && i.bluetooth) {
        PathKind::BtPan
    } else {
        PathKind::Lan
    }
}

/// Which local path reaches `peer`: the interface whose IPv4 subnet holds
/// it. `None` when no local subnet does.
pub(super) fn route(peer: IpAddr, ifs: &[Iface]) -> Option<PathKind> {
    let IpAddr::V4(p) = peer else {
        return None;
    };
    ifs.iter()
        .find(|i| match (i.ip, i.mask) {
            (IpAddr::V4(ip), Some(IpAddr::V4(m))) => {
                u32::from(m) != 0 && (u32::from(ip) & u32::from(m)) == (u32::from(p) & u32::from(m))
            }
            _ => false,
        })
        .map(|i| {
            if i.bluetooth {
                PathKind::BtPan
            } else {
                PathKind::Lan
            }
        })
}

/// What to do about one session this tick.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Nothing,
    /// Dial these addresses; a connection that works replaces the session.
    Dial(Vec<SocketAddr>),
}

/// The inputs of [`decide`], gathered under the state lock.
#[derive(Debug)]
pub(super) struct View {
    pub path: PathKind,
    pub age: Duration,
    pub quiet: Duration,
    pub local_gone: bool,
    pub remote: Option<SocketAddr>,
    /// Addresses from discovery announcements within [`FRESH`].
    pub fresh: Vec<SocketAddr>,
    /// Addresses the device was last reached at.
    pub last: Vec<SocketAddr>,
}

pub(super) fn decide(v: &View, ifs: &[Iface]) -> Action {
    let lan = |a: &SocketAddr| route(a.ip(), ifs) == Some(PathKind::Lan);
    match v.path {
        PathKind::Ble => {
            // Any IP path beats Bluetooth LE (R6.5).
            if v.age < net::REPLACE_AFTER || v.fresh.is_empty() {
                return Action::Nothing;
            }
            Action::Dial(v.fresh.clone())
        }
        _ if v.quiet >= QUIET_DEAD || v.local_gone => {
            // The path stopped answering: try every other address (R6.1).
            let mut addrs: Vec<SocketAddr> = Vec::new();
            for a in v.fresh.iter().chain(v.last.iter()) {
                if Some(*a) != v.remote && !addrs.contains(a) {
                    addrs.push(*a);
                }
            }
            if addrs.is_empty() || v.age < net::REPLACE_AFTER {
                Action::Nothing
            } else {
                Action::Dial(addrs)
            }
        }
        PathKind::BtPan if v.age >= net::REPLACE_AFTER => {
            // Back to the LAN as soon as it is announced again (R6.5).
            let addrs: Vec<SocketAddr> = v.fresh.iter().copied().filter(lan).collect();
            if addrs.is_empty() {
                Action::Nothing
            } else {
                Action::Dial(addrs)
            }
        }
        _ => Action::Nothing,
    }
}

#[derive(Default)]
struct Track {
    link: Option<usize>,
    rx: u64,
    rx_at: Option<Instant>,
    probe_at: Option<Instant>,
    next_dial: Option<Instant>,
}

fn probe() -> bytes::Bytes {
    bytes::Bytes::from(urdevice_proto::encode_datagram(
        DatagramHeader {
            service: ServiceId::BLUETOOTH,
            direction: Direction::ToController,
            activity: 0,
        },
        &[],
    ))
}

pub(super) async fn paths_loop(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(TICK);
    let mut tracks: HashMap<DeviceId, Track> = HashMap::new();
    let dialing: Arc<Mutex<HashSet<DeviceId>>> = Arc::default();
    loop {
        tick.tick().await;
        let now = Instant::now();
        let ifs = interfaces();
        let local: HashSet<IpAddr> = ifs.iter().map(|i| i.ip).collect();
        let mut jobs = Vec::new();
        let mut changed = false;
        {
            let mut st = lock(&inner.state);
            tracks.retain(|p, _| st.sessions.contains_key(p));
            let peers: Vec<DeviceId> = st.sessions.keys().copied().collect();
            for peer in peers {
                let fresh = st
                    .nearby
                    .get(&peer.short())
                    .filter(|n| now.duration_since(n.last_seen) < FRESH)
                    .map(|n| n.addrs.clone())
                    .unwrap_or_default();
                let last = st
                    .paired(peer)
                    .map(|d| d.last_addrs.clone())
                    .unwrap_or_default();
                let Some(s) = st.sessions.get_mut(&peer) else {
                    continue;
                };
                let t = tracks.entry(peer).or_default();
                let id = match &s.link {
                    Link::Quic(c) => c.stable_id(),
                    Link::Ble(_) => usize::MAX,
                };
                if t.link != Some(id) {
                    *t = Track {
                        link: Some(id),
                        next_dial: t.next_dial,
                        ..Track::default()
                    };
                }
                let mut view = View {
                    path: s.path,
                    age: s.opened.elapsed(),
                    quiet: Duration::ZERO,
                    local_gone: false,
                    remote: s.link.remote_address(),
                    fresh,
                    last,
                };
                if let Link::Quic(c) = &s.link {
                    if let Some(ip) = c.local_ip() {
                        // An empty list means the lookup failed, not that every
                        // interface went away.
                        view.local_gone = !local.is_empty() && !local.contains(&ip);
                        let path = classify_ip(ip, &ifs);
                        if !view.local_gone && path != s.path {
                            s.path = path;
                            view.path = path;
                            changed = true;
                            tracing::info!(peer = %peer.short_hex(), path = path.as_str(), "session path changed");
                        }
                    }
                    let rx = c.stats().udp_rx.datagrams;
                    if t.rx_at.is_none() || rx != t.rx {
                        t.rx = rx;
                        t.rx_at = Some(now);
                    }
                    view.quiet = t.rx_at.map_or(Duration::ZERO, |at| now.duration_since(at));
                    if view.quiet >= PROBE_AFTER
                        && t.probe_at
                            .is_none_or(|p| now.duration_since(p) >= PROBE_AFTER)
                    {
                        t.probe_at = Some(now);
                        let _ = c.send_datagram(probe());
                    }
                    let stalled = view.quiet >= QUIET_DEAD || view.local_gone;
                    if stalled != s.stalled {
                        s.stalled = stalled;
                        tracing::info!(peer = %peer.short_hex(), stalled, "session path state");
                    }
                }
                if let Action::Dial(addrs) = decide(&view, &ifs)
                    && t.next_dial.is_none_or(|n| now >= n)
                    && !lock(&dialing).contains(&peer)
                {
                    t.next_dial = Some(now + DIAL_EVERY);
                    jobs.push((peer, addrs));
                }
            }
        }
        if changed {
            inner.changed();
        }
        for (peer, addrs) in jobs {
            lock(&dialing).insert(peer);
            let inner = inner.clone();
            let dialing = dialing.clone();
            tokio::spawn(async move {
                tracing::debug!(peer = %peer.short_hex(), n = addrs.len(), "dialing another path");
                let r = net::connect_paired(&inner, peer, &addrs).await;
                lock(&dialing).remove(&peer);
                if let Err(e) = r {
                    tracing::debug!(peer = %peer.short_hex(), error = %e, "path dial ended");
                }
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn iface(name: &str, ip: &str, mask: &str, bluetooth: bool) -> Iface {
        let _ = name;
        Iface {
            ip: ip.parse().unwrap(),
            mask: Some(mask.parse().unwrap()),
            bluetooth,
        }
    }

    fn ifs() -> Vec<Iface> {
        vec![
            iface("wlan0", "192.168.18.13", "255.255.255.0", false),
            iface("enxe848b8c82000", "10.124.64.208", "255.255.255.0", true),
        ]
    }

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn view(path: PathKind) -> View {
        View {
            path,
            age: Duration::from_secs(60),
            quiet: Duration::ZERO,
            local_gone: false,
            remote: Some(a("192.168.18.23:47811")),
            fresh: vec![],
            last: vec![a("192.168.18.23:47811")],
        }
    }

    #[test]
    fn interface_names() {
        assert!(bluetooth_name("bnep0"));
        assert!(bluetooth_name("bt-pan"));
        assert!(bluetooth_name("Bluetooth Network Connection"));
        assert!(!bluetooth_name("wlan0"));
        assert!(!bluetooth_name("enxe848b8c82000"));
        assert!(!is_bluetooth_interface("../../etc"));
    }

    #[test]
    fn path_label_and_routes() {
        let ifs = ifs();
        assert_eq!(
            classify_ip("10.124.64.208".parse().unwrap(), &ifs),
            PathKind::BtPan
        );
        assert_eq!(
            classify_ip("192.168.18.13".parse().unwrap(), &ifs),
            PathKind::Lan
        );
        assert_eq!(
            route("10.124.64.53".parse().unwrap(), &ifs),
            Some(PathKind::BtPan)
        );
        assert_eq!(
            route("192.168.18.23".parse().unwrap(), &ifs),
            Some(PathKind::Lan)
        );
        assert_eq!(route("8.8.8.8".parse().unwrap(), &ifs), None);
    }

    #[test]
    fn a_healthy_lan_session_is_left_alone() {
        let mut v = view(PathKind::Lan);
        v.fresh = vec![a("192.168.18.23:47811"), a("10.124.64.53:47811")];
        assert_eq!(decide(&v, &ifs()), Action::Nothing);
    }

    #[test]
    fn a_quiet_lan_session_dials_the_pan_address() {
        let mut v = view(PathKind::Lan);
        v.quiet = QUIET_DEAD;
        v.fresh = vec![a("10.124.64.53:47811"), a("192.168.18.23:47811")];
        assert_eq!(
            decide(&v, &ifs()),
            Action::Dial(vec![a("10.124.64.53:47811")])
        );
        // Without another address there is nothing to dial.
        v.fresh.clear();
        assert_eq!(decide(&v, &ifs()), Action::Nothing);
        // A lost local interface counts at once.
        let mut v = view(PathKind::Lan);
        v.local_gone = true;
        v.fresh = vec![a("10.124.64.53:47811")];
        assert!(matches!(decide(&v, &ifs()), Action::Dial(_)));
    }

    #[test]
    fn pan_moves_back_when_the_lan_is_announced() {
        let mut v = view(PathKind::BtPan);
        v.remote = Some(a("10.124.64.53:47811"));
        v.fresh = vec![a("10.124.64.53:47811")];
        assert_eq!(decide(&v, &ifs()), Action::Nothing);
        v.fresh.push(a("192.168.18.23:47811"));
        assert_eq!(
            decide(&v, &ifs()),
            Action::Dial(vec![a("192.168.18.23:47811")])
        );
        // A brand-new session is not moved yet.
        v.age = Duration::from_secs(1);
        assert_eq!(decide(&v, &ifs()), Action::Nothing);
    }

    #[test]
    fn ble_moves_to_any_announced_address() {
        let mut v = view(PathKind::Ble);
        v.remote = None;
        assert_eq!(decide(&v, &ifs()), Action::Nothing);
        v.fresh = vec![a("10.124.64.53:47811")];
        assert_eq!(
            decide(&v, &ifs()),
            Action::Dial(vec![a("10.124.64.53:47811")])
        );
    }
}
