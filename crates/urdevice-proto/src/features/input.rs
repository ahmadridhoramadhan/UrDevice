//! Remote control: pointer, keyboard, text, and presenter (#18, #19, design §9.3):
//! wire messages for `ServiceId::INPUT`.
//!
//! Field numbers and stream kinds are local to this service, so they never
//! clash with another feature. `proto/features/input.proto` lists the same
//! schema; the numbers are the wire contract and are never reused.
//!
//! Input events travel in a [`Batch`]:
//!
//! - **Datagrams** (`ServiceId::INPUT`, `Direction::ToHost`, the activity id)
//!   carry a [`Batch`] directly as the payload. They are for events whose loss
//!   is harmless: [`Move`], [`Abs`], [`Scroll`], and [`Laser`].
//! - **The control stream** carries a [`Batch`] wrapped in [`Input`]. It is for
//!   events whose loss would leave something stuck or drop what the user typed:
//!   [`Button`], [`Key`], and [`Text`]. It is also the fallback when the path
//!   takes no datagrams.
//!
//! The host answers [`Hello`] with [`Status`] (`Direction::ToController`).
//!
//! Everything here is untrusted input. [`Event::sanitize`] and
//! [`Batch::sanitize`] clamp or drop what a peer sends, so a hostile
//! controller cannot make the host do unbounded work.

use prost::Message;

/// Most events the host takes from one [`Batch`]. The rest are dropped.
pub const MAX_EVENTS: usize = 64;

/// Longest [`Text`] in bytes of UTF-8. A sender splits longer text.
pub const MAX_TEXT_BYTES: usize = 4096;

/// Largest relative movement or scroll in one event, in either axis.
pub const MAX_DELTA: i32 = 20_000;

/// Largest normalized coordinate of [`Abs`] and [`Laser`]: 0 is the left or
/// top edge, this is the right or bottom edge.
pub const COORD_MAX: u32 = 65_535;

/// Scroll units in one wheel notch (the Windows convention). A [`Scroll`]
/// is in these units: positive `dy` scrolls down, positive `dx` right.
pub const SCROLL_NOTCH: i32 = 120;

/// Modifier bits for [`Key::modifiers`].
pub mod modifier {
    /// Either Control key.
    pub const CTRL: u32 = 1;
    /// Either Shift key.
    pub const SHIFT: u32 = 2;
    /// Either Alt (Option) key.
    pub const ALT: u32 = 4;
    /// The Windows, Command, or Super key.
    pub const META: u32 = 8;
    /// All bits that mean something.
    pub const ALL: u32 = CTRL | SHIFT | ALT | META;
}

/// Mouse buttons for [`Button::button`].
pub mod button {
    /// The primary button.
    pub const LEFT: u32 = 0;
    /// The secondary button.
    pub const RIGHT: u32 = 1;
    /// The wheel button.
    pub const MIDDLE: u32 = 2;
    /// The "back" side button.
    pub const BACK: u32 = 3;
    /// The "forward" side button.
    pub const FORWARD: u32 = 4;
    /// Number of buttons.
    pub const COUNT: u32 = 5;
}

/// USB HID usages (page 0x07, Keyboard) for [`Key::usage`]. Only the ones the
/// apps send by name are listed; any usage in [`is_valid_usage`] is accepted.
pub mod usage {
    pub const A: u32 = 0x04;
    pub const B: u32 = 0x05;
    pub const ENTER: u32 = 0x28;
    pub const ESCAPE: u32 = 0x29;
    pub const BACKSPACE: u32 = 0x2A;
    pub const TAB: u32 = 0x2B;
    pub const SPACE: u32 = 0x2C;
    pub const F1: u32 = 0x3A;
    pub const F5: u32 = 0x3E;
    pub const F12: u32 = 0x45;
    pub const PAGE_UP: u32 = 0x4B;
    pub const DELETE: u32 = 0x4C;
    pub const HOME: u32 = 0x4A;
    pub const END: u32 = 0x4D;
    pub const PAGE_DOWN: u32 = 0x4E;
    pub const RIGHT: u32 = 0x4F;
    pub const LEFT: u32 = 0x50;
    pub const DOWN: u32 = 0x51;
    pub const UP: u32 = 0x52;
    pub const LEFT_CTRL: u32 = 0xE0;
    pub const LEFT_SHIFT: u32 = 0xE1;
    pub const LEFT_ALT: u32 = 0xE2;
    pub const LEFT_META: u32 = 0xE3;
    pub const RIGHT_CTRL: u32 = 0xE4;
    pub const RIGHT_SHIFT: u32 = 0xE5;
    pub const RIGHT_ALT: u32 = 0xE6;
    pub const RIGHT_META: u32 = 0xE7;
}

/// True for a keyboard usage the hosts can inject: letters, digits, the
/// editing and navigation keys, F1 to F24, the keypad, and the modifiers.
pub fn is_valid_usage(u: u32) -> bool {
    matches!(u, 0x04..=0x65 | 0x68..=0x73 | 0xE0..=0xE7)
}

/// What the host's input adapter can do now, in [`Status::state`].
pub mod state {
    /// Input works.
    pub const READY: u32 = 0;
    /// The OS needs a permission the user must grant; see [`super::Status::permission`].
    pub const MISSING_PERMISSION: u32 = 1;
    /// Input is not available on this host.
    pub const UNAVAILABLE: u32 = 2;
}

/// A remote-control message on the control stream.
#[derive(Clone, PartialEq, Message)]
pub struct Input {
    /// The message.
    #[prost(oneof = "input::Msg", tags = "1, 2, 3")]
    pub msg: Option<input::Msg>,
}

/// Nested types for [`Input`]: `input::Msg` is the oneof.
#[allow(clippy::module_inception)]
pub mod input {
    /// One remote-control message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// Controller to host: events that must not be lost.
        #[prost(message, tag = "1")]
        Batch(super::Batch),
        /// Controller to host: a session started; prepare the adapter and report.
        #[prost(message, tag = "2")]
        Hello(super::Hello),
        /// Host to controller: what the adapter can do.
        #[prost(message, tag = "3")]
        Status(super::Status),
    }
}

impl From<input::Msg> for Input {
    fn from(msg: input::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

/// A group of events, sent every few milliseconds.
#[derive(Clone, PartialEq, Message)]
pub struct Batch {
    /// The events, in order.
    #[prost(message, repeated, tag = "1")]
    pub events: Vec<Event>,
}

/// One input event.
#[derive(Clone, PartialEq, Message)]
pub struct Event {
    /// What happened.
    #[prost(oneof = "event::Kind", tags = "1, 2, 3, 4, 5, 6, 7, 8")]
    pub kind: Option<event::Kind>,
}

/// Nested types for [`Event`]: `event::Kind` is the oneof.
pub mod event {
    /// The kinds of event.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Kind {
        /// Move the pointer by a distance.
        #[prost(message, tag = "1")]
        Move(super::Move),
        /// Put the pointer at a place on a display.
        #[prost(message, tag = "2")]
        Abs(super::Abs),
        /// Press or release a mouse button.
        #[prost(message, tag = "3")]
        Button(super::Button),
        /// Turn the wheel.
        #[prost(message, tag = "4")]
        Scroll(super::Scroll),
        /// Press or release a key.
        #[prost(message, tag = "5")]
        Key(super::Key),
        /// Type Unicode text.
        #[prost(message, tag = "6")]
        Text(super::Text),
        /// Show, move, or hide the presenter's laser dot.
        #[prost(message, tag = "7")]
        Laser(super::Laser),
        /// A system navigation action: Back, Home, or Recents (Android hosts).
        #[prost(message, tag = "8")]
        Nav(super::Nav),
    }
}

impl From<event::Kind> for Event {
    fn from(kind: event::Kind) -> Self {
        Self { kind: Some(kind) }
    }
}

/// Relative pointer movement, in pixels of the host's display.
#[derive(Clone, PartialEq, Message)]
pub struct Move {
    /// Right is positive.
    #[prost(sint32, tag = "1")]
    pub dx: i32,
    /// Down is positive.
    #[prost(sint32, tag = "2")]
    pub dy: i32,
}

/// An absolute pointer position.
#[derive(Clone, PartialEq, Message)]
pub struct Abs {
    /// 0 to [`COORD_MAX`] across the display.
    #[prost(uint32, tag = "1")]
    pub x: u32,
    /// 0 to [`COORD_MAX`] down the display.
    #[prost(uint32, tag = "2")]
    pub y: u32,
    /// Display index; 0 is the primary display.
    #[prost(uint32, tag = "3")]
    pub display: u32,
}

/// A mouse button changing state.
#[derive(Clone, PartialEq, Message)]
pub struct Button {
    /// One of [`button`].
    #[prost(uint32, tag = "1")]
    pub button: u32,
    /// True when pressed.
    #[prost(bool, tag = "2")]
    pub down: bool,
}

/// Wheel movement in [`SCROLL_NOTCH`] units.
#[derive(Clone, PartialEq, Message)]
pub struct Scroll {
    /// Right is positive.
    #[prost(sint32, tag = "1")]
    pub dx: i32,
    /// Down is positive.
    #[prost(sint32, tag = "2")]
    pub dy: i32,
}

/// A key changing state.
#[derive(Clone, PartialEq, Message)]
pub struct Key {
    /// USB HID usage, page 0x07 (see [`usage`]).
    #[prost(uint32, tag = "1")]
    pub usage: u32,
    /// True when pressed.
    #[prost(bool, tag = "2")]
    pub down: bool,
    /// [`modifier`] bits held for this key: the host presses them before the
    /// key goes down and releases them after it goes up.
    #[prost(uint32, tag = "3")]
    pub modifiers: u32,
}

/// Unicode text, typed as text and not as layout-dependent key codes (R10.4).
#[derive(Clone, PartialEq, Message)]
pub struct Text {
    /// At most [`MAX_TEXT_BYTES`] bytes.
    #[prost(string, tag = "1")]
    pub text: String,
}

/// The presenter's laser dot, in normalized coordinates of the display.
#[derive(Clone, PartialEq, Message)]
pub struct Laser {
    /// 0 to [`COORD_MAX`] across the display.
    #[prost(uint32, tag = "1")]
    pub x: u32,
    /// 0 to [`COORD_MAX`] down the display.
    #[prost(uint32, tag = "2")]
    pub y: u32,
    /// False hides the dot (the user let go).
    #[prost(bool, tag = "3")]
    pub on: bool,
}

/// System navigation actions for [`Nav::action`]. Only Android hosts act on
/// them; a desktop host ignores them.
pub mod nav {
    /// The Back action.
    pub const BACK: u32 = 0;
    /// The Home action.
    pub const HOME: u32 = 1;
    /// The Recents (app switcher) action.
    pub const RECENTS: u32 = 2;
    /// Number of actions.
    pub const COUNT: u32 = 3;
}

/// A system navigation action (R9.3).
#[derive(Clone, PartialEq, Message)]
pub struct Nav {
    /// One of [`nav`].
    #[prost(uint32, tag = "1")]
    pub action: u32,
}

/// Controller to host: a remote-control session begins.
#[derive(Clone, PartialEq, Message)]
pub struct Hello {}

/// Host to controller: whether the host can inject input.
#[derive(Clone, PartialEq, Message)]
pub struct Status {
    /// One of [`state`].
    #[prost(uint32, tag = "1")]
    pub state: u32,
    /// For [`state::MISSING_PERMISSION`]: `accessibility`, `uinput`, or `portal`.
    #[prost(string, tag = "2")]
    pub permission: String,
    /// Name of the host's back-end, for example `x11` or `windows`. Display only.
    #[prost(string, tag = "3")]
    pub backend: String,
    /// Width of the host's primary display in pixels; 0 when unknown. The
    /// controller shapes its remote-control window like it.
    #[prost(uint32, tag = "4")]
    pub width: u32,
    /// Height of the host's primary display in pixels; 0 when unknown.
    #[prost(uint32, tag = "5")]
    pub height: u32,
    /// Empty when the host can type text. Otherwise the permission it lacks
    /// for typing (`input_method`: the UrDevice keyboard is not enabled).
    /// Pointer input may work while this is set.
    #[prost(string, tag = "6")]
    pub text_permission: String,
}

/// Largest display side a [`Status`] may report, in pixels.
pub const MAX_DISPLAY_SIDE: u32 = 65_535;

/// Longest string a [`Status`] may carry.
pub const MAX_STATUS_TEXT: usize = 64;

impl Status {
    /// Clamp what a host sent, before the controller shows it.
    pub fn sanitize(mut self) -> Self {
        self.state = self.state.min(state::UNAVAILABLE);
        self.permission = clip(self.permission);
        self.backend = clip(self.backend);
        self.text_permission = clip(self.text_permission);
        self.width = self.width.min(MAX_DISPLAY_SIDE);
        self.height = self.height.min(MAX_DISPLAY_SIDE);
        self
    }
}

fn clip(mut s: String) -> String {
    if s.len() > MAX_STATUS_TEXT {
        let mut end = MAX_STATUS_TEXT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s.retain(|c| !c.is_control());
    s
}

impl Event {
    /// A copy with every number clamped into range, or `None` when the event
    /// is empty or cannot be valid (an unknown key or button, too much text).
    pub fn sanitize(&self) -> Option<Event> {
        use event::Kind;
        let kind = match self.kind.as_ref()? {
            Kind::Move(m) => Kind::Move(Move {
                dx: m.dx.clamp(-MAX_DELTA, MAX_DELTA),
                dy: m.dy.clamp(-MAX_DELTA, MAX_DELTA),
            }),
            Kind::Abs(a) => Kind::Abs(Abs {
                x: a.x.min(COORD_MAX),
                y: a.y.min(COORD_MAX),
                display: a.display.min(63),
            }),
            Kind::Button(b) if b.button < button::COUNT => Kind::Button(b.clone()),
            Kind::Button(_) => return None,
            Kind::Scroll(s) => Kind::Scroll(Scroll {
                dx: s.dx.clamp(-MAX_DELTA, MAX_DELTA),
                dy: s.dy.clamp(-MAX_DELTA, MAX_DELTA),
            }),
            Kind::Key(k) if is_valid_usage(k.usage) => Kind::Key(Key {
                usage: k.usage,
                down: k.down,
                modifiers: k.modifiers & modifier::ALL,
            }),
            Kind::Key(_) => return None,
            Kind::Text(t) if !t.text.is_empty() && t.text.len() <= MAX_TEXT_BYTES => {
                // Control characters other than tab and newline are not text.
                if t.text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    return None;
                }
                Kind::Text(t.clone())
            }
            Kind::Text(_) => return None,
            Kind::Laser(l) => Kind::Laser(Laser {
                x: l.x.min(COORD_MAX),
                y: l.y.min(COORD_MAX),
                on: l.on,
            }),
            Kind::Nav(n) if n.action < nav::COUNT => Kind::Nav(n.clone()),
            Kind::Nav(_) => return None,
        };
        Some(Event { kind: Some(kind) })
    }

    /// True for the events a datagram may carry: losing one is harmless.
    pub fn is_lossy(&self) -> bool {
        use event::Kind;
        matches!(
            self.kind,
            Some(Kind::Move(_) | Kind::Abs(_) | Kind::Scroll(_) | Kind::Laser(_))
        )
    }
}

impl Batch {
    /// The valid events of a batch, at most [`MAX_EVENTS`], clamped.
    pub fn sanitize(&self) -> Vec<Event> {
        self.events
            .iter()
            .take(MAX_EVENTS)
            .filter_map(Event::sanitize)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: event::Kind) -> Event {
        Event::from(kind)
    }

    #[test]
    fn batch_round_trips_and_a_full_move_batch_fits_a_datagram() {
        let b = Batch {
            events: (0..MAX_EVENTS)
                .map(|i| {
                    ev(event::Kind::Move(Move {
                        dx: -(i as i32),
                        dy: 300,
                    }))
                })
                .collect(),
        };
        let bytes = b.encode_to_vec();
        assert_eq!(Batch::decode(bytes.as_slice()).unwrap(), b);
        // The smallest QUIC datagram on a 1280-byte path leaves about 1.1 KB.
        assert!(bytes.len() < 1100, "{}", bytes.len());
    }

    #[test]
    fn control_message_round_trips() {
        let m = Input::from(input::Msg::Status(Status {
            state: state::MISSING_PERMISSION,
            permission: "accessibility".into(),
            backend: "macos".into(),
            width: 1080,
            height: 2400,
            text_permission: "input_method".into(),
        }));
        assert_eq!(Input::decode(m.encode_to_vec().as_slice()).unwrap(), m);
    }

    #[test]
    fn sanitize_clamps_and_drops() {
        let moved = ev(event::Kind::Move(Move {
            dx: i32::MAX,
            dy: i32::MIN,
        }))
        .sanitize()
        .unwrap();
        assert_eq!(
            moved.kind,
            Some(event::Kind::Move(Move {
                dx: MAX_DELTA,
                dy: -MAX_DELTA
            }))
        );
        let abs = ev(event::Kind::Abs(Abs {
            x: u32::MAX,
            y: 5,
            display: 9999,
        }))
        .sanitize()
        .unwrap();
        assert_eq!(
            abs.kind,
            Some(event::Kind::Abs(Abs {
                x: COORD_MAX,
                y: 5,
                display: 63
            }))
        );
        assert!(
            ev(event::Kind::Button(Button {
                button: 99,
                down: true
            }))
            .sanitize()
            .is_none()
        );
        for bad in [0, 3, 0x66, 0x74, 0xDF, 0xE8, u32::MAX] {
            assert!(
                ev(event::Kind::Key(Key {
                    usage: bad,
                    down: true,
                    modifiers: 0
                }))
                .sanitize()
                .is_none(),
                "{bad:#x}"
            );
        }
        let key = ev(event::Kind::Key(Key {
            usage: usage::A,
            down: true,
            modifiers: u32::MAX,
        }))
        .sanitize()
        .unwrap();
        assert_eq!(
            key.kind,
            Some(event::Kind::Key(Key {
                usage: usage::A,
                down: true,
                modifiers: modifier::ALL
            }))
        );
        assert!(Event::default().sanitize().is_none());
    }

    #[test]
    fn text_is_limited_and_has_no_control_characters() {
        let t = |s: &str| ev(event::Kind::Text(Text { text: s.into() }));
        assert!(t("héllo 世界 😀\n\tok").sanitize().is_some());
        assert!(t("").sanitize().is_none());
        assert!(t("a\u{1b}[31m").sanitize().is_none());
        assert!(t("a\0b").sanitize().is_none());
        assert!(t(&"x".repeat(MAX_TEXT_BYTES)).sanitize().is_some());
        assert!(t(&"x".repeat(MAX_TEXT_BYTES + 1)).sanitize().is_none());
    }

    #[test]
    fn batch_sanitize_caps_the_event_count() {
        let b = Batch {
            events: (0..MAX_EVENTS * 3)
                .map(|_| ev(event::Kind::Move(Move { dx: 1, dy: 1 })))
                .collect(),
        };
        assert_eq!(b.sanitize().len(), MAX_EVENTS);
    }

    #[test]
    fn lossy_events_are_the_pointer_ones() {
        assert!(ev(event::Kind::Move(Move::default())).is_lossy());
        assert!(ev(event::Kind::Laser(Laser::default())).is_lossy());
        assert!(!ev(event::Kind::Button(Button::default())).is_lossy());
        assert!(!ev(event::Kind::Key(Key::default())).is_lossy());
        assert!(!ev(event::Kind::Text(Text::default())).is_lossy());
    }

    #[test]
    fn status_text_is_clamped() {
        let s = Status {
            state: 99,
            permission: "é".repeat(100),
            backend: "a\nb".into(),
            width: u32::MAX,
            height: 5,
            text_permission: "x\u{7}y".into(),
        }
        .sanitize();
        assert_eq!(s.state, state::UNAVAILABLE);
        assert!(s.permission.len() <= MAX_STATUS_TEXT);
        assert_eq!(s.backend, "ab");
        assert_eq!(s.text_permission, "xy");
        assert_eq!((s.width, s.height), (MAX_DISPLAY_SIDE, 5));
    }

    #[test]
    fn nav_actions_are_checked_and_reliable() {
        for action in 0..nav::COUNT {
            let e = ev(event::Kind::Nav(Nav { action }));
            assert_eq!(e.sanitize().as_ref(), Some(&e));
            assert!(!e.is_lossy(), "a lost Home must not be silent");
        }
        assert!(
            ev(event::Kind::Nav(Nav { action: nav::COUNT }))
                .sanitize()
                .is_none()
        );
        let b = Batch {
            events: vec![ev(event::Kind::Nav(Nav { action: nav::HOME }))],
        };
        assert_eq!(Batch::decode(b.encode_to_vec().as_slice()).unwrap(), b);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(Batch::decode(&[0xff, 0xff, 0xff][..]).is_err());
        assert!(Input::decode(&[0x0a, 0xff][..]).is_err());
    }
}
