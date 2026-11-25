package io.github.deathedit.tether

import android.content.Context
import io.github.deathedit.tether.core.StreamOptions
import io.github.deathedit.tether.core.VideoCodec
import io.github.deathedit.tether.core.VideoResolution

/** Stream settings, kept in SharedPreferences. */
class Settings(context: Context) {
    private val prefs = context.getSharedPreferences("tether-settings", Context.MODE_PRIVATE)

    var resolution: VideoResolution
        get() = runCatching { VideoResolution.valueOf(prefs.getString("resolution", null)!!) }
            .getOrDefault(VideoResolution.P720)
        set(v) = prefs.edit().putString("resolution", v.name).apply()

    var fps: Int
        get() = prefs.getInt("fps", 60)
        set(v) = prefs.edit().putInt("fps", v).apply()

    var codec: VideoCodec
        get() = runCatching { VideoCodec.valueOf(prefs.getString("codec", null)!!) }
            .getOrDefault(VideoCodec.H264)
        set(v) = prefs.edit().putString("codec", v.name).apply()

    var audio: Boolean
        get() = prefs.getBoolean("audio", true)
        set(v) = prefs.edit().putBoolean("audio", v).apply()

    /** Show the latency pill over the game on the top screen. */
    var showOverlay: Boolean
        get() = prefs.getBoolean("show_overlay", true)
        set(v) = prefs.edit().putBoolean("show_overlay", v).apply()

    /** The console picked on the home screen last time. */
    var lastConsole: String?
        get() = prefs.getString("last_console", null)
        set(v) = prefs.edit().putString("last_console", v).apply()

    fun streamOptions() = StreamOptions(resolution, fps.toUInt(), codec, audio)

    /** "720p · 60 fps · H.264", for subtitles. */
    fun summary(): String = "${resolutionLabel(resolution)} · $fps fps · ${codecLabel(codec)}"

    companion object {
        fun resolutionLabel(r: VideoResolution) = when (r) {
            VideoResolution.P360 -> "360p"
            VideoResolution.P540 -> "540p"
            VideoResolution.P720 -> "720p"
            VideoResolution.P1080 -> "1080p"
        }

        fun codecLabel(c: VideoCodec) = when (c) {
            VideoCodec.H264 -> "H.264"
            VideoCodec.H265 -> "HEVC"
        }
    }
}
