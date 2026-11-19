//! Clipboard sharing (#16): bindings for the Android app.
//!
//! Kotlin writes incoming items through [`ClipboardHostFfi`], listens through
//! [`ClipboardListener`], and hands content to send (notification action,
//! Quick Settings tile, share sheet, foreground changes) to the
//! `clipboard_send_*` methods. The sensitive mark (`EXTRA_IS_SENSITIVE`) goes
//! in with the content, and the core drops sensitive content in one place.

use std::sync::Arc;

use urdevice_core::services::Grant;
use urdevice_core::services::clipboard::{
    ClipData, ClipEvent, Clipboard, ClipboardEvents, ClipboardHost, SkipReason, Submit,
};
use urdevice_core::{CoreError, Result as CoreResult};

use crate::{FfiError, UrDeviceNode, device_id};

/// Writes an item to the system clipboard. Return false when it failed.
#[uniffi::export(with_foreign)]
pub trait ClipboardHostFfi: Send + Sync {
    /// `mime` is `text/plain`, `text/html`, or `image/png`. `alt_text` is the
    /// plain-text version of an HTML item.
    fn write(&self, mime: String, bytes: Vec<u8>, alt_text: Option<String>) -> bool;
}

/// Told what happened, never the content.
#[uniffi::export(with_foreign)]
pub trait ClipboardListener: Send + Sync {
    /// Runs on a core thread; post UI work to the main thread.
    fn on_event(&self, event: FfiClipEvent);
}

/// What happened to the clipboard. No content.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FfiClipEvent {
    /// `received`, `sent`, `skipped`, `refused`, or `settings`.
    pub kind: String,
    /// The other device (hex), or empty.
    pub device_id: String,
    /// MIME type for `received` and `sent`.
    pub mime: String,
    /// Size in bytes for `received` and `sent`.
    pub len: u64,
    /// For `skipped`: `paused`, `sensitive`, `too_large`, `unsupported`,
    /// `no_device`. For `refused`: the peer's reason.
    pub reason: String,
}

/// What became of content handed over to send.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FfiClipSubmit {
    /// Delivery to this many devices started.
    Sending { count: u32 },
    /// Not sent: `paused`, `sensitive`, `too_large`, `unsupported`, `no_device`.
    Skipped { reason: String },
    /// It is what this device just received or sent.
    Echo,
}

fn skip_name(r: SkipReason) -> &'static str {
    match r {
        SkipReason::Paused => "paused",
        SkipReason::Sensitive => "sensitive",
        SkipReason::TooLarge => "too_large",
        SkipReason::Unsupported => "unsupported",
        SkipReason::NoDevice => "no_device",
    }
}

impl From<Submit> for FfiClipSubmit {
    fn from(s: Submit) -> Self {
        match s {
            Submit::Sending(n) => Self::Sending {
                count: u32::try_from(n).unwrap_or(u32::MAX),
            },
            Submit::Skipped(r) => Self::Skipped {
                reason: skip_name(r).into(),
            },
            Submit::Echo => Self::Echo,
        }
    }
}

struct Host(Arc<dyn ClipboardHostFfi>);

impl ClipboardHost for Host {
    fn write(&self, _grant: &Grant, data: &ClipData) -> CoreResult<()> {
        if self
            .0
            .write(data.mime.clone(), data.bytes.clone(), data.alt_text.clone())
        {
            Ok(())
        } else {
            Err(CoreError::Unsupported {
                feature: "clipboard",
                platform: "android",
            })
        }
    }
}

struct Listener(Arc<dyn ClipboardListener>);

impl ClipboardEvents for Listener {
    fn on_event(&self, e: &ClipEvent) {
        let event =
            |kind: &str, peer: Option<String>, mime: &str, len: usize, reason: &str| FfiClipEvent {
                kind: kind.into(),
                device_id: peer.unwrap_or_default(),
                mime: mime.into(),
                len: len as u64,
                reason: reason.into(),
            };
        self.0.on_event(match e {
            ClipEvent::Received { peer, mime, len } => {
                event("received", Some(peer.hex()), mime, *len, "")
            }
            ClipEvent::Sent { peer, mime, len } => event("sent", Some(peer.hex()), mime, *len, ""),
            ClipEvent::Skipped(r) => event("skipped", None, "", 0, skip_name(*r)),
            ClipEvent::Refused { peer, reason } => {
                event("refused", Some(peer.hex()), "", 0, reason)
            }
            ClipEvent::SettingsChanged => event("settings", None, "", 0, ""),
        });
    }
}

impl UrDeviceNode {
    fn clipboard(&self) -> Result<Arc<Clipboard>, FfiError> {
        self.node.service::<Clipboard>().ok_or(FfiError::NotFound)
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Set the Kotlin side that writes incoming items to the system clipboard.
    pub fn clipboard_set_host(&self, host: Arc<dyn ClipboardHostFfi>) {
        if let Ok(c) = self.clipboard() {
            c.host.set(Arc::new(Host(host)));
        }
    }

    /// Set the listener for clipboard events.
    pub fn clipboard_set_listener(&self, listener: Arc<dyn ClipboardListener>) {
        if let Ok(c) = self.clipboard() {
            c.events.set(Arc::new(Listener(listener)));
        }
    }

    /// Send text, or rich text with its plain-text version. `device` is one
    /// device, or `None` for every connected one. `sensitive` is the source's
    /// own mark (`EXTRA_IS_SENSITIVE`): such content is never sent.
    /// `automatic` is true for a change the app noticed on its own (not a
    /// user action): content just received from a peer is then not sent back.
    pub fn clipboard_send_text(
        &self,
        text: String,
        html: Option<String>,
        sensitive: bool,
        device: Option<String>,
        automatic: bool,
    ) -> Result<FfiClipSubmit, FfiError> {
        let peer = device.as_deref().map(device_id).transpose()?;
        let data = match html {
            Some(h) if !h.trim().is_empty() => {
                ClipData::html(h, Some(text).filter(|t| !t.is_empty()))
            }
            _ => ClipData::text(text),
        };
        Ok(self
            .clipboard()?
            .submit(data, sensitive, peer, automatic)
            .into())
    }

    /// Send a PNG image. See [`UrDeviceNode::clipboard_send_text`].
    pub fn clipboard_send_png(
        &self,
        png: Vec<u8>,
        sensitive: bool,
        device: Option<String>,
        automatic: bool,
    ) -> Result<FfiClipSubmit, FfiError> {
        let peer = device.as_deref().map(device_id).transpose()?;
        Ok(self
            .clipboard()?
            .submit(ClipData::png(png), sensitive, peer, automatic)
            .into())
    }

    /// True while "Pause clipboard on all devices" is on.
    pub fn clipboard_paused(&self) -> bool {
        self.clipboard().is_ok_and(|c| c.paused())
    }

    /// Turn "Pause clipboard on all devices" on or off.
    pub fn clipboard_set_paused(&self, paused: bool) -> Result<(), FfiError> {
        Ok(self.clipboard()?.set_paused(paused)?)
    }

    /// True when clipboard sync with `device` is on.
    pub fn clipboard_peer_enabled(&self, device: String) -> Result<bool, FfiError> {
        Ok(self.clipboard()?.peer_enabled(device_id(&device)?))
    }

    /// Turn clipboard sync with `device` on or off.
    pub fn clipboard_set_peer_enabled(
        &self,
        device: String,
        enabled: bool,
    ) -> Result<(), FfiError> {
        Ok(self
            .clipboard()?
            .set_peer_enabled(device_id(&device)?, enabled)?)
    }
}
