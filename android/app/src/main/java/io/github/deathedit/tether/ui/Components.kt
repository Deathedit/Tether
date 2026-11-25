package io.github.deathedit.tether.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.composed
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * A clear ring around whatever has focus, so moving through the screen with
 * a controller's d-pad shows where you are. Put it before `clickable`.
 */
fun Modifier.focusRing(shape: Shape = RoundedCornerShape(12.dp)): Modifier = composed {
    var focused by remember { mutableStateOf(false) }
    this
        .onFocusChanged { focused = it.isFocused }
        .border(if (focused) 3.dp else 0.dp, if (focused) Palette.accent else Color.Transparent, shape)
        .background(if (focused) Palette.accent.copy(alpha = 0.12f) else Color.Transparent, shape)
}

@Composable
fun StatusDot(color: Color, size: Int = 8) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(color))
}

/** One console in the home screen's row of cards. */
@Composable
fun ConsoleCard(
    name: String,
    state: String,
    stateColor: Color,
    selected: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    keyFocus: Boolean = true,
) {
    val shape = RoundedCornerShape(14.dp)
    Column(
        modifier
            .height(76.dp)
            .clip(shape)
            .background(Palette.surface)
            .border(BorderStroke(if (selected) 2.dp else 0.dp, if (selected) Palette.accent else Color.Transparent), shape)
            .focusProperties { canFocus = keyFocus }
            .focusRing(shape)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(name, style = Type.body.copy(fontWeight = FontWeight.SemiBold), maxLines = 1)
        Text(state, style = Type.label.copy(color = stateColor))
    }
}

/** The dashed "Add console" card. */
@Composable
fun AddCard(onClick: () -> Unit, modifier: Modifier = Modifier, keyFocus: Boolean = true) {
    val shape = RoundedCornerShape(14.dp)
    Box(
        modifier
            .height(76.dp)
            .clip(shape)
            .focusProperties { canFocus = keyFocus }
            .focusRing(shape)
            .clickable(onClick = onClick)
            .dashedBorder(Palette.outline, 14f),
        contentAlignment = Alignment.CenterStart,
    ) {
        Text("+  Add console", style = Type.body.copy(color = Palette.textMuted), modifier = Modifier.padding(start = 16.dp))
    }
}

private fun Modifier.dashedBorder(color: Color, radiusDp: Float): Modifier = drawBehind {
    drawRoundRect(
        color = color,
        cornerRadius = CornerRadius(radiusDp * density),
        style = Stroke(
            width = 1.5f * density,
            pathEffect = PathEffect.dashPathEffect(floatArrayOf(8f * density, 6f * density)),
        ),
    )
}

/** "(A) Connect" style hint for the controller's face buttons. */
@Composable
fun ButtonHint(letter: String, label: String, onClick: () -> Unit, keyFocus: Boolean = true) {
    Row(
        Modifier.clip(RoundedCornerShape(20.dp))
            .focusProperties { canFocus = keyFocus }
            .focusRing(RoundedCornerShape(20.dp))
            .clickable(onClick = onClick)
            .padding(end = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(26.dp).border(1.5.dp, Palette.textMuted, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(letter, style = Type.label.copy(color = Palette.text, fontWeight = FontWeight.Bold))
        }
        Spacer(Modifier.width(10.dp))
        Text(label, style = Type.body.copy(color = Palette.textMuted))
    }
}

/** Label over a monospace value, on a dark tile. */
@Composable
fun StatTile(label: String, value: String, modifier: Modifier = Modifier) {
    Column(
        modifier
            .clip(RoundedCornerShape(12.dp))
            .background(Palette.surface)
            .padding(horizontal = 14.dp, vertical = 10.dp),
    ) {
        Text(label, style = Type.label)
        Spacer(Modifier.height(4.dp))
        Text(value, style = Type.monoValue, maxLines = 1)
    }
}

/** Small line graph of recent values, with a dot on the newest. */
@Composable
fun Sparkline(values: List<Float>, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        if (values.size < 2) return@Canvas
        val max = (values.maxOrNull() ?: 1f).coerceAtLeast(1f)
        val min = (values.minOrNull() ?: 0f).coerceAtMost(max - 1f)
        val stepX = size.width / (values.size - 1)
        fun y(v: Float) = size.height - (v - min) / (max - min) * size.height * 0.8f - size.height * 0.1f
        val line = Path()
        values.forEachIndexed { i, v -> if (i == 0) line.moveTo(0f, y(v)) else line.lineTo(i * stepX, y(v)) }
        val fill = Path().apply {
            addPath(line)
            lineTo(size.width, size.height)
            lineTo(0f, size.height)
            close()
        }
        drawPath(fill, Palette.surface)
        drawPath(line, Palette.accent, style = Stroke(width = 2f * density))
        drawCircle(Palette.accent, radius = 4f * density, center = Offset(size.width, y(values.last())))
    }
}

enum class ButtonKind { Primary, Secondary, Danger }

@Composable
fun BigButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    kind: ButtonKind = ButtonKind.Secondary,
    enabled: Boolean = true,
) {
    val (bg, fg) = when (kind) {
        ButtonKind.Primary -> Palette.accent to Palette.onAccent
        ButtonKind.Secondary -> Palette.surface to Palette.text
        ButtonKind.Danger -> Palette.dangerSurface to Palette.danger
    }
    Box(
        modifier
            .clip(RoundedCornerShape(14.dp))
            .background(if (enabled) bg else bg.copy(alpha = 0.4f))
            .focusRing(RoundedCornerShape(14.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .padding(vertical = 14.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = Type.body.copy(color = fg, fontWeight = FontWeight.SemiBold, fontSize = 16.sp))
    }
}

/** Icon over a label; the bottom row of the session companion. */
@Composable
fun RowScope.ActionButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    kind: ButtonKind = ButtonKind.Secondary,
) {
    val (bg, fg) = when (kind) {
        ButtonKind.Danger -> Palette.dangerSurface to Palette.danger
        else -> Palette.surface to Palette.text
    }
    Column(
        Modifier
            .weight(1f)
            .clip(RoundedCornerShape(12.dp))
            .background(bg)
            .focusRing(RoundedCornerShape(12.dp))
            .clickable(onClick = onClick)
            .padding(vertical = 10.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(icon, contentDescription = label, tint = fg, modifier = Modifier.size(22.dp))
        Spacer(Modifier.height(4.dp))
        Text(label, style = Type.label.copy(color = fg))
    }
}

@Composable
fun SectionLabel(text: String) {
    Text(text.uppercase(), style = Type.eyebrow, modifier = Modifier.fillMaxWidth())
}
