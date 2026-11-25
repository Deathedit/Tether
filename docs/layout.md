# Repository layout

```
third_party/chiaki-ng/   git submodule, pinned to v1.10.1
crates/chiaki-sys/       builds chiaki-ng's lib/ with CMake, bindgen bindings
crates/rp-core/          safe Rust API: discovery, wake, registration, PSN sign-in,
                         sessions (video, audio, haptics, controller, mic)
crates/tether-core/      app logic: config, paired consoles, secrets, stream
                         settings, wake-and-wait, audio playback and microphone
                         (Opus + cpal)
crates/desktop/          the `tether` binary: Slint UI (ui/*.slint), CLI,
                         FFmpeg decoding (hardware or software), wgpu renderer,
                         SDL3 controllers and rumble
crates/tether-ffi/       the Android app's Rust core (libtether_ffi.so): UniFFI
                         API for Kotlin, MediaCodec decoder drawing straight
                         onto the SurfaceView, JNI hand-off of the surface
crates/uniffi-bindgen/   generates tether-ffi's Kotlin bindings from the .so
android/                 Gradle project: Kotlin + Jetpack Compose app (home,
                         pairing, settings, stream), bottom-screen companion
                         (Presentation), phone sign-in page, build-rust.sh
```
