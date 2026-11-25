package io.github.deathedit.tether

import android.view.Surface

/**
 * Hands the SurfaceView's surface to the Rust video decoder, which renders
 * into it directly with MediaCodec. One call when the surface appears and
 * one (with null) when it goes away; nothing per frame.
 */
object NativeSurface {
    init {
        System.loadLibrary("tether_ffi")
    }

    @JvmStatic
    external fun set(surface: Surface?)
}
