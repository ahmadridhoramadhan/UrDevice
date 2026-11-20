//! The few SPA pods the PipeWire stream needs, built and read by hand (no
//! libspa headers at build time).
//!
//! A pod is a `u32` body size and a `u32` type, then the body, padded to 8
//! bytes. An object body is its object type and param id, then properties:
//! key, flags, and a pod each.

/// SPA types.
const T_ID: u32 = 3;
const T_INT: u32 = 4;
const T_RECTANGLE: u32 = 10;
const T_FRACTION: u32 = 11;
const T_OBJECT: u32 = 15;
const T_CHOICE: u32 = 19;

const OBJECT_FORMAT: u32 = 0x40003;
/// `SPA_PARAM_EnumFormat`.
pub(super) const PARAM_ENUM_FORMAT: u32 = 3;
/// `SPA_PARAM_Format`.
pub(super) const PARAM_FORMAT: u32 = 4;

const FORMAT_MEDIA_TYPE: u32 = 1;
const FORMAT_MEDIA_SUBTYPE: u32 = 2;
const FORMAT_VIDEO_FORMAT: u32 = 0x20001;
const FORMAT_VIDEO_SIZE: u32 = 0x20003;
const FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;

const MEDIA_TYPE_VIDEO: u32 = 2;
const MEDIA_SUBTYPE_RAW: u32 = 1;

const CHOICE_RANGE: u32 = 1;
const CHOICE_ENUM: u32 = 3;

/// `SPA_VIDEO_FORMAT_*` values this capture takes.
pub(super) const VIDEO_RGBX: u32 = 7;
pub(super) const VIDEO_BGRX: u32 = 8;
pub(super) const VIDEO_RGBA: u32 = 11;
pub(super) const VIDEO_BGRA: u32 = 12;

struct Builder(Vec<u8>);

impl Builder {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_ne_bytes());
    }

    fn pad(&mut self) {
        while !self.0.len().is_multiple_of(8) {
            self.0.push(0);
        }
    }

    /// A property whose value is a choice of `n` children of `child_type`,
    /// each `child_size` bytes, written by `values`.
    fn choice(&mut self, key: u32, kind: u32, child_type: u32, child_size: u32, values: &[u32]) {
        self.u32(key);
        self.u32(0);
        // Choice kind and flags, the child pod header, then the values.
        self.u32(16 + values.len() as u32 * 4);
        self.u32(T_CHOICE);
        self.u32(kind);
        self.u32(0);
        self.u32(child_size);
        self.u32(child_type);
        for v in values {
            self.u32(*v);
        }
        self.pad();
    }

    fn id(&mut self, key: u32, v: u32) {
        self.u32(key);
        self.u32(0);
        self.u32(4);
        self.u32(T_ID);
        self.u32(v);
        self.pad();
    }
}

/// The `EnumFormat` the stream offers: raw video in BGRx, BGRA, RGBx, or
/// RGBA, any size, up to `fps` frames per second. No DMA-BUF modifier, so the
/// compositor sends shared-memory buffers the CPU can read.
pub(super) fn enum_format(fps: u32) -> Vec<u64> {
    let mut b = Builder(Vec::with_capacity(256));
    b.u32(0); // body size, set below
    b.u32(T_OBJECT);
    b.u32(OBJECT_FORMAT);
    b.u32(PARAM_ENUM_FORMAT);
    b.id(FORMAT_MEDIA_TYPE, MEDIA_TYPE_VIDEO);
    b.id(FORMAT_MEDIA_SUBTYPE, MEDIA_SUBTYPE_RAW);
    b.choice(
        FORMAT_VIDEO_FORMAT,
        CHOICE_ENUM,
        T_ID,
        4,
        &[VIDEO_BGRX, VIDEO_BGRX, VIDEO_BGRA, VIDEO_RGBX, VIDEO_RGBA],
    );
    b.choice(
        FORMAT_VIDEO_SIZE,
        CHOICE_RANGE,
        T_RECTANGLE,
        8,
        &[1920, 1080, 1, 1, 8192, 8192],
    );
    b.choice(
        FORMAT_VIDEO_FRAMERATE,
        CHOICE_RANGE,
        T_FRACTION,
        8,
        &[fps, 1, 0, 1, fps.max(1), 1],
    );
    let body = (b.0.len() - 8) as u32;
    b.0[..4].copy_from_slice(&body.to_ne_bytes());
    // Copy into u64s so the pod is 8-byte aligned in memory.
    b.0.chunks(8)
        .map(|c| {
            let mut x = [0u8; 8];
            x[..c.len()].copy_from_slice(c);
            u64::from_ne_bytes(x)
        })
        .collect()
}

/// What the negotiated `Format` says.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct VideoFormat {
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

fn rd(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The first value of a property pod: the value itself, or the default of a
/// choice.
fn value(b: &[u8], at: usize, size: u32, ty: u32) -> Option<(u32, usize)> {
    if ty == T_CHOICE {
        let child_ty = rd(b, at + 12)?;
        let _ = size;
        Some((child_ty, at + 16))
    } else {
        Some((ty, at))
    }
}

/// Read a `Format` object pod of `len` bytes. Hostile-input safe: every read
/// is bounds-checked and the walk stops at the pod's end.
pub(super) fn parse_format(pod: &[u8]) -> Option<VideoFormat> {
    let body = rd(pod, 0)? as usize;
    if rd(pod, 4)? != T_OBJECT || pod.len() < 8 + body || body < 8 {
        return None;
    }
    let end = 8 + body;
    let mut at = 16; // header + object type + id
    let mut out = VideoFormat::default();
    while at + 16 <= end {
        let key = rd(pod, at)?;
        let size = rd(pod, at + 8)?;
        let ty = rd(pod, at + 12)?;
        let v = at + 16;
        if let Some((vty, vat)) = value(pod, v, size, ty) {
            match (key, vty) {
                (FORMAT_VIDEO_FORMAT, T_ID) | (FORMAT_VIDEO_FORMAT, T_INT) => out.format = rd(pod, vat)?,
                (FORMAT_VIDEO_SIZE, T_RECTANGLE) => {
                    out.width = rd(pod, vat)?;
                    out.height = rd(pod, vat + 4)?;
                }
                _ => {}
            }
        }
        let next = v.checked_add((size as usize).checked_add(7)? & !7)?;
        if next <= at {
            return None;
        }
        at = next;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(p: &[u64]) -> Vec<u8> {
        p.iter().flat_map(|x| x.to_ne_bytes()).collect()
    }

    #[test]
    fn enum_format_is_well_formed() {
        let p = bytes(&enum_format(30));
        let body = rd(&p, 0).unwrap() as usize;
        assert_eq!(p.len(), 8 + body);
        assert_eq!(rd(&p, 4), Some(T_OBJECT));
        // The parser walks it to the end and reads the defaults.
        let f = parse_format(&p).unwrap();
        assert_eq!(f, VideoFormat { format: VIDEO_BGRX, width: 1920, height: 1080 });
    }

    #[test]
    fn parses_a_fixed_format() {
        let mut b = Builder(Vec::new());
        b.u32(0);
        b.u32(T_OBJECT);
        b.u32(OBJECT_FORMAT);
        b.u32(PARAM_FORMAT);
        b.id(FORMAT_MEDIA_TYPE, MEDIA_TYPE_VIDEO);
        b.id(FORMAT_VIDEO_FORMAT, VIDEO_RGBA);
        b.u32(FORMAT_VIDEO_SIZE);
        b.u32(0);
        b.u32(8);
        b.u32(T_RECTANGLE);
        b.u32(2560);
        b.u32(1440);
        let body = (b.0.len() - 8) as u32;
        b.0[..4].copy_from_slice(&body.to_ne_bytes());
        assert_eq!(
            parse_format(&b.0),
            Some(VideoFormat { format: VIDEO_RGBA, width: 2560, height: 1440 })
        );
    }

    #[test]
    fn garbage_is_refused_without_panicking() {
        assert_eq!(parse_format(&[]), None);
        assert_eq!(parse_format(&[255; 7]), None);
        let mut p = bytes(&enum_format(30));
        p.truncate(40);
        let _ = parse_format(&p);
        for n in 0..200u32 {
            let junk: Vec<u8> = (0..n).map(|i| (i * 31 + n) as u8).collect();
            let _ = parse_format(&junk);
        }
    }
}
