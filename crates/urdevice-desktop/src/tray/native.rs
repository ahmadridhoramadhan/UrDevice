//! Windows and macOS tray, using `tray-icon`.

use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;
use std::time::Duration;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use super::Rgba;

/// Holds the tray icon once the event loop has created it.
pub(crate) struct Guard(#[allow(dead_code)] Rc<RefCell<Option<TrayIcon>>>);

impl std::fmt::Debug for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Guard")
    }
}

pub(crate) fn install(version: &'static str, icon: Rgba) -> Result<Guard, Box<dyn Error>> {
    let slot = Rc::new(RefCell::new(None));
    let target = Rc::clone(&slot);

    // macOS requires the tray to be created on the main thread after the event
    // loop has started, so build it from the first event-loop tick.
    slint::Timer::single_shot(Duration::ZERO, move || match build(version, icon) {
        Ok(tray) => *target.borrow_mut() = Some(tray),
        Err(err) => eprintln!("urdevice: tray icon unavailable: {err}"),
    });

    Ok(Guard(slot))
}

fn build(version: &'static str, icon: Rgba) -> Result<TrayIcon, Box<dyn Error>> {
    let open_item = MenuItem::new("Open", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&open_item, &PredefinedMenuItem::separator(), &quit_item])?;

    let (open_id, quit_id) = (open_item.id().clone(), quit_item.id().clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == quit_id {
            let _ = slint::quit_event_loop();
        } else if event.id == open_id {
            super::open_main_window();
        }
    }));
    // A left click on the icon opens the window too.
    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            super::open_main_window();
        }
    }));

    let tray = TrayIconBuilder::new()
        .with_icon(Icon::from_rgba(icon.pixels, icon.width, icon.height)?)
        .with_tooltip(format!("UrDevice {version}"))
        .with_menu(Box::new(menu))
        .build()?;
    Ok(tray)
}
