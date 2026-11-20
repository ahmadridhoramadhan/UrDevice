//! A PipeWire video stream on the portal's remote, with `libpipewire-0.3`
//! loaded at run time. Every desktop with the ScreenCast portal ships the
//! library; building against it would need its headers and libclang on
//! every build machine, so only the few calls below are declared here.
//!
//! The stream runs on a `pw_thread_loop`. Its `process` callback dequeues a
//! buffer, copies it out only when the [`FrameHandler`] wants a frame, and
//! queues it back. Buffers are mapped shared memory: the offered format has
//! no DMA-BUF modifier.

use std::ffi::{c_char, c_int, c_void};
use std::os::fd::{IntoRawFd, OwnedFd};
use std::sync::{Arc, Once};

use libloading::Library;
use urdevice_core::Result;

use super::super::{FrameHandler, copy_rows, refused};
use super::pod;

type Ptr = *mut c_void;

/// `struct spa_hook`: six pointers, owned by the caller while registered.
#[repr(C)]
struct SpaHook([usize; 6]);

/// `struct pw_stream_events`, version 2.
#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(Ptr)>,
    state_changed: Option<unsafe extern "C" fn(Ptr, c_int, c_int, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(Ptr, u32, Ptr)>,
    io_changed: Option<unsafe extern "C" fn(Ptr, u32, Ptr, u32)>,
    param_changed: Option<unsafe extern "C" fn(Ptr, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(Ptr, Ptr)>,
    remove_buffer: Option<unsafe extern "C" fn(Ptr, Ptr)>,
    process: Option<unsafe extern "C" fn(Ptr)>,
    drained: Option<unsafe extern "C" fn(Ptr)>,
    command: Option<unsafe extern "C" fn(Ptr, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(Ptr)>,
}

/// `struct pw_buffer` (first field only).
#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
}

/// `struct spa_buffer`.
#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: Ptr,
    datas: *mut SpaData,
}

/// `struct spa_data`.
#[repr(C)]
struct SpaData {
    kind: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: Ptr,
    chunk: *mut SpaChunk,
}

/// `struct spa_chunk`.
#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

const STREAM_STATE_ERROR: c_int = -1;
const STREAM_STATE_UNCONNECTED: c_int = 0;
const STREAM_STATE_STREAMING: c_int = 3;
const DIRECTION_INPUT: u32 = 0;
const FLAG_AUTOCONNECT: u32 = 1 << 0;
const FLAG_MAP_BUFFERS: u32 = 1 << 2;
const CHUNK_FLAG_CORRUPTED: i32 = 1;

macro_rules! api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Api {
            _lib: Library,
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            fn load() -> std::result::Result<Self, libloading::Error> {
                // SAFETY: the library is the system's libpipewire; each symbol
                // is declared with its C signature from pipewire's headers.
                unsafe {
                    let lib = Library::new("libpipewire-0.3.so.0")?;
                    $(let $name = *lib.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(
                        concat!(stringify!($name), "\0").as_bytes(),
                    )?;)*
                    Ok(Self { _lib: lib, $($name,)* })
                }
            }
        }
    };
}

api! {
    pw_init: fn(*mut c_int, *mut *mut *mut c_char);
    pw_thread_loop_new: fn(*const c_char, *const c_void) -> Ptr;
    pw_thread_loop_get_loop: fn(Ptr) -> Ptr;
    pw_thread_loop_start: fn(Ptr) -> c_int;
    pw_thread_loop_stop: fn(Ptr);
    pw_thread_loop_destroy: fn(Ptr);
    pw_thread_loop_lock: fn(Ptr);
    pw_thread_loop_unlock: fn(Ptr);
    pw_context_new: fn(Ptr, Ptr, usize) -> Ptr;
    pw_context_destroy: fn(Ptr);
    pw_context_connect_fd: fn(Ptr, c_int, Ptr, usize) -> Ptr;
    pw_core_disconnect: fn(Ptr) -> c_int;
    pw_properties_new_string: fn(*const c_char) -> Ptr;
    pw_stream_new: fn(Ptr, *const c_char, Ptr) -> Ptr;
    pw_stream_add_listener: fn(Ptr, *mut SpaHook, *const StreamEvents, Ptr);
    pw_stream_connect: fn(Ptr, u32, u32, u32, *mut *const c_void, u32) -> c_int;
    pw_stream_disconnect: fn(Ptr) -> c_int;
    pw_stream_destroy: fn(Ptr);
    pw_stream_dequeue_buffer: fn(Ptr) -> *mut PwBuffer;
    pw_stream_queue_buffer: fn(Ptr, *mut PwBuffer) -> c_int;
}

/// State the callbacks see, behind the `data` pointer.
struct Shared {
    api: Arc<Api>,
    stream: Ptr,
    handler: Arc<dyn FrameHandler>,
    format: pod::VideoFormat,
    streaming: bool,
    ended: bool,
}

/// A running stream. Dropping it stops the loop and frees everything.
pub(super) struct Capture {
    api: Arc<Api>,
    tloop: Ptr,
    context: Ptr,
    core: Ptr,
    stream: Ptr,
    _hook: Box<SpaHook>,
    _events: Box<StreamEvents>,
    shared: *mut Shared,
}

// SAFETY: the raw pointers are only touched with the thread loop locked or
// after it stopped (in `drop`).
unsafe impl Send for Capture {}

static INIT: Once = Once::new();

unsafe extern "C" fn on_state(data: Ptr, _old: c_int, state: c_int, _err: *const c_char) {
    // SAFETY: `data` is the `Shared` registered with the listener.
    let s = unsafe { &mut *(data as *mut Shared) };
    if state == STREAM_STATE_STREAMING {
        s.streaming = true;
    }
    let lost = state == STREAM_STATE_ERROR || (state == STREAM_STATE_UNCONNECTED && s.streaming);
    if lost && !s.ended {
        s.ended = true;
        tracing::info!(state, "pipewire stream ended");
        s.handler.ended("capture_failed");
    }
}

unsafe extern "C" fn on_param(data: Ptr, id: u32, param: *const c_void) {
    if id != pod::PARAM_FORMAT || param.is_null() {
        return;
    }
    // SAFETY: `data` is our `Shared`; `param` is a valid pod whose header
    // gives its body size.
    unsafe {
        let s = &mut *(data as *mut Shared);
        let size = *(param as *const u32) as usize + 8;
        let bytes = std::slice::from_raw_parts(param as *const u8, size.min(64 * 1024));
        if let Some(f) = pod::parse_format(bytes) {
            tracing::info!(
                format = f.format,
                width = f.width,
                height = f.height,
                "pipewire format"
            );
            s.format = f;
        }
    }
}

unsafe extern "C" fn on_process(data: Ptr) {
    // SAFETY: `data` is our `Shared`; buffers come from this stream and go
    // back to it; the mapped memory is valid until the buffer is queued.
    unsafe {
        let s = &mut *(data as *mut Shared);
        let b = (s.api.pw_stream_dequeue_buffer)(s.stream);
        if b.is_null() {
            return;
        }
        let buf = (*b).buffer;
        if !buf.is_null() && (*buf).n_datas > 0 && !(*buf).datas.is_null() && s.handler.wants() {
            let d = &*(*buf).datas;
            let f = s.format;
            if !d.data.is_null() && !d.chunk.is_null() && f.width > 0 && f.height > 0 {
                let c = &*d.chunk;
                let max = d.maxsize as usize;
                let offset = (c.offset as usize).min(max);
                let size = (c.size as usize).min(max - offset);
                if size > 0 && c.flags & CHUNK_FLAG_CORRUPTED == 0 {
                    let stride = if c.stride > 0 {
                        c.stride as usize
                    } else {
                        f.width as usize * 4
                    };
                    let src = std::slice::from_raw_parts((d.data as *const u8).add(offset), size);
                    let swap = matches!(f.format, pod::VIDEO_RGBX | pod::VIDEO_RGBA);
                    if let Some(frame) = copy_rows(src, f.width, f.height, stride, swap) {
                        s.handler.frame(frame);
                    }
                }
            }
        }
        (s.api.pw_stream_queue_buffer)(s.stream, b);
    }
}

impl Capture {
    /// Connect to the portal's PipeWire remote `fd` and stream node `node`.
    pub(super) fn start(
        fd: OwnedFd,
        node: u32,
        fps: u32,
        _size: Option<(u32, u32)>,
        handler: Arc<dyn FrameHandler>,
    ) -> Result<Self> {
        let api = Arc::new(Api::load().map_err(|e| {
            tracing::warn!(error = %e, "libpipewire not available");
            refused("capture_failed")
        })?);
        // SAFETY: the calls follow pipewire's documented order; every
        // pointer is checked before use and freed in `drop`.
        unsafe {
            INIT.call_once(|| (api.pw_init)(std::ptr::null_mut(), std::ptr::null_mut()));
            let tloop = (api.pw_thread_loop_new)(c"urdevice-screen".as_ptr(), std::ptr::null());
            if tloop.is_null() {
                return Err(refused("capture_failed"));
            }
            let context = (api.pw_context_new)(
                (api.pw_thread_loop_get_loop)(tloop),
                std::ptr::null_mut(),
                0,
            );
            if context.is_null() {
                (api.pw_thread_loop_destroy)(tloop);
                return Err(refused("capture_failed"));
            }
            if (api.pw_thread_loop_start)(tloop) < 0 {
                (api.pw_context_destroy)(context);
                (api.pw_thread_loop_destroy)(tloop);
                return Err(refused("capture_failed"));
            }
            (api.pw_thread_loop_lock)(tloop);
            let mut cap = Self {
                api: api.clone(),
                tloop,
                context,
                core: std::ptr::null_mut(),
                stream: std::ptr::null_mut(),
                _hook: Box::new(SpaHook([0; 6])),
                _events: Box::new(StreamEvents {
                    version: 2,
                    destroy: None,
                    state_changed: Some(on_state),
                    control_info: None,
                    io_changed: None,
                    param_changed: Some(on_param),
                    add_buffer: None,
                    remove_buffer: None,
                    process: Some(on_process),
                    drained: None,
                    command: None,
                    trigger_done: None,
                }),
                shared: std::ptr::null_mut(),
            };
            let fail = |cap: Self| {
                (cap.api.pw_thread_loop_unlock)(cap.tloop);
                drop(cap);
                Err(refused("capture_failed"))
            };
            // PipeWire owns the socket from here and closes it.
            cap.core =
                (api.pw_context_connect_fd)(context, fd.into_raw_fd(), std::ptr::null_mut(), 0);
            if cap.core.is_null() {
                return fail(cap);
            }
            let props = (api.pw_properties_new_string)(
                c"media.type=Video media.category=Capture media.role=Screen".as_ptr(),
            );
            cap.stream = (api.pw_stream_new)(cap.core, c"urdevice-screen".as_ptr(), props);
            if cap.stream.is_null() {
                return fail(cap);
            }
            cap.shared = Box::into_raw(Box::new(Shared {
                api: api.clone(),
                stream: cap.stream,
                handler,
                format: pod::VideoFormat::default(),
                streaming: false,
                ended: false,
            }));
            let hook: *mut SpaHook = &mut *cap._hook;
            (api.pw_stream_add_listener)(cap.stream, hook, &*cap._events, cap.shared as Ptr);
            let params = pod::enum_format(fps);
            let mut list = [params.as_ptr() as *const c_void];
            let r = (api.pw_stream_connect)(
                cap.stream,
                DIRECTION_INPUT,
                node,
                FLAG_AUTOCONNECT | FLAG_MAP_BUFFERS,
                list.as_mut_ptr(),
                1,
            );
            if r < 0 {
                return fail(cap);
            }
            (api.pw_thread_loop_unlock)(tloop);
            Ok(cap)
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: the loop is stopped first, so no callback runs while the
        // stream, core, and shared state are freed.
        unsafe {
            (self.api.pw_thread_loop_stop)(self.tloop);
            if !self.stream.is_null() {
                (self.api.pw_stream_disconnect)(self.stream);
                (self.api.pw_stream_destroy)(self.stream);
            }
            if !self.core.is_null() {
                (self.api.pw_core_disconnect)(self.core);
            }
            (self.api.pw_context_destroy)(self.context);
            (self.api.pw_thread_loop_destroy)(self.tloop);
            if !self.shared.is_null() {
                drop(Box::from_raw(self.shared));
            }
        }
    }
}
