//! Replacing a file whole (M3 V1): what saves and settings reach the disk through, and the one
//! place that knows how each OS makes a rename stick.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Replaces `target` with `bytes` so that a crash at any moment leaves the old file or the new
/// one, whole: write a temp file beside it (`<target>.tmp`), flush the temp to the disk, rename
/// it over `target`, flush the directory.
///
/// A failed step returns its error with the old file in place, unless it was the last, the
/// directory's flush: the rename has happened by then, so `target` is the new file, though a
/// power cut could still undo it. Retry from `bytes`, never by flushing again: Linux forgets a
/// failed `fsync`'s dirty pages, so a second `fsync` can succeed with nothing on the disk, and
/// a retry here writes a new temp. A process killed part way leaves a stale temp, which the next
/// call for the same `target` overwrites. Two calls for one `target` at once would share a temp,
/// so the caller keeps them apart.
///
/// **Where each OS's guarantee stops.** A process's death alone can't damage either file on any
/// of them: the temp's contents never show at `target` until the rename, and the rename is
/// atomic. For a power cut:
///
/// | OS | Flushes | Stops at |
/// |---|---|---|
/// | macOS, iOS | `F_FULLFSYNC` on the temp and on the directory (`File::sync_all`), then APFS's atomic rename | A drive that lies about its cache |
/// | Linux, Android | `fsync` on the temp and the directory | The same, and a filesystem that refuses a directory's `fsync`, which is taken as done: the rename may then be lost, though never torn |
/// | Windows | `FlushFileBuffers` on the temp, then `MoveFileExW` with `MOVEFILE_WRITE_THROUGH`, since a directory has no handle to flush | The same, and Microsoft documents write-through's flush but not the replace's atomicity across a power cut; NTFS's journal is what provides it |
///
/// On Windows the rename also waits about a third of a second for another process that has the
/// temp or `target` open to let go, as an indexer or a virus scanner does for a moment.
///
/// No test here can cut the power: what's proved is a kill at every step, by fault injection and
/// by `SIGKILL`.
pub fn durable_replace(target: &Path, bytes: &[u8]) -> io::Result<()> {
    durable_replace_with(target, bytes, &mut OsSteps)
}

/// [`durable_replace`] through `steps`, so a test can make any step fail or stop. A step that
/// returns an error gets the temp cleaned up; one that panics is a process dying there, and
/// leaves it.
pub fn durable_replace_with(target: &Path, bytes: &[u8], steps: &mut impl ReplaceSteps) -> io::Result<()> {
    let temp = target.with_added_extension("tmp");
    let replaced = replace(&temp, target, bytes, steps);
    if replaced.is_err() {
        // Gone already if only the directory's flush failed, and the next write overwrites it if
        // this fails too, so there's nothing to report.
        let _ = fs::remove_file(&temp);
    }
    replaced
}

fn replace(temp: &Path, target: &Path, bytes: &[u8], steps: &mut impl ReplaceSteps) -> io::Result<()> {
    let mut file = File::create(temp)?;
    steps.write_temp(&mut file, bytes)?;
    steps.sync_temp(&file)?;
    // Windows can't rename a file that's still open.
    drop(file);
    steps.rename(temp, target)?;
    steps.sync_dir(containing_dir(target))
}

/// The directory holding `file`. `Path::parent` of a bare name is empty, which no OS opens.
fn containing_dir(file: &Path) -> &Path {
    file.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// The steps of [`durable_replace`], in order. The provided methods are the real ones, so a fault
/// injector overrides the step it's after and calls [`OsSteps`] for what it lets through.
pub trait ReplaceSteps {
    /// Writes the new contents into the freshly truncated temp.
    fn write_temp(&mut self, temp: &mut File, bytes: &[u8]) -> io::Result<()> {
        temp.write_all(bytes)
    }

    /// Flushes the temp's contents to the disk.
    fn sync_temp(&mut self, temp: &File) -> io::Result<()> {
        temp.sync_all()
    }

    /// Renames the closed temp over the target.
    fn rename(&mut self, temp: &Path, target: &Path) -> io::Result<()> {
        rename_over(temp, target)
    }

    /// Flushes the rename to the disk.
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        sync_dir(dir)
    }
}

/// The real steps.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsSteps;

impl ReplaceSteps for OsSteps {}

#[cfg(not(target_os = "windows"))]
fn rename_over(temp: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temp, target)
}

/// The waits before each new try at the rename while another process has the temp or the target
/// open, as an indexer or a virus scanner does for a moment after a file's written: about a third
/// of a second in all, and then its error stands.
#[cfg(target_os = "windows")]
const RENAME_WAITS: [std::time::Duration; 5] = [
    std::time::Duration::from_millis(10),
    std::time::Duration::from_millis(20),
    std::time::Duration::from_millis(40),
    std::time::Duration::from_millis(80),
    std::time::Duration::from_millis(160),
];

/// `std::fs::rename` has no way to ask for write-through, and there's no directory to flush
/// afterward, so this is the rename's only chance to reach the disk. A file another process holds
/// gets [`RENAME_WAITS`] to be let go.
#[cfg(target_os = "windows")]
fn rename_over(temp: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let wide = |path: &Path| path.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (temp, target) = (wide(temp), wide(target));
    let mut waits = RENAME_WAITS.iter();
    loop {
        // SAFETY: both are NUL-terminated wide strings that outlive the call.
        let moved = unsafe {
            MoveFileExW(temp.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)
        };
        if moved != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        match waits.next() {
            Some(wait) if is_held(&error) => std::thread::sleep(*wait),
            _ => return Err(error),
        }
    }
}

/// Whether a rename failed because another process has one of the files open: Windows says
/// `ERROR_SHARING_VIOLATION` (32) for a held temp, and `ERROR_ACCESS_DENIED` (5) for a target open
/// without `FILE_SHARE_DELETE`. A real lack of permission is 5 as well, and costs only the waits.
#[cfg(target_os = "windows")]
fn is_held(error: &io::Error) -> bool {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    matches!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION))
}

#[cfg(not(target_os = "windows"))]
fn sync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir)?.sync_all() {
        // Some filesystems can't flush a directory, and refuse with EINVAL or ENOTSUP. A save
        // on one of them is as durable as that filesystem allows, which is no reason to fail it.
        Err(error) if matches!(error.kind(), io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported) => {
            Ok(())
        }
        result => result,
    }
}

/// Nothing to do: `rename_over`'s write-through has flushed the rename.
#[cfg(target_os = "windows")]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};
    use std::path::PathBuf;

    use super::*;

    /// A directory of the test's own, removed afterward.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sapphire_platform-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create the test's directory");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("file")
        }

        fn temp(&self) -> PathBuf {
            self.0.join("file.tmp")
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Fails or stops at one step, after writing half the temp if that's the step.
    struct Fault {
        step: usize,
        stop: bool,
    }

    impl Fault {
        fn hit(&self, step: usize) -> io::Result<()> {
            if step != self.step {
                return Ok(());
            }
            if self.stop {
                panic::resume_unwind(Box::new("the process dies here"));
            }
            Err(io::Error::other("injected"))
        }
    }

    impl ReplaceSteps for Fault {
        fn write_temp(&mut self, temp: &mut File, bytes: &[u8]) -> io::Result<()> {
            let written = if self.step == 0 { bytes.len() / 2 } else { bytes.len() };
            OsSteps.write_temp(temp, &bytes[..written])?;
            self.hit(0)
        }

        fn sync_temp(&mut self, temp: &File) -> io::Result<()> {
            self.hit(1)?;
            OsSteps.sync_temp(temp)
        }

        fn rename(&mut self, temp: &Path, target: &Path) -> io::Result<()> {
            self.hit(2)?;
            OsSteps.rename(temp, target)
        }

        fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
            self.hit(3)?;
            OsSteps.sync_dir(dir)
        }
    }

    #[test]
    fn creates_and_replaces_with_no_temp_left() {
        let dir = TestDir::new("replaces");
        durable_replace(&dir.file(), b"first").expect("create");
        assert_eq!(fs::read(dir.file()).expect("read"), b"first");
        durable_replace(&dir.file(), b"second, and longer").expect("replace");
        assert_eq!(fs::read(dir.file()).expect("read"), b"second, and longer");
        assert!(!dir.temp().exists());
    }

    #[test]
    fn a_stale_temp_is_overwritten() {
        let dir = TestDir::new("stale");
        fs::write(dir.temp(), b"a much longer temp a killed process left behind").expect("write");
        durable_replace(&dir.file(), b"new").expect("replace");
        assert_eq!(fs::read(dir.file()).expect("read"), b"new");
        assert!(!dir.temp().exists());
    }

    #[test]
    fn a_failed_step_keeps_the_old_file_and_cleans_up() {
        let dir = TestDir::new("fails");
        for step in 0..4 {
            durable_replace(&dir.file(), b"old").expect("set up");
            let result = durable_replace_with(&dir.file(), b"new", &mut Fault { step, stop: false });
            assert!(result.is_err(), "step {step}");
            // The rename comes before the last step.
            let expected: &[u8] = if step == 3 { b"new" } else { b"old" };
            assert_eq!(fs::read(dir.file()).expect("read"), expected, "step {step}");
            assert!(!dir.temp().exists(), "step {step}");
        }
    }

    #[test]
    fn a_process_dying_at_a_step_keeps_a_whole_file() {
        let dir = TestDir::new("stops");
        for step in 0..4 {
            durable_replace(&dir.file(), b"old").expect("set up");
            let died = panic::catch_unwind(AssertUnwindSafe(|| {
                durable_replace_with(&dir.file(), b"new", &mut Fault { step, stop: true })
            }));
            assert!(died.is_err(), "step {step}");
            let expected: &[u8] = if step == 3 { b"new" } else { b"old" };
            assert_eq!(fs::read(dir.file()).expect("read"), expected, "step {step}");
            assert_eq!(dir.temp().exists(), step != 3, "step {step}: a death leaves its temp");
            durable_replace(&dir.file(), b"after").expect("the next write");
            assert_eq!(fs::read(dir.file()).expect("read"), b"after", "step {step}");
        }
    }

    /// Another process holding the target open, as an indexer or a virus scanner does for a moment,
    /// makes the rename wait and try again; one that keeps it open gets its error back, with the old
    /// file kept.
    #[cfg(target_os = "windows")]
    #[test]
    fn a_rename_waits_out_a_file_held_for_a_moment() {
        use std::os::windows::fs::OpenOptionsExt as _;

        let dir = TestDir::new("held");
        durable_replace(&dir.file(), b"old").expect("set up");
        let hold =
            || fs::OpenOptions::new().read(true).share_mode(0).open(dir.file()).expect("hold the target");

        let held = hold();
        let error = durable_replace(&dir.file(), b"new").expect_err("held throughout");
        assert!(is_held(&error), "{error}");
        drop(held);
        assert_eq!(fs::read(dir.file()).expect("read"), b"old");
        assert!(!dir.temp().exists());

        let held = hold();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            drop(held);
        });
        durable_replace(&dir.file(), b"new").expect("the hold ends in time");
        release.join().expect("the hold's thread");
        assert_eq!(fs::read(dir.file()).expect("read"), b"new");
    }

    #[test]
    fn a_bare_file_name_lives_in_the_current_directory() {
        assert_eq!(containing_dir(Path::new("save")), Path::new("."));
        assert_eq!(containing_dir(Path::new("user/save")), Path::new("user"));
    }
}
