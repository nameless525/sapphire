//! Writes a session's files off the frame's path, one batch at a time, so that the files on the disk always
//! agree with the `session.ron` there.
//!
//! A batch in flight is never joined by another: two could land out of order and the older win. What's asked
//! for meanwhile waits, the newest for each file replacing what waited for it, and goes once the batch in flight
//! reports back. Making the bytes (a run's record, the document's text) is the job's too, so the frame pays only
//! for copies.
//!
//! **What agrees (M4 F3).** A run's journal and `frames.csv` only grow ([`Disk::append`]), and the writer
//! remembers how far each is on the disk, so each batch adds only what came since; a write that fails is sent
//! again from there with the next. Every other file is replaced whole ([`Disk::replace`]). A batch writes
//! `session.ron` last, and only if everything before it in the batch was written: so the document on the disk
//! never names more than the files beside it hold (whatever asks for a run's record asks for the document with
//! it, so the two are overtaken together), and a kill at any moment leaves the document of the last
//! batch written, or the one before it, with everything it names.
//!
//! **A record overtaken (M4 F4).** A document names the records asked for before it. A run's record that
//! overtakes one waiting leaves the document waiting with it naming a record that will never be written, so
//! `Writer::write` takes that document out too, and it waits for the next. A batch sent between a record and
//! its document (a run's end once did this) then carries no document, rather than one naming ticks the
//! journal skips past.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use sapphire_gameplay::replay::{Replay, Trail};

use crate::LOG_TARGET;
use crate::document::SessionDocument;
use crate::journal::{self, Written};

/// How a session reaches the disk: `sapphire_platform`'s `durable_replace` and `durable_append` in a game,
/// which this crate doesn't link, so a test can fail or stop any write.
pub trait Disk: Send + Sync {
    /// Replaces `path` with `bytes`, so a kill at any moment leaves the old file or the new one, whole.
    fn replace(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;

    /// Writes `bytes` into `path` at `at`, making it if it isn't there and cutting anything past `at` first. A
    /// kill leaves the first `at` bytes as they were, and perhaps some of `bytes`.
    fn append(&self, path: &Path, at: u64, bytes: &[u8]) -> io::Result<()>;
}

/// What one file is to hold, as the frame hands it over.
#[derive(Clone)]
pub(crate) enum Content {
    /// Replaced whole.
    Text(String),
    /// `session.ron`, written last.
    Document(Box<SessionDocument>),
    /// A run's journal, grown to this replay and its checksums.
    Run(Box<RunKept>),
    /// `frames.csv`, grown to these pieces: the header and each keep's rows.
    Pieces(Vec<Arc<str>>),
}

/// A run as a keep found it: its file, the checksums it reached, and whether it had ended.
#[derive(Clone)]
pub(crate) struct RunKept {
    pub(crate) replay: Replay,
    pub(crate) trail: Trail,
    pub(crate) ended: bool,
}

/// How far a growing file is on the disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Grown {
    #[default]
    Nothing,
    Run(Written),
    /// Its length and how many pieces it holds.
    Pieces {
        bytes: u64,
        pieces: usize,
    },
}

/// What a session's writes have come to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriterStats {
    /// Files written, or grown.
    pub written: u32,
    /// Writes that failed, each logged. Each goes again with the next batch.
    pub failed: u32,
    /// Files a newer copy replaced before they were written.
    pub overtaken: u32,
    /// Bytes written: what the keeps cost the disk.
    pub bytes: u64,
    /// Batches written, and the most bytes one took.
    pub batches: u32,
    pub largest_batch: u64,
}

/// How one file of a batch went.
struct Outcome {
    path: PathBuf,
    /// The bytes written, or the error.
    result: Result<u64, String>,
    /// Where it has grown to, for a growing file written.
    grown: Option<Grown>,
    /// What it was to hold, for one that failed, to go again.
    content: Option<Content>,
}

/// See the module.
pub(crate) struct Writer {
    disk: Arc<dyn Disk>,
    /// What's asked for and not yet in flight, by path.
    waiting: BTreeMap<PathBuf, Content>,
    /// The growing files, by how far they are on the disk as of the last batch's report.
    grown: HashMap<PathBuf, Grown>,
    /// The batch in flight.
    in_flight: Option<Receiver<Vec<Outcome>>>,
    stats: WriterStats,
}

impl Writer {
    pub(crate) fn new(disk: Arc<dyn Disk>) -> Self {
        Self {
            disk,
            waiting: BTreeMap::new(),
            grown: HashMap::new(),
            in_flight: None,
            stats: WriterStats::default(),
        }
    }

    pub(crate) fn stats(&self) -> WriterStats {
        self.stats
    }

    /// Asks for `path` to hold `content`, in place of anything waiting to go there.
    ///
    /// A run's record that overtakes one waiting takes the waiting document with it (M4 F4): that document names
    /// the record overtaken, which now won't be written, so it waits for the next document, which names the new
    /// one. Sent, it would name ticks the run's journal skips past.
    pub(crate) fn write(&mut self, path: PathBuf, content: Content) {
        let record = matches!(content, Content::Run(_));
        let Some(overtaken) = self.waiting.insert(path, content) else { return };
        self.stats.overtaken += 1;
        if record && matches!(overtaken, Content::Run(_)) {
            let before = self.waiting.len();
            self.waiting.retain(|_, waiting| !matches!(waiting, Content::Document(_)));
            self.stats.overtaken += (before - self.waiting.len()) as u32;
        }
    }

    /// Hears how the batch in flight went, and sends what's waiting once it has.
    pub(crate) fn poll(&mut self) {
        if let Some(in_flight) = &self.in_flight {
            match in_flight.try_recv() {
                Ok(outcomes) => self.report(outcomes),
                Err(TryRecvError::Empty) => return,
                // The job died, and its panic has been logged. What it held is lost to this writer, so the
                // growing files are taken as unknown: the next batch begins each anew.
                Err(TryRecvError::Disconnected) => self.lost(),
            }
            self.in_flight = None;
        }
        if self.waiting.is_empty() {
            return;
        }
        let batch = self.batch();
        let disk = Arc::clone(&self.disk);
        let (report, in_flight) = mpsc::channel();
        sapphire_jobs::spawn_async(move || {
            let _ = report.send(write_batch(&*disk, batch));
        });
        self.in_flight = Some(in_flight);
    }

    /// Waits for the batch in flight, then writes what's waiting, here.
    pub(crate) fn finish(&mut self) {
        self.wait_in_flight();
        if !self.waiting.is_empty() {
            let batch = self.batch();
            let outcomes = write_batch(&*self.disk, batch);
            self.report(outcomes);
        }
    }

    /// Waits for the batch in flight and hears how it went, sending nothing.
    pub(crate) fn wait_in_flight(&mut self) {
        if let Some(in_flight) = self.in_flight.take() {
            match in_flight.recv() {
                Ok(outcomes) => self.report(outcomes),
                Err(_) => self.lost(),
            }
        }
    }

    /// What's waiting, each growing file with how far it is.
    fn batch(&mut self) -> Vec<(PathBuf, Grown, Content)> {
        std::mem::take(&mut self.waiting)
            .into_iter()
            .map(|(path, content)| {
                let grown = self.grown.get(&path).copied().unwrap_or_default();
                (path, grown, content)
            })
            .collect()
    }

    fn lost(&mut self) {
        self.stats.failed += 1;
        // A file begun again from nothing is cut to nothing first, so nothing half known stays in it.
        self.grown.clear();
    }

    fn report(&mut self, outcomes: Vec<Outcome>) {
        let mut batch = 0;
        for Outcome { path, result, grown, content } in outcomes {
            match result {
                Ok(bytes) => {
                    self.stats.written += 1;
                    batch += bytes;
                    if let Some(grown) = grown {
                        self.grown.insert(path, grown);
                    }
                }
                Err(error) => {
                    self.stats.failed += 1;
                    tracing::warn!(target: LOG_TARGET, path = %path.display(), %error, "a session's file wasn't written");
                    // It goes again with the next batch, unless something newer waits for it.
                    if let Some(content) = content {
                        self.waiting.entry(path).or_insert(content);
                    }
                }
            }
        }
        self.stats.bytes += batch;
        self.stats.batches += 1;
        self.stats.largest_batch = self.stats.largest_batch.max(batch);
    }
}

/// Writes `batch`, `session.ron` last and only if everything else was written, each making its directory
/// first. How each went.
fn write_batch(disk: &dyn Disk, batch: Vec<(PathBuf, Grown, Content)>) -> Vec<Outcome> {
    let (documents, files): (Vec<_>, Vec<_>) =
        batch.into_iter().partition(|(_, _, content)| matches!(content, Content::Document(_)));
    let mut outcomes: Vec<Outcome> =
        files.into_iter().map(|(path, grown, content)| write_one(disk, path, grown, content)).collect();
    let whole = outcomes.iter().all(|outcome| outcome.result.is_ok());
    for (path, grown, content) in documents {
        outcomes.push(if whole {
            write_one(disk, path, grown, content)
        } else {
            Outcome {
                path,
                result: Err("not written, since a file it names wasn't".to_owned()),
                grown: None,
                content: Some(content),
            }
        });
    }
    outcomes
}

fn write_one(disk: &dyn Disk, path: PathBuf, grown: Grown, content: Content) -> Outcome {
    let written = (|| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        }
        let failed = |error: io::Error| error.to_string();
        match &content {
            Content::Text(text) => {
                disk.replace(&path, text.as_bytes()).map_err(failed)?;
                Ok((text.len() as u64, None))
            }
            Content::Document(document) => {
                let text = sapphire_serialize::to_ron(&**document).map_err(|error| error.to_string())?;
                disk.replace(&path, text.as_bytes()).map_err(failed)?;
                Ok((text.len() as u64, None))
            }
            Content::Run(run) => {
                let RunKept { replay, trail, ended } = &**run;
                let from = match grown {
                    Grown::Run(written) => written,
                    _ => Written::default(),
                };
                let (bytes, now) = journal::record(from, replay, trail, *ended)
                    .ok_or("the run is behind what its journal holds")?;
                disk.append(&path, from.bytes, &bytes).map_err(failed)?;
                Ok((bytes.len() as u64, Some(Grown::Run(now))))
            }
            Content::Pieces(pieces) => {
                let (bytes, from) = match grown {
                    Grown::Pieces { bytes, pieces } => (bytes, pieces),
                    _ => (0, 0),
                };
                let added: String = pieces.get(from..).unwrap_or_default().concat();
                disk.append(&path, bytes, added.as_bytes()).map_err(failed)?;
                let grown = Grown::Pieces { bytes: bytes + added.len() as u64, pieces: pieces.len() };
                Ok((added.len() as u64, Some(grown)))
            }
        }
    })();
    match written {
        Ok((bytes, grown)) => Outcome { path, result: Ok(bytes), grown, content: None },
        Err(error) => Outcome { path, result: Err(error), grown: None, content: Some(content) },
    }
}
