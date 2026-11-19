//! One module per feature (design §9). Each adds its own exported methods on
//! [`crate::UrDeviceNode`] and its own callback interfaces, so features never
//! edit `lib.rs`. See `docs/dev/adding-a-feature.md`.

mod adb;
mod automations;
mod bluetooth;
mod clip_history;
mod clipboard;
mod display;
mod file_browser;
mod files;
mod gallery;
mod input;
mod inspector;
mod kvm;
mod media;
mod notify;
mod port_profiles;
mod proximity_lock;
mod ring;
mod screen;
mod speaker;
mod telephony;
mod terminal;
mod tunnel;
mod webcam;
