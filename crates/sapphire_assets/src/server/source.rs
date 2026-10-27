//! Where an asset server's pack bytes come from ([`PackSource`]), and packs held in memory
//! ([`MemoryPackSource`]).

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

/// An open pack file, as its [`PackSource`] numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PackFile(pub u64);

/// How urgently a read is wanted: the IO thread serves queued reads in this order, oldest first
/// within a priority (spec §5.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LoadPriority {
    /// Blocks the frame: what the player is about to see.
    Critical,
    High,
    #[default]
    Normal,
    /// Speculative: may wait behind everything else.
    Prefetch,
}

/// Called with a read's bytes, or its error, on whatever thread did the reading.
pub type ReadDone = Box<dyn FnOnce(io::Result<Vec<u8>>) + Send>;

/// Reads pack files for an [`AssetServer`](super::AssetServer).
///
/// Apps implement it over the platform's IO thread (`sapphire_platform::io`), which opens packs
/// from the content root and serves positioned reads by priority; this crate stays free of the
/// platform layer, so tools reach it without SDL (ADR 0001). [`MemoryPackSource`] serves packs held
/// in memory.
pub trait PackSource: Send + Sync {
    /// Opens the pack called `name`, stored as [`crate::pack::file_name`], and returns it with its
    /// length in bytes.
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)>;

    /// Reads `len` bytes of `file` from `offset`, then calls `done` with them, on whatever thread
    /// the read finished on (possibly this one, before returning). `done` is always called.
    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone);

    /// Closes `file`, which the server reads no more: a remounted pack's earlier file (hot
    /// reload), or every pack it has mounted when the server is dropped, so a source that
    /// outlives its server holds nothing open for it. Reads of it still queued may fail; their
    /// results are no longer wanted. Sources that hold nothing open per file keep the default,
    /// which does nothing.
    fn close(&self, file: PackFile) {
        let _ = file;
    }
}

impl<S: PackSource + ?Sized> PackSource for Arc<S> {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        (**self).open(name)
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        (**self).read(file, offset, len, priority, done);
    }

    fn close(&self, file: PackFile) {
        (**self).close(file);
    }
}

/// Packs held in memory, read on the calling thread: tests and tools.
///
/// Replacing a pack ([`Self::insert`]) and remounting it ([`AssetServer::remount`]) reloads what
/// changed, as a rewritten pack file does on desktop. Live link (M1 A9) takes the same path with
/// a source of its own (`sapphire_livelink::LivePacks`), which serves a pushed pack over the app's
/// files, so a push holds and sends only the assets that changed.
///
/// [`AssetServer::remount`]: super::AssetServer::remount
#[derive(Debug, Default)]
pub struct MemoryPackSource {
    state: Mutex<MemoryPacks>,
}

#[derive(Debug, Default)]
struct MemoryPacks {
    /// The packs by name, each the latest inserted under it.
    packs: Vec<(String, Arc<[u8]>)>,
    /// Every file opened, by [`PackFile`] number: the bytes it opened, until it is closed.
    files: Vec<Option<Arc<[u8]>>>,
}

impl MemoryPackSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the pack called `name`, or replaces it for later opens: files opened before keep
    /// reading the bytes they opened, until they are closed.
    pub fn insert(&self, name: &str, bytes: impl Into<Arc<[u8]>>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes = bytes.into();
        match state.packs.iter_mut().find(|(existing, _)| existing == name) {
            Some((_, pack)) => *pack = bytes,
            None => state.packs.push((name.to_owned(), bytes)),
        }
    }

    /// The bytes behind `file`, if it was opened here and is still open.
    fn bytes(&self, file: PackFile) -> Option<Arc<[u8]>> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.files.get(usize::try_from(file.0).ok()?)?.clone()
    }
}

impl PackSource for MemoryPackSource {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes = state
            .packs
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, bytes)| bytes.clone())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no pack called {name:?}")))?;
        let len = bytes.len() as u64;
        // Closed files' numbers are given out again, as file descriptors are.
        let number = match state.files.iter().position(Option::is_none) {
            Some(free) => {
                state.files[free] = Some(bytes);
                free
            }
            None => {
                state.files.push(Some(bytes));
                state.files.len() - 1
            }
        };
        Ok((PackFile(number as u64), len))
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, _priority: LoadPriority, done: ReadDone) {
        let result = self
            .bytes(file)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such open pack file"))
            .and_then(|bytes| {
                let start = usize::try_from(offset).ok().filter(|&start| start <= bytes.len());
                start
                    .and_then(|start| bytes.get(start..start.checked_add(len)?))
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end of the pack")
                    })
            });
        done(result);
    }

    fn close(&self, file: PackFile) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = usize::try_from(file.0).ok().and_then(|index| state.files.get_mut(index)) {
            *open = None;
        }
    }
}
