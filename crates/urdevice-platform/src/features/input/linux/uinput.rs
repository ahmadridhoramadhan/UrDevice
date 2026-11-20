//! `uinput` back-end: a virtual keyboard and mouse in the kernel (design §9.3,
//! R9.2). It works on any compositor, but it needs write access to
//! `/dev/uinput` and it only moves the pointer by relative steps.
//!
//! Text is typed for a US layout where the character has a key, and through
//! the Ctrl+Shift+U Unicode entry (GTK, IBus) for the rest. That is best
//! effort: a layout-independent path needs the portal or XTest.

use std::ffi::{c_int, c_long, c_ulong};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use urdevice_core::services::input::{Missing, PointerEvent, button};
use urdevice_core::{CoreError, Result};

use super::super::keymap::evdev;
use super::super::{Backend, Fail, ScrollAccum};

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;
const REL_WHEEL_HI_RES: u16 = 0x0b;
const REL_HWHEEL_HI_RES: u16 = 0x0c;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;
const BUS_VIRTUAL: u16 = 0x06;

const UI_DEV_CREATE: c_ulong = 0x5501;
const UI_DEV_DESTROY: c_ulong = 0x5502;
const UI_DEV_SETUP: c_ulong = 0x405c_5503;
const UI_SET_EVBIT: c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: c_ulong = 0x4004_5565;
const UI_SET_RELBIT: c_ulong = 0x4004_5566;

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

/// `struct uinput_setup`: an `input_id`, a name of 80 bytes, and a force
/// feedback count.
#[repr(C)]
struct Setup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

pub(super) struct Uinput {
    file: File,
    scroll: ScrollAccum,
}

pub(super) fn open() -> std::result::Result<Box<dyn Backend>, Fail> {
    let file = match OpenOptions::new()
        .write(true)
        .custom_flags(0o4000) // O_NONBLOCK
        .open("/dev/uinput")
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(Fail::Missing(Missing::Uinput));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The module may just not be loaded yet; the rule installer loads it.
            return Err(Fail::Missing(Missing::Uinput));
        }
        Err(_) => return Err(Fail::Unavailable("uinput could not be opened")),
    };
    create(&file).map_err(|_| Fail::Unavailable("uinput device not created"))?;
    // Compositors need a moment to notice a new device; events sent before
    // that are lost.
    std::thread::sleep(Duration::from_millis(400));
    Ok(Box::new(Uinput {
        file,
        scroll: ScrollAccum::default(),
    }))
}

fn check(r: c_int) -> std::io::Result<()> {
    if r < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn create(file: &File) -> std::io::Result<()> {
    let fd = file.as_raw_fd();
    let enable = |req: c_ulong, v: c_int| -> std::io::Result<()> {
        // SAFETY: `fd` is an open uinput descriptor and the request takes one int.
        check(unsafe { ioctl(fd, req, v) })
    };
    enable(UI_SET_EVBIT, c_int::from(EV_KEY))?;
    enable(UI_SET_EVBIT, c_int::from(EV_REL))?;
    for code in 1..=255 {
        enable(UI_SET_KEYBIT, code)?;
    }
    for code in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA] {
        enable(UI_SET_KEYBIT, c_int::from(code))?;
    }
    for code in [
        REL_X,
        REL_Y,
        REL_HWHEEL,
        REL_WHEEL,
        REL_WHEEL_HI_RES,
        REL_HWHEEL_HI_RES,
    ] {
        enable(UI_SET_RELBIT, c_int::from(code))?;
    }
    let mut setup = Setup {
        bustype: BUS_VIRTUAL,
        vendor: 0,
        product: 0,
        version: 1,
        name: [0; 80],
        ff_effects_max: 0,
    };
    let name = b"UrDevice virtual input";
    setup.name[..name.len()].copy_from_slice(name);
    // SAFETY: `setup` is a valid `uinput_setup` that lives through the call.
    check(unsafe { ioctl(fd, UI_DEV_SETUP, &raw const setup) })?;
    // SAFETY: no argument.
    check(unsafe { ioctl(fd, UI_DEV_CREATE) })
}

/// One `struct input_event`: a zero timestamp (the kernel stamps it), the
/// type, the code, and the value.
fn event(buf: &mut Vec<u8>, kind: u16, code: u16, value: i32) {
    buf.extend_from_slice(&(0 as c_long).to_ne_bytes());
    buf.extend_from_slice(&(0 as c_long).to_ne_bytes());
    buf.extend_from_slice(&kind.to_ne_bytes());
    buf.extend_from_slice(&code.to_ne_bytes());
    buf.extend_from_slice(&value.to_ne_bytes());
}

impl Uinput {
    /// Write events, then the report that makes them take effect.
    fn emit(&mut self, events: &[(u16, u16, i32)]) -> Result<()> {
        let mut buf = Vec::with_capacity((events.len() + 1) * 24);
        for (kind, code, value) in events {
            event(&mut buf, *kind, *code, *value);
        }
        event(&mut buf, EV_SYN, 0, 0);
        self.file.write_all(&buf).map_err(CoreError::from)
    }

    fn tap(&mut self, code: u16, shift: bool) -> Result<()> {
        let mut v = Vec::new();
        if shift {
            v.push((EV_KEY, 42, 1));
        }
        v.push((EV_KEY, code, 1));
        v.push((EV_KEY, code, 0));
        if shift {
            v.push((EV_KEY, 42, 0));
        }
        self.emit(&v)
    }

    /// Enter a character through Ctrl+Shift+U, its hex code, and Space.
    fn unicode(&mut self, ch: char) -> Result<()> {
        // KEY_LEFTCTRL 29, KEY_LEFTSHIFT 42, KEY_U 22, KEY_SPACE 57.
        self.emit(&[
            (EV_KEY, 29, 1),
            (EV_KEY, 42, 1),
            (EV_KEY, 22, 1),
            (EV_KEY, 22, 0),
        ])?;
        self.emit(&[(EV_KEY, 42, 0), (EV_KEY, 29, 0)])?;
        for d in format!("{:x}", ch as u32).chars() {
            if let Some((code, shift)) = us_char(d) {
                self.tap(code, shift)?;
            }
        }
        self.tap(57, false)
    }
}

/// The evdev key and whether Shift is held to type `c` on a US keyboard.
pub(super) fn us_char(c: char) -> Option<(u16, bool)> {
    const LETTERS: [u16; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
        45, 21, 44,
    ];
    const DIGITS: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    // Shifted digit row: ) ! @ # $ % ^ & * (
    const SHIFTED_DIGITS: &str = ")!@#$%^&*(";
    Some(match c {
        'a'..='z' => (LETTERS[(c as u8 - b'a') as usize], false),
        'A'..='Z' => (LETTERS[(c as u8 - b'A') as usize], true),
        '0'..='9' => (DIGITS[(c as u8 - b'0') as usize], false),
        ' ' => (57, false),
        '\n' => (28, false),
        '\t' => (15, false),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        '\\' => (43, false),
        '|' => (43, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        c => {
            let i = SHIFTED_DIGITS.find(c)?;
            (DIGITS[i], true)
        }
    })
}

impl Backend for Uinput {
    fn name(&self) -> &'static str {
        "uinput"
    }

    fn pointer(&mut self, e: &PointerEvent) -> Result<()> {
        match *e {
            PointerEvent::Move { dx, dy } => {
                let mut v = Vec::new();
                if dx != 0 {
                    v.push((EV_REL, REL_X, dx));
                }
                if dy != 0 {
                    v.push((EV_REL, REL_Y, dy));
                }
                if v.is_empty() { Ok(()) } else { self.emit(&v) }
            }
            // A virtual mouse does not know where the screen is.
            PointerEvent::Abs { .. } => Err(CoreError::unsupported("input", "uinput-absolute")),
            PointerEvent::Button { button: b, down } => {
                let code = match b {
                    button::LEFT => BTN_LEFT,
                    button::RIGHT => BTN_RIGHT,
                    button::MIDDLE => BTN_MIDDLE,
                    button::BACK => BTN_SIDE,
                    button::FORWARD => BTN_EXTRA,
                    _ => return Ok(()),
                };
                self.emit(&[(EV_KEY, code, i32::from(down))])
            }
            PointerEvent::Scroll { dx, dy } => {
                // The wire has down positive; REL_WHEEL has up positive.
                let (nx, ny) = self.scroll.add(dx, dy);
                let mut v = Vec::new();
                if dy != 0 {
                    v.push((EV_REL, REL_WHEEL_HI_RES, -dy));
                }
                if ny != 0 {
                    v.push((EV_REL, REL_WHEEL, -ny));
                }
                if dx != 0 {
                    v.push((EV_REL, REL_HWHEEL_HI_RES, dx));
                }
                if nx != 0 {
                    v.push((EV_REL, REL_HWHEEL, nx));
                }
                if v.is_empty() { Ok(()) } else { self.emit(&v) }
            }
        }
    }

    fn key(&mut self, usage: u32, down: bool) -> Result<()> {
        match evdev(usage) {
            Some(code) => self.emit(&[(EV_KEY, code, i32::from(down))]),
            None => Ok(()),
        }
    }

    fn text(&mut self, text: &str) -> Result<()> {
        for ch in text.chars() {
            match us_char(ch) {
                Some((code, shift)) => self.tap(code, shift)?,
                None if ch.is_control() => {}
                None => self.unicode(ch)?,
            }
        }
        Ok(())
    }
}

impl Drop for Uinput {
    fn drop(&mut self) {
        // SAFETY: the descriptor is open until `file` drops after this.
        let _ = unsafe { ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn us_keyboard_characters() {
        assert_eq!(us_char('a'), Some((30, false)));
        assert_eq!(us_char('A'), Some((30, true)));
        assert_eq!(us_char('1'), Some((2, false)));
        assert_eq!(us_char('0'), Some((11, false)));
        assert_eq!(us_char('!'), Some((2, true)));
        assert_eq!(us_char(')'), Some((11, true)));
        assert_eq!(us_char('('), Some((10, true)));
        assert_eq!(us_char('@'), Some((3, true)));
        assert_eq!(us_char('?'), Some((53, true)));
        assert_eq!(us_char(' '), Some((57, false)));
        assert_eq!(us_char('é'), None);
        assert_eq!(us_char('世'), None);
    }

    #[test]
    fn every_ascii_character_has_a_key_except_control_ones() {
        for c in (0x20u8..0x7F).map(char::from) {
            assert!(us_char(c).is_some(), "{c:?}");
        }
    }

    #[test]
    fn an_event_is_24_bytes_on_64_bit() {
        let mut b = Vec::new();
        event(&mut b, EV_KEY, 30, 1);
        assert_eq!(b.len(), 2 * std::mem::size_of::<c_long>() + 8);
        assert_eq!(std::mem::size_of::<Setup>(), 92);
    }
}
