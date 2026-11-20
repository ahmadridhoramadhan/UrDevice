//! Windows hardware H.264 through a Media Foundation transform (design §10).
//!
//! Hardware encoder MFTs are asynchronous: they say when they want input
//! (`METransformNeedInput`) and when output is ready
//! (`METransformHaveOutput`) through their event generator. `encode` polls
//! those events without blocking for long, feeds one NV12 sample, and
//! returns whatever output is ready, which may be the previous frame's.
//! Any failure makes `AutoEncoder` switch to `openh264`.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate,
    IMFMediaEventGenerator, IMFSample, IMFTransform, METransformHaveOutput, METransformNeedInput,
    MF_E_NO_EVENTS_AVAILABLE, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE,
    MF_EVENT_FLAG_NO_WAIT, MF_LOW_LATENCY, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO,
    MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer,
    MFCreateSample, MFMediaType_Video, MFSTARTUP_NOSOCKET, MFSampleExtension_CleanPoint,
    MFShutdown, MFStartup, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_REGISTER_TYPE_INFO, MFTEnumEx,
    MFVideoFormat_H264, MFVideoFormat_NV12, MFVideoInterlace_Progressive,
    eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_Main,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};
use windows::core::Interface;

use crate::MediaError;
use crate::convert::{BgraFrame, bgra_to_nv12};
use crate::encoder::{EncodedUnit, EncoderSettings, VideoEncoder};

/// Longest wait for the MFT to ask for input before a frame is skipped.
const INPUT_WAIT: Duration = Duration::from_millis(40);
/// Longest wait for output after feeding a frame.
const OUTPUT_WAIT: Duration = Duration::from_millis(20);

fn err(e: windows::core::Error) -> MediaError {
    MediaError::Encoder(format!("media foundation: {e}"))
}

fn var_u32(v: u32) -> VARIANT {
    let mut x = VARIANT::default();
    // SAFETY: writes the tag and the matching union member of a fresh VARIANT.
    unsafe {
        let inner = &mut *x.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    x
}

fn var_true() -> VARIANT {
    let mut x = VARIANT::default();
    // SAFETY: as in `var_u32`.
    unsafe {
        let inner = &mut *x.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = VARIANT_TRUE;
    }
    x
}

fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// A hardware H.264 encoder MFT.
pub(crate) struct MfEncoder {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    codec: Option<ICodecAPI>,
    settings: EncoderSettings,
    in_id: u32,
    out_id: u32,
    need_input: u32,
    nv12: Vec<u8>,
}

// SAFETY: the MFT is created in the multithreaded apartment and used from
// one thread at a time (`&mut self`); hardware MFTs are free-threaded.
unsafe impl Send for MfEncoder {}

impl MfEncoder {
    pub(crate) fn open(settings: EncoderSettings) -> Result<Self, MediaError> {
        // SAFETY: plain Media Foundation calls with valid arguments; every
        // returned interface is owned by the windows crate's smart pointers.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(err)?;
            let transform = first_hardware_encoder()?;
            let attrs = transform.GetAttributes().map_err(err)?;
            attrs
                .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                .map_err(err)?;
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            let events: IMFMediaEventGenerator = transform.cast().map_err(err)?;

            let (mut ins, mut outs) = ([0u32], [0u32]);
            let (in_id, out_id) = match transform.GetStreamIDs(&mut ins, &mut outs) {
                Ok(()) => (ins[0], outs[0]),
                Err(_) => (0, 0),
            };

            let out_type = MFCreateMediaType().map_err(err)?;
            out_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(err)?;
            out_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(err)?;
            out_type
                .SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate_bps)
                .map_err(err)?;
            out_type
                .SetUINT64(&MF_MT_FRAME_SIZE, pack(settings.width, settings.height))
                .map_err(err)?;
            out_type
                .SetUINT64(&MF_MT_FRAME_RATE, pack(settings.fps, 1))
                .map_err(err)?;
            out_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
                .map_err(err)?;
            out_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(err)?;
            out_type
                .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)
                .map_err(err)?;
            transform.SetOutputType(out_id, &out_type, 0).map_err(err)?;

            let in_type = MFCreateMediaType().map_err(err)?;
            in_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(err)?;
            in_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
                .map_err(err)?;
            in_type
                .SetUINT64(&MF_MT_FRAME_SIZE, pack(settings.width, settings.height))
                .map_err(err)?;
            in_type
                .SetUINT64(&MF_MT_FRAME_RATE, pack(settings.fps, 1))
                .map_err(err)?;
            in_type
                .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
                .map_err(err)?;
            in_type
                .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(err)?;
            transform.SetInputType(in_id, &in_type, 0).map_err(err)?;

            let codec: Option<ICodecAPI> = transform.cast().ok();
            if let Some(c) = &codec {
                // Best effort: not every encoder takes every setting.
                let set = |api, v: VARIANT| {
                    let _ = c.SetValue(&api, &v);
                };
                set(CODECAPI_AVLowLatencyMode, var_true());
                set(
                    CODECAPI_AVEncCommonRateControlMode,
                    var_u32(eAVEncCommonRateControlMode_CBR.0 as u32),
                );
                set(
                    CODECAPI_AVEncCommonMeanBitRate,
                    var_u32(settings.bitrate_bps),
                );
                set(
                    CODECAPI_AVEncMPVGOPSize,
                    var_u32(settings.keyframe_interval()),
                );
                set(CODECAPI_AVEncMPVDefaultBPictureCount, var_u32(0u32));
            }

            transform
                .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
                .map_err(err)?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(err)?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(err)?;
            tracing::info!(
                width = settings.width,
                height = settings.height,
                "hardware H.264 encoder ready"
            );
            Ok(Self {
                transform,
                events,
                codec,
                settings,
                in_id,
                out_id,
                need_input: 0,
                nv12: Vec::new(),
            })
        }
    }

    /// Handle every pending event; collect output. Returns false when no
    /// event was waiting.
    fn pump(&mut self, out: &mut Vec<EncodedUnit>, pts_us: u64) -> Result<bool, MediaError> {
        // SAFETY: valid interfaces owned by `self`.
        unsafe {
            let ev = match self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(ev) => ev,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(false),
                Err(e) => return Err(err(e)),
            };
            let kind = ev.GetType().map_err(err)?;
            if kind == METransformNeedInput.0 as u32 {
                self.need_input += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                self.drain(out, pts_us)?;
            }
            Ok(true)
        }
    }

    fn drain(&mut self, out: &mut Vec<EncodedUnit>, pts_us: u64) -> Result<(), MediaError> {
        // SAFETY: the MFT provides its own samples (hardware encoders do);
        // the ManuallyDrop fields are taken exactly once below.
        unsafe {
            let mut buf = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: self.out_id,
                ..Default::default()
            }];
            let mut status = 0u32;
            let r = self.transform.ProcessOutput(0, &mut buf, &mut status);
            let sample: Option<IMFSample> = std::mem::ManuallyDrop::take(&mut buf[0].pSample);
            drop(std::mem::ManuallyDrop::take(&mut buf[0].pEvents));
            match r {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    let t = self
                        .transform
                        .GetOutputAvailableType(self.out_id, 0)
                        .map_err(err)?;
                    self.transform
                        .SetOutputType(self.out_id, &t, 0)
                        .map_err(err)?;
                    return Ok(());
                }
                Err(e) => return Err(err(e)),
            }
            let Some(sample) = sample else {
                return Ok(());
            };
            let keyframe = sample
                .GetUINT32(&MFSampleExtension_CleanPoint)
                .is_ok_and(|v| v != 0);
            let mb = sample.ConvertToContiguousBuffer().map_err(err)?;
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            mb.Lock(&mut ptr, None, Some(&mut len)).map_err(err)?;
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            let _ = mb.Unlock();
            let pts = sample
                .GetSampleTime()
                .ok()
                .and_then(|t| u64::try_from(t / 10).ok())
                .unwrap_or(pts_us);
            if !data.is_empty() {
                out.push(EncodedUnit {
                    data,
                    keyframe,
                    pts_us: pts,
                });
            }
            Ok(())
        }
    }

    fn feed(&mut self, frame: &BgraFrame, pts_us: u64, keyframe: bool) -> Result<(), MediaError> {
        let (w, h) = (self.settings.width, self.settings.height);
        bgra_to_nv12(frame, w, h, &mut self.nv12)?;
        // SAFETY: the buffer is locked while written and is as large as the
        // NV12 data copied into it.
        unsafe {
            let len = u32::try_from(self.nv12.len()).map_err(|_| MediaError::BadSize(w, h))?;
            let mb = MFCreateMemoryBuffer(len).map_err(err)?;
            let mut ptr = std::ptr::null_mut();
            mb.Lock(&mut ptr, None, None).map_err(err)?;
            std::ptr::copy_nonoverlapping(self.nv12.as_ptr(), ptr, self.nv12.len());
            mb.Unlock().map_err(err)?;
            mb.SetCurrentLength(len).map_err(err)?;
            let sample = MFCreateSample().map_err(err)?;
            sample.AddBuffer(&mb).map_err(err)?;
            let t = i64::try_from(pts_us.saturating_mul(10)).unwrap_or(i64::MAX);
            sample.SetSampleTime(t).map_err(err)?;
            sample
                .SetSampleDuration(10_000_000 / i64::from(self.settings.fps.max(1)))
                .map_err(err)?;
            if keyframe && let Some(c) = &self.codec {
                let _ = c.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &var_u32(1u32));
            }
            self.transform
                .ProcessInput(self.in_id, &sample, 0)
                .map_err(err)?;
        }
        Ok(())
    }
}

impl VideoEncoder for MfEncoder {
    fn name(&self) -> &'static str {
        "media-foundation"
    }

    fn hardware(&self) -> bool {
        true
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
        frame.validate()?;
        let mut out = Vec::new();
        let deadline = Instant::now() + INPUT_WAIT;
        while self.need_input == 0 {
            if !self.pump(&mut out, pts_us)? {
                if Instant::now() >= deadline {
                    // The encoder is busy: skip this frame.
                    return Ok(out);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.need_input -= 1;
        self.feed(frame, pts_us, keyframe)?;
        let deadline = Instant::now() + OUTPUT_WAIT;
        let before = out.len();
        while out.len() == before && Instant::now() < deadline {
            if !self.pump(&mut out, pts_us)? {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // Take anything else already waiting.
        while self.pump(&mut out, pts_us)? {}
        Ok(out)
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        // SAFETY: valid interface; errors are irrelevant at shutdown.
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            // Balances the MFStartup in `open`.
            let _ = MFShutdown();
        }
    }
}

/// The first hardware H.264 encoder that takes NV12.
unsafe fn first_hardware_encoder() -> Result<IMFTransform, MediaError> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: out-pointers are valid; the returned array is freed below
    // after every element was released.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut list,
            &mut count,
        )
        .map_err(err)?;
        if list.is_null() || count == 0 {
            return Err(MediaError::Encoder("no hardware H.264 encoder".into()));
        }
        let items = std::slice::from_raw_parts_mut(list, count as usize);
        let mut found = None;
        for a in items.iter_mut() {
            if let Some(act) = a.take()
                && found.is_none()
            {
                found = act.ActivateObject::<IMFTransform>().ok();
            }
        }
        CoTaskMemFree(Some(list as *const _));
        found.ok_or_else(|| MediaError::Encoder("hardware encoder did not activate".into()))
    }
}
