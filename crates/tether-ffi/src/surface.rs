//! The surface video is drawn on.
//!
//! UniFFI can't pass Android objects, so the SurfaceView's `Surface` comes
//! through one plain JNI call: Kotlin's `NativeSurface.set(surface)` when the
//! surface is created (and `set(null)` when it's destroyed). Rust turns it
//! into an `ANativeWindow` that the MediaCodec decoder renders into directly.

#[cfg(target_os = "android")]
mod imp {
    use std::sync::Mutex;

    use jni_sys::{JNIEnv, jclass, jobject};
    use ndk::native_window::NativeWindow;

    static WINDOW: Mutex<Option<NativeWindow>> = Mutex::new(None);

    /// The current surface, if one is set. The decoder keeps its own handle
    /// (ANativeWindow is reference counted).
    pub fn current() -> Option<NativeWindow> {
        WINDOW.lock().unwrap().clone()
    }

    /// `io.github.deathedit.tether.NativeSurface.set(Surface?)`.
    ///
    /// # Safety
    /// Called by the JVM with a valid `env` and a `Surface` or null.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn Java_io_github_deathedit_tether_NativeSurface_set(
        env: *mut JNIEnv,
        _class: jclass,
        surface: jobject,
    ) {
        let window = if surface.is_null() {
            None
        } else {
            // SAFETY: valid env and Surface from the JVM.
            unsafe { NativeWindow::from_surface(env, surface) }
        };
        tracing::info!(
            "surface {}",
            if window.is_some() { "set" } else { "cleared" }
        );
        *WINDOW.lock().unwrap() = window;
    }
}

#[cfg(target_os = "android")]
pub use imp::*;
