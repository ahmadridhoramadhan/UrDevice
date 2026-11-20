//! X11 fallback: one `GetImage` of the monitor's rectangle on the root
//! window per frame, monitors from RandR.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use urdevice_core::Result;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

use super::super::{CaptureHandle, Display, FrameHandler, copy_rows, refused};

struct Monitor {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

fn monitors() -> Option<(Vec<Display>, Vec<Monitor>)> {
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let reply = conn.randr_get_monitors(root, true).ok()?.reply().ok()?;
    let mut displays = Vec::new();
    let mut rects = Vec::new();
    for (i, m) in reply.monitors.iter().enumerate() {
        let name = conn
            .get_atom_name(m.name)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.name).into_owned())
            .unwrap_or_else(|| format!("Display {}", i + 1));
        displays.push(Display {
            id: i as u32 + 1,
            name,
            width: u32::from(m.width),
            height: u32::from(m.height),
            primary: m.primary,
        });
        rects.push(Monitor {
            x: m.x,
            y: m.y,
            width: m.width,
            height: m.height,
        });
    }
    if displays.is_empty() {
        // No RandR monitors: the whole root window.
        let s = conn.setup().roots.get(screen)?;
        displays.push(Display {
            id: 1,
            name: "Screen".into(),
            width: u32::from(s.width_in_pixels),
            height: u32::from(s.height_in_pixels),
            primary: true,
        });
        rects.push(Monitor {
            x: 0,
            y: 0,
            width: s.width_in_pixels,
            height: s.height_in_pixels,
        });
    }
    Some((displays, rects))
}

pub(super) fn displays() -> Option<Vec<Display>> {
    monitors().map(|(d, _)| d)
}

pub(super) fn start(id: u32, fps: u32, handler: Arc<dyn FrameHandler>) -> Result<CaptureHandle> {
    let (_, rects) = monitors().ok_or_else(|| refused("capture_failed"))?;
    let index = (id as usize)
        .checked_sub(1)
        .ok_or_else(|| refused("no_display"))?;
    let rect = rects.into_iter().nth(index).ok_or_else(|| refused("no_display"))?;
    let (conn, screen) = x11rb::connect(None).map_err(|_| refused("capture_failed"))?;
    let root = conn
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| refused("capture_failed"))?
        .root;
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let every = Duration::from_secs(1) / fps;
    std::thread::Builder::new()
        .name("x11-capture".into())
        .spawn(move || {
            let mut next = Instant::now();
            while !stop2.load(Ordering::Relaxed) {
                let now = Instant::now();
                if now < next {
                    std::thread::sleep(next - now);
                }
                next += every;
                if !handler.wants() {
                    continue;
                }
                let reply = conn
                    .get_image(
                        ImageFormat::Z_PIXMAP,
                        root,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        !0,
                    )
                    .ok()
                    .and_then(|c| c.reply().ok());
                let Some(img) = reply else {
                    handler.ended("capture_failed");
                    return;
                };
                // Depth 24 and 32 use 32 bits per pixel, BGRx in memory.
                if img.depth != 24 && img.depth != 32 {
                    handler.ended("capture_failed");
                    return;
                }
                let (w, h) = (u32::from(rect.width), u32::from(rect.height));
                if let Some(f) = copy_rows(&img.data, w, h, w as usize * 4, false) {
                    handler.frame(f);
                }
            }
        })
        .map_err(|_| refused("capture_failed"))?;
    Ok(CaptureHandle::new(move || stop.store(true, Ordering::Relaxed)))
}
