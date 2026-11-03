plugins {
    id("com.android.application")
}

// `cargo xtask bundle --platform android` passes everything that varies per app or per
// build as -Psapphire.* properties, including where its generated inputs are; the
// project doesn't build without them (spec §7.3).
fun sapphire(name: String): String =
    providers.gradleProperty("sapphire.$name").orNull
        ?: error("missing -Psapphire.$name: build with `cargo xtask bundle --platform android`")

// Each app builds in a directory of its own under target/android/, so switching apps keeps the
// other's outputs and nothing lands in the source tree.
layout.buildDirectory.set(file(sapphire("buildDir")))

android {
    // The app's identity, from its Cargo.toml. xtask writes the activity into this Java package.
    namespace = sapphire("applicationId")
    compileSdk = 36
    // The NDK xtask linked the app library with; AGP uses it only to strip the `.so`.
    ndkPath = sapphire("ndkPath")
    ndkVersion = sapphire("ndkVersion")

    defaultConfig {
        applicationId = sapphire("applicationId")
        minSdk = 29 // Android 10 (spec §2).
        targetSdk = 36
        versionCode = sapphire("versionCode").toInt()
        versionName = sapphire("versionName")
        manifestPlaceholders["appLabel"] = sapphire("displayName")
        manifestPlaceholders["activity"] = sapphire("activity")
        ndk {
            abiFilters += "arm64-v8a" // 64-bit only (spec §6.7).
        }
    }

    // xtask's generated inputs (under target/android/), kept out of the source tree: the app's
    // activity, SDL's Java glue, the app library and the mobile cook.
    sourceSets.getByName("main") {
        val gen = sapphire("genDir")
        java.directories += "$gen/java"
        jniLibs.directories += "$gen/jniLibs"
        assets.directories += "$gen/assets"
    }

    // Packs compress their own blocks. Stored as they are, the IO thread's reads of a block range
    // seek straight to it; deflated by the APK, every read would inflate the pack from its start.
    androidResources {
        noCompress += "spack"
    }

    buildTypes {
        release {
            // M0 release builds are for profiling on test devices: signed with the debug key,
            // and debuggable so the activity accepts launch arguments. Store builds, from
            // `xtask package` (M4), are neither.
            isDebuggable = true
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
