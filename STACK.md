# Tether: tech stack

A PS5 Remote Play client for:

- **Linux** (x86_64)
- **Windows** (x86_64)
- **Android** (arm64), first of all the **AYN Thor**, which has two screens

Written in Rust. "Tether" is a working name.

Design: [design canvas](https://claude.ai/artifact/Mvt1WPLdvqK4VBUJBCQJCj) (private until shared from its Share menu) and `screenshots/` (the same 7 screens as images).

---

## 1. Shape of the project

```
            ┌───────────────────────── tether-core (Rust) ─────────────────────────┐
            │ protocol (chiaki-ng over FFI) · session · audio · config · stats      │
            └───────────────┬─────────────────────────────────────┬────────────────┘
                            │ Rust API                            │ UniFFI (Kotlin bindings)
            ┌───────────────▼──────────────┐       ┌──────────────▼───────────────────┐
            │ Desktop app (Rust)           │       │ Android app (Kotlin)             │
            │ Slint UI · wgpu · FFmpeg     │       │ Jetpack Compose UI               │
            │ SDL3 controllers             │       │ MediaCodec → SurfaceView         │
            │ Linux + Windows              │       │ bottom screen = companion        │
            └──────────────────────────────┘       └──────────────────────────────────┘
```

All the hard parts live in shared Rust: the protocol, the session, audio and
settings. Each platform gets a thin native shell around it.

**Why Android gets its own Kotlin UI and doesn't reuse Slint:**

- **Lowest latency.** Android's hardware decoder (MediaCodec) can draw
  straight into a `SurfaceView`, with no copy and no extra GPU pass. Moonlight
  does the same on Android.
- **Two screens.** Putting a window on the Thor's bottom screen uses Android
  display APIs that are easy from Kotlin and awkward from a pure-native app.
- **Small cost.** The design is only 7 screens, so building it twice
  (Slint + Compose) is a small amount of work.

**Alternative:** Slint on Android too, for one UI codebase. The cost is one
extra frame copy on the video path and JNI glue code for the bottom screen.
It works, but it's slower and more fiddly on the device you care most about.

---

## 2. The protocol: wrap chiaki-ng, don't rewrite it

There is no maintained Rust implementation of PlayStation Remote Play. The
proven open-source one is **chiaki-ng** (C/C++, actively maintained in 2026).

- **Choice:** build chiaki-ng's core C library (`lib/`) into a `chiaki-sys`
  crate with the `cmake` and `bindgen` crates, and wrap it in safe Rust.
- **What it already does:** discovery, wake, PIN registration, the encrypted
  UDP session (Takion), congestion control and PSN holepunch for playing over
  the internet.
- **Fixes come free:** staying on upstream means you get their fixes when
  Sony changes something.
- **Android:** the original Chiaki shipped an Android app built on the same C
  library, so the library has been built with the NDK before. **Check early**
  that the current chiaki-ng `lib/` and its dependencies build with the NDK,
  especially OpenSSL (or mbedtls), json-c, curl and miniupnpc.

**Licence:** chiaki-ng is **AGPL-3.0**, so the whole app is AGPL-3.0 and its
source must be public. Every dependency below is compatible.

**Trademarks:** don't use Sony names or logos in the app name, icon or
branding. "Works with PS5" as plain text is fine.

---

## 3. Stack

### Shared core (`tether-core`, Rust)

| Part | Pick |
|---|---|
| Protocol | `chiaki-sys` (our crate: `cmake` + `bindgen` on chiaki-ng `lib/`, git submodule pinned to a release tag) |
| Safe API | `rp-core` (our crate): `Discovery`, `wake()`, `register(pin)`, `Session` with events on `crossbeam-channel` |
| Audio out | chiaki hands over raw Opus frames; `opus` (libopus, built from source by its crate) decodes them on chiaki's thread into a small jitter buffer (30 ms default) drained by `cpal` (WASAPI on Windows, ALSA/PipeWire on Linux, AAudio on Android) |
| Mic | `cpal` input → `opus` encode (48 kHz stereo, CBR so every 10 ms packet is exactly the 40 bytes chiaki's audio sender accepts) → chiaki's audio sender |
| Config | `serde` + `toml`, `directories` (desktop) / app files dir passed in (Android) |
| Secrets | `keyring` on desktop; Android Keystore through the Kotlin side |
| Logging / errors | `tracing` (+ `tracing-android` on Android), `thiserror`, `anyhow` |
| Kotlin bindings | **UniFFI**, generating Kotlin from the Rust API |

### Desktop app (Linux, Windows)

| Part | Pick |
|---|---|
| UI | **Slint**, using its wgpu integration to show the video texture |
| Video decode | `ffmpeg-next` (FFmpeg 7.x): VAAPI on Linux, D3D11VA on Windows, software fallback |
| Video render | `wgpu`, NV12 → RGB shader (Vulkan / D3D12) |
| Controllers | `sdl3` crate on its own thread (gamepad subsystem only; winit keeps the window): buttons, sticks, triggers, touchpad, gyro → chiaki's controller state on every change; rumble, adaptive triggers, light bar, player LEDs back. DualSense haptics (an audio stream) are turned into rumble for now; playing them on the controller's audio interface is a TODO |
| Packaging | AppImage (Linux); zip or MSI with `cargo-wix` (Windows); FFmpeg, SDL3 and libopus are linked statically on Windows |
| Native deps | Linux: system packages via pkg-config (Nix flake dev shell, or Arch packages). Windows: vcpkg (`vcpkg.json`, triplet `x64-windows-static-md`), except SDL3, which its crate builds from source. libopus is always built from source by its crate |
| C toolchain (Windows) | chiaki-ng's C code is compiled with **clang-cl** + Ninja: it uses C99 variable-length arrays that MSVC's `cl.exe` rejects (upstream builds Windows with MinGW). clang-cl still emits MSVC-ABI objects, so Rust stays on `x86_64-pc-windows-msvc` |

### Android app (AYN Thor first)

| Part | Pick |
|---|---|
| Language / UI | Kotlin, Jetpack Compose, min SDK 30 (Android 11) |
| Rust build | `cargo-ndk` → `aarch64-linux-android` (+ `x86_64-linux-android` for the emulator), loaded as a `.so` |
| Video decode | **MediaCodec from Rust** through the `ndk` crate (`AMediaCodec`). Kotlin passes the `SurfaceView`'s `Surface` down once; Rust turns it into an `ANativeWindow`, and the decoder draws straight onto it. No per-frame JNI calls. Set `KEY_LOW_LATENCY` and the vendor low-latency keys when the decoder supports them. |
| Controllers | Android `KeyEvent` / `MotionEvent` from the gamepad, forwarded to the core. The Thor's built-in controls are a standard Android gamepad. Rumble uses `InputDevice.vibratorManager` (Android 12+). |
| Touchpad | The touchpad area on the bottom screen sends touch coordinates to the core as DualSense touchpad input |
| Bottom screen | Find it with `DisplayManager.getDisplays()`: the non-default display. **Don't hardcode its ID.** One developer reports it as display 4 on the Thor. Show the companion UI there with Android's `Presentation` class. It lives inside the main Activity, so it shares the session state for free. On phones with one screen, the companion becomes an overlay. |
| Packaging | APK / AAB via Gradle, sideloaded first |

---

## 4. Repository layout

```
tether/
  Cargo.toml                    # Rust workspace
  third_party/chiaki-ng/        # git submodule, pinned to a release tag
  crates/
    chiaki-sys/                 # build.rs: cmake lib only (no GUI) + bindgen; desktop + NDK
    rp-core/                    # safe protocol API
    tether-core/                # session orchestration, audio, config, stats
    tether-ffi/                 # UniFFI surface + Android MediaCodec decoder (ndk crate)
    desktop/                    # Slint UI, ffmpeg decode, wgpu render, SDL3 input
      ui/                       # consoles, pairing, connecting, session, quick-menu, settings
  android/                      # Gradle project
    app/src/main/java/.../      # Compose screens, StreamActivity, CompanionPresentation
    app/src/main/jniLibs/       # filled by cargo-ndk
  packaging/  linux/  windows/
```

**Threads:** chiaki's session thread → video packets → decoder (FFmpeg on
desktop, MediaCodec on Android) → screen. Audio has its own thread. Input is
sent to the session as soon as it arrives. The UI only gets state changes
and stats, about 4 times a second.

---

## 5. CI (GitHub Actions)

- `ubuntu-24.04`: Linux build + tests, inside the Nix flake's dev shell (same environment as NixOS)
- `windows-latest`: Windows build + tests (vcpkg + clang-cl)
- `ubuntu-24.04`: Android build, with the NDK via `android-actions/setup-android`, then `cargo-ndk` and Gradle `assembleRelease`

---

## 6. Milestones (in order)

1. **Discover:** desktop CLI that lists consoles on the LAN and can wake one.
2. **Pair:** register with the 8-digit PIN and store the keys. You need the
   PSN account ID; copy chiaki-ng's sign-in flow for getting it.
3. **First picture (desktop):** start a session and show software-decoded
   video in a plain window.
4. **Play (desktop):** audio, SDL3 controller input, rumble.
5. **Desktop UI:** Slint screens from the design, then hardware decoding.
6. **Android core:** `chiaki-sys` builds with the NDK, UniFFI bindings, and a
   bare Activity that streams video through MediaCodec into a SurfaceView.
7. **Android app:** Compose screens from the design, gamepad input, audio,
   and the bottom-screen companion with touchpad, stats and quick actions.
8. **Ship:** release builds for all three targets in CI.

---

## 7. Prompt for your coding agent

> You are building **Tether**, an AGPL-3.0 PS5 Remote Play client. Targets:
> Linux x86_64, Windows x86_64 and Android arm64 (first of all the AYN Thor,
> a dual-screen Android handheld). Read `STACK.md` and follow it. The UI must
> match the screens in `screenshots/` (design canvas:
> https://claude.ai/artifact/Mvt1WPLdvqK4VBUJBCQJCj).
>
> Rules:
> - Shared logic goes in Rust (`chiaki-sys`, `rp-core`, `tether-core`).
>   Desktop is a Rust app with a Slint UI. Android is a Kotlin + Jetpack
>   Compose app that calls the Rust core through UniFFI.
> - Use chiaki-ng's C library (git submodule in `third_party/chiaki-ng`,
>   pinned to a release tag) through `chiaki-sys`, built with the `cmake` and
>   `bindgen` crates. Do not re-implement the protocol.
> - Put all `unsafe` code in `chiaki-sys`, `rp-core` and the Android decoder
>   in `tether-ffi`.
> - Android video: MediaCodec driven from Rust via the `ndk` crate, drawing
>   straight onto the SurfaceView's ANativeWindow. No per-frame JNI.
> - Android bottom screen: find the secondary display with `DisplayManager`
>   (never hardcode its ID) and show the companion UI with `Presentation`.
> - Keep video decode and rendering off the UI thread.
> - No Sony logos or trademarks in names, icons or UI text.
> - Work milestone by milestone, in the order in `STACK.md` section 6. Each
>   milestone must pass CI before you start the next.
>
> Start with milestone 1: set up the workspace, the `chiaki-sys` build and a
> `tether discover` CLI that prints each console's name, IP address and state
> (ready / rest mode).
