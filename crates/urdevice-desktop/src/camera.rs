//! Desktop QR scanner (R3.10, design §7.5): camera frames from `nokhwa`,
//! decoding with `rqrr`.
//!
//! The camera runs only while the scan window is open. Frames stay in memory:
//! they are shown in the window and decoded, never written or sent. Dropping
//! the [`Scanner`] stops the camera.

use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
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

/// Formats `nokhwa` turns into RGB itself.
const UNCOMPRESSED: [FrameFormat; 4] = [
    FrameFormat::YUYV,
    FrameFormat::NV12,
    FrameFormat::RAWRGB,
    FrameFormat::RAWBGR,
];
/// The size the scanner aims for: enough for a QR code, cheap to decode.
const TARGET: (u32, u32) = (640, 480);
/// Uncompressed frames wider than this are slow over USB; MJPEG wins then.
const MAX_UNCOMPRESSED_WIDTH: u32 = 1280;
/// Largest MJPEG frame side the decoder accepts.
const MAX_JPEG_SIDE: usize = 4096;
/// Frames in a row that fail to decode before the user is told.
const BAD_FRAME_LIMIT: u32 = 60;

/// Pick the format to scan with (#47): an uncompressed format near 640x480
/// if the camera has one, else MJPEG near 640x480. Many webcams offer only
/// MJPEG, or no uncompressed format at 640x480.
fn pick_format(formats: &[CameraFormat]) -> Option<CameraFormat> {
    let rank = |f: &CameraFormat| {
        let (w, h) = (f.resolution().x(), f.resolution().y());
        let tier = match f.format() {
            x if UNCOMPRESSED.contains(&x) && w <= MAX_UNCOMPRESSED_WIDTH => 0,
            FrameFormat::MJPEG => 1,
            _ => 2,
        };
        let dx = i64::from(w) - i64::from(TARGET.0);
        let dy = i64::from(h) - i64::from(TARGET.1);
        (tier, dx * dx + dy * dy, f.frame_rate().abs_diff(30))
    };
    formats
        .iter()
        .filter(|f| f.format() == FrameFormat::MJPEG || UNCOMPRESSED.contains(&f.format()))
        .min_by_key(|f| rank(f))
        .copied()
}

/// One camera frame as packed RGB, with its width and height.
fn to_rgb(frame: &nokhwa::Buffer) -> Option<(Vec<u8>, u32, u32)> {
    if frame.source_frame_format() == FrameFormat::MJPEG {
        return decode_mjpeg(frame.buffer());
    }
    let img = frame.decode_image::<RgbFormat>().ok()?;
    let (w, h) = (img.width(), img.height());
    Some((img.into_raw(), w, h))
}

/// An APP0 segment reading "AVI1": it tells the decoder the frame is MJPEG.
const AVI1_APP0: [u8; 9] = [0xff, 0xe0, 0x00, 0x07, b'A', b'V', b'I', b'1', 0x00];

/// True when the JPEG carries its own Huffman tables (a DHT segment before
/// the scan data).
fn has_dht(jpeg: &[u8]) -> bool {
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xff {
        match jpeg[i + 1] {
            0xc4 => return true,
            0xda => return false,
            _ => i += 2 + usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]])),
        }
    }
    false
}

/// Webcam MJPEG often leaves out the Huffman tables. The decoder fills in
/// the standard ones only for frames marked "AVI1", so mark frames that
/// have none.
fn with_mjpeg_mark(jpeg: &[u8]) -> Cow<'_, [u8]> {
    if !jpeg.starts_with(&[0xff, 0xd8]) || has_dht(jpeg) {
        return Cow::Borrowed(jpeg);
    }
    let mut out = Vec::with_capacity(jpeg.len() + AVI1_APP0.len());
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&AVI1_APP0);
    out.extend_from_slice(&jpeg[2..]);
    Cow::Owned(out)
}

/// Decode an MJPEG frame in pure Rust.
fn decode_mjpeg(data: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(MAX_JPEG_SIDE)
        .set_max_height(MAX_JPEG_SIDE);
    let data = with_mjpeg_mark(data);
    let mut d =
        zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(data.as_ref()), options);
    let rgb = d.decode().ok()?;
    let (w, h) = d.dimensions()?;
    if rgb.len() != w.checked_mul(h)?.checked_mul(3)? {
        return None;
    }
    Some((rgb, u32::try_from(w).ok()?, u32::try_from(h).ok()?))
}

fn open_camera(index: u32) -> Result<nokhwa::Camera, nokhwa::NokhwaError> {
    let any = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
    let mut cam = nokhwa::Camera::new(CameraIndex::Index(index), any)?;
    match cam.compatible_camera_formats().map(|f| pick_format(&f)) {
        Ok(Some(f)) => {
            let exact = RequestedFormat::new::<RgbFormat>(RequestedFormatType::Exact(f));
            if let Err(e) = cam.set_camera_requset(exact) {
                tracing::debug!(error = %e, "keeping the camera's own format");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!(error = %e, "camera formats unknown"),
    }
    let f = cam.camera_format();
    tracing::info!(
        format = %f.format(),
        width = f.resolution().x(),
        height = f.resolution().y(),
        "camera format"
    );
    Ok(cam)
}

fn run(weak: slint::Weak<ScanWindow>, stop: Arc<AtomicBool>) {
    let mut cam = match open_camera(0) {
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
    let mut bad_frames = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let Some((raw, w, h)) = cam.frame().ok().as_ref().and_then(to_rgb) else {
            bad_frames += 1;
            if bad_frames == BAD_FRAME_LIMIT {
                tracing::warn!(format = %cam.camera_format().format(), "camera frames can't be read");
                status(
                    &weak,
                    "This camera's picture can't be read. Try another camera.".into(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
            continue;
        };
        bad_frames = 0;
        let found = decode(&raw, w, h);
        let mut buf = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(w, h);
        buf.make_mut_bytes().copy_from_slice(&raw);
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
    use nokhwa::utils::{CameraFormat, FrameFormat, Resolution};

    use super::pick_format;

    /// A 320x240 4:2:2 JPEG of a QR code, like an MJPEG webcam frame.
    const QR_JPEG: &[u8] = include_bytes!("../tests/fixtures/qr-320x240.jpg");

    fn fmt(w: u32, h: u32, f: FrameFormat, fps: u32) -> CameraFormat {
        CameraFormat::new(Resolution::new(w, h), f, fps)
    }

    #[test]
    fn picks_uncompressed_near_640x480() {
        let formats = [
            fmt(1920, 1080, FrameFormat::MJPEG, 30),
            fmt(640, 480, FrameFormat::MJPEG, 30),
            fmt(800, 600, FrameFormat::YUYV, 15),
            fmt(640, 480, FrameFormat::YUYV, 30),
        ];
        assert_eq!(
            pick_format(&formats),
            Some(fmt(640, 480, FrameFormat::YUYV, 30))
        );
    }

    #[test]
    fn falls_back_to_mjpeg_when_there_is_no_yuyv() {
        let formats = [
            fmt(1920, 1080, FrameFormat::MJPEG, 30),
            fmt(1280, 720, FrameFormat::MJPEG, 30),
            fmt(640, 480, FrameFormat::MJPEG, 30),
            fmt(640, 480, FrameFormat::GRAY, 30),
        ];
        assert_eq!(
            pick_format(&formats),
            Some(fmt(640, 480, FrameFormat::MJPEG, 30))
        );
    }

    #[test]
    fn prefers_small_mjpeg_over_huge_yuyv() {
        let formats = [
            fmt(1920, 1080, FrameFormat::YUYV, 5),
            fmt(640, 360, FrameFormat::MJPEG, 30),
        ];
        assert_eq!(
            pick_format(&formats),
            Some(fmt(640, 360, FrameFormat::MJPEG, 30))
        );
    }

    #[test]
    fn yuyv_at_another_size_is_still_used() {
        // nokhwa's own `Closest` request fails here: it looks up frame rates
        // at the requested size, not the one it found.
        let formats = [fmt(1280, 720, FrameFormat::YUYV, 10)];
        assert_eq!(pick_format(&formats), Some(formats[0]));
        assert_eq!(pick_format(&[]), None);
    }

    #[test]
    fn decodes_an_mjpeg_frame_and_finds_the_code() {
        let (rgb, w, h) = super::decode_mjpeg(QR_JPEG).unwrap();
        assert_eq!((w, h), (320, 240));
        assert_eq!(
            super::decode(&rgb, w, h).as_deref(),
            Some("urdevice://pair?d=TEST-FIXTURE-0123456789")
        );
    }

    #[test]
    fn decodes_mjpeg_without_huffman_tables() {
        // Many webcams send MJPEG frames with no DHT segment.
        let stripped = strip_dht(QR_JPEG);
        assert!(super::has_dht(QR_JPEG));
        assert!(!super::has_dht(&stripped));
        let (rgb, w, h) = super::decode_mjpeg(&stripped).unwrap();
        assert!(super::decode(&rgb, w, h).is_some());
    }

    #[test]
    fn broken_mjpeg_is_skipped() {
        assert!(super::decode_mjpeg(&QR_JPEG[..200]).is_none());
        assert!(super::decode_mjpeg(b"not a jpeg").is_none());
    }

    /// Drop every DHT (0xFFC4) segment before the scan data.
    fn strip_dht(jpeg: &[u8]) -> Vec<u8> {
        let mut out = jpeg[..2].to_vec();
        let mut i = 2;
        while i + 4 <= jpeg.len() && jpeg[i] == 0xff {
            let marker = jpeg[i + 1];
            let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
            if marker == 0xda {
                break;
            }
            if marker != 0xc4 {
                out.extend_from_slice(&jpeg[i..i + 2 + len]);
            }
            i += 2 + len;
        }
        out.extend_from_slice(&jpeg[i..]);
        out
    }

    /// Manual check with a real camera: hold a UrDevice QR code in front of it.
    /// `cargo test -p urdevice-desktop real_camera -- --ignored --nocapture`
    /// Set `URDEVICE_TEST_CAMERA` to use another camera index than 0.
    #[test]
    #[ignore = "needs a camera"]
    fn real_camera_finds_a_code() {
        let index = std::env::var("URDEVICE_TEST_CAMERA")
            .ok()
            .and_then(|i| i.parse().ok())
            .unwrap_or(0);
        let mut cam = super::open_camera(index).unwrap();
        let f = cam.camera_format();
        println!("format {} {}", f.format(), f.resolution());
        cam.open_stream().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut frames = 0;
        while std::time::Instant::now() < deadline {
            let Some((rgb, w, h)) = cam.frame().ok().as_ref().and_then(super::to_rgb) else {
                continue;
            };
            frames += 1;
            if let Some(text) = super::decode(&rgb, w, h) {
                println!("found after {frames} frames: {} chars", text.len());
                return;
            }
        }
        panic!("no code found in {frames} frames");
    }

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
