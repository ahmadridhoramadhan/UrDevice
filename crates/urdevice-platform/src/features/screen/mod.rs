//! Screen sharing (#21, design §10): desktop screen capture.
//!
//! [`displays`] lists what can be shared and [`start`] captures one display,
//! handing BGRA frames to a [`FrameHandler`] on the back-end's own thread:
//!
//! | OS | Back-end |
//! |---|---|
//! | Windows | Windows.Graphics.Capture on a free-threaded frame pool |
//! | macOS | ScreenCaptureKit (needs the Screen Recording permission) |
//! | Linux, Wayland | ScreenCast portal, then a PipeWire stream (libpipewire loaded at run time) |
//! | Linux, X11 | `GetImage` on the root window, per RandR monitor |
//!
//! The back-ends ask [`FrameHandler::wants`] before copying a frame, so a
//! slow network or encoder costs no copies. Frames are never stored or
//! logged.

use std::sync::Arc;

use urdevice_core::Result;
pub use urdevice_core::services::screen::Display;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// One captured frame: BGRA (or BGRx), 4 bytes per pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: usize,
    /// The pixels.
    pub data: Vec<u8>,
}

/// Receives frames from a capture back-end. Called on the back-end's thread.
pub trait FrameHandler: Send + Sync {
    /// False to skip this frame without copying it.
    fn wants(&self) -> bool;
    /// One frame.
    fn frame(&self, frame: Frame);
    /// Capture ended on its own: `stopped` (the user ended it in the OS),
    /// `capture_failed`, or `no_display`.
    fn ended(&self, reason: &'static str);
}

/// A running capture. Dropping it stops the capture.
pub struct CaptureHandle {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl CaptureHandle {
    pub(crate) fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        Self {
            stop: Some(Box::new(stop)),
        }
    }

    /// Stop now.
    pub fn stop(&mut self) {
        if let Some(f) = self.stop.take() {
            f();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

impl std::fmt::Debug for CaptureHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CaptureHandle")
    }
}

/// Displays that can be shared. On Wayland the portal's own dialog picks the
/// monitor, so the list is one entry with id 0 and no size.
pub fn displays() -> Vec<Display> {
    #[cfg(target_os = "linux")]
    {
        linux::displays()
    }
    #[cfg(target_os = "windows")]
    {
        windows::displays()
    }
    #[cfg(target_os = "macos")]
    {
        macos::displays()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Vec::new()
    }
}

/// Start capturing display `id` at about `fps` frames per second. May block
/// while an OS dialog is open (the Wayland portal). Errors are
/// [`urdevice_core::CoreError::Refused`] with `consent_declined`,
/// `no_display`, or `capture_failed`.
pub fn start(id: u32, fps: u32, handler: Arc<dyn FrameHandler>) -> Result<CaptureHandle> {
    let fps = fps.clamp(1, 60);
    #[cfg(target_os = "linux")]
    {
        linux::start(id, fps, handler)
    }
    #[cfg(target_os = "windows")]
    {
        windows::start(id, fps, handler)
    }
    #[cfg(target_os = "macos")]
    {
        macos::start(id, fps, handler)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = (id, fps, handler);
        Err(urdevice_core::CoreError::Refused("no_host".into()))
    }
}

pub(crate) fn refused(reason: &str) -> urdevice_core::CoreError {
    urdevice_core::CoreError::Refused(reason.into())
}

/// Copy `height` rows of `width * 4` bytes out of a buffer with `stride`,
/// converting RGBA-order pixels to BGRA when `swap_rb` is set. Returns
/// `None` when the buffer is too short.
pub(crate) fn copy_rows(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    swap_rb: bool,
) -> Option<Frame> {
    let row = width as usize * 4;
    if width == 0 || height == 0 || stride < row {
        return None;
    }
    let need = stride
        .checked_mul(height as usize - 1)?
        .checked_add(row)?;
    if src.len() < need {
        return None;
    }
    let mut data = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        data.extend_from_slice(&src[y * stride..y * stride + row]);
    }
    if swap_rb {
        for px in data.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    Some(Frame {
        width,
        height,
        stride: row,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_rows_strips_padding_and_swaps() {
        let src = [1, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
        let f = copy_rows(&src, 1, 2, 6, false).unwrap();
        assert_eq!((f.stride, f.data), (4, vec![1, 2, 3, 4, 5, 6, 7, 8]));
        let f = copy_rows(&src, 1, 2, 6, true).unwrap();
        assert_eq!(f.data, vec![3, 2, 1, 4, 7, 6, 5, 8]);
        assert!(copy_rows(&src[..9], 1, 2, 6, false).is_none());
        assert!(copy_rows(&src, 2, 2, 6, false).is_none());
        assert!(copy_rows(&src, 0, 2, 6, false).is_none());
    }
}
