//! Core logs on Android go to logcat under the tag `UrDevice`.

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::{CString, c_char, c_int};
    use std::io::Write;

    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }

    const INFO: c_int = 4;

    /// Buffers one formatted event and writes it as one logcat line.
    pub(super) struct Logcat(Vec<u8>);

    impl Write for Logcat {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Logcat {
        fn drop(&mut self) {
            let text: Vec<u8> = self.0.iter().copied().filter(|b| *b != 0).collect();
            if let (Ok(tag), Ok(text)) = (CString::new("UrDevice"), CString::new(text)) {
                // SAFETY: both pointers are valid NUL-terminated strings for the call.
                unsafe {
                    __android_log_write(INFO, tag.as_ptr(), text.as_ptr());
                }
            }
        }
    }

    pub(super) fn init(filter: &str) {
        urdevice_core::logging::init_with_writer(filter, || Logcat(Vec::new()));
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    pub(super) fn init(filter: &str) {
        urdevice_core::logging::init(filter);
    }
}

pub(crate) fn init(filter: &str) {
    imp::init(filter);
}
