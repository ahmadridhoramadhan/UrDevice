//! Desktop adapters for the UrDevice core.
//!
//! Each supported OS has its own module, picked with `#[cfg(target_os = ...)]`.
//! Linux is split again into X11 and Wayland back-ends, chosen at run time.
//! For now every adapter is a stub that reports [`CoreError::Unsupported`];
//! the feature phases replace them one by one.
//!
//! [`CoreError::Unsupported`]: urdevice_core::CoreError::Unsupported

use std::fmt;
use std::sync::Arc;

use urdevice_core::platform::{Clipboard, FileStore, InputSink, MediaHost, Overlay, ScreenSource};

mod secrets;
mod stub;
mod system;

pub use secrets::DesktopSecretStore;
pub use system::{battery, data_dir, device_name, platform_id};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// The set of platform adapters for the running desktop.
#[derive(Clone)]
pub struct Platform {
    /// Name of the platform or back-end, for example `windows` or `linux-wayland`.
    pub name: &'static str,
    /// System clipboard.
    pub clipboard: Arc<dyn Clipboard>,
    /// Input injection.
    pub input: Arc<dyn InputSink>,
    /// Screen capture.
    pub screen: Arc<dyn ScreenSource>,
    /// Media control.
    pub media: Arc<dyn MediaHost>,
    /// Presenter overlay.
    pub overlay: Arc<dyn Overlay>,
    /// Incoming files and gallery.
    pub files: Arc<dyn FileStore>,
}

impl fmt::Debug for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Platform")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Adapters for the desktop this process runs on.
pub fn current() -> Platform {
    #[cfg(target_os = "windows")]
    {
        windows::platform()
    }
    #[cfg(target_os = "macos")]
    {
        macos::platform()
    }
    #[cfg(target_os = "linux")]
    {
        linux::platform()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        stub::platform("unsupported")
    }
}

#[cfg(test)]
mod tests {
    use urdevice_core::CoreError;
    use urdevice_core::platform::PermStatus;

    use super::*;

    #[test]
    fn current_platform_has_a_name() {
        assert!(!current().name.is_empty());
    }

    #[test]
    fn stubs_report_unsupported() {
        let platform = current();
        assert!(matches!(
            platform.clipboard.read(),
            Err(CoreError::Unsupported { .. })
        ));
        assert_eq!(platform.input.permission_status(), PermStatus::Unsupported);
        assert!(platform.screen.displays().is_empty());
        assert!(platform.media.now_playing().is_none());
        assert!(platform.files.gallery().is_none());
    }
}
