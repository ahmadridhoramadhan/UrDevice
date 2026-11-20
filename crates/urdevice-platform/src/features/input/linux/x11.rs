//! X11 back-end: the XTest extension (design §9.3, R9.2).
//!
//! Pointer and keys are fake input events. XTest has no Unicode text, so text
//! goes through a spare keycode: the keycode gets the character's keysym for
//! the moment of the key press, the way `xdotool type` does it, and its old
//! mapping is put back after the text (R10.4).

use std::time::Duration;

use urdevice_core::services::input::{PointerEvent, button};
use urdevice_core::{CoreError, Result};
use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use super::super::keymap::evdev;
use super::super::{Backend, ScrollAccum};
use super::keysym;

/// X keycodes are evdev codes plus 8 on every XKB server in use.
const EVDEV_OFFSET: u16 = 8;

pub(super) struct X11 {
    conn: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    /// The keycode used to type characters the layout does not have.
    scratch: u8,
    per_keycode: u8,
    /// What `scratch` held before text was typed.
    original: Vec<u32>,
    scroll: ScrollAccum,
}

pub(super) fn open() -> std::result::Result<Box<dyn Backend>, String> {
    let (conn, screen_num) = RustConnection::connect(None).map_err(|e| e.to_string())?;
    let (root, width, height, min, max) = {
        let setup = conn.setup();
        let screen = &setup.roots[screen_num];
        (
            screen.root,
            screen.width_in_pixels,
            screen.height_in_pixels,
            setup.min_keycode,
            setup.max_keycode,
        )
    };
    // Fails when the server has no XTEST extension.
    conn.xtest_get_version(2, 2)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let count = max - min + 1;
    let map = conn
        .get_keyboard_mapping(min, count)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let per = usize::from(map.keysyms_per_keycode).max(1);
    // A keycode with no symbol at all is free; otherwise take the last one.
    let free = map
        .keysyms
        .chunks(per)
        .position(|c| c.iter().all(|s| *s == 0));
    let index = free.unwrap_or(usize::from(count) - 1);
    let scratch = min + index as u8;
    let original = map.keysyms[index * per..(index + 1) * per].to_vec();
    Ok(Box::new(X11 {
        conn,
        root,
        width,
        height,
        scratch,
        per_keycode: map.keysyms_per_keycode.max(1),
        original,
        scroll: ScrollAccum::default(),
    }))
}

fn xerr(e: impl std::fmt::Display) -> CoreError {
    CoreError::Io(std::io::Error::other(e.to_string()))
}

impl X11 {
    fn fake(&self, kind: u8, detail: u8, root: Window, px: i16, py: i16) -> Result<()> {
        self.conn
            .xtest_fake_input(kind, detail, 0, root, px, py, 0)
            .map_err(xerr)?;
        Ok(())
    }

    fn button_click(&self, b: u8) -> Result<()> {
        self.fake(BUTTON_PRESS_EVENT, b, NONE, 0, 0)?;
        self.fake(BUTTON_RELEASE_EVENT, b, NONE, 0, 0)
    }

    fn keycode(usage: u32) -> Option<u8> {
        evdev(usage).and_then(|c| u8::try_from(c + EVDEV_OFFSET).ok())
    }

    fn set_scratch(&self, keysyms: &[u32]) -> Result<()> {
        self.conn
            .change_keyboard_mapping(1, self.scratch, self.per_keycode, keysyms)
            .map_err(xerr)?;
        // Wait until the server applied it.
        self.conn
            .get_input_focus()
            .map_err(xerr)?
            .reply()
            .map_err(xerr)?;
        Ok(())
    }

    fn type_char(&self, ch: char) -> Result<()> {
        let key = match ch {
            '\n' => Self::keycode(0x28),
            '\t' => Self::keycode(0x2B),
            _ => None,
        };
        if let Some(code) = key {
            self.fake(KEY_PRESS_EVENT, code, NONE, 0, 0)?;
            return self.fake(KEY_RELEASE_EVENT, code, NONE, 0, 0);
        }
        let Some(sym) = keysym(ch) else { return Ok(()) };
        self.set_scratch(&vec![sym; usize::from(self.per_keycode)])?;
        self.fake(KEY_PRESS_EVENT, self.scratch, NONE, 0, 0)?;
        self.fake(KEY_RELEASE_EVENT, self.scratch, NONE, 0, 0)?;
        self.conn
            .get_input_focus()
            .map_err(xerr)?
            .reply()
            .map_err(xerr)?;
        // Let the application read the new mapping before the next change.
        std::thread::sleep(Duration::from_millis(3));
        Ok(())
    }
}

impl Backend for X11 {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn pointer(&mut self, e: &PointerEvent) -> Result<()> {
        let clamp = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        match *e {
            PointerEvent::Move { dx, dy } => {
                // detail 1 means the position is relative to the pointer.
                self.fake(MOTION_NOTIFY_EVENT, 1, NONE, clamp(dx), clamp(dy))?;
            }
            PointerEvent::Abs { x, y, .. } => {
                // The whole X screen: Xinerama and RandR joins are one screen.
                let px = u32::from(self.width.saturating_sub(1)) * x / 65535;
                let py = u32::from(self.height.saturating_sub(1)) * y / 65535;
                self.fake(
                    MOTION_NOTIFY_EVENT,
                    0,
                    self.root,
                    clamp(px as i32),
                    clamp(py as i32),
                )?;
            }
            PointerEvent::Button { button: b, down } => {
                let n = match b {
                    button::LEFT => 1,
                    button::MIDDLE => 2,
                    button::RIGHT => 3,
                    button::BACK => 8,
                    button::FORWARD => 9,
                    _ => return Ok(()),
                };
                let kind = if down {
                    BUTTON_PRESS_EVENT
                } else {
                    BUTTON_RELEASE_EVENT
                };
                self.fake(kind, n, NONE, 0, 0)?;
            }
            PointerEvent::Scroll { dx, dy } => {
                // Buttons 4 and 5 are up and down, 6 and 7 left and right.
                let (nx, ny) = self.scroll.add(dx, dy);
                let vertical = if ny < 0 { 4 } else { 5 };
                for _ in 0..ny.unsigned_abs().min(50) {
                    self.button_click(vertical)?;
                }
                let horizontal = if nx < 0 { 6 } else { 7 };
                for _ in 0..nx.unsigned_abs().min(50) {
                    self.button_click(horizontal)?;
                }
            }
        }
        self.conn.flush().map_err(xerr)
    }

    fn key(&mut self, usage: u32, down: bool) -> Result<()> {
        let Some(code) = Self::keycode(usage) else {
            return Ok(());
        };
        let kind = if down {
            KEY_PRESS_EVENT
        } else {
            KEY_RELEASE_EVENT
        };
        self.fake(kind, code, NONE, 0, 0)?;
        self.conn.flush().map_err(xerr)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        let mut result = Ok(());
        for ch in text.chars() {
            if let Err(e) = self.type_char(ch) {
                result = Err(e);
                break;
            }
        }
        // Put the spare keycode back, even after an error.
        let restore = self.set_scratch(&self.original.clone());
        self.conn.flush().map_err(xerr)?;
        result.and(restore)
    }
}
