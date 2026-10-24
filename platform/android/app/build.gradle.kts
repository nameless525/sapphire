plugins {
    id("com.android.application")
}

android {
    namespace = "dev.sapphire.sandbox"
    compileSdk = 36
    // xtask passes the NDK it linked the app library with. AGP needs one only to strip the
    // packaged `.so`, and otherwise looks for its own default NDK version.
    providers.gradleProperty("sapphire.ndkPath").orNull?.let { ndkPath = it }
    providers.gradleProperty("sapphire.ndkVersion").orNull?.let { ndkVersion = it }

    defaultConfig {
        applicationId = "dev.sapphire.sandbox"
        minSdk = 29 // Android 10 (spec §2).
        targetSdk = 36
        versionCode = 1
        // The workspace version (Cargo.toml), passed by xtask as it is for Apple bundles.
        versionName = providers.gradleProperty("sapphire.version").getOrElse("0.0.0-dev")
        ndk {
            abiFilters += "arm64-v8a" // 64-bit only (spec §6.7).
        }
    }

    buildTypes {
        release {
            // M0: release builds are for profiling on test devices. Store signing and the
            // AAB arrive with `xtask package` (spec §7.4).
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
