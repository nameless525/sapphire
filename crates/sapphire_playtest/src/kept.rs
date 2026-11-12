//! A session as a host reads it back (M4 F3): its document, its frames and each run's replay and checksums,
//! all as of the last keep the document says it had.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sapphire_gameplay::replay::{ChecksumFile, Replay, checksums_file_name};

use crate::document::{KeptRun, SessionDocument};
use crate::frames::{FrameRow, FramesError};
use crate::{DOCUMENT_FILE, FRAMES_FILE, journal};

/// A session that can't be read at all. A run that can't is said so on its own ([`RunFiles`]).
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("reading {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    Document { path: PathBuf, source: Box<sapphire_serialize::DocumentError> },
    #[error("{path}: {source}")]
    Frames { path: PathBuf, source: FramesError },
    #[error("{path} holds {found} rows, short of the {expected} the session's document names")]
    FewerRows { path: PathBuf, found: usize, expected: u32 },
}

/// A run's file and the checksums the device reached, as of the document's ticks.
#[derive(Clone, Debug, PartialEq)]
pub struct RunFiles {
    pub replay: Replay,
    /// The run's checksums, under the file's stem, for the device's architecture.
    pub checksums: ChecksumFile,
}

/// A kept session, read: see the module.
#[derive(Clone, Debug)]
pub struct Kept {
    pub document: SessionDocument,
    /// `frames.csv`'s rows that the document counts.
    pub frames: Vec<FrameRow>,
    /// Each of the document's runs, in its order, or why it doesn't read.
    pub runs: Vec<Result<RunFiles, String>>,
}

impl Kept {
    /// Reads the session in `dir`, of any version.
    pub fn read(dir: &Path) -> Result<Self, ReadError> {
        let path = dir.join(DOCUMENT_FILE);
        let text =
            fs::read_to_string(&path).map_err(|source| ReadError::Read { path: path.clone(), source })?;
        let document = SessionDocument::from_ron(&text)
            .map_err(|source| ReadError::Document { path, source: Box::new(source) })?;
        let path = dir.join(FRAMES_FILE);
        let mut frames = match fs::read_to_string(&path) {
            Ok(text) => FrameRow::read_all(&text)
                .map_err(|source| ReadError::Frames { path: path.clone(), source })?,
            // A session killed before its first keep.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(ReadError::Read { path, source }),
        };
        if let Some(rows) = document.frames {
            if frames.len() < rows as usize {
                return Err(ReadError::FewerRows { path, found: frames.len(), expected: rows });
            }
            frames.truncate(rows as usize);
        }
        let architecture = document.device.architecture.clone();
        let runs = document.runs.iter().map(|run| read_run(dir, run, &architecture)).collect();
        Ok(Self { document, frames, runs })
    }

    /// Writes each run's file and checksums where the document names them (`runs/<n>/<name>.sreplay` and
    /// `checksums.<architecture>.txt`), from its journal: so `sapphire_replay check --dir runs/<n>` plays it,
    /// and `--replay` draws it. A run that doesn't read is passed over, and a session from before version 2
    /// already has them.
    pub fn settle(&self, dir: &Path) -> io::Result<()> {
        for (run, files) in self.document.runs.iter().zip(&self.runs) {
            let (Some(_), Ok(files)) = (&run.journal, files) else { continue };
            let file = dir.join(&run.file);
            let checksums = file.with_file_name(checksums_file_name(files.checksums.architecture()));
            fs::write(&file, files.replay.encode())?;
            fs::write(checksums, files.checksums.to_text())?;
        }
        Ok(())
    }
}

/// The run's stem, which its checksums are under.
fn stem(file: &str) -> &str {
    let name = file.rsplit('/').next().unwrap_or(file);
    name.strip_suffix(".sreplay").unwrap_or(name)
}

/// One run's file and checksums, from its journal or, before version 2, the files themselves.
fn read_run(dir: &Path, run: &KeptRun, architecture: &str) -> Result<RunFiles, String> {
    let name = stem(&run.file);
    let (replay, checksums) = match &run.journal {
        Some(path) => {
            let bytes = fs::read(dir.join(path)).map_err(|error| format!("reading {path}: {error}"))?;
            journal::read(&bytes, (run.ticks, run.ended), name, architecture)
                .map_err(|error| format!("{path}: {error}"))?
        }
        None => {
            let file = dir.join(&run.file);
            let bytes = fs::read(&file).map_err(|error| format!("reading it: {error}"))?;
            let replay = Replay::decode(&bytes).map_err(|error| error.to_string())?;
            // Another architecture's checksums are read where they are, if they are.
            let path = file.with_file_name(checksums_file_name(architecture));
            let checksums = match fs::read_to_string(&path) {
                Ok(text) => {
                    ChecksumFile::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?
                }
                Err(error) => return Err(format!("reading {}: {error}", path.display())),
            };
            (replay, checksums)
        }
    };
    // Whether a replay here reaches them is for whoever plays it.
    let recorded = checksums.get(name).ok_or_else(|| format!("its checksums have no run named {name}"))?;
    if recorded.ticks() != run.ticks {
        return Err(format!(
            "its checksums hold {} ticks, and the session says {}",
            recorded.ticks(),
            run.ticks
        ));
    }
    if replay.ticks() != run.ticks {
        return Err(format!("its file holds {} ticks, and the session says {}", replay.ticks(), run.ticks));
    }
    Ok(RunFiles { replay, checksums })
}
