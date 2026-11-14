//! Linux adapters, split into X11 and Wayland back-ends.
//!
//! Wayland uses the RemoteDesktop and ScreenCast portals first and falls back
//! to `uinput` for input on compositors without the portal (design §9.3, §17).

use crate::Platform;

mod wayland;
mod x11;

/// The kind of graphical session this process runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// An X11 session (or XWayland without a Wayland socket).
    X11,
    /// A Wayland session.
    Wayland,
}

impl Session {
    /// Detect the session from the process environment.
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
        )
    }

    /// Pick a session from `XDG_SESSION_TYPE` and whether `WAYLAND_DISPLAY` is set.
    ///
    /// `XDG_SESSION_TYPE` wins when it names a known session. Otherwise a
    /// Wayland socket means Wayland, and anything else is treated as X11.
    pub fn from_env(session_type: Option<&str>, has_wayland_display: bool) -> Self {
        match session_type.map(str::to_ascii_lowercase).as_deref() {
            Some("wayland") => Self::Wayland,
            Some("x11") => Self::X11,
            _ if has_wayland_display => Self::Wayland,
            _ => Self::X11,
        }
    }
}

pub(crate) fn platform() -> Platform {
    match Session::detect() {
        Session::X11 => x11::platform(),
        Session::Wayland => wayland::platform(),
    }
}

#[cfg(test)]
mod tests {
    use super::Session;

    #[test]
    fn session_type_wins() {
        assert_eq!(Session::from_env(Some("wayland"), false), Session::Wayland);
        assert_eq!(Session::from_env(Some("x11"), true), Session::X11);
        assert_eq!(Session::from_env(Some("Wayland"), false), Session::Wayland);
    }

    #[test]
    fn falls_back_to_wayland_socket() {
        assert_eq!(Session::from_env(None, true), Session::Wayland);
        assert_eq!(Session::from_env(Some("tty"), true), Session::Wayland);
        assert_eq!(Session::from_env(None, false), Session::X11);
    }
}
