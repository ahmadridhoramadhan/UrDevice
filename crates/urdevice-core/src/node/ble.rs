//! The Bluetooth LE control channel (design §8.2, R6.2, R6.3).
//!
//! When a desktop and an Android phone have no IP path, the desktop (GATT
//! central) connects to the phone's GATT server and runs a session over it.
//! Every byte is a Noise IK message with the BLE static keys pinned over the
//! QUIC session, so the channel is end to end encrypted and authenticated by
//! the paired identities; Bluetooth pairing plays no part. Inside, the
//! session speaks the same control protocol as QUIC (Hello, Welcome,
//! permissions, activities, feature messages), but only the features in
//! [`bluetooth::allowed_service`] pass, and streams are refused
//! ([`CoreError::FallbackPath`]).
//!
//! Plaintext of one Noise transport message: a type byte, then the data.
//! Control messages are cut into [`CHUNK`]-byte pieces so pings and input
//! events are not stuck behind a large clipboard item.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::time::timeout;
use urdevice_proto::{PROTO_VERSION, ServiceId, control};

use super::link::{BleConn, Link, PathKind};
use super::net::{self, Flow};
use super::{Inner, Node, feature, lock};
use crate::identity::DeviceId;
use crate::services::bluetooth::frame::{Reassembler, Splitter};
use crate::services::bluetooth::noise::{self, Initiator, Responder, Transport};
use crate::services::bluetooth::{self, BleCentral, BlePeripheral, RadioLink};
use crate::transport::close;
use crate::{CoreError, Result};

const T_CONTROL_MORE: u8 = 1;
const T_CONTROL_END: u8 = 2;
const T_DATAGRAM: u8 = 3;
const T_PING: u8 = 4;
const T_PONG: u8 = 5;

/// Control bytes per Noise message.
const CHUNK: usize = 1024;
/// Largest control message over the channel: a 64 KB clipboard item and its
/// envelope (R6.2).
const MAX_CONTROL: usize = 80 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Keep-alive; its round trip is the RTT on the device view (design §6.4).
const PING_EVERY: Duration = Duration::from_secs(2);
/// No packet for this long: the link is gone.
const DEAD_AFTER: Duration = Duration::from_secs(8);
/// How often the node checks whether a peer needs the channel.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// How long the central scans per attempt.
const SCAN: Duration = Duration::from_secs(8);
const MIN_RETRY: Duration = Duration::from_secs(4);
const MAX_RETRY: Duration = Duration::from_secs(60);
/// Most links a phone serves at once; more are refused before any crypto.
const MAX_INCOMING: usize = 4;

/// The radio adapters and the channel's own bookkeeping.
#[derive(Default)]
pub(super) struct Radios {
    central: RwLock<Option<Arc<dyn BleCentral>>>,
    peripheral: RwLock<Option<Arc<dyn BlePeripheral>>>,
    advertising: Mutex<Option<bool>>,
    busy: AtomicBool,
    incoming: AtomicUsize,
    retry: Mutex<HashMap<DeviceId, (Instant, Duration)>>,
    /// Radio address that answered for a peer, so another phone is skipped.
    addrs: Mutex<HashMap<DeviceId, String>>,
}

impl Node {
    /// Give the node the desktop's GATT central (`btleplug`). Without one,
    /// the desktop never uses the Bluetooth LE channel.
    pub fn ble_set_central(&self, central: Arc<dyn BleCentral>) {
        match self.inner.ble.central.write() {
            Ok(mut g) => *g = Some(central),
            Err(p) => *p.into_inner() = Some(central),
        }
    }

    /// Give the node the phone's GATT server. The node switches its
    /// advertising on and off.
    pub fn ble_set_peripheral(&self, peripheral: Arc<dyn BlePeripheral>) {
        *lock(&self.inner.ble.advertising) = None;
        match self.inner.ble.peripheral.write() {
            Ok(mut g) => *g = Some(peripheral),
            Err(p) => *p.into_inner() = Some(peripheral),
        }
    }

    /// A central connected to this device's GATT server. The node runs the
    /// responder side of the handshake and, if the central is a paired
    /// device, a session.
    pub fn ble_incoming(&self, link: RadioLink) {
        let inner = self.inner.clone();
        if inner.ble.incoming.fetch_add(1, Ordering::AcqRel) >= MAX_INCOMING {
            inner.ble.incoming.fetch_sub(1, Ordering::AcqRel);
            tracing::debug!("too many Bluetooth LE links; refusing one");
            return;
        }
        self.inner.rt.spawn(async move {
            if let Err(e) = responder(&inner, link).await {
                tracing::debug!(error = %e, "Bluetooth LE link ended");
            }
            inner.ble.incoming.fetch_sub(1, Ordering::AcqRel);
        });
    }

    /// Try the Bluetooth LE channel to `device` now, whatever the other
    /// paths do. For tests and diagnostics; the node starts the channel on
    /// its own when it is needed.
    #[doc(hidden)]
    pub fn ble_connect_now(&self, device: &str) -> Result<()> {
        let id = DeviceId::from_hex(device).ok_or(CoreError::NotFound)?;
        let central = read_slot(&self.inner.ble.central).ok_or(CoreError::NotFound)?;
        let key = lock(&self.inner.store)
            .ble_key(id)?
            .ok_or(CoreError::NotFound)?;
        let inner = self.inner.clone();
        self.inner.rt.spawn(async move {
            if let Err(e) = initiator(&inner, central.as_ref(), id, key).await {
                tracing::debug!(peer = %id.short_hex(), error = %e, "Bluetooth LE attempt ended");
            }
        });
        Ok(())
    }
}

fn read_slot<T: ?Sized>(s: &RwLock<Option<Arc<T>>>) -> Option<Arc<T>> {
    match s.read() {
        Ok(g) => g.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

impl Inner {
    /// This device's BLE static public key, for Hello and Welcome.
    pub(super) fn ble_public(&self) -> Vec<u8> {
        self.identity()
            .ble_static()
            .map(|k| k.public().to_vec())
            .unwrap_or_default()
    }

    /// Pin a paired peer's BLE static key, received over its authenticated
    /// QUIC session (design §4.1). The first key stays: it is derived from
    /// the peer's identity, so a different one is never legitimate.
    pub(super) fn pin_ble_key(&self, peer: DeviceId, key: &[u8]) {
        let Ok(key) = <[u8; noise::KEY_LEN]>::try_from(key) else {
            return;
        };
        let store = lock(&self.store);
        match store.ble_key(peer) {
            Ok(Some(k)) if k == key => {}
            Ok(Some(_)) => {
                tracing::warn!(peer = %peer.short_hex(), "peer sent a different BLE key; keeping the pinned one");
            }
            Ok(None) => {
                if let Err(e) = store.set_ble_key(peer, &key) {
                    tracing::warn!(peer = %peer.short_hex(), error = %e, "could not pin the BLE key");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not read the BLE key"),
        }
    }

    fn peer_by_ble_key(&self, key: &[u8; noise::KEY_LEN]) -> Option<DeviceId> {
        let ids: Vec<DeviceId> = lock(&self.state).paired.iter().map(|d| d.id).collect();
        let store = lock(&self.store);
        ids.into_iter()
            .find(|id| store.ble_key(*id).ok().flatten().as_ref() == Some(key))
    }
}

// ---------------------------------------------------------------------------
// When to use the channel
// ---------------------------------------------------------------------------

/// Paired devices that need the channel now: the other side has an Android
/// role (phone for a desktop, desktop for a phone), auto-connect is on, and
/// there is no working session and no recent LAN announcement.
fn wanted(inner: &Inner, now: Instant) -> Vec<DeviceId> {
    let android_here = inner.platform == "android";
    let st = lock(&inner.state);
    st.paired
        .iter()
        .filter(|d| d.auto_connect)
        .filter(|d| (d.platform == "android") != android_here)
        .filter(|d| !st.connecting.contains(&d.id))
        .filter(|d| st.paused.get(&d.id).is_none_or(|t| *t <= now))
        .filter(|d| match st.sessions.get(&d.id) {
            Some(s) => !s.link.is_ble() && s.stalled,
            None => st
                .nearby
                .get(&d.id.short())
                .is_none_or(|n| now.duration_since(n.last_seen) >= super::paths::FRESH),
        })
        .map(|d| d.id)
        .collect()
}

pub(super) async fn ble_loop(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(CHECK_EVERY);
    loop {
        tick.tick().await;
        let now = Instant::now();
        let peers: Vec<(DeviceId, [u8; noise::KEY_LEN])> = {
            let wanted = wanted(&inner, now);
            let store = lock(&inner.store);
            wanted
                .into_iter()
                .filter_map(|id| store.ble_key(id).ok().flatten().map(|k| (id, k)))
                .collect()
        };
        if inner.platform == "android" {
            let on = !peers.is_empty();
            if let Some(p) = read_slot(&inner.ble.peripheral) {
                let mut adv = lock(&inner.ble.advertising);
                if *adv != Some(on) {
                    *adv = Some(on);
                    tracing::info!(on, "Bluetooth LE advertising");
                    p.set_advertising(on);
                }
            }
            continue;
        }
        let Some(central) = read_slot(&inner.ble.central) else {
            continue;
        };
        let next = {
            let retry = lock(&inner.ble.retry);
            peers
                .into_iter()
                .find(|(id, _)| retry.get(id).is_none_or(|(at, _)| *at <= now))
        };
        let Some((peer, key)) = next else { continue };
        if inner.ble.busy.swap(true, Ordering::AcqRel) {
            continue;
        }
        let inner2 = inner.clone();
        tokio::spawn(async move {
            let r = initiator(&inner2, central.as_ref(), peer, key).await;
            {
                let mut retry = lock(&inner2.ble.retry);
                let delay = retry
                    .get(&peer)
                    .map_or(MIN_RETRY, |(_, d)| (*d * 2).min(MAX_RETRY));
                retry.insert(peer, (Instant::now() + delay, delay));
            }
            inner2.ble.busy.store(false, Ordering::Release);
            if let Err(e) = r {
                tracing::debug!(peer = %peer.short_hex(), error = %e, "Bluetooth LE attempt ended");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// The wire: packets, Noise, and message types
// ---------------------------------------------------------------------------

enum Event {
    Control(control::Msg),
    Datagram(Vec<u8>),
    Ping([u8; 8]),
    Pong([u8; 8]),
}

struct Wire {
    tx: mpsc::Sender<Vec<u8>>,
    packet: usize,
    split: Splitter,
    join: Reassembler,
    noise: Option<Transport>,
    control: Vec<u8>,
}

fn lost() -> CoreError {
    CoreError::Network("Bluetooth LE link lost".into())
}

impl Wire {
    fn new(tx: mpsc::Sender<Vec<u8>>, packet: usize) -> Self {
        Self {
            tx,
            packet,
            split: Splitter::default(),
            join: Reassembler::default(),
            noise: None,
            control: Vec::new(),
        }
    }

    async fn send_raw(&mut self, msg: &[u8]) -> Result<()> {
        let packets = self
            .split
            .split(msg, self.packet)
            .map_err(|e| CoreError::Protocol(e.to_string()))?;
        for p in packets {
            self.tx.send(p).await.map_err(|_| lost())?;
        }
        Ok(())
    }

    async fn send_sealed(&mut self, kind: u8, data: &[u8]) -> Result<()> {
        let noise = self.noise.as_mut().ok_or(CoreError::Stopped)?;
        let mut plain = Vec::with_capacity(1 + data.len());
        plain.push(kind);
        plain.extend_from_slice(data);
        let sealed = noise.encrypt(&plain)?;
        self.send_raw(&sealed).await
    }

    async fn send_control(&mut self, msg: control::Msg) -> Result<()> {
        let bytes = prost::Message::encode_to_vec(&urdevice_proto::Control::from(msg));
        if bytes.len() > MAX_CONTROL {
            return Err(CoreError::FallbackPath);
        }
        let mut chunks = bytes.chunks(CHUNK).peekable();
        if chunks.peek().is_none() {
            return self.send_sealed(T_CONTROL_END, &[]).await;
        }
        while let Some(c) = chunks.next() {
            let kind = if chunks.peek().is_none() {
                T_CONTROL_END
            } else {
                T_CONTROL_MORE
            };
            self.send_sealed(kind, c).await?;
        }
        Ok(())
    }

    /// Feed one packet. Returns a whole Noise message when one completes.
    fn join(&mut self, packet: &[u8]) -> Result<Option<Vec<u8>>> {
        self.join
            .push(packet)
            .map_err(|e| CoreError::Protocol(format!("bluetooth: {e}")))
    }

    /// Decrypt one Noise message and turn it into an event.
    fn open(&mut self, sealed: &[u8]) -> Result<Option<Event>> {
        let noise = self.noise.as_mut().ok_or(CoreError::Stopped)?;
        let plain = noise.decrypt(sealed)?;
        let Some((&kind, data)) = plain.split_first() else {
            return Err(CoreError::Protocol("empty bluetooth message".into()));
        };
        let nonce =
            || <[u8; 8]>::try_from(data).map_err(|_| CoreError::Protocol("bad ping".into()));
        match kind {
            T_CONTROL_MORE | T_CONTROL_END => {
                if self.control.len() + data.len() > MAX_CONTROL {
                    return Err(CoreError::Protocol(
                        "bluetooth control message too large".into(),
                    ));
                }
                self.control.extend_from_slice(data);
                if kind == T_CONTROL_MORE {
                    return Ok(None);
                }
                let body = std::mem::take(&mut self.control);
                Ok(Some(Event::Control(urdevice_proto::decode_control(&body)?)))
            }
            T_DATAGRAM if data.len() <= bluetooth::MAX_DATAGRAM => {
                Ok(Some(Event::Datagram(data.to_vec())))
            }
            T_PING => Ok(Some(Event::Ping(nonce()?))),
            T_PONG => Ok(Some(Event::Pong(nonce()?))),
            _ => Err(CoreError::Protocol("unknown bluetooth message".into())),
        }
    }
}

async fn recv_raw(rx: &mut mpsc::Receiver<Vec<u8>>, wire: &mut Wire) -> Result<Vec<u8>> {
    loop {
        let p = rx.recv().await.ok_or_else(lost)?;
        if let Some(m) = wire.join(&p)? {
            return Ok(m);
        }
    }
}

async fn recv_control(rx: &mut mpsc::Receiver<Vec<u8>>, wire: &mut Wire) -> Result<control::Msg> {
    loop {
        let m = recv_raw(rx, wire).await?;
        if let Some(Event::Control(c)) = wire.open(&m)? {
            return Ok(c);
        }
    }
}

// ---------------------------------------------------------------------------
// Handshakes
// ---------------------------------------------------------------------------

/// Desktop side: connect to the phone and run a session.
async fn initiator(
    inner: &Arc<Inner>,
    central: &dyn BleCentral,
    peer: DeviceId,
    key: [u8; noise::KEY_LEN],
) -> Result<()> {
    let skip: Vec<String> = lock(&inner.ble.addrs)
        .iter()
        .filter(|(p, _)| **p != peer)
        .map(|(_, a)| a.clone())
        .collect();
    let (addr, radio) = central.connect(SCAN, skip).await?;
    let RadioLink { tx, mut rx, packet } = radio;
    let mut wire = Wire::new(tx, packet);
    let local = inner.identity().ble_static()?;
    let w = timeout(HANDSHAKE_TIMEOUT, async {
        let (init, m1) = Initiator::start(&local, &key)?;
        wire.send_raw(&m1).await?;
        let m2 = recv_raw(&mut rx, &mut wire).await?;
        wire.noise = Some(init.finish(&m2)?);
        wire.send_control(net::hello(inner)).await?;
        recv_control(&mut rx, &mut wire).await
    })
    .await
    .map_err(|_| CoreError::Network("Bluetooth LE handshake timed out".into()))??;
    let control::Msg::Welcome(w) = w else {
        return Err(CoreError::Protocol("expected Welcome".into()));
    };
    if w.proto != PROTO_VERSION {
        return Err(CoreError::Protocol("unsupported version".into()));
    }
    net::check_caps(&w.capabilities)?;
    lock(&inner.ble.addrs).insert(peer, addr);
    lock(&inner.ble.retry).remove(&peer);
    inner.refresh_peer_info(peer, &w.name, &w.platform);
    run(inner, wire, rx, peer, true, &w.capabilities).await
}

/// Phone side: a central connected; find out who it is.
async fn responder(inner: &Arc<Inner>, radio: RadioLink) -> Result<()> {
    let RadioLink { tx, mut rx, packet } = radio;
    let mut wire = Wire::new(tx, packet);
    let local = inner.identity().ble_static()?;
    let (peer, h) = timeout(HANDSHAKE_TIMEOUT, async {
        let m1 = recv_raw(&mut rx, &mut wire).await?;
        let (resp, remote) = Responder::read(&local, &m1)?;
        // Only a paired device's pinned key gets an answer (R6.3).
        let peer = inner
            .peer_by_ble_key(&remote)
            .ok_or_else(|| CoreError::Protocol("unknown Bluetooth LE key".into()))?;
        let (m2, transport) = resp.accept()?;
        wire.send_raw(&m2).await?;
        wire.noise = Some(transport);
        let h = recv_control(&mut rx, &mut wire).await?;
        Ok::<_, CoreError>((peer, h))
    })
    .await
    .map_err(|_| CoreError::Network("Bluetooth LE handshake timed out".into()))??;
    let control::Msg::Hello(h) = h else {
        return Err(CoreError::Protocol("expected Hello".into()));
    };
    if !(h.proto_min..=h.proto_max).contains(&PROTO_VERSION) {
        return Err(CoreError::Protocol("unsupported version".into()));
    }
    net::check_caps(&h.capabilities)?;
    wire.send_control(net::welcome(inner)?).await?;
    inner.refresh_peer_info(peer, &h.name, &h.platform);
    run(inner, wire, rx, peer, false, &h.capabilities).await
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

async fn run(
    inner: &Arc<Inner>,
    mut wire: Wire,
    mut rx: mpsc::Receiver<Vec<u8>>,
    peer: DeviceId,
    initiated_here: bool,
    caps: &[String],
) -> Result<()> {
    let (tx, mut ctl_rx) = mpsc::unbounded_channel::<control::Msg>();
    let (dg_tx, mut dg_rx) = mpsc::channel::<Vec<u8>>(32);
    let (conn, mut closed) = BleConn::new(dg_tx);
    let link = Link::Ble(conn.clone());
    if !net::open_session(inner, &link, PathKind::Ble, &tx, peer, initiated_here, caps)? {
        return Ok(());
    }
    let mut tick = tokio::time::interval(PING_EVERY);
    let mut last_rx = Instant::now();
    let mut ping: Option<([u8; 8], Instant)> = None;
    let result: Result<()> = loop {
        tokio::select! {
            p = rx.recv() => {
                let Some(p) = p else { break Err(lost()) };
                last_rx = Instant::now();
                let event = match wire.join(&p) {
                    Ok(Some(m)) => wire.open(&m),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                };
                let event = match event {
                    Ok(Some(e)) => e,
                    Ok(None) => continue,
                    Err(e) => {
                        net::record_violation(inner, peer);
                        break Err(e);
                    }
                };
                match event {
                    Event::Control(m) => {
                        if let control::Msg::Feature(f) = &m {
                            let allowed = ServiceId::from_wire(f.service)
                                .is_some_and(|id| bluetooth::allowed_message(id, &f.body));
                            if !allowed {
                                if f.direction() == urdevice_proto::Direction::ToHost {
                                    inner.refuse(peer, f.service, f.activity, "fallback_path");
                                }
                                continue;
                            }
                        }
                        match net::handle_msg(inner, &link, peer, m) {
                            Ok(Flow::Continue) => {}
                            Ok(Flow::Close) => break Ok(()),
                            Err(e) => {
                                net::record_violation(inner, peer);
                                break Err(e);
                            }
                        }
                    }
                    Event::Datagram(d) => {
                        let ok = urdevice_proto::decode_datagram(&d)
                            .is_ok_and(|(h, _)| bluetooth::allowed_service(h.service));
                        if ok {
                            feature::incoming_datagram(inner, peer, &d);
                        }
                    }
                    Event::Ping(n) => {
                        if let Err(e) = wire.send_sealed(T_PONG, &n).await {
                            break Err(e);
                        }
                    }
                    Event::Pong(n) => {
                        if let Some((sent, at)) = ping
                            && sent == n
                        {
                            conn.set_rtt(u32::try_from(at.elapsed().as_millis()).unwrap_or(u32::MAX));
                            ping = None;
                        }
                    }
                }
            }
            m = ctl_rx.recv() => {
                let Some(m) = m else { break Ok(()) };
                match wire.send_control(m).await {
                    Ok(()) => {}
                    Err(CoreError::FallbackPath) => {
                        tracing::debug!(peer = %peer.short_hex(), "control message too large for Bluetooth LE; dropped");
                    }
                    Err(e) => break Err(e),
                }
            }
            d = dg_rx.recv() => {
                if let Some(d) = d
                    && let Err(e) = wire.send_sealed(T_DATAGRAM, &d).await
                {
                    break Err(e);
                }
            }
            _ = tick.tick() => {
                if last_rx.elapsed() > DEAD_AFTER {
                    break Err(CoreError::Network("Bluetooth LE link timed out".into()));
                }
                let n = crate::crypto::random_u64().unwrap_or(0).to_be_bytes();
                ping = Some((n, Instant::now()));
                if let Err(e) = wire.send_sealed(T_PING, &n).await {
                    break Err(e);
                }
            }
            r = closed.changed() => {
                let _ = r;
                break Ok(());
            }
        }
    };
    net::close_session(inner, &link, peer);
    link.close(close::NORMAL, b"");
    result
}
