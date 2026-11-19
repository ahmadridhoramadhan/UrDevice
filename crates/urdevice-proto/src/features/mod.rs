//! Per-feature wire messages (design §6.2).
//!
//! Each feature service has its own module here, with `prost` structs written
//! by hand like the core messages in the crate root. A feature encodes its
//! messages into [`crate::Feature::body`], a stream's [`crate::StreamOpen::meta`],
//! or a datagram payload. Only the service ids in [`crate::services`] are shared
//! between features.

pub mod adb;
pub mod automations;
pub mod bluetooth;
pub mod clip_history;
pub mod clipboard;
pub mod display;
pub mod file_browser;
pub mod files;
pub mod gallery;
pub mod input;
pub mod inspector;
pub mod kvm;
pub mod media;
pub mod notify;
pub mod port_profiles;
pub mod proximity_lock;
pub mod ring;
pub mod screen;
pub mod speaker;
pub mod telephony;
pub mod terminal;
pub mod tunnel;
pub mod webcam;
