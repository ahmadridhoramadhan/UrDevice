//! macOS media host (R11.3, design §9.4).
//!
//! Now-playing information is private API (MediaRemote), so the host offers
//! one session named "System" with no title and no artwork. Its commands are
//! the system media keys (play/pause, next, previous), posted as
//! system-defined events. Volume and mute go through the system's own
//! `volume settings`, which also handles output devices that have no master
//! channel.
//!
//! Untested on a Mac (no device); it compiles in CI.

use std::process::{Command, Stdio};

use objc2::encode::{Encoding, RefEncode};
use objc2::msg_send;
use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_foundation::NSPoint;
use urdevice_core::services::Grant;
use urdevice_core::services::media::{
    CmdKind, MediaHost, MediaSession, MediaState, PlayState, VolumeChange,
};
use urdevice_core::{CoreError, Result};

use super::parse::{osascript_bool, osascript_volume};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

// `input/macos.rs` declares `CGEventPost` too, with a plain `c_void` event pointer.
#[allow(clashing_extern_declarations)]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventPost(tap: u32, event: *mut CgEvent);
}

/// `CGEventRef` points at this opaque struct.
#[repr(C)]
struct CgEvent {
    _private: [u8; 0],
}

// SAFETY: `__CGEvent` is the name Objective-C gives the struct behind
// `CGEventRef`, so a pointer to it has the encoding `^{__CGEvent=}`.
unsafe impl RefEncode for CgEvent {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CGEvent", &[]));
}

const SESSION_ID: &str = "system";

/// `NSEventTypeSystemDefined`.
const NS_EVENT_SYSTEM_DEFINED: usize = 14;
/// `NX_SUBTYPE_AUX_CONTROL_BUTTONS`.
const SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;
/// `kCGHIDEventTap`.
const HID_EVENT_TAP: u32 = 0;

const KEY_PLAY: isize = 16;
const KEY_NEXT: isize = 17;
const KEY_PREVIOUS: isize = 18;

pub(crate) struct System;

impl System {
    pub(crate) fn new() -> Self {
        Self
    }
}

/// Press and release one media key.
fn media_key(key: isize) -> Result<()> {
    let ok = autoreleasepool(|_| {
        let Some(cls) = AnyClass::get(c"NSEvent") else {
            return false;
        };
        let mut all = true;
        for down in [true, false] {
            let state: isize = if down { 0xA } else { 0xB };
            let data1: isize = (key << 16) | (state << 8);
            let flags: usize = (state as usize) << 8;
            // SAFETY: `+[NSEvent otherEventWithType:location:modifierFlags:
            // timestamp:windowNumber:context:subtype:data1:data2:]` takes
            // exactly these argument types and returns an autoreleased
            // NSEvent (or nil), which `-CGEvent` turns into a CGEventRef
            // owned by the NSEvent. CGEventPost copies what it needs.
            unsafe {
                let ev: *mut AnyObject = msg_send![
                    cls,
                    otherEventWithType: NS_EVENT_SYSTEM_DEFINED,
                    location: NSPoint { x: 0.0, y: 0.0 },
                    modifierFlags: flags,
                    timestamp: 0.0f64,
                    windowNumber: 0isize,
                    context: std::ptr::null_mut::<AnyObject>(),
                    subtype: SUBTYPE_AUX_CONTROL_BUTTONS,
                    data1: data1,
                    data2: -1isize
                ];
                if ev.is_null() {
                    all = false;
                    continue;
                }
                let cg: *mut CgEvent = msg_send![ev, CGEvent];
                if cg.is_null() {
                    all = false;
                    continue;
                }
                CGEventPost(HID_EVENT_TAP, cg);
            }
        }
        all
    });
    if ok {
        Ok(())
    } else {
        Err(CoreError::Io(std::io::Error::other(
            "could not post a media key",
        )))
    }
}

fn osascript(script: &str) -> Option<String> {
    let out = Command::new("osascript")
        .args(["-e", script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn read_volume() -> Option<(f32, bool)> {
    let level = osascript_volume(&osascript("output volume of (get volume settings)")?)?;
    let muted = osascript("output muted of (get volume settings)")
        .as_deref()
        .and_then(osascript_bool)
        .unwrap_or(false);
    Some((level, muted))
}

fn set_level(level: f32) -> Result<()> {
    let pct = (level.clamp(0.0, 1.0) * 100.0).round() as u32;
    osascript(&format!("set volume output volume {pct}"))
        .map(|_| ())
        .ok_or_else(|| CoreError::Io(std::io::Error::other("could not set the volume")))
}

impl MediaHost for System {
    fn state(&self) -> MediaState {
        let vol = read_volume();
        MediaState {
            sessions: vec![MediaSession {
                id: SESSION_ID.into(),
                app_name: "System".into(),
                state: PlayState::Stopped as i32,
                can_next: true,
                can_prev: true,
                can_play_pause: true,
                ..MediaSession::default()
            }],
            volume: vol.map_or(0.0, |v| v.0),
            muted: vol.is_some_and(|v| v.1),
            can_volume: vol.is_some(),
            unavailable: String::new(),
        }
    }

    fn command(
        &self,
        _grant: &Grant,
        session_id: &str,
        kind: CmdKind,
        _seek_ms: u64,
    ) -> Result<()> {
        if session_id != SESSION_ID {
            return Err(CoreError::InvalidInput("bad media session".into()));
        }
        match kind {
            CmdKind::PlayPause => media_key(KEY_PLAY),
            CmdKind::Next => media_key(KEY_NEXT),
            CmdKind::Prev => media_key(KEY_PREVIOUS),
            // The System session says `can_seek = false`.
            CmdKind::Seek => Err(CoreError::unsupported("media seek", "macos")),
        }
    }

    fn volume(&self, _grant: &Grant, change: &VolumeChange) -> Result<()> {
        match change {
            VolumeChange::Set(v) => set_level(if v.is_finite() { *v } else { 0.0 }),
            VolumeChange::Step(n) => {
                let (now, _) =
                    read_volume().ok_or_else(|| CoreError::unsupported("media volume", "macos"))?;
                set_level(now + *n as f32 * 0.02)
            }
            VolumeChange::Mute(m) => osascript(&format!("set volume output muted {m}"))
                .map(|_| ())
                .ok_or_else(|| CoreError::Io(std::io::Error::other("could not set mute"))),
        }
    }
}
