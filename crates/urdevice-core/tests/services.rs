//! Feature routing and the permission check in front of services (design
//! §6.2, §7.4, §9), end to end between two in-process nodes.
//!
//! The `Echo` service here is also the smallest example of a feature
//! service and its test.

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::{Arc, Mutex, OnceLock};

use harness::{Dev, Pair, WAIT, block_on, holds_for, service, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::{
    Access, Datagram, Direction, Message, Service, ServiceCtx, ServiceId, Stream,
};
use urdevice_core::{Capability, CoreError, Mode};

/// What the service saw.
#[derive(Clone, Debug, PartialEq)]
enum Seen {
    Msg(Access, Vec<u8>),
    Stream(Access, u32, Vec<u8>),
    Datagram(Access, Vec<u8>),
    Refused(u64, String),
    Ended(u64),
}

/// Echoes host-side messages back to the controller and records everything.
struct Echo {
    id: ServiceId,
    cap: Capability,
    ctx: OnceLock<ServiceCtx>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Echo {
    fn new(id: u16, cap: Capability) -> Arc<Self> {
        Arc::new(Self {
            id: ServiceId(id),
            cap,
            ctx: OnceLock::new(),
            seen: Arc::default(),
        })
    }

    fn ctx(&self) -> &ServiceCtx {
        self.ctx.get().unwrap()
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn saw(&self, f: impl Fn(&Seen) -> bool) -> bool {
        self.seen().iter().any(f)
    }

    fn clear(&self) {
        self.seen.lock().unwrap().clear();
    }

    fn record(&self, s: Seen) {
        self.seen.lock().unwrap().push(s);
    }

    /// Send `body` to the host as a protobuf `bytes` field.
    fn send(&self, peer: DeviceId, activity: u64, body: &[u8]) {
        self.ctx()
            .send(peer, Direction::ToHost, activity, &body.to_vec())
            .unwrap();
    }
}

impl Service for Echo {
    fn id(&self) -> ServiceId {
        self.id
    }

    fn capability(&self) -> Capability {
        self.cap
    }

    fn start(&self, ctx: ServiceCtx) {
        let _ = self.ctx.set(ctx);
    }

    fn on_message(&self, ctx: &ServiceCtx, msg: Message) -> urdevice_core::Result<()> {
        let body: Vec<u8> = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad body".into()))?;
        if let Access::Host(g) = &msg.access {
            ctx.send(msg.peer, Direction::ToController, g.activity(), &body)?;
        }
        self.record(Seen::Msg(msg.access, body));
        Ok(())
    }

    fn on_stream(&self, ctx: &ServiceCtx, mut s: Stream) {
        let seen = self.seen.clone();
        ctx.spawn(async move {
            let data = s.recv.read_to_end(64 * 1024).await.unwrap_or_default();
            if let Some(send) = s.send.as_mut() {
                let _ = send.write_all(&data).await;
                let _ = send.finish();
            }
            seen.lock()
                .unwrap()
                .push(Seen::Stream(s.access, s.kind, data));
        });
    }

    fn on_datagram(&self, _ctx: &ServiceCtx, d: Datagram) {
        self.record(Seen::Datagram(d.access, d.payload));
    }

    fn on_refused(&self, _ctx: &ServiceCtx, _peer: DeviceId, activity: u64, reason: &str) {
        self.record(Seen::Refused(activity, reason.into()));
    }

    fn on_activity_end(&self, _ctx: &ServiceCtx, _peer: DeviceId, activity: u64) {
        self.record(Seen::Ended(activity));
    }
}

const CLIP: u16 = 0xff01;
const INPUT: u16 = 0xff02;

/// Two devices that both run an echo service for `cap`.
fn pair(id: u16, cap: Capability) -> (Pair, Arc<Echo>, Arc<Echo>) {
    let (ea, eb) = (Echo::new(id, cap), Echo::new(id, cap));
    let p = Pair::with(
        Dev::with("Laptop", "linux", vec![service(ea.clone())]),
        Dev::with("Phone", "android", vec![service(eb.clone())]),
    );
    // The node hands out the same instance.
    assert!(Arc::ptr_eq(&p.a.node.service::<Echo>().unwrap(), &ea));
    (p, ea, eb)
}

#[test]
fn allow_reaches_the_host_and_the_reply_comes_back() {
    let (p, host, ctl) = pair(CLIP, Capability::Clipboard);
    assert_eq!(
        p.b.remote_mode(&p.a, Capability::Clipboard),
        Some(Mode::Allow)
    );
    ctl.send(p.a.id(), 0, b"hi");
    wait_until("host got it", WAIT, || {
        host.saw(|s| matches!(s, Seen::Msg(Access::Host(g), b) if b == b"hi" && g.activity() == 0 && g.capability() == Capability::Clipboard))
    });
    wait_until("reply", WAIT, || {
        ctl.saw(|s| matches!(s, Seen::Msg(Access::Controller { activity: 0 }, b) if b == b"hi"))
    });
}

#[test]
fn deny_and_always_ask_refuse_before_the_service() {
    let (p, host, ctl) = pair(CLIP, Capability::Clipboard);
    p.a.set(Capability::Clipboard, Mode::Deny);
    ctl.send(p.a.id(), 0, b"x");
    wait_until("denied", WAIT, || {
        ctl.saw(|s| matches!(s, Seen::Refused(0, r) if r == "denied"))
    });

    // Always Ask: a message without an approved request is refused.
    p.a.set(Capability::Clipboard, Mode::Ask);
    ctl.send(p.a.id(), 0, b"x");
    wait_until("ask", WAIT, || {
        ctl.saw(|s| matches!(s, Seen::Refused(0, r) if r == "ask"))
    });
    // A made-up activity id is refused too.
    ctl.send(p.a.id(), 12345, b"x");
    wait_until("no_activity", WAIT, || {
        ctl.saw(|s| matches!(s, Seen::Refused(12345, r) if r == "no_activity"))
    });
    holds_for("host saw nothing", 200, || host.seen().is_empty());

    // The service asks; the host's user allows; traffic naming the
    // activity gets through.
    let a_id = p.a.id();
    let ctx = ctl.ctx().clone();
    let request = std::thread::spawn(move || block_on(ctx.request(a_id)));
    let prompt = p.a.answer_prompt(true);
    assert_eq!(prompt.capability, "clipboard.v1");
    let activity = request.join().unwrap().unwrap();
    assert_ne!(activity, 0);
    ctl.send(p.a.id(), activity, b"ok");
    wait_until("granted message", WAIT, || {
        host.saw(|s| matches!(s, Seen::Msg(Access::Host(g), b) if b == b"ok" && g.activity() == activity))
    });

    // Deny revokes it on both devices (R5.5).
    p.a.set(Capability::Clipboard, Mode::Deny);
    wait_until("ended on both", WAIT, || {
        host.saw(|s| *s == Seen::Ended(activity)) && ctl.saw(|s| *s == Seen::Ended(activity))
    });

    // A declined request comes back as Refused.
    p.a.set(Capability::Clipboard, Mode::Ask);
    let ctx = ctl.ctx().clone();
    let request = std::thread::spawn(move || block_on(ctx.request(a_id)));
    p.a.answer_prompt(false);
    assert!(matches!(request.join().unwrap(), Err(CoreError::Refused(r)) if r == "declined"));
}

#[test]
fn watched_capability_needs_a_running_activity() {
    let (p, host, ctl) = pair(INPUT, Capability::Input);
    p.a.set(Capability::Input, Mode::Allow);
    // Even with Allow, input needs an activity, so the indicator shows (R5.4).
    ctl.send(p.a.id(), 0, b"x");
    wait_until("no_activity", WAIT, || {
        ctl.saw(|s| matches!(s, Seen::Refused(0, r) if r == "no_activity"))
    });

    let activity = block_on(ctl.ctx().request(p.a.id())).unwrap();
    wait_until("indicator", WAIT, || {
        p.a.snap().activities.iter().any(|a| a.incoming) && !p.b.snap().activities.is_empty()
    });

    // Datagrams and streams that name the activity reach the host.
    let a_id = p.a.id();
    wait_until("datagram", WAIT, || {
        ctl.ctx()
            .send_datagram(a_id, Direction::ToHost, activity, b"move")
            .unwrap();
        host.saw(|s| matches!(s, Seen::Datagram(Access::Host(g), d) if d == b"move" && g.activity() == activity))
    });
    assert!(ctl.ctx().max_datagram_payload(a_id).unwrap() > 100);
    let echoed = block_on(async {
        let (mut send, mut recv) = ctl
            .ctx()
            .open_bi(a_id, Direction::ToHost, activity, 7, b"meta".to_vec())
            .await
            .unwrap();
        send.write_all(b"bulk").await.unwrap();
        send.finish().unwrap();
        recv.read_to_end(1024).await.unwrap()
    });
    assert_eq!(echoed, b"bulk");
    wait_until("stream seen", WAIT, || {
        host.saw(|s| {
            matches!(s, Seen::Stream(Access::Host(g), 7, d) if d == b"bulk" && g.activity() == activity)
        })
    });

    // Stop on the host ends it on both sides; later datagrams are dropped.
    let id = p.a.snap().activities[0].id;
    p.a.node.stop_activity(id);
    wait_until("ended", WAIT, || {
        host.saw(|s| *s == Seen::Ended(activity)) && ctl.saw(|s| *s == Seen::Ended(activity))
    });
    host.clear();
    ctl.ctx()
        .send_datagram(a_id, Direction::ToHost, activity, b"late")
        .unwrap();
    holds_for("late datagram dropped", 300, || {
        !host.saw(|s| matches!(s, Seen::Datagram(..)))
    });

    // A stream without the activity is refused before the service sees it.
    let refused = block_on(async {
        let (mut send, mut recv) = ctl
            .ctx()
            .open_bi(a_id, Direction::ToHost, 0, 7, Vec::new())
            .await
            .unwrap();
        let _ = send.write_all(b"x").await;
        let _ = send.finish();
        recv.read_to_end(1024).await
    });
    assert!(refused.is_err());
}

#[test]
fn unknown_service_and_unshipped_capability_are_unsupported() {
    // Only the phone has the service.
    let eb = Echo::new(0xff03, Capability::Clipboard);
    let p = Pair::with(
        Dev::new("Laptop"),
        Dev::with("Phone", "android", vec![service(eb.clone())]),
    );
    eb.send(p.a.id(), 0, b"x");
    wait_until("unsupported", WAIT, || {
        eb.saw(|s| matches!(s, Seen::Refused(0, r) if r == "unsupported"))
    });

    // A Release 2 capability that has not shipped is never offered.
    let (ea, eb) = (
        Echo::new(0xff04, Capability::Notify),
        Echo::new(0xff04, Capability::Notify),
    );
    let p = Pair::with(
        Dev::with("Laptop", "linux", vec![service(ea.clone())]),
        Dev::with("Phone", "android", vec![service(eb.clone())]),
    );
    if !Capability::Notify.shipped() {
        eb.send(p.a.id(), 0, b"x");
        wait_until("unsupported", WAIT, || {
            eb.saw(|s| matches!(s, Seen::Refused(0, r) if r == "unsupported"))
        });
        assert!(ea.seen().is_empty());
        assert!(matches!(
            block_on(eb.ctx().request(p.a.id())),
            Err(CoreError::Refused(r)) if r == "unsupported"
        ));
    }
}

#[test]
fn malformed_body_closes_the_session() {
    let (p, _host, ctl) = pair(CLIP, Capability::Clipboard);
    // Not a valid protobuf `bytes` field: the host's service returns a
    // protocol error and the node drops the session.
    ctl.ctx()
        .send(p.a.id(), Direction::ToHost, 0, &RawBytes(vec![0xff, 0xff]))
        .unwrap();
    wait_until("session closed", WAIT, || !p.a.connected_to(&p.b));
}

/// A message whose encoding is the raw bytes, for the malformed-body test.
#[derive(Debug, Default)]
struct RawBytes(Vec<u8>);

impl prost::Message for RawBytes {
    fn encode_raw(&self, buf: &mut impl bytes::BufMut) {
        buf.put_slice(&self.0);
    }
    fn merge_field(
        &mut self,
        _tag: u32,
        _wire_type: prost::encoding::WireType,
        _buf: &mut impl bytes::Buf,
        _ctx: prost::encoding::DecodeContext,
    ) -> Result<(), prost::DecodeError> {
        Ok(())
    }
    fn encoded_len(&self) -> usize {
        self.0.len()
    }
    fn clear(&mut self) {
        self.0.clear();
    }
}
