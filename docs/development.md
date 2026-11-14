# Development

This page lists what you need to build UrDevice on each platform. The repository layout is described in [design §2](spec/design.md#2-repository-layout).

```
UrDevice/
├─ Cargo.toml              # Rust workspace
├─ rust-toolchain.toml     # pinned Rust version (1.95.0)
├─ crates/
│  ├─ urdevice-proto/      # protobuf schemas (empty for now)
│  ├─ urdevice-core/       # platform traits, core_version()
│  ├─ urdevice-platform/   # desktop adapters: windows, macos, linux/{x11,wayland}
│  ├─ urdevice-media/      # capture and encode (placeholder)
│  ├─ urdevice-ffi/        # UniFFI bindings for Android
│  └─ urdevice-desktop/    # Slint app, binary name `urdevice`
└─ android/
   ├─ app/                 # Compose app
   └─ core-bindings/       # Android library: Rust .so files + generated Kotlin
```

## Rust (all platforms)

Install [rustup](https://rustup.rs). The first `cargo` command in the repository installs the version pinned in `rust-toolchain.toml`.

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Dependency checks (install once with `cargo install --locked cargo-deny cargo-audit`):

```sh
cargo deny check
cargo audit
```

## Desktop app

```sh
cargo run -p urdevice-desktop
```

The app opens a Slint window and a tray icon. Both show the core version.

### Linux

Install the fontconfig development package:

```sh
# Debian / Ubuntu
sudo apt install libfontconfig1-dev
# Fedora
sudo dnf install fontconfig-devel
```

The app runs on both X11 and Wayland. It picks the back-end from `XDG_SESSION_TYPE` and `WAYLAND_DISPLAY`. The tray uses the StatusNotifierItem protocol. KDE and most wlroots desktops show it. GNOME needs the AppIndicator extension. Without a tray host, the app still runs and only the window is shown.

### Windows

Install the [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload.

### macOS

Install the Xcode Command Line Tools:

```sh
xcode-select --install
```

## Android

You need:

- JDK 17 or newer, with `javac` (a full JDK, not only a JRE)
- Android SDK with platform `android-37.0`, build tools `36.0.0`, and NDK `27.0.12077973`
- Rust Android targets and `cargo-ndk`:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install --locked cargo-ndk
```

Tell Gradle where the SDK is, either with `ANDROID_HOME` or with `android/local.properties`:

```properties
sdk.dir=/path/to/Android/Sdk
```

Then build from `android/`:

```sh
./gradlew assembleDebug
```

The APK is written to `android/app/build/outputs/apk/debug/app-debug.apk`. Its first screen shows the Rust core version.

### How the Rust build is wired

The `core-bindings` module runs two tasks before it compiles:

| Task | What it does | Output |
|---|---|---|
| `cargoNdkBuild<Variant>` | `cargo ndk` builds `liburdevice_ffi.so` for each ABI (`--release` for release builds) | `core-bindings/build/generated/rust/<variant>/jniLibs` |
| `generateUniFfiBindings` | builds `urdevice-ffi` for the host, then runs the crate's own `uniffi-bindgen` binary on it | `core-bindings/build/generated/uniffi/kotlin` |

Generated files stay under `build/`, so a clean checkout builds without any committed binaries. To build only one ABI while working locally:

```sh
./gradlew assembleDebug -Purdevice.abis=arm64-v8a
```

## Ports

- UDP discovery: `47810`
- QUIC transport: `47811`
