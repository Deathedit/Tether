package io.github.deathedit.tether

import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import io.github.deathedit.tether.core.ControllerInput
import io.github.deathedit.tether.core.buttonBits
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * Turns Android gamepad events (the Thor's built-in controls, or any paired
 * controller) into the console's controller state. Buttons are mapped by
 * position: the bottom face button is Cross, right Circle, left Square, top
 * Triangle.
 */
class GamepadInput(private val send: (ControllerInput) -> Unit) {
    private val bits = buttonBits()
    private var buttons = 0u
    private var hatButtons = 0u
    private var extraButtons = 0u
    private var l2 = 0
    private var r2 = 0
    private var digitalL2 = false
    private var digitalR2 = false
    private var lx = 0; private var ly = 0; private var rx = 0; private var ry = 0
    private var last: ControllerInput? = null

    private val keyMap: Map<Int, UInt> = mapOf(
        KeyEvent.KEYCODE_BUTTON_A to bits.cross,
        KeyEvent.KEYCODE_BUTTON_B to bits.circle,
        KeyEvent.KEYCODE_BUTTON_X to bits.square,
        KeyEvent.KEYCODE_BUTTON_Y to bits.triangle,
        KeyEvent.KEYCODE_BUTTON_L1 to bits.l1,
        KeyEvent.KEYCODE_BUTTON_R1 to bits.r1,
        KeyEvent.KEYCODE_BUTTON_THUMBL to bits.l3,
        KeyEvent.KEYCODE_BUTTON_THUMBR to bits.r3,
        KeyEvent.KEYCODE_BUTTON_START to bits.options,
        KeyEvent.KEYCODE_BUTTON_SELECT to bits.share,
        KeyEvent.KEYCODE_BUTTON_MODE to bits.home,
        KeyEvent.KEYCODE_DPAD_UP to bits.dpadUp,
        KeyEvent.KEYCODE_DPAD_DOWN to bits.dpadDown,
        KeyEvent.KEYCODE_DPAD_LEFT to bits.dpadLeft,
        KeyEvent.KEYCODE_DPAD_RIGHT to bits.dpadRight,
    )

    /** Whether this event comes from a gamepad or joystick. */
    fun isGamepad(source: Int): Boolean =
        source and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD ||
            source and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK ||
            source and InputDevice.SOURCE_DPAD == InputDevice.SOURCE_DPAD

    /** Returns true if the key was a controller button (and consumed). */
    fun onKey(event: KeyEvent): Boolean {
        val down = event.action == KeyEvent.ACTION_DOWN
        when (event.keyCode) {
            KeyEvent.KEYCODE_BUTTON_L2 -> { digitalL2 = down; update(); return true }
            KeyEvent.KEYCODE_BUTTON_R2 -> { digitalR2 = down; update(); return true }
        }
        val bit = keyMap[event.keyCode] ?: return false
        if (event.repeatCount > 0) return true
        buttons = if (down) buttons or bit else buttons and bit.inv()
        update()
        return true
    }

    /** Returns true if the motion was a controller's sticks or triggers. */
    fun onMotion(event: MotionEvent): Boolean {
        if (!isGamepad(event.source) || event.action != MotionEvent.ACTION_MOVE) return false
        lx = stick(event, MotionEvent.AXIS_X)
        ly = stick(event, MotionEvent.AXIS_Y)
        rx = stick(event, MotionEvent.AXIS_Z)
        ry = stick(event, MotionEvent.AXIS_RZ)
        l2 = trigger(maxOf(event.getAxisValue(MotionEvent.AXIS_LTRIGGER), event.getAxisValue(MotionEvent.AXIS_BRAKE)))
        r2 = trigger(maxOf(event.getAxisValue(MotionEvent.AXIS_RTRIGGER), event.getAxisValue(MotionEvent.AXIS_GAS)))
        // Some controllers report the d-pad as a hat axis instead of keys.
        val hx = event.getAxisValue(MotionEvent.AXIS_HAT_X)
        val hy = event.getAxisValue(MotionEvent.AXIS_HAT_Y)
        hatButtons = 0u
        if (hx < -0.5f) hatButtons = hatButtons or bits.dpadLeft
        if (hx > 0.5f) hatButtons = hatButtons or bits.dpadRight
        if (hy < -0.5f) hatButtons = hatButtons or bits.dpadUp
        if (hy > 0.5f) hatButtons = hatButtons or bits.dpadDown
        update()
        return true
    }

    /** Holds an on-screen button (Home, Capture, ...) down or releases it. */
    fun setExtra(bit: UInt, pressed: Boolean) {
        extraButtons = if (pressed) extraButtons or bit else extraButtons and bit.inv()
        update()
    }

    /** Sends "nothing pressed", e.g. when the window loses focus. */
    fun reset() {
        buttons = 0u; hatButtons = 0u; extraButtons = 0u
        l2 = 0; r2 = 0; digitalL2 = false; digitalR2 = false
        lx = 0; ly = 0; rx = 0; ry = 0
        update()
    }

    private fun update() {
        val input = ControllerInput(
            buttons = buttons or hatButtons or extraButtons,
            l2 = (if (digitalL2) 255 else l2).toUByte(),
            r2 = (if (digitalR2) 255 else r2).toUByte(),
            leftX = lx.toShort(), leftY = ly.toShort(),
            rightX = rx.toShort(), rightY = ry.toShort(),
        )
        if (input != last) {
            last = input
            send(input)
        }
    }

    private fun stick(event: MotionEvent, axis: Int): Int {
        val range = event.device?.getMotionRange(axis, event.source)
        var v = event.getAxisValue(axis)
        // Ignore the stick's resting noise.
        if (abs(v) < (range?.flat ?: 0f).coerceAtLeast(0.04f)) v = 0f
        return (v.coerceIn(-1f, 1f) * 32767f).roundToInt()
    }

    private fun trigger(v: Float): Int = (v.coerceIn(0f, 1f) * 255f).roundToInt()
}
