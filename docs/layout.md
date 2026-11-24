# Repository layout

```
third_party/chiaki-ng/   git submodule, pinned to v1.10.1
crates/chiaki-sys/       builds chiaki-ng's lib/ with CMake, bindgen bindings
crates/rp-core/          safe Rust API: discovery, wake, registration, PSN sign-in
crates/tether-core/      app logic: config, paired consoles, secrets, wake-and-wait
crates/desktop/          the `tether` binary: CLI, FFmpeg decoder, video window
                         (Slint UI and wgpu rendering in milestone 5)
```
