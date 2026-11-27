//! Android host: pointer, key, and text events become taps, swipes, global
//! actions, and keyboard input (#19, R9.3, R10.3, design §9.3).
//!
//! A phone has no cursor. [`TouchSink`] keeps a virtual pointer and turns the
//! controller's events into what Android's accessibility service can run:
//!
//! - **Pointer position.** `Abs` sets it, `Move` shifts it (in pixels of the
//!   host's display).
//! - **Left button.** Press and release at the same place is a tap. Held for
//!   [`LONG_PRESS_MS`] or more, it is a long press. Moved while held, it is a
//!   swipe along the path the pointer took, in the time the user took. The
//!   service can only run a finished stroke, so the gesture runs when the
//!   button is released.
//! - **Wheel.** A short swipe from the pointer; wheel down moves the finger up.
//! - **Right button, or the side "back" button.** Back, like a mouse on Android.
//! - **`Nav` events.** Back, Home, Recents.
//! - **Text and keys.** The UrDevice keyboard commits the text. Keys go to the
//!   focused field as key events with the modifiers held, so Ctrl+A and Ctrl+C
//!   work where the app accepts them. Escape is Back.
//!
//! The Kotlin side implements [`TouchHost`]. Nothing here logs where the
//! pointer is or what is typed.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use super::{COORD_MAX, KeyEvent, Missing, PermStatus, PointerEvent, SCROLL_NOTCH, button, usage};
use crate::services::Grant;
use crate::{CoreError, Result};

/// A press that lasts this long is a long press (Android's own limit is 400 ms).
pub const LONG_PRESS_MS: u64 = 500;

/// A tap lasts this long, so Android takes it as a click and not a press.
const TAP_MS: u32 = 50;

/// A long press never runs shorter than this: it must clear Android's limit.
const LONG_PRESS_MIN_MS: u32 = 600;

/// No gesture runs longer than this.
pub const MAX_GESTURE_MS: u32 = 5_000;

/// A swipe is never shorter than this.
const MIN_SWIPE_MS: u32 = 60;

/// Movement under this many pixels is still a tap (Android's touch slop).
const TAP_SLOP_PX: u32 = 12;

/// Most points a swipe carries to the host.
pub const MAX_SWIPE_POINTS: usize = 24;

/// Most points the sink keeps while the button is down. The rest of the path
/// is dropped, but the last position still counts.
const MAX_STROKE_POINTS: usize = 512;

/// Fraction of the display one wheel notch scrolls.
const SCROLL_PER_NOTCH: f32 = 0.12;

/// The most one wheel event scrolls, as a fraction of the display.
const SCROLL_MAX: f32 = 0.6;

/// How long a wheel swipe lasts.
const SCROLL_MS: u32 = 180;

/// What the sink assumes for a host that does not know its display size.
const FALLBACK_SIZE: (u32, u32) = (1080, 2400);

/// Android modifier bits of `KeyEvent.getMetaState()`.
pub mod meta {
    /// `META_SHIFT_ON`.
    pub const SHIFT: u32 = 0x1;
    /// `META_ALT_ON`.
    pub const ALT: u32 = 0x2;
    /// `META_CTRL_ON`.
    pub const CTRL: u32 = 0x1000;
    /// `META_META_ON`.
    pub const META: u32 = 0x1_0000;
}

/// A gesture for the accessibility service, in pixels of the display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Touch one point for `duration_ms`.
    Tap {
        /// Pixels from the left.
        x: u32,
        /// Pixels from the top.
        y: u32,
        /// How long the finger stays down.
        duration_ms: u32,
    },
    /// Drag along `points` (at least two) in `duration_ms`.
    Swipe {
        /// The path, start first.
        points: Vec<(u32, u32)>,
        /// How long the stroke takes.
        duration_ms: u32,
    },
}

/// What Android's accessibility service and keyboard provide. Implemented in
/// Kotlin. Methods may block briefly; the core calls them from its input
/// worker, one at a time. Each takes the [`Grant`] so an adapter never acts
/// without proof the permission engine allowed this peer.
pub trait TouchHost: Send + Sync {
    /// True while the UrDevice accessibility service is turned on and
    /// connected.
    fn gestures_ready(&self) -> bool;

    /// True while the UrDevice keyboard is turned on and is the keyboard in use.
    fn text_ready(&self) -> bool;

    /// The display in pixels; `(0, 0)` when unknown.
    fn display_size(&self) -> (u32, u32);

    /// Run a gesture. `Ok` when the service accepted it.
    fn gesture(&self, grant: &Grant, gesture: &Gesture) -> Result<()>;

    /// Run a global action: one of [`nav`](super::nav).
    fn nav(&self, grant: &Grant, action: u32) -> Result<()>;

    /// Commit text at the cursor of the focused field. No control characters
    /// other than tab; newlines arrive as [`TouchHost::key`] Enter.
    fn text(&self, grant: &Grant, text: &str) -> Result<()>;

    /// Send a key event to the focused field. `code` is an Android
    /// `KeyEvent.KEYCODE_*`, `meta` the [`meta`] bits held.
    fn key(&self, grant: &Grant, code: u32, down: bool, meta: u32) -> Result<()>;

    /// Show the pointer on the phone's screen at `(x, y)` pixels, or hide it. The controller's
    /// mouse moves this pointer, so the user sees where a click will land. Cosmetic: a host
    /// that cannot draw it does nothing.
    fn cursor(&self, grant: &Grant, x: u32, y: u32, shown: bool) -> Result<()> {
        let _ = (grant, x, y, shown);
        Ok(())
    }
}

/// An Android key code for a USB HID usage, or `None` for a key the phone has
/// no use for. Letters, digits, editing and navigation keys, and F1 to F12.
pub fn android_keycode(u: u32) -> Option<u32> {
    Some(match u {
        // A to Z: KEYCODE_A is 29.
        0x04..=0x1D => 29 + (u - 0x04),
        // 1 to 9: KEYCODE_1 is 8.
        0x1E..=0x26 => 8 + (u - 0x1E),
        0x27 => 7, // 0
        usage::ENTER => 66,
        usage::BACKSPACE => 67, // KEYCODE_DEL
        usage::TAB => 61,
        usage::SPACE => 62,
        0x2D => 69, // -
        0x2E => 70, // =
        0x2F => 71, // [
        0x30 => 72, // ]
        0x31 => 73, // \
        0x33 => 74, // ;
        0x34 => 75, // '
        0x35 => 68, // `
        0x36 => 55, // ,
        0x37 => 56, // .
        0x38 => 76, // /
        usage::F1..=0x45 => 131 + (u - usage::F1),
        usage::PAGE_UP => 92,
        usage::DELETE => 112, // KEYCODE_FORWARD_DEL
        usage::HOME => 122,   // KEYCODE_MOVE_HOME
        usage::END => 123,    // KEYCODE_MOVE_END
        usage::PAGE_DOWN => 93,
        usage::RIGHT => 22,
        usage::LEFT => 21,
        usage::DOWN => 20,
        usage::UP => 19,
        _ => return None,
    })
}

/// The [`meta`] bit for a modifier key usage, or `None` for any other key.
fn modifier_bit(u: u32) -> Option<u32> {
    match u {
        usage::LEFT_CTRL | usage::RIGHT_CTRL => Some(meta::CTRL),
        usage::LEFT_SHIFT | usage::RIGHT_SHIFT => Some(meta::SHIFT),
        usage::LEFT_ALT | usage::RIGHT_ALT => Some(meta::ALT),
        usage::LEFT_META | usage::RIGHT_META => Some(meta::META),
        _ => None,
    }
}

/// A finger the sink is building: the left button is down.
#[derive(Debug)]
struct Stroke {
    start_ms: u64,
    /// Normalized path, start first.
    path: Vec<(f32, f32)>,
}

/// What the state machine decided; [`TouchSink`] runs it.
#[derive(Debug, PartialEq)]
enum Act {
    Gesture(Gesture),
    Nav(u32),
}

/// The pointer and the stroke, with no clock and no host: all time comes in
/// as `now_ms`, so tests run it directly.
#[derive(Debug)]
struct State {
    /// Pointer position, 0.0 to 1.0 across and down the display.
    pos: (f32, f32),
    stroke: Option<Stroke>,
}

impl State {
    fn new() -> Self {
        Self {
            pos: (0.5, 0.5),
            stroke: None,
        }
    }

    fn set_pos(&mut self, x: f32, y: f32) {
        self.pos = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
        if let Some(s) = &mut self.stroke
            && s.path.len() < MAX_STROKE_POINTS
        {
            s.path.push(self.pos);
        }
    }

    fn apply(&mut self, e: &PointerEvent, now_ms: u64, size: (u32, u32)) -> Vec<Act> {
        let (w, h) = if size.0 > 0 && size.1 > 0 {
            size
        } else {
            FALLBACK_SIZE
        };
        match *e {
            PointerEvent::Abs { x, y, .. } => {
                self.set_pos(
                    x.min(COORD_MAX) as f32 / COORD_MAX as f32,
                    y.min(COORD_MAX) as f32 / COORD_MAX as f32,
                );
                Vec::new()
            }
            PointerEvent::Move { dx, dy } => {
                self.set_pos(
                    self.pos.0 + dx as f32 / w as f32,
                    self.pos.1 + dy as f32 / h as f32,
                );
                Vec::new()
            }
            PointerEvent::Button { button, down } => self.button(button, down, now_ms, (w, h)),
            PointerEvent::Scroll { dx, dy } => {
                let per = |v: i32| {
                    (v as f32 / SCROLL_NOTCH as f32 * SCROLL_PER_NOTCH)
                        .clamp(-SCROLL_MAX, SCROLL_MAX)
                };
                // Wheel down scrolls the content down: the finger moves up.
                let (from, to) = (self.pos, (self.pos.0 - per(dx), self.pos.1 - per(dy)));
                let to = (to.0.clamp(0.0, 1.0), to.1.clamp(0.0, 1.0));
                if px(from, (w, h)) == px(to, (w, h)) {
                    return Vec::new();
                }
                vec![Act::Gesture(Gesture::Swipe {
                    points: vec![px(from, (w, h)), px(to, (w, h))],
                    duration_ms: SCROLL_MS,
                })]
            }
        }
    }

    fn button(&mut self, b: u32, down: bool, now_ms: u64, size: (u32, u32)) -> Vec<Act> {
        match (b, down) {
            (button::LEFT, true) => {
                // A lost release: finish the old stroke first.
                let acts = self.finish(now_ms, size);
                self.stroke = Some(Stroke {
                    start_ms: now_ms,
                    path: vec![self.pos],
                });
                acts
            }
            (button::LEFT, false) => self.finish(now_ms, size),
            // The secondary and "back" buttons are Back, as for a mouse on Android.
            (button::RIGHT | button::BACK, true) => vec![Act::Nav(super::nav::BACK)],
            _ => Vec::new(),
        }
    }

    /// End the stroke, if any: a tap, a long press, or a swipe.
    fn finish(&mut self, now_ms: u64, size: (u32, u32)) -> Vec<Act> {
        let Some(stroke) = self.stroke.take() else {
            return Vec::new();
        };
        let held = now_ms.saturating_sub(stroke.start_ms);
        let start = px(stroke.path[0], size);
        let end = px(self.pos, size);
        let far = stroke
            .path
            .iter()
            .any(|p| dist(px(*p, size), start) > TAP_SLOP_PX);
        if !far {
            let duration_ms = if held >= LONG_PRESS_MS {
                (held as u32).clamp(LONG_PRESS_MIN_MS, MAX_GESTURE_MS)
            } else {
                TAP_MS
            };
            return vec![Act::Gesture(Gesture::Tap {
                x: start.0,
                y: start.1,
                duration_ms,
            })];
        }
        let mut points: Vec<(u32, u32)> = thin(&stroke.path, MAX_SWIPE_POINTS)
            .into_iter()
            .map(|p| px(p, size))
            .collect();
        // The last known position is where the finger lifts.
        if points.last() != Some(&end) {
            points.push(end);
        }
        points.dedup();
        if points.len() < 2 {
            return Vec::new();
        }
        vec![Act::Gesture(Gesture::Swipe {
            points,
            duration_ms: (held as u32).clamp(MIN_SWIPE_MS, MAX_GESTURE_MS),
        })]
    }
}

/// A normalized point in pixels of a display, kept inside it.
fn px(p: (f32, f32), size: (u32, u32)) -> (u32, u32) {
    let at = |v: f32, side: u32| (v.clamp(0.0, 1.0) * side.saturating_sub(1) as f32).round() as u32;
    (at(p.0, size.0), at(p.1, size.1))
}

fn dist(a: (u32, u32), b: (u32, u32)) -> u32 {
    a.0.abs_diff(b.0).max(a.1.abs_diff(b.1))
}

/// At most `max` points of `path`, always keeping the first and the last.
fn thin(path: &[(f32, f32)], max: usize) -> Vec<(f32, f32)> {
    if path.len() <= max {
        return path.to_vec();
    }
    let last = path.len() - 1;
    (0..max).map(|i| path[i * last / (max - 1)]).collect()
}

/// The [`InputSink`](super::InputSink) of an Android host (#19).
pub struct TouchSink {
    host: Arc<dyn TouchHost>,
    state: Mutex<State>,
    /// [`meta`] bits of the modifier keys held now.
    meta: Mutex<u32>,
    clock: Instant,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl TouchSink {
    /// A sink that runs gestures and keys through `host`.
    pub fn new(host: Arc<dyn TouchHost>) -> Self {
        Self {
            host,
            state: Mutex::new(State::new()),
            meta: Mutex::new(0),
            clock: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    fn need_gestures(&self) -> Result<()> {
        if self.host.gestures_ready() {
            Ok(())
        } else {
            Err(CoreError::PermissionDenied(
                "the accessibility service is off".into(),
            ))
        }
    }

    fn need_text(&self) -> Result<()> {
        if self.host.text_ready() {
            Ok(())
        } else {
            Err(CoreError::PermissionDenied(
                "the UrDevice keyboard is not in use".into(),
            ))
        }
    }

    fn run(&self, grant: &Grant, acts: Vec<Act>) -> Result<()> {
        let mut result = Ok(());
        for act in acts {
            let r = match act {
                Act::Gesture(g) => self.host.gesture(grant, &g),
                Act::Nav(a) => self.host.nav(grant, a),
            };
            if r.is_err() {
                result = r;
            }
        }
        result
    }
}

impl super::InputSink for TouchSink {
    fn name(&self) -> &'static str {
        "android"
    }

    fn permission_status(&self) -> PermStatus {
        if self.host.gestures_ready() {
            PermStatus::Ready
        } else {
            PermStatus::Missing(Missing::AccessibilityService)
        }
    }

    fn text_status(&self) -> PermStatus {
        if self.host.text_ready() {
            PermStatus::Ready
        } else {
            PermStatus::Missing(Missing::InputMethod)
        }
    }

    fn display_size(&self) -> Option<(u32, u32)> {
        let s = self.host.display_size();
        (s.0 > 0 && s.1 > 0).then_some(s)
    }

    fn pointer(&self, grant: &Grant, event: &PointerEvent) -> Result<()> {
        self.need_gestures()?;
        let size = self.host.display_size();
        let now = self.now_ms();
        let (acts, at) = {
            let mut st = lock(&self.state);
            let acts = st.apply(event, now, size);
            (acts, st.pos)
        };
        let result = self.run(grant, acts);
        // Only a pointer that moves by distance (the remote-control window) is drawn. A screen
        // window points straight at the picture, which already shows the screen, so it draws none.
        if matches!(event, PointerEvent::Move { .. }) {
            let (w, h) = if size.0 > 0 && size.1 > 0 {
                size
            } else {
                FALLBACK_SIZE
            };
            let (x, y) = px(at, (w, h));
            let _ = self.host.cursor(grant, x, y, true);
        }
        result
    }

    fn key(&self, grant: &Grant, event: &KeyEvent) -> Result<()> {
        if let Some(bit) = modifier_bit(event.usage) {
            // Modifiers ride on the key that follows; they are not sent alone.
            let mut m = lock(&self.meta);
            if event.down {
                *m |= bit;
            } else {
                *m &= !bit;
            }
            return Ok(());
        }
        if event.usage == usage::ESCAPE {
            // Escape is Back (the key has no use in most Android fields).
            if event.down {
                self.need_gestures()?;
                return self.host.nav(grant, super::nav::BACK);
            }
            return Ok(());
        }
        let Some(code) = android_keycode(event.usage) else {
            return Ok(());
        };
        self.need_text()?;
        let meta = *lock(&self.meta);
        self.host.key(grant, code, event.down, meta)
    }

    fn text(&self, grant: &Grant, text: &str) -> Result<()> {
        self.need_text()?;
        // A newline is the Enter key: the keyboard commits plain text only.
        let mut result = Ok(());
        let mut parts = text.split('\n').peekable();
        while let Some(part) = parts.next() {
            if !part.is_empty()
                && let Err(e) = self.host.text(grant, part)
            {
                result = Err(e);
            }
            if parts.peek().is_some() {
                for down in [true, false] {
                    if let Err(e) = self.host.key(grant, 66, down, 0) {
                        result = Err(e);
                    }
                }
            }
        }
        result
    }

    fn nav(&self, grant: &Grant, action: u32) -> Result<()> {
        self.need_gestures()?;
        self.host.nav(grant, action)
    }

    fn cancel(&self, grant: &Grant) {
        lock(&self.state).stroke = None;
        // The session is over: the pointer leaves the screen.
        let _ = self.host.cursor(grant, 0, 0, false);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::super::{InputSink, nav};
    use super::*;
    use crate::identity::DeviceId;
    use crate::permission::Capability;

    const SIZE: (u32, u32) = (1000, 2000);

    fn abs(x: f32, y: f32) -> PointerEvent {
        PointerEvent::Abs {
            x: (x * COORD_MAX as f32) as u32,
            y: (y * COORD_MAX as f32) as u32,
            display: 0,
        }
    }

    fn left(down: bool) -> PointerEvent {
        PointerEvent::Button {
            button: button::LEFT,
            down,
        }
    }

    #[test]
    fn a_quick_press_in_place_is_a_tap() {
        let mut s = State::new();
        assert!(s.apply(&abs(0.5, 0.25), 0, SIZE).is_empty());
        assert!(s.apply(&left(true), 10, SIZE).is_empty());
        let acts = s.apply(&left(false), 90, SIZE);
        assert_eq!(
            acts,
            vec![Act::Gesture(Gesture::Tap {
                x: 499,
                y: 500,
                duration_ms: TAP_MS
            })]
        );
    }

    #[test]
    fn a_tiny_wobble_is_still_a_tap() {
        let mut s = State::new();
        s.apply(&abs(0.5, 0.5), 0, SIZE);
        s.apply(&left(true), 0, SIZE);
        s.apply(&PointerEvent::Move { dx: 5, dy: -4 }, 20, SIZE);
        let acts = s.apply(&left(false), 60, SIZE);
        assert!(matches!(
            acts.as_slice(),
            [Act::Gesture(Gesture::Tap { .. })]
        ));
    }

    #[test]
    fn a_long_hold_is_a_long_press() {
        let mut s = State::new();
        s.apply(&abs(0.1, 0.1), 0, SIZE);
        s.apply(&left(true), 1_000, SIZE);
        let acts = s.apply(&left(false), 1_000 + LONG_PRESS_MS + 5, SIZE);
        let [Act::Gesture(Gesture::Tap { duration_ms, .. })] = acts.as_slice() else {
            panic!("{acts:?}")
        };
        assert!(*duration_ms >= LONG_PRESS_MIN_MS);
        // A very long hold is capped.
        s.apply(&left(true), 0, SIZE);
        let acts = s.apply(&left(false), 60_000, SIZE);
        let [Act::Gesture(Gesture::Tap { duration_ms, .. })] = acts.as_slice() else {
            panic!("{acts:?}")
        };
        assert_eq!(*duration_ms, MAX_GESTURE_MS);
    }

    #[test]
    fn dragging_makes_a_swipe_from_where_it_started_to_where_it_ended() {
        let mut s = State::new();
        s.apply(&abs(0.5, 0.8), 0, SIZE);
        s.apply(&left(true), 0, SIZE);
        for i in 1..=10 {
            s.apply(&abs(0.5, 0.8 - 0.05 * i as f32), i * 20, SIZE);
        }
        let acts = s.apply(&left(false), 220, SIZE);
        let [
            Act::Gesture(Gesture::Swipe {
                points,
                duration_ms,
            }),
        ] = acts.as_slice()
        else {
            panic!("{acts:?}")
        };
        assert_eq!(*duration_ms, 220);
        assert_eq!(points.first(), Some(&(499, 1599)));
        assert_eq!(points.last(), Some(&(499, 600)));
        assert!(points.len() >= 2 && points.len() <= MAX_SWIPE_POINTS + 1);
    }

    #[test]
    fn a_long_path_is_thinned_and_a_flood_is_bounded() {
        let mut s = State::new();
        s.apply(&abs(0.0, 0.0), 0, SIZE);
        s.apply(&left(true), 0, SIZE);
        for i in 0..5_000u32 {
            s.apply(&abs(0.0, i as f32 / 5_000.0), 1, SIZE);
        }
        assert!(s.stroke.as_ref().unwrap().path.len() <= MAX_STROKE_POINTS);
        let acts = s.apply(&left(false), 100, SIZE);
        let [Act::Gesture(Gesture::Swipe { points, .. })] = acts.as_slice() else {
            panic!("{acts:?}")
        };
        assert!(points.len() <= MAX_SWIPE_POINTS + 1);
        // The finger lifts where the pointer is, even after the path filled.
        assert_eq!(points.last().unwrap().1, 1_999);
    }

    #[test]
    fn a_second_press_finishes_a_lost_release() {
        let mut s = State::new();
        s.apply(&left(true), 0, SIZE);
        let acts = s.apply(&left(true), 50, SIZE);
        assert_eq!(acts.len(), 1);
        assert!(s.stroke.is_some());
    }

    #[test]
    fn release_without_press_does_nothing() {
        assert!(State::new().apply(&left(false), 5, SIZE).is_empty());
    }

    #[test]
    fn relative_moves_shift_the_pointer_and_stay_on_the_display() {
        let mut s = State::new();
        s.apply(&PointerEvent::Move { dx: 100, dy: -200 }, 0, SIZE);
        assert!((s.pos.0 - 0.6).abs() < 1e-4 && (s.pos.1 - 0.4).abs() < 1e-4);
        s.apply(
            &PointerEvent::Move {
                dx: 100_000,
                dy: -100_000,
            },
            0,
            SIZE,
        );
        assert_eq!(s.pos, (1.0, 0.0));
    }

    #[test]
    fn the_wheel_swipes_the_opposite_way() {
        let mut s = State::new();
        s.apply(&abs(0.5, 0.5), 0, SIZE);
        let acts = s.apply(&PointerEvent::Scroll { dx: 0, dy: 120 }, 0, SIZE);
        let [
            Act::Gesture(Gesture::Swipe {
                points,
                duration_ms,
            }),
        ] = acts.as_slice()
        else {
            panic!("{acts:?}")
        };
        assert_eq!(*duration_ms, SCROLL_MS);
        assert!(points[1].1 < points[0].1, "wheel down moves the finger up");
        assert_eq!(points[1].0, points[0].0);
        // A huge wheel turn is bounded and stays on the display.
        let acts = s.apply(
            &PointerEvent::Scroll {
                dx: -20_000,
                dy: 20_000,
            },
            0,
            SIZE,
        );
        let [Act::Gesture(Gesture::Swipe { points, .. })] = acts.as_slice() else {
            panic!("{acts:?}")
        };
        assert!(points.iter().all(|p| p.0 < SIZE.0 && p.1 < SIZE.1));
    }

    #[test]
    fn right_and_back_buttons_are_back_and_others_do_nothing() {
        let mut s = State::new();
        let b = |button, down| PointerEvent::Button { button, down };
        assert_eq!(
            s.apply(&b(button::RIGHT, true), 0, SIZE),
            vec![Act::Nav(nav::BACK)]
        );
        assert!(s.apply(&b(button::RIGHT, false), 0, SIZE).is_empty());
        assert_eq!(
            s.apply(&b(button::BACK, true), 0, SIZE),
            vec![Act::Nav(nav::BACK)]
        );
        assert!(s.apply(&b(button::MIDDLE, true), 0, SIZE).is_empty());
        assert!(s.apply(&b(button::FORWARD, true), 0, SIZE).is_empty());
    }

    #[test]
    fn an_unknown_display_size_does_not_divide_by_zero() {
        let mut s = State::new();
        s.apply(&PointerEvent::Move { dx: 10, dy: 10 }, 0, (0, 0));
        s.apply(&left(true), 0, (0, 0));
        assert!(!s.apply(&left(false), 10, (0, 0)).is_empty());
    }

    #[test]
    fn keycodes_match_android() {
        assert_eq!(android_keycode(usage::A), Some(29)); // KEYCODE_A
        assert_eq!(android_keycode(0x1D), Some(54)); // Z
        assert_eq!(android_keycode(0x1E), Some(8)); // 1
        assert_eq!(android_keycode(0x27), Some(7)); // 0
        assert_eq!(android_keycode(usage::ENTER), Some(66));
        assert_eq!(android_keycode(usage::BACKSPACE), Some(67));
        assert_eq!(android_keycode(usage::F1), Some(131));
        assert_eq!(android_keycode(usage::F12), Some(142));
        assert_eq!(android_keycode(usage::UP), Some(19));
        assert_eq!(android_keycode(usage::LEFT_CTRL), None);
        assert_eq!(android_keycode(0x65), None);
    }

    // --- the sink with a fake host -----------------------------------------

    #[derive(Debug, PartialEq)]
    enum Did {
        Gesture(Gesture),
        Nav(u32),
        Text(String),
        Key(u32, bool, u32),
    }

    struct Host {
        gestures: Mutex<bool>,
        text: Mutex<bool>,
        log: Mutex<Vec<Did>>,
        cursors: Mutex<Vec<(u32, u32, bool)>>,
    }

    impl Host {
        fn new(gestures: bool, text: bool) -> Arc<Self> {
            Arc::new(Self {
                gestures: Mutex::new(gestures),
                text: Mutex::new(text),
                log: Mutex::default(),
                cursors: Mutex::default(),
            })
        }
        fn take(&self) -> Vec<Did> {
            std::mem::take(&mut *self.log.lock().unwrap())
        }
    }

    impl TouchHost for Host {
        fn gestures_ready(&self) -> bool {
            *self.gestures.lock().unwrap()
        }
        fn text_ready(&self) -> bool {
            *self.text.lock().unwrap()
        }
        fn display_size(&self) -> (u32, u32) {
            SIZE
        }
        fn gesture(&self, _: &Grant, g: &Gesture) -> Result<()> {
            self.log.lock().unwrap().push(Did::Gesture(g.clone()));
            Ok(())
        }
        fn nav(&self, _: &Grant, a: u32) -> Result<()> {
            self.log.lock().unwrap().push(Did::Nav(a));
            Ok(())
        }
        fn text(&self, _: &Grant, t: &str) -> Result<()> {
            self.log.lock().unwrap().push(Did::Text(t.into()));
            Ok(())
        }
        fn key(&self, _: &Grant, c: u32, d: bool, m: u32) -> Result<()> {
            self.log.lock().unwrap().push(Did::Key(c, d, m));
            Ok(())
        }
        fn cursor(&self, _: &Grant, x: u32, y: u32, shown: bool) -> Result<()> {
            self.cursors.lock().unwrap().push((x, y, shown));
            Ok(())
        }
    }

    fn grant() -> Grant {
        Grant {
            peer: DeviceId([7; 32]),
            capability: Capability::Input,
            activity: 1,
        }
    }

    fn k(usage: u32, down: bool) -> KeyEvent {
        KeyEvent { usage, down }
    }

    #[test]
    fn the_status_names_what_is_missing() {
        let h = Host::new(false, false);
        let sink = TouchSink::new(h.clone());
        assert_eq!(
            sink.permission_status(),
            PermStatus::Missing(Missing::AccessibilityService)
        );
        assert_eq!(
            sink.text_status(),
            PermStatus::Missing(Missing::InputMethod)
        );
        assert_eq!(sink.display_size(), Some(SIZE));
        *h.gestures.lock().unwrap() = true;
        assert_eq!(sink.permission_status(), PermStatus::Ready);
    }

    #[test]
    fn nothing_runs_while_the_service_is_off() {
        let h = Host::new(false, false);
        let sink = TouchSink::new(h.clone());
        let g = grant();
        assert!(sink.pointer(&g, &left(true)).is_err());
        assert!(sink.nav(&g, nav::HOME).is_err());
        assert!(sink.text(&g, "hi").is_err());
        assert!(sink.key(&g, &k(usage::A, true)).is_err());
        assert!(sink.key(&g, &k(usage::ESCAPE, true)).is_err());
        assert!(h.take().is_empty());
    }

    #[test]
    fn a_tap_and_a_global_action_reach_the_host() {
        let h = Host::new(true, false);
        let sink = TouchSink::new(h.clone());
        let g = grant();
        sink.pointer(&g, &abs(0.5, 0.5)).unwrap();
        sink.pointer(&g, &left(true)).unwrap();
        sink.pointer(&g, &left(false)).unwrap();
        sink.nav(&g, nav::RECENTS).unwrap();
        let log = h.take();
        assert!(matches!(
            log.as_slice(),
            [Did::Gesture(Gesture::Tap { .. }), Did::Nav(2)]
        ));
    }

    #[test]
    fn stop_drops_a_stroke_that_was_not_finished() {
        let h = Host::new(true, false);
        let sink = TouchSink::new(h.clone());
        let g = grant();
        sink.pointer(&g, &left(true)).unwrap();
        sink.pointer(&g, &PointerEvent::Move { dx: 300, dy: 0 })
            .unwrap();
        sink.cancel(&g);
        // The core then releases the held button; nothing runs.
        sink.pointer(&g, &left(false)).unwrap();
        assert!(h.take().is_empty());
    }

    #[test]
    fn text_goes_to_the_keyboard_with_newlines_as_enter() {
        let h = Host::new(false, true);
        let sink = TouchSink::new(h.clone());
        sink.text(&grant(), "héllo\n世界\n").unwrap();
        assert_eq!(
            h.take(),
            vec![
                Did::Text("héllo".into()),
                Did::Key(66, true, 0),
                Did::Key(66, false, 0),
                Did::Text("世界".into()),
                Did::Key(66, true, 0),
                Did::Key(66, false, 0),
            ]
        );
    }

    #[test]
    fn shortcuts_carry_the_modifiers_that_are_held() {
        let h = Host::new(true, true);
        let sink = TouchSink::new(h.clone());
        let g = grant();
        sink.key(&g, &k(usage::LEFT_CTRL, true)).unwrap();
        sink.key(&g, &k(usage::A, true)).unwrap();
        sink.key(&g, &k(usage::A, false)).unwrap();
        sink.key(&g, &k(usage::LEFT_CTRL, false)).unwrap();
        sink.key(&g, &k(usage::A, true)).unwrap();
        // Unknown keys are skipped; Escape is Back.
        sink.key(&g, &k(0x65, true)).unwrap();
        sink.key(&g, &k(usage::ESCAPE, true)).unwrap();
        sink.key(&g, &k(usage::ESCAPE, false)).unwrap();
        assert_eq!(
            h.take(),
            vec![
                Did::Key(29, true, meta::CTRL),
                Did::Key(29, false, meta::CTRL),
                Did::Key(29, true, 0),
                Did::Nav(nav::BACK),
            ]
        );
    }

    #[test]
    fn the_pointer_is_drawn_while_it_moves_by_distance_and_hidden_on_cancel() {
        let host = Host::new(true, true);
        let sink = TouchSink::new(host.clone());
        let g = grant();
        sink.pointer(&g, &abs(0.5, 0.25)).unwrap();
        sink.pointer(&g, &PointerEvent::Move { dx: 100, dy: 0 })
            .unwrap();
        // A button does not move the pointer.
        sink.pointer(&g, &left(true)).unwrap();
        sink.cancel(&g);
        let c = host.cursors.lock().unwrap().clone();
        // The absolute move (a screen window) draws nothing; the relative one does.
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(c[0].0.abs_diff(600) <= 1 && c[0].2, "{c:?}");
        assert_eq!(c[1], (0, 0, false));
    }
}
