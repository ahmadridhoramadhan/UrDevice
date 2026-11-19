//! Clipboard sharing end to end between two in-process nodes (#16, R7).

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness::{Dev, Pair, WAIT, holds_for, wait_until};
use urdevice_core::services::clipboard::{
    ClipData, ClipEvent, Clipboard, ClipboardEvents, ClipboardHost, SkipReason, Submit,
};
use urdevice_core::services::{Direction, Grant};
use urdevice_core::{Capability, Mode};
use urdevice_proto::features::clipboard::{Clip, MAX_CLIP_BYTES, mime};

/// A fake system clipboard: records what the service writes.
#[derive(Default)]
struct Fake {
    writes: Mutex<Vec<ClipData>>,
}

impl ClipboardHost for Fake {
    fn write(&self, grant: &Grant, data: &ClipData) -> urdevice_core::Result<()> {
        assert_eq!(grant.capability(), Capability::Clipboard);
        self.writes.lock().unwrap().push(data.clone());
        Ok(())
    }
}

impl Fake {
    fn writes(&self) -> Vec<ClipData> {
        self.writes.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<ClipEvent>>);

impl ClipboardEvents for Events {
    fn on_event(&self, e: &ClipEvent) {
        self.0.lock().unwrap().push(e.clone());
    }
}

impl Events {
    fn has(&self, f: impl Fn(&ClipEvent) -> bool) -> bool {
        self.0.lock().unwrap().iter().any(f)
    }
}

struct Side {
    svc: Arc<Clipboard>,
    host: Arc<Fake>,
    events: Arc<Events>,
}

fn side(d: &Dev) -> Side {
    let svc = d.node.service::<Clipboard>().unwrap();
    let host = Arc::new(Fake::default());
    let events = Arc::new(Events::default());
    svc.host.set(host.clone());
    svc.events.set(events.clone());
    Side { svc, host, events }
}

fn pair() -> (Pair, Side, Side) {
    let p = Pair::with(
        Dev::with("Laptop", "linux", vec![]),
        Dev::with("Phone", "android", vec![]),
    );
    let (a, b) = (side(&p.a), side(&p.b));
    (p, a, b)
}

/// A PNG header with the given size, padded to `len` bytes. Enough for the
/// service, which checks the header and never decodes the pixels.
fn png(w: u32, h: u32, len: usize) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(&13u32.to_be_bytes());
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    v.resize(len.max(v.len()), 0x5a);
    v
}

#[test]
fn text_html_and_png_reach_the_other_device_within_a_second() {
    let (p, a, b) = pair();

    let t = Instant::now();
    assert_eq!(
        a.svc.local_changed(ClipData::text("hello"), false),
        Submit::Sending(1)
    );
    wait_until("text", 2, || b.host.writes().len() == 1);
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    assert_eq!(b.host.writes()[0], ClipData::text("hello"));

    let html = ClipData::html("<b>bold</b>", Some("bold".into()));
    a.svc.local_changed(html.clone(), false);
    wait_until("html", 2, || b.host.writes().len() == 2);
    assert_eq!(b.host.writes()[1], html);

    let img = ClipData::png(png(64, 64, 200 * 1024));
    a.svc.local_changed(img.clone(), false);
    wait_until("png", 2, || b.host.writes().len() == 3);
    assert_eq!(b.host.writes()[2], img);

    let to = p.b.id();
    assert!(
        a.events
            .has(|e| matches!(e, ClipEvent::Sent { peer, .. } if *peer == to))
    );
    assert!(
        b.events
            .has(|e| matches!(e, ClipEvent::Received { len: 5, .. }))
    );
}

#[test]
fn it_works_in_both_directions() {
    let (_p, a, b) = pair();
    b.svc.send_manual(ClipData::text("from phone"), false, None);
    wait_until("laptop got it", WAIT, || a.host.writes().len() == 1);
    assert_eq!(a.host.writes()[0], ClipData::text("from phone"));
}

#[test]
fn large_images_go_over_a_stream_quickly() {
    let (_p, a, b) = pair();
    // 10 MB is the limit (R7.2) and far over the 512 KiB inline limit.
    let img = ClipData::png(png(4000, 4000, MAX_CLIP_BYTES));
    let t = Instant::now();
    assert_eq!(a.svc.local_changed(img.clone(), false), Submit::Sending(1));
    wait_until("10 MB image", WAIT, || b.host.writes().len() == 1);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    assert_eq!(b.host.writes()[0].bytes, img.bytes);
}

#[test]
fn over_ten_megabytes_is_not_sent() {
    let (_p, a, b) = pair();
    let big = ClipData::png(png(10, 10, MAX_CLIP_BYTES + 1));
    assert_eq!(
        a.svc.local_changed(big, false),
        Submit::Skipped(SkipReason::TooLarge)
    );
    let big_text = ClipData::text("x".repeat(MAX_CLIP_BYTES + 1));
    assert_eq!(
        a.svc.local_changed(big_text, false),
        Submit::Skipped(SkipReason::TooLarge)
    );
    holds_for("nothing arrived", 300, || b.host.writes().is_empty());
}

#[test]
fn sensitive_content_is_never_sent() {
    let (_p, a, b) = pair();
    assert_eq!(
        a.svc.local_changed(ClipData::text("hunter2"), true),
        Submit::Skipped(SkipReason::Sensitive)
    );
    // The manual actions honour the mark too.
    assert_eq!(
        a.svc.send_manual(ClipData::text("hunter2"), true, None),
        Submit::Skipped(SkipReason::Sensitive)
    );
    assert!(
        a.events
            .has(|e| *e == ClipEvent::Skipped(SkipReason::Sensitive))
    );
    holds_for("nothing arrived", 300, || b.host.writes().is_empty());
}

#[test]
fn received_content_is_not_sent_back() {
    let (_p, a, b) = pair();
    a.svc.local_changed(ClipData::text("one\ntwo"), false);
    wait_until("received", WAIT, || b.host.writes().len() == 1);
    // The watcher on b reports what b just wrote, as the OS may return it:
    // with Windows line endings.
    assert_eq!(
        b.svc.local_changed(ClipData::text("one\r\ntwo"), false),
        Submit::Echo
    );
    assert_eq!(
        b.svc.local_changed(ClipData::text("one\ntwo"), false),
        Submit::Echo
    );
    holds_for("no echo reached a", 300, || a.host.writes().is_empty());
    // New content still goes out.
    assert_eq!(
        b.svc.local_changed(ClipData::text("three"), false),
        Submit::Sending(1)
    );
    wait_until("a got three", WAIT, || a.host.writes().len() == 1);
    // And a repeat of what b sent last is not sent again.
    assert_eq!(
        b.svc.local_changed(ClipData::text("three"), false),
        Submit::Echo
    );
}

#[test]
fn pause_and_the_per_device_switch_stop_both_directions() {
    let (p, a, b) = pair();

    a.svc.set_paused(true).unwrap();
    assert!(a.svc.paused());
    assert_eq!(
        a.svc.local_changed(ClipData::text("x"), false),
        Submit::Skipped(SkipReason::Paused)
    );
    // The other device still sends, but a writes nothing while paused.
    b.svc.local_changed(ClipData::text("y"), false);
    holds_for("paused device writes nothing", 400, || {
        a.host.writes().is_empty()
    });
    a.svc.set_paused(false).unwrap();
    b.svc.local_changed(ClipData::text("z"), false);
    wait_until("resumed", WAIT, || a.host.writes().len() == 1);

    // Per-device switch on a: ignores b and sends nothing to b.
    a.svc.set_peer_enabled(p.b.id(), false).unwrap();
    assert!(!a.svc.peer_enabled(p.b.id()));
    assert_eq!(
        a.svc.local_changed(ClipData::text("q"), false),
        Submit::Skipped(SkipReason::NoDevice)
    );
    b.svc.local_changed(ClipData::text("r"), false);
    holds_for("switched-off device is ignored", 400, || {
        a.host.writes().len() == 1
    });
    a.svc.set_peer_enabled(p.b.id(), true).unwrap();
    b.svc.local_changed(ClipData::text("s"), false);
    wait_until("switched on again", WAIT, || a.host.writes().len() == 2);
    assert!(b.host.writes().is_empty());
}

#[test]
fn deny_on_the_host_means_nothing_is_sent_or_written() {
    let (p, a, b) = pair();
    p.b.set(Capability::Clipboard, Mode::Deny);
    wait_until("a sees deny", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Clipboard) == Some(Mode::Deny)
    });
    // a knows b is off, so it does not even try.
    assert_eq!(
        a.svc.local_changed(ClipData::text("x"), false),
        Submit::Skipped(SkipReason::NoDevice)
    );
    holds_for("nothing written", 300, || b.host.writes().is_empty());
}

#[test]
fn always_ask_prompts_the_host_once_per_item() {
    let (p, a, b) = pair();
    p.b.set(Capability::Clipboard, Mode::Ask);
    wait_until("a sees ask", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Clipboard) == Some(Mode::Ask)
    });
    a.svc.local_changed(ClipData::text("first"), false);
    let prompt = p.b.answer_prompt(false);
    assert_eq!(prompt.capability, "clipboard.v1");
    wait_until("refusal shown", WAIT, || {
        a.events
            .has(|e| matches!(e, ClipEvent::Refused { reason, .. } if reason == "declined"))
    });
    assert!(b.host.writes().is_empty());

    a.svc.local_changed(ClipData::text("second"), false);
    p.b.answer_prompt(true);
    wait_until("approved item written", WAIT, || b.host.writes().len() == 1);
    assert_eq!(b.host.writes()[0], ClipData::text("second"));

    // A large item asks first too, then streams.
    a.svc
        .local_changed(ClipData::png(png(2000, 2000, 2 * 1024 * 1024)), false);
    p.b.answer_prompt(true);
    wait_until("large approved item", WAIT, || b.host.writes().len() == 2);
}

#[test]
fn a_stale_allow_falls_back_to_asking() {
    // a may still believe b's setting is Allow when b has just changed it;
    // the refusal `ask` makes a request approval and send the item again.
    let (p, a, b) = pair();
    p.b.set(Capability::Clipboard, Mode::Ask);
    a.svc.local_changed(ClipData::text("late"), false);
    p.b.answer_prompt(true);
    wait_until("written after approval", WAIT, || {
        b.host.writes().len() == 1
    });
}

#[test]
fn hostile_items_are_not_written() {
    let (p, a, b) = pair();
    let ctx = a.svc.test_ctx().unwrap().clone();
    let to = p.b.id();
    let send = |clip: Clip| ctx.send(to, Direction::ToHost, 0, &clip).unwrap();
    let id = |n: u8| vec![n; 16];

    // Wrong type, not a PNG, bad PNG size, invalid UTF-8: dropped quietly.
    send(Clip {
        id: id(1),
        mime: "application/x-sh".into(),
        bytes: b"rm -rf /".to_vec(),
        ..Default::default()
    });
    send(Clip {
        id: id(2),
        mime: mime::PNG.into(),
        bytes: b"not a png".to_vec(),
        ..Default::default()
    });
    send(Clip {
        id: id(3),
        mime: mime::PNG.into(),
        bytes: png(60_000, 60_000, 100),
        ..Default::default()
    });
    send(Clip {
        id: id(4),
        mime: mime::TEXT.into(),
        bytes: vec![0xff, 0xfe, 0xfd],
        ..Default::default()
    });
    holds_for("hostile items dropped", 400, || b.host.writes().is_empty());
    assert!(p.a.connected_to(&p.b));

    // A good item with a repeated id is written once.
    for _ in 0..3 {
        send(Clip {
            id: id(9),
            mime: mime::TEXT.into(),
            bytes: b"once".to_vec(),
            ..Default::default()
        });
    }
    wait_until("written", WAIT, || !b.host.writes().is_empty());
    holds_for("only once", 300, || b.host.writes().len() == 1);

    // Pushed data for the controller side is never written.
    ctx.send(
        to,
        Direction::ToController,
        0,
        &Clip {
            id: id(10),
            mime: mime::TEXT.into(),
            bytes: b"push".to_vec(),
            ..Default::default()
        },
    )
    .unwrap();
    holds_for("controller side is not a write", 300, || {
        b.host.writes().len() == 1
    });

    // An oversize inline item breaks the protocol and closes the session.
    send(Clip {
        id: id(11),
        mime: mime::TEXT.into(),
        bytes: vec![b'a'; 600 * 1024],
        ..Default::default()
    });
    wait_until("session closed", WAIT, || !p.a.connected_to(&p.b));
}

#[test]
fn a_flood_is_rate_limited() {
    let (p, a, b) = pair();
    let ctx = a.svc.test_ctx().unwrap().clone();
    for n in 0..60u8 {
        ctx.send(
            p.b.id(),
            Direction::ToHost,
            0,
            &Clip {
                id: vec![n; 16],
                mime: mime::TEXT.into(),
                bytes: vec![b'a'; 10],
                ..Default::default()
            },
        )
        .unwrap();
    }
    wait_until("some arrived", WAIT, || !b.host.writes().is_empty());
    std::thread::sleep(Duration::from_millis(500));
    assert!(b.host.writes().len() <= 20, "{}", b.host.writes().len());
}

#[test]
fn sending_without_a_connection_is_a_clean_skip() {
    let d = Dev::new("Alone");
    let s = side(&d);
    assert_eq!(
        s.svc.local_changed(ClipData::text("x"), false),
        Submit::Skipped(SkipReason::NoDevice)
    );
}

#[test]
fn settings_are_kept_in_the_store() {
    let (p, a, _b) = pair();
    a.svc.set_paused(true).unwrap();
    a.svc.set_peer_enabled(p.b.id(), false).unwrap();
    let ctx = a.svc.test_ctx().unwrap();
    assert_eq!(ctx.setting("paused").as_deref(), Some("1"));
    assert_eq!(
        ctx.setting(&format!("peer.{}", p.b.id().hex())).as_deref(),
        Some("0")
    );
}
