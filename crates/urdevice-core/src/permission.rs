//! Capabilities and permission modes (R5, design §7.4).
//!
//! The setting is looked up by capability only and applies to every paired
//! device. The engine that enforces it lives in [`crate::node`], because it
//! needs the UI prompt channel; this module holds the pure parts.

use std::fmt;
use std::str::FromStr;

use urdevice_proto::WireMode;

/// A feature one device can request on another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Clipboard (`clipboard.v1`).
    Clipboard,
    /// Receive files (`files.v1`).
    Files,
    /// Gallery, Android hosts only (`gallery.v1`).
    Gallery,
    /// Remote control: pointer, keyboard, presenter (`input.v1`).
    Input,
    /// Media control (`media.v1`).
    Media,
    /// Screen sharing (`screen.h264.v1`).
    Screen,
    /// Localhost sharing (`tunnel.v1`).
    Tunnel,
    /// Remote terminal (`term.v1`). Changed only on the device itself.
    Terminal,
    /// File changes: upload, rename, delete (`fs.write.v1`). Changed only on the device itself.
    FsWrite,

    // Release 2 (design §18). Each one is listed in the settings and the
    // handshake only after its feature ships (see [`Capability::shipped`]).
    /// Notification mirroring (`notify.v1`).
    Notify,
    /// Find my device (`ring.v1`).
    Ring,
    /// Phone as webcam, Android hosts only (`camera.v1`).
    Camera,
    /// Phone as microphone, Android hosts only (`mic.v1`).
    Mic,
    /// SMS and calls, Android hosts only (`telephony.v1`).
    Telephony,
    /// Shared mouse and keyboard across desktops (`kvm.v1`).
    Kvm,
    /// Second display (`display.v1`).
    Display,
    /// Speaker: play another device's audio here (`audio.out.v1`).
    AudioOut,
    /// Shared clipboard history (`cliphist.v1`).
    ClipHistory,
    /// Proximity lock (`lock.v1`).
    Lock,
    /// Remote DevTools and ADB (`adb.v1`).
    Adb,
    /// Remote file browser, reading (`fs.browse.v1`).
    FsBrowse,
}

/// A permission mode. The UI labels are Allow, Always Ask, and Deny.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Start at once.
    Allow,
    /// Ask the user on this device every time.
    Ask,
    /// Refuse.
    Deny,
}

impl Capability {
    /// Every capability, in the order the UI lists them.
    pub const ALL: [Capability; 21] = [
        Self::Clipboard,
        Self::Files,
        Self::Gallery,
        Self::Input,
        Self::Media,
        Self::Screen,
        Self::Tunnel,
        Self::Notify,
        Self::Ring,
        Self::Camera,
        Self::Mic,
        Self::Telephony,
        Self::Kvm,
        Self::Display,
        Self::AudioOut,
        Self::ClipHistory,
        Self::Lock,
        Self::Adb,
        Self::FsBrowse,
        Self::Terminal,
        Self::FsWrite,
    ];

    /// The Release 1 capabilities. They are always listed, whether or not
    /// their feature is built yet.
    pub const RELEASE_1: [Capability; 9] = [
        Self::Clipboard,
        Self::Files,
        Self::Gallery,
        Self::Input,
        Self::Media,
        Self::Screen,
        Self::Tunnel,
        Self::Terminal,
        Self::FsWrite,
    ];

    /// Wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clipboard => "clipboard.v1",
            Self::Files => "files.v1",
            Self::Gallery => "gallery.v1",
            Self::Input => "input.v1",
            Self::Media => "media.v1",
            Self::Screen => "screen.h264.v1",
            Self::Tunnel => "tunnel.v1",
            Self::Terminal => "term.v1",
            Self::FsWrite => "fs.write.v1",
            Self::Notify => "notify.v1",
            Self::Ring => "ring.v1",
            Self::Camera => "camera.v1",
            Self::Mic => "mic.v1",
            Self::Telephony => "telephony.v1",
            Self::Kvm => "kvm.v1",
            Self::Display => "display.v1",
            Self::AudioOut => "audio.out.v1",
            Self::ClipHistory => "cliphist.v1",
            Self::Lock => "lock.v1",
            Self::Adb => "adb.v1",
            Self::FsBrowse => "fs.browse.v1",
        }
    }

    /// Onboarding default (R5.2). The Release 2 defaults are provisional
    /// ("default when built", design §18): the feature that ships one
    /// confirms it in the design.
    pub fn default_mode(self) -> Mode {
        match self {
            Self::Clipboard | Self::Media => Mode::Allow,
            Self::Files | Self::Gallery | Self::Input | Self::Screen => Mode::Ask,
            Self::Tunnel | Self::Terminal | Self::FsWrite => Mode::Deny,
            // A lost phone must ring without anyone there to approve it.
            Self::Ring => Mode::Allow,
            // Follows the clipboard and the BLE presence check.
            Self::ClipHistory | Self::Lock => Mode::Allow,
            Self::Notify
            | Self::Camera
            | Self::Mic
            | Self::Telephony
            | Self::Kvm
            | Self::Display
            | Self::AudioOut
            | Self::FsBrowse => Mode::Ask,
            Self::Adb => Mode::Deny,
        }
    }

    /// True when only the local UI may change the setting, and switching it to
    /// Allow needs a warning and an extra confirmation (R5.2, R21.2).
    pub fn local_only(self) -> bool {
        matches!(self, Self::Terminal | Self::FsWrite)
    }

    /// True for capabilities only an Android host offers.
    pub fn android_only(self) -> bool {
        matches!(
            self,
            Self::Gallery | Self::Camera | Self::Mic | Self::Telephony
        )
    }

    /// True when an approval starts a running activity that shows the
    /// indicator and Stop button on the host (R5.4). Feature traffic for a
    /// watched capability is accepted only inside such an activity.
    pub fn is_watched(self) -> bool {
        matches!(
            self,
            Self::Input
                | Self::Screen
                | Self::Terminal
                | Self::Camera
                | Self::Mic
                | Self::Kvm
                | Self::Display
                | Self::AudioOut
        )
    }

    /// True when this build lists the capability: every Release 1
    /// capability, and a Release 2 one once its service module adds it to
    /// its `SHIPPED` list.
    pub fn shipped(self) -> bool {
        Self::RELEASE_1.contains(&self) || crate::services::shipped(self)
    }

    /// True when a device on `platform` offers the capability as a host.
    pub fn offered_on(self, platform: &str) -> bool {
        self.shipped() && (!self.android_only() || platform == "android")
    }

    /// Capabilities shown in the settings and sent in the handshake for a
    /// device on `platform`.
    pub fn for_platform(platform: &str) -> Vec<Capability> {
        Self::ALL
            .into_iter()
            .filter(|c| c.offered_on(platform))
            .collect()
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, ()> {
        Self::ALL.into_iter().find(|c| c.as_str() == s).ok_or(())
    }
}

impl Mode {
    /// Storage string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// Parse the storage string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    /// Wire value.
    pub fn to_wire(self) -> WireMode {
        match self {
            Self::Allow => WireMode::Allow,
            Self::Ask => WireMode::Ask,
            Self::Deny => WireMode::Deny,
        }
    }

    /// From the wire value. Unknown values read as Deny.
    pub fn from_wire(v: i32) -> Self {
        match WireMode::try_from(v) {
            Ok(WireMode::Allow) => Self::Allow,
            Ok(WireMode::Ask) => Self::Ask,
            _ => Self::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_r5_2() {
        use Capability::*;
        assert_eq!(Clipboard.default_mode(), Mode::Allow);
        assert_eq!(Media.default_mode(), Mode::Allow);
        for c in [Files, Gallery, Input, Screen] {
            assert_eq!(c.default_mode(), Mode::Ask, "{c}");
        }
        for c in [Tunnel, Terminal, FsWrite] {
            assert_eq!(c.default_mode(), Mode::Deny, "{c}");
        }
    }

    #[test]
    fn strings_round_trip() {
        for c in Capability::ALL {
            assert_eq!(c.as_str().parse::<Capability>(), Ok(c));
        }
        assert!("nope.v1".parse::<Capability>().is_err());
        for m in [Mode::Allow, Mode::Ask, Mode::Deny] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
            assert_eq!(Mode::from_wire(m.to_wire() as i32), m);
        }
        assert_eq!(Mode::from_wire(99), Mode::Deny);
    }

    #[test]
    fn release_2_waits_until_shipped() {
        let linux = Capability::for_platform("linux");
        for c in Capability::ALL {
            let r1 = Capability::RELEASE_1.contains(&c);
            assert_eq!(c.shipped(), r1 || crate::services::shipped(c), "{c}");
            if r1 && !c.android_only() {
                assert!(linux.contains(&c), "{c}");
            }
            assert_eq!(linux.contains(&c), c.offered_on("linux"), "{c}");
        }
        let wire: std::collections::HashSet<_> =
            Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(wire.len(), Capability::ALL.len());
    }

    #[test]
    fn gallery_only_on_android() {
        assert!(Capability::for_platform("android").contains(&Capability::Gallery));
        assert!(!Capability::for_platform("linux").contains(&Capability::Gallery));
    }
}
