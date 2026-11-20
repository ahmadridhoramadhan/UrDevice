//! macOS input back-end: `CGEventPost` (design §9.3, R9.2).
//!
//! Posting events needs the Accessibility permission (Privacy & Security >
//! Accessibility). [`trusted`] checks it; [`open`] asks macOS to show its
//! prompt when `prompt` is set, and then reports the permission as missing
//! until the user turns it on. Text uses `CGEventKeyboardSetUnicodeString`,
//! so it does not depend on the keyboard layout (R10.4).
//!
//! The Core Graphics and Application Services calls are declared here
//! directly: a handful of C functions, no extra crate.
//!
//! Untested on a Mac (none available): it compiles in CI only.

use std::ffi::c_void;

use urdevice_core::services::input::{Missing, PointerEvent, button};
use urdevice_core::{CoreError, Result};

use super::keymap;
use super::{Backend, Fail};

type CGEventRef = *mut c_void;
type CGEventSourceRef = *mut c_void;
type CFTypeRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFStringRef = *const c_void;
type CFBooleanRef = *const c_void;
type CFAllocatorRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

/// `kCGEventSourceStateHIDSystemState`.
const SOURCE_HID_SYSTEM_STATE: i32 = 1;
/// `kCGHIDEventTap`.
const HID_EVENT_TAP: u32 = 0;
/// `kCGScrollEventUnitPixel`.
const SCROLL_UNIT_PIXEL: u32 = 0;

// CGEventType
const LEFT_MOUSE_DOWN: u32 = 1;
const LEFT_MOUSE_UP: u32 = 2;
const RIGHT_MOUSE_DOWN: u32 = 3;
const RIGHT_MOUSE_UP: u32 = 4;
const MOUSE_MOVED: u32 = 5;
const LEFT_MOUSE_DRAGGED: u32 = 6;
const RIGHT_MOUSE_DRAGGED: u32 = 7;
const OTHER_MOUSE_DOWN: u32 = 25;
const OTHER_MOUSE_UP: u32 = 26;
const OTHER_MOUSE_DRAGGED: u32 = 27;

// CGMouseButton
const BUTTON_LEFT: u32 = 0;
const BUTTON_RIGHT: u32 = 1;
const BUTTON_CENTER: u32 = 2;

// CGEventField
const FIELD_MOUSE_CLICK_STATE: u32 = 1;
const FIELD_MOUSE_DELTA_X: u32 = 4;
const FIELD_MOUSE_DELTA_Y: u32 = 5;
const FIELD_MOUSE_BUTTON_NUMBER: u32 = 3;

// CGEventFlags
const FLAG_SHIFT: u64 = 0x0002_0000;
const FLAG_CONTROL: u64 = 0x0004_0000;
const FLAG_ALTERNATE: u64 = 0x0008_0000;
const FLAG_COMMAND: u64 = 0x0010_0000;

/// Pixels in one wheel notch (120 wire units).
const PIXELS_PER_NOTCH: i32 = 40;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceCreate(state: i32) -> CGEventSourceRef;
    fn CGEventCreate(source: CGEventSourceRef) -> CGEventRef;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    fn CGEventCreateMouseEvent(
        source: CGEventSourceRef,
        kind: u32,
        position: CGPoint,
        button: u32,
    ) -> CGEventRef;
    fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, key: u16, down: bool) -> CGEventRef;
    fn CGEventCreateScrollWheelEvent2(
        source: CGEventSourceRef,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        wheel2: i32,
        wheel3: i32,
    ) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(event: CGEventRef, len: usize, text: *const u16);
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    static kCFBooleanTrue: CFBooleanRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// True when this app may post events (Accessibility is on).
pub(crate) fn trusted() -> bool {
    // SAFETY: a plain query with no arguments.
    unsafe { AXIsProcessTrusted() }
}

/// Ask macOS to show its "would like to control this computer" prompt, which
/// also adds UrDevice to the Accessibility list. Returns whether it is on.
fn prompt_for_trust() -> bool {
    // SAFETY: the key and value are the framework's own constants; the
    // dictionary is created, used for one call, and released here.
    unsafe {
        let keys: [CFTypeRef; 1] = [kAXTrustedCheckOptionPrompt];
        let values: [CFTypeRef; 1] = [kCFBooleanTrue];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        let ok = AXIsProcessTrustedWithOptions(dict);
        if !dict.is_null() {
            CFRelease(dict);
        }
        ok
    }
}

pub(crate) fn open(prompt: bool) -> std::result::Result<Box<dyn Backend>, Fail> {
    let ok = if prompt {
        prompt_for_trust()
    } else {
        trusted()
    };
    if !ok {
        return Err(Fail::Missing(Missing::Accessibility));
    }
    // SAFETY: creates a source object that this back-end owns until dropped.
    let source = unsafe { CGEventSourceCreate(SOURCE_HID_SYSTEM_STATE) };
    if source.is_null() {
        return Err(Fail::Unavailable("no event source"));
    }
    Ok(Box::new(Mac {
        source,
        flags: 0,
        down: [false; 3],
    }))
}

pub(crate) struct Mac {
    source: CGEventSourceRef,
    /// Modifier flags held now, put on every event.
    flags: u64,
    /// Left, right, and middle buttons held (for drag events).
    down: [bool; 3],
}

// SAFETY: the source is only used from the worker thread that owns the
// back-end, one call at a time, and CGEventSource is safe to use that way.
unsafe impl Send for Mac {}

impl Drop for Mac {
    fn drop(&mut self) {
        // SAFETY: `source` came from CGEventSourceCreate and is released once.
        unsafe { CFRelease(self.source as CFTypeRef) };
    }
}

fn failed() -> CoreError {
    CoreError::Io(std::io::Error::other(
        "could not create a Core Graphics event",
    ))
}

impl Mac {
    fn post(&self, event: CGEventRef) -> Result<()> {
        if event.is_null() {
            return Err(failed());
        }
        // SAFETY: `event` is a live event this call owns; it is released after
        // posting.
        unsafe {
            CGEventSetFlags(event, self.flags);
            CGEventPost(HID_EVENT_TAP, event);
            CFRelease(event as CFTypeRef);
        }
        Ok(())
    }

    fn location(&self) -> CGPoint {
        // SAFETY: an event made from the source reports the cursor position;
        // it is released straight away.
        unsafe {
            let e = CGEventCreate(self.source);
            if e.is_null() {
                return CGPoint { x: 0.0, y: 0.0 };
            }
            let p = CGEventGetLocation(e);
            CFRelease(e as CFTypeRef);
            p
        }
    }

    fn mouse(&self, kind: u32, at: CGPoint, btn: u32, delta: (i64, i64)) -> Result<()> {
        // SAFETY: plain construction of an event, checked for null in `post`.
        let e = unsafe { CGEventCreateMouseEvent(self.source, kind, at, btn) };
        if !e.is_null() {
            // SAFETY: `e` is non-null and owned here.
            unsafe {
                CGEventSetIntegerValueField(e, FIELD_MOUSE_DELTA_X, delta.0);
                CGEventSetIntegerValueField(e, FIELD_MOUSE_DELTA_Y, delta.1);
            }
        }
        self.post(e)
    }

    fn held(&self) -> Option<usize> {
        self.down.iter().position(|d| *d)
    }

    fn key_event(&self, code: u16, down: bool) -> Result<()> {
        // SAFETY: plain construction, checked for null in `post`.
        let e = unsafe { CGEventCreateKeyboardEvent(self.source, code, down) };
        self.post(e)
    }
}

/// The flag a modifier key sets while it is held.
fn modifier_flag(usage: u32) -> u64 {
    match usage {
        0xE0 | 0xE4 => FLAG_CONTROL,
        0xE1 | 0xE5 => FLAG_SHIFT,
        0xE2 | 0xE6 => FLAG_ALTERNATE,
        0xE3 | 0xE7 => FLAG_COMMAND,
        _ => 0,
    }
}

impl Backend for Mac {
    fn name(&self) -> &'static str {
        "macos"
    }

    fn pointer(&mut self, e: &PointerEvent) -> Result<()> {
        // SAFETY: a plain query.
        let bounds = unsafe { CGDisplayBounds(CGMainDisplayID()) };
        match *e {
            PointerEvent::Move { dx, dy } => {
                let p = self.location();
                // Keep the cursor on the main display.
                let nx = (p.x + f64::from(dx))
                    .clamp(bounds.origin.x, bounds.origin.x + bounds.size.width - 1.0);
                let ny = (p.y + f64::from(dy))
                    .clamp(bounds.origin.y, bounds.origin.y + bounds.size.height - 1.0);
                let kind = match self.held() {
                    Some(0) => LEFT_MOUSE_DRAGGED,
                    Some(1) => RIGHT_MOUSE_DRAGGED,
                    Some(_) => OTHER_MOUSE_DRAGGED,
                    None => MOUSE_MOVED,
                };
                let btn = self.held().map_or(BUTTON_LEFT, |b| b as u32);
                self.mouse(
                    kind,
                    CGPoint { x: nx, y: ny },
                    btn,
                    (i64::from(dx), i64::from(dy)),
                )
            }
            PointerEvent::Abs { x, y, .. } => {
                let fx = f64::from(x) / 65535.0;
                let fy = f64::from(y) / 65535.0;
                let at = CGPoint {
                    x: bounds.origin.x + fx * (bounds.size.width - 1.0),
                    y: bounds.origin.y + fy * (bounds.size.height - 1.0),
                };
                let kind = if self.held().is_some() {
                    LEFT_MOUSE_DRAGGED
                } else {
                    MOUSE_MOVED
                };
                self.mouse(kind, at, BUTTON_LEFT, (0, 0))
            }
            PointerEvent::Button { button: b, down } => {
                let (kind, btn, slot) = match (b, down) {
                    (button::LEFT, true) => (LEFT_MOUSE_DOWN, BUTTON_LEFT, 0),
                    (button::LEFT, false) => (LEFT_MOUSE_UP, BUTTON_LEFT, 0),
                    (button::RIGHT, true) => (RIGHT_MOUSE_DOWN, BUTTON_RIGHT, 1),
                    (button::RIGHT, false) => (RIGHT_MOUSE_UP, BUTTON_RIGHT, 1),
                    (button::MIDDLE, true) => (OTHER_MOUSE_DOWN, BUTTON_CENTER, 2),
                    (button::MIDDLE, false) => (OTHER_MOUSE_UP, BUTTON_CENTER, 2),
                    (button::BACK | button::FORWARD, _) => {
                        // Buttons 4 and 5 are "other" buttons with numbers 3 and 4.
                        let n = if b == button::BACK { 3 } else { 4 };
                        let kind = if down {
                            OTHER_MOUSE_DOWN
                        } else {
                            OTHER_MOUSE_UP
                        };
                        let at = self.location();
                        // SAFETY: plain construction, checked for null below.
                        let ev = unsafe {
                            CGEventCreateMouseEvent(self.source, kind, at, BUTTON_CENTER)
                        };
                        if !ev.is_null() {
                            // SAFETY: `ev` is non-null and owned here.
                            unsafe {
                                CGEventSetIntegerValueField(ev, FIELD_MOUSE_BUTTON_NUMBER, n)
                            };
                        }
                        return self.post(ev);
                    }
                    _ => return Ok(()),
                };
                self.down[slot] = down;
                let at = self.location();
                // SAFETY: plain construction, checked for null below.
                let ev = unsafe { CGEventCreateMouseEvent(self.source, kind, at, btn) };
                if !ev.is_null() && down {
                    // SAFETY: `ev` is non-null and owned here.
                    unsafe { CGEventSetIntegerValueField(ev, FIELD_MOUSE_CLICK_STATE, 1) };
                }
                self.post(ev)
            }
            PointerEvent::Scroll { dx, dy } => {
                // The wire has down and right positive. Core Graphics has up
                // and left positive for wheel 1 and wheel 2.
                let px = |v: i32| -> i32 {
                    let exact = f64::from(v) * f64::from(PIXELS_PER_NOTCH) / 120.0;
                    exact.round() as i32
                };
                let (sy, sx) = (-px(dy), -px(dx));
                if sy == 0 && sx == 0 {
                    return Ok(());
                }
                // SAFETY: plain construction, checked for null in `post`.
                let ev = unsafe {
                    CGEventCreateScrollWheelEvent2(self.source, SCROLL_UNIT_PIXEL, 2, sy, sx, 0)
                };
                self.post(ev)
            }
        }
    }

    fn key(&mut self, usage: u32, down: bool) -> Result<()> {
        let Some(code) = keymap::mac(usage) else {
            return Ok(());
        };
        let flag = modifier_flag(usage);
        if down {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
        self.key_event(code, down)
    }

    fn text(&mut self, text: &str) -> Result<()> {
        // A few UTF-16 units per event keeps apps that cap the string happy.
        // A newline is the Return key, which every app understands.
        let mut units: Vec<u16> = Vec::new();
        let flush = |this: &Self, units: &mut Vec<u16>| -> Result<()> {
            for chunk in units.chunks(16) {
                for down in [true, false] {
                    // SAFETY: plain construction, checked for null in `post`.
                    let ev = unsafe { CGEventCreateKeyboardEvent(this.source, 0, down) };
                    if !ev.is_null() {
                        // SAFETY: `chunk` outlives the call, which copies the string.
                        unsafe { CGEventKeyboardSetUnicodeString(ev, chunk.len(), chunk.as_ptr()) };
                    }
                    this.post(ev)?;
                }
            }
            units.clear();
            Ok(())
        };
        let mut buf = [0u16; 2];
        for ch in text.chars() {
            if ch == '\n' {
                flush(self, &mut units)?;
                self.key_event(0x24, true)?;
                self.key_event(0x24, false)?;
            } else {
                units.extend_from_slice(ch.encode_utf16(&mut buf));
            }
        }
        flush(self, &mut units)
    }
}
