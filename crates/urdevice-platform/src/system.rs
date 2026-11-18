//! Small desktop facts: data directory, device name, battery (R4.9).

use std::path::PathBuf;

/// App data directory, for example `~/.local/share/urdevice` on Linux.
/// `URDEVICE_DATA_DIR` overrides it, which lets two instances run side by side.
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("URDEVICE_DATA_DIR") {
        return PathBuf::from(d);
    }
    directories::ProjectDirs::from("dev", "UrDevice", "urdevice")
        .map(|p| p.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("urdevice"))
}

/// A first device name: the host name.
pub fn device_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let host = host.split('.').next().unwrap_or("").trim().to_owned();
    if host.is_empty() {
        "My computer".to_owned()
    } else {
        host
    }
}

/// Platform string for the core.
pub fn platform_id() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Battery level (0–100) and charging flag, or `None` on a desktop without a battery.
pub fn battery() -> Option<(u8, bool)> {
    let manager = starship_battery::Manager::new().ok()?;
    let battery = manager.batteries().ok()?.flatten().next()?;
    let pct = (battery.state_of_charge().value * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let charging = matches!(
        battery.state(),
        starship_battery::State::Charging | starship_battery::State::Full
    );
    Some((pct, charging))
}

#[cfg(test)]
mod tests {
    #[test]
    fn facts_are_sane() {
        assert!(!super::device_name().is_empty());
        if let Some((pct, _)) = super::battery() {
            assert!(pct <= 100);
        }
    }
}
