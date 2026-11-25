package io.github.deathedit.tether

import android.content.Intent
import android.media.MediaCodecList
import android.net.Uri
import android.os.Bundle
import android.view.KeyEvent
import android.view.MotionEvent
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.lifecycleScope
import io.github.deathedit.tether.ui.HomeCompanion
import io.github.deathedit.tether.ui.HomeScreen
import io.github.deathedit.tether.ui.PairScreen
import io.github.deathedit.tether.ui.SettingsScreen
import io.github.deathedit.tether.ui.TetherTheme
import kotlinx.coroutines.Job

/**
 * Home: pick a console, pair new ones, settings. On the AYN Thor the bottom
 * screen shows the network to the selected console and big buttons.
 */
class MainActivity : ComponentActivity() {
    private enum class Screen { Home, Pair, Settings }

    private val settings by lazy { Settings(this) }
    private val model by lazy { HomeModel(core, settings) }
    private val secondScreen by lazy { SecondScreen(this) }
    private var screen by mutableStateOf(Screen.Home)
    private var accountId by mutableStateOf<String?>(null)
    private var loops: List<Job> = emptyList()
    private var hatX = 0f
    private val decoderName by lazy { lowLatencyDecoder() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            TetherTheme {
                BackHandler(enabled = screen != Screen.Home) { screen = Screen.Home }
                when (screen) {
                    Screen.Home -> HomeScreen(
                        model, settings,
                        onConnect = ::connect,
                        onRest = { model.restMode(lifecycleScope) },
                        onSettings = { screen = Screen.Settings },
                        onAdd = { openPair() },
                    )
                    Screen.Pair -> PairScreen(
                        model, accountId, model.unpaired,
                        onSaveAccount = { id ->
                            model.run(lifecycleScope, "Saving…", { core.setAccountId(id) }) { refresh() }
                        },
                        onBrowserSignIn = {
                            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(core.loginUrl())))
                        },
                        onFinishSignIn = { redirect ->
                            model.run(lifecycleScope, "Signing in…", { core.finishLogin(redirect) }) { refresh() }
                        },
                        onPair = { address, code ->
                            var name = ""
                            model.run(lifecycleScope, "Pairing…", { name = core.pair(address, code).name }) {
                                refresh()
                                model.select(model.paired.indexOfFirst { it.name == name })
                                screen = Screen.Home
                            }
                        },
                        onBack = { screen = Screen.Home },
                    )
                    Screen.Settings -> SettingsScreen(
                        settings, accountId, model.selectedConsole,
                        onUnpair = { c ->
                            model.run(lifecycleScope, "Forgetting…", { core.unpair(c.hostId) }) {
                                refresh()
                                screen = Screen.Home
                            }
                        },
                        onBack = { screen = Screen.Home },
                    )
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        refresh()
        loops = listOf(model.scan(lifecycleScope), model.probe(lifecycleScope))
        secondScreen.show {
            HomeCompanion(
                model, decoderName,
                onConnect = ::connect,
                onRest = { model.restMode(lifecycleScope) },
                onSettings = { screen = Screen.Settings },
                onAdd = { openPair() },
            )
        }
    }

    override fun onPause() {
        loops.forEach { it.cancel() }
        secondScreen.dismiss()
        super.onPause()
    }

    private fun refresh() {
        accountId = core.accountId()
        model.reload()
    }

    private fun openPair() {
        model.message = null
        screen = Screen.Pair
    }

    private fun connect() {
        val c = model.selectedConsole ?: return openPair()
        if (model.busy != null) return
        startActivity(
            Intent(this, StreamActivity::class.java)
                .putExtra(StreamActivity.EXTRA_HOST_ID, c.hostId)
                .putExtra(StreamActivity.EXTRA_NAME, c.name),
        )
    }

    // Controller on the home screen: left/right pick, A connect, Y rest,
    // X settings, B back.
    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.action != KeyEvent.ACTION_DOWN) return super.dispatchKeyEvent(event)
        if (screen != Screen.Home) {
            if (event.keyCode == KeyEvent.KEYCODE_BUTTON_B) {
                screen = Screen.Home
                return true
            }
            return super.dispatchKeyEvent(event)
        }
        when (event.keyCode) {
            KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_BUTTON_L1 -> model.move(-1)
            KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_BUTTON_R1 -> model.move(1)
            KeyEvent.KEYCODE_BUTTON_A, KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER -> connect()
            KeyEvent.KEYCODE_BUTTON_Y -> model.restMode(lifecycleScope)
            KeyEvent.KEYCODE_BUTTON_X -> screen = Screen.Settings
            else -> return super.dispatchKeyEvent(event)
        }
        return true
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (screen == Screen.Home && event.action == MotionEvent.ACTION_MOVE) {
            val x = event.getAxisValue(MotionEvent.AXIS_HAT_X)
            if (x != hatX) {
                if (x < -0.5f) model.move(-1)
                if (x > 0.5f) model.move(1)
                hatX = x
                return true
            }
        }
        return super.dispatchGenericMotionEvent(event)
    }

    /** The hardware H.264 decoder MediaCodec would pick, for the companion. */
    private fun lowLatencyDecoder(): String = runCatching {
        MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .firstOrNull { !it.isEncoder && it.isHardwareAccelerated && it.supportedTypes.any { t -> t == "video/avc" } }
            ?.name?.substringAfterLast("c2.")?.substringBefore(".avc")?.uppercase()
    }.getOrNull() ?: "MediaCodec"
}
