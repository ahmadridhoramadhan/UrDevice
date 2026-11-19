//! Media control (#20, design §9.4): desktop host adapters.
//!
//! [`host`] returns the [`MediaHost`] for the OS this process runs on:
//! MPRIS over D-Bus on Linux, the System Media Transport Controls plus Core
//! Audio on Windows, and media keys plus the system volume on macOS. Each
//! reads the active sessions and the system volume, and acts on the commands
//! the core service passes in with its `Grant`.

use std::sync::Arc;

use urdevice_core::services::media::MediaHost;

pub(crate) mod artwork;
mod parse;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// The media adapter for this OS. On an OS without one the host reports no
/// sessions with the reason `unsupported`.
pub fn host() -> Arc<dyn MediaHost> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(linux::Mpris::new())
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(windows::Smtc::new())
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::System::new())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Arc::new(Unsupported)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
struct Unsupported;

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
impl MediaHost for Unsupported {
    fn state(&self) -> urdevice_core::services::media::MediaState {
        urdevice_core::services::media::MediaState {
            unavailable: "unsupported".into(),
            ..Default::default()
        }
    }

    fn command(
        &self,
        _grant: &urdevice_core::services::Grant,
        _session_id: &str,
        _kind: urdevice_core::services::media::CmdKind,
        _seek_ms: u64,
    ) -> urdevice_core::Result<()> {
        Err(urdevice_core::CoreError::unsupported("media", "this OS"))
    }

    fn volume(
        &self,
        _grant: &urdevice_core::services::Grant,
        _change: &urdevice_core::services::media::VolumeChange,
    ) -> urdevice_core::Result<()> {
        Err(urdevice_core::CoreError::unsupported("media", "this OS"))
    }
}
