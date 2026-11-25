# Building

Clone with submodules:

```
git clone --recursive <repo-url>
# or, in an existing clone:
git submodule update --init --recursive
```

All platforms need Rust (stable), CMake, a C compiler, libclang (for bindgen),
`protoc`, Python 3 with the `protobuf` package (chiaki-ng's nanopb code
generator uses both), and FFmpeg. Linux also needs SDL3 and ALSA's library
(sound goes through PipeWire or PulseAudio if they run). libopus is always
built from source as part of the Rust build.

## NixOS

The flake has a dev shell with everything, including the Rust toolchain:

```
nix develop
cargo build --release
```

(Needs flakes enabled. With direnv, `echo "use flake" > .envrc && direnv allow`.)

## Arch Linux

```
sudo pacman -S --needed base-devel rust cmake clang pkgconf \
  openssl json-c miniupnpc libevent protobuf python-protobuf ffmpeg \
  fontconfig libxkbcommon vulkan-icd-loader libva sdl3 alsa-lib
cargo build --release
```

For hardware decoding and rendering you also need your GPU's VAAPI driver
(`libva-mesa-driver` for AMD, `intel-media-driver` for Intel,
`libva-nvidia-driver` for NVIDIA) and its Vulkan driver (`vulkan-radeon`,
`vulkan-intel`, or the NVIDIA driver).

Use `rustup` instead of the `rust` package if you prefer; `rust-toolchain.toml`
pins stable.

## Windows

Needs Visual Studio 2022 (or Build Tools) with the C++ workload, which brings
CMake and vcpkg, plus LLVM (libclang), Python 3 and `protoc`:

```
winget install LLVM.LLVM Python.Python.3.13 Google.Protobuf
python -m pip install protobuf
```

Build the native libraries once, from the repository root, in a
"Developer PowerShell for VS 2022":

```
vcpkg install --triplet x64-windows-static-md
```

This fills `vcpkg_installed/` (including a static FFmpeg). To use another
location, set `TETHER_VCPKG_INSTALLED` to the triplet directory.

SDL3 and libopus don't come from vcpkg: cargo builds them from source with
CMake and links them statically. Outside a Developer Prompt that needs
`cmake` on PATH, or `CMAKE` set to its full path (Visual Studio's copy is in
`Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\` under the Visual
Studio folder; `winget install Kitware.CMake` also works).

Then, from any terminal (no Developer Prompt needed), point the FFmpeg
bindings at the vcpkg tree and build:

```
$env:FFMPEG_DIR = "$PWD\vcpkg_installed\x64-windows-static-md"
cargo build --release
```

chiaki-ng's C code is compiled with clang-cl (from LLVM) because it uses C
features MSVC's compiler lacks. The build finds clang-cl, CMake and Ninja on
PATH or in the LLVM and Visual Studio folders; override with `CLANG_CL`,
`CMAKE` or `NINJA`.

If CMake picks the wrong Python or `protoc`, set the `PYTHON` or `PROTOC`
environment variables to the full paths.

## Android

The APK is built on a Linux (or macOS) machine: chiaki-ng builds OpenSSL for
Android with OpenSSL's own `Configure` script and `make`. CI builds a
debug-signed APK on every push (the `tether-android-debug` artifact), which
installs directly on a phone or the AYN Thor.

To build it yourself you need, besides the Linux build tools above:

- JDK 17, the Android SDK (platform 35) and the NDK (`ndk;28.2.13676358`),
  with `ANDROID_NDK_HOME` pointing at the NDK
- the Rust targets: `rustup target add aarch64-linux-android x86_64-linux-android`
- `cargo install cargo-ndk`
- Gradle 8.14

```
sh android/build-rust.sh                # Rust core (.so) + Kotlin bindings
gradle --project-dir android assembleDebug
adb install android/app/build/outputs/apk/debug/app-debug.apk
```

`build-rust.sh` writes `libtether_ffi.so` into `android/app/src/main/jniLibs/`
and the generated Kotlin into `android/app/src/main/java/.../core/`; both are
build outputs and not committed.
