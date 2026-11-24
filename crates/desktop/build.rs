//! Windows system libraries that vcpkg's static FFmpeg needs but
//! ffmpeg-sys-next doesn't link when FFMPEG_DIR is used.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        for lib in [
            "mfplat", "mfuuid", "strmiids", "ole32", "bcrypt", "user32", "secur32", "ws2_32",
        ] {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
}
