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
}

impl<S: PackSource + ?Sized> PackSource for Arc<S> {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        (**self).open(name)
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        (**self).read(file, offset, len, priority, done);
    }
}

/// Packs held in memory, read on the calling thread: tests, tools, and packs a live-link session
/// pushes to a device.
#[derive(Debug, Default)]
pub struct MemoryPackSource {
    packs: Mutex<Vec<(String, Arc<[u8]>)>>,
}

impl MemoryPackSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the pack called `name`, or replaces it for later opens: files opened before keep
    /// reading the bytes they opened.
    pub fn insert(&self, name: &str, bytes: impl Into<Arc<[u8]>>) {
        self.packs.lock().unwrap_or_else(PoisonError::into_inner).push((name.to_owned(), bytes.into()));
    }

    /// The bytes behind `file`, if it was opened here.
    fn bytes(&self, file: PackFile) -> Option<Arc<[u8]>> {
        let packs = self.packs.lock().unwrap_or_else(PoisonError::into_inner);
        packs.get(usize::try_from(file.0).ok()?).map(|(_, bytes)| bytes.clone())
    }
}

impl PackSource for MemoryPackSource {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let packs = self.packs.lock().unwrap_or_else(PoisonError::into_inner);
        packs
            .iter()
            .rposition(|(existing, _)| existing == name)
            .map(|index| (PackFile(index as u64), packs[index].1.len() as u64))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no pack called {name:?}")))
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, _priority: LoadPriority, done: ReadDone) {
        let result = self
            .bytes(file)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such pack file"))
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
}
