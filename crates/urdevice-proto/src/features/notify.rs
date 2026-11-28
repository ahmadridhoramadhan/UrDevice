//! Notification mirroring (#26, design §18.2): wire messages for `ServiceId::NOTIFY`.
//!
//! Every message travels as the body of a [`crate::Feature`] envelope, wrapped
//! in [`Notify`]. Field numbers are local to this service and are the wire
//! contract: never reuse or renumber one. `proto/features/notify.proto` lists
//! the same schema.
//!
//! The receiver sends [`Subscribe`] to the host (`Direction::ToHost`). While
//! the receiver is subscribed, the host answers with [`State`]
//! (`Direction::ToController`): the notifications that are on the host now,
//! from the apps the receiver picked. A notification that is dismissed on the
//! host is missing from the next [`State`], which is how dismissal reaches the
//! receiver (R18.5).
//!
//! Everything here is untrusted input. [`State::sanitize`] and
//! [`Subscribe::sanitize`] clamp what a peer sends.

use prost::Message;

/// Most notifications a [`State`] may carry. More are dropped.
pub const MAX_ITEMS: usize = 50;

/// Most apps in a [`State`] or a [`Subscribe`].
pub const MAX_APPS: usize = 200;

/// Longest key or app id, in bytes.
pub const MAX_ID: usize = 256;

/// Longest app name or title, in bytes.
pub const MAX_TITLE: usize = 256;

/// Longest notification text, in bytes.
pub const MAX_TEXT: usize = 1024;

/// Longest `unavailable` reason, in bytes.
pub const MAX_REASON: usize = 64;

/// A notification message.
#[derive(Clone, PartialEq, Message)]
pub struct Notify {
    /// The message.
    #[prost(oneof = "message::Msg", tags = "1, 2")]
    pub msg: Option<message::Msg>,
}

/// Nested types for [`Notify`]: `message::Msg` is the oneof.
pub mod message {
    /// One notification message.
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Msg {
        /// Receiver to host: start, change, or stop mirroring (R18.2).
        #[prost(message, tag = "1")]
        Subscribe(super::Subscribe),
        /// Host to receiver: the notifications on the host now.
        #[prost(message, tag = "2")]
        State(super::State),
    }
}

impl From<message::Msg> for Notify {
    fn from(msg: message::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

/// Receiver to host: turn mirroring on or off, and say which apps (R18.2).
#[derive(Clone, PartialEq, Message)]
pub struct Subscribe {
    /// True to receive [`State`] now and on every change; false to stop.
    #[prost(bool, tag = "1")]
    pub on: bool,
    /// The ids of the apps to mirror. Notifications from any other app never
    /// leave the host.
    #[prost(string, repeated, tag = "2")]
    pub apps: Vec<String>,
}

impl Subscribe {
    /// Clamp a subscription a peer sent: at most [`MAX_APPS`] apps, each id
    /// non-empty and at most [`MAX_ID`] bytes, without duplicates.
    pub fn sanitize(&mut self) {
        self.apps.truncate(MAX_APPS);
        let mut seen = std::collections::HashSet::new();
        self.apps
            .retain(|a| !a.is_empty() && a.len() <= MAX_ID && seen.insert(a.clone()));
    }
}

/// One notification on the host.
#[derive(Clone, PartialEq, Message)]
pub struct Item {
    /// Stable id on the host while the notification is shown.
    #[prost(string, tag = "1")]
    pub key: String,
    /// The id of the app that posted it (the Android package name).
    #[prost(string, tag = "2")]
    pub app_id: String,
    /// The app's name.
    #[prost(string, tag = "3")]
    pub app_name: String,
    /// The title. For a private notification: "New notification from <app>"
    /// (R18.3).
    #[prost(string, tag = "4")]
    pub title: String,
    /// The text; empty for a private notification.
    #[prost(string, tag = "5")]
    pub text: String,
    /// When the app posted it, in milliseconds since the Unix epoch.
    #[prost(uint64, tag = "6")]
    pub posted_ms: u64,
    /// The host hid the content because the notification is private (R18.3).
    #[prost(bool, tag = "7")]
    pub hidden: bool,
}

impl Item {
    /// Clamp an item a peer sent: cut each text at a character boundary.
    pub fn sanitize(&mut self) {
        cut(&mut self.key, MAX_ID);
        cut(&mut self.app_id, MAX_ID);
        cut(&mut self.app_name, MAX_TITLE);
        cut(&mut self.title, MAX_TITLE);
        cut(&mut self.text, MAX_TEXT);
    }
}

/// One app that has a notification on the host. The receiver lists it so the
/// user can pick it.
#[derive(Clone, PartialEq, Message)]
pub struct App {
    /// The app id (the Android package name).
    #[prost(string, tag = "1")]
    pub id: String,
    /// The app's name.
    #[prost(string, tag = "2")]
    pub name: String,
}

/// Host to receiver: everything the receiver draws.
#[derive(Clone, PartialEq, Message)]
pub struct State {
    /// The notifications from the apps the receiver picked, newest first.
    #[prost(message, repeated, tag = "1")]
    pub items: Vec<Item>,
    /// Every app that has a notification on the host now, picked or not.
    #[prost(message, repeated, tag = "2")]
    pub apps: Vec<App>,
    /// Why the host cannot read its notifications: `listener_permission`
    /// (Android notification access is off), `unsupported`, or empty.
    #[prost(string, tag = "3")]
    pub unavailable: String,
}

impl State {
    /// Clamp a state a peer sent: at most [`MAX_ITEMS`] items and
    /// [`MAX_APPS`] apps, no empty or repeated keys or app ids.
    pub fn sanitize(&mut self) {
        self.items.truncate(MAX_ITEMS);
        let mut keys = std::collections::HashSet::new();
        self.items.retain_mut(|i| {
            i.sanitize();
            !i.key.is_empty() && !i.app_id.is_empty() && keys.insert(i.key.clone())
        });
        self.apps.truncate(MAX_APPS);
        let mut ids = std::collections::HashSet::new();
        self.apps.retain_mut(|a| {
            cut(&mut a.id, MAX_ID);
            cut(&mut a.name, MAX_TITLE);
            !a.id.is_empty() && ids.insert(a.id.clone())
        });
        cut(&mut self.unavailable, MAX_REASON);
    }
}

/// Cut `s` to at most `max` bytes without splitting a character.
pub fn cut(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(key: &str) -> Item {
        Item {
            key: key.into(),
            app_id: "com.chat".into(),
            app_name: "Chat".into(),
            title: "Ana".into(),
            text: "hello".into(),
            posted_ms: 1,
            hidden: false,
        }
    }

    #[test]
    fn round_trip() {
        let m = Notify::from(message::Msg::State(State {
            items: vec![item("a")],
            apps: vec![App {
                id: "com.chat".into(),
                name: "Chat".into(),
            }],
            unavailable: String::new(),
        }));
        assert_eq!(Notify::decode(m.encode_to_vec().as_slice()).unwrap(), m);
        let s = Notify::from(message::Msg::Subscribe(Subscribe {
            on: true,
            apps: vec!["com.chat".into()],
        }));
        assert_eq!(Notify::decode(s.encode_to_vec().as_slice()).unwrap(), s);
    }

    #[test]
    fn garbage_does_not_panic() {
        for len in 0..64u8 {
            let junk: Vec<u8> = (0..len)
                .map(|i| i.wrapping_mul(37).wrapping_add(11))
                .collect();
            let _ = Notify::decode(junk.as_slice());
        }
    }

    #[test]
    fn sanitize_clamps_hostile_state() {
        let mut st = State {
            items: (0..120).map(|i| item(&format!("k{i}"))).collect(),
            apps: (0..400)
                .map(|i| App {
                    id: format!("app{i}"),
                    name: "é".repeat(MAX_TITLE),
                })
                .collect(),
            unavailable: "x".repeat(500),
        };
        st.items[0].text = "é".repeat(MAX_TEXT);
        st.items[1].key = String::new();
        st.items[2].key = "k3".into(); // repeats a later key: the first wins
        st.sanitize();
        assert!(st.items.len() <= MAX_ITEMS);
        assert!(st.items.iter().all(|i| !i.key.is_empty()));
        assert!(st.items[0].text.len() <= MAX_TEXT);
        let mut keys: Vec<_> = st.items.iter().map(|i| i.key.clone()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), st.items.len());
        assert!(st.apps.len() <= MAX_APPS);
        assert!(st.apps.iter().all(|a| a.name.len() <= MAX_TITLE));
        assert!(st.unavailable.len() <= MAX_REASON);
    }

    #[test]
    fn sanitize_clamps_subscription() {
        let mut s = Subscribe {
            on: true,
            apps: vec![
                String::new(),
                "a".into(),
                "a".into(),
                "b".repeat(MAX_ID + 1),
            ],
        };
        s.apps.extend((0..500).map(|i| format!("p{i}")));
        s.sanitize();
        assert_eq!(s.apps[0], "a");
        assert!(s.apps.len() <= MAX_APPS);
        assert!(!s.apps.iter().any(String::is_empty));
    }
}
