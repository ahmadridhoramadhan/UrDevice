//! The phone's microphone as a virtual source (T11.5, R17.4).
//!
//! Linux (PipeWire, through its PulseAudio-compatible server): two `pactl` modules make the
//! device, a null sink and a source that reads the sink's monitor. The source is a plain
//! microphone named "UrDevice Microphone"; browsers and call apps list it. A `pw-cat` process
//! plays the raw PCM it reads on its standard input into the sink. The sink runs on its own
//! clock, so the source delivers audio as soon as an app records from it. (A `pw-cat` stream
//! made into an `Audio/Source/Virtual` node was tried first: it showed in the settings but
//! stayed suspended, so apps got silence and a web microphone test never finished.) All
//! arguments are fixed; nothing from the network reaches a command line. The pipe is made small
//! and non-blocking, so a stall drops chunks instead of piling them up as delay. The modules
//! and the player go away when the microphone is dropped; modules a crash left behind are
//! removed on the next open. Windows and macOS need a virtual audio driver that is not built yet
//! ([`VirtualMicError::NotAvailable`]).
//!
//! Audio is never stored or logged.

/// Why no virtual microphone could be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualMicError {
    /// This OS has no virtual microphone yet (Windows, macOS).
    NotAvailable,
    /// `pw-cat` or `pactl` is not installed.
    NotInstalled,
    /// The player did not start.
    Failed(String),
}

impl std::fmt::Display for VirtualMicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => f.write_str("no virtual microphone on this system"),
            Self::NotInstalled => f.write_str("pw-cat or pactl is not installed"),
            Self::Failed(why) => write!(f, "the virtual microphone failed: {why}"),
        }
    }
}

impl std::error::Error for VirtualMicError {}

/// A microphone other apps can pick. Dropping it removes the microphone.
pub trait VirtualMic: Send {
    /// Play one chunk of 16-bit little-endian PCM, mono, 48 kHz.
    fn write(&mut self, pcm: &[u8]) -> std::io::Result<()>;
}

/// Open the virtual microphone of this OS.
pub fn open_virtual_mic() -> Result<Box<dyn VirtualMic>, VirtualMicError> {
    #[cfg(target_os = "linux")]
    {
        linux::open()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(VirtualMicError::NotAvailable)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::process::{Child, ChildStdin, Command, Stdio};

    use super::{VirtualMic, VirtualMicError};

    /// The sink the player writes to, and the source apps pick.
    const SINK: &str = "urdevice_mic_sink";
    const SOURCE: &str = "urdevice_mic";

    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }

    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const F_SETPIPE_SZ: i32 = 1031;
    const O_NONBLOCK: i32 = 0o4000;

    /// The pipe holds one page: 2048 samples, about 43 ms. A write of at most this size is
    /// all or nothing.
    const PIPE_BYTES: i32 = 4096;
    const SLICE: usize = 2048;

    /// Run `pactl` with fixed arguments and return what it printed.
    fn pactl(args: &[&str]) -> Result<String, VirtualMicError> {
        let out = Command::new("pactl")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => VirtualMicError::NotInstalled,
                _ => VirtualMicError::Failed(e.to_string()),
            })?;
        if !out.status.success() {
            return Err(VirtualMicError::Failed(format!(
                "pactl {} failed",
                args.first().copied().unwrap_or("")
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    fn unload(id: &str) {
        let _ = pactl(&["unload-module", id]);
    }

    /// Remove modules of ours that a crash left behind, newest first (the source before the sink).
    fn remove_stale() {
        let Ok(list) = pactl(&["list", "short", "modules"]) else {
            return;
        };
        let mut ids: Vec<u32> = list
            .lines()
            .filter(|l| l.contains(SINK) || l.contains(&format!("source_name={SOURCE}")))
            .filter_map(|l| l.split('\t').next()?.parse().ok())
            .collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        for id in ids {
            unload(&id.to_string());
        }
    }

    /// The sink, the source, and the player. Dropping it removes all three.
    struct Mic {
        child: Child,
        stdin: ChildStdin,
        source_module: String,
        sink_module: String,
    }

    impl VirtualMic for Mic {
        fn write(&mut self, pcm: &[u8]) -> std::io::Result<()> {
            // Slices of an even size, so a sample is never cut. A full pipe means the player
            // stalled: the rest of the chunk is dropped.
            for slice in pcm.chunks(SLICE) {
                match self.stdin.write(slice) {
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        }
    }

    impl Drop for Mic {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            unload(&self.source_module);
            unload(&self.sink_module);
        }
    }

    pub(super) fn open() -> Result<Box<dyn VirtualMic>, VirtualMicError> {
        remove_stale();
        let sink_module = pactl(&[
            "load-module",
            "module-null-sink",
            &format!("sink_name={SINK}"),
            "rate=48000",
            "channels=1",
            "channel_map=mono",
            "sink_properties=\"device.description='UrDevice Microphone (output)'\"",
        ])?;
        let source_module = match pactl(&[
            "load-module",
            "module-remap-source",
            &format!("master={SINK}.monitor"),
            &format!("source_name={SOURCE}"),
            "channels=1",
            "channel_map=mono",
            "source_properties=\"device.description='UrDevice Microphone'\"",
        ]) {
            Ok(id) => id,
            Err(e) => {
                unload(&sink_module);
                return Err(e);
            }
        };
        let spawned = Command::new("pw-cat")
            .args(["--playback", "--raw", "--rate", "48000", "--channels", "1"])
            .args([
                "--format",
                "s16",
                "--latency",
                "40ms",
                "--target",
                SINK,
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let undo = || {
            unload(&source_module);
            unload(&sink_module);
        };
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                undo();
                return Err(match e.kind() {
                    std::io::ErrorKind::NotFound => VirtualMicError::NotInstalled,
                    _ => VirtualMicError::Failed(e.to_string()),
                });
            }
        };
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            undo();
            return Err(VirtualMicError::Failed("no pipe".into()));
        };
        let fd = stdin.as_raw_fd();
        // SAFETY: `fd` is the open write end of the pipe of `stdin`, which lives on after
        // these calls; the commands only change the pipe's size and its blocking flag.
        let ok = unsafe {
            fcntl(fd, F_SETPIPE_SZ, PIPE_BYTES) >= 0 && {
                let flags = fcntl(fd, F_GETFL);
                flags >= 0 && fcntl(fd, F_SETFL, flags | O_NONBLOCK) >= 0
            }
        };
        if !ok {
            let _ = child.kill();
            let _ = child.wait();
            undo();
            return Err(VirtualMicError::Failed("pipe setup".into()));
        }
        tracing::info!("virtual microphone opened");
        Ok(Box::new(Mic {
            child,
            stdin,
            source_module,
            sink_module,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_text() {
        assert!(VirtualMicError::NotInstalled.to_string().contains("pactl"));
        assert!(
            VirtualMicError::Failed("x".into())
                .to_string()
                .contains('x')
        );
    }

    /// Needs a running PipeWire with `pw-cat` and `pactl`:
    /// `cargo test -p urdevice-platform -- --ignored the_virtual_microphone`. Plays a tone into the
    /// microphone and records it back from the source an app would pick.
    #[test]
    #[ignore = "needs PipeWire"]
    #[cfg(target_os = "linux")]
    fn the_virtual_microphone_carries_sound_and_goes_away() {
        let listed = || {
            let out = std::process::Command::new("pactl")
                .args(["list", "short", "sources"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains("urdevice_mic\t")
        };
        let mut mic = open_virtual_mic().unwrap();
        assert!(listed(), "the microphone is listed while open");
        let mut rec = std::process::Command::new("parecord")
            .args([
                "--device=urdevice_mic",
                "--raw",
                "--rate=48000",
                "--channels=1",
            ])
            .args(["--format=s16le", "--file-format=wav"])
            .arg(std::env::temp_dir().join("urdevice-mic-test.raw"))
            .spawn()
            .unwrap();
        // Two seconds of a tone, in 20 ms chunks, at the pace of real time.
        let mut phase = 0u32;
        for _ in 0..100 {
            let chunk: Vec<u8> = (0..960)
                .flat_map(|i| {
                    let sample = ((((phase + i) as f32) * 0.0587).sin() * 8000.0) as i16;
                    sample.to_le_bytes()
                })
                .collect();
            phase += 960;
            mic.write(&chunk).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = rec.kill();
        let _ = rec.wait();
        drop(mic);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!listed(), "it is gone after drop");
        let path = std::env::temp_dir().join("urdevice-mic-test.raw");
        let bytes = std::fs::read(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        // A WAV header, then samples: some must be loud.
        let loud = bytes
            .get(44..)
            .unwrap_or_default()
            .chunks_exact(2)
            .any(|p| i16::from_le_bytes([p[0], p[1]]).unsigned_abs() > 4000);
        assert!(loud, "the recording has the tone in it");
    }
}
