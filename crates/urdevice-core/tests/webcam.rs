//! Phone as webcam end to end between two in-process nodes (design §18.1, R17).

#![allow(clippy::unwrap_used)]

mod harness;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness::{Dev, Pair, WAIT, block_on, wait_until};
use urdevice_core::identity::DeviceId;
use urdevice_core::services::Grant;
use urdevice_core::services::screen::{
    Capture, Display, EncodedFrame, FrameSink, ScreenEvents, ScreenHost, VideoFrame, VideoTarget,
};
use urdevice_core::services::webcam::Camera;
use urdevice_core::{Capability, Mode};

#[derive(Default)]
struct FakeCapture {
    stopped: AtomicBool,
    controls: Mutex<Vec<(bool, u32)>>,
}

impl Capture for FakeCapture {
    fn request_keyframe(&self) {}
    fn set_target(&self, _target: VideoTarget) {}
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }
    fn control(&self, flash: bool, zoom_x100: u32) {
        self.controls.lock().unwrap().push((flash, zoom_x100));
    }
}

/// A phone with a back and a front camera.
#[derive(Default)]
struct FakePhone {
    started: Mutex<Vec<u32>>,
    captures: Mutex<Vec<Arc<FakeCapture>>>,
}

impl ScreenHost for FakePhone {
    fn displays(&self) -> Vec<Display> {
        vec![
            Display {
                id: 0,
                name: "Back camera".into(),
                width: 0,
                height: 0,
                primary: true,
            },
            Display {
                id: 1,
                name: "Front camera".into(),
                width: 0,
                height: 0,
                primary: false,
            },
        ]
    }

    fn start(
        &self,
        _grant: &Grant,
        display_id: u32,
        _target: VideoTarget,
        sink: Arc<dyn FrameSink>,
    ) -> urdevice_core::Result<Arc<dyn Capture>> {
        self.started.lock().unwrap().push(display_id);
        let cap = Arc::new(FakeCapture::default());
        self.captures.lock().unwrap().push(cap.clone());
        let c = cap.clone();
        std::thread::spawn(move || {
            let mut n: u64 = 0;
            while !c.stopped.load(Ordering::Relaxed) {
                if sink.ready() {
                    sink.push(EncodedFrame {
                        pts_us: n * 10_000,
                        keyframe: n == 0,
                        data: vec![0, 0, 0, 1, n as u8],
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
    displays: Mutex<Vec<Display>>,
    frames: Mutex<Vec<VideoFrame>>,
    ended: Mutex<Vec<u64>>,
}

impl ScreenEvents for Viewer {
    fn opened(&self, _peer: DeviceId, _activity: u64) {}
    fn displays(&self, _peer: DeviceId, _activity: u64, displays: Vec<Display>) {
        *self.displays.lock().unwrap() = displays;
    }
    fn stream_started(&self, _peer: DeviceId, _activity: u64, _display_id: u32) {}
    fn frame(&self, _peer: DeviceId, _activity: u64, frame: VideoFrame) {
        self.frames.lock().unwrap().push(frame);
    }
    fn error(&self, _peer: DeviceId, _activity: u64, _reason: &str) {}
    fn ended(&self, _peer: DeviceId, activity: u64) {
        self.ended.lock().unwrap().push(activity);
    }
}

/// The laptop (`a`) views the phone (`b`).
fn setup() -> (Pair, Arc<FakePhone>, Arc<Viewer>) {
    let p = Pair::with(
        Dev::new("Laptop"),
        Dev::with("Phone", "android", Vec::new()),
    );
    let phone = Arc::new(FakePhone::default());
    p.b.node
        .service::<Camera>()
        .unwrap()
        .screen()
        .host
        .set(phone.clone());
    let viewer = Arc::new(Viewer::default());
    p.a.node
        .service::<Camera>()
        .unwrap()
        .screen()
        .events
        .set(viewer.clone());
    wait_until("laptop sees the camera", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Camera).is_some()
    });
    (p, phone, viewer)
}

/// Open a camera session from the laptop, answering the phone's Always Ask prompt.
fn open(p: &Pair) -> u64 {
    let svc = p.a.node.service::<Camera>().unwrap();
    let phone = p.b.id();
    let req = std::thread::spawn(move || block_on(svc.screen().open(phone)));
    let prompt = p.b.answer_prompt(true);
    assert_eq!(prompt.capability, Capability::Camera.as_str());
    req.join().unwrap().unwrap()
}

#[test]
fn the_phone_streams_a_camera_after_approval_with_the_indicator() {
    let (p, phone, viewer) = setup();
    let activity = open(&p);
    let screen = p.a.node.service::<Camera>().unwrap().screen().clone();
    let phone_id = p.b.id();

    // The phone shows the indicator and Stop (R5.4, R17.6).
    wait_until("indicator", WAIT, || {
        p.b.snap()
            .activities
            .iter()
            .any(|x| x.incoming && x.capability == Capability::Camera.as_str())
    });

    screen.list_displays(phone_id, activity).unwrap();
    wait_until("cameras", WAIT, || {
        viewer.displays.lock().unwrap().len() == 2
    });
    assert!(viewer.displays.lock().unwrap()[0].primary);

    screen.start(phone_id, activity, 1, 1280, 720, 30).unwrap();
    wait_until("frames", WAIT, || viewer.frames.lock().unwrap().len() >= 3);
    assert_eq!(*phone.started.lock().unwrap(), [1]);
    assert!(viewer.frames.lock().unwrap()[0].keyframe);

    // Switching the camera starts a second stream in the same activity.
    screen.start(phone_id, activity, 0, 1280, 720, 30).unwrap();
    wait_until("switched", WAIT, || {
        *phone.started.lock().unwrap() == [1, 0]
    });
}

#[test]
fn flash_and_zoom_reach_the_running_camera_and_are_clamped() {
    let (p, phone, viewer) = setup();
    let activity = open(&p);
    let screen = p.a.node.service::<Camera>().unwrap().screen().clone();
    let phone_id = p.b.id();
    screen.start(phone_id, activity, 0, 1280, 720, 30).unwrap();
    wait_until("frames", WAIT, || !viewer.frames.lock().unwrap().is_empty());
    let cap = phone.captures.lock().unwrap()[0].clone();

    screen.control(phone_id, activity, true, 250).unwrap();
    wait_until("control", WAIT, || {
        *cap.controls.lock().unwrap() == [(true, 250)]
    });
    std::thread::sleep(Duration::from_millis(80));
    // Over the limit: clamped by the viewer, and again by the phone.
    screen.control(phone_id, activity, false, u32::MAX).unwrap();
    wait_until("clamped", WAIT, || {
        cap.controls.lock().unwrap().last() == Some(&(false, 1000))
    });
}

#[test]
fn stopping_ends_the_stream_on_both_devices() {
    let (p, phone, viewer) = setup();
    let activity = open(&p);
    let screen = p.a.node.service::<Camera>().unwrap().screen().clone();
    screen.start(p.b.id(), activity, 0, 0, 0, 0).unwrap();
    wait_until("frames", WAIT, || !viewer.frames.lock().unwrap().is_empty());
    let cap = phone.captures.lock().unwrap()[0].clone();
    screen.stop(p.b.id(), activity);
    wait_until("capture stopped", WAIT, || {
        cap.stopped.load(Ordering::Relaxed)
    });
    wait_until("viewer told", WAIT, || {
        viewer.ended.lock().unwrap().contains(&activity)
    });
}

#[test]
fn deny_on_the_phone_refuses_the_camera() {
    let (p, _phone, _viewer) = setup();
    p.b.set(Capability::Camera, Mode::Deny);
    wait_until("laptop sees deny", WAIT, || {
        p.a.remote_mode(&p.b, Capability::Camera) == Some(Mode::Deny)
    });
    let svc = p.a.node.service::<Camera>().unwrap();
    let phone_id = p.b.id();
    let r = block_on(svc.screen().open(phone_id));
    assert!(r.is_err(), "a denied camera never starts");
}
