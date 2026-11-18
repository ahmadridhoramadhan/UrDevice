//! Desktop QR scanner (R3.10, design §7.5): camera frames from `nokhwa`,
//! decoding with `rqrr`.
//!
//! The camera runs only while the scan window is open. Frames stay in memory:
//! they are shown in the window and decoded, never written or sent. Dropping
//! the [`Scanner`] stops the camera.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use slint::ComponentHandle;

use crate::ui::ScanWindow;

/// True when the OS lists at least one camera. "Scan QR" is hidden otherwise.
pub(crate) fn available() -> bool {
    if std::env::var_os("URDEVICE_NO_CAMERA").is_some() {
        return false;
    }
    nokhwa::query(ApiBackend::Auto)
        .map(|c| !c.is_empty())
        .unwrap_or(false)
}

/// A running scan: the window plus the camera thread.
pub(crate) struct Scanner {
    stop: Arc<AtomicBool>,
    window: ScanWindow,
}

impl Drop for Scanner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.window.hide();
    }
}

impl Scanner {
    /// Open the camera. `on_code` runs on the UI thread with the first QR text found.
    pub(crate) fn open(on_code: impl Fn(String) + 'static) -> Result<Self, String> {
        let window = ScanWindow::new().map_err(|e| e.to_string())?;
        window.set_status("Starting the camera…".into());
        let stop = Arc::new(AtomicBool::new(false));
        let on_code: Arc<dyn Fn(String)> = Arc::new(on_code);
        // `on_code` is not Send; keep it on the UI thread behind a thread-local slot.
        CALLBACK.with(|c| *c.borrow_mut() = Some(on_code));

        let weak = window.as_weak();
        let stop_cancel = stop.clone();
        window.on_cancel(move || {
            stop_cancel.store(true, Ordering::Relaxed);
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
        window.show().map_err(|e| e.to_string())?;

        let weak = window.as_weak();
        let stop_thread = stop.clone();
        std::thread::Builder::new()
            .name("urdevice-camera".into())
            .spawn(move || run(weak, stop_thread))
            .map_err(|e| e.to_string())?;
        Ok(Self { stop, window })
    }
}

type Callback = Arc<dyn Fn(String)>;

thread_local! {
    static CALLBACK: std::cell::RefCell<Option<Callback>> = const { std::cell::RefCell::new(None) };
}

fn status(weak: &slint::Weak<ScanWindow>, text: String) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            w.set_status(text.into());
        }
    });
}

fn run(weak: slint::Weak<ScanWindow>, stop: Arc<AtomicBool>) {
    // Uncompressed frames: MJPEG decoding would pull in mozjpeg (IJG license).
    let format = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
        CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30),
    ));
    let mut cam = match nokhwa::Camera::new(CameraIndex::Index(0), format) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "camera open failed");
            status(&weak, camera_help());
            return;
        }
    };
    if let Err(e) = cam.open_stream() {
        tracing::warn!(error = %e, "camera stream failed");
        status(&weak, camera_help());
        return;
    }
    status(
        &weak,
        "Hold the other device's QR code in front of the camera.".into(),
    );
    while !stop.load(Ordering::Relaxed) {
        let Ok(frame) = cam.frame() else { continue };
        let Ok(rgb) = frame.decode_image::<RgbFormat>() else {
            continue;
        };
        let (w, h) = (rgb.width(), rgb.height());
        let raw = rgb.into_raw();
        let found = decode(&raw, w, h);
        let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
        buf.make_mut_bytes()
            .copy_from_slice(&raw[..(w * h * 3) as usize]);
        let weak2 = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = weak2.upgrade() {
                win.set_frame(slint::Image::from_rgb8(buf));
            }
            if let Some(text) = found {
                let cb = CALLBACK.with(|c| c.borrow_mut().take());
                if let Some(cb) = cb {
                    cb(text);
                }
            }
        });
    }
    let _ = cam.stop_stream();
}

/// Find a UrDevice pairing code in one RGB frame.
fn decode(rgb: &[u8], w: u32, h: u32) -> Option<String> {
    let (wu, hu) = (w as usize, h as usize);
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(wu, hu, |x, y| {
        let i = (y * wu + x) * 3;
        let (r, g, b) = (rgb[i] as u32, rgb[i + 1] as u32, rgb[i + 2] as u32);
        ((r * 30 + g * 59 + b * 11) / 100) as u8
    });
    img.detect_grids()
        .into_iter()
        .filter_map(|g| g.decode().ok().map(|(_, text)| text))
        .find(|t| t.starts_with(urdevice_core::pairing::QR_PREFIX))
}

fn camera_help() -> String {
    if cfg!(target_os = "windows") {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:privacy-webcam"])
            .spawn();
        "The camera is blocked. Allow desktop apps to use the camera in Settings, then try again."
            .into()
    } else if cfg!(target_os = "macos") {
        "The camera is blocked. Allow UrDevice in System Settings → Privacy & Security → Camera."
            .into()
    } else {
        "The camera can't be opened. Check that your user can read /dev/video*.".into()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_a_rendered_pairing_code() {
        let text = format!("{}{}", urdevice_core::pairing::QR_PREFIX, "A".repeat(120));
        let m = urdevice_core::qr::encode(&text).unwrap();
        let scale = 4usize;
        let quiet = 4usize;
        let side = (m.size as usize + quiet * 2) * scale;
        let mut rgb = vec![255u8; side * side * 3];
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = (
                    (x / scale).wrapping_sub(quiet),
                    (y / scale).wrapping_sub(quiet),
                );
                let n = m.size as usize;
                if mx < n && my < n && m.modules[my * n + mx] {
                    rgb[(y * side + x) * 3..(y * side + x) * 3 + 3].fill(0);
                }
            }
        }
        assert_eq!(
            super::decode(&rgb, side as u32, side as u32).as_deref(),
            Some(text.as_str())
        );
    }
}
