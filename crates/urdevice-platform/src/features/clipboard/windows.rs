//! Windows clipboard back-end (design §9.1): `AddClipboardFormatListener`
//! through `clipboard-win`'s `Monitor`, the standard formats for text, the
//! registered `HTML Format` and `PNG` formats, and `CF_DIB` for screenshots.
//!
//! Sensitive content (R7.3) is recognised by the formats password managers
//! and Windows itself use to opt out of monitoring, history, and cloud sync.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use clipboard_win::{Clipboard as Open, Monitor, SysResult, formats, raw, register_format};
use urdevice_core::services::clipboard::{ClipData, Clipboard};
use urdevice_core::{CoreError, Result};
use urdevice_proto::features::clipboard::{MAX_CLIP_BYTES, mime};

use super::common::{Snapshot, cf_html_extract, cf_html_wrap, dib_to_png, png_to_dib};
use super::{Backend, unsupported};

/// How often a busy clipboard (another app holds it open) is retried.
const OPEN_ATTEMPTS: usize = 40;

/// Formats that mark content as not to be monitored (R7.3).
const EXCLUDE_FROM_MONITOR: &str = "ExcludeClipboardContentFromMonitorProcessing";
/// DWORD formats: 0 means "not for history" or "not for the cloud".
const HISTORY: &str = "CanIncludeInClipboardHistory";
const CLOUD: &str = "CanUploadToCloudClipboard";
const HTML_FORMAT: &str = "HTML Format";
const PNG_FORMAT: &str = "PNG";

fn format(name: &str) -> Option<u32> {
    register_format(name).map(std::num::NonZeroU32::get)
}

/// True when the open clipboard carries a sensitive mark.
fn is_sensitive() -> bool {
    if format(EXCLUDE_FROM_MONITOR).is_some_and(raw::is_format_avail) {
        return true;
    }
    [HISTORY, CLOUD].into_iter().any(|name| {
        let Some(f) = format(name).filter(|f| raw::is_format_avail(*f)) else {
            return false;
        };
        let mut v = Vec::new();
        raw::get_vec(f, &mut v).is_ok() && v.len() >= 4 && v[..4] == [0, 0, 0, 0]
    })
}

/// Read the clipboard. `Err(true)`: sensitive, nothing read.
fn read() -> std::result::Result<Snapshot, bool> {
    let _open = Open::new_attempts(OPEN_ATTEMPTS).map_err(|_| false)?;
    if is_sensitive() {
        return Err(true);
    }
    let mut snap = Snapshot::default();
    if raw::is_format_avail(formats::CF_UNICODETEXT) {
        let mut b = Vec::new();
        if raw::get_string(&mut b).is_ok() {
            snap.text = String::from_utf8(b).ok();
        }
    }
    if let Some(f) = format(HTML_FORMAT).filter(|f| raw::is_format_avail(*f)) {
        let mut b = Vec::new();
        if raw::get_vec(f, &mut b).is_ok() && b.len() <= MAX_CLIP_BYTES * 2 {
            snap.html = cf_html_extract(&b);
        }
    }
    if snap.text.is_none() && snap.html.is_none() {
        let png = format(PNG_FORMAT).filter(|f| raw::is_format_avail(*f));
        let mut b = Vec::new();
        if let Some(f) = png
            && raw::get_vec(f, &mut b).is_ok()
        {
            snap.png = Some(b);
        } else if raw::is_format_avail(formats::CF_DIB) {
            b.clear();
            if raw::get_vec(formats::CF_DIB, &mut b).is_ok() && b.len() <= MAX_CLIP_BYTES * 4 {
                snap.png = dib_to_png(&b);
            }
        }
    }
    Ok(snap)
}

fn write_all(d: &ClipData) -> SysResult<()> {
    match d.mime.as_str() {
        mime::PNG => {
            if let Some(f) = format(PNG_FORMAT) {
                raw::set(f, &d.bytes)?;
            }
            // Apps that predate the PNG format read a DIB.
            if let Some(dib) = png_to_dib(&d.bytes) {
                raw::set_without_clear(formats::CF_DIB, &dib)?;
            }
        }
        mime::HTML => {
            // The plain text first: `set_string` clears the clipboard.
            raw::set_string(d.alt_text.as_deref().unwrap_or(""))?;
            if let Some(f) = format(HTML_FORMAT) {
                let html = String::from_utf8_lossy(&d.bytes);
                raw::set_without_clear(f, &cf_html_wrap(&html))?;
            }
        }
        _ => raw::set_string_with(
            &String::from_utf8_lossy(&d.bytes),
            clipboard_win::options::DoClear,
        )?,
    }
    Ok(())
}

pub(crate) struct Windows {
    /// Sequence number of our last write, so the watcher skips it.
    own_seq: Arc<AtomicU32>,
}

impl Backend for Windows {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn write(&self, data: &ClipData) -> Result<()> {
        let io = |e: &dyn std::fmt::Debug| CoreError::Network(format!("windows clipboard: {e:?}"));
        let _open = Open::new_attempts(OPEN_ATTEMPTS).map_err(|e| io(&e))?;
        write_all(data).map_err(|e| io(&e))?;
        if let Some(n) = raw::seq_num() {
            self.own_seq.store(n.get(), Ordering::SeqCst);
        }
        Ok(())
    }
}

pub(crate) fn start(svc: Arc<Clipboard>) -> Result<Arc<dyn Backend>> {
    let own_seq = Arc::new(AtomicU32::new(0));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let seq = own_seq.clone();
    std::thread::Builder::new()
        .name("clipboard-windows".into())
        .spawn(move || {
            let mut monitor = match Monitor::new() {
                Ok(m) => {
                    let _ = ready_tx.send(true);
                    m
                }
                Err(e) => {
                    tracing::info!(error = ?e, "no Windows clipboard listener");
                    let _ = ready_tx.send(false);
                    return;
                }
            };
            while let Ok(true) = monitor.recv() {
                if raw::seq_num().is_some_and(|n| n.get() == seq.load(Ordering::SeqCst)) {
                    continue;
                }
                // Some apps put the data on after announcing the change.
                std::thread::sleep(Duration::from_millis(20));
                match read() {
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
    if !ready_rx.recv().unwrap_or(false) {
        return Err(unsupported());
    }
    Ok(Arc::new(Windows { own_seq }))
}
