//! Platform traits (design §9).
//!
//! The core implements every feature against these traits. Desktop adapters in
//! `urdevice-platform` implement them per OS; Android implements them in Kotlin
//! through UniFFI callback interfaces.
//!
//! The data types here are placeholders sized for project setup. Each feature
//! phase in `docs/spec/tasks.md` fills in the fields it needs.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::Result;

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A boxed, sendable stream.
pub type BoxStream<'a, T> = Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'a>>;

// ---------------------------------------------------------------------------
// Clipboard (R7, design §9.1)
// ---------------------------------------------------------------------------

/// Clipboard content in one format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipContent {
    /// MIME type, for example `text/plain;charset=utf-8` or `image/png`.
    pub mime: String,
    /// Raw bytes in that format.
    pub bytes: Vec<u8>,
}

/// A change seen by the clipboard watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipEvent {
    /// The new clipboard content.
    pub content: ClipContent,
    /// True when the source app marked the content as sensitive (R7.3).
    pub sensitive: bool,
}

/// Read, write, and watch the system clipboard.
pub trait Clipboard: Send + Sync {
    /// Read the current clipboard content.
    fn read(&self) -> Result<ClipContent>;
    /// Replace the clipboard content.
    fn write(&self, content: ClipContent) -> Result<()>;
    /// Stream of clipboard changes.
    fn watch(&self) -> BoxStream<'static, ClipEvent>;
}

// ---------------------------------------------------------------------------
// Remote input (R9, R10, design §9.3)
// ---------------------------------------------------------------------------

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Middle button or wheel click.
    Middle,
}

/// A pointer event to inject.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvent {
    /// Relative movement in pixels.
    Move {
        /// Horizontal delta.
        dx: f32,
        /// Vertical delta.
        dy: f32,
    },
    /// Absolute position in normalized display coordinates (0.0 to 1.0).
    Abs {
        /// Horizontal position.
        x: f32,
        /// Vertical position.
        y: f32,
        /// Target display.
        display: DisplayId,
    },
    /// Button press or release.
    Button {
        /// Which button.
        button: MouseButton,
        /// True for press, false for release.
        down: bool,
    },
    /// Scroll by the given amount.
    Scroll {
        /// Horizontal scroll.
        dx: f32,
        /// Vertical scroll.
        dy: f32,
    },
}

/// Modifier keys held during a key event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Shift key.
    pub shift: bool,
    /// Control key.
    pub ctrl: bool,
    /// Alt or Option key.
    pub alt: bool,
    /// Windows, Command, or Super key.
    pub meta: bool,
}

/// A key event. `key` is a USB HID usage code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// USB HID usage code.
    pub key: u16,
    /// True for press, false for release.
    pub down: bool,
    /// Modifiers held at the time of the event.
    pub modifiers: Modifiers,
}

/// Whether the OS lets UrDevice inject input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermStatus {
    /// Injection is allowed.
    Granted,
    /// The user has not decided yet.
    NotDetermined,
    /// The user or the OS refused.
    Denied,
    /// This platform cannot inject input at all.
    Unsupported,
}

/// Inject pointer, key, and text input into the local device.
pub trait InputSink: Send + Sync {
    /// Inject a pointer event.
    fn pointer(&self, event: PointerEvent) -> Result<()>;
    /// Inject a key event.
    fn key(&self, event: KeyEvent) -> Result<()>;
    /// Type Unicode text as-is (R10.4).
    fn text(&self, text: &str) -> Result<()>;
    /// Report whether injection is allowed.
    fn permission_status(&self) -> PermStatus;
}

// ---------------------------------------------------------------------------
// Screen sharing (R12, design §10)
// ---------------------------------------------------------------------------

/// Identifies one display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayId(pub u32);

/// Information about one display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    /// Display identifier.
    pub id: DisplayId,
    /// Human-readable name.
    pub name: String,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// True for the primary display.
    pub primary: bool,
}

/// Capture settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureCfg {
    /// Target frames per second.
    pub fps: u32,
    /// Maximum output width; the source is scaled down to fit.
    pub max_width: u32,
}

/// One captured frame. The pixel format is decided in the media phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame data.
    pub data: Vec<u8>,
}

/// Stream of captured frames.
pub type FrameStream = BoxStream<'static, Frame>;

/// List and capture displays.
pub trait ScreenSource: Send + Sync {
    /// Displays that can be captured.
    fn displays(&self) -> Vec<DisplayInfo>;
    /// Start capturing one display.
    fn start(&self, display: DisplayId, cfg: CaptureCfg) -> Result<FrameStream>;
}

// ---------------------------------------------------------------------------
// Media control (R11, design §9.4)
// ---------------------------------------------------------------------------

/// Playback state of the current media session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    /// Media is playing.
    Playing,
    /// Media is paused.
    Paused,
    /// Nothing is playing.
    Stopped,
}

/// What is playing now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    /// Track title.
    pub title: String,
    /// Artist name.
    pub artist: String,
    /// Playback state.
    pub state: PlaybackState,
}

/// A media command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaCmd {
    /// Toggle play and pause.
    PlayPause,
    /// Next track.
    Next,
    /// Previous track.
    Prev,
    /// Set volume, 0.0 to 1.0.
    VolSet(f32),
    /// Change volume by a number of steps.
    VolStep(i8),
    /// Toggle mute.
    Mute,
}

/// Read and control media playback.
pub trait MediaHost: Send + Sync {
    /// The current media session, if any.
    fn now_playing(&self) -> Option<NowPlaying>;
    /// Send a media command.
    fn command(&self, cmd: MediaCmd) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Presenter overlay (R9b, design §9.3)
// ---------------------------------------------------------------------------

/// Draw the presenter laser pointer.
pub trait Overlay: Send + Sync {
    /// Show the laser at a normalized position, or hide it with `None`.
    fn laser(&self, pos: Option<(f32, f32)>) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Files and gallery (R8, design §9.2)
// ---------------------------------------------------------------------------

/// Read access to a photo gallery. Only Android provides one.
pub trait Gallery: Send + Sync {
    /// Number of items in the gallery.
    fn len(&self) -> Result<usize>;
    /// True when the gallery has no items.
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// Where received files go, and the local gallery.
pub trait FileStore: Send + Sync {
    /// Folder for incoming files.
    fn incoming_dir(&self) -> PathBuf;
    /// The local gallery, if this platform has one.
    fn gallery(&self) -> Option<Box<dyn Gallery>>;
}

// ---------------------------------------------------------------------------
// UI callbacks (R5)
// ---------------------------------------------------------------------------

/// A question the core asks the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPrompt {
    /// Name of the device asking.
    pub device_name: String,
    /// What it wants to do, for example `remote input`.
    pub capability: String,
}

/// The user's answer to a [`PermissionPrompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Allow this time only.
    AllowOnce,
    /// Allow and remember.
    AllowAlways,
    /// Refuse.
    Deny,
}

/// A message to show the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Short title.
    pub title: String,
    /// Body text.
    pub body: String,
}

/// Ask the user and show notices.
pub trait UiDelegate: Send + Sync {
    /// Ask the user to decide on a permission.
    fn ask(&self, req: PermissionPrompt) -> BoxFuture<'static, Decision>;
    /// Show a notice.
    fn notify(&self, notice: Notice);
}
