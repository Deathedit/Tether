//! Builds chiaki-ng's core library (`lib/` only, no GUI or CLI) with CMake and
//! generates Rust bindings for it with bindgen.
//!
//! Native dependencies (OpenSSL, json-c, miniupnpc, libevent) come from:
//! - Linux: system packages, found with pkg-config.
//! - Windows: a vcpkg install tree (triplet `x64-windows-static-md`). By default
//!   `<workspace>/vcpkg_installed/x64-windows-static-md`, or set
//!   `TETHER_VCPKG_INSTALLED` to another triplet directory.
//! - Android: chiaki-ng's own CMake downloads and builds OpenSSL, json-c and
//!   miniupnpc for the target (as its Gradle build does); libevent isn't used.
//!   Needs the NDK (`ANDROID_NDK_HOME`, as set by cargo-ndk) and a Unix-like
//!   build host, since OpenSSL builds with its own Configure script and make.
//!
//! curl is the copy bundled with chiaki-ng, except on Linux when the system
//! has curl 8.11 or newer (see `use_system_curl`).
//!
//! Build tools needed on every platform: CMake, a C compiler, Python 3 with the
//! `protobuf` package, `protoc` (for nanopb) and libclang (for bindgen).

use std::env;
use std::path::{Path, PathBuf};

/// Static libraries produced by chiaki-ng's CMake build, in link order.
/// Each entry is the file stem without the platform's `lib` prefix.
const CMAKE_LIBS: &[&str] = &[
    "chiaki",
    "libcurl",
    "protobuf-nanopb",
    "jerasure",
    "gf_complete",
];

/// On Android, the dependencies chiaki-ng's CMake built itself, in link order.
const ANDROID_DEP_LIBS: &[&str] = &["json-c", "miniupnpc", "ssl", "crypto"];

/// Lowest Android API level Tether supports (Android 11), unless cargo-ndk
/// passes another.
const ANDROID_MIN_API: &str = "30";

/// Libraries that chiaki-ng's lib links against, looked up with pkg-config.
/// `libssl` is needed on Linux, where the bundled curl uses OpenSSL for TLS
/// (on Windows it uses Schannel, and linking libssl there is harmless).
const PKG_CONFIG_LIBS: &[&str] = &["libssl", "libcrypto", "libevent", "json-c", "miniupnpc"];

/// The first curl with WebSocket support built in by default, which chiaki-ng
/// needs from a system curl.
const SYSTEM_CURL_MIN: &str = "8.11";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let chiaki_dir = workspace_dir.join("third_party").join("chiaki-ng");

    if !chiaki_dir.join("lib").join("CMakeLists.txt").exists() {
        panic!(
            "chiaki-ng sources not found at {}. Run `git submodule update --init --recursive`.",
            chiaki_dir.display()
        );
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        chiaki_dir.join("lib").display()
    );
    for var in [
        "TETHER_VCPKG_INSTALLED",
        "PROTOC",
        "PYTHON",
        "PKG_CONFIG_PATH",
        "ANDROID_NDK_HOME",
        "ANDROID_NDK_ROOT",
        "CARGO_NDK_ANDROID_PLATFORM",
        "TETHER_SYSTEM_CURL",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    if target_os == "windows" && target_env == "msvc" {
        setup_vcpkg(&workspace_dir);
    }
    let android = (target_os == "android").then(Android::from_env);

    let system_curl = use_system_curl(&target_os);

    let build_dir = build_chiaki(&chiaki_dir, android.as_ref(), system_curl);
    let cmake_libs: Vec<&str> = CMAKE_LIBS
        .iter()
        .copied()
        .filter(|lib| !(system_curl && *lib == "libcurl"))
        .collect();
    link_cmake_libs(&build_dir, &target_os, &cmake_libs);
    let include_paths = if android.is_some() {
        link_cmake_libs(&build_dir, &target_os, ANDROID_DEP_LIBS);
        Vec::new()
    } else {
        let mut libs = PKG_CONFIG_LIBS.to_vec();
        if system_curl {
            libs.push("libcurl");
        }
        link_pkg_config_libs(&target_os, &libs)
    };
    link_system_libs(&target_os);
    generate_bindings(&chiaki_dir, &build_dir, &include_paths, android.as_ref());
}

/// Whether to build chiaki-ng against the system's libcurl instead of the
/// copy it bundles (curl 8.11.1, which doesn't compile against OpenSSL 4, as
/// on Arch). Linux uses the system one when pkg-config finds a new enough
/// libcurl; `TETHER_SYSTEM_CURL=1` or `=0` forces the choice.
fn use_system_curl(target_os: &str) -> bool {
    match env::var("TETHER_SYSTEM_CURL").as_deref() {
        Ok("1") => return true,
        Ok("0") => return false,
        _ => {}
    }
    target_os == "linux"
        && pkg_config::Config::new()
            .atleast_version(SYSTEM_CURL_MIN)
            .cargo_metadata(false)
            .probe("libcurl")
            .is_ok()
}

/// Android NDK settings for an Android target.
struct Android {
    ndk: PathBuf,
    /// NDK ABI name: arm64-v8a, x86_64, ...
    abi: &'static str,
    api: String,
}

impl Android {
    fn from_env() -> Self {
        let ndk = ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK"]
            .iter()
            .find_map(env::var_os)
            .map(PathBuf::from)
            .expect("Android NDK not found: set ANDROID_NDK_HOME (cargo-ndk does this)");
        let abi = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
            "aarch64" => "arm64-v8a",
            "x86_64" => "x86_64",
            "arm" => "armeabi-v7a",
            "x86" => "x86",
            other => panic!("unsupported Android architecture {other}"),
        };
        let api = env::var("CARGO_NDK_ANDROID_PLATFORM")
            .ok()
            .filter(|p| p.parse::<u32>().is_ok_and(|n| n >= 30))
            .unwrap_or_else(|| ANDROID_MIN_API.to_owned());
        Self { ndk, abi, api }
    }

    /// The NDK's sysroot for the build host (headers for bindgen).
    fn sysroot(&self) -> PathBuf {
        let host = if cfg!(target_os = "windows") {
            "windows-x86_64"
        } else if cfg!(target_os = "macos") {
            "darwin-x86_64"
        } else {
            "linux-x86_64"
        };
        self.ndk
            .join("toolchains/llvm/prebuilt")
            .join(host)
            .join("sysroot")
    }
}

/// Points pkg-config and CMake at the vcpkg install tree.
fn setup_vcpkg(workspace_dir: &Path) {
    let installed = env::var_os("TETHER_VCPKG_INSTALLED")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_dir
                .join("vcpkg_installed")
                .join("x64-windows-static-md")
        });

    if !installed.join("include").join("openssl").exists() {
        panic!(
            "vcpkg packages not found in {}.\n\
             From the repository root run:\n    \
             vcpkg install --triplet x64-windows-static-md\n\
             or set TETHER_VCPKG_INSTALLED to an installed triplet directory.",
            installed.display()
        );
    }

    let pkgconf = installed.join("tools").join("pkgconf").join("pkgconf.exe");
    let pc_path = installed.join("lib").join("pkgconfig");
    // SAFETY: build scripts are single-threaded at this point.
    unsafe {
        env::set_var("PKG_CONFIG", &pkgconf);
        env::set_var("PKG_CONFIG_PATH", &pc_path);
        env::set_var("TETHER_VCPKG_INSTALLED", &installed);
    }
}

fn build_chiaki(chiaki_dir: &Path, android: Option<&Android>, system_curl: bool) -> PathBuf {
    let mut cfg = cmake::Config::new(chiaki_dir);

    // Always build Release: a Debug build on MSVC would link the debug CRT,
    // which Rust binaries don't use.
    cfg.profile("Release");
    cfg.build_target("chiaki-lib");

    // On Windows chiaki-ng patches a copy of curl with `git apply` run from the
    // build dir. Our build dir is inside the Tether repo, and inside a repo
    // `git apply` silently skips paths outside it, so the patch would be lost.
    // Stop git from finding the enclosing repo.
    cfg.env("GIT_CEILING_DIRECTORIES", env::var_os("OUT_DIR").unwrap());

    for (key, value) in [
        ("CHIAKI_ENABLE_TESTS", "OFF"),
        ("CHIAKI_ENABLE_CLI", "OFF"),
        ("CHIAKI_ENABLE_GUI", "OFF"),
        ("CHIAKI_ENABLE_BOREALIS", "OFF"),
        ("CHIAKI_ENABLE_SETSU", "OFF"),
        ("CHIAKI_ENABLE_STEAMDECK_NATIVE", "OFF"),
        ("CHIAKI_ENABLE_STEAM_SHORTCUT", "OFF"),
        ("CHIAKI_ENABLE_SPEEX", "OFF"),
        ("CHIAKI_ENABLE_FFMPEG_DECODER", "OFF"),
        ("CHIAKI_ENABLE_PI_DECODER", "OFF"),
        ("CHIAKI_GUI_ENABLE_SDL_GAMECONTROLLER", "OFF"),
        // Tether decodes Opus itself (opus crate), so chiaki hands us raw frames.
        ("CHIAKI_LIB_ENABLE_OPUS", "OFF"),
        ("CHIAKI_USE_SYSTEM_JERASURE", "OFF"),
        ("CHIAKI_USE_SYSTEM_NANOPB", "OFF"),
        // nanopb switches MSVC builds to the static CRT (/MT) by default;
        // Rust links the dynamic one (/MD), and mixing them breaks linking.
        ("nanopb_MSVC_STATIC_RUNTIME", "OFF"),
        // The bundled curl only does plain HTTP(S) for chiaki. Without these
        // it picks up whatever optional libraries the build machine has (e.g.
        // zlib in a Nix shell) and then needs them at link time.
        ("CURL_ZLIB", "OFF"),
        ("CURL_BROTLI", "OFF"),
        ("CURL_ZSTD", "OFF"),
        ("USE_NGHTTP2", "OFF"),
        ("USE_LIBIDN2", "OFF"),
        ("CURL_USE_LIBPSL", "OFF"),
        ("CURL_USE_LIBSSH2", "OFF"),
    ] {
        cfg.define(key, value);
    }
    cfg.define(
        "CHIAKI_USE_SYSTEM_CURL",
        if system_curl { "ON" } else { "OFF" },
    );

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        use_clang_cl(&mut cfg);
    }

    match android {
        Some(android) => setup_android(&mut cfg, android),
        None => {
            cfg.define("CHIAKI_ENABLE_ANDROID", "OFF");
        }
    }

    if let Some(installed) = env::var_os("TETHER_VCPKG_INSTALLED") {
        let installed = PathBuf::from(installed);
        cfg.define("CMAKE_PREFIX_PATH", &installed);
        cfg.define("PKG_CONFIG_EXECUTABLE", env::var_os("PKG_CONFIG").unwrap());
        cfg.env("PKG_CONFIG_PATH", env::var_os("PKG_CONFIG_PATH").unwrap());
        cfg.define("OPENSSL_ROOT_DIR", &installed);
    }
    if let Some(protoc) = env::var_os("PROTOC") {
        cfg.define("PROTOC", protoc);
    }
    if let Some(python) = env::var_os("PYTHON") {
        cfg.define("PYTHON_EXECUTABLE", python);
    }

    cfg.build().join("build")
}

/// Cross-compiles with the NDK's CMake toolchain, the way chiaki-ng's own
/// Gradle build configures it.
fn setup_android(cfg: &mut cmake::Config, android: &Android) {
    cfg.define(
        "CMAKE_TOOLCHAIN_FILE",
        android.ndk.join("build/cmake/android.toolchain.cmake"),
    );
    cfg.define("ANDROID_ABI", android.abi);
    cfg.define("ANDROID_PLATFORM", format!("android-{}", android.api));
    for (key, value) in [
        // Android code paths (no libevent; Android networking in holepunch).
        ("CHIAKI_ENABLE_ANDROID", "ON"),
        // Download and build these for the target instead of using system ones.
        ("CHIAKI_LIB_OPENSSL_EXTERNAL_PROJECT", "ON"),
        ("CHIAKI_LIB_JSONC_EXTERNAL_PROJECT", "ON"),
        ("CHIAKI_LIB_MINIUPNPC_EXTERNAL_PROJECT", "ON"),
    ] {
        cfg.define(key, value);
    }

    // CHIAKI_ENABLE_ANDROID also configures chiaki-ng's own JNI library,
    // which needs Google's `oboe` package from Gradle. We never build that
    // target (only chiaki-lib), so a stub package satisfies its find_package.
    let stub = PathBuf::from(env::var("OUT_DIR").unwrap()).join("oboe-stub");
    std::fs::create_dir_all(&stub).unwrap();
    std::fs::write(
        stub.join("oboeConfig.cmake"),
        "# Stub so chiaki-ng's android/app CMake configures; never linked.\n\
         if(NOT TARGET oboe::oboe)\n  add_library(oboe::oboe INTERFACE IMPORTED)\nendif()\n",
    )
    .unwrap();
    cfg.define("oboe_DIR", &stub);
}

/// chiaki-ng uses C99 variable-length arrays, which MSVC's `cl.exe` rejects
/// (upstream builds Windows with MinGW). clang-cl accepts them and still
/// produces MSVC-ABI objects that link into a Rust `*-windows-msvc` binary.
///
/// Builds with Ninja + clang-cl, taking the MSVC environment (INCLUDE, LIB,
/// PATH) from the `cc` crate, so no Developer Prompt is needed. CMake, Ninja
/// and clang-cl are found on PATH, then in the Visual Studio and LLVM install
/// folders. Override with the `CMAKE`, `NINJA` or `CLANG_CL` env vars.
fn use_clang_cl(cfg: &mut cmake::Config) {
    let target = env::var("TARGET").unwrap();
    let cl = cc::windows_registry::find_tool(&target, "cl.exe")
        .expect("MSVC (Visual Studio 2022 or Build Tools, C++ workload) not found");
    for (key, value) in cl.get_envs() {
        cfg.env(key, value);
    }
    // <VS>\VC\Tools\MSVC\<ver>\bin\Hostx64\x64\cl.exe
    let vs_root = cl.path().ancestors().nth(8).map(Path::to_path_buf);
    let vs_cmake = vs_root
        .as_ref()
        .map(|vs| vs.join(r"Common7\IDE\CommonExtensions\Microsoft\CMake"));

    let program_files = env::var_os("ProgramFiles").map(PathBuf::from);
    let clang_cl = find_program(
        "CLANG_CL",
        "clang-cl.exe",
        [
            vs_root.as_ref().map(|vs| vs.join(r"VC\Tools\Llvm\x64\bin")),
            program_files.as_ref().map(|p| p.join(r"LLVM\bin")),
        ],
    )
    .expect("clang-cl not found. Install LLVM (`winget install LLVM.LLVM`) or set CLANG_CL.");
    let ninja = find_program(
        "NINJA",
        "ninja.exe",
        [vs_cmake.as_ref().map(|c| c.join("Ninja"))],
    )
    .expect("ninja not found. It ships with Visual Studio's CMake component; or set NINJA.");
    if env::var_os("CMAKE").is_none() {
        if let Some(cmake) = find_program(
            "CMAKE",
            "cmake.exe",
            [vs_cmake.as_ref().map(|c| c.join(r"CMake\bin"))],
        ) {
            // SAFETY: build scripts are single-threaded; cmake-rs reads CMAKE.
            unsafe { env::set_var("CMAKE", cmake) };
        }
    }

    let fwd = |p: &Path| p.to_string_lossy().replace('\\', "/");
    cfg.generator("Ninja");
    cfg.define("CMAKE_MAKE_PROGRAM", fwd(&ninja));
    cfg.define("CMAKE_C_COMPILER", fwd(&clang_cl));
    cfg.define("CMAKE_CXX_COMPILER", fwd(&clang_cl));
    // gf-complete calls the `__cpuidex` intrinsic under `_MSC_VER` without
    // including <intrin.h>; cl.exe tolerates that, clang-cl does not.
    cfg.cflag("-FIintrin.h");
}

/// Looks for a program: `env_var` override, then PATH, then `extra_dirs`.
fn find_program<const N: usize>(
    env_var: &str,
    exe: &str,
    extra_dirs: [Option<PathBuf>; N],
) -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed={env_var}");
    if let Some(path) = env::var_os(env_var) {
        return Some(PathBuf::from(path));
    }
    let path_dirs = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    path_dirs
        .into_iter()
        .chain(extra_dirs.into_iter().flatten())
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

/// Finds the static libraries chiaki-ng's build produced and links them.
fn link_cmake_libs(build_dir: &Path, target_os: &str, libs: &[&str]) {
    let mut found = Vec::new();
    collect_static_libs(build_dir, &mut found);

    for wanted in libs {
        let hit = found.iter().find(|(stem, _)| {
            let stem = if target_os == "windows" {
                stem.as_str()
            } else {
                stem.strip_prefix("lib").unwrap_or(stem)
            };
            let wanted = if target_os == "windows" {
                wanted
            } else {
                wanted.strip_prefix("lib").unwrap_or(wanted)
            };
            stem == wanted
        });
        let Some((stem, dir)) = hit else {
            panic!(
                "chiaki-ng build did not produce the `{wanted}` static library (searched {})",
                build_dir.display()
            );
        };
        let name = if target_os == "windows" {
            stem.as_str()
        } else {
            stem.strip_prefix("lib").unwrap_or(stem)
        };
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static={name}");
    }
}

fn collect_static_libs(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // FetchContent and CMake's own scratch dirs never hold our libs.
            if path.file_name().is_some_and(|n| n == "CMakeFiles") {
                continue;
            }
            collect_static_libs(&path, out);
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("lib" | "a")) {
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            out.push((stem, path.parent().unwrap().to_path_buf()));
        }
    }
}

fn link_pkg_config_libs(target_os: &str, libs: &[&str]) -> Vec<PathBuf> {
    let windows = target_os == "windows";
    let mut include_paths = Vec::new();
    for lib in libs {
        let probe = pkg_config::Config::new()
            // vcpkg's static-md triplet only ships static libraries.
            .statik(windows)
            .cargo_metadata(false)
            .probe(lib)
            .unwrap_or_else(|e| panic!("pkg-config could not find `{lib}`: {e}"));

        for dir in &probe.link_paths {
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
        for name in &probe.libs {
            // Some vcpkg .pc files name a library that is installed with a
            // `lib` prefix (miniupnpc.pc says -lminiupnpc, the file is
            // libminiupnpc.lib), so match names against the real files.
            let name = if windows {
                resolve_msvc_lib_name(name, &probe.link_paths)
            } else {
                name.clone()
            };
            println!("cargo:rustc-link-lib={name}");
        }
        include_paths.extend(probe.include_paths);
    }
    include_paths
}

fn resolve_msvc_lib_name(name: &str, dirs: &[PathBuf]) -> String {
    let exists = |file: &str| dirs.iter().any(|d| d.join(file).is_file());
    if !exists(&format!("{name}.lib")) && exists(&format!("lib{name}.lib")) {
        format!("lib{name}")
    } else {
        name.to_owned()
    }
}

fn link_system_libs(target_os: &str) {
    let libs: &[&str] = match target_os {
        "windows" => &[
            "ws2_32", "wsock32", "bcrypt", "iphlpapi", "crypt32", "secur32", "advapi32", "user32",
        ],
        "android" => &["log"],
        _ => &["pthread"],
    };
    for lib in libs {
        println!("cargo:rustc-link-lib={lib}");
    }
}

fn generate_bindings(
    chiaki_dir: &Path,
    build_dir: &Path,
    include_paths: &[PathBuf],
    android: Option<&Android>,
) {
    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!(
            "-I{}",
            chiaki_dir.join("lib").join("include").display()
        ))
        // Generated config.h
        .clang_arg(format!(
            "-I{}",
            build_dir.join("lib").join("include").display()
        ))
        .allowlist_function("chiaki_.*")
        .allowlist_type("Chiaki.*|chiaki_.*")
        .allowlist_var("CHIAKI_.*")
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .derive_default(true)
        .layout_tests(false)
        .rust_edition(bindgen::RustEdition::Edition2024)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    for path in include_paths {
        builder = builder.clang_arg(format!("-I{}", path.display()));
    }
    if let Some(android) = android {
        let target = env::var("TARGET").unwrap();
        builder = builder
            .clang_arg(format!("--target={target}{}", android.api))
            .clang_arg(format!("--sysroot={}", android.sysroot().display()));
    }

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    builder
        .generate()
        .expect("bindgen failed on chiaki-ng headers")
        .write_to_file(&out)
        .expect("failed to write bindings.rs");
}
