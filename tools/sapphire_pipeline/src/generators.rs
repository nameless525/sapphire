//! What writes the sources a cook then reads: each generator's table of [`Output`]s says what it makes and
//! how, so naming a file and making it are one row, and the check for what's missing asks nothing else.
//! Saying what's there costs a few hundred small allocations and a stat each: no generator's made until one
//! of its files is missing.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use crate::samples::{Existing, Generated, write_files};
use crate::{combat, lookdev, samples};

type Make = Box<dyn FnOnce() -> io::Result<Vec<u8>>>;

/// A file a generator writes: its name, relative to the generator's directory, and how to make its bytes.
pub struct Output {
    pub name: String,
    make: Make,
}

impl Output {
    pub fn new(name: impl Into<String>, make: impl FnOnce() -> io::Result<Vec<u8>> + 'static) -> Self {
        Self { name: name.into(), make: Box::new(make) }
    }

    /// One whose making can't fail.
    pub fn plain(name: impl Into<String>, make: impl FnOnce() -> Vec<u8> + 'static) -> Self {
        Self::new(name, move || Ok(make()))
    }
}

/// Makes `outputs`: their names and bytes.
pub fn made(outputs: Vec<Output>) -> io::Result<Vec<(String, Vec<u8>)>> {
    outputs.into_iter().map(|output| Ok((output.name, (output.make)()?))).collect()
}

/// A pipeline command that generates sources, the directory it writes (relative to the content directory),
/// and what it writes there, for that directory, which some read their inputs under.
#[derive(Clone, Copy, Debug)]
pub struct Generator {
    pub command: &'static str,
    pub dir: &'static str,
    pub outputs: fn(&Path) -> Vec<Output>,
}

/// The generators a cook runs when one of their files is missing.
pub const GENERATORS: [Generator; 3] = [
    Generator { command: "gen-samples", dir: "samples", outputs: |_| samples::files() },
    Generator { command: "gen-lookdev", dir: "lookdev", outputs: lookdev::outputs },
    Generator { command: "gen-combat", dir: "combat", outputs: combat::outputs },
];

/// Why generation stopped.
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("failed to take the generation lock {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "missing generated source{}: {}. --locked never writes sources: run `cargo xtask content` once \
         without --locked (or `sapphire_pipeline {command} --out {}`) and commit what it generates",
        if files.len() == 1 { "" } else { "s" },
        files.iter().map(|path| path.display().to_string()).collect::<Vec<_>>().join(", "),
        dir.display()
    )]
    Missing { files: Vec<PathBuf>, command: &'static str, dir: PathBuf },
    #[error("{command} failed to write {}", dir.display())]
    Write {
        command: &'static str,
        dir: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// Runs each of `generators` with a file missing under the content directory `source`, holding the lock file
/// at `lock` from before the first check until the last generator is done (review BC-007). A run that waited
/// for the lock checks once it has it, so it finds what the other run generated. Returns the generators that
/// ran and what each did.
///
/// A `locked` run (`--locked`, so CI) never generates: a missing file is an error naming it and how to
/// generate it. Generated sources are committed, and `--locked` never writes sources.
///
/// The lock is released before a cook takes its own, so the two are never nested: a run holding a cook's lock
/// (an app running against its content) never blocks another run's generation, and generation never waits on
/// a cook.
pub fn generate_missing(
    lock: &Path,
    source: &Path,
    generators: &[Generator],
    locked: bool,
) -> Result<Vec<(&'static str, Generated)>, GenerateError> {
    let _lock = take_lock(lock).map_err(|source| GenerateError::Lock { path: lock.to_owned(), source })?;
    let missing_of = |generator: &Generator| -> Vec<PathBuf> {
        let dir = source.join(generator.dir);
        (generator.outputs)(&dir)
            .into_iter()
            .map(|output| dir.join(output.name))
            .filter(|path| !path.exists())
            .collect()
    };
    if locked {
        let missing: Vec<(&Generator, Vec<PathBuf>)> = generators
            .iter()
            .map(|generator| (generator, missing_of(generator)))
            .filter(|(_, files)| !files.is_empty())
            .collect();
        return match missing.first() {
            Some((generator, _)) => Err(GenerateError::Missing {
                command: generator.command,
                dir: source.join(generator.dir),
                files: missing.iter().flat_map(|(_, files)| files.iter().cloned()).collect(),
            }),
            None => Ok(Vec::new()),
        };
    }
    let mut ran = Vec::new();
    for generator in generators {
        if missing_of(generator).is_empty() {
            continue;
        }
        let dir = source.join(generator.dir);
        let generated = write_files(&dir, (generator.outputs)(&dir), Existing::Keep).map_err(|source| {
            GenerateError::Write { command: generator.command, dir: dir.clone(), source }
        })?;
        ran.push((generator.command, generated));
    }
    Ok(ran)
}

/// The lock on the file at `path`, created if it isn't there: released when the file's dropped.
fn take_lock(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::options().create(true).truncate(false).write(true).open(path)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            eprintln!("waiting for another run to finish generating sources...");
            file.lock()?;
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    use super::*;
    use crate::test_util::TempDir;

    /// Adding a file to what a generator makes is one row: it's generated when it's missing, and a locked run
    /// names it, with nothing else told of it.
    #[test]
    fn a_new_output_needs_only_its_row() {
        fn with_one(_: &Path) -> Vec<Output> {
            vec![Output::plain("a.txt", || b"a".to_vec())]
        }
        fn with_two(dir: &Path) -> Vec<Output> {
            let mut outputs = with_one(dir);
            outputs.push(Output::plain("deep/added.txt", || b"added".to_vec()));
            outputs
        }
        let scratch = TempDir::new("generators_row");
        let lock = scratch.path().join("generate.lock");
        let source = scratch.path();
        let before = [Generator { command: "gen-test", dir: "gen", outputs: with_one }];
        let after = [Generator { command: "gen-test", dir: "gen", outputs: with_two }];
        generate_missing(&lock, source, &before, false).unwrap();
        assert!(generate_missing(&lock, source, &before, true).unwrap().is_empty(), "nothing's missing");

        let added = source.join("gen/deep/added.txt");
        let error = generate_missing(&lock, source, &after, true).unwrap_err();
        assert!(matches!(error, GenerateError::Missing { .. }), "{error}");
        assert!(error.to_string().contains(&added.display().to_string()), "{error}");
        assert!(!added.exists(), "--locked never writes");

        let ran = generate_missing(&lock, source, &after, false).unwrap();
        assert_eq!(ran[0].0, "gen-test");
        assert_eq!(ran[0].1.created, std::slice::from_ref(&added));
        assert_eq!(fs::read(&added).unwrap(), b"added");
    }

    #[test]
    fn concurrent_runs_generate_once() {
        // Review BC-007: two runs that find a source missing must not both generate, nor go on to
        // cook while the other is still generating.
        static RUNS: AtomicUsize = AtomicUsize::new(0);
        fn outputs(_: &Path) -> Vec<Output> {
            vec![
                Output::plain("a.txt", || b"generated".to_vec()),
                Output::plain("nested/b.txt", || {
                    RUNS.fetch_add(1, Ordering::SeqCst);
                    // A slow generator.
                    thread::sleep(Duration::from_millis(200));
                    b"generated".to_vec()
                }),
            ]
        }
        let generators = [Generator { command: "gen-test", dir: "gen", outputs }];
        let scratch = TempDir::new("generators_once");
        let dir = scratch.path();
        fs::create_dir_all(dir.join("gen")).unwrap();
        fs::write(dir.join("gen/a.txt"), "kept").unwrap();
        let lock = dir.join("generate.lock");
        let start = Barrier::new(2);
        let ran: Vec<Vec<&str>> = thread::scope(|scope| {
            let threads: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        let ran = generate_missing(&lock, dir, &generators, false).unwrap();
                        // Whichever run generated, every source is there once either goes on.
                        assert!(dir.join("gen/a.txt").exists() && dir.join("gen/nested/b.txt").exists());
                        ran.into_iter().map(|(command, _)| command).collect()
                    })
                })
                .collect();
            threads.into_iter().map(|thread| thread.join().unwrap()).collect()
        });
        assert_eq!(RUNS.load(Ordering::SeqCst), 1, "one run generates");
        assert_eq!(ran.concat(), ["gen-test"]);
        assert_eq!(fs::read_to_string(dir.join("gen/a.txt")).unwrap(), "kept");

        // Nothing missing: nothing runs.
        assert!(generate_missing(&lock, dir, &generators, false).unwrap().is_empty());
        assert_eq!(RUNS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn locked_runs_never_generate() {
        // A missing generated source fails a locked run, naming it, and nothing is written.
        fn outputs(_: &Path) -> Vec<Output> {
            let never = || panic!("--locked never writes sources");
            vec![Output::plain("a.txt", never), Output::plain("b.txt", never)]
        }
        let generators = [Generator { command: "gen-test", dir: "gen", outputs }];
        let scratch = TempDir::new("generators_locked");
        let dir = scratch.path();
        fs::create_dir_all(dir.join("gen")).unwrap();
        fs::write(dir.join("gen/a.txt"), "committed").unwrap();
        let lock = dir.join("generate.lock");
        let error = generate_missing(&lock, dir, &generators, true).unwrap_err().to_string();
        assert!(error.contains(&dir.join("gen/b.txt").display().to_string()), "{error}");
        assert!(!error.contains("a.txt"), "only what's missing is named: {error}");
        assert!(error.contains("--locked never writes sources") && error.contains("gen-test"), "{error}");
        assert!(!dir.join("gen/b.txt").exists());

        // With everything there, a locked run goes on.
        fs::write(dir.join("gen/b.txt"), "committed").unwrap();
        assert!(generate_missing(&lock, dir, &generators, true).unwrap().is_empty());
    }

    /// What each real generator says it makes is committed, so a fresh checkout's `--locked` cook goes on.
    #[test]
    fn every_output_the_generators_declare_is_in_the_tree() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let mut count = 0;
        for generator in GENERATORS {
            let dir = content.join(generator.dir);
            for output in (generator.outputs)(&dir) {
                count += 1;
                assert!(
                    dir.join(&output.name).is_file(),
                    "{}: {} is missing",
                    generator.command,
                    output.name
                );
            }
        }
        assert!(count > 100, "{count} outputs: the tables are read");
    }
}
