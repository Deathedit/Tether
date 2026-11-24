# Tether

A Remote Play client for Linux, Windows and Android (AYN Thor first), written
in Rust on top of [chiaki-ng](https://github.com/streetpea/chiaki-ng). Works
with PS5. Licensed AGPL-3.0.

See [STACK.md](STACK.md) for the architecture and milestones.

## Status

Milestones 1 (discover), 2 (pair), 3 (first picture) and 4 (play). Command line
plus a plain video window so far, with sound, controllers and rumble:

```
tether discover              # consoles on the LAN, and whether they're paired
tether login                 # sign in to PSN once (gets your account ID)
tether pair 192.168.1.20     # pair using the 8-digit code the console shows
tether consoles              # paired consoles
tether wake "Living Room"    # wake a paired console (--wait: until it's ready)
tether stream "Living Room"  # wake it if needed and show its video in a window
tether unpair "Living Room"
```

`tether stream` takes `--resolution 360p|540p|720p|1080p` (default 720p),
`--fps 30|60` and `--codec h264|h265`. Video is decoded in software for now.

While streaming:

- **Sound** plays on the default output device. `--audio-buffer <ms>` (default
  30) trades delay for robustness on a jittery network; `--no-audio` turns
  it off.
- **Controllers** (anything SDL3 knows: DualSense, DualShock 4, Xbox, Switch
  Pro, ...) drive the console, including the DualSense touchpad and motion
  sensors. Several controllers act as one. Plug them in before or during
  the stream.
- **Rumble** comes back to the controller. Games that use DualSense haptics
  get them as rumble (true haptics are not supported yet); DualSense
  adaptive triggers, light bar and player lights work.
- **Microphone**: the DualSense mute button switches your default
  microphone on and off for voice chat; `--mic` starts with it on.

On Linux, SDL3 needs read access to the controller's `/dev/hidraw*` node for
rumble, triggers, touchpad and motion (Steam's udev rules, or your
distribution's `game-devices-udev` package, set that up).

```
$ tether discover
NAME         ADDRESS       STATE  PAIRED
Living Room  192.168.1.20  ready  yes
```

To pair, turn the console on and open **Settings > System > Remote Play >
Link Device**; it shows the code for a few minutes.

`tether login` opens Sony's sign-in page in your browser. After you sign in
the browser ends on a blank page; paste that page's address back into the
terminal. Only the numeric account ID is kept. If you already use chiaki-ng,
`tether login --account-id <base64>` takes its account ID directly.

Where things are stored:

- `config.toml` (account ID, paired consoles): `~/.config/tether/` on Linux,
  `%APPDATA%\tether\config\` on Windows.
- Console keys: the system credential store (Windows Credential Manager, or
  the Secret Service, i.e. gnome-keyring or KWallet, on Linux). Without a
  keyring daemon they go to `secrets.toml` next to `config.toml`, readable
  only by you.

## Building

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

### NixOS

The flake has a dev shell with everything, including the Rust toolchain:

```
nix develop
cargo build --release
```

(Needs flakes enabled. With direnv, `echo "use flake" > .envrc && direnv allow`.)

### Arch Linux

```
sudo pacman -S --needed base-devel rust cmake clang pkgconf \
  openssl json-c miniupnpc libevent protobuf python-protobuf ffmpeg \
  sdl3 alsa-lib
cargo build --release
```

Use `rustup` instead of the `rust` package if you prefer; `rust-toolchain.toml`
pins stable.

### Windows

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

## Layout

```
third_party/chiaki-ng/   git submodule, pinned to v1.10.1
crates/chiaki-sys/       builds chiaki-ng's lib/ with CMake, bindgen bindings
crates/rp-core/          safe Rust API: discovery, wake, registration, PSN sign-in,
                         sessions (video, audio, haptics, controller, mic)
crates/tether-core/      app logic: config, paired consoles, secrets, wake-and-wait,
                         audio playback and microphone (Opus + cpal)
crates/desktop/          the `tether` binary: CLI, FFmpeg decoder, video window,
                         SDL3 controllers and rumble
                         (Slint UI and wgpu rendering in milestone 5)
```
