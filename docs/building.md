# Building

Clone with submodules:

```
git clone --recursive <repo-url>
# or, in an existing clone:
git submodule update --init --recursive
```

All platforms need Rust (stable), CMake, a C compiler, libclang (for bindgen),
`protoc`, Python 3 with the `protobuf` package (chiaki-ng's nanopb code
generator uses both), and FFmpeg.

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
  fontconfig libxkbcommon vulkan-icd-loader libva
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
location, set `TETHER_VCPKG_INSTALLED` to the triplet directory. Then, from any
terminal (no Developer Prompt needed), point the FFmpeg bindings at it and
build:

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
