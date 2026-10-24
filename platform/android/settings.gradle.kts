// Android packaging for the sandbox (spec §7.3). Build it through
// `cargo xtask bundle --platform android`, which first generates the gitignored inputs:
// SDL's Java glue, the app's `.so` in `jniLibs/` and the mobile cook in `assets/content/`.

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
