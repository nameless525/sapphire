// Android packaging for the apps (spec §7.3). Build it through
// `cargo xtask bundle --platform android [--app <package>]`, which first generates the inputs
// under target/android/: the app's activity, SDL's Java glue, its `.so` in `jniLibs/` and the
// mobile cook in `assets/content/`.

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode = RepositoriesMode.FAIL_ON_PROJECT_REPOS
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "sapphire-android"
include(":app")
