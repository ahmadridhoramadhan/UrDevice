//! Service ids: the only numbers feature services share (design §6.2).
//!
//! Every feature service has one id. It goes in [`crate::Feature::service`],
//! [`crate::StreamOpen::service`], and the first two bytes of each datagram.
//! Message field numbers and stream kinds are local to a service, so two
//! features never pick the same number. The table in
//! `docs/dev/adding-a-feature.md` lists who owns which id. Never reuse or
//! renumber an id.

/// Identifies a feature service on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServiceId(pub u16);

impl ServiceId {
    // Release 1: 1 to 15.
    /// Clipboard sharing, `clipboard.v1` (#16, design §9.1).
    pub const CLIPBOARD: Self = Self(1);
    /// File transfer, `files.v1` (#17, design §9.2).
    pub const FILES: Self = Self(2);
    /// Android gallery, `gallery.v1` (#17, design §9.2).
    pub const GALLERY: Self = Self(3);
    /// Remote control: pointer, keyboard, text, presenter, `input.v1` (#18, #19, design §9.3).
    pub const INPUT: Self = Self(4);
    /// Media control, `media.v1` (#20, design §9.4).
    pub const MEDIA: Self = Self(5);
    /// Screen sharing, `screen.h264.v1` (#21, design §10).
    pub const SCREEN: Self = Self(6);
    /// Localhost sharing, `tunnel.v1` (#22, design §11).
    pub const TUNNEL: Self = Self(7);
    /// Bluetooth fallback control messages, if any (#23, design §8).
    pub const BLUETOOTH: Self = Self(8);

    // Release 2: 16 to 63.
    /// Notification mirroring, `notify.v1` (#26, design §18.2).
    pub const NOTIFY: Self = Self(16);
    /// Find my device, `ring.v1` (#27, design §18.9).
    pub const RING: Self = Self(17);
    /// Phone as webcam, `camera.v1` (#28, design §18.1).
    pub const CAMERA: Self = Self(18);
    /// Phone as microphone, `mic.v1` (#28, design §18.1).
    pub const MIC: Self = Self(19);
    /// SMS and calls, `telephony.v1` (#29, design §18.13).
    pub const TELEPHONY: Self = Self(20);
    /// Shared mouse and keyboard, `kvm.v1` (#30, design §18.3).
    pub const KVM: Self = Self(21);
    /// Second display, `display.v1` (#31, design §18.4).
    pub const DISPLAY: Self = Self(22);
    /// Remote terminal, `term.v1` (#32, design §18.5).
    pub const TERMINAL: Self = Self(23);
    /// Port forwarding profiles, `tunnel.v1` (#33, design §18.7).
    pub const PORT_PROFILES: Self = Self(24);
    /// HTTP request inspector. Local only; reserved (#34, design §18.8).
    pub const INSPECTOR: Self = Self(25);
    /// Remote DevTools and ADB, `adb.v1` (#35, design §18.6).
    pub const ADB: Self = Self(26);
    /// Remote file browser, reading, `fs.browse.v1` (#36, design §18.10).
    pub const FS_BROWSE: Self = Self(27);
    /// Remote file browser, changes, `fs.write.v1` (#36, design §18.10).
    pub const FS_WRITE: Self = Self(28);
    /// Shared clipboard history, `cliphist.v1` (#37, design §18.11).
    pub const CLIP_HISTORY: Self = Self(29);
    /// Proximity lock, `lock.v1` (#38, design §18.12).
    pub const PROXIMITY_LOCK: Self = Self(30);
    /// Automations. Remote actions use the other services; reserved (#39, design §18.14).
    pub const AUTOMATIONS: Self = Self(31);
    /// Speaker, `audio.out.v1` (#40, design §18.15).
    pub const SPEAKER: Self = Self(32);

    /// First id for in-process tests. Ids from here to `u16::MAX` never ship.
    pub const TEST_FIRST: Self = Self(0xff00);

    /// From the `u32` wire field. `None` when it does not fit.
    pub fn from_wire(v: u32) -> Option<Self> {
        u16::try_from(v).ok().map(Self)
    }

    /// The `u32` wire field.
    pub fn to_wire(self) -> u32 {
        u32::from(self.0)
    }
}

impl std::fmt::Display for ServiceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "service {}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trip() {
        assert_eq!(ServiceId::from_wire(4), Some(ServiceId::INPUT));
        assert_eq!(ServiceId::INPUT.to_wire(), 4);
        assert_eq!(ServiceId::from_wire(70_000), None);
    }

    #[test]
    fn ids_are_unique_and_below_the_test_range() {
        let all = [
            ServiceId::CLIPBOARD,
            ServiceId::FILES,
            ServiceId::GALLERY,
            ServiceId::INPUT,
            ServiceId::MEDIA,
            ServiceId::SCREEN,
            ServiceId::TUNNEL,
            ServiceId::BLUETOOTH,
            ServiceId::NOTIFY,
            ServiceId::RING,
            ServiceId::CAMERA,
            ServiceId::MIC,
            ServiceId::TELEPHONY,
            ServiceId::KVM,
            ServiceId::DISPLAY,
            ServiceId::TERMINAL,
            ServiceId::PORT_PROFILES,
            ServiceId::INSPECTOR,
            ServiceId::ADB,
            ServiceId::FS_BROWSE,
            ServiceId::FS_WRITE,
            ServiceId::CLIP_HISTORY,
            ServiceId::PROXIMITY_LOCK,
            ServiceId::AUTOMATIONS,
            ServiceId::SPEAKER,
        ];
        let set: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(set.len(), all.len());
        assert!(all.iter().all(|s| s.0 > 0 && *s < ServiceId::TEST_FIRST));
    }
}
