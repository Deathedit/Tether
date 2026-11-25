package io.github.deathedit.tether.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.ui.semantics.Role
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import io.github.deathedit.tether.HomeModel
import io.github.deathedit.tether.Settings
import io.github.deathedit.tether.core.ConsoleInfo
import io.github.deathedit.tether.core.ConsoleState
import io.github.deathedit.tether.core.VideoCodec
import io.github.deathedit.tether.core.VideoResolution
import kotlin.math.roundToInt

fun stateLabel(state: ConsoleState?): Pair<String, androidx.compose.ui.graphics.Color> = when (state) {
    ConsoleState.READY -> "Ready" to Palette.ready
    ConsoleState.STANDBY -> "Rest mode" to Palette.rest
    ConsoleState.UNKNOWN -> "Unknown" to Palette.textMuted
    null -> "Not found" to Palette.textFaint
}

/** Top screen: the selected console, the console cards and button hints. */
@Composable
fun HomeScreen(
    model: HomeModel,
    settings: Settings,
    onConnect: () -> Unit,
    onRest: () -> Unit,
    onSettings: () -> Unit,
    onAdd: () -> Unit,
) {
    val console = model.selectedConsole
    Column(
        Modifier.fillMaxSize().background(Palette.background).padding(horizontal = 40.dp, vertical = 28.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("⚯", style = Type.brand.copy(color = Palette.accent))
            Spacer(Modifier.width(10.dp))
            Text("Tether", style = Type.brand)
            Spacer(Modifier.weight(1f))
            model.latency.lastOrNull()?.let {
                Text("${it.roundToInt()} ms", style = Type.monoSmall)
            }
        }
        Spacer(Modifier.weight(1f))
        if (console == null) {
            Text("No consoles yet", style = Type.hero)
            Spacer(Modifier.height(8.dp))
            Text("Add one to get started.", style = Type.monoSmall.copy(fontSize = Type.monoSmall.fontSize))
        } else {
            val (label, color) = stateLabel(model.stateOf(console))
            Row(verticalAlignment = Alignment.CenterVertically) {
                StatusDot(color, 10)
                Spacer(Modifier.width(10.dp))
                Text(label, style = Type.body.copy(color = color))
            }
            Spacer(Modifier.height(4.dp))
            Text(console.name, style = Type.hero, maxLines = 1)
            Spacer(Modifier.height(6.dp))
            Text("PS5 · ${settings.summary()}", style = Type.monoSmall)
        }
        model.busy?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, style = Type.muted)
        } ?: model.message?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, style = Type.muted.copy(color = Palette.rest))
        }
        Spacer(Modifier.weight(1f))
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            val visible = model.paired.take(3)
            visible.forEachIndexed { i, c ->
                val (l, col) = stateLabel(model.stateOf(c))
                ConsoleCard(c.name, l, col, selected = i == model.selected, onClick = {
                    if (i == model.selected) onConnect() else model.select(i)
                }, modifier = Modifier.weight(1f))
            }
            AddCard(onAdd, Modifier.weight(1f))
            repeat(3 - visible.size) { Spacer(Modifier.weight(1f)) }
        }
        Spacer(Modifier.height(18.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
            if (console != null) {
                ButtonHint("A", "Connect", onConnect)
                ButtonHint("Y", "Rest mode", onRest)
            } else {
                ButtonHint("A", "Add console", onAdd)
            }
            ButtonHint("X", "Settings", onSettings)
        }
    }
}

/** Bottom screen at home: network to the selected console and big buttons. */
@Composable
fun HomeCompanion(model: HomeModel, decoder: String, onConnect: () -> Unit, onRest: () -> Unit, onSettings: () -> Unit, onAdd: () -> Unit) {
    val console = model.selectedConsole
    Column(Modifier.fillMaxSize().background(Palette.background).padding(28.dp)) {
        if (console == null) {
            Spacer(Modifier.weight(1f))
            Text("Pair a console to start", style = Type.title)
            Spacer(Modifier.height(6.dp))
            Text("On the console: Settings › System › Remote Play › Link Device.", style = Type.muted)
            Spacer(Modifier.weight(1f))
            BigButton("Add console", onAdd, Modifier.fillMaxWidth(), ButtonKind.Primary)
            return@Column
        }
        val samples = model.latency
        val now = samples.lastOrNull()
        val jitter = if (samples.size > 1) samples.zipWithNext { a, b -> kotlin.math.abs(a - b) }.average() else null
        val quality = when {
            now == null -> "—" to Palette.textMuted
            now < 30 -> "Good" to Palette.ready
            now < 60 -> "Fair" to Palette.rest
            else -> "Poor" to Palette.danger
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("NETWORK TO ${console.name.uppercase()}", style = Type.eyebrow, modifier = Modifier.weight(1f), maxLines = 1)
            Text(quality.first, style = Type.label.copy(color = quality.second))
        }
        Spacer(Modifier.height(10.dp))
        Row(verticalAlignment = Alignment.Bottom) {
            Text(now?.roundToInt()?.toString() ?: "—", style = Type.monoBig)
            Text(" ms", style = Type.monoValue.copy(color = Palette.textMuted), modifier = Modifier.padding(bottom = 8.dp))
            Spacer(Modifier.width(24.dp))
            Sparkline(samples.toList(), Modifier.weight(1f).height(48.dp))
        }
        Spacer(Modifier.height(14.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            StatTile("State", stateLabel(model.stateOf(console)).first, Modifier.weight(1f))
            StatTile("Jitter", jitter?.let { "%.1f ms".format(it) } ?: "—", Modifier.weight(1f))
            StatTile("Decoder", decoder, Modifier.weight(1f))
        }
        Spacer(Modifier.weight(1f))
        BigButton("Connect to ${console.name}", onConnect, Modifier.fillMaxWidth(), ButtonKind.Primary, enabled = model.busy == null)
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            BigButton("Rest mode", onRest, Modifier.weight(1f), enabled = model.busy == null)
            BigButton("Settings", onSettings, Modifier.weight(1f))
        }
    }
}

@Composable
private fun field(value: String, onChange: (String) -> Unit, label: String, keyboard: KeyboardType = KeyboardType.Text, secret: Boolean = false) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = keyboard),
        visualTransformation = if (secret) PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = Palette.accent,
            unfocusedBorderColor = Palette.outline,
            focusedLabelColor = Palette.accent,
            unfocusedLabelColor = Palette.textMuted,
            focusedTextColor = Palette.text,
            unfocusedTextColor = Palette.text,
            cursorColor = Palette.accent,
        ),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun Page(title: String, onBack: () -> Unit, content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxSize().background(Palette.background).verticalScroll(rememberScrollState()).padding(horizontal = 40.dp, vertical = 28.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("‹  Back", style = Type.body.copy(color = Palette.textMuted), modifier = Modifier.clip(RoundedCornerShape(8.dp)).focusRing(RoundedCornerShape(8.dp)).clickable(onClick = onBack).padding(8.dp))
            Spacer(Modifier.width(16.dp))
            Text(title, style = Type.title)
        }
        Spacer(Modifier.height(20.dp))
        content()
    }
}

/** Add a console: PSN account (once), then the console's 8-digit code. */
@Composable
fun PairScreen(
    model: HomeModel,
    accountId: String?,
    unpaired: List<ConsoleInfo>,
    onSaveAccount: (String) -> Unit,
    onPhoneSignIn: () -> Unit,
    onPair: (address: String, code: String) -> Unit,
    onBack: () -> Unit,
) {
    var account by remember { mutableStateOf("") }
    var address by remember { mutableStateOf(unpaired.firstOrNull()?.address ?: "") }
    var code by remember { mutableStateOf("") }
    Page("Add console", onBack) {
        if (accountId == null) {
            SectionLabel("1 · PSN account")
            Spacer(Modifier.height(8.dp))
            Text("Pairing needs your PSN account ID. Sign in with your phone, or paste the ID (e.g. from the desktop app's config.toml).", style = Type.muted)
            Spacer(Modifier.height(10.dp))
            field(account, { account = it }, "Account ID (base64)")
            Spacer(Modifier.height(8.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                BigButton("Sign in with your phone", onPhoneSignIn, Modifier.weight(1f), ButtonKind.Primary)
                BigButton("Save account ID", { onSaveAccount(account) }, Modifier.weight(1f), enabled = account.isNotBlank())
            }
            Spacer(Modifier.height(24.dp))
            SectionLabel("2 · Console")
        } else {
            SectionLabel("Console")
        }
        Spacer(Modifier.height(8.dp))
        Text("On the console: Settings › System › Remote Play › Link Device, and keep the code on screen.", style = Type.muted)
        if (unpaired.isNotEmpty()) {
            Spacer(Modifier.height(10.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                unpaired.take(3).forEach { c ->
                    val (l, col) = stateLabel(c.state)
                    ConsoleCard(c.name, "${c.address} · $l", col, selected = address == c.address, onClick = { address = c.address }, modifier = Modifier.weight(1f))
                }
            }
        }
        Spacer(Modifier.height(10.dp))
        // IPv4 only: digits and dots, at most "255.255.255.255".
        field(
            address,
            { address = it.filter { c -> c.isDigit() || c == '.' }.take(15) },
            "Console IP address",
            KeyboardType.Decimal,
        )
        Spacer(Modifier.height(8.dp))
        field(code, { code = it.filter(Char::isDigit).take(8) }, "8-digit code", KeyboardType.NumberPassword)
        Spacer(Modifier.height(12.dp))
        BigButton(
            model.busy ?: "Pair",
            { onPair(address, code) },
            Modifier.fillMaxWidth(),
            ButtonKind.Primary,
            enabled = model.busy == null && accountId != null && address.isNotBlank() && code.length == 8,
        )
        model.message?.let {
            Spacer(Modifier.height(10.dp))
            Text(it, style = Type.muted.copy(color = Palette.danger))
        }
    }
}

@Composable
private fun <T> Choice(label: String, options: List<Pair<T, String>>, value: T, onPick: (T) -> Unit) {
    Text(label, style = Type.label)
    Spacer(Modifier.height(6.dp))
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        options.forEach { (v, text) ->
            val on = v == value
            Box(
                Modifier.clip(RoundedCornerShape(10.dp))
                    .background(if (on) Palette.accent else Palette.surface)
                    .focusRing(RoundedCornerShape(10.dp))
                    .clickable { onPick(v) }
                    .padding(horizontal = 16.dp, vertical = 10.dp),
            ) {
                Text(text, style = Type.body.copy(color = if (on) Palette.onAccent else Palette.text, fontWeight = FontWeight.Medium))
            }
        }
    }
    Spacer(Modifier.height(16.dp))
}

/** A setting with a switch; the whole row takes focus and toggles. */
@Composable
private fun SwitchRow(title: String, detail: String?, checked: Boolean, onChange: (Boolean) -> Unit) {
    val shape = RoundedCornerShape(12.dp)
    Row(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .focusRing(shape)
            .toggleable(value = checked, role = Role.Switch, onValueChange = onChange)
            .padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = Type.body)
            detail?.let { Text(it, style = Type.label) }
        }
        Switch(
            checked = checked,
            onCheckedChange = null,
            colors = SwitchDefaults.colors(checkedTrackColor = Palette.accent, checkedThumbColor = Palette.onAccent),
        )
    }
}

/** Stream settings, account and the selected console. */
@Composable
fun SettingsScreen(
    settings: Settings,
    accountId: String?,
    console: ConsoleInfo?,
    onUnpair: (ConsoleInfo) -> Unit,
    onBack: () -> Unit,
) {
    var resolution by remember { mutableStateOf(settings.resolution) }
    var fps by remember { mutableStateOf(settings.fps) }
    var codec by remember { mutableStateOf(settings.codec) }
    var audio by remember { mutableStateOf(settings.audio) }
    var overlay by remember { mutableStateOf(settings.showOverlay) }
    Page("Settings", onBack) {
        SectionLabel("Stream")
        Spacer(Modifier.height(10.dp))
        Choice(
            "Resolution",
            listOf(VideoResolution.P540 to "540p", VideoResolution.P720 to "720p", VideoResolution.P1080 to "1080p"),
            resolution,
        ) { resolution = it; settings.resolution = it }
        Choice("Frame rate", listOf(30 to "30 fps", 60 to "60 fps"), fps) { fps = it; settings.fps = it }
        Choice("Codec", listOf(VideoCodec.H264 to "H.264", VideoCodec.H265 to "HEVC"), codec) { codec = it; settings.codec = it }
        SwitchRow("Sound", null, audio) { audio = it; settings.audio = it }
        Spacer(Modifier.height(8.dp))
        SwitchRow("Latency on the game screen", "Off: the top screen shows only the game.", overlay) {
            overlay = it; settings.showOverlay = it
        }
        Spacer(Modifier.height(24.dp))
        SectionLabel("Account")
        Spacer(Modifier.height(8.dp))
        Text(accountId?.let { "PSN account ID: $it" } ?: "Not signed in.", style = Type.monoSmall)
        console?.let { c ->
            Spacer(Modifier.height(24.dp))
            SectionLabel("Console")
            Spacer(Modifier.height(8.dp))
            Text("${c.name} · ${c.address} · ${c.hostId}", style = Type.monoSmall)
            Spacer(Modifier.height(10.dp))
            BigButton("Forget ${c.name}", { onUnpair(c) }, Modifier.fillMaxWidth(), ButtonKind.Danger)
        }
    }
}
