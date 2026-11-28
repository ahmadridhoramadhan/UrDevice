//! Notification mirroring (#26): bindings for the Android app.
//!
//! Kotlin gives the core the notifications that are on this phone
//! ([`NotifyHostFfi`]), for desktops that turned mirroring on. The phone does
//! not receive notifications from other devices yet.

use std::sync::Arc;

use urdevice_core::services::notify::{Notify, NotifyApp, NotifyHost, NotifyItem, NotifyState};

use crate::UrDeviceNode;

/// One notification on the phone.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiNotifItem {
    /// Stable while the notification is shown (the Android key).
    pub key: String,
    /// The package name of the app that posted it.
    pub app_id: String,
    pub app_name: String,
    /// "New notification from <app>" when `hidden`.
    pub title: String,
    /// Empty when `hidden`.
    pub text: String,
    /// Milliseconds since the Unix epoch.
    pub posted_ms: u64,
    /// The content was hidden because the notification is private (R18.3).
    pub hidden: bool,
}

/// One app that has a notification on the phone.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiNotifApp {
    pub id: String,
    pub name: String,
}

/// Everything the phone has to show.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiNotifState {
    /// Newest first. Secret notifications are not in the list.
    pub items: Vec<FfiNotifItem>,
    /// Every app that has a notification now.
    pub apps: Vec<FfiNotifApp>,
    /// Empty, `listener_permission`, or `unsupported`.
    pub unavailable: String,
}

/// The phone's notifications, implemented in Kotlin. Called from core worker
/// threads.
#[uniffi::export(with_foreign)]
pub trait NotifyHostFfi: Send + Sync {
    fn state(&self) -> FfiNotifState;
}

impl From<FfiNotifState> for NotifyState {
    fn from(s: FfiNotifState) -> Self {
        Self {
            items: s
                .items
                .into_iter()
                .map(|i| NotifyItem {
                    key: i.key,
                    app_id: i.app_id,
                    app_name: i.app_name,
                    title: i.title,
                    text: i.text,
                    posted_ms: i.posted_ms,
                    hidden: i.hidden,
                })
                .collect(),
            apps: s
                .apps
                .into_iter()
                .map(|a| NotifyApp {
                    id: a.id,
                    name: a.name,
                })
                .collect(),
            unavailable: s.unavailable,
        }
    }
}

struct HostAdapter(Arc<dyn NotifyHostFfi>);

impl NotifyHost for HostAdapter {
    fn state(&self) -> NotifyState {
        self.0.state().into()
    }
}

#[uniffi::export]
impl UrDeviceNode {
    /// Give the core this phone's notifications.
    pub fn notify_set_host(&self, host: Arc<dyn NotifyHostFfi>) {
        if let Some(s) = self.node.service::<Notify>() {
            s.set_host(Arc::new(HostAdapter(host)));
        }
    }
}
