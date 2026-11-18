//! Discovery: mDNS plus UDP broadcast (R2, design §5).
//!
//! Discovery data is unauthenticated. It only says where to try connecting;
//! every trust decision happens in TLS. Inbound packets are size-limited,
//! parsed strictly, and rate-limited per source address (R2.6).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc};
use urdevice_proto::{Announce, MAX_ANNOUNCE, PROTO_VERSION};

use crate::pairing::{clean_name, clean_platform};
use crate::{CoreError, Result};

/// UDP discovery port (design §17.1).
pub const DISCOVERY_PORT: u16 = 47810;
/// mDNS service type.
pub const SERVICE_TYPE: &str = "_urdevice._udp.local.";
/// Announcement interval (R2.2).
pub const ANNOUNCE_EVERY: Duration = Duration::from_secs(5);
/// Inbound packets allowed per second per source (R2.6).
pub const RATE_PER_SEC: f64 = 20.0;

/// How a device was seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    /// UDP broadcast.
    Broadcast,
    /// mDNS, from the core or from Android's NsdManager.
    Mdns,
}

/// One sighting of a device on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// First 8 bytes of its Device ID.
    pub short_id: [u8; 8],
    /// Cleaned display name.
    pub name: String,
    /// Cleaned platform string.
    pub platform: String,
    /// Where its QUIC endpoint is.
    pub addr: SocketAddr,
    /// Source.
    pub via: Via,
}

/// Validate an announcement from `src`. `None` for anything malformed.
pub fn parse_announce(packet: &[u8], src: SocketAddr) -> Option<Seen> {
    let a = urdevice_proto::decode_announce(packet).ok()?;
    if a.v != PROTO_VERSION {
        return None;
    }
    let short_id: [u8; 8] = a.short_id.as_slice().try_into().ok()?;
    let port = u16::try_from(a.port).ok().filter(|p| *p != 0)?;
    Some(Seen {
        short_id,
        name: clean_name(&a.name),
        platform: clean_platform(&a.platform),
        addr: SocketAddr::new(src.ip(), port),
        via: Via::Broadcast,
    })
}

/// Token bucket per source address.
#[derive(Debug, Default)]
pub struct RateLimiter {
    buckets: HashMap<IpAddr, (f64, Instant)>,
}

impl RateLimiter {
    /// True when a packet from `ip` may be processed now.
    pub fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.buckets.len() > 1024 {
            self.buckets
                .retain(|_, (_, t)| now.duration_since(*t) < Duration::from_secs(10));
        }
        let (tokens, last) = self.buckets.entry(ip).or_insert((RATE_PER_SEC, now));
        let refill = now.duration_since(*last).as_secs_f64() * RATE_PER_SEC;
        *tokens = (*tokens + refill).min(RATE_PER_SEC);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Settings for [`Discovery::start`].
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    /// UDP broadcast port. 0 turns UDP discovery off.
    pub udp_port: u16,
    /// Run the built-in mDNS responder and browser.
    pub mdns: bool,
}

#[derive(Debug)]
struct Shared {
    me: Mutex<Announce>,
    hidden: AtomicBool,
    kick: Notify,
    stopped: AtomicBool,
}

/// Running discovery. Dropping it stops announcing.
#[derive(Debug)]
pub struct Discovery {
    shared: Arc<Shared>,
    #[cfg(feature = "mdns")]
    mdns: Option<mdns::Mdns>,
}

impl Discovery {
    /// Start announcing `me` and report sightings on `tx`. Must run inside a
    /// tokio runtime.
    pub fn start(
        cfg: &DiscoveryConfig,
        me: Announce,
        hidden: bool,
        tx: mpsc::Sender<Seen>,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            me: Mutex::new(me.clone()),
            hidden: AtomicBool::new(hidden),
            kick: Notify::new(),
            stopped: AtomicBool::new(false),
        });
        if cfg.udp_port != 0 {
            let sock = Arc::new(bind_udp(cfg.udp_port)?);
            tokio::spawn(send_loop(sock.clone(), cfg.udp_port, shared.clone()));
            tokio::spawn(recv_loop(
                sock,
                me.short_id.clone(),
                tx.clone(),
                shared.clone(),
            ));
        }
        #[cfg(feature = "mdns")]
        let mdns = if cfg.mdns {
            match mdns::Mdns::start(&me, hidden, tx) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(error = %e, "mDNS is unavailable; using UDP broadcast only");
                    None
                }
            }
        } else {
            None
        };
        Ok(Self {
            shared,
            #[cfg(feature = "mdns")]
            mdns,
        })
    }

    /// Hidden mode: stop announcing (R2.7).
    pub fn set_hidden(&self, hidden: bool) {
        self.shared.hidden.store(hidden, Ordering::Relaxed);
        #[cfg(feature = "mdns")]
        if let (Some(m), Ok(me)) = (&self.mdns, self.shared.me.lock()) {
            m.update(&me, hidden);
        }
        self.shared.kick.notify_one();
    }

    /// Change the announced name.
    pub fn set_name(&self, name: &str) {
        if let Ok(mut me) = self.shared.me.lock() {
            me.name = name.to_owned();
            #[cfg(feature = "mdns")]
            if let Some(m) = &self.mdns {
                m.update(&me, self.shared.hidden.load(Ordering::Relaxed));
            }
        }
        self.shared.kick.notify_one();
    }

    /// Announce right away, for example when the Pair screen opens.
    pub fn announce_now(&self) {
        self.shared.kick.notify_one();
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
        self.shared.kick.notify_one();
    }
}

fn bind_udp(port: u16) -> Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    s.set_reuse_port(true)?;
    s.set_broadcast(true)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port).into())?;
    UdpSocket::from_std(s.into()).map_err(CoreError::from)
}

/// Directed broadcast addresses of the local IPv4 interfaces.
fn broadcast_targets() -> Vec<Ipv4Addr> {
    let mut out = vec![Ipv4Addr::BROADCAST];
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if let if_addrs::IfAddr::V4(v4) = i.addr
                && let Some(b) = v4.broadcast
                && !v4.ip.is_loopback()
                && !out.contains(&b)
            {
                out.push(b);
            }
        }
    }
    out
}

async fn send_loop(sock: Arc<UdpSocket>, port: u16, shared: Arc<Shared>) {
    loop {
        if shared.stopped.load(Ordering::Relaxed) {
            return;
        }
        if !shared.hidden.load(Ordering::Relaxed) {
            let packet = shared
                .me
                .lock()
                .ok()
                .and_then(|me| urdevice_proto::encode_announce(&me).ok());
            if let Some(packet) = packet {
                for target in broadcast_targets() {
                    let _ = sock
                        .send_to(&packet, SocketAddr::new(IpAddr::V4(target), port))
                        .await;
                }
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(ANNOUNCE_EVERY) => {}
            _ = shared.kick.notified() => {}
        }
    }
}

async fn recv_loop(
    sock: Arc<UdpSocket>,
    own_short: Vec<u8>,
    tx: mpsc::Sender<Seen>,
    shared: Arc<Shared>,
) {
    let mut buf = vec![0u8; MAX_ANNOUNCE + 1];
    let mut limiter = RateLimiter::default();
    loop {
        if shared.stopped.load(Ordering::Relaxed) {
            return;
        }
        let (n, src) = match sock.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(error = %e, "discovery receive failed");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        if !limiter.allow(src.ip(), Instant::now()) {
            continue;
        }
        let Some(seen) = parse_announce(&buf[..n], src) else {
            continue;
        };
        if seen.short_id.as_slice() == own_short.as_slice() {
            continue;
        }
        if tx.send(seen).await.is_err() {
            return;
        }
    }
}

#[cfg(feature = "mdns")]
mod mdns {
    use std::collections::HashMap;
    use std::net::SocketAddr;

    use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
    use tokio::sync::mpsc;
    use urdevice_proto::{Announce, PROTO_VERSION};

    use super::{SERVICE_TYPE, Seen, Via};
    use crate::pairing::{clean_name, clean_platform};
    use crate::{CoreError, Result};

    pub(super) struct Mdns {
        daemon: ServiceDaemon,
        registered: std::sync::Mutex<Option<String>>,
    }

    impl std::fmt::Debug for Mdns {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("Mdns").finish_non_exhaustive()
        }
    }

    fn info(me: &Announce) -> Result<ServiceInfo> {
        let short = data_encoding::HEXLOWER.encode(&me.short_id);
        let host = format!("urdevice-{short}.local.");
        let props: HashMap<String, String> = [
            ("v".to_owned(), PROTO_VERSION.to_string()),
            ("id".to_owned(), short),
            ("p".to_owned(), me.platform.clone()),
        ]
        .into_iter()
        .collect();
        let port = u16::try_from(me.port).unwrap_or(0);
        ServiceInfo::new(SERVICE_TYPE, &me.name, &host, "", port, props)
            .map(ServiceInfo::enable_addr_auto)
            .map_err(|e| CoreError::Network(e.to_string()))
    }

    impl Mdns {
        pub(super) fn start(me: &Announce, hidden: bool, tx: mpsc::Sender<Seen>) -> Result<Self> {
            let daemon = ServiceDaemon::new().map_err(|e| CoreError::Network(e.to_string()))?;
            let rx = daemon
                .browse(SERVICE_TYPE)
                .map_err(|e| CoreError::Network(e.to_string()))?;
            let own = me.short_id.clone();
            std::thread::Builder::new()
                .name("urdevice-mdns".into())
                .spawn(move || {
                    while let Ok(ev) = rx.recv() {
                        if let ServiceEvent::ServiceResolved(s) = ev {
                            for seen in resolved(&s, &own) {
                                if tx.blocking_send(seen).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                })
                .map_err(CoreError::Io)?;
            let m = Self {
                daemon,
                registered: std::sync::Mutex::new(None),
            };
            m.update(me, hidden);
            Ok(m)
        }

        pub(super) fn update(&self, me: &Announce, hidden: bool) {
            let Ok(mut reg) = self.registered.lock() else {
                return;
            };
            if let Some(full) = reg.take() {
                let _ = self.daemon.unregister(&full);
            }
            if hidden {
                return;
            }
            match info(me) {
                Ok(i) => {
                    let full = i.get_fullname().to_owned();
                    match self.daemon.register(i) {
                        Ok(()) => *reg = Some(full),
                        Err(e) => tracing::warn!(error = %e, "mDNS register failed"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "mDNS service info is invalid"),
            }
        }
    }

    impl Drop for Mdns {
        fn drop(&mut self) {
            let _ = self.daemon.shutdown();
        }
    }

    fn resolved(s: &mdns_sd::ResolvedService, own: &[u8]) -> Vec<Seen> {
        let Some(id) = s.get_property_val_str("id") else {
            return vec![];
        };
        if s.get_property_val_str("v") != Some(&PROTO_VERSION.to_string()) {
            return vec![];
        }
        let Ok(bytes) = data_encoding::HEXLOWER_PERMISSIVE.decode(id.as_bytes()) else {
            return vec![];
        };
        let Ok(short_id) = <[u8; 8]>::try_from(bytes.as_slice()) else {
            return vec![];
        };
        if short_id.as_slice() == own {
            return vec![];
        }
        let name = s
            .get_fullname()
            .strip_suffix(&format!(".{SERVICE_TYPE}"))
            .unwrap_or("Unknown device");
        let platform = clean_platform(s.get_property_val_str("p").unwrap_or(""));
        s.get_addresses_v4()
            .into_iter()
            .take(4)
            .map(|ip| Seen {
                short_id,
                name: clean_name(name),
                platform: platform.clone(),
                addr: SocketAddr::new(ip.into(), s.get_port()),
                via: Via::Mdns,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(name: &str, port: u32) -> Vec<u8> {
        urdevice_proto::encode_announce(&Announce {
            v: PROTO_VERSION,
            short_id: vec![5; 8],
            name: name.into(),
            port,
            platform: "android".into(),
        })
        .unwrap()
    }

    #[test]
    fn parses_valid_and_rejects_bad_announcements() {
        let src: SocketAddr = "192.168.1.20:47810".parse().unwrap();
        let seen = parse_announce(&packet("Pixel", 47811), src).unwrap();
        assert_eq!(seen.addr, "192.168.1.20:47811".parse().unwrap());
        assert_eq!(seen.name, "Pixel");
        assert!(parse_announce(&packet("Pixel", 0), src).is_none());
        assert!(parse_announce(&packet("Pixel", 70000), src).is_none());
        assert!(parse_announce(b"URDV\xff\xff", src).is_none());
        assert!(parse_announce(&[0; 600], src).is_none());
    }

    #[test]
    fn rate_limit_is_twenty_per_second_per_source() {
        let mut l = RateLimiter::default();
        let t = Instant::now();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let allowed = (0..50).filter(|_| l.allow(a, t)).count();
        assert_eq!(allowed, 20);
        assert!(l.allow(b, t));
        assert!(l.allow(a, t + Duration::from_millis(100)));
    }
}
