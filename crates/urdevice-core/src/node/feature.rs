//! Routing of feature traffic to services, with the permission check in
//! front (design §6.2, §7.4, §9).
//!
//! Three kinds of traffic reach a service: [`urdevice_proto::Feature`]
//! messages on the control stream, extra QUIC streams that start with a
//! [`urdevice_proto::StreamOpen`] header, and datagrams with a
//! [`urdevice_proto::DatagramHeader`]. Anything sent to the host side goes
//! through [`State::gate`] first.

use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::timeout;
use urdevice_proto::{Direction, ServiceId, control};

use super::{GrantEntry, Inner, State, lock, unix_now};
use crate::identity::DeviceId;
use crate::permission::{Capability, Mode};
use crate::services::{Access, Datagram, Grant, Message, Service, Stream, stream_code};
use crate::transport::close;
use crate::{CoreError, Result};

/// How long a peer has to send the header of a new stream.
const STREAM_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
/// Most approvals kept per peer for capabilities without an indicator.
/// The oldest goes first.
pub(super) const MAX_GRANTS_PER_PEER: usize = 64;

impl State {
    /// The permission engine for feature traffic sent to this device as the
    /// host (design §7.4). Returns the activity the traffic may use, or the
    /// refusal reason.
    ///
    /// - Deny refuses everything.
    /// - Traffic that names an activity must name one this device granted to
    ///   that peer for that capability and that has not ended.
    /// - Traffic without an activity is accepted only when the setting is
    ///   Allow and the capability has no indicator (R5.4). Always Ask needs
    ///   an `ActivityRequest` first.
    pub(super) fn gate(
        &self,
        platform: &str,
        peer: DeviceId,
        cap: Capability,
        activity: u64,
    ) -> std::result::Result<u64, &'static str> {
        if !cap.offered_on(platform) {
            return Err("unsupported");
        }
        let mode = self.mode(cap);
        if mode == Mode::Deny {
            return Err("denied");
        }
        if activity != 0 {
            let running = self
                .activities
                .iter()
                .any(|a| a.incoming && a.peer == peer && a.request_id == activity && a.cap == cap);
            let granted = self
                .grants
                .get(&(peer, activity))
                .is_some_and(|g| g.incoming && g.cap == cap);
            return if running || granted {
                Ok(activity)
            } else {
                Err("no_activity")
            };
        }
        if cap.is_watched() {
            return Err("no_activity");
        }
        match mode {
            Mode::Allow => Ok(0),
            _ => Err("ask"),
        }
    }
}

impl Inner {
    /// Check an incoming item and say which side it is for.
    fn access(
        &self,
        peer: DeviceId,
        cap: Capability,
        direction: Direction,
        activity: u64,
    ) -> std::result::Result<Access, &'static str> {
        let st = lock(&self.state);
        let Some(session) = st.sessions.get(&peer) else {
            return Err("offline");
        };
        match direction {
            Direction::ToHost => st
                .gate(&self.platform, peer, cap, activity)
                .map(|activity| {
                    Access::Host(Grant {
                        peer,
                        capability: cap,
                        activity,
                    })
                }),
            // The peer is the host; it listed what it offers in the handshake.
            Direction::ToController if session.caps.contains(&cap) => {
                Ok(Access::Controller { activity })
            }
            Direction::ToController => Err("unsupported"),
        }
    }

    pub(super) fn service_ctx(&self, s: &dyn Service) -> ServiceCtx {
        ServiceCtx {
            inner: self.this.clone(),
            service: s.id(),
            capability: s.capability(),
        }
    }

    /// Tell the services of `cap` that activities ended.
    pub(super) fn notify_ended(&self, ended: &[(DeviceId, Capability, u64)]) {
        for (peer, cap, activity) in ended {
            for s in self.services.for_capability(*cap) {
                s.on_activity_end(&self.service_ctx(s.as_ref()), *peer, *activity);
            }
        }
    }

    pub(super) fn notify_peer(&self, peer: DeviceId, connected: bool) {
        for s in self.services.iter() {
            s.on_peer(&self.service_ctx(s.as_ref()), peer, connected);
        }
    }

    /// End approvals without an indicator and tell both sides' services.
    pub(super) fn end_grants(&self, pred: impl Fn(&(DeviceId, u64), &GrantEntry) -> bool) {
        let ended: Vec<((DeviceId, u64), GrantEntry, Option<_>)> = {
            let mut st = lock(&self.state);
            let keys: Vec<_> = st
                .grants
                .iter()
                .filter(|(k, g)| pred(k, g))
                .map(|(k, _)| *k)
                .collect();
            keys.into_iter()
                .filter_map(|k| {
                    let g = st.grants.remove(&k)?;
                    let tx = st.sessions.get(&k.0).map(|s| s.tx.clone());
                    Some((k, g, tx))
                })
                .collect()
        };
        if ended.is_empty() {
            return;
        }
        for ((_, request_id), _, tx) in &ended {
            if let Some(tx) = tx {
                let _ = tx.send(control::Msg::ActivityStop(urdevice_proto::ActivityStop {
                    request_id: *request_id,
                }));
            }
        }
        let list: Vec<_> = ended
            .iter()
            .map(|((peer, id), g, _)| (*peer, g.cap, *id))
            .collect();
        self.notify_ended(&list);
    }

    fn refuse(&self, peer: DeviceId, service: u32, activity: u64, reason: &str) {
        let tx = lock(&self.state).sessions.get(&peer).map(|s| s.tx.clone());
        if let Some(tx) = tx {
            let _ = tx.send(control::Msg::FeatureRefused(
                urdevice_proto::FeatureRefused {
                    service,
                    activity,
                    reason: reason.into(),
                },
            ));
        }
    }
}

/// A [`urdevice_proto::Feature`] message arrived on the control stream.
/// An error is a protocol violation and closes the session.
pub(super) fn on_feature(
    inner: &Arc<Inner>,
    peer: DeviceId,
    f: urdevice_proto::Feature,
) -> Result<()> {
    let direction = f.direction();
    let service = ServiceId::from_wire(f.service).and_then(|id| inner.services.get(id).cloned());
    let Some(service) = service else {
        if direction == Direction::ToHost {
            inner.refuse(peer, f.service, f.activity, "unsupported");
        }
        return Ok(());
    };
    match inner.access(peer, service.capability(), direction, f.activity) {
        Ok(access) => service.on_message(
            &inner.service_ctx(service.as_ref()),
            Message {
                peer,
                access,
                body: f.body,
            },
        ),
        Err(reason) => {
            tracing::debug!(peer = %peer.short_hex(), service = f.service, reason, "feature message refused");
            if direction == Direction::ToHost {
                inner.refuse(peer, f.service, f.activity, reason);
            }
            Ok(())
        }
    }
}

/// The peer refused a message one of our services sent.
pub(super) fn on_refused(inner: &Arc<Inner>, peer: DeviceId, r: urdevice_proto::FeatureRefused) {
    let Some(service) =
        ServiceId::from_wire(r.service).and_then(|id| inner.services.get(id).cloned())
    else {
        return;
    };
    let reason: String = r.reason.chars().take(16).collect();
    service.on_refused(
        &inner.service_ctx(service.as_ref()),
        peer,
        r.activity,
        &reason,
    );
}

/// Accept the streams and datagrams a peer opens during a session. Runs
/// until the connection closes; the session aborts it when it ends.
pub(super) async fn accept_traffic(inner: Arc<Inner>, conn: quinn::Connection, peer: DeviceId) {
    loop {
        tokio::select! {
            bi = conn.accept_bi() => match bi {
                Ok((send, recv)) => {
                    tokio::spawn(incoming_stream(inner.clone(), conn.clone(), peer, Some(send), recv));
                }
                Err(_) => break,
            },
            uni = conn.accept_uni() => match uni {
                Ok(recv) => {
                    tokio::spawn(incoming_stream(inner.clone(), conn.clone(), peer, None, recv));
                }
                Err(_) => break,
            },
            d = conn.read_datagram() => match d {
                Ok(d) => incoming_datagram(&inner, peer, &d),
                Err(_) => break,
            },
        }
    }
}

async fn read_open(recv: &mut quinn::RecvStream) -> Result<urdevice_proto::StreamOpen> {
    let mut header = [0u8; 4];
    recv.read_exact(&mut header).await?;
    let len = urdevice_proto::stream_open_len(header)?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(urdevice_proto::decode_stream_open(&body)?)
}

async fn incoming_stream(
    inner: Arc<Inner>,
    conn: quinn::Connection,
    peer: DeviceId,
    mut send: Option<quinn::SendStream>,
    mut recv: quinn::RecvStream,
) {
    let open = match timeout(STREAM_OPEN_TIMEOUT, read_open(&mut recv)).await {
        Ok(Ok(o)) => o,
        Ok(Err(CoreError::Protocol(e))) => {
            tracing::debug!(peer = %peer.short_hex(), error = %e, "bad stream header");
            conn.close(close::PROTOCOL.into(), b"protocol");
            super::net::record_violation(&inner, peer);
            return;
        }
        _ => return,
    };
    let mut refuse = |code: u32| {
        if let Some(s) = send.as_mut() {
            let _ = s.reset(code.into());
        }
        let _ = recv.stop(code.into());
    };
    let service = ServiceId::from_wire(open.service).and_then(|id| inner.services.get(id).cloned());
    let Some(service) = service else {
        refuse(stream_code::UNSUPPORTED);
        return;
    };
    match inner.access(peer, service.capability(), open.direction(), open.activity) {
        Ok(access) => service.on_stream(
            &inner.service_ctx(service.as_ref()),
            Stream {
                peer,
                access,
                kind: open.kind,
                meta: open.meta,
                send,
                recv,
            },
        ),
        Err(reason) => {
            tracing::debug!(peer = %peer.short_hex(), service = open.service, reason, "feature stream refused");
            refuse(if reason == "unsupported" {
                stream_code::UNSUPPORTED
            } else {
                stream_code::REFUSED
            });
        }
    }
}

fn incoming_datagram(inner: &Arc<Inner>, peer: DeviceId, d: &[u8]) {
    let Ok((h, payload)) = urdevice_proto::decode_datagram(d) else {
        return;
    };
    let Some(service) = inner.services.get(h.service).cloned() else {
        return;
    };
    if let Ok(access) = inner.access(peer, service.capability(), h.direction, h.activity) {
        service.on_datagram(
            &inner.service_ctx(service.as_ref()),
            Datagram {
                peer,
                access,
                payload: payload.to_vec(),
            },
        );
    }
}

// ---------------------------------------------------------------------------
// The handle a service uses
// ---------------------------------------------------------------------------

/// What a [`Service`] can do with the node. Cheap to clone; keep the one
/// passed to [`Service::start`]. Every call fails with
/// [`CoreError::Stopped`] after the node stopped.
#[derive(Clone)]
pub struct ServiceCtx {
    inner: Weak<Inner>,
    service: ServiceId,
    capability: Capability,
}

impl std::fmt::Debug for ServiceCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceCtx")
            .field("service", &self.service)
            .field("capability", &self.capability)
            .finish_non_exhaustive()
    }
}

impl ServiceCtx {
    fn inner(&self) -> Result<Arc<Inner>> {
        self.inner.upgrade().ok_or(CoreError::Stopped)
    }

    /// This service's id.
    pub fn service(&self) -> ServiceId {
        self.service
    }

    /// This service's capability.
    pub fn capability(&self) -> Capability {
        self.capability
    }

    /// This device's ID.
    pub fn this_device(&self) -> Result<DeviceId> {
        Ok(self.inner()?.id())
    }

    /// This device's platform: `linux`, `windows`, `macos`, or `android`.
    pub fn platform(&self) -> Result<String> {
        Ok(self.inner()?.platform.clone())
    }

    /// Paired devices with an open session.
    pub fn connected(&self) -> Vec<DeviceId> {
        let Ok(inner) = self.inner() else {
            return Vec::new();
        };
        lock(&inner.state).sessions.keys().copied().collect()
    }

    /// True when `peer` is connected and offers this capability as a host
    /// (it listed it in the handshake).
    pub fn peer_offers(&self, peer: DeviceId) -> bool {
        self.inner().is_ok_and(|inner| {
            lock(&inner.state)
                .sessions
                .get(&peer)
                .is_some_and(|s| s.caps.contains(&self.capability))
        })
    }

    /// Display name of a paired device.
    pub fn peer_name(&self, peer: DeviceId) -> String {
        self.inner()
            .map(|inner| lock(&inner.state).peer_name(peer))
            .unwrap_or_default()
    }

    /// Platform of a paired device.
    pub fn peer_platform(&self, peer: DeviceId) -> Option<String> {
        let inner = self.inner().ok()?;
        let st = lock(&inner.state);
        st.paired(peer).map(|d| d.platform.clone())
    }

    /// This device's setting for the capability (for drawing, not for
    /// checks: the node already checked what reaches the service).
    pub fn local_mode(&self) -> Mode {
        self.inner()
            .map(|inner| lock(&inner.state).mode(self.capability))
            .unwrap_or(Mode::Deny)
    }

    /// The peer's setting for the capability from its `PermissionState`.
    /// For the UI only: the peer checks for itself (design §6.3).
    pub fn remote_mode(&self, peer: DeviceId) -> Option<Mode> {
        let inner = self.inner().ok()?;
        let st = lock(&inner.state);
        st.sessions
            .get(&peer)?
            .remote_perms
            .iter()
            .find(|p| p.capability == self.capability)
            .map(|p| p.mode)
    }

    /// Send one of the service's messages on the control stream. `activity`
    /// is the granted request it belongs to, or 0.
    pub fn send(
        &self,
        peer: DeviceId,
        direction: Direction,
        activity: u64,
        body: &impl prost::Message,
    ) -> Result<()> {
        let inner = self.inner()?;
        let msg = control::Msg::Feature(urdevice_proto::Feature {
            service: self.service.to_wire(),
            direction: direction as i32,
            activity,
            body: body.encode_to_vec(),
        });
        let len = prost::Message::encoded_len(&urdevice_proto::Control::from(msg.clone()));
        if len > urdevice_proto::MAX_FRAME {
            return Err(CoreError::InvalidInput(format!(
                "message of {len} bytes is over the control limit; use a stream"
            )));
        }
        let tx = lock(&inner.state).sessions.get(&peer).map(|s| s.tx.clone());
        let tx = tx.ok_or_else(|| CoreError::Network("not connected".into()))?;
        tx.send(msg)
            .map_err(|_| CoreError::Network("not connected".into()))
    }

    fn conn(&self, peer: DeviceId) -> Result<quinn::Connection> {
        let inner = self.inner()?;
        let conn = lock(&inner.state)
            .sessions
            .get(&peer)
            .map(|s| s.conn.clone());
        conn.ok_or_else(|| CoreError::Network("not connected".into()))
    }

    fn header(
        &self,
        direction: Direction,
        activity: u64,
        kind: u32,
        meta: Vec<u8>,
    ) -> Result<Vec<u8>> {
        Ok(urdevice_proto::encode_stream_open(
            &urdevice_proto::StreamOpen {
                service: self.service.to_wire(),
                direction: direction as i32,
                activity,
                kind,
                meta,
            },
        )?)
    }

    /// Open a bidirectional stream to `peer` (bulk data, tunnels). `kind`
    /// and `meta` are the service's own; the peer's service gets them in
    /// [`Stream`].
    pub async fn open_bi(
        &self,
        peer: DeviceId,
        direction: Direction,
        activity: u64,
        kind: u32,
        meta: Vec<u8>,
    ) -> Result<(quinn::SendStream, quinn::RecvStream)> {
        let header = self.header(direction, activity, kind, meta)?;
        let (mut send, recv) = self.conn(peer)?.open_bi().await?;
        send.write_all(&header).await?;
        Ok((send, recv))
    }

    /// Open a unidirectional stream to `peer` (video, audio).
    pub async fn open_uni(
        &self,
        peer: DeviceId,
        direction: Direction,
        activity: u64,
        kind: u32,
        meta: Vec<u8>,
    ) -> Result<quinn::SendStream> {
        let header = self.header(direction, activity, kind, meta)?;
        let mut send = self.conn(peer)?.open_uni().await?;
        send.write_all(&header).await?;
        Ok(send)
    }

    /// Send a datagram: unreliable and unordered, for input events.
    pub fn send_datagram(
        &self,
        peer: DeviceId,
        direction: Direction,
        activity: u64,
        payload: &[u8],
    ) -> Result<()> {
        let d = urdevice_proto::encode_datagram(
            urdevice_proto::DatagramHeader {
                service: self.service,
                direction,
                activity,
            },
            payload,
        );
        self.conn(peer)?
            .send_datagram(bytes::Bytes::from(d))
            .map_err(|e| CoreError::Network(e.to_string()))
    }

    /// Largest datagram payload the path to `peer` carries now, or `None`
    /// when the peer does not take datagrams (then use the control stream).
    pub fn max_datagram_payload(&self, peer: DeviceId) -> Option<usize> {
        let size = self.conn(peer).ok()?.max_datagram_size()?;
        size.checked_sub(urdevice_proto::DATAGRAM_HEADER)
    }

    /// Ask `peer` to start an activity of this capability (design §7.4). The
    /// peer's engine allows it, prompts its user (Always Ask), or refuses.
    /// Returns the activity id to put in later messages, streams, and
    /// datagrams, or [`CoreError::Refused`] with `denied`, `declined`,
    /// `timeout`, `unsupported`, or `offline`.
    pub async fn request(&self, peer: DeviceId) -> Result<u64> {
        let inner = self.inner()?;
        let (tx, rx) = oneshot::channel();
        let request_id = {
            let mut st = lock(&inner.state);
            let Some(stx) = st.sessions.get(&peer).map(|s| s.tx.clone()) else {
                return Err(CoreError::Refused("offline".into()));
            };
            let id = st.next_id();
            st.pending_requests.insert((peer, id), self.capability);
            st.waiters.insert((peer, id), tx);
            let _ = stx.send(control::Msg::ActivityRequest(
                urdevice_proto::ActivityRequest {
                    request_id: id,
                    capability: self.capability.as_str().into(),
                },
            ));
            id
        };
        // The host waits for its user for up to its own Always Ask timeout.
        let budget = inner.ask_timeout + Duration::from_secs(15);
        let r = timeout(budget, rx).await;
        drop(inner);
        match r {
            Ok(Ok(Ok(()))) => Ok(request_id),
            Ok(Ok(Err(reason))) => Err(CoreError::Refused(reason)),
            Ok(Err(_)) => Err(CoreError::Refused("offline".into())),
            Err(_) => {
                if let Ok(inner) = self.inner() {
                    let mut st = lock(&inner.state);
                    st.pending_requests.remove(&(peer, request_id));
                    st.waiters.remove(&(peer, request_id));
                }
                Err(CoreError::Refused("timeout".into()))
            }
        }
    }

    /// End an activity, on either side. Both devices' services get
    /// [`Service::on_activity_end`].
    pub fn stop(&self, peer: DeviceId, activity: u64) {
        let Ok(inner) = self.inner() else { return };
        let is_activity = lock(&inner.state)
            .activities
            .iter()
            .any(|a| a.peer == peer && a.request_id == activity);
        if is_activity {
            inner.stop_activities(|a| a.peer == peer && a.request_id == activity, "stopped");
        } else {
            inner.end_grants(|k, _| *k == (peer, activity));
        }
    }

    /// Record a decision in the activity log (R5.6). Never put content here.
    pub fn log(&self, peer: DeviceId, decision: &str) {
        let Ok(inner) = self.inner() else { return };
        let name = lock(&inner.state).peer_name(peer);
        let _ = lock(&inner.store).log_activity(
            peer,
            &name,
            self.capability.as_str(),
            decision,
            unix_now(),
        );
    }

    /// Ask the UI to redraw from a fresh snapshot.
    pub fn changed(&self) {
        if let Ok(inner) = self.inner() {
            inner.changed();
        }
    }

    /// Run a task on the node's runtime. It is not cancelled on its own;
    /// end it in [`Service::on_activity_end`] or when its streams close.
    pub fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if let Ok(inner) = self.inner() {
            inner.rt.spawn(fut);
        }
    }
}

/// Record an approval without an indicator, keeping at most
/// [`MAX_GRANTS_PER_PEER`] per peer. Returns the approval it pushed out, so
/// the caller can tell its services once the state lock is released.
pub(super) fn add_grant(
    st: &mut State,
    peer: DeviceId,
    request_id: u64,
    entry: GrantEntry,
) -> Option<(DeviceId, Capability, u64)> {
    let mine: Vec<u64> = st
        .grants
        .keys()
        .filter(|(p, _)| *p == peer)
        .map(|(_, id)| *id)
        .collect();
    let mut evicted = None;
    if mine.len() >= MAX_GRANTS_PER_PEER
        && let Some(oldest) = mine.iter().min()
        && let Some(g) = st.grants.remove(&(peer, *oldest))
    {
        evicted = Some((peer, g.cap, *oldest));
    }
    st.grants.insert((peer, request_id), entry);
    evicted
}
