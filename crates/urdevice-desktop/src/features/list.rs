//! Every desktop feature, in the order of the device-view tiles.

use super::Feature;

pub(crate) fn all() -> Vec<Box<dyn Feature>> {
    vec![
        Box::new(super::clipboard::Clipboard),
        Box::new(super::files::Files),
        Box::new(super::gallery::Gallery),
        Box::new(super::input::Input),
        Box::new(super::media::Media),
        Box::new(super::screen::Screen),
        Box::new(super::tunnel::Tunnel),
        Box::new(super::bluetooth::Bluetooth),
        Box::new(super::notify::Notify),
        Box::new(super::ring::Ring),
        Box::new(super::webcam::Webcam),
        Box::new(super::telephony::Telephony),
        Box::new(super::kvm::Kvm),
        Box::new(super::display::Display),
        Box::new(super::terminal::Terminal),
        Box::new(super::port_profiles::PortProfiles),
        Box::new(super::inspector::Inspector),
        Box::new(super::adb::Adb),
        Box::new(super::file_browser::FileBrowser),
        Box::new(super::clip_history::ClipHistory),
        Box::new(super::proximity_lock::ProximityLock),
        Box::new(super::automations::Automations),
        Box::new(super::speaker::Speaker),
    ]
}
