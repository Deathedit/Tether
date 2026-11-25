plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// One version for all of Tether: [workspace.package] version in Cargo.toml.
val tetherVersion: String = Regex("""(?m)^version\s*=\s*"([^"]+)"""")
    .find(rootProject.file("../Cargo.toml").readText())!!.groupValues[1]
val tetherVersionCode: Int = tetherVersion.substringBefore('-').split('.')
    .map { it.toInt() }.let { (major, minor, patch) -> major * 10000 + minor * 100 + patch }

android {
    namespace = "io.github.deathedit.tether"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.deathedit.tether"
        // Android 11: MediaCodec low-latency mode and the AYN Thor's OS.
        minSdk = 30
        targetSdk = 35
        versionCode = tetherVersionCode
        versionName = tetherVersion
        ndk {
            // Built by ../build-rust.sh into src/main/jniLibs.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    signingConfigs {
        // A fixed debug key (not a secret: it only signs debug builds), so
        // every CI build can update the previous one on a device. Each runner
        // would otherwise generate its own and installs would be refused.
        getByName("debug") {
            storeFile = file("debug.keystore")
            storeType = "PKCS12"
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
        // The release key is a secret: CI writes it from GitHub secrets and
        // passes its path and passwords in these variables. Without them the
        // release APK is built unsigned.
        val releaseKeystore = System.getenv("TETHER_KEYSTORE")
        if (!releaseKeystore.isNullOrEmpty()) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = System.getenv("TETHER_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("TETHER_KEY_ALIAS")
                keyPassword = System.getenv("TETHER_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = signingConfigs.findByName("release")
        }
    }

    buildFeatures {
        compose = true
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

    implementation(platform("androidx.compose:compose-bom:2025.05.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.9.0")
    implementation("androidx.savedstate:savedstate-ktx:1.3.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // QR code for signing in from a phone.
    implementation("com.google.zxing:core:3.5.3")
}
