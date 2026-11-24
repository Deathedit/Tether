# Tether

A Remote Play client for Linux, Windows and Android (AYN Thor first), written
in Rust on top of [chiaki-ng](https://github.com/streetpea/chiaki-ng). Works
with PS5. Licensed AGPL-3.0.

See [STACK.md](STACK.md) for the architecture and milestones.

## Status

Milestone 1 (discover): `tether discover` lists consoles on the LAN, and
`tether wake` wakes one from rest mode.

```
$ tether discover
NAME         ADDRESS       STATE
Living Room  192.168.1.20  ready
```

`tether wake <host> --regist-key <hex>` needs the registration key that
pairing produces. Pairing comes in milestone 2; until then you can take the key
from an existing chiaki-ng setup.

## Building

Clone with submodules:

```
git clone --recursive <repo-url>
# or, in an existing clone:
git submodule update --init --recursive
```

All platforms need Rust (stable), CMake, a C compiler, libclang (for bindgen),
`protoc`, and Python 3 with the `protobuf` package (chiaki-ng's nanopb code
generator uses both).

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
  openssl json-c miniupnpc libevent protobuf python-protobuf
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

This fills `vcpkg_installed/`. To use another location, set
`TETHER_VCPKG_INSTALLED` to the triplet directory. Then, from any terminal (no
Developer Prompt needed):

```
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
crates/rp-core/          safe Rust API: discovery, wake (more to come)
crates/desktop/          the `tether` binary (CLI now, Slint UI later)
```
