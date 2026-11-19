//! Clipboard sharing (#16): desktop OS adapters (design §9.1, R7).
//!
//! One back-end per OS watches the clipboard and writes to it:
//!
//! | OS | Watch | Sensitive mark |
//! |---|---|---|
//! | Windows | `AddClipboardFormatListener` (through `clipboard-win`) | `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory`, `CanUploadToCloudClipboard` |
//! | macOS | `NSPasteboard.changeCount`, polled every 500 ms | `org.nspasteboard.ConcealedType` |
//! | Linux, X11 and Wayland through XWayland | XFixes selection events | `x-kde-passwordManagerHint` |
//!
//! [`attach`] connects the back-end to the core's
//! [`Clipboard`] service: changes go to
//! [`Clipboard::local_changed`], incoming items are written through the
//! service's [`ClipboardHost`]. Clipboard content is never logged.

use std::sync::Arc;

use urdevice_core::services::Grant;
use urdevice_core::services::clipboard::{ClipData, Clipboard, ClipboardHost};
use urdevice_core::{CoreError, Result};

mod common;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod x11;

/// What every OS back-end provides. Its watcher thread starts in `start` and
/// calls [`Clipboard::local_changed`] on its own.
pub(crate) trait Backend: Send + Sync {
    /// Name for logs and the UI.
    fn name(&self) -> &'static str;
    /// Replace the system clipboard. The back-end must not report this write
    /// back as a local change where the OS lets it tell (the core also
    /// recognises it by hash).
    fn write(&self, data: &ClipData) -> Result<()>;
}

pub(crate) fn unsupported() -> CoreError {
    CoreError::Unsupported {
        feature: "clipboard",
        platform: std::env::consts::OS,
    }
}

struct Host(Arc<dyn Backend>);

impl ClipboardHost for Host {
    fn write(&self, _grant: &Grant, data: &ClipData) -> Result<()> {
        self.0.write(data)
    }
}

/// The running adapter. Keep it for as long as the app runs.
pub struct DesktopClipboard {
    backend: Arc<dyn Backend>,
}

impl DesktopClipboard {
    /// The back-end's name, for example `x11` or `windows`.
    pub fn name(&self) -> &'static str {
        self.backend.name()
    }
}

/// Connect the desktop clipboard to the core service: start watching, and
/// make the service write incoming items here. Returns an error, and leaves
/// the service without a host, where the OS offers no usable clipboard
/// interface (for example a Wayland session without XWayland).
pub fn attach(svc: &Arc<Clipboard>) -> Result<DesktopClipboard> {
    #[cfg(target_os = "linux")]
    let backend = x11::start(svc.clone())?;
    #[cfg(target_os = "windows")]
    let backend = windows::start(svc.clone())?;
    #[cfg(target_os = "macos")]
    let backend = macos::start(svc.clone())?;
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    let backend: Arc<dyn Backend> = {
        let _ = svc;
        return Err(unsupported());
    };
    svc.host.set(Arc::new(Host(backend.clone())));
    tracing::info!(backend = backend.name(), "desktop clipboard attached");
    Ok(DesktopClipboard { backend })
}
