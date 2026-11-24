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
            # FFmpeg with VAAPI (nixpkgs builds it with libva).
            ffmpeg
            # Slint finds system fonts through fontconfig.
            fontconfig
          ];

          # The UI loads these at runtime (dlopen) rather than linking them:
          # winit's Wayland/X11 libraries, the Vulkan loader for wgpu, and
          # libva for hardware decoding. On NixOS the GPU drivers themselves
          # (Vulkan ICDs, VAAPI drivers) come from `hardware.graphics`.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              libxkbcommon
              wayland
              xorg.libX11
              xorg.libXcursor
              xorg.libXrandr
              xorg.libXi
              vulkan-loader
              libva
              fontconfig
              freetype
            ]
          );
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
