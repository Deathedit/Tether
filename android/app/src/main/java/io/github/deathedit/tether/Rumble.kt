package io.github.deathedit.tether

import android.content.Context
import android.os.CombinedVibration
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.view.InputDevice

/**
 * Plays the console's rumble on the controller's motors when it has them
 * (InputDevice vibrators), otherwise on the device's own vibrator.
 */
class Rumble(private val context: Context) {
    private var current = 0

    fun set(left: Int, right: Int) {
        val strength = maxOf(left, right).coerceIn(0, 255)
        if (strength == current) return
        current = strength
        val pad = gamepadVibrators()
        if (pad != null && pad.vibratorIds.isNotEmpty()) {
            if (strength == 0) {
                pad.cancel()
            } else {
                // Long one-shot that the next change replaces.
                pad.vibrate(CombinedVibration.createParallel(VibrationEffect.createOneShot(LONG_MS, strength)))
            }
            return
        }
        val vibrator = deviceVibrator() ?: return
        if (!vibrator.hasVibrator()) return
        if (strength == 0) {
            vibrator.cancel()
        } else {
            val amplitude = if (vibrator.hasAmplitudeControl()) strength else VibrationEffect.DEFAULT_AMPLITUDE
            vibrator.vibrate(VibrationEffect.createOneShot(LONG_MS, amplitude))
        }
    }

    fun stop() = set(0, 0)

    private fun gamepadVibrators(): VibratorManager? {
        for (id in InputDevice.getDeviceIds()) {
            val device: InputDevice = InputDevice.getDevice(id) ?: continue
            if ((device.sources and InputDevice.SOURCE_GAMEPAD) == InputDevice.SOURCE_GAMEPAD) {
                return device.getVibratorManager()
            }
        }
        return null
    }

    private fun deviceVibrator(): Vibrator? =
        (context.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)?.defaultVibrator

    private companion object {
        const val LONG_MS = 5000L
    }
}
