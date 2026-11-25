package io.github.deathedit.tether

import android.app.Activity
import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.text.InputType
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import io.github.deathedit.tether.core.ConsoleInfo
import io.github.deathedit.tether.core.ConsoleState
import java.util.concurrent.Executors

/**
 * Bare test screen for milestone 6: account ID, pairing and the console
 * list. The designed Compose screens arrive in milestone 7.
 */
class MainActivity : Activity() {
    private val worker = Executors.newSingleThreadExecutor()

    private lateinit var accountStatus: TextView
    private lateinit var accountInput: EditText
    private lateinit var redirectInput: EditText
    private lateinit var addressInput: EditText
    private lateinit var pinInput: EditText
    private lateinit var consoles: LinearLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 48, 48, 48)
        }
        fun heading(text: String) = column.addView(TextView(this).apply {
            this.text = text
            textSize = 20f
            setTextColor(Color.WHITE)
            setPadding(0, 32, 0, 8)
        })
        fun field(hint: String, type: Int = InputType.TYPE_CLASS_TEXT) = EditText(this).apply {
            this.hint = hint
            inputType = type
            column.addView(this)
        }
        fun button(text: String, onClick: () -> Unit) = column.addView(Button(this).apply {
            this.text = text
            setOnClickListener { onClick() }
        })

        column.addView(TextView(this).apply {
            text = "Tether"
            textSize = 28f
            setTextColor(Color.WHITE)
        })

        heading("PSN account")
        accountStatus = TextView(this).also { column.addView(it) }
        accountInput = field("Account ID (base64), e.g. from the desktop app")
        button("Save account ID") { saveAccountId() }
        button("Sign in with browser") {
            startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(core.loginUrl())))
        }
        redirectInput = field(
            "Paste the address the browser ended on",
            InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI,
        )
        button("Finish sign-in") { finishSignIn() }

        heading("Pair a console")
        column.addView(TextView(this).apply {
            text = "On the console: Settings > System > Remote Play > Link Device."
        })
        addressInput = field("Console IP address", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        pinInput = field("8-digit code", InputType.TYPE_CLASS_NUMBER)
        button("Pair") { pair() }

        heading("Consoles")
        button("Scan network") { scan() }
        consoles = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        column.addView(consoles)

        setContentView(ScrollView(this).apply {
            setBackgroundColor(Color.rgb(16, 18, 24))
            addView(column)
        })
        refresh()
    }

    override fun onResume() {
        super.onResume()
        refresh()
    }

    /** Runs [work] off the UI thread, then [done] on it; errors become toasts. */
    private fun <T> background(work: () -> T, done: (T) -> Unit) {
        worker.execute {
            val result = runCatching(work)
            runOnUiThread {
                result.fold(done) { e -> toast(e.message ?: e.toString()) }
            }
        }
    }

    private fun toast(text: String) = Toast.makeText(this, text, Toast.LENGTH_LONG).show()

    private fun refresh() {
        val id = core.accountId()
        accountStatus.text = if (id != null) "Account ID: $id" else "No account ID yet."
        showConsoles(core.paired())
    }

    private fun saveAccountId() {
        val text = accountInput.text.toString()
        background({ core.setAccountId(text) }) { accountInput.text.clear(); refresh() }
    }

    private fun finishSignIn() {
        val text = redirectInput.text.toString()
        background({ core.finishLogin(text) }) { redirectInput.text.clear(); refresh() }
    }

    private fun pair() {
        val address = addressInput.text.toString()
        val pin = pinInput.text.toString()
        toast("Pairing…")
        background({ core.pair(address, pin) }) { console ->
            pinInput.text.clear()
            toast("Paired with ${console.name}")
            refresh()
        }
    }

    private fun scan() {
        background({ core.discover(2000u) }) { found ->
            if (found.isEmpty()) toast("No consoles found")
            // Paired ones from the store, updated with what answered.
            val paired = core.paired().associateBy { it.hostId }
            val merged = paired.values.map { p -> found.find { it.hostId == p.hostId } ?: p } +
                found.filter { it.hostId !in paired }
            showConsoles(merged)
        }
    }

    private fun showConsoles(list: List<ConsoleInfo>) {
        consoles.removeAllViews()
        if (list.isEmpty()) {
            consoles.addView(TextView(this).apply { text = "None yet." })
            return
        }
        for (c in list) {
            val state = when (c.state) {
                ConsoleState.READY -> "ready"
                ConsoleState.STANDBY -> "rest mode"
                ConsoleState.UNKNOWN -> "?"
            }
            consoles.addView(Button(this).apply {
                text = if (c.paired) "Stream ${c.name} (${c.address}, $state)"
                       else "${c.name} (${c.address}, $state): tap to pair"
                setOnClickListener(View.OnClickListener {
                    if (c.paired) {
                        startActivity(Intent(this@MainActivity, StreamActivity::class.java)
                            .putExtra(StreamActivity.EXTRA_HOST_ID, c.hostId)
                            .putExtra(StreamActivity.EXTRA_NAME, c.name))
                    } else {
                        addressInput.setText(c.address)
                        pinInput.requestFocus()
                    }
                })
            })
        }
    }

    override fun onDestroy() {
        worker.shutdown()
        super.onDestroy()
    }
}
