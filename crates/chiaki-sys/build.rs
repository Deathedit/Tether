//! Builds chiaki-ng's core library (`lib/` only, no GUI or CLI) with CMake and
//! generates Rust bindings for it with bindgen.
//!
//! Native dependencies (OpenSSL, json-c, miniupnpc, libevent) come from:
//! - Linux: system packages, found with pkg-config.
//! - Windows: a vcpkg install tree (triplet `x64-windows-static-md`). By default
//!   `<workspace>/vcpkg_installed/x64-windows-static-md`, or set
//!   `TETHER_VCPKG_INSTALLED` to another triplet directory.
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

/// Libraries that chiaki-ng's lib links against, looked up with pkg-config.
const PKG_CONFIG_LIBS: &[&str] = &["libcrypto", "libevent", "json-c", "miniupnpc"];

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
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    if target_os == "windows" && target_env == "msvc" {
        setup_vcpkg(&workspace_dir);
    }

    let build_dir = build_chiaki(&chiaki_dir);
    link_cmake_libs(&build_dir, &target_os);
    let include_paths = link_pkg_config_libs(&target_os);
    link_system_libs(&target_os);
    generate_bindings(&chiaki_dir, &build_dir, &include_paths);
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

fn build_chiaki(chiaki_dir: &Path) -> PathBuf {
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
        ("CHIAKI_ENABLE_ANDROID", "OFF"),
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
        ("CHIAKI_USE_SYSTEM_CURL", "OFF"),
        // nanopb switches MSVC builds to the static CRT (/MT) by default;
        // Rust links the dynamic one (/MD), and mixing them breaks linking.
        ("nanopb_MSVC_STATIC_RUNTIME", "OFF"),
    ] {
        cfg.define(key, value);
    }

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        use_clang_cl(&mut cfg);
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
fn link_cmake_libs(build_dir: &Path, target_os: &str) {
    let mut found = Vec::new();
    collect_static_libs(build_dir, &mut found);

    for wanted in CMAKE_LIBS {
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

fn link_pkg_config_libs(target_os: &str) -> Vec<PathBuf> {
    let windows = target_os == "windows";
    let mut include_paths = Vec::new();
    for lib in PKG_CONFIG_LIBS {
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

fn generate_bindings(chiaki_dir: &Path, build_dir: &Path, include_paths: &[PathBuf]) {
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

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    builder
        .generate()
        .expect("bindgen failed on chiaki-ng headers")
        .write_to_file(&out)
        .expect("failed to write bindings.rs");
}
