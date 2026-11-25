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

## Releases

`.github/workflows/release.yml` builds everything a release ships:

| Target  | File                                   |
|---------|----------------------------------------|
| NixOS   | the flake's `tether` package (built as a check, not uploaded) |
| Arch    | `tether-<version>-1-x86_64.pkg.tar.zst` |
| Windows | `tether-<version>-windows-x86_64.zip`   |
| Android | `tether-<version>-android.apk`          |

### Installing

- **NixOS**: `nix run github:Deathedit/Tether`, or add the flake as an input
  and put `inputs.tether.packages.${system}.default` in
  `environment.systemPackages`. Needs Nix 2.27 or newer (the flake fetches
  the chiaki-ng submodule).
- **Arch**: `sudo pacman -U tether-*.pkg.tar.zst` from the release, or build
  it yourself with `makepkg -si` in `packaging/arch/`.
- **Windows**: unzip and run `tether.exe`. It needs the Microsoft Visual C++
  runtime, which most PCs already have (`winget install
  Microsoft.VCRedist.2015+.x64` otherwise).
- **Android**: install the APK. Release builds are signed with a different
  key than CI's debug builds, so uninstall a debug build first (you'll have
  to pair again).

### Making a release

1. Set the new version in `Cargo.toml` (`[workspace.package]`; the Android
   app reads it from there) and `pkgver` in `packaging/arch/PKGBUILD`.
2. Commit, then tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. The workflow checks that the tag matches the version, builds all targets
   and creates a **draft** release with the files. Check it and publish it
   on GitHub.

To try the builds without releasing, run the workflow by hand (Actions >
Release > Run workflow) on any branch. The files are then uploaded as
workflow artifacts, and the APK is unsigned unless the key secrets exist.

### Android release key

The key that signs release APKs is yours: make it once, keep a backup, and
never commit it. Every later release must be signed with the same key, or
Android refuses to update the installed app.

```
keytool -genkeypair -v -keystore tether-release.jks -storetype PKCS12 \
  -alias tether -keyalg RSA -keysize 4096 -validity 10000 -dname "CN=Tether"
```

(`keytool` comes with any JDK. A PKCS12 keystore uses one password for the
store and the key.) Then add four repository secrets under Settings >
Secrets and variables > Actions, or with `gh secret set`:

| Secret                     | Value                                   |
|----------------------------|-----------------------------------------|
| `TETHER_KEYSTORE_BASE64`   | the keystore file, base64-encoded       |
| `TETHER_KEYSTORE_PASSWORD` | the password                            |
| `TETHER_KEY_ALIAS`         | `tether`                                |
| `TETHER_KEY_PASSWORD`      | the password (same as above)            |

To base64-encode it: `base64 -w0 tether-release.jks` on Linux, or in
PowerShell `[Convert]::ToBase64String([IO.File]::ReadAllBytes("$PWD\tether-release.jks"))`.

To sign a local build, set `TETHER_KEYSTORE` (the keystore's path) and the
three other variables, then run `gradle --project-dir android assembleRelease`.
