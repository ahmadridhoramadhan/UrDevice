//! Album artwork for the media state: any PNG or JPEG becomes a PNG of at
//! most 256 px a side and 96 KiB (R11.2, design §9.4).
//!
//! The bytes come from another program on this device (a player's cover
//! file, a Windows thumbnail), so decoding has hard limits and a bad image
//! simply gives `None`.

// Used by the Linux and Windows adapters.
#![allow(dead_code)]

use std::io::Cursor;

use urdevice_core::services::media::MAX_ARTWORK;

/// Longest side of the PNG we send.
pub(crate) const MAX_SIDE: u32 = 256;

/// Largest input image we decode, per side.
const MAX_INPUT_SIDE: u32 = 8192;

/// Largest input file we read, in bytes.
pub(crate) const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

struct Rgba {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// Turn a PNG or JPEG into a small PNG, or `None` when it is neither, is too
/// large to decode, or cannot be made small enough.
pub(crate) fn to_small_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES {
        return None;
    }
    let img = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        decode_png(bytes)?
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes)?
    } else {
        return None;
    };
    let mut side = MAX_SIDE;
    loop {
        let small = fit(&img, side);
        let png = encode_png(&small)?;
        if png.len() <= MAX_ARTWORK {
            return Some(png);
        }
        side = side * 3 / 4;
        if side < 48 {
            return None;
        }
    }
}

fn decode_png(bytes: &[u8]) -> Option<Rgba> {
    let mut dec =
        png::Decoder::new_with_limits(Cursor::new(bytes), png::Limits { bytes: 256 << 20 });
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let info = reader.info();
    if info.width == 0
        || info.height == 0
        || info.width > MAX_INPUT_SIDE
        || info.height > MAX_INPUT_SIDE
    {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let f = reader.next_frame(&mut buf).ok()?;
    buf.truncate(f.buffer_size());
    let n = (f.width as usize).checked_mul(f.height as usize)?;
    let px = match f.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Indexed => return None,
    };
    (px.len() == n.checked_mul(4)?).then_some(Rgba {
        w: f.width,
        h: f.height,
        px,
    })
}

fn decode_jpeg(bytes: &[u8]) -> Option<Rgba> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGB)
        .set_max_width(MAX_INPUT_SIDE as usize)
        .set_max_height(MAX_INPUT_SIDE as usize);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(Cursor::new(bytes), options);
    let rgb = d.decode().ok()?;
    let (w, h) = d.dimensions()?;
    if w == 0 || h == 0 || rgb.len() != w.checked_mul(h)?.checked_mul(3)? {
        return None;
    }
    Some(Rgba {
        w: u32::try_from(w).ok()?,
        h: u32::try_from(h).ok()?,
        px: rgb
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
    })
}

/// Shrink `img` so neither side is over `side`, averaging the source pixels
/// that fall in each target pixel. A smaller image is returned unchanged.
fn fit(img: &Rgba, side: u32) -> Rgba {
    if img.w <= side && img.h <= side {
        return Rgba {
            w: img.w,
            h: img.h,
            px: img.px.clone(),
        };
    }
    let scale = f64::from(side) / f64::from(img.w.max(img.h));
    let tw = ((f64::from(img.w) * scale).round() as u32).max(1);
    let th = ((f64::from(img.h) * scale).round() as u32).max(1);
    let mut px = Vec::with_capacity(tw as usize * th as usize * 4);
    for ty in 0..th {
        let y0 = (u64::from(ty) * u64::from(img.h) / u64::from(th)) as u32;
        let y1 = ((u64::from(ty + 1) * u64::from(img.h) / u64::from(th)) as u32)
            .max(y0 + 1)
            .min(img.h);
        for tx in 0..tw {
            let x0 = (u64::from(tx) * u64::from(img.w) / u64::from(tw)) as u32;
            let x1 = ((u64::from(tx + 1) * u64::from(img.w) / u64::from(tw)) as u32)
                .max(x0 + 1)
                .min(img.w);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                let row = (y as usize * img.w as usize + x0 as usize) * 4;
                for c in img.px[row..row + (x1 - x0) as usize * 4].chunks_exact(4) {
                    for (s, v) in sum.iter_mut().zip(c) {
                        *s += u64::from(*v);
                    }
                }
            }
            let n = u64::from(y1 - y0) * u64::from(x1 - x0);
            px.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    Rgba { w: tw, h: th, px }
}

fn encode_png(img: &Rgba) -> Option<Vec<u8>> {
    let opaque = img.px.chunks_exact(4).all(|p| p[3] == 255);
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, img.w, img.h);
    enc.set_depth(png::BitDepth::Eight);
    if opaque {
        enc.set_color(png::ColorType::Rgb);
        let rgb: Vec<u8> = img
            .px
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        enc.write_header().ok()?.write_image_data(&rgb).ok()?;
    } else {
        enc.set_color(png::ColorType::Rgba);
        enc.write_header().ok()?.write_image_data(&img.px).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_png(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let img = Rgba {
            w,
            h,
            px: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    [
                        x as u8,
                        y as u8,
                        (x ^ y) as u8,
                        if alpha { (x % 200) as u8 } else { 255 },
                    ]
                })
                .collect(),
        };
        encode_png(&img).unwrap()
    }

    fn dims(png_bytes: &[u8]) -> (u32, u32) {
        let r = png::Decoder::new(Cursor::new(png_bytes))
            .read_info()
            .unwrap();
        (r.info().width, r.info().height)
    }

    #[test]
    fn large_png_is_shrunk_to_fit() {
        let big = gradient_png(1000, 600, false);
        let small = to_small_png(&big).unwrap();
        let (w, h) = dims(&small);
        assert_eq!(w, 256);
        assert!((150..=155).contains(&h), "keeps the aspect ratio: {h}");
        assert!(small.len() <= MAX_ARTWORK);
    }

    #[test]
    fn small_png_keeps_its_size_and_alpha() {
        let p = gradient_png(64, 64, true);
        let out = to_small_png(&p).unwrap();
        assert_eq!(dims(&out), (64, 64));
        let r = png::Decoder::new(Cursor::new(&out)).read_info().unwrap();
        assert_eq!(r.info().color_type, png::ColorType::Rgba);
    }

    #[test]
    fn rubbish_is_refused() {
        assert!(to_small_png(b"").is_none());
        assert!(to_small_png(b"GIF89a....").is_none());
        assert!(to_small_png(&[0x89, b'P', b'N', b'G', 1, 2, 3]).is_none());
        assert!(to_small_png(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).is_none());
        let huge = vec![0u8; MAX_INPUT_BYTES + 1];
        assert!(to_small_png(&huge).is_none());
    }

    #[test]
    fn truncated_png_does_not_panic() {
        let p = gradient_png(300, 300, false);
        for cut in [10, 40, p.len() / 2, p.len() - 1] {
            let _ = to_small_png(&p[..cut]);
        }
    }

    #[test]
    fn noisy_image_is_shrunk_until_it_fits() {
        // Pseudo-random pixels do not compress, so 256 px is over the budget.
        let (w, h) = (512u32, 512u32);
        let mut x = 0x1234_5678u32;
        let px: Vec<u8> = (0..w * h)
            .flat_map(|_| {
                let mut p = [0u8, 0, 0, 255];
                for c in p.iter_mut().take(3) {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    *c = (x >> 8) as u8;
                }
                p
            })
            .collect();
        let png = encode_png(&Rgba { w, h, px }).unwrap();
        let out = to_small_png(&png).unwrap();
        assert!(out.len() <= MAX_ARTWORK);
        assert!(dims(&out).0 < 256);
    }
}
