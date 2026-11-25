package io.github.deathedit.tether.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** Colours from the design (screenshots/6-thor-home.png, 7-thor-in-session.png). */
object Palette {
    val background = Color(0xFF0E1015)
    val surface = Color(0xFF1A1D24)
    val surfaceHigh = Color(0xFF232731)
    val outline = Color(0xFF2C303A)
    val text = Color(0xFFF2F3F5)
    val textMuted = Color(0xFF9AA0AA)
    val textFaint = Color(0xFF6B717C)
    val accent = Color(0xFFA9B4FF)
    val onAccent = Color(0xFF12141A)
    val ready = Color(0xFF6FE3A0)
    val rest = Color(0xFFFFC24D)
    val danger = Color(0xFFF28B82)
    val dangerSurface = Color(0xFF2A1A1C)
}

val Mono = FontFamily.Monospace

object Type {
    val brand = TextStyle(fontSize = 18.sp, fontWeight = FontWeight.SemiBold, color = Palette.text)
    val hero = TextStyle(fontSize = 52.sp, fontWeight = FontWeight.SemiBold, color = Palette.text, letterSpacing = (-1).sp)
    val title = TextStyle(fontSize = 22.sp, fontWeight = FontWeight.SemiBold, color = Palette.text)
    val body = TextStyle(fontSize = 15.sp, color = Palette.text)
    val muted = TextStyle(fontSize = 14.sp, color = Palette.textMuted)
    val label = TextStyle(fontSize = 12.sp, color = Palette.textMuted)
    val eyebrow = TextStyle(fontSize = 12.sp, color = Palette.textMuted, letterSpacing = 1.5.sp)
    val monoSmall = TextStyle(fontSize = 13.sp, fontFamily = Mono, color = Palette.textMuted)
    val monoValue = TextStyle(fontSize = 18.sp, fontFamily = Mono, color = Palette.text)
    val monoBig = TextStyle(fontSize = 56.sp, fontFamily = Mono, color = Palette.text)
}

@Composable
fun TetherTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            background = Palette.background,
            surface = Palette.surface,
            primary = Palette.accent,
            onPrimary = Palette.onAccent,
            onBackground = Palette.text,
            onSurface = Palette.text,
            error = Palette.danger,
        ),
        content = content,
    )
}
