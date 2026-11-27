//! The `openh264` software encoder and decoder, built from source by
//! `openh264-sys2` (BSD-2-Clause).

use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod,
    RateControlMode, UsageType,
};
use openh264::formats::{YUVBuffer, YUVSource};

use crate::MediaError;
use crate::convert::{BgraFrame, bgra_to_i420};
use crate::encoder::{EncodedUnit, EncoderSettings, VideoEncoder};

/// `openh264` set up for screen content in real time.
pub struct SoftEncoder {
    enc: Encoder,
    settings: EncoderSettings,
}

impl SoftEncoder {
    /// Open with low-latency settings.
    pub fn open(settings: EncoderSettings) -> Result<Self, MediaError> {
        settings.validate()?;
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(settings.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(settings.fps as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            .usage_type(UsageType::ScreenContentRealTime)
            .complexity(Complexity::Low)
            .intra_frame_period(IntraFramePeriod::from_num_frames(
                settings.keyframe_interval(),
            ))
            .adaptive_quantization(false)
            .background_detection(false)
            .skip_frames(true)
            .max_slice_len(4000)
            .num_threads(threads());
        let enc = Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| MediaError::Encoder(e.to_string()))?;
        Ok(Self { enc, settings })
    }
}

/// Slices encoded in parallel: the encoder is the slowest part of sharing this computer's
/// screen, and the picture splits into slices that need no data from each other.
fn threads() -> u16 {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(4)) as u16
}

impl VideoEncoder for SoftEncoder {
    fn name(&self) -> &'static str {
        "openh264"
    }

    fn hardware(&self) -> bool {
        false
    }

    fn settings(&self) -> EncoderSettings {
        self.settings
    }

    fn encode(
        &mut self,
        frame: &BgraFrame,
        pts_us: u64,
        keyframe: bool,
    ) -> Result<Vec<EncodedUnit>, MediaError> {
        let (w, h) = (self.settings.width, self.settings.height);
        let mut yuv = Vec::new();
        bgra_to_i420(frame, w, h, &mut yuv)?;
        let buf = YUVBuffer::from_vec(yuv, w as usize, h as usize);
        if keyframe {
            self.enc.force_intra_frame();
        }
        let ts = openh264::Timestamp::from_millis(pts_us / 1000);
        let out = match self.enc.encode_at(&buf, ts) {
            Ok(bs) => {
                let kind = bs.frame_type();
                let data = bs.to_vec();
                match kind {
                    FrameType::Skip | FrameType::Invalid => Vec::new(),
                    _ if data.is_empty() => Vec::new(),
                    _ => vec![EncodedUnit {
                        data,
                        keyframe: matches!(kind, FrameType::IDR),
                        pts_us,
                    }],
                }
            }
            Err(e) => return Err(MediaError::Encoder(e.to_string())),
        };
        Ok(out)
    }
}

/// A decoded picture in RGBA.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RgbaFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, no padding.
    pub data: Vec<u8>,
}

/// Largest picture the decoder accepts, in pixels (8K). A hostile stream
/// cannot make the viewer allocate more.
pub const MAX_DECODE_PIXELS: usize = 7680 * 4320;

/// The `openh264` decoder.
pub struct H264Decoder {
    dec: Decoder,
}

impl H264Decoder {
    /// Open a decoder.
    pub fn new() -> Result<Self, MediaError> {
        let dec = Decoder::with_api_config(OpenH264API::from_source(), DecoderConfig::new())
            .map_err(|e| MediaError::Decoder(e.to_string()))?;
        Ok(Self { dec })
    }

    /// Decode one access unit into `out`. Returns false when the decoder
    /// produced no picture yet (it needs more data).
    pub fn decode(&mut self, au: &[u8], out: &mut RgbaFrame) -> Result<bool, MediaError> {
        self.decode_with(au, out, true)
    }

    /// Like [`decode`](Self::decode), but with `convert` false the picture is decoded (the next
    /// frames need it) and not turned into RGBA: for a frame that a newer one will replace
    /// before it is shown. Returns false then.
    pub fn decode_with(
        &mut self,
        au: &[u8],
        out: &mut RgbaFrame,
        convert: bool,
    ) -> Result<bool, MediaError> {
        let pic = self
            .dec
            .decode(au)
            .map_err(|e| MediaError::Decoder(e.to_string()))?;
        let Some(pic) = pic else { return Ok(false) };
        if !convert {
            return Ok(false);
        }
        let (w, h) = pic.dimensions();
        if w == 0 || h == 0 || w.saturating_mul(h) > MAX_DECODE_PIXELS {
            return Err(MediaError::BadSize(w as u32, h as u32));
        }
        out.width = w as u32;
        out.height = h as u32;
        out.data.resize(w * h * 4, 0);
        pic.write_rgba8(&mut out.data);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::{AutoEncoder, Backend};

    fn gradient(w: u32, h: u32, shift: u32) -> BgraFrame {
        let stride = w as usize * 4;
        let mut data = vec![0u8; stride * h as usize];
        for y in 0..h {
            for x in 0..w {
                let i = y as usize * stride + x as usize * 4;
                data[i] = ((x + shift) % 256) as u8;
                data[i + 1] = (y % 256) as u8;
                data[i + 2] = ((x + y) % 256) as u8;
                data[i + 3] = 255;
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
    fn encode_decode_round_trip_with_keyframes() {
        let settings = EncoderSettings {
            width: 320,
            height: 180,
            fps: 30,
            bitrate_bps: 1_000_000,
        };
        let mut enc = AutoEncoder::open(settings, Backend::Software).unwrap();
        assert_eq!(enc.name(), "openh264");
        let mut dec = H264Decoder::new().unwrap();
        let mut pic = RgbaFrame::default();
        let mut keys = Vec::new();
        let mut decoded = 0;
        for i in 0..12u32 {
            // Source is larger than the encode size: scaled in the encoder.
            let units = enc
                .encode(&gradient(640, 360, i * 4), u64::from(i) * 33_333, i == 6)
                .unwrap();
            for u in units {
                assert!(u.data.starts_with(&[0, 0, 0, 1]) || u.data.starts_with(&[0, 0, 1]));
                keys.push((i, u.keyframe));
                if dec.decode(&u.data, &mut pic).unwrap() {
                    decoded += 1;
                    assert_eq!((pic.width, pic.height), (320, 180));
                    assert_eq!(pic.data.len(), 320 * 180 * 4);
                }
            }
        }
        assert_eq!(keys.first(), Some(&(0, true)));
        assert!(keys.contains(&(6, true)), "{keys:?}");
        assert!(decoded >= 6, "{decoded}");

        // A new size starts with a keyframe.
        enc.reconfigure(EncoderSettings {
            width: 160,
            height: 90,
            ..settings
        })
        .unwrap();
        let u = enc
            .encode(&gradient(640, 360, 0), 1_000_000, false)
            .unwrap();
        assert!(u[0].keyframe);
        assert!(dec.decode(&u[0].data, &mut pic).unwrap());
        assert_eq!((pic.width, pic.height), (160, 90));
    }

    #[test]
    #[ignore = "timing probe: cargo test -p urdevice-media bench_1080p -- --ignored --nocapture"]
    fn bench_1080p() {
        let settings = EncoderSettings {
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate_bps: 8_000_000,
        };
        let mut enc = AutoEncoder::open(settings, Backend::Software).unwrap();
        let mut dec = H264Decoder::new().unwrap();
        let mut pic = RgbaFrame::default();
        let (mut te, mut td) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        let n = 60u32;
        for i in 0..n {
            let f = gradient(1920, 1080, i * 7);
            let t = std::time::Instant::now();
            let units = enc.encode(&f, u64::from(i) * 33_333, i == 0).unwrap();
            te += t.elapsed();
            for u in units {
                let t = std::time::Instant::now();
                let _ = dec.decode(&u.data, &mut pic).unwrap();
                td += t.elapsed();
            }
        }
        let f = gradient(1920, 1080, 3);
        let mut yuv = Vec::new();
        let t = std::time::Instant::now();
        for _ in 0..n {
            bgra_to_i420(&f, 1920, 1080, &mut yuv).unwrap();
        }
        println!("convert only {:?}/frame", t.elapsed() / n);
        println!(
            "encode {:?}/frame, decode+convert {:?}/frame",
            te / n,
            td / n
        );
    }

    #[test]
    fn decoder_survives_garbage() {
        let mut dec = H264Decoder::new().unwrap();
        let mut pic = RgbaFrame::default();
        for n in 0..50u8 {
            let mut junk = vec![0, 0, 0, 1];
            junk.extend((0..n).map(|i| i.wrapping_mul(97).wrapping_add(n)));
            let _ = dec.decode(&junk, &mut pic);
        }
    }
}
