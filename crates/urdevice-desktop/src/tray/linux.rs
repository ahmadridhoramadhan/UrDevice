//! Linux tray over the StatusNotifierItem D-Bus protocol, using `ksni`.

use std::error::Error;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;

use super::Rgba;

/// Keeps the tray service running.
pub(crate) struct Guard(#[allow(dead_code)] Handle<UrDeviceTray>);

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Guard")
    }
}

pub(crate) struct UrDeviceTray {
    version: &'static str,
    icon: ksni::Icon,
}

impl ksni::Tray for UrDeviceTray {
    fn id(&self) -> String {
        "urdevice".into()
    }

    fn title(&self) -> String {
        "UrDevice".into()
    }

    // A click on the icon itself opens the window.
    fn activate(&mut self, _x: i32, _y: i32) {
        super::open_main_window();
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "UrDevice".into(),
            description: format!("Core version {}", self.version),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open".into(),
                icon_name: "window-new".into(),
                activate: Box::new(|_| super::open_main_window()),
                ..Default::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|_| {
                    let _ = slint::quit_event_loop();
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub(crate) fn install(version: &'static str, icon: Rgba) -> Result<Guard, Box<dyn Error>> {
    let tray = UrDeviceTray {
        version,
        icon: to_argb(icon),
    };
    let handle = tray.spawn()?;
    Ok(Guard(handle))
}

/// StatusNotifierItem pixmaps are ARGB32 in network byte order.
fn to_argb(icon: Rgba) -> ksni::Icon {
    let mut data = icon.pixels;
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1); // RGBA -> ARGB
    }
    ksni::Icon {
        width: icon.width as i32,
        height: icon.height as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rgba_to_argb() {
        let icon = Rgba {
            width: 1,
            height: 1,
            pixels: vec![1, 2, 3, 4],
        };
        assert_eq!(to_argb(icon).data, vec![4, 1, 2, 3]);
    }
}
