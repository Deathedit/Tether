package io.github.deathedit.tether

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import io.github.deathedit.tether.core.Tether
import io.github.deathedit.tether.core.TetherException
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.Inet4Address
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import java.net.URLDecoder
import java.security.SecureRandom

/**
 * Sign in to PSN from a phone instead of on the device.
 *
 * Sony's sign-in always ends on its own Remote Play page with a one-time
 * code in the address, so that address has to come back to this device. This
 * serves a small page on the local network (shown as a QR code): it links to
 * Sony's sign-in and takes the address you land on, which the phone posts
 * back here. A random token in the path keeps other devices on the network
 * out; the server stops after one successful sign-in or when [stop]ped.
 */
class PhoneSignIn(
    private val context: Context,
    private val core: Tether,
    private val onState: (State) -> Unit,
) {
    sealed class State {
        /** Show [url] as a QR code; waiting for the phone. */
        data class Waiting(val url: String) : State()
        object SigningIn : State()
        data class Done(val accountId: String) : State()
        data class Failed(val message: String) : State()
    }

    private var server: ServerSocket? = null
    private var url = ""
    private val token = ByteArray(9).also { SecureRandom().nextBytes(it) }
        .joinToString("") { "%02x".format(it) }

    fun start() {
        val ip = localAddress()
        if (ip == null) {
            onState(State.Failed("Connect to Wi-Fi first: the phone reaches this device over the local network."))
            return
        }
        val socket = ServerSocket(0)
        server = socket
        url = "http://$ip:${socket.localPort}/$token"
        onState(State.Waiting(url))
        Thread({
            try {
                while (!socket.isClosed) {
                    val client = socket.accept()
                    if (handle(client)) break
                }
            } catch (_: SocketException) {
                // stopped
            } finally {
                runCatching { socket.close() }
            }
        }, "phone-sign-in").start()
    }

    fun stop() {
        runCatching { server?.close() }
        server = null
    }

    /** Serves one request. Returns true once sign-in succeeded. */
    private fun handle(client: Socket): Boolean = client.use { s ->
        s.soTimeout = 15_000
        val reader = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.UTF_8))
        val requestLine = reader.readLine() ?: return false
        val parts = requestLine.split(" ")
        if (parts.size < 2) return false
        val (method, path) = parts[0] to parts[1].substringBefore('?')
        var length = 0
        while (true) {
            val line = reader.readLine() ?: break
            if (line.isEmpty()) break
            if (line.startsWith("Content-Length:", ignoreCase = true)) {
                length = line.substringAfter(':').trim().toIntOrNull() ?: 0
            }
        }
        val out = s.getOutputStream()
        fun respond(status: String, html: String) {
            val body = html.toByteArray(Charsets.UTF_8)
            out.write(
                ("HTTP/1.1 $status\r\nContent-Type: text/html; charset=utf-8\r\n" +
                    "Content-Length: ${body.size}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n")
                    .toByteArray(Charsets.US_ASCII),
            )
            out.write(body)
            out.flush()
        }

        if (path != "/$token") {
            respond("404 Not Found", page("Not found", "<p>Scan the QR code on your device again.</p>"))
            return false
        }
        if (method == "GET") {
            respond("200 OK", formPage(null))
            return false
        }
        if (method != "POST" || length <= 0 || length > 8192) {
            respond("400 Bad Request", formPage("Something went wrong; paste the address again."))
            return false
        }
        val body = CharArray(length)
        var read = 0
        while (read < length) {
            val n = reader.read(body, read, length - read)
            if (n < 0) break
            read += n
        }
        val redirect = String(body, 0, read).split('&')
            .map { it.split('=', limit = 2) }
            .firstOrNull { it[0] == "redirect" }
            ?.getOrNull(1)
            ?.let { URLDecoder.decode(it, "UTF-8") }
            .orEmpty()
        onState(State.SigningIn)
        return try {
            val id = core.finishLogin(redirect)
            respond("200 OK", page("Signed in", "<p>Done. You can put your phone away; your device is signed in.</p>"))
            onState(State.Done(id))
            true
        } catch (e: TetherException) {
            val message = e.message ?: "Sign-in failed"
            respond("200 OK", formPage("$message. Sign in again and paste the new address."))
            onState(State.Waiting(url))
            false
        }
    }

    private fun formPage(error: String?): String {
        val login = core.loginUrl().htmlEscape()
        return page(
            "Sign in to Tether",
            """
            <ol>
              <li><a class="button" href="$login" target="_blank" rel="noopener">Sign in to PSN</a>
                  <p>Sign in on Sony's page. It ends on a blank or error page; that's expected.</p></li>
              <li><p>Copy that page's full address from the address bar, come back to this tab and paste it here:</p>
                  <form method="post">
                    <textarea name="redirect" rows="4" placeholder="https://remoteplay.dl.playstation.net/remoteplay/redirect?code=…" required></textarea>
                    <button type="submit">Send to my device</button>
                  </form></li>
            </ol>
            ${error?.let { "<p class=\"error\">${it.htmlEscape()}</p>" } ?: ""}
            <p class="note">Only your numeric account ID is kept. Your password goes to Sony, never to Tether.</p>
            """,
        )
    }

    private fun page(title: String, content: String) = """
        <!doctype html><html><head><meta charset="utf-8">
        <meta name="viewport" content="width=device-width, initial-scale=1">
        <title>${title.htmlEscape()}</title>
        <style>
          body { background:#0E1015; color:#F2F3F5; font:16px/1.5 system-ui, sans-serif; margin:0; padding:24px; }
          h1 { font-size:22px; margin:0 0 16px; }
          ol { padding-left:20px; } li { margin-bottom:20px; }
          p { color:#9AA0AA; margin:8px 0; }
          a.button, button { display:block; width:100%; box-sizing:border-box; text-align:center; padding:14px;
            border:0; border-radius:12px; background:#A9B4FF; color:#12141A; font-weight:600; font-size:16px;
            text-decoration:none; margin-top:8px; }
          textarea { width:100%; box-sizing:border-box; background:#1A1D24; color:#F2F3F5; border:1px solid #2C303A;
            border-radius:12px; padding:12px; font:14px monospace; }
          .error { color:#F28B82; } .note { font-size:13px; color:#6B717C; }
        </style></head>
        <body><h1>${title.htmlEscape()}</h1>$content</body></html>
    """.trimIndent()

    /** This device's IPv4 address on the current network (Wi-Fi). */
    private fun localAddress(): String? {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return null
        val props: LinkProperties = cm.getLinkProperties(cm.activeNetwork) ?: return null
        return props.linkAddresses
            .map { it.address }
            .firstOrNull { it is Inet4Address && !it.isLoopbackAddress }
            ?.hostAddress
    }

    private fun String.htmlEscape() = replace("&", "&amp;").replace("<", "&lt;")
        .replace(">", "&gt;").replace("\"", "&quot;")
}
