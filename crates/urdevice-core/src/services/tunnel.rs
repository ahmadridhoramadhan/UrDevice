//! Localhost sharing and Expose on LAN (#22, design §11, R13).
//!
//! Service id: `ServiceId::TUNNEL`, capability `tunnel.v1` (Localhost,
//! default Deny).
//!
//! **Host.** The user shares loopback TCP ports ([`Tunnel::share`]); the
//! list lives in the trust store (`shared_port`, MAC-protected) and is pushed
//! to every connected device, empty while the Localhost setting is Deny. A
//! viewer's stream is accepted only after the node's permission check
//! (`Access::Host`), and only when its port is in the shared list; the host
//! then connects to `127.0.0.1:P` (or `[::1]:P`) and nothing else, so a peer
//! can never choose the target host (R13.5).
//!
//! **Viewer.** [`Tunnel::open`] asks the host for one activity, binds
//! `127.0.0.1:P` (or the next free port), and opens one QUIC stream per
//! accepted TCP connection (R13.2–R13.4). Stopping the activity on either
//! device closes the listener and every connection.
//!
//! **Expose on LAN** ([`Tunnel::set_lan`], R13.6) opens a listener on this
//! device's LAN addresses that forwards to `127.0.0.1:P`. "1 hour" turns
//! itself off; "Always on" needs `confirmed` and is restored after a restart.
//!
//! Nothing here logs traffic; only ports, sizes, and device ids.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use urdevice_proto::features::tunnel as wire;

use super::{Access, Direction, Grant, Message, Registry, Service, ServiceCtx, ServiceId, Stream};
use super::{Slot, stream_code};
use crate::identity::DeviceId;
use crate::permission::{Capability, Mode};
use crate::store::SharedPort;
use crate::{CoreError, Result};

/// Release 2 capabilities this module ships. Release 1 capabilities are
/// always listed, so this stays empty.
pub(crate) const SHIPPED: &[Capability] = &[];

/// How long "1 hour" keeps a port open on the LAN.
pub const LAN_HOUR: u64 = 3600;
/// How many ports above the requested one the viewer tries before it lets
/// the OS pick one.
const PORT_SEARCH: u16 = 20;
/// Most TCP connections at once through one tunnel or LAN listener.
const MAX_CONNS: u32 = 256;
/// Most TCP connections the host forwards at once, for all peers together.
const MAX_HOST_CONNS: u32 = 512;
/// How long the host waits for the shared port to accept a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Copy buffer per direction.
const BUF: usize = 32 * 1024;
/// How often the housekeeping task runs: LAN expiry, setting changes,
/// stats redraws.
const TICK: Duration = Duration::from_secs(1);

/// Expose on LAN choice (R13.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanMode {
    /// Only paired devices reach the port, through their tunnels.
    Off,
    /// Open on the LAN for one hour.
    OneHour,
    /// Open on the LAN until the user turns it off, also after a restart.
    Always,
}

/// A port this device shares, for the "This device" view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedPortView {
    pub port: u16,
    pub label: String,
    pub lan: LanMode,
    /// Unix seconds when "1 hour" ends; 0 otherwise.
    pub lan_until: u64,
    /// True while the LAN listener is bound. False with `lan` on means it
    /// could not bind yet (no network, or the port is already open on the LAN).
    pub lan_listening: bool,
    /// The LAN addresses the port is reachable on.
    pub lan_addrs: Vec<SocketAddr>,
    /// Open LAN connections.
    pub lan_connections: u32,
}

/// A port a paired device shares with this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemotePort {
    pub port: u16,
    pub label: String,
}

/// Where a tunnel is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TunnelStatus {
    /// Waiting for the host to allow it (Always Ask) .
    Opening,
    /// Listening on `local_port`.
    Open,
    /// Refused or failed: `denied`, `declined`, `timeout`, `offline`,
    /// `unsupported`, `bind`, or `refused_port` (the host does not share it).
    Failed(String),
}

/// One active tunnel, on the viewer (outgoing) or on the host (incoming).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelView {
    /// Local id for [`Tunnel::stop`].
    pub id: u64,
    pub peer: DeviceId,
    pub peer_name: String,
    /// True on the host: `peer` uses this device's port.
    pub incoming: bool,
    /// The host's port.
    pub remote_port: u16,
    /// The viewer's loopback port (viewer only, once open).
    pub local_port: Option<u16>,
    pub status: TunnelStatus,
    /// Open TCP connections.
    pub connections: u32,
    /// Bytes from the viewer to the host.
    pub bytes_up: u64,
    /// Bytes from the host to the viewer.
    pub bytes_down: u64,
}

/// Optional listener for the UI. The UI can also redraw on the node's
/// `Changed` event and read [`Tunnel::tunnels`].
pub trait TunnelEvents: Send + Sync {
    /// A viewer tunnel is listening on `local_port`.
    fn opened(&self, id: u64, peer: DeviceId, remote_port: u16, local_port: u16);
    /// A viewer tunnel could not open; `reason` as in [`TunnelStatus::Failed`].
    fn failed(&self, id: u64, peer: DeviceId, remote_port: u16, reason: &str);
}

#[derive(Default)]
struct Stats {
    conns: AtomicU32,
    up: AtomicU64,
    down: AtomicU64,
}

impl Stats {
    fn total(&self) -> u64 {
        self.up.load(Ordering::Relaxed) + self.down.load(Ordering::Relaxed)
    }
}

struct ViewerEntry {
    id: u64,
    peer: DeviceId,
    port: u16,
    local_port: Option<u16>,
    activity: Option<u64>,
    status: TunnelStatus,
    stats: Arc<Stats>,
    /// Dropping it stops the listener and its connections.
    _stop: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
}

struct HostEntry {
    id: u64,
    peer: DeviceId,
    activity: u64,
    port: u16,
    stats: Arc<Stats>,
    _stop: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
}

struct LanEntry {
    addrs: Vec<SocketAddr>,
    stats: Arc<Stats>,
    _stop: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    remote: HashMap<DeviceId, Vec<RemotePort>>,
    viewer: Vec<ViewerEntry>,
    host: Vec<HostEntry>,
    lan: HashMap<u16, LanEntry>,
    host_conns: u32,
    /// Whether the last list pushed to peers was non-empty-able (setting
    /// not Deny), to spot setting changes.
    last_open: Option<bool>,
    last_activity: u64,
    lan_retry: u32,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

#[derive(Default)]
struct Shared {
    ctx: OnceLock<ServiceCtx>,
    st: Mutex<State>,
    events: Slot<dyn TunnelEvents>,
}

/// The localhost sharing service.
#[derive(Default)]
pub struct Tunnel {
    s: Arc<Shared>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn stop_channel() -> (watch::Sender<bool>, watch::Receiver<bool>) {
    watch::channel(false)
}

/// Resolves when the sender says stop or is dropped.
async fn stopped(rx: &mut watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.st.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn ctx(&self) -> Result<&ServiceCtx> {
        self.ctx.get().ok_or(CoreError::Stopped)
    }

    /// The list for peers: empty while the setting is Deny (no point
    /// showing ports nobody may open, and less to learn about this device).
    fn list_for_peers(&self, ctx: &ServiceCtx) -> wire::SharedPorts {
        if ctx.local_mode() == Mode::Deny {
            return wire::SharedPorts::default();
        }
        wire::SharedPorts {
            ports: ctx
                .shared_ports()
                .unwrap_or_default()
                .into_iter()
                .take(wire::MAX_PORTS)
                .map(|p| wire::PortInfo {
                    port: u32::from(p.port),
                    label: p.label,
                })
                .collect(),
        }
    }

    fn push(&self, ctx: &ServiceCtx, to: Option<DeviceId>) {
        let list = self.list_for_peers(ctx);
        let peers = match to {
            Some(p) => vec![p],
            None => ctx.connected(),
        };
        for p in peers {
            if let Err(e) = ctx.send(p, Direction::ToController, 0, &list) {
                tracing::debug!(peer = %p.short_hex(), error = %e, "port list not sent");
            }
        }
    }

    fn is_shared(ctx: &ServiceCtx, port: u16) -> bool {
        ctx.shared_ports()
            .unwrap_or_default()
            .iter()
            .any(|p| p.port == port)
    }

    // -- host: tunnels --------------------------------------------------------

    /// A viewer's connection, after the node's permission check.
    fn host_stream(self: &Arc<Self>, ctx: &ServiceCtx, grant: &Grant, stream: Stream) {
        let Stream {
            peer,
            kind,
            meta,
            send,
            mut recv,
            ..
        } = stream;
        let Some(mut send) = send else {
            let _ = recv.stop(stream_code::UNSUPPORTED.into());
            return;
        };
        let refuse = |send: &mut quinn::SendStream, recv: &mut quinn::RecvStream, code: u32| {
            let _ = send.reset(code.into());
            let _ = recv.stop(code.into());
        };
        let port = match (kind, wire::decode_open(&meta)) {
            (wire::STREAM_TCP, Some(p)) => p,
            _ => {
                refuse(&mut send, &mut recv, stream_code::UNSUPPORTED);
                return;
            }
        };
        // R13.5: only ports the user shared, read from the trust store now.
        if !Self::is_shared(ctx, port) {
            tracing::info!(peer = %peer.short_hex(), port, "tunnel to a port that is not shared refused");
            refuse(&mut send, &mut recv, stream_code::REFUSED);
            return;
        }
        let (stats, mut stop_rx) = {
            let mut st = self.lock();
            if st.host_conns >= MAX_HOST_CONNS {
                drop(st);
                refuse(&mut send, &mut recv, stream_code::CANCELLED);
                return;
            }
            let activity = grant.activity();
            let pos = st
                .host
                .iter()
                .position(|h| h.peer == peer && h.activity == activity && h.port == port);
            let pos = match pos {
                Some(p) => p,
                None => {
                    let id = st.id();
                    let (tx, rx) = stop_channel();
                    st.host.push(HostEntry {
                        id,
                        peer,
                        activity,
                        port,
                        stats: Arc::default(),
                        _stop: tx,
                        stop_rx: rx,
                    });
                    st.host.len() - 1
                }
            };
            st.host_conns += 1;
            let h = &st.host[pos];
            (h.stats.clone(), h.stop_rx.clone())
        };
        stats.conns.fetch_add(1, Ordering::Relaxed);
        ctx.changed();
        let me = self.clone();
        let ctx2 = ctx.clone();
        ctx.spawn(async move {
            let tcp = tokio::select! {
                t = connect_loopback(port) => t,
                () = stopped(&mut stop_rx) => Err(io::Error::other("stopped")),
            };
            match tcp {
                Ok(tcp) => {
                    // Host side: bytes from the viewer go "up".
                    pipe(tcp, send, recv, &stats.down, &stats.up, stop_rx).await;
                }
                Err(e) => {
                    tracing::debug!(port, error = %e, "shared port did not accept");
                    let _ = send.reset(stream_code::CANCELLED.into());
                    let _ = recv.stop(stream_code::CANCELLED.into());
                }
            }
            stats.conns.fetch_sub(1, Ordering::Relaxed);
            {
                let mut st = me.lock();
                st.host_conns = st.host_conns.saturating_sub(1);
            }
            ctx2.changed();
        });
    }

    // -- viewer -----------------------------------------------------------------

    fn viewer_open(self: &Arc<Self>, peer: DeviceId, port: u16) -> Result<u64> {
        let ctx = self.ctx()?.clone();
        if port == 0 {
            return Err(CoreError::InvalidInput("port 0".into()));
        }
        let (id, stats, stop_rx) = {
            let mut st = self.lock();
            if let Some(v) = st.viewer.iter().find(|v| {
                v.peer == peer && v.port == port && !matches!(v.status, TunnelStatus::Failed(_))
            }) {
                return Ok(v.id);
            }
            st.viewer.retain(|v| !(v.peer == peer && v.port == port));
            let id = st.id();
            let (tx, rx) = stop_channel();
            let stats: Arc<Stats> = Arc::default();
            st.viewer.push(ViewerEntry {
                id,
                peer,
                port,
                local_port: None,
                activity: None,
                status: TunnelStatus::Opening,
                stats: stats.clone(),
                _stop: tx,
                stop_rx: rx.clone(),
            });
            (id, stats, rx)
        };
        ctx.changed();
        let me = self.clone();
        ctx.clone().spawn(async move {
            let activity = match ctx.request(peer).await {
                Ok(a) => a,
                Err(e) => {
                    let reason = match e {
                        CoreError::Refused(r) => r,
                        _ => "offline".into(),
                    };
                    me.viewer_failed(&ctx, id, &reason);
                    return;
                }
            };
            // Stopped while the host was asking its user.
            let still_here = {
                let mut st = me.lock();
                match st.viewer.iter_mut().find(|v| v.id == id) {
                    Some(v) => {
                        v.activity = Some(activity);
                        true
                    }
                    None => false,
                }
            };
            if !still_here {
                ctx.stop(peer, activity);
                return;
            }
            let listener = match bind_local(port) {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(port, error = %e, "no loopback port for the tunnel");
                    me.viewer_failed(&ctx, id, "bind");
                    ctx.stop(peer, activity);
                    return;
                }
            };
            let local_port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            {
                let mut st = me.lock();
                if let Some(v) = st.viewer.iter_mut().find(|v| v.id == id) {
                    v.local_port = Some(local_port);
                    v.status = TunnelStatus::Open;
                }
            }
            tracing::info!(peer = %peer.short_hex(), port, local_port, "tunnel open");
            if let Some(ev) = me.events.get() {
                ev.opened(id, peer, port, local_port);
            }
            ctx.changed();
            me.accept_loop(ctx, id, peer, port, activity, listener, stats, stop_rx)
                .await;
        });
        Ok(id)
    }

    fn viewer_failed(&self, ctx: &ServiceCtx, id: u64, reason: &str) {
        let hit = {
            let mut st = self.lock();
            st.viewer.iter_mut().find(|v| v.id == id).map(|v| {
                v.status = TunnelStatus::Failed(reason.to_owned());
                v.local_port = None;
                // Keep the failed entry when the caller stops the activity.
                v.activity = None;
                // Drop the sender: stops the listener and connections.
                let (tx, rx) = stop_channel();
                v._stop = tx;
                v.stop_rx = rx;
                (v.peer, v.port)
            })
        };
        if let Some((peer, port)) = hit {
            tracing::info!(peer = %peer.short_hex(), port, reason, "tunnel failed");
            if let Some(ev) = self.events.get() {
                ev.failed(id, peer, port, reason);
            }
        }
        ctx.changed();
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept_loop(
        self: Arc<Self>,
        ctx: ServiceCtx,
        id: u64,
        peer: DeviceId,
        port: u16,
        activity: u64,
        listener: std::net::TcpListener,
        stats: Arc<Stats>,
        mut stop_rx: watch::Receiver<bool>,
    ) {
        let listener = match TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(_) => {
                self.viewer_failed(&ctx, id, "bind");
                ctx.stop(peer, activity);
                return;
            }
        };
        let meta = prost::Message::encode_to_vec(&wire::TunnelOpen {
            port: u32::from(port),
        });
        loop {
            let accepted = tokio::select! {
                a = listener.accept() => a,
                () = stopped(&mut stop_rx) => break,
            };
            let Ok((tcp, _)) = accepted else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            if stats.conns.load(Ordering::Relaxed) >= MAX_CONNS {
                drop(tcp);
                continue;
            }
            let _ = tcp.set_nodelay(true);
            stats.conns.fetch_add(1, Ordering::Relaxed);
            ctx.changed();
            let (ctx2, stats2, meta2, me) =
                (ctx.clone(), stats.clone(), meta.clone(), self.clone());
            let conn_stop = stop_rx.clone();
            ctx.spawn(async move {
                match ctx2
                    .open_bi(peer, Direction::ToHost, activity, wire::STREAM_TCP, meta2)
                    .await
                {
                    Ok((send, recv)) => {
                        let refused =
                            pipe(tcp, send, recv, &stats2.up, &stats2.down, conn_stop).await;
                        if refused {
                            me.viewer_failed(&ctx2, id, "refused_port");
                            ctx2.stop(peer, activity);
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "tunnel stream not opened"),
                }
                stats2.conns.fetch_sub(1, Ordering::Relaxed);
                ctx2.changed();
            });
        }
    }

    // -- LAN exposure -------------------------------------------------------------

    /// Bind the LAN listener for `port` if it is not bound. Returns the
    /// addresses it listens on.
    fn start_lan(self: &Arc<Self>, ctx: &ServiceCtx, port: u16) -> Result<Vec<SocketAddr>> {
        if let Some(e) = self.lock().lan.get(&port) {
            return Ok(e.addrs.clone());
        }
        let mut bound = Vec::new();
        let mut last_err = None;
        for ip in crate::node::local_ipv4_addrs() {
            match bind_reuse(SocketAddr::new(ip, port)) {
                Ok(l) => bound.push(l),
                Err(e) => last_err = Some(e),
            }
        }
        if bound.is_empty() {
            return Err(CoreError::Network(match last_err {
                Some(e) => format!("could not open port {port} on the LAN: {e}"),
                None => "this device has no LAN address".into(),
            }));
        }
        let addrs: Vec<SocketAddr> = bound.iter().filter_map(|l| l.local_addr().ok()).collect();
        let (tx, rx) = stop_channel();
        let stats: Arc<Stats> = Arc::default();
        for l in bound {
            let (stats, mut rx, ctx2) = (stats.clone(), rx.clone(), ctx.clone());
            ctx.spawn(async move {
                let Ok(l) = TcpListener::from_std(l) else {
                    return;
                };
                loop {
                    let accepted = tokio::select! {
                        a = l.accept() => a,
                        () = stopped(&mut rx) => break,
                    };
                    let Ok((tcp, from)) = accepted else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    };
                    if stats.conns.load(Ordering::Relaxed) >= MAX_CONNS {
                        continue;
                    }
                    tracing::debug!(port, from = %from.ip(), "LAN connection");
                    stats.conns.fetch_add(1, Ordering::Relaxed);
                    ctx2.changed();
                    let (stats, mut rx, ctx3) = (stats.clone(), rx.clone(), ctx2.clone());
                    ctx2.spawn(async move {
                        let target = tokio::select! {
                            t = connect_loopback(port) => t.ok(),
                            () = stopped(&mut rx) => None,
                        };
                        if let Some(target) = target {
                            tcp_pipe(tcp, target, &stats, rx).await;
                        }
                        stats.conns.fetch_sub(1, Ordering::Relaxed);
                        ctx3.changed();
                    });
                }
            });
        }
        tracing::info!(port, addrs = addrs.len(), "port open on the LAN");
        self.lock().lan.insert(
            port,
            LanEntry {
                addrs: addrs.clone(),
                stats,
                _stop: tx,
            },
        );
        Ok(addrs)
    }

    fn stop_lan(&self, port: u16) {
        if self.lock().lan.remove(&port).is_some() {
            tracing::info!(port, "port closed on the LAN");
        }
    }

    /// Housekeeping, once per [`TICK`]: end "1 hour" exposures, retry LAN
    /// listeners that could not bind, re-send the list when the setting
    /// changes between Deny and the rest, and redraw while traffic flows.
    fn tick(self: &Arc<Self>, ctx: &ServiceCtx) {
        let t = now();
        let ports = ctx.shared_ports().unwrap_or_default();
        let mut changed = false;
        for p in &ports {
            match p.lan_expose_until {
                Some(u) if u != 0 && u <= t => {
                    let off = SharedPort {
                        lan_expose_until: None,
                        ..p.clone()
                    };
                    let _ = ctx.save_shared_port(&off);
                    self.stop_lan(p.port);
                    changed = true;
                }
                None => {
                    if self.lock().lan.contains_key(&p.port) {
                        self.stop_lan(p.port);
                        changed = true;
                    }
                }
                Some(_) => {}
            }
        }
        // LAN listeners for removed ports.
        let gone: Vec<u16> = self
            .lock()
            .lan
            .keys()
            .copied()
            .filter(|k| !ports.iter().any(|p| p.port == *k))
            .collect();
        for g in gone {
            self.stop_lan(g);
            changed = true;
        }
        // Retry unbound exposures every 10 s (network not up at boot).
        let retry = {
            let mut st = self.lock();
            st.lan_retry = (st.lan_retry + 1) % 10;
            st.lan_retry == 0
        };
        if retry {
            for p in &ports {
                let on = matches!(p.lan_expose_until, Some(u) if u == 0 || u > t);
                if on
                    && !self.lock().lan.contains_key(&p.port)
                    && self.start_lan(ctx, p.port).is_ok()
                {
                    changed = true;
                }
            }
        }
        let open = ctx.local_mode() != Mode::Deny;
        let flipped = {
            let mut st = self.lock();
            let f = st.last_open.is_some_and(|o| o != open);
            st.last_open = Some(open);
            f
        };
        if flipped {
            self.push(ctx, None);
        }
        let activity: u64 = {
            let st = self.lock();
            st.viewer
                .iter()
                .map(|v| v.stats.total())
                .chain(st.host.iter().map(|h| h.stats.total()))
                .chain(st.lan.values().map(|l| l.stats.total()))
                .sum()
        };
        {
            let mut st = self.lock();
            if st.last_activity != activity {
                st.last_activity = activity;
                changed = true;
            }
        }
        if changed {
            ctx.changed();
        }
    }
}

/// Connect to the shared port on this device's loopback, IPv4 first. The
/// only place the host opens a TCP connection, and it never takes a host
/// name from the network.
async fn connect_loopback(port: u16) -> io::Result<TcpStream> {
    let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port);
    let try_one = |a| async move {
        tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(a))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))?
    };
    let s = match try_one(v4).await {
        Ok(s) => s,
        Err(_) => try_one(v6).await?,
    };
    let _ = s.set_nodelay(true);
    Ok(s)
}

/// Bind the viewer's listener on loopback: `port`, else the next free port
/// within [`PORT_SEARCH`], else one the OS picks.
pub fn bind_local(port: u16) -> io::Result<std::net::TcpListener> {
    let candidates = (0..=PORT_SEARCH)
        .filter_map(|d| port.checked_add(d))
        .chain(std::iter::once(0));
    let mut last = None;
    for p in candidates {
        match std::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), p)) {
            Ok(l) => {
                l.set_nonblocking(true)?;
                return Ok(l);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free port")))
}

/// A non-blocking listener without `SO_REUSEADDR` sharing tricks: binding a
/// LAN address must fail if something else already serves it.
fn bind_reuse(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    let l = std::net::TcpListener::bind(addr)?;
    l.set_nonblocking(true)?;
    Ok(l)
}

/// Copy between a TCP connection and a QUIC stream until both directions
/// end or `stop` fires. `out` counts TCP→QUIC bytes, `inn` QUIC→TCP bytes.
/// Returns true when the peer refused the stream (`stream_code::REFUSED`).
async fn pipe(
    tcp: TcpStream,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    out: &AtomicU64,
    inn: &AtomicU64,
    mut stop: watch::Receiver<bool>,
) -> bool {
    let (mut tr, mut tw) = tcp.into_split();
    let refused = AtomicBool::new(false);
    let is_refused = |code: quinn::VarInt| {
        if code.into_inner() == u64::from(stream_code::REFUSED) {
            refused.store(true, Ordering::Relaxed);
        }
    };
    let clean = {
        let up = async {
            let mut buf = vec![0u8; BUF];
            loop {
                let n = tr.read(&mut buf).await?;
                if n == 0 {
                    let _ = send.finish();
                    return Ok::<(), io::Error>(());
                }
                match send.write_all(&buf[..n]).await {
                    Ok(()) => {}
                    Err(quinn::WriteError::Stopped(code)) => {
                        is_refused(code);
                        return Err(io::Error::other("stopped"));
                    }
                    Err(e) => return Err(io::Error::other(e)),
                }
                out.fetch_add(n as u64, Ordering::Relaxed);
            }
        };
        let down = async {
            let mut buf = vec![0u8; BUF];
            loop {
                match recv.read(&mut buf).await {
                    Ok(Some(n)) => {
                        tw.write_all(&buf[..n]).await?;
                        inn.fetch_add(n as u64, Ordering::Relaxed);
                    }
                    Ok(None) => {
                        let _ = tw.shutdown().await;
                        return Ok::<(), io::Error>(());
                    }
                    Err(quinn::ReadError::Reset(code)) => {
                        is_refused(code);
                        return Err(io::Error::other("reset"));
                    }
                    Err(e) => return Err(io::Error::other(e)),
                }
            }
        };
        tokio::select! {
            r = async { tokio::try_join!(up, down) } => r.is_ok(),
            () = stopped(&mut stop) => false,
        }
    };
    // After a clean end, dropping the finished stream lets quinn deliver
    // what is still in flight; a reset would throw it away.
    if !clean {
        let _ = send.reset(stream_code::CANCELLED.into());
        let _ = recv.stop(stream_code::CANCELLED.into());
    }
    refused.load(Ordering::Relaxed)
}

/// Copy between two TCP connections (LAN exposure).
async fn tcp_pipe(a: TcpStream, b: TcpStream, stats: &Stats, mut stop: watch::Receiver<bool>) {
    let (mut a, mut b) = (a, b);
    tokio::select! {
        r = tokio::io::copy_bidirectional(&mut a, &mut b) => {
            if let Ok((x, y)) = r {
                stats.up.fetch_add(x, Ordering::Relaxed);
                stats.down.fetch_add(y, Ordering::Relaxed);
            }
        }
        () = stopped(&mut stop) => {}
    }
}

impl Tunnel {
    fn ctx(&self) -> Result<&ServiceCtx> {
        self.s.ctx()
    }

    /// Set the UI listener.
    pub fn set_events(&self, events: Arc<dyn TunnelEvents>) {
        self.s.events.set(events);
    }

    // -- host -------------------------------------------------------------------

    /// Share `port` (1-65535) with every paired device, or change its label
    /// (R13.1). Keeps its LAN setting.
    pub fn share(&self, port: u16, label: &str) -> Result<()> {
        let ctx = self.ctx()?;
        if port == 0 {
            return Err(CoreError::InvalidInput("port 0".into()));
        }
        let ports = ctx.shared_ports()?;
        let old = ports.iter().find(|p| p.port == port);
        if old.is_none() && ports.len() >= wire::MAX_PORTS {
            return Err(CoreError::InvalidInput("too many shared ports".into()));
        }
        ctx.save_shared_port(&SharedPort {
            port,
            label: wire::clean_label(label),
            lan_expose_until: old.and_then(|p| p.lan_expose_until),
        })?;
        tracing::info!(port, "port shared");
        self.s.push(ctx, None);
        ctx.changed();
        Ok(())
    }

    /// Stop sharing `port`: closes its LAN listener and ends every tunnel
    /// to it.
    pub fn unshare(&self, port: u16) -> Result<()> {
        let ctx = self.ctx()?;
        ctx.remove_shared_port(port)?;
        self.s.stop_lan(port);
        let ended: Vec<(DeviceId, u64)> = {
            let mut st = self.s.lock();
            let ended = st
                .host
                .iter()
                .filter(|h| h.port == port)
                .map(|h| (h.peer, h.activity))
                .collect();
            st.host.retain(|h| h.port != port);
            ended
        };
        for (peer, activity) in ended {
            if activity != 0 {
                ctx.stop(peer, activity);
            }
        }
        tracing::info!(port, "port no longer shared");
        self.s.push(ctx, None);
        ctx.changed();
        Ok(())
    }

    /// Turn Expose on LAN on or off for a shared port (R13.6). "Always on"
    /// needs `confirmed` (the UI's extra confirmation), checked here too.
    pub fn set_lan(&self, port: u16, mode: LanMode, confirmed: bool) -> Result<()> {
        let ctx = self.ctx()?;
        let ports = ctx.shared_ports()?;
        let Some(p) = ports.into_iter().find(|p| p.port == port) else {
            return Err(CoreError::NotFound);
        };
        let until = match mode {
            LanMode::Off => None,
            LanMode::OneHour => Some(now() + LAN_HOUR),
            LanMode::Always if confirmed => Some(0),
            LanMode::Always => {
                return Err(CoreError::InvalidInput(
                    "Always on needs a confirmation".into(),
                ));
            }
        };
        if until.is_some() {
            self.s.start_lan(ctx, port)?;
        } else {
            self.s.stop_lan(port);
        }
        ctx.save_shared_port(&SharedPort {
            lan_expose_until: until,
            ..p
        })?;
        ctx.changed();
        Ok(())
    }

    /// The ports this device shares.
    pub fn shared(&self) -> Vec<SharedPortView> {
        let Ok(ctx) = self.ctx() else {
            return Vec::new();
        };
        let st = self.s.lock();
        ctx.shared_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let (lan, lan_until) = match p.lan_expose_until {
                    None => (LanMode::Off, 0),
                    Some(0) => (LanMode::Always, 0),
                    Some(u) => (LanMode::OneHour, u),
                };
                let entry = st.lan.get(&p.port);
                SharedPortView {
                    port: p.port,
                    label: p.label,
                    lan,
                    lan_until,
                    lan_listening: entry.is_some(),
                    lan_addrs: entry.map(|e| e.addrs.clone()).unwrap_or_default(),
                    lan_connections: entry.map_or(0, |e| e.stats.conns.load(Ordering::Relaxed)),
                }
            })
            .collect()
    }

    /// Ports open on the LAN now, for the persistent indicator (R13.6).
    pub fn lan_exposed(&self) -> Vec<u16> {
        self.shared()
            .into_iter()
            .filter(|p| p.lan != LanMode::Off)
            .map(|p| p.port)
            .collect()
    }

    // -- viewer -----------------------------------------------------------------

    /// The ports `peer` shares with this device.
    pub fn remote_ports(&self, peer: DeviceId) -> Vec<RemotePort> {
        self.s.lock().remote.get(&peer).cloned().unwrap_or_default()
    }

    /// Start using `peer`'s port: asks the host (Always Ask prompts there),
    /// then listens on `127.0.0.1:port` or the next free port. Returns the
    /// tunnel id at once; [`Tunnel::tunnels`] shows the local port when it
    /// is open. Opening a port that is already open returns its id.
    pub fn open(&self, peer: DeviceId, port: u16) -> Result<u64> {
        self.s.viewer_open(peer, port)
    }

    /// Stop a tunnel, on the viewer or on the host. Both devices close it.
    pub fn stop(&self, id: u64) {
        let Ok(ctx) = self.ctx() else { return };
        let target = {
            let mut st = self.s.lock();
            if let Some(pos) = st.viewer.iter().position(|v| v.id == id) {
                let v = st.viewer.remove(pos);
                v.activity.map(|a| (v.peer, a))
            } else if let Some(pos) = st.host.iter().position(|h| h.id == id) {
                let h = st.host.remove(pos);
                Some((h.peer, h.activity)).filter(|(_, a)| *a != 0)
            } else {
                None
            }
        };
        if let Some((peer, activity)) = target {
            ctx.stop(peer, activity);
        }
        ctx.changed();
    }

    /// Every tunnel: outgoing ones first, then incoming ones.
    pub fn tunnels(&self) -> Vec<TunnelView> {
        let Ok(ctx) = self.ctx() else {
            return Vec::new();
        };
        let st = self.s.lock();
        let view = |id, peer, incoming, port, local_port, status, stats: &Stats| TunnelView {
            id,
            peer,
            peer_name: ctx.peer_name(peer),
            incoming,
            remote_port: port,
            local_port,
            status,
            connections: stats.conns.load(Ordering::Relaxed),
            bytes_up: stats.up.load(Ordering::Relaxed),
            bytes_down: stats.down.load(Ordering::Relaxed),
        };
        st.viewer
            .iter()
            .map(|v| {
                view(
                    v.id,
                    v.peer,
                    false,
                    v.port,
                    v.local_port,
                    v.status.clone(),
                    &v.stats,
                )
            })
            .chain(st.host.iter().map(|h| {
                view(
                    h.id,
                    h.peer,
                    true,
                    h.port,
                    None,
                    TunnelStatus::Open,
                    &h.stats,
                )
            }))
            .collect()
    }
}

impl Service for Tunnel {
    fn id(&self) -> ServiceId {
        ServiceId::TUNNEL
    }

    fn capability(&self) -> Capability {
        Capability::Tunnel
    }

    fn start(&self, ctx: ServiceCtx) {
        let _ = self.s.ctx.set(ctx.clone());
        // Restore exposures: "Always on" and unexpired "1 hour" (R13.6).
        // `tick` drops expired ones; binding happens on the runtime.
        let s = self.s.clone();
        ctx.clone().spawn(async move {
            let t = now();
            for p in ctx.shared_ports().unwrap_or_default() {
                if matches!(p.lan_expose_until, Some(u) if u == 0 || u > t)
                    && let Err(e) = s.start_lan(&ctx, p.port)
                {
                    tracing::warn!(port = p.port, error = %e, "LAN exposure not restored yet");
                }
            }
            let mut every = tokio::time::interval(TICK);
            loop {
                every.tick().await;
                if ctx.this_device().is_err() {
                    break;
                }
                s.tick(&ctx);
            }
        });
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> Result<()> {
        // Only the host's list comes as a message, and it is for drawing.
        let Access::Controller { .. } = msg.access else {
            return Ok(());
        };
        let Some(list) = wire::decode_ports(&msg.body) else {
            return Err(CoreError::Protocol("bad SharedPorts".into()));
        };
        let list = list
            .into_iter()
            .map(|(port, label)| RemotePort { port, label })
            .collect();
        self.s.lock().remote.insert(msg.peer, list);
        ctx.changed();
        Ok(())
    }

    fn on_stream(&self, ctx: &ServiceCtx, stream: Stream) {
        match stream.access.clone() {
            Access::Host(grant) => self.s.host_stream(ctx, &grant, stream),
            Access::Controller { .. } => {
                // A host never opens tunnel streams to a viewer.
                let mut stream = stream;
                if let Some(s) = stream.send.as_mut() {
                    let _ = s.reset(stream_code::UNSUPPORTED.into());
                }
                let _ = stream.recv.stop(stream_code::UNSUPPORTED.into());
            }
        }
    }

    fn on_activity_end(&self, ctx: &ServiceCtx, peer: DeviceId, activity: u64) {
        {
            let mut st = self.s.lock();
            // Dropping the entries drops their stop senders.
            st.viewer
                .retain(|v| !(v.peer == peer && v.activity == Some(activity)));
            st.host
                .retain(|h| !(h.peer == peer && h.activity == activity));
        }
        ctx.changed();
    }

    fn on_peer(&self, ctx: &ServiceCtx, peer: DeviceId, connected: bool) {
        if connected {
            self.s.push(ctx, Some(peer));
        } else {
            let mut st = self.s.lock();
            st.remote.remove(&peer);
            st.viewer.retain(|v| v.peer != peer);
            st.host.retain(|h| h.peer != peer);
        }
        ctx.changed();
    }
}

pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Tunnel::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_local_takes_the_next_free_port() {
        let first = bind_local(0).unwrap();
        let p = first.local_addr().unwrap().port();
        // `p` is taken now; asking for it gives another loopback port.
        let second = bind_local(p).unwrap();
        let a = second.local_addr().unwrap();
        assert!(a.ip().is_loopback());
        assert_ne!(a.port(), p);
    }
}
