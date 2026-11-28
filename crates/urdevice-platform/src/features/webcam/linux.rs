//! Linux virtual camera: a `v4l2loopback` device written with YUYV pictures (spike T10.2).
//!
//! The module is set up once with root: `modprobe v4l2loopback devices=1 video_nr=10
//! card_label="UrDevice Camera" exclusive_caps=1`. With `exclusive_caps=1` the device looks like a
//! plain camera to other apps only while this app writes to it, which is what browsers and call
//! apps need. A loopback device is a virtual one, so it is found under `/sys/devices/virtual`.

use std::io::Write;
use std::path::{Path, PathBuf};

use v4l::video::Output;
use v4l::{Device, Format, FourCC};

use super::{OUT_HEIGHT, OUT_WIDTH, VirtualCamera, VirtualCameraError, rgba_to_yuyv_fit};

/// The card label the setup in the docs gives the device.
const LABEL: &str = "UrDevice";

struct Loopback {
    device: Device,
    /// The card label other apps list the camera by. The module fixes it when it loads.
    label: String,
}

impl VirtualCamera for Loopback {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn write(&mut self, rgba: &[u8], width: u32, height: u32) -> std::io::Result<()> {
        let Some(frame) = rgba_to_yuyv_fit(rgba, width, height, OUT_WIDTH, OUT_HEIGHT) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "bad picture",
            ));
        };
        self.device.write_all(&frame)
    }
}

/// The card label of `/dev/videoN`, from sysfs.
fn label_of(dev: &Path) -> String {
    let name = dev.file_name().map(|n| n.to_string_lossy().into_owned());
    name.and_then(|n| std::fs::read_to_string(format!("/sys/class/video4linux/{n}/name")).ok())
        .map(|l| l.trim().to_owned())
        .unwrap_or_default()
}

/// `/dev/videoN` of the loopback devices under `class_dir` (`/sys/class/video4linux`), those
/// with the UrDevice label first.
fn loopback_devices(class_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(class_dir) else {
        return Vec::new();
    };
    let mut found: Vec<(bool, PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        // `video0`, `video10`: digits after the prefix, nothing else.
        let Some(n) = name.strip_prefix("video") else {
            continue;
        };
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let path = e.path();
        let virtual_device =
            std::fs::canonicalize(&path).is_ok_and(|p| p.starts_with("/sys/devices/virtual"));
        if !virtual_device {
            continue;
        }
        let label = std::fs::read_to_string(path.join("name")).unwrap_or_default();
        found.push((
            label.trim().contains(LABEL),
            PathBuf::from(format!("/dev/{name}")),
        ));
    }
    found.sort_by_key(|(ours, p)| (!*ours, p.clone()));
    found.into_iter().map(|(_, p)| p).collect()
}

pub(super) fn open() -> Result<Box<dyn VirtualCamera>, VirtualCameraError> {
    let devices = loopback_devices(Path::new("/sys/class/video4linux"));
    if devices.is_empty() {
        return Err(VirtualCameraError::NotSetUp);
    }
    let mut last = String::new();
    for path in devices {
        let attempt = Device::with_path(&path).and_then(|device| {
            let fmt = Format::new(OUT_WIDTH, OUT_HEIGHT, FourCC::new(b"YUYV"));
            device.set_format(&fmt)?;
            Ok(device)
        });
        match attempt {
            Ok(device) => {
                tracing::info!(device = %path.display(), "virtual camera opened");
                let label = label_of(&path);
                return Ok(Box::new(Loopback { device, label }));
            }
            Err(e) => last = format!("{}: {e}", path.display()),
        }
    }
    Err(VirtualCameraError::Failed(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_class_directory_means_no_devices() {
        assert!(loopback_devices(Path::new("/nonexistent/video4linux")).is_empty());
    }
}
