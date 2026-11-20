//! Windows capture with Windows.Graphics.Capture (design §10).
//!
//! One D3D11 device per capture, a free-threaded frame pool of two BGRA
//! buffers, and a `FrameArrived` handler that copies the frame through a
//! CPU-readable staging texture when the handler wants one. Windows 11 draws
//! its own yellow capture border unless the app opts out; the border stays
//! as an extra indicator.

use std::sync::{Arc, Mutex};

use urdevice_core::Result;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, LPARAM, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{BOOL, Interface};

use super::{CaptureHandle, Display, FrameHandler, copy_rows, refused};

/// `MONITORINFOF_PRIMARY`.
const PRIMARY: u32 = 1;

struct Monitor {
    handle: HMONITOR,
    name: String,
    width: u32,
    height: u32,
    primary: bool,
}

unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the `Vec<HMONITOR>` passed to EnumDisplayMonitors.
    let list = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    list.push(m);
    BOOL(1)
}

fn monitors() -> Vec<Monitor> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut handles as *mut _ as isize),
        );
    }
    handles
        .into_iter()
        .enumerate()
        .filter_map(|(i, h)| {
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            // SAFETY: `info` is a MONITORINFOEXW with its size set.
            let ok = unsafe { GetMonitorInfoW(h, &mut info as *mut _ as *mut MONITORINFO) };
            if !ok.as_bool() {
                return None;
            }
            let r = info.monitorInfo.rcMonitor;
            let dev = String::from_utf16_lossy(&info.szDevice);
            let dev = dev.trim_end_matches('\0').trim_start_matches("\\\\.\\");
            Some(Monitor {
                handle: h,
                name: format!("Display {} ({dev})", i + 1),
                width: (r.right - r.left).max(0) as u32,
                height: (r.bottom - r.top).max(0) as u32,
                primary: info.monitorInfo.dwFlags & PRIMARY != 0,
            })
        })
        .collect()
}

pub(super) fn displays() -> Vec<Display> {
    monitors()
        .into_iter()
        .enumerate()
        .map(|(i, m)| Display {
            id: i as u32 + 1,
            name: m.name,
            width: m.width,
            height: m.height,
            primary: m.primary,
        })
        .collect()
}

/// The CPU-side copy state, used from the frame pool's worker thread.
struct Copier {
    winrt: IDirect3DDevice,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

// SAFETY: D3D11 devices are free-threaded; the immediate context is only
// used behind the `Mutex` that holds this.
unsafe impl Send for Copier {}

impl Copier {
    /// Copy one frame texture to memory.
    unsafe fn read(&mut self, tex: &ID3D11Texture2D) -> Option<super::Frame> {
        // SAFETY: valid D3D11 objects; the staging texture matches the
        // source's size and format; the mapping is released before return.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            let (w, h) = (desc.Width, desc.Height);
            if !matches!(&self.staging, Some((_, sw, sh)) if *sw == w && *sh == h) {
                let mut sd = desc;
                sd.Usage = D3D11_USAGE_STAGING;
                sd.BindFlags = 0;
                sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                sd.MiscFlags = 0;
                sd.MipLevels = 1;
                sd.ArraySize = 1;
                let mut t = None;
                self.device.CreateTexture2D(&sd, None, Some(&mut t)).ok()?;
                self.staging = Some((t?, w, h));
            }
            let (staging, _, _) = self.staging.as_ref()?;
            let dst: ID3D11Resource = staging.cast().ok()?;
            let src: ID3D11Resource = tex.cast().ok()?;
            self.context.CopyResource(&dst, &src);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&dst, 0, D3D11_MAP_READ, 0, Some(&mut map))
                .ok()?;
            let stride = map.RowPitch as usize;
            let len = stride * h as usize;
            let bytes = std::slice::from_raw_parts(map.pData as *const u8, len);
            let frame = copy_rows(bytes, w, h, stride, false);
            self.context.Unmap(&dst, 0);
            frame
        }
    }
}

pub(super) fn start(id: u32, fps: u32, handler: Arc<dyn FrameHandler>) -> Result<CaptureHandle> {
    let monitor = monitors()
        .into_iter()
        .nth((id as usize).checked_sub(1).ok_or_else(|| refused("no_display"))?)
        .ok_or_else(|| refused("no_display"))?;
    let fail = |e: windows::core::Error| {
        tracing::warn!(error = %e, "Windows.Graphics.Capture failed");
        refused("capture_failed")
    };
    // SAFETY: standard WinRT/D3D11 set-up with checked results.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(fail)?;
        let device = device.ok_or_else(|| refused("capture_failed"))?;
        let context = context.ok_or_else(|| refused("capture_failed"))?;
        let dxgi: IDXGIDevice = device.cast().map_err(fail)?;
        let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .and_then(|i| i.cast())
            .map_err(fail)?;
        let interop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(fail)?;
        let item: GraphicsCaptureItem = interop.CreateForMonitor(monitor.handle).map_err(fail)?;
        let size = item.Size().map_err(fail)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(fail)?;
        let session = pool.CreateCaptureSession(&item).map_err(fail)?;
        let _ = session.SetIsCursorCaptureEnabled(true);
        // Windows 11 24H2: deliver at most `fps` frames a second.
        let _ = session.SetMinUpdateInterval(windows::Foundation::TimeSpan {
            Duration: 10_000_000 / i64::from(fps.max(1)),
        });

        let copier = Arc::new(Mutex::new(Copier {
            winrt: winrt_device.clone(),
            device,
            context,
            staging: None,
        }));
        let last_size = Arc::new(Mutex::new(size));
        let h = handler.clone();
        let token = pool
            .FrameArrived(&TypedEventHandler::new(
                move |pool: windows::core::Ref<Direct3D11CaptureFramePool>, _| {
                    let Some(pool) = pool.as_ref() else {
                        return Ok(());
                    };
                    let frame = pool.TryGetNextFrame()?;
                    let content = frame.ContentSize()?;
                    let mut c = match copier.lock() {
                        Ok(g) => g,
                        Err(p) => p.into_inner(),
                    };
                    {
                        let mut last = match last_size.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        if content.Width != last.Width || content.Height != last.Height {
                            // The display changed size: new buffers.
                            *last = content;
                            pool.Recreate(
                                &c.winrt,
                                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                                2,
                                content,
                            )?;
                            return Ok(());
                        }
                    }
                    if !h.wants() {
                        return Ok(());
                    }
                    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
                    let tex: ID3D11Texture2D = access.GetInterface()?;
                    if let Some(f) = c.read(&tex) {
                        h.frame(f);
                    }
                    Ok(())
                },
            ))
            .map_err(fail)?;
        session.StartCapture().map_err(fail)?;
        tracing::info!(display = id, "Windows.Graphics.Capture started");

        let parts = Parts {
            pool,
            session,
            token,
        };
        Ok(CaptureHandle::new(move || parts.close()))
    }
}

struct Parts {
    pool: Direct3D11CaptureFramePool,
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    token: i64,
}

// SAFETY: WinRT capture objects are agile (free-threaded).
unsafe impl Send for Parts {}

impl Parts {
    fn close(self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
