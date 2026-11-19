//! Network flows of the node: accept, connect, pairing, and the session loop
//! (design §6, §7).

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::timeout;
use urdevice_proto::{PROTO_VERSION, control};

use super::{
    Activity, Backoff, CodePairing, GrantEntry, Inner, MAX_BACKOFF, NodeEvent, PermissionView,
    Prompt, PromptView, Session, feature, lock, unix_now,
};
use crate::discovery::Seen;
use crate::identity::DeviceId;
use crate::pairing::{self, EXPORTER_PAIR, EXPORTER_SAS, ScannedInvite, TOKEN_TTL};
use crate::permission::{Capability, Mode};
use crate::transport::{self, Expect, SERVER_NAME, close};
use crate::{CoreError, Result};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const DIAL_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_CAPS: usize = 64;
const MAX_CAP_LEN: usize = 32;
const VIOLATION_LIMIT: usize = 3;
const VIOLATION_WINDOW: Duration = Duration::from_secs(600);

type ReadFut = Pin<Box<dyn Future<Output = (quinn::RecvStream, Result<control::Msg>)> + Send>>;

async fn read_owned(mut recv: quinn::RecvStream) -> (quinn::RecvStream, Result<control::Msg>) {
    let r = read_msg(&mut recv).await;
    (recv, r)
}

async fn read_msg(recv: &mut quinn::RecvStream) -> Result<control::Msg> {
    let mut header = [0u8; 4];
    recv.read_exact(&mut header).await?;
    let len = urdevice_proto::frame_len(header)?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(urdevice_proto::decode_control(&body)?)
}

async fn write_msg(send: &mut quinn::SendStream, msg: control::Msg) -> Result<()> {
    let frame = urdevice_proto::encode_frame(&msg.into())?;
    send.write_all(&frame).await?;
    Ok(())
}

/// Cancel-safe reader for the control stream: a pending read survives a
/// `select!` branch that fires first.
struct Reader {
    recv: Option<quinn::RecvStream>,
    fut: Option<ReadFut>,
}

impl Reader {
    fn new(recv: quinn::RecvStream) -> Self {
        Self {
            recv: Some(recv),
            fut: None,
        }
    }

    async fn next(&mut self) -> Result<control::Msg> {
        if self.fut.is_none() {
            let recv = self.recv.take().ok_or(CoreError::Stopped)?;
            self.fut = Some(Box::pin(read_owned(recv)));
        }
        let Some(fut) = self.fut.as_mut() else {
            return Err(CoreError::Stopped);
        };
        let (recv, res) = fut.await;
        self.fut = None;
        self.recv = Some(recv);
        res
    }

    async fn next_timeout(&mut self, d: Duration) -> Result<control::Msg> {
        timeout(d, self.next())
            .await
            .map_err(|_| CoreError::Network("timed out".into()))?
    }
}

fn close_code(e: &quinn::ConnectionError) -> Option<u32> {
    match e {
        quinn::ConnectionError::ApplicationClosed(c) => {
            u32::try_from(c.error_code.into_inner()).ok()
        }
        _ => None,
    }
}

fn hello(inner: &Inner) -> control::Msg {
    let st = lock(&inner.state);
    control::Msg::Hello(urdevice_proto::Hello {
        proto_min: PROTO_VERSION,
        proto_max: PROTO_VERSION,
        app_version: crate::core_version().into(),
        capabilities: local_caps(&inner.platform),
        name: st.name.clone(),
        platform: inner.platform.clone(),
    })
}

fn local_caps(platform: &str) -> Vec<String> {
    Capability::for_platform(platform)
        .into_iter()
        .map(|c| c.as_str().to_owned())
        .collect()
}

fn check_caps(caps: &[String]) -> Result<()> {
    if caps.len() > MAX_CAPS || caps.iter().any(|c| c.len() > MAX_CAP_LEN) {
        return Err(CoreError::Protocol("capability list too large".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dialing
// ---------------------------------------------------------------------------

/// Try every address at once; the first finished handshake wins.
async fn dial(
    inner: &Arc<Inner>,
    addrs: &[SocketAddr],
    expect: Expect,
) -> Result<quinn::Connection> {
    let cfg = transport::client_config(&inner.identity(), expect)?;
    let mut set = JoinSet::new();
    for addr in addrs.iter().copied().take(8) {
        let ep = inner.endpoint.clone();
        let cfg = cfg.clone();
        set.spawn(async move {
            let connecting = ep.connect_with(cfg, addr, SERVER_NAME)?;
            timeout(DIAL_TIMEOUT, connecting)
                .await
                .map_err(|_| CoreError::Network("timed out".into()))?
                .map_err(CoreError::from)
        });
    }
    let mut last = CoreError::Network("no address".into());
    while let Some(r) = set.join_next().await {
        match r {
            Ok(Ok(conn)) => return Ok(conn),
            Ok(Err(e)) => last = e,
            Err(_) => {}
        }
    }
    Err(last)
}

// ---------------------------------------------------------------------------
// Accept side
// ---------------------------------------------------------------------------

pub(super) async fn accept_loop(inner: Arc<Inner>) {
    while let Some(incoming) = inner.endpoint.accept().await {
        let inner = inner.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_incoming(inner, incoming).await {
                tracing::debug!(error = %e, "incoming connection ended");
            }
        });
    }
}

async fn handle_incoming(inner: Arc<Inner>, incoming: quinn::Incoming) -> Result<()> {
    let conn = timeout(HANDSHAKE_TIMEOUT, incoming)
        .await
        .map_err(|_| CoreError::Network("handshake timed out".into()))??;
    let peer = transport::peer_id(&conn)?;
    let (send, recv) = timeout(HANDSHAKE_TIMEOUT, conn.accept_bi())
        .await
        .map_err(|_| CoreError::Network("no control stream".into()))??;
    let mut reader = Reader::new(recv);
    let first = reader.next_timeout(HANDSHAKE_TIMEOUT).await?;
    match first {
        control::Msg::Hello(h) if inner.policy.is_paired(&peer) => {
            serve_session(inner, conn, send, reader, peer, h).await
        }
        control::Msg::PairProof(p) => qr_host(inner, conn, send, reader, peer, p).await,
        control::Msg::PairStart(p) => {
            code_host(inner, conn, send, reader, peer, p.name, p.platform).await
        }
        _ => {
            conn.close(close::PROTOCOL.into(), b"unexpected");
            Err(CoreError::Protocol("unexpected first message".into()))
        }
    }
}

async fn serve_session(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    reader: Reader,
    peer: DeviceId,
    h: urdevice_proto::Hello,
) -> Result<()> {
    let name = pairing::clean_name(&h.name);
    if !(h.proto_min..=h.proto_max).contains(&PROTO_VERSION) {
        conn.close(close::VERSION.into(), b"version");
        inner.emit(NodeEvent::Notice {
            code: "update_required".into(),
            device_name: name,
        });
        return Err(CoreError::Protocol("unsupported version".into()));
    }
    check_caps(&h.capabilities)?;
    let welcome = {
        let st = lock(&inner.state);
        urdevice_proto::Welcome {
            proto: PROTO_VERSION,
            capabilities: local_caps(&inner.platform),
            session_id: crate::crypto::random_bytes(16)?,
            name: st.name.clone(),
            platform: inner.platform.clone(),
        }
    };
    write_msg(&mut send, control::Msg::Welcome(welcome)).await?;
    inner.refresh_peer_info(peer, &h.name, &h.platform);
    run_session(inner, conn, send, reader, peer, false, &h.capabilities).await
}

/// Client side after the TLS handshake (and pairing, if any): Hello, Welcome.
async fn client_session(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut reader: Reader,
    peer: DeviceId,
) -> Result<()> {
    write_msg(&mut send, hello(&inner)).await?;
    let msg = match reader.next_timeout(HANDSHAKE_TIMEOUT).await {
        Ok(m) => m,
        Err(e) => {
            if conn.close_reason().as_ref().and_then(close_code) == Some(close::VERSION) {
                let name = lock(&inner.state).peer_name(peer);
                inner.emit(NodeEvent::Notice {
                    code: "update_required".into(),
                    device_name: name,
                });
            }
            return Err(e);
        }
    };
    let control::Msg::Welcome(w) = msg else {
        conn.close(close::PROTOCOL.into(), b"expected welcome");
        return Err(CoreError::Protocol("expected Welcome".into()));
    };
    if w.proto != PROTO_VERSION {
        conn.close(close::VERSION.into(), b"version");
        let name = pairing::clean_name(&w.name);
        inner.emit(NodeEvent::Notice {
            code: "update_required".into(),
            device_name: name,
        });
        return Err(CoreError::Protocol("unsupported version".into()));
    }
    check_caps(&w.capabilities)?;
    inner.refresh_peer_info(peer, &w.name, &w.platform);
    run_session(inner, conn, send, reader, peer, true, &w.capabilities).await
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

enum Flow {
    Continue,
    Close,
}

async fn run_session(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut reader: Reader,
    peer: DeviceId,
    initiated_here: bool,
    peer_caps: &[String],
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<control::Msg>();
    let my_id = inner.id();
    {
        let mut st = lock(&inner.state);
        if st.paired(peer).is_none() {
            conn.close(close::USER.into(), b"not paired");
            return Err(CoreError::NotFound);
        }
        if let Some(existing) = st.sessions.get(&peer) {
            // The connection started by the lower Device ID wins (design §6.1).
            let new_wins = initiated_here == (my_id < peer);
            if !new_wins {
                conn.close(close::DUPLICATE.into(), b"duplicate");
                return Ok(());
            }
            existing.conn.close(close::DUPLICATE.into(), b"duplicate");
        }
        st.sessions.insert(
            peer,
            Session {
                conn: conn.clone(),
                tx: tx.clone(),
                remote_perms: Vec::new(),
                battery: None,
                // Unknown strings come from newer versions; ignore them.
                caps: peer_caps.iter().filter_map(|c| c.parse().ok()).collect(),
            },
        );
        st.backoff.remove(&peer);
        let _ = tx.send(st.permission_state(&inner.platform));
        let _ = tx.send(st.device_status());
    }
    // The peer pressed Reconnect: resume automatic reconnection here too (design §6.5).
    if !initiated_here {
        let _ = inner.set_auto_connect(peer, true, None);
    }
    let _ = lock(&inner.store).touch(peer, unix_now(), &[conn.remote_address()]);
    let _ = inner.reload_paired();
    tracing::info!(peer = %peer.short_hex(), addr = %conn.remote_address(), "session open");
    inner.notify_peer(peer, true);
    inner.changed();

    // Streams and datagrams for the feature services (design §6.2).
    let traffic = tokio::spawn(feature::accept_traffic(inner.clone(), conn.clone(), peer));

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if write_msg(&mut send, msg).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });

    let result = loop {
        tokio::select! {
            msg = reader.next() => match msg {
                Ok(m) => match handle_msg(&inner, &conn, peer, m) {
                    Ok(Flow::Continue) => {}
                    Ok(Flow::Close) => break Ok(()),
                    Err(e) => {
                        conn.close(close::PROTOCOL.into(), b"protocol");
                        record_violation(&inner, peer);
                        break Err(e);
                    }
                },
                Err(e) => break Err(e),
            },
            reason = conn.closed() => {
                break match close_code(&reason) {
                    Some(close::DUPLICATE | close::USER | close::SHUTDOWN | close::NORMAL) => Ok(()),
                    _ => Err(CoreError::Network(reason.to_string())),
                };
            }
        }
    };
    writer.abort();
    traffic.abort();

    let still_ours = {
        let mut st = lock(&inner.state);
        let ours = st
            .sessions
            .get(&peer)
            .is_some_and(|s| s.conn.stable_id() == conn.stable_id());
        if ours {
            st.sessions.remove(&peer);
            st.backoff.insert(
                peer,
                Backoff {
                    next: Instant::now() + Duration::from_secs(1),
                    delay: Duration::from_secs(2),
                },
            );
            st.pending_requests.retain(|(p, _), _| *p != peer);
            // Dropping a waiter tells its service the peer went offline.
            st.waiters.retain(|(p, _), _| *p != peer);
        }
        ours
    };
    if still_ours {
        inner.stop_activities(|a| a.peer == peer, "stopped");
        inner.end_grants(|(p, _), _| *p == peer);
        inner.notify_peer(peer, false);
        let _ = lock(&inner.store).touch(peer, unix_now(), &[conn.remote_address()]);
        let _ = inner.reload_paired();
        tracing::info!(peer = %peer.short_hex(), "session closed");
        inner.changed();
    }
    conn.close(close::NORMAL.into(), b"");
    result
}

pub(super) fn record_violation(inner: &Inner, peer: DeviceId) {
    let mut st = lock(&inner.state);
    let now = Instant::now();
    let v = st.violations.entry(peer).or_default();
    v.retain(|t| now.duration_since(*t) < VIOLATION_WINDOW);
    v.push_back(now);
    if v.len() >= VIOLATION_LIMIT {
        // Repeated malformed messages pause auto-connect to that peer (design §14).
        tracing::warn!(peer = %peer.short_hex(), "repeated protocol violations; pausing reconnects");
        st.paused.insert(peer, now + VIOLATION_WINDOW);
    }
}

fn handle_msg(
    inner: &Arc<Inner>,
    conn: &quinn::Connection,
    peer: DeviceId,
    msg: control::Msg,
) -> Result<Flow> {
    match msg {
        control::Msg::PermissionState(ps) => {
            if ps.entries.len() > MAX_CAPS {
                return Err(CoreError::Protocol("permission list too large".into()));
            }
            let perms: Vec<PermissionView> = ps
                .entries
                .iter()
                .filter_map(|e| {
                    let cap = e.capability.parse::<Capability>().ok()?;
                    Some(PermissionView {
                        capability: cap,
                        mode: Mode::from_wire(e.mode),
                    })
                })
                .collect();
            if let Some(s) = lock(&inner.state).sessions.get_mut(&peer) {
                s.remote_perms = perms;
            }
            inner.changed();
        }
        control::Msg::DeviceStatus(ds) => {
            let battery = ds
                .battery_pct
                .filter(|p| *p <= 100)
                .and_then(|p| u8::try_from(p).ok())
                .map(|p| (p, ds.charging.unwrap_or(false)));
            if let Some(s) = lock(&inner.state).sessions.get_mut(&peer) {
                s.battery = battery;
            }
            inner.changed();
        }
        control::Msg::Disconnect(_) => {
            let name = lock(&inner.state).peer_name(peer);
            let _ = inner.set_auto_connect(peer, false, Some(name.clone()));
            inner.emit(NodeEvent::Notice {
                code: "disconnected_by".into(),
                device_name: name,
            });
            conn.close(close::USER.into(), b"disconnect");
            return Ok(Flow::Close);
        }
        control::Msg::Unpair(_) => {
            let name = lock(&inner.state).peer_name(peer);
            let _ = inner.remove_pairing(peer);
            inner.emit(NodeEvent::Notice {
                code: "unpaired_by".into(),
                device_name: name,
            });
            conn.close(close::USER.into(), b"unpair");
            return Ok(Flow::Close);
        }
        control::Msg::ActivityRequest(r) => {
            tokio::spawn(handle_request(inner.clone(), peer, r));
        }
        control::Msg::ActivityReply(r) => {
            let reason: String = r.reason.chars().take(16).collect();
            let (cap, name, started, waiter, evicted) = {
                let mut st = lock(&inner.state);
                let Some(cap) = st.pending_requests.remove(&(peer, r.request_id)) else {
                    return Ok(Flow::Continue);
                };
                let name = st.peer_name(peer);
                let started = r.granted && cap.is_watched();
                let waiter = st.waiters.remove(&(peer, r.request_id));
                let mut evicted = None;
                if r.granted && !started && waiter.is_some() {
                    // A service asked: keep the approval so its traffic can name it.
                    evicted = feature::add_grant(
                        &mut st,
                        peer,
                        r.request_id,
                        GrantEntry {
                            cap,
                            incoming: false,
                        },
                    );
                }
                if started {
                    let id = st.next_id();
                    st.activities.push(Activity {
                        id,
                        peer,
                        peer_name: name.clone(),
                        cap,
                        request_id: r.request_id,
                        incoming: false,
                        started_at: unix_now(),
                    });
                }
                (cap, name, started, waiter, evicted)
            };
            if let Some(e) = evicted {
                inner.notify_ended(&[e]);
            }
            match waiter {
                // The service shows the outcome itself.
                Some(w) => {
                    let _ = w.send(if r.granted { Ok(()) } else { Err(reason) });
                }
                None => inner.emit(NodeEvent::ActivityResult {
                    device_id: peer.hex(),
                    device_name: name,
                    capability: cap.as_str().into(),
                    granted: r.granted,
                    reason,
                }),
            }
            if started {
                inner.changed();
            }
        }
        control::Msg::ActivityStop(s) => {
            let mut stopped = None;
            {
                let mut st = lock(&inner.state);
                if let Some(pos) = st
                    .activities
                    .iter()
                    .position(|a| a.peer == peer && a.request_id == s.request_id)
                {
                    stopped = Some(st.activities.remove(pos));
                }
            }
            let grant = if stopped.is_none() {
                lock(&inner.state).grants.remove(&(peer, s.request_id))
            } else {
                None
            };
            if let Some(g) = grant {
                inner.notify_ended(&[(peer, g.cap, s.request_id)]);
            }
            if let Some(a) = stopped {
                inner.notify_ended(&[(peer, a.cap, a.request_id)]);
                if a.incoming {
                    let _ = lock(&inner.store).log_activity(
                        peer,
                        &a.peer_name,
                        a.cap.as_str(),
                        "stopped",
                        unix_now(),
                    );
                } else {
                    inner.emit(NodeEvent::Notice {
                        code: "stopped_by".into(),
                        device_name: a.peer_name,
                    });
                }
                inner.changed();
            }
        }
        control::Msg::Feature(f) => feature::on_feature(inner, peer, f)?,
        control::Msg::FeatureRefused(r) => feature::on_refused(inner, peer, r),
        _ => return Err(CoreError::Protocol("unexpected message in session".into())),
    }
    Ok(Flow::Continue)
}

/// The host side of the permission engine (design §7.4). Every request is
/// checked here, whatever the peer's copy of the settings says.
async fn handle_request(inner: Arc<Inner>, peer: DeviceId, r: urdevice_proto::ActivityRequest) {
    let cap = r
        .capability
        .parse::<Capability>()
        .ok()
        .filter(|c| c.offered_on(&inner.platform));
    let (mode, name) = {
        let st = lock(&inner.state);
        (cap.map(|c| st.mode(c)), st.peer_name(peer))
    };
    let (granted, reason) = match (cap, mode) {
        (Some(_), Some(Mode::Allow)) => (true, "allowed"),
        (Some(_), Some(Mode::Deny)) => (false, "denied"),
        (Some(c), Some(Mode::Ask)) => match ask(&inner, peer, &name, c).await {
            Some(true) => (true, "allowed"),
            Some(false) => (false, "declined"),
            None => (false, "timeout"),
        },
        _ => (false, "unsupported"),
    };
    if let Some(c) = cap {
        let _ = lock(&inner.store).log_activity(peer, &name, c.as_str(), reason, unix_now());
        tracing::info!(peer = %peer.short_hex(), cap = %c, reason, "request decided");
    }
    let mut st = lock(&inner.state);
    let Some(tx) = st.sessions.get(&peer).map(|s| s.tx.clone()) else {
        return;
    };
    let mut evicted = None;
    if granted && let Some(c) = cap {
        if c.is_watched() {
            let id = st.next_id();
            st.activities.push(Activity {
                id,
                peer,
                peer_name: name,
                cap: c,
                request_id: r.request_id,
                incoming: true,
                started_at: unix_now(),
            });
        } else {
            // No indicator, but feature traffic may name the approval.
            evicted = feature::add_grant(
                &mut st,
                peer,
                r.request_id,
                GrantEntry {
                    cap: c,
                    incoming: true,
                },
            );
        }
    }
    drop(st);
    if let Some(e) = evicted {
        inner.notify_ended(&[e]);
    }
    let _ = tx.send(control::Msg::ActivityReply(urdevice_proto::ActivityReply {
        request_id: r.request_id,
        granted,
        reason: if granted {
            String::new()
        } else {
            reason.into()
        },
    }));
    inner.changed();
}

/// Show the Always Ask prompt. `None` when it timed out.
async fn ask(inner: &Arc<Inner>, peer: DeviceId, name: &str, cap: Capability) -> Option<bool> {
    let (reply, rx) = oneshot::channel();
    let id = {
        let mut st = lock(&inner.state);
        let id = st.next_id();
        st.prompts.insert(id, Prompt { reply });
        id
    };
    inner.emit(NodeEvent::Prompt(PromptView {
        id,
        device_name: name.into(),
        device_id: peer.hex(),
        capability: cap.as_str().into(),
        timeout_secs: u32::try_from(inner.ask_timeout.as_secs()).unwrap_or(30),
    }));
    let answer = timeout(inner.ask_timeout, rx)
        .await
        .ok()
        .and_then(|r| r.ok());
    lock(&inner.state).prompts.remove(&id);
    inner.emit(NodeEvent::PromptClosed { id });
    answer
}

// ---------------------------------------------------------------------------
// QR pairing (design §7.1)
// ---------------------------------------------------------------------------

async fn qr_host(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut reader: Reader,
    peer: DeviceId,
    proof: urdevice_proto::PairProof,
) -> Result<()> {
    let name = pairing::clean_name(&proof.name);
    let reject = |reason: &'static str| {
        control::Msg::PairReject(urdevice_proto::PairReject {
            reason: reason.into(),
        })
    };
    if let Err(reason) = inner.pairing_allowed() {
        let _ = write_msg(&mut send, reject(reason)).await;
        finish_reject(&conn).await;
        return Err(CoreError::Pairing(reason.into()));
    }
    let Some(token) = inner.take_token() else {
        inner.pairing_failed(&name, "expired");
        let _ = write_msg(&mut send, reject("expired")).await;
        finish_reject(&conn).await;
        return Err(CoreError::Pairing("expired".into()));
    };
    let binding = transport::exporter(&conn, EXPORTER_PAIR, 32)?;
    if peer == inner.id()
        || !pairing::verify_pair_proof(&token, &binding, inner.id(), peer, &proof.hmac)
    {
        inner.pairing_failed(&name, "bad_proof");
        let _ = write_msg(&mut send, reject("bad_proof")).await;
        finish_reject(&conn).await;
        return Err(CoreError::Pairing("bad_proof".into()));
    }
    inner.store_pairing(peer, &proof.name, &proof.platform, conn.remote_address())?;
    let me = lock(&inner.state).name.clone();
    write_msg(
        &mut send,
        control::Msg::PairAccept(urdevice_proto::PairAccept {
            name: me,
            platform: inner.platform.clone(),
        }),
    )
    .await?;
    inner.emit(NodeEvent::PairingFinished {
        name,
        ok: true,
        reason: String::new(),
    });
    inner.changed();
    let msg = reader.next_timeout(HANDSHAKE_TIMEOUT).await?;
    let control::Msg::Hello(h) = msg else {
        return Err(CoreError::Protocol("expected Hello".into()));
    };
    serve_session(inner, conn, send, reader, peer, h).await
}

async fn finish_reject(conn: &quinn::Connection) {
    tokio::time::sleep(Duration::from_millis(200)).await;
    conn.close(close::PAIRING.into(), b"pairing");
}

pub(super) async fn qr_scan(inner: Arc<Inner>, invite: ScannedInvite) {
    let name = invite.name.clone();
    if let Err(e) = qr_scan_inner(inner.clone(), invite).await {
        let reason = match &e {
            CoreError::Pairing(r) => r.clone(),
            CoreError::Network(_) => "unreachable".into(),
            _ => "failed".into(),
        };
        tracing::info!(reason = %reason, "QR pairing failed");
        // Only report failures that happened before the pair was stored.
        if !matches!(e, CoreError::Stopped) {
            inner.emit(NodeEvent::PairingFinished {
                name,
                ok: false,
                reason,
            });
        }
    }
}

async fn qr_scan_inner(inner: Arc<Inner>, invite: ScannedInvite) -> Result<()> {
    let conn = dial(&inner, &invite.addrs, Expect::Device(invite.device_id)).await?;
    let (mut send, recv) = conn.open_bi().await?;
    let mut reader = Reader::new(recv);
    let binding = transport::exporter(&conn, EXPORTER_PAIR, 32)?;
    let hmac = pairing::pair_proof(&invite.token, &binding, invite.device_id, inner.id());
    let me = lock(&inner.state).name.clone();
    write_msg(
        &mut send,
        control::Msg::PairProof(urdevice_proto::PairProof {
            hmac: hmac.to_vec(),
            name: me,
            platform: inner.platform.clone(),
        }),
    )
    .await?;
    let reply = reader
        .next_timeout(HANDSHAKE_TIMEOUT)
        .await
        .map_err(|_| CoreError::Pairing("rejected".into()))?;
    match reply {
        control::Msg::PairAccept(a) => {
            inner.store_pairing(
                invite.device_id,
                &a.name,
                &a.platform,
                conn.remote_address(),
            )?;
            inner.emit(NodeEvent::PairingFinished {
                name: pairing::clean_name(&a.name),
                ok: true,
                reason: String::new(),
            });
            inner.changed();
            // Errors from here on are session errors, not pairing failures.
            let _ = client_session(inner, conn, send, reader, invite.device_id).await;
            Err(CoreError::Stopped)
        }
        control::Msg::PairReject(r) => {
            conn.close(close::PAIRING.into(), b"");
            Err(CoreError::Pairing(r.reason.chars().take(16).collect()))
        }
        _ => {
            conn.close(close::PROTOCOL.into(), b"");
            Err(CoreError::Pairing("rejected".into()))
        }
    }
}

// ---------------------------------------------------------------------------
// Code-confirmation pairing (design §7.2)
// ---------------------------------------------------------------------------

pub(super) async fn code_initiate(inner: Arc<Inner>, short: [u8; 8], addrs: Vec<SocketAddr>) {
    if let Err(e) = code_initiate_inner(inner.clone(), short, addrs).await
        && !matches!(e, CoreError::Stopped)
    {
        let reason = match &e {
            CoreError::Pairing(r) => r.clone(),
            _ => "unreachable".into(),
        };
        inner.emit(NodeEvent::PairingFinished {
            name: String::new(),
            ok: false,
            reason,
        });
    }
}

async fn code_initiate_inner(
    inner: Arc<Inner>,
    short: [u8; 8],
    addrs: Vec<SocketAddr>,
) -> Result<()> {
    let conn = dial(&inner, &addrs, Expect::AnyForCodePairing).await?;
    let peer = transport::peer_id(&conn)?;
    if peer.short() != short || peer == inner.id() {
        conn.close(close::PAIRING.into(), b"");
        return Err(CoreError::Pairing("rejected".into()));
    }
    let (mut send, recv) = conn.open_bi().await?;
    let mut reader = Reader::new(recv);
    let me = lock(&inner.state).name.clone();
    write_msg(
        &mut send,
        control::Msg::PairStart(urdevice_proto::PairStart {
            name: me,
            platform: inner.platform.clone(),
        }),
    )
    .await?;
    let reply = reader
        .next_timeout(HANDSHAKE_TIMEOUT)
        .await
        .map_err(|_| CoreError::Pairing("rejected".into()))?;
    let (name, platform) = match reply {
        control::Msg::PairReady(r) => (r.name, r.platform),
        control::Msg::PairReject(r) => {
            conn.close(close::PAIRING.into(), b"");
            return Err(CoreError::Pairing(r.reason.chars().take(16).collect()));
        }
        _ => return Err(CoreError::Pairing("rejected".into())),
    };
    code_confirm(inner, conn, send, reader, peer, name, platform, false).await
}

async fn code_host(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    reader: Reader,
    peer: DeviceId,
    name: String,
    platform: String,
) -> Result<()> {
    if let Err(reason) = inner.pairing_allowed() {
        let _ = write_msg(
            &mut send,
            control::Msg::PairReject(urdevice_proto::PairReject {
                reason: reason.into(),
            }),
        )
        .await;
        finish_reject(&conn).await;
        return Err(CoreError::Pairing(reason.into()));
    }
    let me = lock(&inner.state).name.clone();
    write_msg(
        &mut send,
        control::Msg::PairReady(urdevice_proto::PairReady {
            name: me,
            platform: inner.platform.clone(),
        }),
    )
    .await?;
    code_confirm(inner, conn, send, reader, peer, name, platform, true).await
}

#[allow(clippy::too_many_arguments)]
async fn code_confirm(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut reader: Reader,
    peer: DeviceId,
    name: String,
    platform: String,
    incoming: bool,
) -> Result<()> {
    let name = pairing::clean_name(&name);
    let platform = pairing::clean_platform(&platform);
    let code = pairing::sas_code(&transport::exporter(&conn, EXPORTER_SAS, 4)?)?;
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<bool>();
    {
        let mut st = lock(&inner.state);
        if let Some(old) = st.code_pairings.insert(
            peer,
            CodePairing {
                name: name.clone(),
                platform: platform.clone(),
                code,
                confirmed_here: false,
                incoming,
                cmd: cmd_tx,
            },
        ) {
            let _ = old.cmd.send(false);
        }
    }
    inner.changed();

    let deadline = tokio::time::sleep(TOKEN_TTL);
    tokio::pin!(deadline);
    let (mut here, mut there) = (false, false);
    let outcome: std::result::Result<(), String> = loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(true) if !here => {
                    write_msg(&mut send, control::Msg::PairConfirm(urdevice_proto::PairConfirm {})).await?;
                    here = true;
                    if let Some(p) = lock(&inner.state).code_pairings.get_mut(&peer) {
                        p.confirmed_here = true;
                    }
                    inner.changed();
                }
                Some(true) => {}
                Some(false) | None => {
                    let _ = write_msg(&mut send, control::Msg::PairReject(urdevice_proto::PairReject { reason: "cancelled".into() })).await;
                    break Err("cancelled".into());
                }
            },
            msg = reader.next() => match msg {
                Ok(control::Msg::PairConfirm(_)) => there = true,
                Ok(control::Msg::PairReject(r)) => break Err(r.reason.chars().take(16).collect()),
                Ok(_) => break Err("rejected".into()),
                Err(_) => break Err("cancelled".into()),
            },
            _ = &mut deadline => break Err("timeout".into()),
        }
        if here && there {
            break Ok(());
        }
    };
    lock(&inner.state).code_pairings.remove(&peer);
    inner.changed();

    if let Err(reason) = outcome {
        inner.pairing_failed(&name, &reason);
        finish_reject(&conn).await;
        return Err(CoreError::Stopped);
    }
    inner.store_pairing(peer, &name, &platform, conn.remote_address())?;
    inner.emit(NodeEvent::PairingFinished {
        name,
        ok: true,
        reason: String::new(),
    });
    inner.changed();
    let r = if incoming {
        match reader.next_timeout(HANDSHAKE_TIMEOUT).await? {
            control::Msg::Hello(h) => serve_session(inner, conn, send, reader, peer, h).await,
            _ => Err(CoreError::Protocol("expected Hello".into())),
        }
    } else {
        client_session(inner, conn, send, reader, peer).await
    };
    if let Err(e) = r {
        tracing::debug!(error = %e, "session after pairing ended");
    }
    Err(CoreError::Stopped)
}

// ---------------------------------------------------------------------------
// Discovery, reconnect, and housekeeping
// ---------------------------------------------------------------------------

pub(super) async fn seen_loop(inner: Arc<Inner>, mut rx: mpsc::Receiver<Seen>) {
    while let Some(seen) = rx.recv().await {
        let mut changed = false;
        {
            let mut st = lock(&inner.state);
            let now = Instant::now();
            let entry = st.nearby.entry(seen.short_id).or_insert_with(|| {
                changed = true;
                super::Nearby {
                    name: seen.name.clone(),
                    platform: seen.platform.clone(),
                    addrs: Vec::new(),
                    last_seen: now,
                }
            });
            if entry.name != seen.name || !entry.addrs.contains(&seen.addr) {
                changed = true;
            }
            entry.name = seen.name.clone();
            entry.platform = seen.platform.clone();
            entry.last_seen = now;
            entry.addrs.retain(|a| *a != seen.addr);
            entry.addrs.insert(0, seen.addr);
            entry.addrs.truncate(4);
            if st.nearby.len() > 256 {
                st.nearby
                    .retain(|_, n| now.duration_since(n.last_seen) < super::NEARBY_TTL);
            }
            // A paired device announced itself: connect soon (R2.4).
            let paired = st
                .paired
                .iter()
                .find(|d| d.id.short() == seen.short_id)
                .map(|d| d.id);
            if let Some(id) = paired
                && !st.sessions.contains_key(&id)
                && let Some(b) = st.backoff.get_mut(&id)
                && b.delay <= Duration::from_secs(8)
            {
                b.next = b.next.min(now);
            }
        }
        if changed {
            inner.changed();
        }
    }
}

pub(super) async fn connector_loop(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tick.tick().await;
        let now = Instant::now();
        let mut jobs = Vec::new();
        {
            let mut st = lock(&inner.state);
            let candidates: Vec<_> = st
                .paired
                .iter()
                .filter(|d| d.auto_connect)
                .map(|d| (d.id, d.last_addrs.clone()))
                .collect();
            for (id, last) in candidates {
                if st.sessions.contains_key(&id) || st.connecting.contains(&id) {
                    continue;
                }
                if st.paused.get(&id).is_some_and(|t| *t > now) {
                    continue;
                }
                let mut addrs = st
                    .nearby
                    .get(&id.short())
                    .map(|n| n.addrs.clone())
                    .unwrap_or_default();
                for a in last {
                    if !addrs.contains(&a) {
                        addrs.push(a);
                    }
                }
                if addrs.is_empty() {
                    continue;
                }
                let b = st.backoff.entry(id).or_insert(Backoff {
                    next: now,
                    delay: Duration::from_secs(1),
                });
                if b.next > now {
                    continue;
                }
                b.next = now + b.delay;
                b.delay = (b.delay * 2).min(MAX_BACKOFF);
                st.connecting.insert(id);
                jobs.push((id, addrs));
            }
        }
        for (id, addrs) in jobs {
            let inner = inner.clone();
            tokio::spawn(async move {
                let r = connect_paired(&inner, id, &addrs).await;
                lock(&inner.state).connecting.remove(&id);
                if let Err(e) = r {
                    tracing::debug!(peer = %id.short_hex(), error = %e, "connect attempt ended");
                }
            });
        }
    }
}

async fn connect_paired(inner: &Arc<Inner>, id: DeviceId, addrs: &[SocketAddr]) -> Result<()> {
    let conn = dial(inner, addrs, Expect::Device(id)).await?;
    // Connected: the session task takes over, so stop counting it as a dial.
    lock(&inner.state).connecting.remove(&id);
    let (send, recv) = conn.open_bi().await?;
    client_session(inner.clone(), conn, send, Reader::new(recv), id).await
}

/// Latency changes smaller than this do not redraw the UI (R4.8).
const RTT_STEP_MS: u32 = 2;

pub(super) async fn housekeeping_loop(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_prune = Instant::now();
    let mut was_locked = false;
    let mut shown_rtt: HashMap<DeviceId, u32> = HashMap::new();
    loop {
        tick.tick().await;
        let mut changed = false;
        {
            let mut st = lock(&inner.state);
            let now = Instant::now();
            // A QR token that expired while the Pair screen is open is replaced (R3.11).
            if st.pair_screen.as_ref().is_some_and(|p| p.expires <= now) {
                match inner.new_pair_screen(&st) {
                    Ok(s) => st.pair_screen = Some(s),
                    Err(_) => st.pair_screen = None,
                }
                changed = true;
            }
            if st.pair_screen.is_some() {
                // The countdown on the Pair screen.
                changed = true;
            }
            let locked = st.lockout.remaining(now).is_some();
            if locked != was_locked {
                was_locked = locked;
                changed = true;
            }
            let before = st.nearby.len();
            st.nearby
                .retain(|_, n| now.duration_since(n.last_seen) < super::NEARBY_TTL);
            changed |= st.nearby.len() != before;
            // Latency on the device view (R4.8): redraw only when it moved.
            shown_rtt.retain(|id, _| st.sessions.contains_key(id));
            for (id, s) in &st.sessions {
                let rtt = super::rtt_ms(&s.conn);
                let moved = shown_rtt
                    .get(id)
                    .is_none_or(|old| old.abs_diff(rtt) >= RTT_STEP_MS);
                if moved {
                    shown_rtt.insert(*id, rtt);
                    changed = true;
                }
            }
        }
        inner.update_policy();
        if last_prune.elapsed() > Duration::from_secs(3600) {
            last_prune = Instant::now();
            let _ = lock(&inner.store).prune_activity(unix_now());
        }
        if changed {
            inner.changed();
        }
    }
}
