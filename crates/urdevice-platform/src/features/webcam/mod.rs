//! Phone as webcam and microphone (#28): desktop OS adapters.
//!
//! **Virtual camera (T11.4, R17.2).** The desktop decodes the phone's picture, turns it to
//! the angle the user chose, and hands it to a [`VirtualCamera`], which other apps then pick as
//! a webcam. Linux writes the picture to a `v4l2loopback` device ([`linux`]). Windows 11
//! (`MFCreateVirtualCamera` with a media source DLL) and macOS (a Camera Extension) need a
//! registered or signed component that is not built yet, so [`open_virtual_camera`] answers
//! [`VirtualCameraError::NotAvailable`] there and the camera shows in its window only (R17.3).
//!
//! **Microphone (T11.5, R17.4).** [`mic`] plays the phone's microphone into a PipeWire
//! virtual source on Linux.
//!
//! Pictures are never stored or logged.

pub mod mic;

#[cfg(target_os = "linux")]
mod linux;

/// Width of the picture the virtual camera offers. Apps see one fixed size; smaller or turned
/// pictures are fitted into it with black bars.
pub const OUT_WIDTH: u32 = 1280;

/// Height of the picture the virtual camera offers.
pub const OUT_HEIGHT: u32 = 720;

/// Clockwise turn of the picture, so a phone held any way up looks upright.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    /// As the camera delivers it.
    #[default]
    None,
    /// A quarter turn clockwise.
    Cw90,
    /// Upside down.
    Cw180,
    /// A quarter turn counter-clockwise.
    Cw270,
}

impl Rotation {
    /// The next quarter turn clockwise.
    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Cw90,
            Self::Cw90 => Self::Cw180,
            Self::Cw180 => Self::Cw270,
            Self::Cw270 => Self::None,
        }
    }

    /// 0, 90, 180, or 270.
    pub fn degrees(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }

    /// From 0, 90, 180, or 270 (anything else is no turn).
    pub fn from_degrees(d: u32) -> Self {
        match d {
            90 => Self::Cw90,
            180 => Self::Cw180,
            270 => Self::Cw270,
            _ => Self::None,
        }
    }

    /// True when width and height swap.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::Cw90 | Self::Cw270)
    }
}

/// An RGBA picture: 4 bytes per pixel, rows `width * 4` bytes apart.
fn valid_rgba(data: &[u8], width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            == Some(data.len())
}

/// Turn an RGBA picture. Returns the turned picture and its size, or `None` when `src` does not
/// hold `width` x `height` pixels.
pub fn rotate_rgba(
    src: &[u8],
    width: u32,
    height: u32,
    rotation: Rotation,
) -> Option<(Vec<u8>, u32, u32)> {
    if !valid_rgba(src, width, height) {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    if rotation == Rotation::None {
        return Some((src.to_vec(), width, height));
    }
    let (rw, rh) = if rotation.swaps_axes() {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; rw * rh * 4];
    for ry in 0..rh {
        for rx in 0..rw {
            let (sx, sy) = source_of(rx, ry, w, h, rotation);
            let s = (sy * w + sx) * 4;
            let d = (ry * rw + rx) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    Some((out, rw as u32, rh as u32))
}

/// The source pixel that lands on (`rx`, `ry`) after the turn.
#[inline]
fn source_of(rx: usize, ry: usize, w: usize, h: usize, rotation: Rotation) -> (usize, usize) {
    match rotation {
        Rotation::None => (rx, ry),
        Rotation::Cw90 => (ry, h - 1 - rx),
        Rotation::Cw180 => (w - 1 - rx, h - 1 - ry),
        Rotation::Cw270 => (w - 1 - ry, rx),
    }
}

/// Convert an RGBA picture to YUYV (4:2:2, BT.601 limited range) of `out_width` x `out_height`,
/// scaled to fit with its shape kept (nearest pixel) and black bars around it. Returns `None`
/// when `src` does not hold `width` x `height` pixels or the output size is zero or odd in width.
pub fn rgba_to_yuyv_fit(
    src: &[u8],
    width: u32,
    height: u32,
    out_width: u32,
    out_height: u32,
) -> Option<Vec<u8>> {
    if !valid_rgba(src, width, height)
        || out_width == 0
        || out_height == 0
        || !out_width.is_multiple_of(2)
    {
        return None;
    }
    let (w, h) = (width as usize, height as usize);
    let (ow, oh) = (out_width as usize, out_height as usize);
    // The shown rectangle: the largest one of the picture's shape that fits.
    let scale = f64::min(ow as f64 / w as f64, oh as f64 / h as f64);
    let dw = (((w as f64 * scale) as usize).max(2)) & !1;
    let dh = ((h as f64 * scale) as usize).max(1);
    let (offx, offy) = (((ow - dw) / 2) & !1, (oh - dh) / 2);
    // Source column of every shown column, once.
    let cols: Vec<usize> = (0..dw).map(|x| (x * w / dw).min(w - 1)).collect();
    let mut out = vec![0u8; ow * oh * 2];
    // Black bars: Y 16, U and V 128.
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&[16, 128, 16, 128]);
    }
    for y in 0..dh {
        let sy = (y * h / dh).min(h - 1);
        let row = &src[sy * w * 4..(sy + 1) * w * 4];
        let o = ((offy + y) * ow + offx) * 2;
        for x in (0..dw).step_by(2) {
            let a = &row[cols[x] * 4..cols[x] * 4 + 3];
            let b = &row[cols[x + 1] * 4..cols[x + 1] * 4 + 3];
            let (r0, g0, b0) = (i32::from(a[0]), i32::from(a[1]), i32::from(a[2]));
            let (r1, g1, b1) = (i32::from(b[0]), i32::from(b[1]), i32::from(b[2]));
            let y0 = ((66 * r0 + 129 * g0 + 25 * b0 + 128) >> 8) + 16;
            let y1 = ((66 * r1 + 129 * g1 + 25 * b1 + 128) >> 8) + 16;
            let (r, g, bl) = ((r0 + r1) / 2, (g0 + g1) / 2, (b0 + b1) / 2);
            let u = ((-38 * r - 74 * g + 112 * bl + 128) >> 8) + 128;
            let v = ((112 * r - 94 * g - 18 * bl + 128) >> 8) + 128;
            let d = o + x * 2;
            out[d] = y0.clamp(0, 255) as u8;
            out[d + 1] = u.clamp(0, 255) as u8;
            out[d + 2] = y1.clamp(0, 255) as u8;
            out[d + 3] = v.clamp(0, 255) as u8;
        }
    }
    Some(out)
}

/// Why no virtual camera could be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualCameraError {
    /// This OS has no virtual camera back-end yet (Windows, macOS).
    NotAvailable,
    /// No `v4l2loopback` device exists: the module is not loaded.
    NotSetUp,
    /// A device exists but could not be opened or set up (a permission or a busy device).
    Failed(String),
}

impl std::fmt::Display for VirtualCameraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => f.write_str("no virtual camera on this system"),
            Self::NotSetUp => f.write_str("no virtual camera device found"),
            Self::Failed(why) => write!(f, "the virtual camera failed: {why}"),
        }
    }
}

impl std::error::Error for VirtualCameraError {}

/// A camera other apps can pick. Dropping it removes the camera (or leaves it without a picture).
pub trait VirtualCamera: Send {
    /// The name call apps list this camera by (the card label of the device).
    fn name(&self) -> String;

    /// Show one RGBA picture of `width` x `height` pixels, already turned.
    fn write(&mut self, rgba: &[u8], width: u32, height: u32) -> std::io::Result<()>;
}

/// Open the virtual camera of this OS.
pub fn open_virtual_camera() -> Result<Box<dyn VirtualCamera>, VirtualCameraError> {
    #[cfg(target_os = "linux")]
    {
        linux::open()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(VirtualCameraError::NotAvailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 x 2 picture with a different red value in each pixel.
    fn pic() -> Vec<u8> {
        (0..6u8).flat_map(|i| [i * 10, 0, 0, 255]).collect()
    }

    fn reds(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4).map(|p| p[0] / 10).collect()
    }

    #[test]
    fn a_quarter_turn_clockwise_moves_the_top_left_to_the_top_right() {
        // 0 1 2      3 0
        // 3 4 5  ->  4 1
        //            5 2
        let (out, w, h) = rotate_rgba(&pic(), 3, 2, Rotation::Cw90).unwrap();
        assert_eq!((w, h), (2, 3));
        assert_eq!(reds(&out), [3, 0, 4, 1, 5, 2]);
    }

    #[test]
    fn turns_add_up_to_a_full_circle() {
        let (a, w, h) = rotate_rgba(&pic(), 3, 2, Rotation::Cw180).unwrap();
        assert_eq!(reds(&a), [5, 4, 3, 2, 1, 0]);
        let (b, w2, h2) = rotate_rgba(&a, w, h, Rotation::Cw180).unwrap();
        assert_eq!((w2, h2), (3, 2));
        assert_eq!(b, pic());
        let (c, w, h) = rotate_rgba(&pic(), 3, 2, Rotation::Cw270).unwrap();
        assert_eq!((w, h), (2, 3));
        assert_eq!(reds(&c), [2, 5, 1, 4, 0, 3]);
        assert_eq!(Rotation::Cw270.next(), Rotation::None);
        assert_eq!(
            Rotation::from_degrees(Rotation::Cw90.degrees()),
            Rotation::Cw90
        );
    }

    #[test]
    fn a_picture_of_the_wrong_size_is_refused() {
        assert!(rotate_rgba(&[0; 7], 3, 2, Rotation::None).is_none());
        assert!(rotate_rgba(&pic(), 0, 2, Rotation::None).is_none());
        assert!(rgba_to_yuyv_fit(&[0; 7], 3, 2, 8, 4).is_none());
        assert!(rgba_to_yuyv_fit(&pic(), 3, 2, 7, 4).is_none());
    }

    #[test]
    fn yuyv_has_the_output_size_and_black_bars_for_a_narrow_picture() {
        // A white 2 x 4 picture in an 8 x 4 box: 2 wide in the middle, black at both sides.
        let white = vec![255u8; 2 * 4 * 4];
        let out = rgba_to_yuyv_fit(&white, 2, 4, 8, 4).unwrap();
        assert_eq!(out.len(), 8 * 4 * 2);
        // First pixel pair of the first row is a bar: Y 16.
        assert_eq!(out[0], 16);
        // The middle pair (x 3 and 4 is not aligned; the picture starts at x 2) is white: Y 235.
        let mid = (2 * 2) as usize;
        assert_eq!(out[mid], 235);
        assert_eq!(out[mid + 1], 128);
    }

    #[test]
    fn a_picture_that_fills_the_box_has_no_bars() {
        let grey = vec![128u8; 4 * 2 * 4];
        let out = rgba_to_yuyv_fit(&grey, 4, 2, 4, 2).unwrap();
        assert!(out.chunks_exact(4).all(|p| p[0] > 100 && p[0] == p[2]));
    }
}
