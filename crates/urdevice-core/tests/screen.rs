//! Screen sharing end to end between two in-process nodes (design §10, R12).

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness::{Pair, WAIT, block_on, holds_for, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::screen::{
    Capture, Display, EncodedFrame, FrameSink, Screen, ScreenEvents, ScreenHost, VideoFrame,
    VideoTarget,
};
use urdevice_core::{Capability, CoreError, Mode};

#[derive(Default)]
struct FakeCapture {
    stopped: AtomicBool,
    keyframes: AtomicU32,
    targets: Mutex<Vec<VideoTarget>>,
}

impl Capture for FakeCapture {
    fn request_keyframe(&self) {
        self.keyframes.fetch_add(1, Ordering::Relaxed);
    }
    fn set_target(&self, target: VideoTarget) {
        self.targets.lock().unwrap().push(target);
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
}

/// A host with two displays that pushes a keyframe and then P-frames every
/// 10 ms, or fails when `fail` is set.
#[derive(Default)]
struct FakeHost {
    grants: Mutex<Vec<(Grant, u32, VideoTarget)>>,
    captures: Mutex<Vec<Arc<FakeCapture>>>,
    fail: Mutex<Option<&'static str>>,
    die_after: Mutex<Option<u32>>,
}

impl ScreenHost for FakeHost {
    fn displays(&self) -> Vec<Display> {
        vec![
            Display {
                id: 1,
                name: "Built-in".into(),
                width: 1920,
                height: 1080,
                primary: true,
            },
            Display {
                id: 2,
                name: "External".into(),
                width: 2560,
                height: 1440,
                primary: false,
            },
        ]
    }

    fn start(
        &self,
        grant: &Grant,
        display_id: u32,
        target: VideoTarget,
        sink: Arc<dyn FrameSink>,
    ) -> urdevice_core::Result<Arc<dyn Capture>> {
        if let Some(r) = *self.fail.lock().unwrap() {
            return Err(CoreError::Refused(r.into()));
        }
        self.grants
            .lock()
            .unwrap()
            .push((grant.clone(), display_id, target));
        let cap = Arc::new(FakeCapture::default());
        self.captures.lock().unwrap().push(cap.clone());
        let c = cap.clone();
        let die_after = *self.die_after.lock().unwrap();
        std::thread::spawn(move || {
            let mut n: u64 = 0;
            while !c.stopped.load(Ordering::Relaxed) {
                if die_after.is_some_and(|d| n >= u64::from(d)) {
                    sink.failed("capture_failed");
                    break;
                }
                if sink.ready() {
                    sink.push(EncodedFrame {
                        pts_us: n * 10_000,
                        keyframe: n == 0,
                        data: vec![0, 0, 0, 1, display_id as u8, n as u8],
                        width: 640,
                        height: 360,
                    });
                    n += 1;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(cap)
    }
}

#[derive(Default)]
struct Viewer {
    opened: Mutex<Vec<u64>>,
    displays: Mutex<Vec<Display>>,
    started: Mutex<Vec<u32>>,
    frames: Mutex<Vec<VideoFrame>>,
    errors: Mutex<Vec<String>>,
    ended: Mutex<Vec<u64>>,
}

impl ScreenEvents for Viewer {
    fn opened(&self, _peer: DeviceId, activity: u64) {
        self.opened.lock().unwrap().push(activity);
    }
    fn displays(&self, _peer: DeviceId, _activity: u64, displays: Vec<Display>) {
        *self.displays.lock().unwrap() = displays;
    }
    fn stream_started(&self, _peer: DeviceId, _activity: u64, display_id: u32) {
        self.started.lock().unwrap().push(display_id);
    }
    fn frame(&self, _peer: DeviceId, _activity: u64, frame: VideoFrame) {
        self.frames.lock().unwrap().push(frame);
    }
    fn error(&self, _peer: DeviceId, _activity: u64, reason: &str) {
        self.errors.lock().unwrap().push(reason.into());
    }
    fn ended(&self, _peer: DeviceId, activity: u64) {
        self.ended.lock().unwrap().push(activity);
    }
}

fn setup() -> (Pair, Arc<FakeHost>, Arc<Viewer>) {
    let p = Pair::new();
    let host = Arc::new(FakeHost::default());
    p.a.node.service::<Screen>().unwrap().host.set(host.clone());
    let viewer = Arc::new(Viewer::default());
    p.b.node
        .service::<Screen>()
        .unwrap()
        .events
        .set(viewer.clone());
    (p, host, viewer)
}

/// Open a session from `b` on `a`, answering `a`'s Always Ask prompt.
fn open(p: &Pair) -> u64 {
    let svc = p.b.node.service::<Screen>().unwrap();
    let a_id = p.a.id();
    let req = std::thread::spawn(move || block_on(svc.open(a_id)));
    let prompt = p.a.answer_prompt(true);
    assert_eq!(prompt.capability, Capability::Screen.as_str());
    req.join().unwrap().unwrap()
}

#[test]
fn view_after_approval_streams_frames_with_indicator() {
    let (p, host, viewer) = setup();
    assert_eq!(p.a.snap().activities.len(), 0);
    let activity = open(&p);
    let svc = p.b.node.service::<Screen>().unwrap();
    let a = p.a.id();

    // The host shows the indicator (R5.4).
    wait_until("indicator", WAIT, || {
        p.a.snap()
            .activities
            .iter()
            .any(|x| x.incoming && x.capability == Capability::Screen.as_str())
    });

    svc.list_displays(a, activity).unwrap();
    wait_until("displays", WAIT, || viewer.displays.lock().unwrap().len() == 2);

    svc.start(a, activity, 2, 1280, 720, 0).unwrap();
    wait_until("frames", WAIT, || viewer.frames.lock().unwrap().len() >= 5);
    {
        let g = host.grants.lock().unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].0.activity(), activity);
        assert_eq!(g[0].0.peer(), p.b.id());
        assert_eq!(g[0].1, 2);
        assert_eq!((g[0].2.max_width, g[0].2.max_height), (1280, 720));
    }
    {
        let f = viewer.frames.lock().unwrap();
        assert!(f[0].keyframe);
        assert_eq!((f[0].width, f[0].height), (640, 360));
        assert!(!f[1].keyframe);
        assert_eq!(f[1].width, 0);
        assert!(f.windows(2).all(|w| w[0].pts_us < w[1].pts_us));
    }
    assert_eq!(*viewer.started.lock().unwrap(), vec![2]);

    // Keyframe requests reach the encoder, rate-limited.
    let cap = host.captures.lock().unwrap()[0].clone();
    for _ in 0..20 {
        svc.request_keyframe(a, activity).unwrap();
    }
    wait_until("keyframe", WAIT, || {
        cap.keyframes.load(Ordering::Relaxed) >= 1
    });
    holds_for("keyframe rate limit", 200, || {
        cap.keyframes.load(Ordering::Relaxed) <= 2
    });

    // Switching the display replaces the stream.
    svc.start(a, activity, 1, 0, 0, 0).unwrap();
    wait_until("old capture stopped", WAIT, || {
        cap.stopped.load(Ordering::Relaxed)
    });
    wait_until("second stream", WAIT, || {
        *viewer.started.lock().unwrap() == vec![2, 1]
    });

    // Stop on the host ends the session on both devices.
    let id = p.a.snap().activities[0].id;
    p.a.node.stop_activity(id);
    wait_until("viewer ended", WAIT, || {
        viewer.ended.lock().unwrap().contains(&activity)
    });
    let cap2 = host.captures.lock().unwrap()[1].clone();
    wait_until("capture stopped", WAIT, || {
        cap2.stopped.load(Ordering::Relaxed)
    });
    wait_until("indicator gone", WAIT, || p.a.snap().activities.is_empty());
    assert!(!p.a.node.service::<Screen>().unwrap().is_sharing());
    // The ended session can no longer be used.
    assert!(svc.list_displays(a, activity).is_err());
}

#[test]
fn deny_and_decline_never_reach_the_host() {
    let (p, host, _viewer) = setup();
    let svc = p.b.node.service::<Screen>().unwrap();
    let a = p.a.id();

    let req = {
        let svc = svc.clone();
        std::thread::spawn(move || block_on(svc.open(a)))
    };
    p.a.answer_prompt(false);
    match req.join().unwrap() {
        Err(CoreError::Refused(r)) => assert_eq!(r, "declined"),
        other => panic!("{other:?}"),
    }

    p.a.set(Capability::Screen, Mode::Deny);
    wait_until("remote deny", WAIT, || {
        p.b.remote_mode(&p.a, Capability::Screen) == Some(Mode::Deny)
    });
    match block_on(svc.open(a)) {
        Err(CoreError::Refused(r)) => assert_eq!(r, "denied"),
        other => panic!("{other:?}"),
    }
    assert!(host.grants.lock().unwrap().is_empty());
    // No session, so nothing can be sent.
    assert!(svc.start(a, 1, 1, 0, 0, 0).is_err());
}

#[test]
fn allow_still_needs_an_activity_and_shows_the_indicator() {
    let (p, host, viewer) = setup();
    p.a.set(Capability::Screen, Mode::Allow);
    wait_until("remote allow", WAIT, || {
        p.b.remote_mode(&p.a, Capability::Screen) == Some(Mode::Allow)
    });
    let svc = p.b.node.service::<Screen>().unwrap();
    let a = p.a.id();
    let activity = block_on(svc.open(a)).unwrap();
    assert_ne!(activity, 0);
    svc.start(a, activity, 1, 0, 0, 0).unwrap();
    wait_until("frames", WAIT, || !viewer.frames.lock().unwrap().is_empty());
    assert!(
        p.a.snap()
            .activities
            .iter()
            .any(|x| x.incoming && x.capability == Capability::Screen.as_str())
    );
    assert_eq!(host.grants.lock().unwrap().len(), 1);

    // Stop from the viewer side.
    svc.stop(a, activity);
    let cap = host.captures.lock().unwrap()[0].clone();
    wait_until("capture stopped", WAIT, || {
        cap.stopped.load(Ordering::Relaxed)
    });
    wait_until("indicator gone", WAIT, || p.a.snap().activities.is_empty());
}

#[test]
fn capture_errors_reach_the_viewer_and_end_the_session() {
    let (p, host, viewer) = setup();
    let activity = open(&p);
    let svc = p.b.node.service::<Screen>().unwrap();
    let a = p.a.id();

    *host.fail.lock().unwrap() = Some("consent_declined");
    svc.start(a, activity, 1, 0, 0, 0).unwrap();
    wait_until("error", WAIT, || {
        viewer
            .errors
            .lock()
            .unwrap()
            .contains(&"consent_declined".to_string())
    });

    // Capture dying on its own ends the activity and the indicator.
    *host.fail.lock().unwrap() = None;
    *host.die_after.lock().unwrap() = Some(3);
    svc.start(a, activity, 1, 0, 0, 0).unwrap();
    wait_until("capture_failed", WAIT, || {
        viewer
            .errors
            .lock()
            .unwrap()
            .contains(&"capture_failed".to_string())
    });
    wait_until("ended", WAIT, || {
        viewer.ended.lock().unwrap().contains(&activity)
    });
    wait_until("indicator gone", WAIT, || p.a.snap().activities.is_empty());
}

#[test]
fn request_view_reports_through_events() {
    let (p, _host, viewer) = setup();
    let svc = p.b.node.service::<Screen>().unwrap();
    svc.request_view(p.a.id()).unwrap();
    p.a.answer_prompt(false);
    wait_until("declined", WAIT, || {
        viewer.errors.lock().unwrap().contains(&"declined".to_string())
    });
    svc.request_view(p.a.id()).unwrap();
    p.a.answer_prompt(true);
    wait_until("opened", WAIT, || viewer.opened.lock().unwrap().len() == 1);
    let activity = viewer.opened.lock().unwrap()[0];
    svc.list_displays(p.a.id(), activity).unwrap();
    wait_until("displays", WAIT, || viewer.displays.lock().unwrap().len() == 2);
}

#[test]
fn no_host_adapter_answers_no_host() {
    let (p, _host, viewer) = setup();
    p.a.node.service::<Screen>().unwrap().host.clear();
    let activity = open(&p);
    let svc = p.b.node.service::<Screen>().unwrap();
    svc.list_displays(p.a.id(), activity).unwrap();
    wait_until("no_host", WAIT, || {
        viewer
            .errors
            .lock()
            .unwrap()
            .contains(&"no_host".to_string())
    });
}
