//! Key code tables: USB HID usage (page 0x07, what the wire carries) to each
//! OS's own code (design §9.3). A usage the OS has no key for maps to `None`
//! and the key is ignored.
//!
//! Linux (X11, the portal, `uinput`) all use evdev codes; an X11 keycode is the
//! evdev code plus 8.

/// Letters A to Z in HID order, as evdev codes.
#[cfg(any(target_os = "linux", test))]
const EVDEV_LETTERS: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];

/// HID 0x28 to 0x38 (Enter to Slash) as evdev codes.
#[cfg(any(target_os = "linux", test))]
const EVDEV_PUNCT: [u16; 17] = [
    28, 1, 14, 15, 57, 12, 13, 26, 27, 43, 43, 39, 40, 41, 51, 52, 53,
];

/// HID 0x59 to 0x61 (keypad 1 to 9) as evdev codes.
#[cfg(any(target_os = "linux", test))]
const EVDEV_KEYPAD: [u16; 9] = [79, 80, 81, 75, 76, 77, 71, 72, 73];

/// The Linux evdev code (`KEY_*`) for a HID usage.
#[cfg(any(target_os = "linux", test))]
pub fn evdev(usage: u32) -> Option<u16> {
    Some(match usage {
        0x04..=0x1D => EVDEV_LETTERS[(usage - 0x04) as usize],
        // 1 to 9 are KEY_1 (2) to KEY_9 (10); 0 is KEY_0 (11).
        0x1E..=0x27 => 2 + (usage - 0x1E) as u16,
        0x28..=0x38 => EVDEV_PUNCT[(usage - 0x28) as usize],
        0x39 => 58,
        // F1 to F10 are 59 to 68; F11 and F12 are 87 and 88.
        0x3A..=0x43 => 59 + (usage - 0x3A) as u16,
        0x44 => 87,
        0x45 => 88,
        0x46 => 99,
        0x47 => 70,
        0x48 => 119,
        0x49 => 110,
        0x4A => 102,
        0x4B => 104,
        0x4C => 111,
        0x4D => 107,
        0x4E => 109,
        0x4F => 106,
        0x50 => 105,
        0x51 => 108,
        0x52 => 103,
        0x53 => 69,
        0x54 => 98,
        0x55 => 55,
        0x56 => 74,
        0x57 => 78,
        0x58 => 96,
        0x59..=0x61 => EVDEV_KEYPAD[(usage - 0x59) as usize],
        0x62 => 82,
        0x63 => 83,
        0x64 => 86,
        0x65 => 127,
        // F13 to F24.
        0x68..=0x73 => 183 + (usage - 0x68) as u16,
        // Left Control, Shift, Alt, Meta, then the right ones.
        0xE0 => 29,
        0xE1 => 42,
        0xE2 => 56,
        0xE3 => 125,
        0xE4 => 97,
        0xE5 => 54,
        0xE6 => 100,
        0xE7 => 126,
        _ => return None,
    })
}

/// A Windows virtual-key code, and whether the key is an "extended" one.
#[cfg(any(target_os = "windows", test))]
pub fn vk(usage: u32) -> Option<(u16, bool)> {
    let plain = |v: u16| Some((v, false));
    let ext = |v: u16| Some((v, true));
    match usage {
        0x04..=0x1D => plain(0x41 + (usage - 0x04) as u16),
        0x1E..=0x26 => plain(0x31 + (usage - 0x1E) as u16),
        0x27 => plain(0x30),
        0x28 => plain(0x0D),
        0x29 => plain(0x1B),
        0x2A => plain(0x08),
        0x2B => plain(0x09),
        0x2C => plain(0x20),
        0x2D => plain(0xBD),
        0x2E => plain(0xBB),
        0x2F => plain(0xDB),
        0x30 => plain(0xDD),
        0x31 | 0x32 => plain(0xDC),
        0x33 => plain(0xBA),
        0x34 => plain(0xDE),
        0x35 => plain(0xC0),
        0x36 => plain(0xBC),
        0x37 => plain(0xBE),
        0x38 => plain(0xBF),
        0x39 => plain(0x14),
        0x3A..=0x45 => plain(0x70 + (usage - 0x3A) as u16),
        0x46 => ext(0x2C),
        0x47 => plain(0x91),
        0x48 => plain(0x13),
        0x49 => ext(0x2D),
        0x4A => ext(0x24),
        0x4B => ext(0x21),
        0x4C => ext(0x2E),
        0x4D => ext(0x23),
        0x4E => ext(0x22),
        0x4F => ext(0x27),
        0x50 => ext(0x25),
        0x51 => ext(0x28),
        0x52 => ext(0x26),
        0x53 => ext(0x90),
        0x54 => ext(0x6F),
        0x55 => plain(0x6A),
        0x56 => plain(0x6D),
        0x57 => plain(0x6B),
        0x58 => ext(0x0D),
        0x59..=0x61 => plain(0x61 + (usage - 0x59) as u16),
        0x62 => plain(0x60),
        0x63 => plain(0x6E),
        0x64 => plain(0xE2),
        0x65 => ext(0x5D),
        0x68..=0x73 => plain(0x7C + (usage - 0x68) as u16),
        0xE0 => plain(0xA2),
        0xE1 => plain(0xA0),
        0xE2 => plain(0xA4),
        0xE3 => ext(0x5B),
        0xE4 => ext(0xA3),
        0xE5 => plain(0xA1),
        0xE6 => ext(0xA5),
        0xE7 => ext(0x5C),
        _ => None,
    }
}

/// Letters A to Z in HID order, as macOS virtual key codes (`kVK_ANSI_*`).
#[cfg(any(target_os = "macos", test))]
const MAC_LETTERS: [u16; 26] = [
    0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23,
    0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
];

/// Digits 1 to 9 and 0 in HID order, as macOS virtual key codes.
#[cfg(any(target_os = "macos", test))]
const MAC_DIGITS: [u16; 10] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19, 0x1D];

/// F1 to F12 as macOS virtual key codes.
#[cfg(any(target_os = "macos", test))]
const MAC_FKEYS: [u16; 12] = [
    0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F,
];

/// HID 0x28 to 0x38 (Enter to Slash) as macOS virtual key codes.
#[cfg(any(target_os = "macos", test))]
const MAC_PUNCT: [u16; 17] = [
    0x24, 0x35, 0x33, 0x30, 0x31, 0x1B, 0x18, 0x21, 0x1E, 0x2A, 0x2A, 0x29, 0x27, 0x32, 0x2B, 0x2F,
    0x2C,
];

/// Keypad 1 to 9 as macOS virtual key codes.
#[cfg(any(target_os = "macos", test))]
const MAC_KEYPAD: [u16; 9] = [0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5B, 0x5C];

/// The macOS virtual key code (`kVK_*`) for a HID usage. Print Screen, Scroll
/// Lock, Pause, and F21 to F24 do not exist on a Mac.
#[cfg(any(target_os = "macos", test))]
pub fn mac(usage: u32) -> Option<u16> {
    Some(match usage {
        0x04..=0x1D => MAC_LETTERS[(usage - 0x04) as usize],
        0x1E..=0x27 => MAC_DIGITS[(usage - 0x1E) as usize],
        0x28..=0x38 => MAC_PUNCT[(usage - 0x28) as usize],
        0x39 => 0x39,
        0x3A..=0x45 => MAC_FKEYS[(usage - 0x3A) as usize],
        0x49 => 0x72,
        0x4A => 0x73,
        0x4B => 0x74,
        0x4C => 0x75,
        0x4D => 0x77,
        0x4E => 0x79,
        0x4F => 0x7C,
        0x50 => 0x7B,
        0x51 => 0x7D,
        0x52 => 0x7E,
        0x53 => 0x47,
        0x54 => 0x4B,
        0x55 => 0x43,
        0x56 => 0x4E,
        0x57 => 0x45,
        0x58 => 0x4C,
        0x59..=0x61 => MAC_KEYPAD[(usage - 0x59) as usize],
        0x62 => 0x52,
        0x63 => 0x41,
        0x64 => 0x0A,
        // F13 to F20.
        0x68 => 0x69,
        0x69 => 0x6B,
        0x6A => 0x71,
        0x6B => 0x6A,
        0x6C => 0x40,
        0x6D => 0x4F,
        0x6E => 0x50,
        0x6F => 0x5A,
        0xE0 => 0x3B,
        0xE1 => 0x38,
        0xE2 => 0x3A,
        0xE3 => 0x37,
        0xE4 => 0x3E,
        0xE5 => 0x3C,
        0xE6 => 0x3D,
        0xE7 => 0x36,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every usage the wire accepts, which the tables are checked against.
    fn wire_usages() -> Vec<u32> {
        (0u32..=0xFF)
            .filter(|u| urdevice_proto::features::input::is_valid_usage(*u))
            .collect()
    }

    #[test]
    fn evdev_has_no_duplicates_and_known_keys() {
        // `0x31` (backslash) and `0x32` (non-US #) share KEY_BACKSLASH.
        let mut seen = std::collections::HashMap::new();
        for u in wire_usages() {
            let code = evdev(u).unwrap_or_else(|| panic!("no evdev code for {u:#x}"));
            if let Some(prev) = seen.insert(code, u) {
                assert!(
                    matches!((prev, u), (0x31, 0x32)),
                    "{prev:#x} and {u:#x} share evdev {code}"
                );
            }
        }
        assert_eq!(evdev(0x04), Some(30)); // A
        assert_eq!(evdev(0x1D), Some(44)); // Z
        assert_eq!(evdev(0x1E), Some(2)); // 1
        assert_eq!(evdev(0x27), Some(11)); // 0
        assert_eq!(evdev(0x28), Some(28)); // Enter
        assert_eq!(evdev(0x2C), Some(57)); // Space
        assert_eq!(evdev(0x3E), Some(63)); // F5
        assert_eq!(evdev(0x45), Some(88)); // F12
        assert_eq!(evdev(0x4F), Some(106)); // Right
        assert_eq!(evdev(0x52), Some(103)); // Up
        assert_eq!(evdev(0xE0), Some(29)); // Left Ctrl
        assert_eq!(evdev(0x68), Some(183)); // F13
        assert_eq!(evdev(0x73), Some(194)); // F24
        assert_eq!(evdev(0x03), None);
    }

    #[test]
    fn windows_codes_cover_every_usage_and_match_known_keys() {
        for u in wire_usages() {
            assert!(vk(u).is_some(), "no virtual key for {u:#x}");
        }
        assert_eq!(vk(0x04), Some((0x41, false))); // A
        assert_eq!(vk(0x1D), Some((0x5A, false))); // Z
        assert_eq!(vk(0x1E), Some((0x31, false))); // 1
        assert_eq!(vk(0x27), Some((0x30, false))); // 0
        assert_eq!(vk(0x28), Some((0x0D, false))); // Enter
        assert_eq!(vk(0x3E), Some((0x74, false))); // F5
        assert_eq!(vk(0x4F), Some((0x27, true))); // Right
        assert_eq!(vk(0x4E), Some((0x22, true))); // Page Down
        assert_eq!(vk(0x58), Some((0x0D, true))); // keypad Enter
        assert_eq!(vk(0x59), Some((0x61, false))); // keypad 1
        assert_eq!(vk(0x62), Some((0x60, false))); // keypad 0
        assert_eq!(vk(0x73), Some((0x87, false))); // F24
        assert_eq!(vk(0xE0), Some((0xA2, false)));
    }

    #[test]
    fn mac_codes_have_no_duplicates_and_known_keys() {
        let mut seen = std::collections::HashMap::new();
        for u in wire_usages() {
            let Some(code) = mac(u) else { continue };
            if let Some(prev) = seen.insert(code, u) {
                assert!(
                    matches!((prev, u), (0x31, 0x32)),
                    "{prev:#x} and {u:#x} share mac {code:#x}"
                );
            }
        }
        // The keys a presentation needs exist on a Mac.
        for u in [0x28, 0x29, 0x05, 0x3E, 0x4B, 0x4E, 0x4F, 0x50, 0x2C] {
            assert!(mac(u).is_some(), "{u:#x}");
        }
        assert_eq!(mac(0x04), Some(0x00)); // A
        assert_eq!(mac(0x05), Some(0x0B)); // B
        assert_eq!(mac(0x1D), Some(0x06)); // Z
        assert_eq!(mac(0x1E), Some(0x12)); // 1
        assert_eq!(mac(0x27), Some(0x1D)); // 0
        assert_eq!(mac(0x28), Some(0x24)); // Return
        assert_eq!(mac(0x3E), Some(0x60)); // F5
        assert_eq!(mac(0x4F), Some(0x7C)); // Right
        assert_eq!(mac(0xE3), Some(0x37)); // Command
        assert_eq!(mac(0x46), None); // Print Screen
    }
}
