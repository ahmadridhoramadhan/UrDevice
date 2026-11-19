//! macOS clipboard back-end (design §9.1): `NSPasteboard` polled every 500 ms
//! on `changeCount`, because the pasteboard has no change notification.
//!
//! Content that password managers mark with `org.nspasteboard.ConcealedType`
//! (or `TransientType`, used for one-time data) is never read or sent (R7.3).
//!
//! Untested on a real Mac (no device); it compiles in CI.

use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;

use objc2_app_kit::NSPasteboard;
use objc2_foundation::{NSData, NSString};
use urdevice_core::services::clipboard::{ClipData, Clipboard};
use urdevice_core::{CoreError, Result};
use urdevice_proto::features::clipboard::mime;

use super::Backend;
use super::common::Snapshot;

const POLL: Duration = Duration::from_millis(500);
const TEXT: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
const PNG: &str = "public.png";
const SENSITIVE: &[&str] = &[
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];

fn ns(s: &str) -> objc2::rc::Retained<NSString> {
    NSString::from_str(s)
}

/// Read the pasteboard. `Err(true)`: sensitive, nothing read.
fn read(pb: &NSPasteboard) -> std::result::Result<Snapshot, bool> {
    if let Some(types) = pb.types() {
        let sensitive = types
            .iter()
            .any(|t| SENSITIVE.contains(&t.to_string().as_str()));
        if sensitive {
            return Err(true);
        }
    }
    let text = pb.stringForType(&ns(TEXT)).map(|s| s.to_string());
    let html = pb
        .dataForType(&ns(HTML))
        .and_then(|d| String::from_utf8(d.to_vec()).ok());
    let png = if text.is_none() && html.is_none() {
        pb.dataForType(&ns(PNG)).map(|d| d.to_vec())
    } else {
        None
    };
    Ok(Snapshot { text, html, png })
}

pub(crate) struct MacOs {
    /// `changeCount` after our last write, so the watcher skips it.
    own_count: Arc<AtomicIsize>,
}

impl Backend for MacOs {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn write(&self, d: &ClipData) -> Result<()> {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ok = match d.mime.as_str() {
            mime::PNG => pb.setData_forType(Some(&NSData::with_bytes(&d.bytes)), &ns(PNG)),
            mime::HTML => {
                let html = pb.setData_forType(Some(&NSData::with_bytes(&d.bytes)), &ns(HTML));
                let text = d
                    .alt_text
                    .as_deref()
                    .is_none_or(|t| pb.setString_forType(&ns(t), &ns(TEXT)));
                html && text
            }
            _ => pb.setString_forType(&ns(&String::from_utf8_lossy(&d.bytes)), &ns(TEXT)),
        };
        self.own_count.store(pb.changeCount(), Ordering::SeqCst);
        if ok {
            Ok(())
        } else {
            Err(CoreError::Network("macos clipboard: write failed".into()))
        }
    }
}

pub(crate) fn start(svc: Arc<Clipboard>) -> Result<Arc<dyn Backend>> {
    let own_count = Arc::new(AtomicIsize::new(-1));
    let own = own_count.clone();
    std::thread::Builder::new()
        .name("clipboard-macos".into())
        .spawn(move || {
            let pb = NSPasteboard::generalPasteboard();
            let mut last = pb.changeCount();
            loop {
                std::thread::sleep(POLL);
                let now = pb.changeCount();
                if now == last {
                    continue;
                }
                last = now;
                if now == own.load(Ordering::SeqCst) {
                    continue;
                }
                match read(&pb) {
                    Err(true) => {
                        svc.local_changed(ClipData::text(""), true);
                    }
                    Ok(snap) => {
                        if let Some(clip) = snap.into_clip() {
                            svc.local_changed(clip, false);
                        }
                    }
                    Err(false) => {}
                }
            }
        })
        .map_err(CoreError::Io)?;
    Ok(Arc::new(MacOs { own_count }))
}
