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

// Play's upload key is the maintainer's (m4-plan §3, "Android packaging"). It reaches the store build only
// through these variables, never the repo or Gradle's properties, and xtask builds with the configuration
// cache off so its passwords aren't stored there. Without it the store build signs with the debug key,
// which every local check takes and Play refuses.
val uploadKeystore = providers.environmentVariable("SAPPHIRE_UPLOAD_KEYSTORE").orNull
fun uploadSecret(name: String): String =
    providers.environmentVariable(name).orNull
        ?: error("SAPPHIRE_UPLOAD_KEYSTORE is set, so $name must be too")

android {
    // The app's identity, from platform/identity.toml. xtask writes the activity into this Java package.
    namespace = sapphire("applicationId")
    compileSdk = 36
    // The NDK xtask linked the app library with; AGP uses it to strip the `.so` and to split out its
    // symbols.
    ndkPath = sapphire("ndkPath")
    ndkVersion = sapphire("ndkVersion")

    defaultConfig {
        applicationId = sapphire("applicationId")
        minSdk = 29 // Android 10 (spec §2).
        targetSdk = 36
        // The workspace's version, and the build number xtask counts from git (spec §7.3).
        versionCode = sapphire("versionCode").toInt()
        versionName = sapphire("versionName")
        manifestPlaceholders["activity"] = sapphire("activity")
        ndk {
            abiFilters += "arm64-v8a" // 64-bit only (spec §6.7).
        }
    }

    // xtask's generated inputs (under target/), kept out of the source tree: the app's activity, SDL's
    // Java glue, the app library, the mobile cook, and the resources naming the app in each language
    // and holding its icon.
    sourceSets.getByName("main") {
        val gen = sapphire("genDir")
        java.directories += "$gen/java"
        jniLibs.directories += "$gen/jniLibs"
        assets.directories += "$gen/assets"
        res.directories += "$gen/res"
    }

    // Packs compress their own blocks. Stored as they are, the IO thread's reads of a block range
    // seek straight to it; deflated by the APK, every read would inflate the pack from its start.
    androidResources {
        noCompress += "spack"
    }

    signingConfigs {
        if (uploadKeystore != null) {
            create("upload") {
                storeFile = file(uploadKeystore)
                storePassword = uploadSecret("SAPPHIRE_UPLOAD_KEYSTORE_PASSWORD")
                keyAlias = uploadSecret("SAPPHIRE_UPLOAD_KEY_ALIAS")
                keyPassword = uploadSecret("SAPPHIRE_UPLOAD_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        // The store build, which `xtask package` makes with bundleRelease: not debuggable, so the activity
        // takes no launch arguments, signed with the upload key when the environment has it, and carrying
        // the library's symbols for Play's crash reports (`BUNDLE-METADATA/…/debugsymbols/`).
        release {
            isDebuggable = false
            signingConfig = signingConfigs.findByName("upload") ?: signingConfigs.getByName("debug")
            ndk {
                debugSymbolLevel = "FULL"
            }
        }
        // Profiling builds for test devices (`xtask bundle|run --release`): the store build, but debuggable
        // so the activity takes launch arguments, always signed with the debug key, and with no symbols
        // to split out.
        create("profile") {
            initWith(getByName("release"))
            isDebuggable = true
            signingConfig = signingConfigs.getByName("debug")
            ndk {
                debugSymbolLevel = "NONE"
            }
        }
    }

    // Release lint fails the store build on any error. lint.xml scopes out SDL's synced Java glue's
    // findings, rule by rule with each reason, and says why each of ours that's left is.
    lint {
        lintConfig = file("lint.xml")
        abortOnError = true
        checkReleaseBuilds = true
        textReport = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// bundletool, as AGP itself runs it: the plugin's own classpath, which Gradle resolved (offline, from its
// cache) to load AGP, holds bundletool and everything it needs. xtask runs it from the list this writes to
// check the store bundle and make its local-testing APKs, with no download.
val agpClasspath: FileCollection = rootProject.buildscript.configurations.getByName("classpath")
val bundletoolClasspath = tasks.register("bundletoolClasspath") {
    val classpath = agpClasspath
    val list = layout.buildDirectory.file("bundletool.classpath")
    inputs.files(classpath)
    outputs.file(list)
    doLast {
        list.get().asFile.writeText(classpath.files.joinToString("\n", postfix = "\n") { it.absolutePath })
    }
}
