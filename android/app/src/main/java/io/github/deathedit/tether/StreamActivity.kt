package io.github.deathedit.tether

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Color
import android.os.Bundle
import android.text.InputType
import android.view.Gravity
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.TextView
import android.widget.Toast
import io.github.deathedit.tether.core.StreamEvent
import io.github.deathedit.tether.core.StreamHandle
import io.github.deathedit.tether.core.StreamListener
import io.github.deathedit.tether.core.StreamOptions
import io.github.deathedit.tether.core.TetherException
import io.github.deathedit.tether.core.VideoCodec
import io.github.deathedit.tether.core.VideoResolution

/**
 * Full-screen stream. The Rust core decodes with MediaCodec straight onto
 * this SurfaceView; Kotlin only shows status and asks for the passcode.
 */
class StreamActivity : Activity(), SurfaceHolder.Callback, StreamListener {
    companion object {
        const val EXTRA_HOST_ID = "host_id"
        const val EXTRA_NAME = "name"
    }

    private lateinit var status: TextView
    private var stream: StreamHandle? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        val surface = SurfaceView(this)
        surface.holder.addCallback(this)
        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 18f
            gravity = Gravity.CENTER
            text = "Connecting to ${intent.getStringExtra(EXTRA_NAME) ?: "console"}…"
        }
        val root = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(surface, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
            addView(status, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
        }
        setContentView(root)
        hideSystemBars()
    }

    private fun hideSystemBars() {
        window.insetsController?.let {
            it.hide(WindowInsets.Type.systemBars())
            it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        NativeSurface.set(holder.surface)
        if (stream != null) return
        val hostId = intent.getStringExtra(EXTRA_HOST_ID) ?: return finish()
        val options = StreamOptions(VideoResolution.P720, 60u, VideoCodec.H264)
        try {
            stream = core.startStream(hostId, options, this)
        } catch (e: TetherException) {
            Toast.makeText(this, e.message, Toast.LENGTH_LONG).show()
            finish()
        }
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        // The decoder renders into this surface; stop before it's gone.
        stream?.stopAndWait(3000u)
        stream = null
        NativeSurface.set(null)
        finish()
    }

    /** Called on a background thread by the Rust core. */
    override fun onEvent(event: StreamEvent) {
        runOnUiThread {
            when (event) {
                is StreamEvent.Waking -> status.text = "Waking the console…"
                is StreamEvent.Connecting -> status.text = "Connecting…"
                is StreamEvent.Connected -> status.text = "Starting video…"
                is StreamEvent.VideoStarted -> status.visibility = View.GONE
                is StreamEvent.LoginPinRequest -> askPasscode(event.previousIncorrect)
                is StreamEvent.Ended -> {
                    if (event.error) Toast.makeText(this, event.reason, Toast.LENGTH_LONG).show()
                    finish()
                }
            }
        }
    }

    private fun askPasscode(retry: Boolean) {
        val input = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD
        }
        AlertDialog.Builder(this)
            .setTitle(if (retry) "Wrong passcode, try again" else "Remote Play passcode")
            .setView(input)
            .setPositiveButton("OK") { _, _ -> stream?.setLoginPin(input.text.toString()) }
            .setNegativeButton("Cancel") { _, _ -> stream?.stop() }
            .show()
    }

    override fun onDestroy() {
        stream?.stop()
        super.onDestroy()
    }
}
