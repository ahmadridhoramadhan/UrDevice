//! Format helpers shared by the clipboard back-ends. Pure functions, so they
//! are tested on every OS even when only one back-end builds there.

// Each back-end uses a different subset.
#![allow(dead_code)]

use urdevice_core::services::clipboard::ClipData;

/// Everything a back-end read from the clipboard at one moment.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    /// Plain text, UTF-8.
    pub text: Option<String>,
    /// Rich text as an HTML document or fragment.
    pub html: Option<String>,
    /// A PNG image.
    pub png: Option<Vec<u8>>,
}

impl Snapshot {
    /// What to send: HTML with its plain-text version when there is HTML,
    /// otherwise text, otherwise the image. An image wins over HTML only
    /// when there is no plain text next to it (copying an image in a browser
    /// puts an `<img>` tag in the HTML slot).
    pub(crate) fn into_clip(self) -> Option<ClipData> {
        let Snapshot { text, html, png } = self;
        let text = text.filter(|t| !t.is_empty());
        let html = html.filter(|h| !h.trim().is_empty());
        if let (Some(png), None) = (&png, &text) {
            return Some(ClipData::png(png.clone()));
        }
        if let Some(html) = html {
            return Some(ClipData::html(html, text));
        }
        text.map(ClipData::text)
    }
}

/// Decode the bytes of a `text/html` clipboard item. Most apps use UTF-8;
/// some (Firefox on X11, Windows apps) use UTF-16 with or without a BOM.
pub(crate) fn html_from_bytes(b: &[u8]) -> Option<String> {
    if let Some(rest) = b.strip_prefix(&[0xff, 0xfe]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = b.strip_prefix(&[0xfe, 0xff]) {
        return utf16(rest, u16::from_be_bytes);
    }
    if let Some(rest) = b.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(rest.to_vec()).ok();
    }
    // UTF-16LE of ASCII-heavy markup has a NUL in every second byte.
    if b.len() >= 4 && b.len().is_multiple_of(2) && b[1] == 0 && b[3] == 0 && b[0] != 0 {
        return utf16(b, u16::from_le_bytes);
    }
    String::from_utf8(b.to_vec()).ok()
}

fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> Option<String> {
    if !b.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<u16> = b.chunks_exact(2).map(|c| f([c[0], c[1]])).collect();
    String::from_utf16(&units).ok()
}

/// Latin-1 bytes (the X11 `STRING` target) to text.
pub(crate) fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

// ---------------------------------------------------------------------------
// Windows "HTML Format" (CF_HTML)
// ---------------------------------------------------------------------------

/// The HTML fragment inside a Windows `HTML Format` item, or `None` when the
/// header is malformed. Offsets in the header are byte offsets into `raw`
/// and are hostile input: they are range-checked.
pub(crate) fn cf_html_extract(raw: &[u8]) -> Option<String> {
    // Only look for fields in the header.
    let head = &raw[..raw.len().min(1024)];
    let find = |name: &str| -> Option<usize> {
        let key = name.as_bytes();
        let pos = head.windows(key.len()).position(|w| w == key)?;
        let digits: Vec<u8> = head[pos + key.len()..]
            .iter()
            .copied()
            .take_while(u8::is_ascii_digit)
            .collect();
        std::str::from_utf8(&digits).ok()?.parse().ok()
    };
    let (start, end) = (find("StartFragment:")?, find("EndFragment:")?);
    if start > end || end > raw.len() {
        return None;
    }
    String::from_utf8(raw[start..end].to_vec()).ok()
}

/// Wrap an HTML fragment as a Windows `HTML Format` item.
pub(crate) fn cf_html_wrap(fragment: &str) -> Vec<u8> {
    const PRE: &str = "<html><body>\r\n<!--StartFragment-->";
    const POST: &str = "<!--EndFragment-->\r\n</body></html>";
    // The header has fixed-width numbers, so its length is known up front.
    let header = |s: usize, e: usize, hs: usize, he: usize| {
        format!(
            "Version:0.9\r\nStartHTML:{hs:010}\r\nEndHTML:{he:010}\r\nStartFragment:{s:010}\r\nEndFragment:{e:010}\r\n"
        )
    };
    let hlen = header(0, 0, 0, 0).len();
    let start_html = hlen;
    let start_frag = hlen + PRE.len();
    let end_frag = start_frag + fragment.len();
    let end_html = end_frag + POST.len();
    let mut out = header(start_frag, end_frag, start_html, end_html).into_bytes();
    out.extend_from_slice(PRE.as_bytes());
    out.extend_from_slice(fragment.as_bytes());
    out.extend_from_slice(POST.as_bytes());
    out
}

// ---------------------------------------------------------------------------
// Windows DIB <-> PNG
// ---------------------------------------------------------------------------

/// Largest image either direction converts, in pixels.
const MAX_PIXELS: u64 = 100_000_000;

/// Convert a `CF_DIB` item (a `BITMAPINFOHEADER` followed by pixels) to PNG.
/// Handles the uncompressed 24 and 32 bit forms that screenshots use.
pub(crate) fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |o: usize| {
        dib.get(o..o + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4])))
    };
    let header = u32_at(0)? as usize;
    if header < 40 || header > dib.len() {
        return None;
    }
    let w = i32::from_le_bytes(dib.get(4..8)?.try_into().ok()?);
    let h = i32::from_le_bytes(dib.get(8..12)?.try_into().ok()?);
    let bpp = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = u32_at(16)?;
    // BI_RGB (0) and BI_BITFIELDS (3) with the usual BGRA masks.
    if !(compression == 0 || compression == 3) || !(bpp == 24 || bpp == 32) || w <= 0 || h == 0 {
        return None;
    }
    let (w, top_down, h) = (w as u32, h < 0, h.unsigned_abs());
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return None;
    }
    let mut pixels = header;
    if compression == 3 && header == 40 {
        pixels += 12;
    }
    let bytes_pp = usize::from(bpp / 8);
    let stride = (w as usize * bytes_pp).checked_add(3)? / 4 * 4;
    let need = stride.checked_mul(h as usize)?;
    let data = dib.get(pixels..pixels.checked_add(need)?)?;
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    // A 32-bit BI_RGB bitmap often has an unused alpha byte of zero. Treat a
    // bitmap whose alpha is all zero as opaque.
    let alpha_used = bytes_pp == 4
        && data
            .chunks_exact(stride)
            .any(|row| row.chunks_exact(4).take(w as usize).any(|p| p[3] != 0));
    for y in 0..h as usize {
        let src_y = if top_down { y } else { h as usize - 1 - y };
        let row = &data[src_y * stride..src_y * stride + w as usize * bytes_pp];
        for px in row.chunks_exact(bytes_pp) {
            let a = if bytes_pp == 4 && alpha_used {
                px[3]
            } else {
                255
            };
            rgba.extend_from_slice(&[px[2], px[1], px[0], a]);
        }
    }
    encode_png(w, h, &rgba)
}

/// Convert a PNG to the bytes of a `CF_DIB` item: a 32-bit top-down
/// `BITMAPINFOHEADER` with straight alpha. `None` when it is not a PNG the
/// `png` decoder accepts, or is too big.
pub(crate) fn png_to_dib(png_bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let info = reader.info();
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (frame.width, frame.height);
    let channels = frame.color_type.samples();
    let src = &buf[..frame.buffer_size()];
    let mut out = Vec::with_capacity(40 + w as usize * h as usize * 4);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(w * h * 4).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for px in src.chunks_exact(channels) {
        let (r, g, b, a) = match channels {
            1 => (px[0], px[0], px[0], 255),
            2 => (px[0], px[0], px[0], px[1]),
            3 => (px[0], px[1], px[2], 255),
            _ => (px[0], px[1], px[2], px[3]),
        };
        out.extend_from_slice(&[b, g, r, a]);
    }
    Some(out)
}

/// Encode 8-bit RGBA as PNG.
pub(crate) fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(rgba).ok()?;
    writer.finish().ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Option<String> {
        Some(s.to_owned())
    }

    #[test]
    fn plain_text_alone() {
        let c = Snapshot {
            text: text("hi"),
            ..Default::default()
        }
        .into_clip()
        .unwrap();
        assert_eq!(c, ClipData::text("hi"));
    }

    #[test]
    fn html_keeps_its_plain_text() {
        let c = Snapshot {
            text: text("hi"),
            html: text("<b>hi</b>"),
            png: Some(vec![1]),
        }
        .into_clip()
        .unwrap();
        assert_eq!(c, ClipData::html("<b>hi</b>", text("hi")));
    }

    #[test]
    fn an_image_beats_the_img_tag_html_but_not_text() {
        let img = Snapshot {
            html: text("<img src=x>"),
            png: Some(vec![1, 2]),
            ..Default::default()
        }
        .into_clip()
        .unwrap();
        assert_eq!(img, ClipData::png(vec![1, 2]));
    }

    #[test]
    fn empty_is_nothing() {
        assert!(Snapshot::default().into_clip().is_none());
        assert!(
            Snapshot {
                text: text(""),
                html: text("  "),
                ..Default::default()
            }
            .into_clip()
            .is_none()
        );
    }

    #[test]
    fn html_encodings() {
        assert_eq!(html_from_bytes(b"<p>x</p>").unwrap(), "<p>x</p>");
        let mut le = vec![0xff, 0xfe];
        le.extend("<p>é</p>".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(html_from_bytes(&le).unwrap(), "<p>é</p>");
        let no_bom: Vec<u8> = "<p>ab</p>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(html_from_bytes(&no_bom).unwrap(), "<p>ab</p>");
        assert!(html_from_bytes(&[0xff, 0xfe, 0x00]).is_none());
        assert!(html_from_bytes(&[0xc3, 0x28]).is_none());
    }

    #[test]
    fn cf_html_round_trip() {
        let wrapped = cf_html_wrap("<b>héllo</b>");
        assert_eq!(cf_html_extract(&wrapped).unwrap(), "<b>héllo</b>");
    }

    #[test]
    fn cf_html_rejects_bad_offsets() {
        let mut w = cf_html_wrap("abc");
        // Point EndFragment past the end.
        let s = String::from_utf8(w.clone())
            .unwrap()
            .replace("EndFragment:", "EndFragment:9");
        w = s.into_bytes();
        assert!(cf_html_extract(&w).is_none());
        assert!(cf_html_extract(b"nothing").is_none());
        assert!(cf_html_extract(b"").is_none());
        assert!(cf_html_extract(b"StartFragment:50\r\nEndFragment:10\r\n").is_none());
    }

    #[test]
    fn dib_and_png_convert_both_ways() {
        // 2x1 image: red, then half-transparent blue.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 128];
        let png = encode_png(2, 1, &rgba).unwrap();
        let dib = png_to_dib(&png).unwrap();
        assert_eq!(&dib[40..], &[0, 0, 255, 255, 255, 0, 0, 128]);
        let back = dib_to_png(&dib).unwrap();
        let mut d = png::Decoder::new(std::io::Cursor::new(back));
        d.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = d.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let f = r.next_frame(&mut buf).unwrap();
        assert_eq!(&buf[..f.buffer_size()], &rgba);
    }

    #[test]
    fn dib_24_bit_bottom_up() {
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&1i32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&24u16.to_le_bytes());
        dib.extend_from_slice(&[0; 24]);
        // Bottom row first, padded to 4 bytes: blue then red.
        dib.extend_from_slice(&[255, 0, 0, 0]);
        dib.extend_from_slice(&[0, 0, 255, 0]);
        assert!(dib_to_png(&dib).is_some());
    }

    #[test]
    fn hostile_dibs_are_none() {
        assert!(dib_to_png(&[]).is_none());
        assert!(dib_to_png(&[40, 0, 0, 0]).is_none());
        let mut dib = vec![0u8; 60];
        dib[0] = 40;
        dib[4..8].copy_from_slice(&100_000i32.to_le_bytes());
        dib[8..12].copy_from_slice(&100_000i32.to_le_bytes());
        dib[14] = 32;
        assert!(dib_to_png(&dib).is_none());
        assert!(png_to_dib(b"not a png").is_none());
    }
}
