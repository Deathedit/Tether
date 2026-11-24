{
  description = "Tether: Remote Play client (works with PS5)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            cmake
            pkg-config
            # nanopb's generator (run by chiaki-ng's CMake) needs protoc and
            # Python with the protobuf package.
            protobuf
            (python3.withPackages (ps: [ ps.protobuf ]))
            # Sets LIBCLANG_PATH and the clang include flags bindgen needs.
            rustPlatform.bindgenHook
          ];

          buildInputs = with pkgs; [
            openssl
            json_c
            miniupnpc
            libevent
            ffmpeg
            # Gamepads (SDL3) and sound (cpal uses ALSA, which PipeWire and
            # PulseAudio serve). libopus is built from source by the opus crate.
            sdl3
            alsa-lib
          ];

          # winit and softbuffer load these at runtime (dlopen) rather than
          # linking them, so they must be findable when `tether stream` runs.
          # SDL3 dlopens libudev to find controllers.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              libxkbcommon
              wayland
              libx11
              libxcursor
              libxrandr
              libxi
              udev
            ]
          );
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
