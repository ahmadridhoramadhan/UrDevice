//! The phone's microphone as a virtual source (T11.5, R17.4).
//!
//! Linux: a `pw-cat` process (package `pipewire-utils`) plays the raw PCM it reads on its standard
//! input into a PipeWire node of class `Audio/Source/Virtual` named "UrDevice Microphone", which
//! call apps list as a microphone. The node exists while the process runs. All arguments are fixed;
//! nothing from the network reaches the command line. While no app records from the
//! microphone the player does not read its input: the pipe is made small and non-blocking, so
//! the chunks that do not fit are dropped instead of piling up as delay. Windows and macOS need a virtual audio
//! driver that is not built yet ([`VirtualMicError::NotAvailable`]).
//!
//! Audio is never stored or logged.

/// Why no virtual microphone could be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualMicError {
    /// This OS has no virtual microphone yet (Windows, macOS).
    NotAvailable,
    /// `pw-cat` is not installed.
    NotInstalled,
    /// The player did not start.
    Failed(String),
}

impl std::fmt::Display for VirtualMicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => f.write_str("no virtual microphone on this system"),
            Self::NotInstalled => f.write_str("pw-cat is not installed"),
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

    /// The name other apps show.
    const DESCRIPTION: &str = "UrDevice Microphone";

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

    struct PwCat {
        child: Child,
        stdin: ChildStdin,
    }

    impl VirtualMic for PwCat {
        fn write(&mut self, pcm: &[u8]) -> std::io::Result<()> {
            // Slices of an even size, so a sample is never cut. A full pipe means nobody is
            // recording right now: the rest of the chunk is dropped.
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

    impl Drop for PwCat {
        fn drop(&mut self) {
            // Closing the pipe ends the player; kill it if it does not.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    pub(super) fn open() -> Result<Box<dyn VirtualMic>, VirtualMicError> {
        let props = format!(
            "{{ media.class=Audio/Source/Virtual node.name=urdevice-mic node.description=\"{DESCRIPTION}\" }}"
        );
        let mut child = Command::new("pw-cat")
            .args(["--playback", "--raw", "--rate", "48000", "--channels", "1"])
            .args(["--format", "s16", "--latency", "40ms", "-P", &props, "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => VirtualMicError::NotInstalled,
                _ => VirtualMicError::Failed(e.to_string()),
            })?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
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
            return Err(VirtualMicError::Failed("pipe setup".into()));
        }
        tracing::info!("virtual microphone opened");
        Ok(Box::new(PwCat { child, stdin }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_read_as_text() {
        assert!(VirtualMicError::NotInstalled.to_string().contains("pw-cat"));
        assert!(
            VirtualMicError::Failed("x".into())
                .to_string()
                .contains('x')
        );
    }

    /// Needs a running PipeWire with `pw-cat`: `cargo test -p urdevice-platform -- --ignored`.
    #[test]
    #[ignore = "needs PipeWire"]
    #[cfg(target_os = "linux")]
    fn the_virtual_source_exists_while_it_is_open() {
        let sources = || {
            let out = std::process::Command::new("pactl")
                .args(["list", "short", "sources"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains("urdevice-mic")
        };
        let mut mic = open_virtual_mic().unwrap();
        // One second of a quiet tone, in 20 ms chunks, at the pace of real time.
        let chunk: Vec<u8> = (0..960)
            .flat_map(|i| {
                let sample = (((i as f32) * 0.1).sin() * 3000.0) as i16;
                sample.to_le_bytes()
            })
            .collect();
        for _ in 0..50 {
            mic.write(&chunk).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(sources(), "the virtual microphone is listed while open");
        drop(mic);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!sources(), "it is gone after drop");
    }
}
