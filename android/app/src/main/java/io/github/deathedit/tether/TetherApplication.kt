package io.github.deathedit.tether

import android.app.Application
import io.github.deathedit.tether.core.Tether

/** Holds the one [Tether] (Rust core) for the whole app. */
class TetherApplication : Application() {
    val core: Tether by lazy { Tether(filesDir.absolutePath, KeystoreStorage(this)) }
}

val android.app.Activity.core: Tether
    get() = (application as TetherApplication).core
