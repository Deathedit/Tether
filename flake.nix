{
  description = "Tether: Remote Play client (works with PS5)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # chiaki-ng is a git submodule; the package needs its sources.
    self.submodules = true;
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

      # Build tools: CMake for chiaki-ng, protoc + Python protobuf for its
      # nanopb generator, and bindgen's libclang setup.
      nativeDeps =
        pkgs: with pkgs; [
          cmake
          pkg-config
          protobuf
          (python3.withPackages (ps: [ ps.protobuf ]))
          rustPlatform.bindgenHook
        ];

      # Linked libraries.
      buildDeps =
        pkgs: with pkgs; [
          openssl
          json_c
          miniupnpc
          libevent
          # FFmpeg with VAAPI (nixpkgs builds it with libva).
          ffmpeg
          # Slint finds system fonts through fontconfig.
          fontconfig
          # Gamepads (SDL3) and sound (cpal uses ALSA, which PipeWire and
          # PulseAudio serve). libopus is built from source by the opus crate.
          sdl3
          alsa-lib
        ];

      # Loaded at runtime (dlopen) rather than linked: winit's Wayland/X11
      # libraries, the Vulkan loader for wgpu, libva for hardware decoding,
      # and libudev, which SDL3 uses to find controllers. On NixOS the GPU
      # drivers themselves (Vulkan ICDs, VAAPI drivers) come from
      # `hardware.graphics`.
      runtimeDeps =
        pkgs: with pkgs; [
          libxkbcommon
          wayland
          libx11
          libxcursor
          libxrandr
          libxi
          vulkan-loader
          libva
          fontconfig
          freetype
          udev
        ];
    in
    {
      packages = forAllSystems (pkgs: rec {
        tether = pkgs.rustPlatform.buildRustPackage {
          pname = "tether";
          inherit version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "-p"
            "tether-desktop"
          ];

          nativeBuildInputs = nativeDeps pkgs;
          buildInputs = buildDeps pkgs;
          # CMake is for chiaki-sys's build script, not for this package.
          dontUseCmakeConfigure = true;
          # CI runs the tests; some need local network ports.
          doCheck = false;

          postInstall = ''
            install -Dm644 packaging/linux/tether.desktop $out/share/applications/tether.desktop
            install -Dm644 packaging/linux/tether.svg $out/share/icons/hicolor/scalable/apps/tether.svg
          '';
          postFixup = ''
            patchelf --add-rpath ${pkgs.lib.makeLibraryPath (runtimeDeps pkgs)} $out/bin/tether
          '';

          meta = {
            description = "Remote Play client (works with PS5), built on chiaki-ng";
            homepage = "https://github.com/Deathedit/Tether";
            license = pkgs.lib.licenses.agpl3Only;
            mainProgram = "tether";
            platforms = pkgs.lib.platforms.linux;
          };
        };
        default = tether;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs =
            (nativeDeps pkgs)
            ++ (with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
            ]);
          buildInputs = buildDeps pkgs;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeDeps pkgs);
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
