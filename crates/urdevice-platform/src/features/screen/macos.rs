//! macOS capture with ScreenCaptureKit (design §10). Compiles in CI;
//! untested at run time (no macOS device).
//!
//! The app needs the Screen Recording permission. Without it
//! `SCShareableContent` fails; the first attempt makes macOS show its own
//! prompt (`CGRequestScreenCaptureAccess`) and the share is refused with
//! `consent_declined` until the user allows it in System Settings.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_graphics::{
    CGMainDisplayID, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType,
};
use urdevice_core::Result;

use super::{CaptureHandle, Display, FrameHandler, copy_rows, refused};

/// How long to wait for ScreenCaptureKit's completion handlers.
const WAIT: Duration = Duration::from_secs(10);

struct Shareable(Retained<SCShareableContent>);
// SAFETY: SCShareableContent is immutable once delivered.
unsafe impl Send for Shareable {}

fn shareable() -> Option<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel::<Option<Shareable>>();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |content: *mut SCShareableContent, _err: *mut NSError| {
        // SAFETY: the content pointer is valid for the call and retained here.
        let c = unsafe { Retained::retain(content) }.map(Shareable);
        if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
            let _ = tx.send(c);
        }
    });
    // SAFETY: the block lives until the handler ran (it is retained by SCK).
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    rx.recv_timeout(WAIT).ok().flatten().map(|s| s.0)
}

fn display_list(content: &SCShareableContent) -> Vec<Retained<SCDisplay>> {
    // SAFETY: plain property read.
    let arr = unsafe { content.displays() };
    (0..arr.count()).map(|i| arr.objectAtIndex(i)).collect()
}

pub(super) fn displays() -> Vec<Display> {
    if !CGPreflightScreenCaptureAccess() {
        // Without the permission the list would be empty; offer the main
        // display so the viewer can ask, which shows macOS's prompt.
        return vec![Display {
            id: CGMainDisplayID(),
            name: "Main display".into(),
            width: 0,
            height: 0,
            primary: true,
        }];
    }
    let Some(content) = shareable() else {
        return Vec::new();
    };
    let main = CGMainDisplayID();
    display_list(&content)
        .iter()
        .enumerate()
        .map(|(i, d)| {
            // SAFETY: plain property reads.
            let (id, w, h) = unsafe { (d.displayID(), d.width(), d.height()) };
            Display {
                id,
                name: format!("Display {}", i + 1),
                width: w.max(0) as u32,
                height: h.max(0) as u32,
                primary: id == main,
            }
        })
        .collect()
}

struct Ivars {
    handler: Arc<dyn FrameHandler>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class adds no
    // `dealloc` and only implements the SCStreamOutput protocol.
    #[unsafe(super(NSObject))]
    #[name = "UrDeviceScreenOutput"]
    #[ivars = Ivars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            let handler = &self.ivars().handler;
            if !handler.wants() {
                return;
            }
            // SAFETY: the sample buffer is valid for the call; the pixel
            // buffer is locked read-only while its memory is copied.
            unsafe {
                let Some(pb) = sample.image_buffer() else { return };
                if CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly) != 0 {
                    return;
                }
                let base = CVPixelBufferGetBaseAddress(&pb) as *const u8;
                let stride = CVPixelBufferGetBytesPerRow(&pb);
                let (w, h) = (CVPixelBufferGetWidth(&pb), CVPixelBufferGetHeight(&pb));
                if !base.is_null() && w > 0 && h > 0 {
                    let bytes = std::slice::from_raw_parts(base, stride * h);
                    if let Some(f) = copy_rows(bytes, w as u32, h as u32, stride, false) {
                        handler.frame(f);
                    }
                }
                CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags::ReadOnly);
            }
        }
    }
);

impl Output {
    fn new(handler: Arc<dyn FrameHandler>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { handler });
        // SAFETY: NSObject's init on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

struct Running {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
}

// SAFETY: SCStream is thread-safe; it is only stopped from here.
unsafe impl Send for Running {}

pub(super) fn start(id: u32, fps: u32, handler: Arc<dyn FrameHandler>) -> Result<CaptureHandle> {
    if !CGPreflightScreenCaptureAccess() {
        let _ = CGRequestScreenCaptureAccess();
        return Err(refused("consent_declined"));
    }
    let content = shareable().ok_or_else(|| refused("consent_declined"))?;
    let display = display_list(&content)
        .into_iter()
        // SAFETY: plain property read.
        .find(|d| unsafe { d.displayID() } == id)
        .ok_or_else(|| refused("no_display"))?;
    // SAFETY: standard ScreenCaptureKit set-up; objects are retained for
    // the life of the capture.
    unsafe {
        let filter = SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &NSArray::new(),
        );
        let config = SCStreamConfiguration::new();
        // Points to pixels: ask for twice the size on Retina; SCK scales
        // down to what the display has.
        config.setWidth((display.width().max(1) as usize) * 2);
        config.setHeight((display.height().max(1) as usize) * 2);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setShowsCursor(true);
        config.setQueueDepth(3);
        config.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: fps.max(1) as i32,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        });
        let stream =
            SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None);
        let output = Output::new(handler);
        let queue = DispatchQueue::new("urdevice.screen", None);
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
            .map_err(|_| refused("capture_failed"))?;
        let (tx, rx) = mpsc::channel::<bool>();
        let tx = Mutex::new(Some(tx));
        let done = RcBlock::new(move |err: *mut NSError| {
            if let Some(tx) = tx.lock().ok().and_then(|mut g| g.take()) {
                let _ = tx.send(err.is_null());
            }
        });
        stream.startCaptureWithCompletionHandler(Some(&done));
        if !rx.recv_timeout(WAIT).unwrap_or(false) {
            return Err(refused("capture_failed"));
        }
        tracing::info!(display = id, "ScreenCaptureKit started");
        let running = Running {
            stream,
            _output: output,
            _queue: queue,
        };
        Ok(CaptureHandle::new(move || {
            running.stream.stopCaptureWithCompletionHandler(None);
            drop(running);
        }))
    }
}
