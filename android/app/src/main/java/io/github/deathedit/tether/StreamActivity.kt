package io.github.deathedit.tether

import android.Manifest
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Bundle
import android.text.InputType
import android.view.Gravity
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.ComposeView
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import io.github.deathedit.tether.core.StreamEvent
import io.github.deathedit.tether.core.StreamHandle
import io.github.deathedit.tether.core.StreamListener
import io.github.deathedit.tether.core.StreamStats
import io.github.deathedit.tether.core.TetherException
import io.github.deathedit.tether.core.buttonBits
import io.github.deathedit.tether.ui.Palette
import io.github.deathedit.tether.ui.SessionCompanion
import io.github.deathedit.tether.ui.StatusDot
import io.github.deathedit.tether.ui.TetherTheme
import io.github.deathedit.tether.ui.TouchpadSink
import io.github.deathedit.tether.ui.Type
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The stream. Top screen: game video only (MediaCodec draws straight onto
 * the SurfaceView), plus a small latency pill. Bottom screen (Thor): live
 * numbers, touchpad and quick actions. Phones get that as an overlay.
 */
class StreamActivity : ComponentActivity(), SurfaceHolder.Callback, StreamListener {
    companion object {
        const val EXTRA_HOST_ID = "host_id"
        const val EXTRA_NAME = "name"
        private const val TAP_MS = 100L
    }

    private val settings by lazy { Settings(this) }
    private val secondScreen by lazy { SecondScreen(this) }
    private val bits = buttonBits()
    private var stream: StreamHandle? = null
    private val gamepad = GamepadInput { input -> stream?.setController(input) }
    private lateinit var rumble: Rumble
    private var statsJob: Job? = null

    private var status by mutableStateOf<String?>(null)
    private var stats by mutableStateOf<StreamStats?>(null)
    private var micOn by mutableStateOf(false)
    private var overlayOpen by mutableStateOf(false)

    private val askMic = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) stream?.setMicrophone(true)
        else Toast.makeText(this, "The microphone needs permission.", Toast.LENGTH_LONG).show()
    }

    private val touchpad = object : TouchpadSink {
        override fun down(x: Float, y: Float) = stream?.touchDown(x, y) ?: -1
        override fun move(id: Int, x: Float, y: Float) { stream?.touchMove(id, x, y) }
        override fun up(id: Int) { stream?.touchUp(id) }
        override fun tap(id: Int) {
            lifecycleScope.launch {
                stream?.touchpadClick(true)
                delay(60)
                stream?.touchpadClick(false)
                stream?.touchUp(id)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        rumble = Rumble(this)
        status = "Connecting to ${intent.getStringExtra(EXTRA_NAME) ?: "console"}…"

        val surface = SurfaceView(this)
        surface.holder.addCallback(this)
        val overlay = ComposeView(this).apply { setContent { TetherTheme { Overlay() } } }
        setContentView(FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(surface, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))
            addView(overlay, FrameLayout.LayoutParams(-1, -1))
        })
        hideSystemBars()
    }

    @Composable
    private fun Overlay() {
        Box(Modifier.fillMaxSize()) {
            status?.let {
                Text(it, style = Type.title, modifier = Modifier.align(Alignment.Center))
            }
            // Latency pill. On a phone it also opens the companion overlay.
            val ms = stats?.latencyMs ?: 0u
            if (status == null && (ms > 0u || !secondScreen.available)) {
                Row(
                    Modifier.align(Alignment.TopEnd).padding(16.dp)
                        .clip(RoundedCornerShape(20.dp))
                        .background(Palette.background.copy(alpha = 0.8f))
                        .clickable(enabled = !secondScreen.available) { overlayOpen = !overlayOpen }
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    StatusDot(if (ms in 1u..40u) Palette.ready else Palette.rest, 6)
                    Spacer(Modifier.width(8.dp))
                    Text(if (ms > 0u) "$ms ms" else "Menu", style = Type.monoSmall)
                }
            }
            if (overlayOpen && !secondScreen.available) {
                Box(
                    Modifier.align(Alignment.CenterEnd).fillMaxHeight().fillMaxWidth(0.45f)
                        .background(Palette.background.copy(alpha = 0.92f)),
                ) { SessionPanel() }
            }
        }
    }

    @Composable
    private fun SessionPanel() {
        SessionCompanion(
            stats = stats,
            micOn = micOn,
            touchpad = touchpad,
            onHome = { tapButton(bits.home) },
            onMic = ::toggleMic,
            onCapture = { tapButton(bits.share) },
            onDisconnect = { stream?.stop() },
        )
    }

    private fun tapButton(bit: UInt) {
        lifecycleScope.launch {
            gamepad.setExtra(bit, true)
            delay(TAP_MS)
            gamepad.setExtra(bit, false)
        }
    }

    private fun toggleMic() {
        val s = stream ?: return
        if (micOn) {
            s.setMicrophone(false)
        } else if (ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            s.setMicrophone(true)
        } else {
            askMic.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    override fun onResume() {
        super.onResume()
        secondScreen.show { SessionPanel() }
    }

    override fun onPause() {
        secondScreen.dismiss()
        super.onPause()
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
        try {
            stream = core.startStream(hostId, settings.streamOptions(), this)
        } catch (e: TetherException) {
            Toast.makeText(this, e.message, Toast.LENGTH_LONG).show()
            finish()
            return
        }
        statsJob = lifecycleScope.launch {
            while (true) {
                delay(1000)
                stats = stream?.stats()
            }
        }
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        // The decoder renders into this surface; stop before it's gone.
        statsJob?.cancel()
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
                is StreamEvent.Waking -> status = "Waking the console…"
                is StreamEvent.Connecting -> status = "Connecting…"
                is StreamEvent.Connected -> status = "Starting video…"
                is StreamEvent.VideoStarted -> status = null
                is StreamEvent.LoginPinRequest -> askPasscode(event.previousIncorrect)
                is StreamEvent.Rumble -> rumble.set(event.left.toInt(), event.right.toInt())
                is StreamEvent.Microphone -> {
                    micOn = event.on
                    event.error?.let { Toast.makeText(this, it, Toast.LENGTH_LONG).show() }
                }
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
        android.app.AlertDialog.Builder(this)
            .setTitle(if (retry) "Wrong passcode, try again" else "Remote Play passcode")
            .setView(input)
            .setPositiveButton("OK") { _, _ -> stream?.setLoginPin(input.text.toString()) }
            .setNegativeButton("Cancel") { _, _ -> stream?.stop() }
            .show()
    }

    override fun onDestroy() {
        statsJob?.cancel()
        stream?.stop()
        rumble.stop()
        super.onDestroy()
    }
}
