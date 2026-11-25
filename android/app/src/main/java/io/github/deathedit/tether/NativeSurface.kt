package io.github.deathedit.tether

import android.content.Context
import android.view.Surface

/**
 * The two plain JNI calls into the Rust core (everything else goes through
 * the UniFFI bindings):
 * - [init] once at start: the app context, which the audio code needs.
 * - [set]: hands the SurfaceView's surface to the MediaCodec decoder, which
 *   renders into it directly. One call when the surface appears and one
 *   (with null) when it goes away; nothing per frame.
 */
object NativeSurface {
    init {
        System.loadLibrary("tether_ffi")
    }

    @JvmStatic
    external fun init(context: Context)

    @JvmStatic
    external fun set(surface: Surface?)
}
