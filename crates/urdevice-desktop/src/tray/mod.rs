//! Tray icon that shows the core version and a Quit item.
//!
//! Windows and macOS use the `tray-icon` crate. Linux talks the
//! StatusNotifierItem D-Bus protocol through `ksni`, which works on KDE, most
//! wlroots desktops, and GNOME with the AppIndicator extension, and needs no GTK.

use std::error::Error;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native;

/// 32 × 32 RGBA app icon used for the tray.
const ICON_PNG: &[u8] = include_bytes!("../../../../assets/urdevice-icon-32.png");

/// Decoded RGBA8 image.
#[derive(Debug, Clone)]
pub(crate) struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn decode_icon() -> Result<Rgba, Box<dyn Error>> {
    let decoder = png::Decoder::new(std::io::Cursor::new(ICON_PNG));
    let mut reader = decoder.read_info()?;
    let size = reader
        .output_buffer_size()
        .ok_or("tray icon is too large")?;
    let mut pixels = vec![0; size];
    let info = reader.next_frame(&mut pixels)?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("tray icon must be 8-bit RGBA".into());
    }
    pixels.truncate(info.buffer_size());
    Ok(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

/// Keeps the tray icon alive. Drop it to remove the icon.
#[derive(Debug)]
pub(crate) struct TrayGuard {
    #[cfg(target_os = "linux")]
    _inner: Option<linux::Guard>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    _inner: Option<native::Guard>,
}

/// Show the tray icon. Errors are reported on stderr and do not stop the app.
pub(crate) fn install(version: &'static str) -> TrayGuard {
    let inner = decode_icon().and_then(|icon| {
        #[cfg(target_os = "linux")]
        let guard = linux::install(version, icon);
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let guard = native::install(version, icon);
        guard
    });

    match inner {
        Ok(guard) => TrayGuard {
            _inner: Some(guard),
        },
        Err(err) => {
            eprintln!("urdevice: tray icon unavailable: {err}");
            TrayGuard { _inner: None }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_decodes_to_rgba() {
        let icon = super::decode_icon().expect("bundled icon decodes");
        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.pixels.len(), 32 * 32 * 4);
    }
}
