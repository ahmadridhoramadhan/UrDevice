//! Pixel conversion from captured BGRA to the 4:2:0 layouts encoders take.
//!
//! Capture back-ends hand over 8-bit BGRA (or BGRx) rows with a stride. The
//! encoders want I420 (`openh264`) or NV12 (Media Foundation). Both
//! conversions scale in the same pass, sampling the nearest source pixel, so
//! a 4K display becomes 1080p without a second copy. BT.601 limited range,
//! which every H.264 decoder assumes when the stream carries no colour info.

use crate::MediaError;

/// One captured frame: BGRA, 4 bytes per pixel, rows `stride` bytes apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BgraFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Bytes from one row to the next, at least `width * 4`.
    pub stride: usize,
    /// The pixels.
    pub data: Vec<u8>,
}

impl BgraFrame {
    /// Check that the buffer holds `height` rows of `stride` bytes and that
    /// `stride` fits a row.
    pub fn validate(&self) -> Result<(), MediaError> {
        let row = self.width as usize * 4;
        let need = self
            .stride
            .checked_mul(self.height.saturating_sub(1) as usize)
            .and_then(|n| n.checked_add(row));
        match need {
            Some(n) if self.width > 0 && self.height > 0 && self.stride >= row && self.data.len() >= n => {
                Ok(())
            }
            _ => Err(MediaError::BadFrame),
        }
    }
}

#[inline]
fn luma(b: u8, g: u8, r: u8) -> u8 {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

#[inline]
fn chroma(b: i32, g: i32, r: i32) -> (u8, u8) {
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// Source column for each destination column (nearest sample).
fn map(dst: u32, src: u32) -> Vec<usize> {
    (0..dst as usize)
        .map(|x| ((x as u64 * src as u64 + src as u64 / 2) / dst as u64).min(src as u64 - 1) as usize)
        .collect()
}

/// Visit the frame scaled to `dw` x `dh`: `y_out(x, y, luma)` per pixel and
/// `uv_out(x/2, y/2, u, v)` per 2x2 block (the average of its four pixels).
fn convert(
    f: &BgraFrame,
    dw: u32,
    dh: u32,
    mut y_out: impl FnMut(usize, usize, u8),
    mut uv_out: impl FnMut(usize, usize, u8, u8),
) -> Result<(), MediaError> {
    f.validate()?;
    if dw < 2 || dh < 2 || !dw.is_multiple_of(2) || !dh.is_multiple_of(2) {
        return Err(MediaError::BadSize(dw, dh));
    }
    let xs = map(dw, f.width);
    let ys = map(dh, f.height);
    let px = |x: usize, y: usize| {
        let i = ys[y] * f.stride + xs[x] * 4;
        (f.data[i], f.data[i + 1], f.data[i + 2])
    };
    for y in (0..dh as usize).step_by(2) {
        for x in (0..dw as usize).step_by(2) {
            let (mut sb, mut sg, mut sr) = (0i32, 0i32, 0i32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (b, g, r) = px(x + dx, y + dy);
                y_out(x + dx, y + dy, luma(b, g, r));
                sb += b as i32;
                sg += g as i32;
                sr += r as i32;
            }
            let (u, v) = chroma(sb / 4, sg / 4, sr / 4);
            uv_out(x / 2, y / 2, u, v);
        }
    }
    Ok(())
}

/// BGRA to I420 (Y plane, then U, then V), scaled to `dw` x `dh` (even).
/// `out` is resized to `dw * dh * 3 / 2`.
pub fn bgra_to_i420(f: &BgraFrame, dw: u32, dh: u32, out: &mut Vec<u8>) -> Result<(), MediaError> {
    let (w, h) = (dw as usize, dh as usize);
    out.resize(w * h * 3 / 2, 0);
    let (yp, rest) = out.split_at_mut(w * h);
    let (up, vp) = rest.split_at_mut(w * h / 4);
    convert(
        f,
        dw,
        dh,
        |x, y, l| yp[y * w + x] = l,
        |x, y, u, v| {
            up[y * (w / 2) + x] = u;
            vp[y * (w / 2) + x] = v;
        },
    )
}

/// BGRA to NV12 (Y plane, then interleaved UV), scaled to `dw` x `dh`
/// (even). `out` is resized to `dw * dh * 3 / 2`.
pub fn bgra_to_nv12(f: &BgraFrame, dw: u32, dh: u32, out: &mut Vec<u8>) -> Result<(), MediaError> {
    let (w, h) = (dw as usize, dh as usize);
    out.resize(w * h * 3 / 2, 0);
    let (yp, uv) = out.split_at_mut(w * h);
    convert(
        f,
        dw,
        dh,
        |x, y, l| yp[y * w + x] = l,
        |x, y, u, v| {
            uv[y * w + x * 2] = u;
            uv[y * w + x * 2 + 1] = v;
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4], pad: usize) -> BgraFrame {
        let stride = w as usize * 4 + pad;
        let mut data = vec![0u8; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                data[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&bgra);
            }
        }
        BgraFrame {
            width: w,
            height: h,
            stride,
            data,
        }
    }

    #[test]
    fn colours_match_bt601() {
        let mut out = Vec::new();
        bgra_to_i420(&solid(4, 4, [0, 0, 0, 255], 0), 4, 4, &mut out).unwrap();
        assert_eq!((out[0], out[16], out[20]), (16, 128, 128));
        bgra_to_i420(&solid(4, 4, [255, 255, 255, 255], 0), 4, 4, &mut out).unwrap();
        assert_eq!(out[0], 235);
        // Pure red: Y 82, U 90, V 240.
        bgra_to_nv12(&solid(4, 4, [0, 0, 255, 255], 8), 4, 4, &mut out).unwrap();
        assert_eq!(out.len(), 24);
        assert_eq!((out[0], out[16], out[17]), (82, 90, 240));
    }

    #[test]
    fn scales_down_and_checks_sizes() {
        let mut out = Vec::new();
        bgra_to_i420(&solid(1920, 1080, [10, 20, 30, 255], 64), 1280, 720, &mut out).unwrap();
        assert_eq!(out.len(), 1280 * 720 * 3 / 2);
        assert!(bgra_to_i420(&solid(4, 4, [0; 4], 0), 3, 4, &mut out).is_err());
        let mut short = solid(4, 4, [0; 4], 0);
        short.data.truncate(10);
        assert!(bgra_to_nv12(&short, 4, 4, &mut out).is_err());
        let mut narrow = solid(4, 4, [0; 4], 0);
        narrow.stride = 8;
        assert!(bgra_to_nv12(&narrow, 4, 4, &mut out).is_err());
    }
}
