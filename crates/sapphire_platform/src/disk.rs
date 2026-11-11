//! What a directory's disk can take, and keeping a directory out of the OS's backup (M4 P4): what a patch asks
//! before it assembles packs, and of the directory it installs into.

use std::io;
use std::path::Path;

/// Bytes the volume holding `path` has free for this app: what the OS lets an unprivileged process write, not
/// the blocks reserved for its own use. `path` must exist.
pub fn free_space(path: &Path) -> io::Result<u64> {
    os_free_space(path)
}

#[cfg(unix)]
fn os_free_space(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path with a NUL byte"))?;
    // SAFETY: `statvfs` is plain old data, which zeroes make valid, and the call writes it whole on success.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid C string, and `stats` a valid place to write one `statvfs`.
    if unsafe { libc::statvfs(path.as_ptr(), &raw mut stats) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // The fields' widths differ by OS; both are at most 64 bits.
    #[expect(clippy::useless_conversion, reason = "u64 on some targets, narrower on others")]
    let (available, fragment) = (u64::from(stats.f_bavail), u64::from(stats.f_frsize));
    Ok(available.saturating_mul(fragment))
}

#[cfg(windows)]
fn os_free_space(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut available = 0u64;
    // SAFETY: `wide` is NUL-terminated, and the one out-pointer given is valid; the others may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(wide.as_ptr(), &raw mut available, std::ptr::null_mut(), std::ptr::null_mut())
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(available) }
}

/// Keeps `dir` and everything in it out of the OS's backups, for data the app fetches again: on iOS,
/// `NSURLIsExcludedFromBackupKey`, which Apple asks of anything downloaded into Application Support (iCloud
/// and the computer's backups). Nothing elsewhere: Android's backup rules leave the directory out in the
/// manifest's own `data_extraction_rules.xml` and `backup_rules.xml`, and a desktop has no backup of its
/// own. `dir` must exist.
pub fn exclude_from_backup(dir: &Path) -> io::Result<()> {
    os_exclude_from_backup(dir)
}

#[cfg(target_os = "ios")]
fn os_exclude_from_backup(dir: &Path) -> io::Result<()> {
    use objc2_foundation::{NSNumber, NSString, NSURL, NSURLIsExcludedFromBackupKey};

    let url = NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&dir.to_string_lossy()), true);
    let excluded = NSNumber::numberWithBool(true);
    // SAFETY: the key's value is an NSNumber holding a bool, as Foundation documents it; the key is
    // Foundation's own constant.
    unsafe { url.setResourceValue_forKey_error(Some(&excluded), NSURLIsExcludedFromBackupKey) }
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

#[cfg(not(target_os = "ios"))]
fn os_exclude_from_backup(dir: &Path) -> io::Result<()> {
    // Checked as iOS would, so a missing directory fails the same everywhere.
    std::fs::metadata(dir).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_has_free_space_and_a_missing_one_fails() {
        let dir = std::env::temp_dir();
        let free = free_space(&dir).expect("the temporary directory's volume");
        assert!(free > 0, "{free} bytes free");
        assert!(free_space(&dir.join("sapphire_platform_no_such_dir")).is_err());
        assert!(exclude_from_backup(&dir).is_ok());
        assert!(exclude_from_backup(&dir.join("sapphire_platform_no_such_dir")).is_err());
    }
}
