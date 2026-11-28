//! Phone as microphone end to end between two in-process nodes (design §18.1, R17.4).

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness::{Dev, Pair, WAIT, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::mic::{Mic, MicCapture, MicEvents, MicHost, MicSink};
use urdevice_core::{Capability, CoreError, Mode};

#[derive(Default)]
struct FakeCapture {
    stopped: AtomicBool,
}

impl MicCapture for FakeCapture {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

/// A phone that records a counting pattern, or refuses.
#[derive(Default)]
struct FakePhone {
    refuse: Mutex<Option<&'static str>>,
    captures: Mutex<Vec<Arc<FakeCapture>>>,
    sinks: Mutex<Vec<Arc<dyn MicSink>>>,
}

impl MicHost for FakePhone {
    fn start(
        &self,
        _grant: &Grant,
        sink: Arc<dyn MicSink>,
    ) -> urdevice_core::Result<Arc<dyn MicCapture>> {
        if let Some(r) = *self.refuse.lock().unwrap() {
            return Err(CoreError::Refused(r.into()));
        }
        self.sinks.lock().unwrap().push(sink.clone());
        let cap = Arc::new(FakeCapture::default());
        self.captures.lock().unwrap().push(cap.clone());
        let c = cap.clone();
        std::thread::spawn(move || {
            let mut n: u8 = 0;
            while !c.stopped.load(Ordering::Relaxed) {
                if sink.ready() {
                    // 20 ms of audio: 960 samples of 16 bits.
                    sink.push(vec![n; 1920]);
                    n = n.wrapping_add(1);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Ok(cap)
    }
}

#[derive(Default)]
struct Listener {
    opened: Mutex<Vec<u64>>,
    started: Mutex<u32>,
    chunks: Mutex<Vec<Vec<u8>>>,
    errors: Mutex<Vec<String>>,
    ended: Mutex<Vec<u64>>,
}

impl MicEvents for Listener {
    fn opened(&self, _peer: DeviceId, activity: u64) {
        self.opened.lock().unwrap().push(activity);
    }
    fn stream_started(&self, _peer: DeviceId, _activity: u64) {
        *self.started.lock().unwrap() += 1;
    }
    fn chunk(&self, _peer: DeviceId, _activity: u64, pcm: Vec<u8>) {
        self.chunks.lock().unwrap().push(pcm);
    }
    fn error(&self, _peer: DeviceId, _activity: u64, reason: &str) {
        self.errors.lock().unwrap().push(reason.into());
    }
    fn ended(&self, _peer: DeviceId, activity: u64) {
        self.ended.lock().unwrap().push(activity);
    }
}

/// The laptop (`a`) listens to the phone (`b`).
fn setup() -> (Pair, Arc<FakePhone>, Arc<Listener>) {
    let p = Pair::with(
        Dev::new("Laptop"),
        Dev::with("Phone", "android", Vec::new()),
    );
    let phone = Arc::new(FakePhone::default());
    p.b.node.service::<Mic>().unwrap().host.set(phone.clone());
    let listener = Arc::new(Listener::default());
    p.a.node
        .service::<Mic>()
        .unwrap()
        .events
        .set(listener.clone());
    wait_until("laptop sees the microphone", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Mic).is_some()
    });
    (p, phone, listener)
}

/// Ask from the laptop, answering the phone's Always Ask prompt.
fn listen(p: &Pair) {
    let svc = p.a.node.service::<Mic>().unwrap();
    svc.request_listen(p.b.id()).unwrap();
    let prompt = p.b.answer_prompt(true);
    assert_eq!(prompt.capability, Capability::Mic.as_str());
}

#[test]
fn the_phone_streams_audio_after_approval_with_the_indicator() {
    let (p, _phone, listener) = setup();
    listen(&p);
    wait_until("indicator", WAIT, || {
        p.b.snap()
            .activities
            .iter()
            .any(|x| x.incoming && x.capability == Capability::Mic.as_str())
    });
    wait_until("chunks", WAIT, || {
        listener.chunks.lock().unwrap().len() >= 5
    });
    assert_eq!(*listener.started.lock().unwrap(), 1);
    let chunks = listener.chunks.lock().unwrap();
    assert!(chunks.iter().all(|c| c.len() == 1920));
    // Chunks arrive in order.
    assert!(
        chunks
            .windows(2)
            .all(|w| w[1][0] == w[0][0].wrapping_add(1))
    );
}

#[test]
fn stopping_ends_the_recording_on_both_devices() {
    let (p, phone, listener) = setup();
    listen(&p);
    wait_until("chunks", WAIT, || {
        !listener.chunks.lock().unwrap().is_empty()
    });
    let activity = listener.opened.lock().unwrap()[0];
    p.a.node.service::<Mic>().unwrap().stop(p.b.id(), activity);
    let cap = phone.captures.lock().unwrap()[0].clone();
    wait_until("recording stopped", WAIT, || {
        cap.stopped.load(Ordering::Relaxed)
    });
    wait_until("listener told", WAIT, || {
        listener.ended.lock().unwrap().contains(&activity)
    });
    assert!(!p.b.node.service::<Mic>().unwrap().is_recording());
}

#[test]
fn a_refused_recording_is_reported_and_leaves_no_session() {
    let (p, phone, listener) = setup();
    *phone.refuse.lock().unwrap() = Some("consent_declined");
    listen(&p);
    wait_until("reason", WAIT, || {
        listener
            .errors
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "consent_declined")
    });
    assert!(listener.chunks.lock().unwrap().is_empty());
}

#[test]
fn recording_that_dies_ends_the_activity() {
    let (p, phone, listener) = setup();
    listen(&p);
    wait_until("chunks", WAIT, || {
        !listener.chunks.lock().unwrap().is_empty()
    });
    phone.sinks.lock().unwrap()[0].failed("capture_failed");
    wait_until("reason", WAIT, || {
        listener
            .errors
            .lock()
            .unwrap()
            .iter()
            .any(|e| e == "capture_failed")
    });
    wait_until("ended", WAIT, || !listener.ended.lock().unwrap().is_empty());
}

#[test]
fn deny_on_the_phone_refuses_the_microphone() {
    let (p, _phone, listener) = setup();
    p.b.set(Capability::Mic, Mode::Deny);
    wait_until("laptop sees deny", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Mic) == Some(Mode::Deny)
    });
    p.a.node
        .service::<Mic>()
        .unwrap()
        .request_listen(p.b.id())
        .unwrap();
    wait_until("refused", WAIT, || {
        !listener.errors.lock().unwrap().is_empty()
    });
    assert!(listener.opened.lock().unwrap().is_empty());
}
