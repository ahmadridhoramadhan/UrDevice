//! UniFFI bindings for the Android app.
//!
//! Built for Android with `cargo-ndk`; the Gradle module `android/core-bindings`
//! runs the build and generates the Kotlin bindings (see `docs/development.md`).

uniffi::setup_scaffolding!();

/// The version of the UrDevice core.
#[uniffi::export]
pub fn core_version() -> String {
    urdevice_core::core_version().to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn exposes_core_version() {
        assert_eq!(super::core_version(), urdevice_core::core_version());
    }
}
