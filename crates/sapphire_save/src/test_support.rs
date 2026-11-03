//! What the crate's tests share: their directory, and the fault injection around a write.

use std::fs::{self, File};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use sapphire_platform::{OsSteps, ReplaceSteps, durable_replace_with};

/// A directory of the test's own, removed afterward.
pub(crate) struct TestDir(PathBuf);

impl TestDir {
    pub(crate) fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("sapphire_save-{name}-{}-{number}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create the test's directory");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The names in the directory, sorted.
    pub(crate) fn listing(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .expect("list")
            .map(|entry| entry.expect("entry").file_name().into_string().expect("UTF-8"))
            .collect();
        names.sort();
        names
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The steps of a write, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Half the temp is on disk.
    WriteTemp,
    SyncTemp,
    Rename,
    /// The rename has happened, and hasn't been flushed.
    SyncDir,
}

impl Step {
    pub(crate) const ALL: [Self; 4] = [Self::WriteTemp, Self::SyncTemp, Self::Rename, Self::SyncDir];

    /// Whether a write stopped at this step has already replaced the file.
    pub(crate) fn after_rename(self) -> bool {
        self == Self::SyncDir
    }
}

/// How a write goes wrong at its step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// The step returns an error: the disk is full, say, and the game lives on.
    Fail,
    /// The process dies there, by a panic that nothing cleans up after.
    Die,
}

/// The real steps, with a fault at one.
struct Faulty {
    step: Step,
    fault: Fault,
}

impl Faulty {
    fn hit(&self, step: Step) -> io::Result<()> {
        if step != self.step {
            return Ok(());
        }
        match self.fault {
            Fault::Fail => Err(io::Error::other("injected")),
            // No panic message: there are hundreds of these, and each is expected.
            Fault::Die => panic::resume_unwind(Box::new(Died)),
        }
    }
}

impl ReplaceSteps for Faulty {
    fn write_temp(&mut self, temp: &mut File, bytes: &[u8]) -> io::Result<()> {
        let written = if self.step == Step::WriteTemp { bytes.len() / 2 } else { bytes.len() };
        OsSteps.write_temp(temp, &bytes[..written])?;
        self.hit(Step::WriteTemp)
    }

    fn sync_temp(&mut self, temp: &File) -> io::Result<()> {
        self.hit(Step::SyncTemp)?;
        OsSteps.sync_temp(temp)
    }

    fn rename(&mut self, temp: &Path, target: &Path) -> io::Result<()> {
        self.hit(Step::Rename)?;
        OsSteps.rename(temp, target)
    }

    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        self.hit(Step::SyncDir)?;
        OsSteps.sync_dir(dir)
    }
}

struct Died;

/// A `replace` with a fault at one step.
pub(crate) fn faulty_replace(
    step: Step,
    fault: Fault,
) -> impl Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync + 'static {
    move |target, bytes| durable_replace_with(target, bytes, &mut Faulty { step, fault })
}

/// Runs `write`, which may die by [`Fault::Die`]. True if it did.
pub(crate) fn died(write: impl FnOnce()) -> bool {
    match panic::catch_unwind(AssertUnwindSafe(write)) {
        Ok(()) => false,
        Err(payload) if payload.is::<Died>() => true,
        Err(payload) => panic::resume_unwind(payload),
    }
}
