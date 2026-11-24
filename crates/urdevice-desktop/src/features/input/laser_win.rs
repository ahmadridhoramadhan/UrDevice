//! The presenter's laser dot on Windows (R9b.3, design §9.3).
//!
//! A transparent Slint window is drawn by the OpenGL renderer, and on Windows
//! that renderer fills the transparent parts with black: the whole screen went
//! dark with only the dot visible. So on Windows the dot is a small Win32
//! *layered* window instead. Its pixels carry their own alpha
//! (`UpdateLayeredWindow`), so no renderer is involved, and the window is
//! only as big as the dot, so the rest of the screen is never covered. It is
//! click-through (`WS_EX_TRANSPARENT`), always on top, and never takes focus.
//!
//! Everything here runs on the UI thread, which pumps the window's messages.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP,
    HDC, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, RegisterClassW, SM_CXSCREEN,
    SM_CYSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, ULW_ALPHA, UpdateLayeredWindow, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

/// Side of the dot's window, in pixels. The dot and its glow fit inside.
const SIDE: i32 = 64;

/// The dot and the window it lives in.
pub(super) struct Dot {
    hwnd: HWND,
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    visible: bool,
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: forwards the window's own message to the default handler.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

impl Dot {
    /// Make the window, hidden. Fails when Windows refuses a window or a bitmap.
    pub(super) fn new() -> Result<Self, String> {
        // SAFETY: plain Win32 calls on the UI thread. Every handle made here is
        // released in `Drop`, or right away on an error path.
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class = w!("UrDeviceLaserDot");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            };
            // Zero means "already registered" on the second dot; creating still works.
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TRANSPARENT
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE,
                class,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                SIDE,
                SIDE,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| e.to_string())?;

            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: SIDE,
                    // A negative height is a top-down bitmap.
                    biHeight: -SIDE,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap =
                match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                    Ok(b) if !bits.is_null() => b,
                    other => {
                        let why = other
                            .err()
                            .map_or("no bitmap memory".to_owned(), |e| e.to_string());
                        let _ = DeleteDC(memory);
                        ReleaseDC(None, screen);
                        let _ = DestroyWindow(hwnd);
                        return Err(why);
                    }
                };
            let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (SIDE * SIDE) as usize);
            pixels.copy_from_slice(&dot_pixels());
            let previous = SelectObject(memory, bitmap.into());
            Ok(Self {
                hwnd,
                screen,
                memory,
                bitmap,
                previous,
                visible: false,
            })
        }
    }

    /// Put the dot's centre at `x`, `y` (0.0 to 1.0 across and down the primary display).
    pub(super) fn show_at(&mut self, x: f32, y: f32) -> Result<(), String> {
        // SAFETY: the handles are alive until `Drop`; the pointers outlive the call.
        unsafe {
            let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let at = POINT {
                x: (x.clamp(0.0, 1.0) * (w - 1).max(0) as f32) as i32 - SIDE / 2,
                y: (y.clamp(0.0, 1.0) * (h - 1).max(0) as f32) as i32 - SIDE / 2,
            };
            let size = SIZE { cx: SIDE, cy: SIDE };
            let from = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            if !self.visible {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.visible = true;
            }
            UpdateLayeredWindow(
                self.hwnd,
                Some(self.screen),
                Some(&at),
                Some(&size),
                Some(self.memory),
                Some(&from),
                Default::default(),
                Some(&blend),
                ULW_ALPHA,
            )
            .map_err(|e| e.to_string())
        }
    }

    /// Hide the dot.
    pub(super) fn hide(&mut self) {
        if self.visible {
            // SAFETY: the window is alive until `Drop`.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }
}

impl Drop for Dot {
    fn drop(&mut self) {
        // SAFETY: each handle was made in `new` and is released once.
        unsafe {
            SelectObject(self.memory, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.memory);
            ReleaseDC(None, self.screen);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The dot as premultiplied BGRA pixels: a red disc, a white ring, a soft red glow.
fn dot_pixels() -> Vec<u32> {
    let centre = SIDE as f32 / 2.0;
    let mut out = Vec::with_capacity((SIDE * SIDE) as usize);
    for py in 0..SIDE {
        for px in 0..SIDE {
            let d =
                ((px as f32 + 0.5 - centre).powi(2) + (py as f32 + 0.5 - centre).powi(2)).sqrt();
            // [r, g, b, a] premultiplied, built from the bottom layer up.
            let mut c = [0.0_f32; 4];
            let mut over = |rgb: [f32; 3], a: f32| {
                let a = a.clamp(0.0, 1.0);
                for i in 0..3 {
                    c[i] = rgb[i] * a + c[i] * (1.0 - a);
                }
                c[3] = a + c[3] * (1.0 - a);
            };
            let red = [1.0, 0.125, 0.125];
            // Glow: fades out between 12 and 31 pixels from the centre.
            let glow = ((31.0 - d) / 19.0).clamp(0.0, 1.0);
            over(red, 0.5 * glow * glow);
            // White ring around the disc.
            over(
                [1.0, 1.0, 1.0],
                0.8 * (13.5 - d).clamp(0.0, 1.0) * (d - 10.5).clamp(0.0, 1.0),
            );
            // The disc.
            over(red, (11.5 - d).clamp(0.0, 1.0));
            let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u32;
            out.push(byte(c[3]) << 24 | byte(c[0]) << 16 | byte(c[1]) << 8 | byte(c[2]));
        }
    }
    out
}
