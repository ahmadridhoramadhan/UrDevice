//! Controller side of a remote-control window (#19, T4.8, R9.1, R10.2, R10.4).
//!
//! A window gets key and pointer events as its toolkit reports them. This
//! module turns them into what the [`Input`](super::Input) calls take, with no
//! UI toolkit and no clock, so the rules are tested here:
//!
//! - A printable character becomes **text** (R10.4), so it types the same on
//!   a different keyboard layout.
//! - A shortcut (Ctrl, Alt, or the Windows or Command key held), or a special
//!   key (Enter, arrows, F1 to F24, ...), becomes a **key** press and release
//!   with the modifiers held. The host holds the modifiers around the key.
//! - Pressing [`RELEASE_HOTKEY`] hands the keyboard back. It is not sent.
//!
//! Keys are never logged.

use std::collections::HashMap;

use super::{COORD_MAX, SCROLL_NOTCH, modifier, usage};

/// The key combination that lets go of a captured keyboard and pointer.
pub const RELEASE_HOTKEY: &str = "Ctrl+Alt+G";

/// The modifier keys held when an event arrived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    /// Control.
    pub ctrl: bool,
    /// Alt (Option).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Windows, Command, or Super.
    pub meta: bool,
}

impl Mods {
    /// The [`modifier`] bits for [`Input::key`](super::Input::key).
    pub fn bits(self) -> u32 {
        let mut b = 0;
        if self.ctrl {
            b |= modifier::CTRL;
        }
        if self.shift {
            b |= modifier::SHIFT;
        }
        if self.alt {
            b |= modifier::ALT;
        }
        if self.meta {
            b |= modifier::META;
        }
        b
    }
}

/// What the window should send for one key event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyOut {
    /// [`Input::key`](super::Input::key).
    Key {
        /// USB HID usage.
        usage: u32,
        /// Pressed or released.
        down: bool,
        /// [`modifier`] bits.
        modifiers: u32,
    },
    /// [`Input::text`](super::Input::text).
    Text(String),
    /// The user pressed [`RELEASE_HOTKEY`]: release the capture.
    Release,
}

/// How one key is told apart on release, whatever Shift did to its character.
fn token(text: &str) -> String {
    text.to_lowercase()
}

/// What a key event's text means.
enum Class {
    /// A modifier or lock key: it rides on the keys after it.
    Modifier,
    /// A key with no printable character, with its usage and any implied Shift.
    Special(u32, u32),
    /// A printable character.
    Char(char),
    /// Text from an input method, more than one character.
    Composed,
    /// Nothing to send.
    Ignored,
}

fn classify(text: &str, ctrl: bool) -> Class {
    let mut it = text.chars();
    let (Some(c), None) = (it.next(), it.next()) else {
        return if text.is_empty() || text.chars().any(char::is_control) {
            Class::Ignored
        } else {
            Class::Composed
        };
    };
    let special = |u| Class::Special(u, 0);
    match c {
        // Slint's modifier and lock codes.
        '\u{10}'..='\u{18}' if c != '\u{19}' => Class::Modifier,
        '\u{19}' => Class::Special(usage::TAB, modifier::SHIFT),
        '\u{8}' => special(usage::BACKSPACE),
        '\t' => special(usage::TAB),
        '\n' | '\r' => special(usage::ENTER),
        '\u{1b}' => special(usage::ESCAPE),
        '\u{7f}' => special(usage::DELETE),
        '\u{F700}' => special(usage::UP),
        '\u{F701}' => special(usage::DOWN),
        '\u{F702}' => special(usage::LEFT),
        '\u{F703}' => special(usage::RIGHT),
        // F1 to F12, then F13 to F24.
        '\u{F704}'..='\u{F70F}' => special(usage::F1 + (c as u32 - 0xF704)),
        '\u{F710}'..='\u{F71B}' => special(0x68 + (c as u32 - 0xF710)),
        '\u{F727}' => special(0x49),
        '\u{F729}' => special(usage::HOME),
        '\u{F72B}' => special(usage::END),
        '\u{F72C}' => special(usage::PAGE_UP),
        '\u{F72D}' => special(usage::PAGE_DOWN),
        '\u{F72F}' => special(0x47),
        '\u{F730}' => special(0x48),
        '\u{F731}' => special(0x46),
        '\u{F735}' => special(0x65),
        // Other private-use codes (Slint's Back, Stop, ...).
        '\u{E000}'..='\u{F8FF}' => Class::Ignored,
        // Ctrl+A arrives as U+0001 on some systems: it is the letter.
        '\u{1}'..='\u{1a}' if ctrl => Class::Char((b'a' + (c as u8 - 1)) as char),
        c if c.is_control() => Class::Ignored,
        c => Class::Char(c),
    }
}

/// The HID usage for a character on a US layout, with the Shift bit when the
/// character needs it. Used for shortcuts, where the layout-independent text
/// path does not apply.
fn char_usage(c: char) -> Option<(u32, u32)> {
    let shift = modifier::SHIFT;
    Some(match c {
        'a'..='z' => (usage::A + (c as u32 - 'a' as u32), 0),
        'A'..='Z' => (usage::A + (c as u32 - 'A' as u32), shift),
        '1'..='9' => (0x1E + (c as u32 - '1' as u32), 0),
        '0' => (0x27, 0),
        ' ' => (usage::SPACE, 0),
        '-' => (0x2D, 0),
        '=' => (0x2E, 0),
        '[' => (0x2F, 0),
        ']' => (0x30, 0),
        '\\' => (0x31, 0),
        ';' => (0x33, 0),
        '\'' => (0x34, 0),
        '`' => (0x35, 0),
        ',' => (0x36, 0),
        '.' => (0x37, 0),
        '/' => (0x38, 0),
        '!' => (0x1E, shift),
        '@' => (0x1F, shift),
        '#' => (0x20, shift),
        '$' => (0x21, shift),
        '%' => (0x22, shift),
        '^' => (0x23, shift),
        '&' => (0x24, shift),
        '*' => (0x25, shift),
        '(' => (0x26, shift),
        ')' => (0x27, shift),
        '_' => (0x2D, shift),
        '+' => (0x2E, shift),
        '{' => (0x2F, shift),
        '}' => (0x30, shift),
        '|' => (0x31, shift),
        ':' => (0x33, shift),
        '"' => (0x34, shift),
        '~' => (0x35, shift),
        '<' => (0x36, shift),
        '>' => (0x37, shift),
        '?' => (0x38, shift),
        _ => return None,
    })
}

/// Turns key events into what to send, and remembers which keys are down so
/// letting go of the capture never leaves one stuck.
#[derive(Debug, Default)]
pub struct KeyboardCapture {
    /// Keys sent as key presses and not yet released: usage and modifiers.
    held: HashMap<String, (u32, u32)>,
    /// The key of the release hotkey, until it comes up.
    swallowed: Option<String>,
}

impl KeyboardCapture {
    /// A key went down (or repeated). `text` is the toolkit's text for it.
    pub fn press(&mut self, text: &str, mods: Mods) -> Vec<KeyOut> {
        let class = classify(text, mods.ctrl);
        if let Class::Char(c) = class
            && mods.ctrl
            && mods.alt
            && !mods.meta
            && c.eq_ignore_ascii_case(&'g')
        {
            self.swallowed = Some(token(text));
            return vec![KeyOut::Release];
        }
        match class {
            Class::Modifier | Class::Ignored => Vec::new(),
            Class::Composed => vec![KeyOut::Text(text.to_owned())],
            Class::Special(u, implied) => {
                let modifiers = mods.bits() | implied;
                self.held.insert(token(text), (u, modifiers));
                vec![KeyOut::Key {
                    usage: u,
                    down: true,
                    modifiers,
                }]
            }
            Class::Char(c) => {
                // Ctrl and Alt together is how Windows reports AltGr: the
                // character is real text, not a shortcut.
                let alt_gr = mods.ctrl && mods.alt && !mods.meta;
                let shortcut = (mods.ctrl || mods.alt || mods.meta) && !alt_gr;
                if !shortcut {
                    return vec![KeyOut::Text(c.to_string())];
                }
                let Some((u, implied)) = char_usage(c) else {
                    return Vec::new();
                };
                let modifiers = mods.bits() | implied;
                self.held.insert(token(&c.to_string()), (u, modifiers));
                vec![KeyOut::Key {
                    usage: u,
                    down: true,
                    modifiers,
                }]
            }
        }
    }

    /// A key came up.
    pub fn release(&mut self, text: &str, mods: Mods) -> Vec<KeyOut> {
        let _ = mods;
        let mut t = token(text);
        // Ctrl+A may come up as U+0001.
        if let Class::Char(c) = classify(text, true) {
            t = token(&c.to_string());
        }
        if self.swallowed.as_deref() == Some(t.as_str()) {
            self.swallowed = None;
            return Vec::new();
        }
        match self.held.remove(&t) {
            Some((u, modifiers)) => vec![KeyOut::Key {
                usage: u,
                down: false,
                modifiers,
            }],
            None => Vec::new(),
        }
    }

    /// Let go of everything held: the window lost focus, or the capture ended.
    pub fn release_all(&mut self) -> Vec<KeyOut> {
        self.swallowed = None;
        let mut keys: Vec<_> = self.held.drain().map(|(_, v)| v).collect();
        keys.sort_unstable();
        keys.into_iter()
            .map(|(u, modifiers)| KeyOut::Key {
                usage: u,
                down: false,
                modifiers,
            })
            .collect()
    }

    /// True while a key press is waiting for its release.
    pub fn any_held(&self) -> bool {
        !self.held.is_empty()
    }
}

/// Backends whose pointer moves only by distance, never to a place: the
/// Wayland portal and `uinput` (design §9.3). For these a window sends
/// [`RelativePointer`] distances and not absolute positions.
pub fn host_moves_by_distance(backend: &str) -> bool {
    matches!(backend, "wayland-portal" | "uinput")
}

/// `pos` out of `extent` (both in the same unit) as a coordinate of 0 to
/// [`COORD_MAX`], kept inside the window.
pub fn normalize(pos: f32, extent: f32) -> u32 {
    if extent.is_nan() || extent <= 0.0 {
        return 0;
    }
    ((pos / extent).clamp(0.0, 1.0) * COORD_MAX as f32).round() as u32
}

/// The distances between pointer positions, for hosts that move by distance.
/// Starts over when the pointer leaves the window and comes back, like a
/// finger lifted from a touchpad.
#[derive(Debug, Default)]
pub struct RelativePointer {
    last: Option<(f32, f32)>,
}

impl RelativePointer {
    /// The pointer is at `x`, `y`. Returns how far it moved, when it moved.
    pub fn moved(&mut self, x: f32, y: f32) -> Option<(i32, i32)> {
        let prev = self.last.replace((x, y))?;
        let (dx, dy) = ((x - prev.0).round() as i32, (y - prev.1).round() as i32);
        (dx != 0 || dy != 0).then_some((dx, dy))
    }

    /// The pointer left the window.
    pub fn leave(&mut self) {
        self.last = None;
    }
}

/// Wheel units for the host from a toolkit scroll delta in logical pixels.
/// Toolkits report positive when the content moves down (wheel turned away
/// from the user); the host takes positive as "scroll down", so the sign flips.
/// One notch of the common 60-pixel step is [`SCROLL_NOTCH`].
pub fn scroll_units(delta: f32) -> i32 {
    (-delta * SCROLL_NOTCH as f32 / 60.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };
    const CTRL: Mods = Mods { ctrl: true, ..NONE };
    const SHIFT: Mods = Mods {
        shift: true,
        ..NONE
    };

    fn key(usage: u32, down: bool, modifiers: u32) -> KeyOut {
        KeyOut::Key {
            usage,
            down,
            modifiers,
        }
    }

    #[test]
    fn printable_characters_are_text_in_any_layout() {
        let mut k = KeyboardCapture::default();
        assert_eq!(k.press("a", NONE), vec![KeyOut::Text("a".into())]);
        assert_eq!(k.press("A", SHIFT), vec![KeyOut::Text("A".into())]);
        assert_eq!(k.press("é", NONE), vec![KeyOut::Text("é".into())]);
        assert_eq!(k.press("世", NONE), vec![KeyOut::Text("世".into())]);
        assert_eq!(k.press(" ", NONE), vec![KeyOut::Text(" ".into())]);
        // Input-method text is text too.
        assert_eq!(k.press("ねこ", NONE), vec![KeyOut::Text("ねこ".into())]);
        assert!(k.release("a", NONE).is_empty());
        assert!(!k.any_held());
    }

    #[test]
    fn special_keys_are_presses_and_releases() {
        let mut k = KeyboardCapture::default();
        assert_eq!(k.press("\n", NONE), vec![key(usage::ENTER, true, 0)]);
        assert_eq!(
            k.press("\u{F702}", SHIFT),
            vec![key(usage::LEFT, true, modifier::SHIFT)]
        );
        assert_eq!(k.press("\u{F704}", NONE), vec![key(usage::F1, true, 0)]);
        assert_eq!(k.press("\u{F70F}", NONE), vec![key(usage::F12, true, 0)]);
        assert_eq!(k.press("\u{F710}", NONE), vec![key(0x68, true, 0)]);
        assert_eq!(
            k.press("\u{19}", NONE),
            vec![key(usage::TAB, true, modifier::SHIFT)]
        );
        assert!(k.any_held());
        assert_eq!(k.release("\n", NONE), vec![key(usage::ENTER, false, 0)]);
        // The release carries the modifiers of the press, not of now.
        assert_eq!(
            k.release("\u{F702}", NONE),
            vec![key(usage::LEFT, false, modifier::SHIFT)]
        );
    }

    #[test]
    fn shortcuts_are_keys_with_modifiers() {
        let mut k = KeyboardCapture::default();
        assert_eq!(
            k.press("c", CTRL),
            vec![key(usage::A + 2, true, modifier::CTRL)]
        );
        // Some systems send the control code.
        assert_eq!(
            k.press("\u{1}", CTRL),
            vec![key(usage::A, true, modifier::CTRL)]
        );
        assert_eq!(
            k.release("c", NONE),
            vec![key(usage::A + 2, false, modifier::CTRL)]
        );
        assert_eq!(
            k.release("\u{1}", NONE),
            vec![key(usage::A, false, modifier::CTRL)]
        );
        let meta = Mods { meta: true, ..NONE };
        assert_eq!(
            k.press("d", meta),
            vec![key(usage::A + 3, true, modifier::META)]
        );
        let cs = Mods {
            ctrl: true,
            shift: true,
            ..NONE
        };
        assert_eq!(
            k.press("T", cs),
            vec![key(usage::A + 19, true, modifier::CTRL | modifier::SHIFT)]
        );
    }

    #[test]
    fn alt_gr_characters_are_text() {
        let mut k = KeyboardCapture::default();
        let alt_gr = Mods {
            ctrl: true,
            alt: true,
            ..NONE
        };
        assert_eq!(k.press("@", alt_gr), vec![KeyOut::Text("@".into())]);
        assert_eq!(k.press("€", alt_gr), vec![KeyOut::Text("€".into())]);
    }

    #[test]
    fn modifiers_alone_send_nothing() {
        let mut k = KeyboardCapture::default();
        for m in [
            '\u{10}', '\u{11}', '\u{12}', '\u{13}', '\u{14}', '\u{15}', '\u{16}', '\u{17}',
            '\u{18}',
        ] {
            assert!(k.press(&m.to_string(), NONE).is_empty());
            assert!(k.release(&m.to_string(), NONE).is_empty());
        }
        // Unknown private-use codes and control characters are dropped.
        assert!(k.press("\u{F748}", NONE).is_empty());
        assert!(k.press("\u{0}", NONE).is_empty());
        assert!(k.press("", NONE).is_empty());
    }

    #[test]
    fn the_release_hotkey_is_not_sent() {
        let mut k = KeyboardCapture::default();
        let both = Mods {
            ctrl: true,
            alt: true,
            ..NONE
        };
        assert_eq!(k.press("g", both), vec![KeyOut::Release]);
        assert!(k.release("g", NONE).is_empty());
        // Ctrl+Alt+Shift+G too, and without Alt it is an ordinary shortcut.
        let shifted = Mods {
            shift: true,
            ..both
        };
        assert_eq!(k.press("G", shifted), vec![KeyOut::Release]);
        assert_eq!(k.release("G", NONE), Vec::new());
        assert_eq!(
            k.press("g", CTRL),
            vec![key(usage::A + 6, true, modifier::CTRL)]
        );
    }

    #[test]
    fn releasing_the_capture_lets_go_of_every_held_key() {
        let mut k = KeyboardCapture::default();
        k.press("\u{F703}", NONE);
        k.press("c", CTRL);
        let up = k.release_all();
        assert_eq!(up.len(), 2);
        assert!(
            up.iter()
                .all(|o| matches!(o, KeyOut::Key { down: false, .. }))
        );
        assert!(!k.any_held());
        assert!(k.release_all().is_empty());
    }

    #[test]
    fn coordinates_stay_in_range() {
        assert_eq!(normalize(0.0, 100.0), 0);
        assert_eq!(normalize(100.0, 100.0), COORD_MAX);
        assert_eq!(normalize(-5.0, 100.0), 0);
        assert_eq!(normalize(500.0, 100.0), COORD_MAX);
        assert_eq!(normalize(50.0, 0.0), 0);
        assert_eq!(normalize(f32::NAN, 100.0), 0);
        assert_eq!(normalize(50.0, f32::NAN), 0);
    }

    #[test]
    fn relative_pointer_sends_distances_and_restarts_on_re_entry() {
        let mut p = RelativePointer::default();
        assert_eq!(p.moved(10.0, 10.0), None, "the first position is the start");
        assert_eq!(p.moved(13.0, 8.0), Some((3, -2)));
        assert_eq!(p.moved(13.2, 8.1), None);
        p.leave();
        assert_eq!(p.moved(200.0, 200.0), None, "no jump after re-entering");
        assert_eq!(p.moved(201.0, 200.0), Some((1, 0)));
    }

    #[test]
    fn wheel_turned_down_scrolls_down() {
        assert_eq!(scroll_units(-60.0), SCROLL_NOTCH);
        assert_eq!(scroll_units(60.0), -SCROLL_NOTCH);
        assert_eq!(scroll_units(0.0), 0);
    }

    #[test]
    fn only_wayland_portal_and_uinput_move_by_distance() {
        assert!(host_moves_by_distance("wayland-portal"));
        assert!(host_moves_by_distance("uinput"));
        for b in ["x11", "windows", "macos", "android", ""] {
            assert!(!host_moves_by_distance(b));
        }
    }
}
