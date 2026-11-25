package io.github.deathedit.tether

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import io.github.deathedit.tether.core.ConsoleInfo
import io.github.deathedit.tether.core.ConsoleState
import io.github.deathedit.tether.core.Tether
import io.github.deathedit.tether.core.TetherException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** State behind the home screens (top and bottom). */
class HomeModel(private val core: Tether, private val settings: Settings) {
    var paired by mutableStateOf(listOf<ConsoleInfo>())
        private set
    /** Consoles that answered discovery but aren't paired. */
    var unpaired by mutableStateOf(listOf<ConsoleInfo>())
        private set
    /** Live state of each console that answered, by host ID. */
    var states by mutableStateOf(mapOf<String, ConsoleState>())
        private set
    var selected by mutableIntStateOf(0)
        private set
    /** Recent round trips to the selected console, in ms (newest last). */
    val latency = mutableStateListOf<Float>()
    var busy by mutableStateOf<String?>(null)
    var message by mutableStateOf<String?>(null)

    val selectedConsole: ConsoleInfo? get() = paired.getOrNull(selected)

    fun reload() {
        paired = core.paired()
        val last = settings.lastConsole
        selected = paired.indexOfFirst { it.hostId == last }.coerceAtLeast(0)
    }

    fun select(index: Int) {
        if (paired.isEmpty()) return
        selected = index.coerceIn(0, paired.size - 1)
        settings.lastConsole = paired[selected].hostId
        latency.clear()
    }

    fun move(delta: Int) = select(selected + delta)

    fun stateOf(c: ConsoleInfo): ConsoleState? = states[c.hostId]

    /** Rediscovers consoles every few seconds while the home screen shows. */
    fun scan(scope: CoroutineScope): Job = scope.launch {
        while (isActive) {
            val found = withContext(Dispatchers.IO) { runCatching { core.discover(1500u) }.getOrNull() }
            if (found != null) {
                states = found.associate { it.hostId to it.state }
                unpaired = found.filter { !it.paired }
            }
            delay(4000)
        }
    }

    /** Probes the selected console's latency once a second. */
    fun probe(scope: CoroutineScope): Job = scope.launch {
        while (isActive) {
            val c = selectedConsole
            if (c != null) {
                val ms = withContext(Dispatchers.IO) { core.probeLatency(c.address) }
                if (ms != null) {
                    latency.add(ms)
                    while (latency.size > 30) latency.removeAt(0)
                }
            }
            delay(1000)
        }
    }

    /** Runs blocking core [work] off the main thread, with a busy message. */
    fun run(scope: CoroutineScope, label: String, work: () -> Unit, done: () -> Unit = {}) {
        if (busy != null) return
        busy = label
        message = null
        scope.launch {
            val error = withContext(Dispatchers.IO) {
                try {
                    work(); null
                } catch (e: TetherException) {
                    e.message
                }
            }
            busy = null
            if (error != null) message = error else done()
        }
    }

    fun restMode(scope: CoroutineScope) {
        val c = selectedConsole ?: return
        run(scope, "Putting ${c.name} to rest…", { core.restMode(c.hostId) }) {
            message = "${c.name} is going into rest mode."
        }
    }
}
