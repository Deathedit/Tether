package io.github.deathedit.tether

import android.app.Presentation
import android.hardware.display.DisplayManager
import android.os.Bundle
import android.view.Display
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.ComposeView
import androidx.lifecycle.setViewTreeLifecycleOwner
import androidx.savedstate.setViewTreeSavedStateRegistryOwner
import io.github.deathedit.tether.ui.TetherTheme

/**
 * Shows Compose content on a second screen (the AYN Thor's bottom screen)
 * with Android's Presentation. The display is found by category, never by a
 * hardcoded ID. The window never takes input focus, so the controller keeps
 * talking to the main screen; touches on the bottom screen still work.
 */
class SecondScreen(private val activity: ComponentActivity) {
    private var presentation: Presentation? = null

    /** Whether the device has a second screen to put the companion on. */
    val available: Boolean get() = findDisplay() != null

    fun show(content: @Composable () -> Unit) {
        dismiss()
        val display = findDisplay() ?: return
        presentation = object : Presentation(activity, display) {
            override fun onCreate(savedInstanceState: Bundle?) {
                super.onCreate(savedInstanceState)
                window?.addFlags(WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE)
                window?.decorView?.let {
                    it.setViewTreeLifecycleOwner(activity)
                    it.setViewTreeSavedStateRegistryOwner(activity)
                }
                setContentView(ComposeView(context).apply {
                    setViewTreeLifecycleOwner(activity)
                    setViewTreeSavedStateRegistryOwner(activity)
                    setContent { TetherTheme { content() } }
                })
            }
        }.also {
            runCatching { it.show() }.onFailure { presentation = null }
        }
    }

    fun dismiss() {
        presentation?.dismiss()
        presentation = null
    }

    private fun findDisplay(): Display? {
        val dm = activity.getSystemService(DisplayManager::class.java) ?: return null
        val current = activity.display?.displayId ?: Display.DEFAULT_DISPLAY
        return dm.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION)
            .firstOrNull { it.displayId != current && it.isValid }
    }
}
