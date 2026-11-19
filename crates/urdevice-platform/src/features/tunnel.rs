//! Localhost sharing and Expose on LAN (#22): desktop OS adapters.
//!
//! The tunnels themselves are plain TCP in the core. The desktop only needs
//! to open `http://localhost:<port>/` in the default browser.

use std::process::{Command, Stdio};

use urdevice_core::Result;

/// The address the viewer's browser opens for a tunnel's local port. Only
/// ever built from a port number, so no outside text reaches the command.
pub fn local_url(port: u16) -> String {
    format!("http://localhost:{port}/")
}

/// Open `http://localhost:<port>/` in the default browser.
pub fn open_in_browser(port: u16) -> Result<()> {
    let url = local_url(port);
    let mut cmd = command(&url);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    // Reap the launcher without blocking the UI thread.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn command(url: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(url);
    c
}

#[cfg(target_os = "macos")]
fn command(url: &str) -> Command {
    let mut c = Command::new("open");
    c.arg(url);
    c
}

#[cfg(target_os = "windows")]
fn command(url: &str) -> Command {
    // `rundll32 url.dll,FileProtocolHandler` hands the URL to the default
    // browser without going through `cmd /c start` and its parsing.
    let mut c = Command::new("rundll32");
    c.arg("url.dll,FileProtocolHandler").arg(url);
    c
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn command(_url: &str) -> Command {
    Command::new("false")
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_is_loopback_only() {
        assert_eq!(super::local_url(5173), "http://localhost:5173/");
    }
}
