//! Stub adapters shared by every OS module until the real ones land.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use urdevice_core::platform::{
    BoxStream, CaptureCfg, ClipContent, ClipEvent, Clipboard, DisplayId, DisplayInfo, FileStore,
    FrameStream, Gallery, InputSink, KeyEvent, MediaCmd, MediaHost, NowPlaying, Overlay,
    PermStatus, PointerEvent, ScreenSource,
};
use urdevice_core::{CoreError, Result};

use crate::Platform;

/// Build a [`Platform`] where every adapter is a stub tagged with `name`.
pub(crate) fn platform(name: &'static str) -> Platform {
    let stub = Arc::new(Stub { platform: name });
    Platform {
        name,
        clipboard: stub.clone(),
        input: stub.clone(),
        screen: stub.clone(),
        media: stub.clone(),
        overlay: stub.clone(),
        files: stub,
    }
}

#[derive(Debug)]
struct Stub {
    platform: &'static str,
}

impl Stub {
    fn unsupported<T>(&self, feature: &'static str) -> Result<T> {
        Err(CoreError::unsupported(feature, self.platform))
    }
}

/// A stream that ends at once.
struct Empty<T>(std::marker::PhantomData<fn() -> T>);

impl<T> futures_core::Stream for Empty<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<T>> {
        Poll::Ready(None)
    }
}

fn empty<T: 'static>() -> BoxStream<'static, T> {
    Box::pin(Empty(std::marker::PhantomData))
}

impl Clipboard for Stub {
    fn read(&self) -> Result<ClipContent> {
        self.unsupported("clipboard")
    }

    fn write(&self, _content: ClipContent) -> Result<()> {
        self.unsupported("clipboard")
    }

    fn watch(&self) -> BoxStream<'static, ClipEvent> {
        empty()
    }
}

impl InputSink for Stub {
    fn pointer(&self, _event: PointerEvent) -> Result<()> {
        self.unsupported("remote input")
    }

    fn key(&self, _event: KeyEvent) -> Result<()> {
        self.unsupported("remote input")
    }

    fn text(&self, _text: &str) -> Result<()> {
        self.unsupported("remote input")
    }

    fn permission_status(&self) -> PermStatus {
        PermStatus::Unsupported
    }
}

impl ScreenSource for Stub {
    fn displays(&self) -> Vec<DisplayInfo> {
        Vec::new()
    }

    fn start(&self, _display: DisplayId, _cfg: CaptureCfg) -> Result<FrameStream> {
        self.unsupported("screen capture")
    }
}

impl MediaHost for Stub {
    fn now_playing(&self) -> Option<NowPlaying> {
        None
    }

    fn command(&self, _cmd: MediaCmd) -> Result<()> {
        self.unsupported("media control")
    }
}

impl Overlay for Stub {
    fn laser(&self, _pos: Option<(f32, f32)>) -> Result<()> {
        self.unsupported("presenter overlay")
    }
}

impl FileStore for Stub {
    fn incoming_dir(&self) -> PathBuf {
        default_incoming_dir()
    }

    fn gallery(&self) -> Option<Box<dyn Gallery>> {
        None
    }
}

/// `<home>/Downloads/UrDevice`, or a temp folder when no home is set.
fn default_incoming_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home {
        Some(home) => PathBuf::from(home).join("Downloads").join("UrDevice"),
        None => std::env::temp_dir().join("UrDevice"),
    }
}
