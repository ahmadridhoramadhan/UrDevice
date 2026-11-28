# Adding a feature

Every feature (clipboard, file transfer, remote input, ...) already has an empty slot in each layer. Building one means filling in your own files. You should not need to edit `node/`, `net.rs`, `lib.rs`, `app.rs`, `main.slint`, `Screens.kt`, or `MainActivity.kt`. The design is in [design §6.2](../../.kiro/specs/urdevice/design.md#62-streams) and [§9](../../.kiro/specs/urdevice/design.md#9-feature-services).

## Your files

`<id>` is the feature id from the table below, `<Name>` its CamelCase form.

| Layer | File | What goes there |
|---|---|---|
| Wire messages | `crates/urdevice-proto/src/features/<id>.rs` | `prost` structs written by hand, like the core messages; mirror them in `crates/urdevice-proto/proto/features/<id>.proto` |
| Core service | `crates/urdevice-core/src/services/<id>.rs` | `impl Service`, the `register` function, the `SHIPPED` list, and the host-adapter trait the platforms implement |
| Desktop adapters | `crates/urdevice-platform/src/features/<id>.rs` | Windows, macOS, and Linux back-ends behind `#[cfg(target_os = ...)]` |
| Desktop screen | `crates/urdevice-desktop/src/features/<id>.rs` and `ui/features/<id>.slint` | `impl Feature`; the Slint view and its `<Name>Feature` global |
| Android bindings | `crates/urdevice-ffi/src/features/<id>.rs` | `#[uniffi::export] impl UrDeviceNode { fn <id>_... }` and callback interfaces |
| Android screen | `android/app/src/main/java/com/urdevice/app/ui/features/<Name>.kt` | `object <Name>Feature : Feature`: tiles, screen, `onCoreStarted` |
| Android host adapter | `android/app/src/main/java/com/urdevice/app/host/<Name>Host.kt` (new file) | Kotlin side of the callback interfaces |
| Android strings | `android/app/src/main/res/values/strings_<id>.xml` (new file) | Your strings; the capability label and description already exist |
| Tests | `crates/urdevice-core/tests/<id>.rs` (new file) | End-to-end tests with `mod harness;` |

Turning a file into a directory (`services/<id>/mod.rs`, ...) is fine. New files never conflict.

## Service ids and capabilities

Service ids are the only numbers features share. Message field numbers and stream kinds are local to each service, so pick them freely in your own files. Never reuse or renumber an id.

| Id | Feature id | Issue | `ServiceId` | Capability |
|---|---|---|---|---|
| 1 | `clipboard` | #16 | `CLIPBOARD` | `clipboard.v1` |
| 2 | `files` | #17 | `FILES` | `files.v1` |
| 3 | `gallery` | #17 | `GALLERY` | `gallery.v1` (Android hosts) |
| 4 | `input` | #18, #19 | `INPUT` | `input.v1` (pointer, keyboard, text, presenter, laser) |
| 5 | `media` | #20 | `MEDIA` | `media.v1` |
| 6 | `screen` | #21 | `SCREEN` | `screen.h264.v1` |
| 7 | `tunnel` | #22 | `TUNNEL` | `tunnel.v1` |
| 8 | `bluetooth` | #23 | `BLUETOOTH` | none; reserved for fallback control messages |
| 9–15 | | | | reserved for Release 1 |
| 16 | `notify` | #26 | `NOTIFY` | `notify.v1` |
| 17 | `ring` | #27 | `RING` | `ring.v1` |
| 18 | `webcam` | #28 | `CAMERA` | `camera.v1` (Android hosts) |
| 19 | `mic` | #28 | `MIC` | `mic.v1` (Android hosts) |
| 20 | `telephony` | #29 | `TELEPHONY` | `telephony.v1` (Android hosts) |
| 21 | `kvm` | #30 | `KVM` | `kvm.v1` |
| 22 | `display` | #31 | `DISPLAY` | `display.v1` |
| 23 | `terminal` | #32 | `TERMINAL` | `term.v1` |
| 24 | `port_profiles` | #33 | `PORT_PROFILES` | `tunnel.v1` |
| 25 | `inspector` | #34 | `INSPECTOR` | none; local only, reserved |
| 26 | `adb` | #35 | `ADB` | `adb.v1` |
| 27 | `file_browser` | #36 | `FS_BROWSE` | `fs.browse.v1` |
| 28 | `file_browser` | #36 | `FS_WRITE` | `fs.write.v1` |
| 29 | `clip_history` | #37 | `CLIP_HISTORY` | `cliphist.v1` |
| 30 | `proximity_lock` | #38 | `PROXIMITY_LOCK` | `lock.v1` |
| 31 | `automations` | #39 | `AUTOMATIONS` | none; remote actions use the other services |
| 32 | `speaker` | #40 | `SPEAKER` | `audio.out.v1` |
| 33–63 | | | | reserved for Release 2 |
| 0xFF00– | | | `TEST_FIRST` | in-process tests only |

The ids live in `crates/urdevice-proto/src/services.rs`, the capabilities in `crates/urdevice-core/src/permission.rs`. Both already list everything above, with labels on both platforms. The Release 2 default modes in `Capability::default_mode` are provisional; confirm or change yours in the design (requirements → design → tasks) in your PR.

A feature with two capabilities (webcam, file browser) registers two services, one per id: the permission engine checks one capability per service.

## How traffic reaches a service

There are three channels. Each carries the service id, a `Direction`, and an activity id.

| Channel | Send with | Arrives in | Use for |
|---|---|---|---|
| Control message (`Feature` envelope, ≤ 1 MiB) | `ctx.send(peer, dir, activity, &msg)` | `Service::on_message` | Requests, replies, state |
| QUIC stream (`StreamOpen` header, then your bytes) | `ctx.open_bi(...)` / `ctx.open_uni(...)` | `Service::on_stream` | Files, video, audio, tunnels, terminals |
| Datagram (11-byte header, then your payload) | `ctx.send_datagram(...)` | `Service::on_datagram` | Input events; check `ctx.max_datagram_payload(peer)` |

`Direction::ToHost` goes to the device that offers the capability; `Direction::ToController` goes back to the device that uses it.

### The permission check (security.md)

The node checks everything sent `ToHost` before your service sees it, with the host's own setting:

- **Deny:** refused (`denied`).
- **Activity named** (non-zero activity id): accepted only if this host granted that activity to that peer for that capability and it has not ended.
- **No activity:** accepted only when the setting is **Allow** and the capability is not *watched*. Watched capabilities (input, screen, terminal, camera, mic, kvm, display, speaker; `Capability::is_watched`) always need an activity, so the indicator and Stop button show (R5.4). **Always Ask** needs an activity too (`ask`).
- The capability must be one this build ships (`unsupported` otherwise).

A refused control message comes back to the sender's `Service::on_refused` with the reason. A refused stream is reset with `stream_code::REFUSED`. A refused datagram is dropped.

What passes arrives with `Access::Host(grant)`. The `Grant` can only be made by the node, so a host-side function that acts on the device should take `&Grant` as proof. Traffic sent `ToController` arrives with `Access::Controller` and is **not** checked: use it only to show data to the local user, never to act on the device.

### Activities

To start anything under Always Ask, or anything watched, the controller asks first:

```rust
let activity = ctx.request(peer).await?; // the host allows, prompts, or refuses
ctx.send(peer, Direction::ToHost, activity, &msg)?;
```

`request` returns the activity id or `CoreError::Refused(reason)` with `denied`, `declined`, `timeout`, `unsupported`, or `offline`. One approval covers one activity: one transfer, one screen session, one remote-control session (design §7.4). End it with `ctx.stop(peer, activity)`. Both devices' services get `on_activity_end` when it ends for any reason: Stop on either device, the setting changing to Deny (R5.5), or the session closing. Cancel your tasks and close your streams there.

When the host's setting is Allow and the capability is not watched, you can skip `request` and send with activity 0. If the host answers `ask`, request and send again.

## Core service

```rust
// crates/urdevice-core/src/services/clipboard.rs
use std::sync::{Arc, OnceLock};

use urdevice_proto::features::clipboard::Clip;
use super::{Access, Direction, Message, Registry, Service, ServiceCtx, ServiceId, Slot};
use crate::permission::Capability;
use crate::{CoreError, Result};

pub(crate) const SHIPPED: &[Capability] = &[]; // Release 2: list your capabilities when done

/// What the platform provides (desktop: urdevice-platform; Android: Kotlin).
pub trait ClipboardHost: Send + Sync {
    fn write(&self, mime: &str, bytes: &[u8]) -> Result<()>;
}

/// What the UI listens to.
pub trait ClipboardEvents: Send + Sync {
    fn received(&self, from: crate::identity::DeviceId, mime: &str, len: usize);
}

#[derive(Default)]
pub struct Clipboard {
    ctx: OnceLock<ServiceCtx>,
    pub host: Slot<dyn ClipboardHost>,
    pub events: Slot<dyn ClipboardEvents>,
}

impl Clipboard {
    /// Called by the UI.
    pub fn send(&self, peer: crate::identity::DeviceId, clip: &Clip) -> Result<()> {
        let ctx = self.ctx.get().ok_or(CoreError::Stopped)?;
        ctx.send(peer, Direction::ToHost, 0, clip)
    }
}

impl Service for Clipboard {
    fn id(&self) -> ServiceId { ServiceId::CLIPBOARD }
    fn capability(&self) -> Capability { Capability::Clipboard }
    fn start(&self, ctx: ServiceCtx) { let _ = self.ctx.set(ctx); }

    fn on_message(&self, _ctx: &ServiceCtx, msg: Message) -> Result<()> {
        // Untrusted: decode with limits; a malformed body closes the session.
        let clip: Clip = prost::Message::decode(msg.body.as_slice())
            .map_err(|_| CoreError::Protocol("bad Clip".into()))?;
        let Access::Host(_grant) = msg.access else { return Ok(()) };
        if let Some(host) = self.host.get() { host.write(&clip.mime, &clip.bytes)?; }
        Ok(())
    }
}

pub(crate) fn register(r: &mut Registry, _platform: &str) {
    r.add(Arc::new(Clipboard::default()));
}
```

Rules that apply to every service:

- Handlers run on the node's threads. Return quickly; put long work in `ctx.spawn(...)`.
- Treat bodies, `meta`, stream bytes, and datagrams as hostile: size limits, validation, no panics. Add proptest or fuzz targets for parsers.
- Never log content (clipboard, files, keys, audio, frames). `tracing` with ids and sizes only.
- Activity-log entries go through `ctx.log(peer, decision)`, without content.
- UI code reaches the service with `node.service::<Clipboard>()`.

## Release 2: shipping the capability

A Release 2 capability stays hidden, out of the handshake and the settings, until its module lists it: `pub(crate) const SHIPPED: &[Capability] = &[Capability::Notify];`. Existing installs then get the default once (Allow is stored as Always Ask on upgrade). Add your capability to the onboarding defaults in the design when you ship it (R5.2).

## Desktop

`src/features/<id>.rs` implements `Feature`:

```rust
impl Feature for Clipboard {
    fn id(&self) -> &'static str { "clipboard" }
    fn tiles(&self) -> &'static [TileSpec] { &[TileSpec { cap: Capability::Clipboard, key: "clipboard.v1" }] }
    fn has_view(&self) -> bool { true } // tiles open ui/features/clipboard.slint
    fn start(&self, ctx: &FeatureCtx) {
        let Some(w) = ctx.window.upgrade() else { return };
        let Some(svc) = ctx.node.service::<urdevice_core::services::clipboard::Clipboard>() else { return };
        svc.host.set(Arc::new(urdevice_platform::features::clipboard::host()));
        let g = w.global::<crate::ui::ClipboardFeature>();
        g.on_send(move |text| { /* svc.send(...) */ });
    }
    fn open(&self, ctx: &FeatureCtx, peer: &PeerView, _tile: &str) {
        if let Some(w) = ctx.window.upgrade() {
            w.global::<crate::ui::ClipboardFeature>().set_peer_id(peer.id.clone().into());
        }
    }
}
```

`ui/features/<id>.slint` holds the `<Name>Feature` global (properties and callbacks for Rust) and the `<Name>View` component. Both are already wired: the global is exported from `main.slint`, and `FeatureView` shows your view when your tile is opened. Until `has_view` returns true, a tile only sends an activity request, as it does today. Core events arrive on core threads; hand them to the UI with `slint::invoke_from_event_loop`.

## Android

The FFI module adds methods on the node and callback interfaces. Multiple `#[uniffi::export] impl UrDeviceNode` blocks are fine.

```rust
// crates/urdevice-ffi/src/features/clipboard.rs
use std::sync::Arc;
use crate::{FfiError, UrDeviceNode, device_id};

#[uniffi::export(with_foreign)]
pub trait ClipboardHostFfi: Send + Sync { fn write(&self, mime: String, bytes: Vec<u8>) -> bool; }

#[uniffi::export(with_foreign)]
pub trait ClipboardListener: Send + Sync { fn on_received(&self, device_id: String, mime: String); }

#[uniffi::export]
impl UrDeviceNode {
    pub fn clipboard_set_host(&self, host: Arc<dyn ClipboardHostFfi>) { /* wrap it in the core trait, set the Slot */ }
    pub fn clipboard_set_listener(&self, l: Arc<dyn ClipboardListener>) { /* ... */ }
    pub fn clipboard_send(&self, device: String, text: String) -> Result<(), FfiError> {
        let peer = device_id(&device)?;
        /* self.node.service::<Clipboard>()...send(peer, ...) */
        Ok(())
    }
}
```

The Kotlin side calls them from `ui/features/<Name>.kt`:

```kotlin
object ClipboardFeature : Feature {
    override val id = "clipboard"
    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("clipboard.v1", "clipboard.v1"))
    override val hasScreen get() = true
    @Composable override fun Screen(peer: FfiPeer, tile: String) { /* ... */ }
    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        node.clipboardSetHost(ClipboardHost(context))       // host/ClipboardHost.kt
        node.clipboardSetListener(ClipboardEvents)           // post to the main thread
    }
}
```

Release 2 tiles use `listed(modes, ...)`, so they show only when the host lists the capability. Callback methods run on core threads; post UI work to the main thread.

## Shared files you may still touch

- `Cargo.toml` and `Cargo.lock` for new dependencies (run `cargo deny check`). Add yours at the end of the table; the orchestrator resolves lock-file conflicts.
- `android/app/src/main/AndroidManifest.xml` for permissions, services, and receivers. Put your entries in one block with a `<!-- <id> -->` comment.
- `.kiro/specs/urdevice/*.md` for your tasks and any decision change.

## Testing without devices

`crates/urdevice-core/tests/harness/mod.rs` starts two nodes on loopback and pairs them:

```rust
mod harness;
use harness::{Dev, Pair, WAIT, block_on, service, wait_until};

#[test]
fn clip_reaches_the_other_device() {
    let p = Pair::with(
        Dev::with("Laptop", "linux", vec![]),
        Dev::with("Phone", "android", vec![]),
    ); // built-in services are registered on both
    let a = p.a.node.service::<Clipboard>().unwrap();
    // set a fake host adapter on `b`, call `a.send(p.b.id(), ...)`, then wait_until(...)
}
```

`Dev::with(name, platform, vec![service(Arc::new(MyService))])` adds an extra service; `p.a.set(cap, mode)` changes a setting; `p.a.answer_prompt(true)` answers the next Always Ask prompt; `block_on` runs `ctx.request(...)` or `ctx.open_bi(...)` from a test thread. `crates/urdevice-core/tests/services.rs` is a complete example.

## Running on the test devices

`scripts/dev/` has small helpers. Set `URDEVICE_LOCK_DIR` to the shared lock directory so each device is used under its lock.

| Script | Does |
|---|---|
| `linux-run.sh [instance]` | `cargo run -p urdevice-desktop`; with an instance name, its own data dir and port |
| `linux-shot.sh [out.png]` | Screenshot through the desktop portal (works on GNOME Wayland) |
| `android-run.sh [abi]` | Build the debug APK (`arm64-v8a` by default), install it, start the app |
| `android-shot.sh [out.png]` | Phone screenshot |
| `win-run.sh [branch]` | Push HEAD to the Windows machine, build there, start the app on its desktop |
| `win-shot.sh [out.png]` | Windows screenshot, copied here |
| `win-stop.sh` | Stop the app on Windows |

`scripts/dev/win/runi.ps1` and `shot.ps1` run on Windows: SSH has no desktop, so programs start through a one-off interactive scheduled task.
