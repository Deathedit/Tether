package io.github.deathedit.tether.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.common.BitMatrix
import com.google.zxing.qrcode.QRCodeWriter
import io.github.deathedit.tether.PhoneSignIn

/** QR code to sign in from a phone, with what happens next. */
@Composable
fun PhoneSignInScreen(state: PhoneSignIn.State?, onBack: () -> Unit) {
    Row(
        Modifier.fillMaxSize().background(Palette.background).padding(horizontal = 40.dp, vertical = 28.dp),
        horizontalArrangement = Arrangement.spacedBy(40.dp),
    ) {
        Column(Modifier.weight(1f)) {
            Text(
                "‹  Back",
                style = Type.body.copy(color = Palette.textMuted),
                modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable(onClick = onBack).padding(8.dp),
            )
            Spacer(Modifier.height(16.dp))
            Text("Sign in with your phone", style = Type.title)
            Spacer(Modifier.height(16.dp))
            Step("1", "Scan the code with your phone's camera. Your phone must be on the same network.")
            Step("2", "Tap Sign in to PSN and sign in on Sony's page. It ends on a blank or error page; that's expected.")
            Step("3", "Copy that page's address, go back to the first tab, paste it and tap Send to my device.")
            Spacer(Modifier.weight(1f))
            val (text, color) = when (state) {
                null, is PhoneSignIn.State.Waiting -> "Waiting for your phone…" to Palette.textMuted
                PhoneSignIn.State.SigningIn -> "Signing in…" to Palette.accent
                is PhoneSignIn.State.Done -> "Signed in." to Palette.ready
                is PhoneSignIn.State.Failed -> state.message to Palette.danger
            }
            Text(text, style = Type.body.copy(color = color))
            (state as? PhoneSignIn.State.Waiting)?.let {
                Spacer(Modifier.height(6.dp))
                Text(it.url, style = Type.monoSmall)
            }
        }
        Box(Modifier.fillMaxHeight().aspectRatio(1f), contentAlignment = Alignment.Center) {
            (state as? PhoneSignIn.State.Waiting)?.let { QrCode(it.url, Modifier.fillMaxSize()) }
        }
    }
}

@Composable
private fun Step(number: String, text: String) {
    Row(Modifier.padding(bottom = 12.dp)) {
        Text(number, style = Type.body.copy(color = Palette.accent), modifier = Modifier.width(24.dp))
        Text(text, style = Type.muted)
    }
}

/** Dark modules on a white square with a quiet zone, so any camera reads it. */
@Composable
fun QrCode(text: String, modifier: Modifier = Modifier) {
    val matrix: BitMatrix = remember(text) {
        QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0, mapOf(EncodeHintType.MARGIN to 2))
    }
    Canvas(modifier.clip(RoundedCornerShape(16.dp)).background(Color.White)) {
        val cell = size.minDimension / matrix.width
        for (y in 0 until matrix.height) {
            for (x in 0 until matrix.width) {
                if (matrix[x, y]) {
                    drawRect(Color.Black, Offset(x * cell, y * cell), Size(cell + 0.5f, cell + 0.5f))
                }
            }
        }
    }
}
