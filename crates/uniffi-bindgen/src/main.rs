//! `cargo run -p uniffi-bindgen -- generate --library <libtether_ffi.so>
//! --language kotlin --out-dir <dir>`
//!
//! Reads the UniFFI metadata embedded in the built library, so it doesn't
//! need to build tether-ffi (or chiaki) for the host.

fn main() {
    uniffi::uniffi_bindgen_main()
}
