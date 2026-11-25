package io.github.deathedit.tether

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Color
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.InputType
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
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
 * this SurfaceView and plays the sound; Kotlin forwards the controller,
 * plays rumble and shows a small latency pill.
 */
class StreamActivity : Activity(), SurfaceHolder.Callback, StreamListener {
    companion object {
        const val EXTRA_HOST_ID = "host_id"
        const val EXTRA_NAME = "name"
    }

    private lateinit var status: TextView
    private lateinit var latency: TextView
    private var stream: StreamHandle? = null
    private val gamepad = GamepadInput { input -> stream?.setController(input) }
    private lateinit var rumble: Rumble
    private val ui = Handler(Looper.getMainLooper())
    private val statsTick = object : Runnable {
        override fun run() {
            stream?.let { s ->
                val st = s.stats()
                if (st.latencyMs > 0u) {
                    latency.text = "● ${st.latencyMs} ms"
                    latency.visibility = View.VISIBLE
                }
            }
            ui.postDelayed(this, 1000)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        rumble = Rumble(this)

        val surface = SurfaceView(this)
        surface.holder.addCallback(this)
        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 18f
            gravity = Gravity.CENTER
            text = "Connecting to ${intent.getStringExtra(EXTRA_NAME) ?: "console"}…"
        }
        latency = TextView(this).apply {
            setTextColor(Color.rgb(200, 205, 215))
            textSize = 12f
            setPadding(24, 8, 24, 8)
            background = GradientDrawable().apply {
                cornerRadius = 40f
                setColor(Color.argb(200, 20, 22, 28))
            }
            visibility = View.GONE
        }
        val root = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(surface, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
            addView(status, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
            addView(latency, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.END).apply {
                setMargins(0, 24, 24, 0)
            })
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

    // Controller input: consume gamepad events so e.g. B doesn't act as Back.
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (stream != null && gamepad.isGamepad(event.source) && gamepad.onKey(event)) return true
        return super.dispatchKeyEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (stream != null && gamepad.onMotion(event)) return true
        return super.dispatchGenericMotionEvent(event)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus) gamepad.reset()
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        NativeSurface.set(holder.surface)
        if (stream != null) return
        val hostId = intent.getStringExtra(EXTRA_HOST_ID) ?: return finish()
        val options = StreamOptions(VideoResolution.P720, 60u, VideoCodec.H264, true)
        try {
            stream = core.startStream(hostId, options, this)
            ui.postDelayed(statsTick, 1000)
        } catch (e: TetherException) {
            Toast.makeText(this, e.message, Toast.LENGTH_LONG).show()
            finish()
        }
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        // The decoder renders into this surface; stop before it's gone.
        ui.removeCallbacks(statsTick)
        stream?.stopAndWait(3000u)
        stream = null
        NativeSurface.set(null)
        rumble.stop()
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
                is StreamEvent.Rumble -> rumble.set(event.left.toInt(), event.right.toInt())
                is StreamEvent.Microphone -> event.error?.let { Toast.makeText(this, it, Toast.LENGTH_LONG).show() }
                is StreamEvent.Ended -> {
                    rumble.stop()
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
        ui.removeCallbacks(statsTick)
        stream?.stop()
        rumble.stop()
        super.onDestroy()
    }
}
