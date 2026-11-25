package io.github.deathedit.tether.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material.icons.outlined.MicOff
import androidx.compose.material.icons.outlined.PowerSettingsNew
import androidx.compose.material.icons.outlined.Screenshot
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.changedToDown
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.unit.dp
import io.github.deathedit.tether.core.StreamStats

/** What the touchpad tells the stream. Coordinates are 0.0-1.0. */
interface TouchpadSink {
    /** Returns an id for the finger, or -1 if two are already down. */
    fun down(x: Float, y: Float): Int
    fun move(id: Int, x: Float, y: Float)
    fun up(id: Int)
    /** A quick tap: click the touchpad at the finger, then lift it. */
    fun tap(id: Int)
}

/** Bottom screen during a stream: live numbers, touchpad, quick actions. */
@Composable
fun SessionCompanion(
    stats: StreamStats?,
    micOn: Boolean,
    touchpad: TouchpadSink,
    onHome: () -> Unit,
    onMic: () -> Unit,
    onCapture: () -> Unit,
    onDisconnect: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.fillMaxSize().background(Palette.background).padding(20.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            StatTile("Latency", stats?.latencyMs?.takeIf { it > 0u }?.let { "$it ms" } ?: "—", Modifier.weight(1f))
            StatTile("Bitrate", stats?.let { "%.1f Mbps".format(it.bitrateMbps) } ?: "—", Modifier.weight(1f))
            StatTile("Frame rate", stats?.let { "%.0f fps".format(it.fps) } ?: "—", Modifier.weight(1f))
            StatTile("Loss", stats?.let { "%.1f %%".format(it.lossPercent) } ?: "—", Modifier.weight(1f))
        }
        Spacer(Modifier.height(14.dp))
        Touchpad(touchpad, Modifier.weight(1f).fillMaxWidth())
        Spacer(Modifier.height(14.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            ActionButton(Icons.Outlined.Home, "Home", onHome)
            ActionButton(if (micOn) Icons.Outlined.Mic else Icons.Outlined.MicOff, if (micOn) "Mic on" else "Mic off", onMic)
            ActionButton(Icons.Outlined.Screenshot, "Capture", onCapture)
            ActionButton(Icons.Outlined.PowerSettingsNew, "Disconnect", onDisconnect, ButtonKind.Danger)
        }
    }
}

/** The DualSense touchpad on the bottom screen. Multi-touch; a quick tap clicks. */
@Composable
fun Touchpad(sink: TouchpadSink, modifier: Modifier = Modifier) {
    val fingers = remember { mutableStateMapOf<PointerId, Offset>() }
    val shape = RoundedCornerShape(18.dp)
    Box(
        modifier
            .clip(shape)
            .background(Palette.surface.copy(alpha = 0.55f))
            .border(1.dp, Palette.outline, shape)
            .pointerInput(sink) {
                val ids = HashMap<PointerId, Int>()
                val starts = HashMap<PointerId, Pair<Long, Offset>>()
                val tapSlop = 24.dp.toPx()
                awaitPointerEventScope {
                    while (true) {
                        val event = awaitPointerEvent()
                        for (change in event.changes) {
                            val x = (change.position.x / size.width).coerceIn(0f, 1f)
                            val y = (change.position.y / size.height).coerceIn(0f, 1f)
                            when {
                                change.changedToDown() -> {
                                    ids[change.id] = sink.down(x, y)
                                    starts[change.id] = change.uptimeMillis to change.position
                                    fingers[change.id] = change.position
                                }
                                change.changedToUp() -> {
                                    val id = ids.remove(change.id) ?: -1
                                    val start = starts.remove(change.id)
                                    fingers.remove(change.id)
                                    val tap = start != null &&
                                        change.uptimeMillis - start.first < 250 &&
                                        (change.position - start.second).getDistance() < tapSlop
                                    if (id >= 0) {
                                        if (tap) sink.tap(id) else sink.up(id)
                                    }
                                }
                                change.positionChanged() -> {
                                    ids[change.id]?.takeIf { it >= 0 }?.let { sink.move(it, x, y) }
                                    fingers[change.id] = change.position
                                }
                            }
                            change.consume()
                        }
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val dash = PathEffect.dashPathEffect(floatArrayOf(6f * density, 6f * density))
            drawLine(Palette.outline, Offset(size.width / 2, 0f), Offset(size.width / 2, size.height), strokeWidth = density, pathEffect = dash)
            drawLine(Palette.outline, Offset(0f, size.height / 2), Offset(size.width, size.height / 2), strokeWidth = density, pathEffect = dash)
            for (p in fingers.values) {
                drawCircle(Palette.accent.copy(alpha = 0.25f), radius = 26f * density, center = p)
                drawCircle(Palette.accent, radius = 9f * density, center = p)
            }
        }
        if (fingers.isEmpty()) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Touchpad", style = Type.title.copy(color = Palette.text))
                Spacer(Modifier.height(4.dp))
                Text("Tap to click · tap a side for left or right click", style = Type.muted)
            }
        }
    }
}
