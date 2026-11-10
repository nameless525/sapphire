//! Writes a session's files off the frame's path, each whole, one batch at a time.
//!
//! A batch in flight is never joined by another: two could land out of order and the older win. What's asked
//! for meanwhile waits, the newest of each file replacing what waited for it, and goes once the batch in
//! flight reports back. Making the bytes (a replay's encoding, a trail's checksums, the document's text) is the
//! job's too, so the frame pays only for copies.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use sapphire_gameplay::replay::{ARCHITECTURE, ChecksumFile, RecordedTrail, Replay, Trail};

use crate::LOG_TARGET;
use crate::document::SessionDocument;
use crate::session::Replace;

/// What one file is to hold, as the frame hands it over.
pub(crate) enum Content {
    Text(String),
    Document(Box<SessionDocument>),
    Replay(Replay),
    /// A run's checksums file, holding its trail under its name.
    Checksums {
        name: String,
        trail: Trail,
    },
}

impl Content {
    fn into_bytes(self) -> Result<Vec<u8>, String> {
        Ok(match self {
            Self::Text(text) => text.into_bytes(),
            Self::Document(document) => {
                sapphire_serialize::to_ron(&*document).map_err(|error| error.to_string())?.into_bytes()
            }
            Self::Replay(replay) => replay.encode(),
            Self::Checksums { name, trail } => {
                let mut file = ChecksumFile::new(ARCHITECTURE);
                file.set(&name, RecordedTrail::from_trail(&trail));
                file.to_text().into_bytes()
            }
        })
    }
}

/// What a session's writes have come to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    /// Files written whole.
    pub written: u32,
    /// Files that weren't, each logged. The document and the frames go again at the next keep, and a run's
    /// files once it has moved.
    pub failed: u32,
    /// Files a newer copy replaced before they were written.
    pub overtaken: u32,
}

/// Each file a batch wrote, with its error if it had one. A batch writes in its paths' order, so a session's
/// `runs/` go before its `session.ron`, which names them.
type Written = Vec<(PathBuf, Option<String>)>;

/// See the module.
pub(crate) struct Writer {
    replace: Replace,
    /// What's asked for and not yet in flight, by path.
    waiting: BTreeMap<PathBuf, Content>,
    /// The batch in flight: each file's error, if it had one.
    in_flight: Option<Receiver<Written>>,
    stats: WriterStats,
}

impl Writer {
    pub(crate) fn new(replace: Replace) -> Self {
        Self { replace, waiting: BTreeMap::new(), in_flight: None, stats: WriterStats::default() }
    }

    pub(crate) fn stats(&self) -> WriterStats {
        self.stats
    }

    /// Asks for `path` to hold `content`, in place of anything waiting to go there.
    pub(crate) fn write(&mut self, path: PathBuf, content: Content) {
        if self.waiting.insert(path, content).is_some() {
            self.stats.overtaken += 1;
        }
    }

    /// Hears how the batch in flight went, and sends what's waiting once it has.
    pub(crate) fn poll(&mut self) {
        if let Some(in_flight) = &self.in_flight {
            match in_flight.try_recv() {
                Ok(results) => self.report(results),
                Err(TryRecvError::Empty) => return,
                // The job died, and its panic has been logged: what it held goes again with the next keep.
                Err(TryRecvError::Disconnected) => self.stats.failed += 1,
            }
            self.in_flight = None;
        }
        if self.waiting.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.waiting);
        let replace = self.replace.clone();
        let (report, in_flight) = mpsc::channel();
        sapphire_jobs::spawn_async(move || {
            let _ = report.send(write_batch(&replace, batch));
        });
        self.in_flight = Some(in_flight);
    }

    /// Waits for the batch in flight, then writes what's waiting, here.
    pub(crate) fn finish(&mut self) {
        if let Some(in_flight) = self.in_flight.take() {
            match in_flight.recv() {
                Ok(results) => self.report(results),
                Err(_) => self.stats.failed += 1,
            }
        }
        let batch = std::mem::take(&mut self.waiting);
        let results = write_batch(&self.replace, batch);
        self.report(results);
    }

    fn report(&mut self, results: Written) {
        for (path, error) in results {
            match error {
                None => self.stats.written += 1,
                Some(error) => {
                    self.stats.failed += 1;
                    tracing::warn!(target: LOG_TARGET, path = %path.display(), %error, "a session's file wasn't written");
                }
            }
        }
    }
}

/// Writes each file of `batch` whole, making its directory first. Each file's error, if it had one.
fn write_batch(replace: &Replace, batch: BTreeMap<PathBuf, Content>) -> Written {
    batch
        .into_iter()
        .map(|(path, content)| {
            let written = content.into_bytes().and_then(|bytes| {
                if let Some(dir) = path.parent() {
                    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
                }
                replace(&path, &bytes).map_err(|error| error.to_string())
            });
            (path, written.err())
        })
        .collect()
}
