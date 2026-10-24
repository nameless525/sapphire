//! Writable per-user directories: the `user://` root of spec §5.1 and its cache.
//!
//! Both are plain functions of the app's [`AppMetadata`], so crash reporting can find
//! them before SDL starts (from `fn main`, or the Android `init` hook).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use sdl3_sys::filesystem::SDL_GetPrefPath;

use crate::system::Os;
use crate::{AppMetadata, PlatformError, sdl};

/// The writable directory for settings and saves, from `SDL_GetPrefPath`. It exists on
/// return, and [`Platform::user_dir`](crate::Platform::user_dir) is the same path.
///
/// | Platform | Location |
/// |---|---|
/// | macOS | `~/Library/Application Support/<organization>/<name>/` |
/// | Windows | `%APPDATA%\<organization>\<name>\` (roams with the user profile) |
/// | Linux | `$XDG_DATA_HOME/<organization>/<name>/` (`~/.local/share/...`) |
/// | iOS | `Library/Application Support/<organization>/<name>/` in the app container (backed up) |
/// | Android | the app's internal files directory |
///
/// Changing [`AppMetadata::organization`] or [`AppMetadata::name`] moves the directory,
/// and so loses existing user data. Needs no SDL initialization: call it from `fn main`
/// before [`run`](crate::run), or from the `init` hook of
/// [`export_android_main!`](crate::export_android_main).
pub fn user_dir(metadata: &AppMetadata) -> Result<PathBuf, PlatformError> {
    let organization = sdl::c_string_lossy(metadata.organization);
    let name = sdl::c_string_lossy(metadata.name);
    // SAFETY: both are valid C strings; we take ownership of the returned SDL allocation.
    let path = unsafe { sdl::owned_string(SDL_GetPrefPath(organization.as_ptr(), name.as_ptr())) }
        .ok_or_else(|| sdl::error("SDL_GetPrefPath"))?;
    Ok(PathBuf::from(path))
}

/// The writable directory for data the engine can rebuild: pipeline caches, logs and
/// crash dumps. It exists on return, and
/// [`Platform::cache_dir`](crate::Platform::cache_dir) is the same path.
///
/// Unlike [`user_dir`], it is never backed up or roamed, and mobile OSes may empty it
/// when storage runs low.
///
/// | Platform | Location |
/// |---|---|
/// | macOS, iOS | `~/Library/Caches/<organization>/<name>/` (in the app container on iOS) |
/// | Windows | `%LOCALAPPDATA%\<organization>\<name>\` |
/// | Linux | `$XDG_CACHE_HOME/<organization>/<name>/` (`~/.cache/...`) |
/// | Android | the app's cache directory (`Context.getCacheDir()`) |
///
/// If the OS location can't be determined, it falls back to `cache/` inside
/// [`user_dir`]. Like `user_dir`, it needs no SDL initialization.
pub fn cache_dir(metadata: &AppMetadata) -> Result<PathBuf, PlatformError> {
    let dir = match os_cache_root(Os::CURRENT, |name| std::env::var_os(name)) {
        Some(root) if Os::CURRENT == Os::Android => root,
        Some(root) => root.join(metadata.organization).join(metadata.name),
        None => user_dir(metadata)?.join("cache"),
    };
    create_dir(&dir)?;
    Ok(dir)
}

fn create_dir(dir: &Path) -> Result<(), PlatformError> {
    std::fs::create_dir_all(dir).map_err(|source| PlatformError::Io { path: dir.to_owned(), source })
}

/// The OS's per-user cache root, which is already app-specific on Android. `env` reads
/// environment variables, so every branch can be tested on any host.
fn os_cache_root(os: Os, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name| env(name).map(PathBuf::from).filter(|path| path.is_absolute());
    match os {
        // On iOS, HOME is the app's container.
        Os::MacOs | Os::Ios => absolute("HOME").map(|home| home.join("Library/Caches")),
        Os::Windows => absolute("LOCALAPPDATA"),
        Os::Linux => absolute("XDG_CACHE_HOME").or_else(|| absolute("HOME").map(|home| home.join(".cache"))),
        Os::Android => android_cache_dir(),
    }
}

#[cfg(target_os = "android")]
fn android_cache_dir() -> Option<PathBuf> {
    use sdl3_sys::system::SDL_GetAndroidCachePath;
    // SAFETY: SDL_GetAndroidCachePath returns null or a string SDL caches for the
    // process; it needs only the JNI setup SDLActivity does before `SDL_main`.
    unsafe { sdl::borrowed_string(SDL_GetAndroidCachePath()) }.map(PathBuf::from)
}

#[cfg(not(target_os = "android"))]
fn android_cache_dir() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        |name| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| OsString::from(value))
    }

    #[test]
    fn cache_roots_follow_os_conventions() {
        let home = [("HOME", "/home/ana")];
        assert_eq!(os_cache_root(Os::MacOs, env(&home)), Some(PathBuf::from("/home/ana/Library/Caches")));
        assert_eq!(os_cache_root(Os::Ios, env(&home)), Some(PathBuf::from("/home/ana/Library/Caches")));
        assert_eq!(os_cache_root(Os::Linux, env(&home)), Some(PathBuf::from("/home/ana/.cache")));
        assert_eq!(
            os_cache_root(Os::Linux, env(&[("HOME", "/home/ana"), ("XDG_CACHE_HOME", "/cache")])),
            Some(PathBuf::from("/cache"))
        );
        if cfg!(windows) {
            assert_eq!(
                os_cache_root(Os::Windows, env(&[("LOCALAPPDATA", r"C:\Users\ana\AppData\Local")])),
                Some(PathBuf::from(r"C:\Users\ana\AppData\Local"))
            );
        }
    }

    #[test]
    fn relative_or_missing_locations_fall_back() {
        // XDG says relative values must be ignored.
        assert_eq!(os_cache_root(Os::Linux, env(&[("XDG_CACHE_HOME", "cache")])), None);
        assert_eq!(os_cache_root(Os::MacOs, env(&[])), None);
        assert_eq!(os_cache_root(Os::Windows, env(&[("LOCALAPPDATA", "relative")])), None);
    }

    #[test]
    fn user_and_cache_dirs_exist_and_differ() {
        let metadata = AppMetadata {
            name: "Dirs Test",
            organization: "Sapphire Tests",
            identifier: "dev.sapphire.dirs_test",
            version: "0.0.0",
        };
        let user = user_dir(&metadata).expect("user dir");
        let cache = cache_dir(&metadata).expect("cache dir");
        assert!(user.is_dir() && cache.is_dir());
        assert_ne!(user, cache);
        // Both are per-app.
        assert!(cache.ends_with("Sapphire Tests/Dirs Test"));
        for dir in [user, cache] {
            std::fs::remove_dir(&dir).expect("remove test dir");
            std::fs::remove_dir(dir.parent().expect("per-app dirs have a parent"))
                .expect("remove test org dir");
        }
    }
}
