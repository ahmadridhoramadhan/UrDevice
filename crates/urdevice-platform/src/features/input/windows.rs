//! Windows input back-end: `SendInput` (design §9.3, R9.2).
//!
//! Text uses `KEYEVENTF_UNICODE`, one UTF-16 unit at a time, so it does not
//! depend on the keyboard layout (R10.4). Windows blocks injection into
//! windows that run elevated (and the secure desktop) unless UrDevice runs
//! elevated too; that is a documented limitation, and the calls just have no
//! effect there.

use std::mem::size_of;

use urdevice_core::services::input::{PointerEvent, button};
use urdevice_core::{CoreError, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    SendInput, VIRTUAL_KEY,
};

use super::Backend;
use super::keymap;

pub(crate) struct Windows;

impl Windows {
    pub(crate) fn new() -> Self {
        Self
    }
}

fn io(msg: &str) -> CoreError {
    CoreError::Io(std::io::Error::other(msg.to_owned()))
}

fn send(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: `inputs` is a valid slice of fully initialised INPUT structs and
    // the size is that of one element, as SendInput requires.
    let n = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        // Blocked by UIPI (an elevated window) or another desktop.
        Err(io("SendInput was blocked"))
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

impl Backend for Windows {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn pointer(&mut self, e: &PointerEvent) -> Result<()> {
        match *e {
            PointerEvent::Move { dx, dy } => send(&[mouse(MOUSEEVENTF_MOVE, dx, dy, 0)]),
            PointerEvent::Abs { x, y, .. } => send(&[mouse(
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                x as i32,
                y as i32,
                0,
            )]),
            PointerEvent::Button { button: b, down } => {
                let (flags, data) = match (b, down) {
                    (button::LEFT, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                    (button::LEFT, false) => (MOUSEEVENTF_LEFTUP, 0),
                    (button::RIGHT, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                    (button::RIGHT, false) => (MOUSEEVENTF_RIGHTUP, 0),
                    (button::MIDDLE, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                    (button::MIDDLE, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                    // XBUTTON1 is Back, XBUTTON2 is Forward.
                    (button::BACK, true) => (MOUSEEVENTF_XDOWN, 1),
                    (button::BACK, false) => (MOUSEEVENTF_XUP, 1),
                    (button::FORWARD, true) => (MOUSEEVENTF_XDOWN, 2),
                    (button::FORWARD, false) => (MOUSEEVENTF_XUP, 2),
                    _ => return Ok(()),
                };
                send(&[mouse(flags, 0, 0, data)])
            }
            PointerEvent::Scroll { dx, dy } => {
                let mut v = Vec::new();
                // The wire has down positive; Windows has away from the user
                // (up) positive for the vertical wheel. Right is positive for
                // both on the horizontal one.
                if dy != 0 {
                    v.push(mouse(MOUSEEVENTF_WHEEL, 0, 0, -dy));
                }
                if dx != 0 {
                    v.push(mouse(MOUSEEVENTF_HWHEEL, 0, 0, dx));
                }
                send(&v)
            }
        }
    }

    fn key(&mut self, usage: u32, down: bool) -> Result<()> {
        let Some((vk, extended)) = keymap::vk(usage) else {
            return Ok(());
        };
        let mut flags = KEYBD_EVENT_FLAGS(0);
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        send(&[keyboard(vk, 0, flags)])
    }

    fn text(&mut self, text: &str) -> Result<()> {
        let mut v = Vec::new();
        for ch in text.chars() {
            if ch == '\n' {
                v.push(keyboard(0x0D, 0, KEYBD_EVENT_FLAGS(0)));
                v.push(keyboard(0x0D, 0, KEYEVENTF_KEYUP));
                continue;
            }
            // Characters outside the BMP are two UTF-16 units, sent in order.
            let mut buf = [0u16; 2];
            for unit in ch.encode_utf16(&mut buf).iter() {
                v.push(keyboard(0, *unit, KEYEVENTF_UNICODE));
                v.push(keyboard(0, *unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
        // Long text goes in pieces so one call never grows without bound.
        for chunk in v.chunks(512) {
            send(chunk)?;
        }
        Ok(())
    }
}
