plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.github.deathedit.tether"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.deathedit.tether"
        // Android 11: MediaCodec low-latency mode and the AYN Thor's OS.
        minSdk = 30
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            // Built by ../build-rust.sh into src/main/jniLibs.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    // UniFFI's Kotlin bindings call into libtether_ffi.so through JNA.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
}
