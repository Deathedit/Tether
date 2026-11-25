//! The two plain JNI calls the app makes (UniFFI can't pass Android objects):
//!
//! - `NativeSurface.init(context)` once at app start: gives Rust the JVM and
//!   the application context, which cpal needs for audio (it queries
//!   `AudioManager` through `ndk-context`).
//! - `NativeSurface.set(surface)` when the SurfaceView's surface is created
//!   (and `set(null)` when it's destroyed). Rust turns it into an
//!   `ANativeWindow` that the MediaCodec decoder renders into directly.

#[cfg(target_os = "android")]
mod imp {
    use std::sync::{Mutex, Once};

    use jni_sys::{JNI_OK, JNIEnv, JavaVM, jclass, jobject};
    use ndk::native_window::NativeWindow;

    static WINDOW: Mutex<Option<NativeWindow>> = Mutex::new(None);
    static CONTEXT: Once = Once::new();

    /// The current surface, if one is set. The decoder keeps its own handle
    /// (ANativeWindow is reference counted).
    pub fn current() -> Option<NativeWindow> {
        WINDOW.lock().unwrap().clone()
    }

    /// `io.github.deathedit.tether.NativeSurface.init(Context)`.
    ///
    /// # Safety
    /// Called by the JVM with a valid `env` and the application context.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn Java_io_github_deathedit_tether_NativeSurface_init(
        env: *mut JNIEnv,
        _class: jclass,
        context: jobject,
    ) {
        CONTEXT.call_once(|| {
            // SAFETY: valid env from the JVM; the global ref keeps the
            // application context alive for the whole process, as
            // ndk-context requires.
            unsafe {
                let (Some(get_vm), Some(new_global)) = ((**env).GetJavaVM, (**env).NewGlobalRef)
                else {
                    return;
                };
                let mut vm: *mut JavaVM = std::ptr::null_mut();
                if get_vm(env, &mut vm) != JNI_OK || vm.is_null() {
                    tracing::error!("JNI GetJavaVM failed");
                    return;
                }
                let global = new_global(env, context);
                ndk_context::initialize_android_context(vm.cast(), global.cast());
            }
            tracing::info!("android context set");
        });
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
