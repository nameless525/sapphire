//! [`LivePacks`]: the pack source a development build's asset server reads through, serving packs
//! pushed over live link in place of the app's own, and [`PushedPack`], a push received whole.
//!
//! # Pushing only what changed
//!
//! A push sends the new pack's header and table of contents first. Every asset whose bytes and
//! blocks the device's current version of the pack already holds (by the blake3 hashes the table
//! lists) is taken from there; only the rest of the new pack's bytes cross the link. The device
//! then serves the new pack as pieces: its head and the received ranges from memory, the other
//! assets from wherever the version before kept them, which for a pack never pushed is the app's
//! own file, opened through its source and kept open while a version reads it. The alignment
//! padding between assets, which nothing reads, is zeros, as the packer writes it.
//! So the asset server sees a whole pack, byte for byte the tool's, and remounts it as it does a
//! pack rewritten on desktop (`AssetServer::remount`): it reloads exactly the assets whose hashes
//! changed, behind their handles. Each block's hash is checked before it is decoded, as always,
//! so a piece taken from the wrong place is a failed reload, never a wrong asset.
//!
//! # What pushed packs may hold
//!
//! A pushed pack is served until another push replaces it or the app restarts, mounted or not:
//! packs the app hasn't mounted are kept for whatever mounts them later. So what they hold is
//! bounded across pushes, by [`PushLimits`]: the pack names served (16 by default) and the
//! received bytes held in memory (1 GiB by default), counting every version served, every file
//! the asset server has open on one, and every push being received or waiting to be applied, each
//! piece of memory once however many versions share it. A push past either is refused when it
//! begins, saying which limit and why, before any of its bytes are sent; nothing pushed is
//! dropped to make room. Restarting the app frees what pushes hold.

use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sapphire_assets::pack::{Block, HEADER_LEN, PackError, PackHeader, Toc};
use sapphire_assets::{AssetServer, LoadPriority, MountError, PackFile, PackSource, ReadDone};

use crate::protocol::{ByteRange, PackOutcome};

/// The most bytes one push may ask for: a pack's changed assets, or a whole pack the device lacks.
pub const MAX_PUSH: u64 = 1 << 30;

/// What pushed packs may hold on a device at once (see the [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PushLimits {
    /// Pack names served from pushes. A push of another name is refused once this many are
    /// served or being pushed; the names served can always be pushed again.
    pub packs: usize,
    /// Received bytes held in memory, by the versions served (and those the asset server still
    /// reads) and by the pushes being received or waiting to be applied. A push that would hold
    /// more is refused.
    pub resident: u64,
}

impl PushLimits {
    /// 16 packs and 1 GiB, what one push may ask for at most ([`MAX_PUSH`]): room for many
    /// edits of a project's packs, within what a phone's development build can spare beside the
    /// app.
    pub const DEFAULT: Self = Self { packs: 16, resident: 1 << 30 };
}

impl Default for PushLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The most received bytes a read copies on the thread that asks: headers, tables of contents
/// and small assets. Larger ones (a pushed texture) are copied on a worker.
const COPY_IN_PLACE: usize = 256 << 10;

/// Packs pushed over live link, served in place of the app's own (see the [module docs](self)).
///
/// The app's asset server reads through it (it is a [`PackSource`]); the live link's connections
/// read the versions it serves, to find what a push needs to send. Packs nobody pushed are read
/// from the app's source as they are. A pushed pack is served from then on, to every later open,
/// until another push replaces it or the app restarts: a desktop app whose pack file changes
/// after a push keeps serving the pushed version. What pushes may hold is bounded by its
/// [`PushLimits`].
pub struct LivePacks {
    base: Arc<dyn PackSource>,
    limits: PushLimits,
    /// Shared with the pushes being received, which give back what they reserved when dropped.
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    /// Pushed packs by name, each the version later opens get.
    pushed: Vec<(String, Arc<Version>)>,
    /// Files open through this source, by [`PackFile`] number.
    files: Vec<Option<Open>>,
    /// Bytes reserved by pushes being received or waiting to be applied.
    reserved: u64,
    /// The names, not served yet, that those pushes are for: one entry a push.
    reserved_names: Vec<String>,
}

impl State {
    /// Received bytes held in memory by the versions served and those open, each piece of
    /// memory once however many versions share it.
    fn held(&self) -> u64 {
        let open = self.files.iter().filter_map(|open| match open {
            Some(Open::Pushed(version)) => Some(version),
            _ => None,
        });
        let mut memory: Vec<(*const u8, u64)> = self
            .pushed
            .iter()
            .map(|(_, version)| version)
            .chain(open)
            .flat_map(|version| &version.pieces)
            .filter_map(|piece| match &piece.backing {
                Backing::Memory { bytes, .. } => Some((bytes.as_ptr(), bytes.len() as u64)),
                Backing::Base { .. } | Backing::Zeros => None,
            })
            .collect();
        memory.sort_unstable();
        memory.dedup_by_key(|(pointer, _)| *pointer);
        memory.iter().map(|(_, len)| len).sum()
    }

    /// Pack names served or being pushed, each once.
    fn names(&self) -> usize {
        let mut names: Vec<&str> = self
            .pushed
            .iter()
            .map(|(name, _)| name.as_str())
            .chain(self.reserved_names.iter().map(String::as_str))
            .collect();
        names.sort_unstable();
        names.dedup();
        names.len()
    }
}

#[derive(Clone)]
enum Open {
    Base(PackFile),
    Pushed(Arc<Version>),
}

impl std::fmt::Debug for LivePacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        let pushed: Vec<(&str, u64)> =
            state.pushed.iter().map(|(name, version)| (name.as_str(), version.len)).collect();
        f.debug_struct("LivePacks").field("pushed", &pushed).finish_non_exhaustive()
    }
}

impl LivePacks {
    /// Serves the packs of `base`, the app's own source, until packs are pushed, within
    /// [`PushLimits::DEFAULT`].
    pub fn new(base: impl PackSource + 'static) -> Self {
        Self::with_limits(base, PushLimits::DEFAULT)
    }

    /// Serves the packs of `base` until packs are pushed, which may hold at most `limits`.
    pub fn with_limits(base: impl PackSource + 'static, limits: PushLimits) -> Self {
        Self { base: Arc::new(base), limits, state: Arc::default() }
    }

    pub fn limits(&self) -> PushLimits {
        self.limits
    }

    /// The packs pushed so far, by name, with their lengths.
    pub fn pushed(&self) -> Vec<(String, u64)> {
        self.lock().pushed.iter().map(|(name, version)| (name.clone(), version.len)).collect()
    }

    /// Received bytes the pushed packs hold in memory now: those of the versions served, and of
    /// older versions the asset server still has open, each piece once. Pushes being received
    /// are not counted, though their reservations count against [`PushLimits::resident`].
    pub fn held(&self) -> u64 {
        self.lock().held()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Reserves `bytes` and, if it isn't served yet, a place for pack `name` for a push, or says
    /// which limit it would pass.
    fn reserve(&self, name: &str, bytes: u64) -> Result<Reservation, PushError> {
        let mut state = self.lock();
        let new = !state.pushed.iter().any(|(pushed, _)| pushed == name);
        let named = new && !state.reserved_names.iter().any(|reserved| reserved == name);
        if named && state.names() >= self.limits.packs {
            return Err(PushError::TooManyPacks(self.limits.packs));
        }
        let held = state.held() + state.reserved;
        if held + bytes > self.limits.resident {
            return Err(PushError::OverBudget { needs: bytes, held, budget: self.limits.resident });
        }
        state.reserved += bytes;
        let name = new.then(|| name.to_owned());
        state.reserved_names.extend(name.clone());
        Ok(Reservation { state: self.state.clone(), bytes, name })
    }

    /// Plans a push of pack `name`, `len` bytes long, whose header and table of contents are
    /// `head`: checks them, and finds the ranges of the new pack the version served now doesn't
    /// hold. Reads that version's table of contents from the app's source if the pack was never
    /// pushed, blocking until it is read.
    pub(crate) fn plan(&self, name: &str, len: u64, head: Vec<u8>) -> Result<PackPlan, PushError> {
        let header = PackHeader::read(&head)?;
        if head.len() as u64 != header.data_offset {
            return Err(PushError::Head { expected: header.data_offset, found: head.len() as u64 });
        }
        let toc = Toc::read(header, &head[HEADER_LEN..], len)?;
        let current = self.current(name);
        let head: Arc<[u8]> = head.into();
        let mut pieces = vec![Piece { start: 0, len: header.data_offset, backing: Backing::memory(&head) }];
        let mut needed = Vec::new();
        // Blocks lie in entry order, so stored ranges come in order of offset. Between two
        // assets lies alignment padding, zeros as the packer writes it, which nothing reads.
        let mut cursor = header.data_offset;
        for entry in toc.entries() {
            let range = toc.stored_range(entry);
            if cursor < range.start {
                pieces.push(Piece { start: cursor, len: range.start - cursor, backing: Backing::Zeros });
            }
            match current.as_ref().and_then(|current| current.holds(&toc, entry)) {
                Some(old) => pieces.extend(old.into_iter().scan(range.start, |start, (len, backing)| {
                    let piece = Piece { start: *start, len, backing };
                    *start += len;
                    Some(piece)
                })),
                // Each asset the device lacks is a range of its own, kept in a buffer of its own
                // once received, so the push that drops it later frees it.
                None => needed.push(ByteRange { offset: range.start, len: range.end - range.start }),
            }
            cursor = range.end;
        }
        // `Toc::read` checked that the data ends with the last block, and the pack with the data.
        debug_assert_eq!(cursor, len, "a pack's assets end where it does");
        let total: u64 = needed.iter().map(|range| range.len).sum();
        if total > MAX_PUSH {
            return Err(PushError::TooLarge(total));
        }
        // The new version's memory: its head and what it asks for. What it takes from the
        // version before is held already.
        let reservation = self.reserve(name, head.len() as u64 + total)?;
        let received = needed.iter().map(|_| Vec::new()).collect();
        Ok(PackPlan { name: name.to_owned(), len, toc, pieces, needed, received, filling: 0, reservation })
    }

    /// The version of pack `name` served now: the pushed one, or the app's own file, opened to
    /// read its table of contents. `None` if the app has no such pack, or it can't be read (the
    /// push then sends everything).
    fn current(&self, name: &str) -> Option<Current> {
        if let Some((_, version)) = self.lock().pushed.iter().find(|(pushed, _)| pushed == name) {
            return Some(Current::Pushed(version.clone()));
        }
        let (file, len) = self.base.open(name).ok()?;
        let file = Arc::new(BaseFile { source: self.base.clone(), file });
        let read = |offset, len| read_blocking(&*self.base, file.file, offset, len);
        let header = PackHeader::read(&read(0, HEADER_LEN as u64).ok()?).ok()?;
        let toc_range = header.toc_range();
        let toc = read(toc_range.start, toc_range.end - toc_range.start).ok()?;
        let toc = Toc::read(header, &toc, len).ok()?;
        Some(Current::Base { file, toc })
    }

    /// Serves `version` as pack `name` from now on, and returns the version it replaces.
    fn install(&self, name: &str, version: Arc<Version>) -> Option<Arc<Version>> {
        let mut state = self.lock();
        match state.pushed.iter_mut().find(|(pushed, _)| pushed == name) {
            Some((_, current)) => Some(std::mem::replace(current, version)),
            None => {
                state.pushed.push((name.to_owned(), version));
                None
            }
        }
    }

    /// Serves `previous` as pack `name` again, or the app's own file if it was never pushed.
    fn restore(&self, name: &str, previous: Option<Arc<Version>>) {
        let mut state = self.lock();
        let index = state.pushed.iter().position(|(pushed, _)| pushed == name);
        match (index, previous) {
            (Some(index), Some(previous)) => state.pushed[index].1 = previous,
            (Some(index), None) => drop(state.pushed.remove(index)),
            (None, Some(previous)) => state.pushed.push((name.to_owned(), previous)),
            (None, None) => {}
        }
    }
}

impl PackSource for LivePacks {
    fn open(&self, name: &str) -> io::Result<(PackFile, u64)> {
        let pushed =
            self.lock().pushed.iter().find(|(pushed, _)| pushed == name).map(|(_, version)| version.clone());
        // The app's source opens outside the lock: it may wait on the file system.
        let (open, len) = match pushed {
            Some(version) => {
                let len = version.len;
                (Open::Pushed(version), len)
            }
            None => {
                let (file, len) = self.base.open(name)?;
                (Open::Base(file), len)
            }
        };
        let mut state = self.lock();
        // Closed files' numbers are given out again, as file descriptors are.
        let number = match state.files.iter().position(Option::is_none) {
            Some(free) => {
                state.files[free] = Some(open);
                free
            }
            None => {
                state.files.push(Some(open));
                state.files.len() - 1
            }
        };
        Ok((PackFile(number as u64), len))
    }

    fn read(&self, file: PackFile, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        let open =
            usize::try_from(file.0).ok().and_then(|index| self.lock().files.get(index).cloned().flatten());
        match open {
            Some(Open::Base(file)) => self.base.read(file, offset, len, priority, done),
            Some(Open::Pushed(version)) => version.read(offset, len, priority, done),
            None => done(Err(io::Error::new(io::ErrorKind::NotFound, "no such open pack file"))),
        }
    }

    fn close(&self, file: PackFile) {
        let open = usize::try_from(file.0).ok().and_then(|index| self.lock().files.get_mut(index)?.take());
        if let Some(Open::Base(file)) = open {
            self.base.close(file);
        }
    }
}

/// What a push can't use.
#[derive(Debug, thiserror::Error)]
pub enum PushError {
    #[error("the pushed pack is invalid: {0}")]
    Pack(#[from] PackError),
    #[error("a push starts with the pack's header and table of contents, {expected} bytes, not {found}")]
    Head { expected: u64, found: u64 },
    #[error("the push needs {0} bytes, more than the {MAX_PUSH} a push may send")]
    TooLarge(u64),
    #[error("a chunk at {found} where the bytes at {expected} come next")]
    OutOfOrder { expected: u64, found: u64 },
    #[error("a chunk at {0} that the device didn't ask for")]
    Unrequested(u64),
    #[error("the push ended with {0} bytes still to send")]
    Incomplete(u64),
    #[error(
        "the device serves {0} pushed packs already, the most it keeps: push one of those again, or restart the \
         app to push others"
    )]
    TooManyPacks(usize),
    #[error(
        "the push needs {} beside the {} pushed packs hold on the device, past the {} they may: restart the app \
         to free them",
        Mib(*needs),
        Mib(*held),
        Mib(*budget)
    )]
    OverBudget { needs: u64, held: u64, budget: u64 },
}

/// Bytes, in MiB, for messages.
struct Mib(u64);

impl std::fmt::Display for Mib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.1} MiB", self.0 as f64 / f64::from(1 << 20))
    }
}

/// A push's claim on its [`PushLimits`] while it is received and waits to be applied: its bytes,
/// and the name it is for if no version of that is served yet. Dropped, it gives them back.
struct Reservation {
    state: Arc<Mutex<State>>,
    bytes: u64,
    name: Option<String>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = lock(&self.state);
        state.reserved -= self.bytes;
        if let Some(name) = &self.name
            && let Some(index) = state.reserved_names.iter().position(|reserved| reserved == name)
        {
            state.reserved_names.swap_remove(index);
        }
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A version of a pushed pack, as the device serves it: pieces covering the whole pack, each from
/// memory or from the app's own file.
pub(crate) struct Version {
    len: u64,
    toc: Toc,
    /// In order of `start`, each starting where the one before ends, from 0 to `len`.
    pieces: Vec<Piece>,
}

struct Piece {
    start: u64,
    len: u64,
    backing: Backing,
}

#[derive(Clone)]
enum Backing {
    /// Bytes received, from `offset` in `bytes`.
    Memory { bytes: Arc<[u8]>, offset: usize },
    /// The app's own pack file, from `offset`.
    Base { file: Arc<BaseFile>, offset: u64 },
    /// Padding between assets.
    Zeros,
}

impl Backing {
    fn memory(bytes: &Arc<[u8]>) -> Self {
        Self::Memory { bytes: bytes.clone(), offset: 0 }
    }

    /// The same bytes, `by` further on.
    fn advanced(&self, by: u64) -> Self {
        match self {
            Self::Memory { bytes, offset } => {
                Self::Memory { bytes: bytes.clone(), offset: offset + by as usize }
            }
            Self::Base { file, offset } => Self::Base { file: file.clone(), offset: offset + by },
            Self::Zeros => Self::Zeros,
        }
    }
}

/// A file of the app's own source that versions read from, closed once the last of them goes.
struct BaseFile {
    source: Arc<dyn PackSource>,
    file: PackFile,
}

impl Drop for BaseFile {
    fn drop(&mut self) {
        self.source.close(self.file);
    }
}

impl Version {
    /// The pieces holding `len` bytes from `start`, each as its length and backing.
    fn slice(&self, start: u64, len: u64) -> Vec<(u64, Backing)> {
        let end = start + len;
        let first = self.pieces.partition_point(|piece| piece.start + piece.len <= start);
        self.pieces[first..]
            .iter()
            .take_while(|piece| piece.start < end)
            .map(|piece| {
                let from = start.max(piece.start);
                let to = end.min(piece.start + piece.len);
                (to - from, piece.backing.advanced(from - piece.start))
            })
            .collect()
    }

    /// Reads `len` bytes from `offset`, from each piece's backing: the app's file through its
    /// source, by `priority`, and received bytes copied, on a worker if there are many of them,
    /// since the asset server reads on the app's thread.
    fn read(self: &Arc<Self>, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        let Some(end) = offset.checked_add(len as u64).filter(|&end| end <= self.len) else {
            return done(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end of the pack")));
        };
        let pieces = self.slice(offset, end - offset);
        // An asset of the app's file lies in one piece: its read goes straight there.
        if let [(_, Backing::Base { file, offset })] = pieces.as_slice() {
            return file.source.read(file.file, *offset, len, priority, done);
        }
        if len > COPY_IN_PLACE {
            let version = self.clone();
            return sapphire_jobs::spawn_async(move || version.gather(offset, len, priority, done));
        }
        self.gather(offset, len, priority, done);
    }

    /// [`Self::read`] of several pieces, or of received bytes, on this thread.
    fn gather(&self, offset: u64, len: usize, priority: LoadPriority, done: ReadDone) {
        let pieces = self.slice(offset, len as u64);
        let mut buffer = vec![0; len];
        let mut reads = Vec::new();
        let mut at = 0;
        for (len, backing) in pieces {
            let len = len as usize;
            match backing {
                Backing::Memory { bytes, offset } => {
                    buffer[at..at + len].copy_from_slice(&bytes[offset..offset + len])
                }
                Backing::Base { file, offset } => reads.push((file, offset, at, len)),
                Backing::Zeros => {}
            }
            at += len;
        }
        if reads.is_empty() {
            return done(Ok(buffer));
        }
        let gather =
            Arc::new(Mutex::new(Gather { buffer, pending: reads.len(), error: None, done: Some(done) }));
        for (file, offset, at, len) in reads {
            let gather = gather.clone();
            file.source.read(
                file.file,
                offset,
                len,
                priority,
                Box::new(move |result| Gather::complete(&gather, at, len, result)),
            );
        }
    }
}

/// A read across several pieces of the app's file, finished when the last of their reads is.
struct Gather {
    buffer: Vec<u8>,
    pending: usize,
    error: Option<io::Error>,
    done: Option<ReadDone>,
}

impl Gather {
    fn complete(gather: &Mutex<Self>, at: usize, len: usize, result: io::Result<Vec<u8>>) {
        let mut state = gather.lock().unwrap_or_else(PoisonError::into_inner);
        match result {
            Ok(bytes) if bytes.len() == len => state.buffer[at..at + len].copy_from_slice(&bytes),
            Ok(_) => {
                let short = io::Error::new(io::ErrorKind::UnexpectedEof, "a short read of the app's pack");
                state.error.get_or_insert(short);
            }
            Err(error) => {
                state.error.get_or_insert(error);
            }
        }
        state.pending -= 1;
        if state.pending > 0 {
            return;
        }
        let done = state.done.take().expect("the last read finishes once");
        let result = match state.error.take() {
            Some(error) => Err(error),
            None => Ok(std::mem::take(&mut state.buffer)),
        };
        drop(state);
        done(result);
    }
}

/// The version of a pack served before a push.
enum Current {
    Pushed(Arc<Version>),
    Base { file: Arc<BaseFile>, toc: Toc },
}

impl Current {
    fn toc(&self) -> &Toc {
        match self {
            Self::Pushed(version) => &version.toc,
            Self::Base { toc, .. } => toc,
        }
    }

    /// Where this version holds the stored blocks the new pack's `entry` lists, as pieces, if it
    /// holds the same asset stored the same way.
    fn holds(&self, new: &Toc, entry: &sapphire_assets::pack::Entry) -> Option<Vec<(u64, Backing)>> {
        let toc = self.toc();
        let old = toc.find(entry.id)?;
        if (old.kind, old.size, old.hash) != (entry.kind, entry.size, entry.hash) {
            return None;
        }
        let (blocks, old_blocks) = (new.blocks(entry), toc.blocks(old));
        if !same_layout(blocks, old_blocks) {
            return None;
        }
        let range = toc.stored_range(old);
        Some(match self {
            Self::Pushed(version) => version.slice(range.start, range.end - range.start),
            Self::Base { file, .. } => {
                vec![(range.end - range.start, Backing::Base { file: file.clone(), offset: range.start })]
            }
        })
    }
}

/// Whether two runs of blocks hold the same stored bytes, laid out the same way from their first.
fn same_layout(new: &[Block], old: &[Block]) -> bool {
    let (Some(new_first), Some(old_first)) = (new.first(), old.first()) else { return false };
    new.len() == old.len()
        && new.iter().zip(old).all(|(new, old)| {
            (new.stored_len, new.len, new.codec, new.hash) == (old.stored_len, old.len, old.codec, old.hash)
                && new.offset - new_first.offset == old.offset - old_first.offset
        })
}

/// Reads `len` bytes of `file` from `offset`, waiting for them.
fn read_blocking(source: &dyn PackSource, file: PackFile, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let len = usize::try_from(len).map_err(io::Error::other)?;
    source.read(
        file,
        offset,
        len,
        LoadPriority::High,
        Box::new(move |result| {
            // The receiver waits below, so the send can't fail.
            let _ = sender.send(result);
        }),
    );
    receiver.recv().unwrap_or_else(|_| Err(io::Error::other("the pack source dropped a read")))
}

/// A push being received: the new pack's pieces the device has, and the ranges it asked for,
/// filled in order as chunks come.
pub(crate) struct PackPlan {
    name: String,
    len: u64,
    toc: Toc,
    /// The head, and the assets the version before holds.
    pieces: Vec<Piece>,
    needed: Vec<ByteRange>,
    /// Each needed range's bytes so far.
    received: Vec<Vec<u8>>,
    /// The range chunks fill now.
    filling: usize,
    /// What the push may hold, until it is applied or dropped.
    reservation: Reservation,
}

impl PackPlan {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The ranges the push must send, in order.
    pub fn needed(&self) -> &[ByteRange] {
        &self.needed
    }

    /// Takes the next bytes the push sends, which must be the next of the needed ranges', at
    /// `offset`.
    pub fn receive(&mut self, offset: u64, bytes: Vec<u8>) -> Result<(), PushError> {
        let Some(range) = self.needed.get(self.filling) else { return Err(PushError::Unrequested(offset)) };
        let buffer = &mut self.received[self.filling];
        let expected = range.offset + buffer.len() as u64;
        if offset != expected {
            return Err(PushError::OutOfOrder { expected, found: offset });
        }
        if bytes.len() as u64 > range.end() - expected {
            return Err(PushError::Unrequested(range.end()));
        }
        // Buffers grow as bytes come, so a table of contents claiming a huge asset reserves
        // nothing up front.
        if buffer.is_empty() {
            *buffer = bytes;
        } else {
            buffer.extend_from_slice(&bytes);
        }
        if buffer.len() as u64 == range.len {
            self.filling += 1;
        }
        Ok(())
    }

    /// The pushed pack, once every needed byte came.
    pub fn finish(self) -> Result<PushedPack, PushError> {
        if let Some(range) = self.needed.get(self.filling) {
            let sent = self.received[self.filling].len() as u64;
            let rest: u64 = self.needed[self.filling + 1..].iter().map(|range| range.len).sum();
            return Err(PushError::Incomplete(range.len - sent + rest));
        }
        let mut pieces = self.pieces;
        let mut received = 0;
        for (range, bytes) in self.needed.iter().zip(self.received) {
            received += range.len;
            let bytes: Arc<[u8]> = bytes.into();
            pieces.push(Piece { start: range.offset, len: range.len, backing: Backing::memory(&bytes) });
        }
        pieces.sort_unstable_by_key(|piece| piece.start);
        debug_assert!(
            pieces.windows(2).all(|pair| pair[0].start + pair[0].len == pair[1].start)
                && pieces.last().is_some_and(|piece| piece.start + piece.len == self.len),
            "a plan's pieces cover its pack"
        );
        let version = Version { len: self.len, toc: self.toc, pieces };
        Ok(PushedPack {
            name: self.name,
            version: Arc::new(version),
            received,
            _reservation: self.reservation,
        })
    }
}

/// A pack pushed whole, for the app to [apply](Self::apply).
pub struct PushedPack {
    name: String,
    version: Arc<Version>,
    received: u64,
    /// Given back once applied, when the version it holds is served (or refused), or if the
    /// push is dropped.
    _reservation: Reservation,
}

impl std::fmt::Debug for PushedPack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushedPack")
            .field("name", &self.name)
            .field("len", &self.version.len)
            .field("received", &self.received)
            .finish()
    }
}

impl PushedPack {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pack's length in bytes.
    pub fn size(&self) -> u64 {
        self.version.len
    }

    /// Bytes the push sent.
    pub fn received(&self) -> u64 {
        self.received
    }

    /// Serves the pack from `packs` from now on and, if `server` has it mounted, remounts it:
    /// the assets whose bytes changed load their new versions behind their handles, as A8's hot
    /// reload does (`AssetServer::remount`). A pack the server refuses (one holding another
    /// mounted pack's asset, say) is served no more: `packs` goes back to the version before, as
    /// the server keeps it.
    pub fn apply(self, packs: &LivePacks, server: &mut AssetServer) -> Result<PackOutcome, MountError> {
        let previous = packs.install(&self.name, self.version.clone());
        let mut outcome =
            PackOutcome { received: self.received, len: self.version.len, ..PackOutcome::default() };
        if server.is_mounted(&self.name) {
            match server.remount(&self.name) {
                Ok(report) => {
                    outcome.mounted = true;
                    (outcome.changed, outcome.removed, outcome.reloading) =
                        (report.changed, report.removed, report.reloading);
                }
                Err(error) => {
                    packs.restore(&self.name, previous);
                    return Err(error);
                }
            }
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::pack::{Codec, PackAsset, PackReader, write_pack};
    use sapphire_assets::{AssetId, AssetKind, MemoryPackSource};

    use super::*;

    fn id(n: u8) -> AssetId {
        AssetId::from_bytes([n; 16])
    }

    /// A rig-kind asset of `len` bytes from `seed`: rigs are checked only when decoded, so plain
    /// bytes pack as one. Blocks of 64 KiB, so larger ones span several.
    fn asset(n: u8, len: usize, seed: u8) -> PackAsset {
        let bytes: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
        PackAsset::encode(id(n), AssetKind::Rig, Codec::Lz4, &bytes, Vec::new(), 3).unwrap()
    }

    /// A rig-kind asset of `len` bytes of noise, which stays its size in a pack.
    fn noise(n: u8, len: usize) -> PackAsset {
        let mut state = 0x9e37_79b9_u32;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                // xorshift32.
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        PackAsset::encode(id(n), AssetKind::Rig, Codec::Lz4, &bytes, Vec::new(), 3).unwrap()
    }

    fn pack(assets: Vec<PackAsset>) -> Vec<u8> {
        write_pack(assets).unwrap()
    }

    fn head(bytes: &[u8]) -> Vec<u8> {
        let header = PackHeader::read(bytes).unwrap();
        bytes[..header.data_offset as usize].to_vec()
    }

    /// Plans `bytes` as pack `name` against what `packs` serves, sends what it asks for in
    /// chunks of `chunk` bytes, and applies nothing: the pushed pack.
    fn push(packs: &LivePacks, name: &str, bytes: &[u8], chunk: usize) -> (Vec<ByteRange>, PushedPack) {
        let mut plan = packs.plan(name, bytes.len() as u64, head(bytes)).unwrap();
        let needed = plan.needed().to_vec();
        for range in &needed {
            let mut offset = range.offset;
            while offset < range.end() {
                let len = chunk.min((range.end() - offset) as usize);
                plan.receive(offset, bytes[offset as usize..offset as usize + len].to_vec()).unwrap();
                offset += len as u64;
            }
        }
        (needed, plan.finish().unwrap())
    }

    /// Reads all of pack `name` through `packs`, as one read.
    fn read_all(packs: &LivePacks, name: &str) -> Vec<u8> {
        let (file, len) = packs.open(name).unwrap();
        let bytes = read_blocking(packs, file, 0, len).unwrap();
        packs.close(file);
        bytes
    }

    #[test]
    fn a_push_sends_only_the_assets_that_changed() {
        let base = Arc::new(MemoryPackSource::new());
        let v1 = pack(vec![asset(1, 70_000, 1), asset(2, 3_000, 2), asset(3, 140_000, 3)]);
        base.insert("shared", v1.clone());
        let packs = LivePacks::new(base.clone());

        // Asset 2 changes, asset 4 is new.
        let v2 = pack(vec![asset(1, 70_000, 1), asset(2, 3_100, 9), asset(3, 140_000, 3), asset(4, 500, 4)]);
        let (needed, pushed) = push(&packs, "shared", &v2, 1000);
        let toc = PackReader::new(&v2).unwrap().toc().clone();
        let range = |n| toc.stored_range(toc.find(id(n)).unwrap());
        let sent: u64 = needed.iter().map(|range| range.len).sum();
        assert_eq!(pushed.received(), sent);
        assert!(needed.iter().all(|needed| {
            [1, 3].iter().all(|&n| needed.end() <= range(n).start || needed.offset >= range(n).end)
        }));
        assert!(needed.iter().any(|needed| needed.offset <= range(2).start && needed.end() >= range(2).end));
        assert!(needed.iter().any(|needed| needed.offset <= range(4).start && needed.end() >= range(4).end));
        let changed = range(2).end - range(2).start + range(4).end - range(4).start;
        assert!(sent < changed + 32, "the unchanged 210 KB stay on the device: sent {sent}");

        // Served whole, byte for byte, from memory and the app's file.
        packs.install("shared", pushed.version.clone());
        assert_eq!(read_all(&packs, "shared"), v2);
        // Every asset decodes from its pieces, read as the server reads them.
        for n in 1..=4 {
            let (file, _) = packs.open("shared").unwrap();
            let entry = toc.find(id(n)).unwrap();
            let range = range(n);
            let stored = read_blocking(&packs, file, range.start, range.end - range.start).unwrap();
            toc.decode_asset(entry, &stored, &mut Default::default()).unwrap();
            packs.close(file);
        }
        // Packs nobody pushed are the app's.
        base.insert("other", v1.clone());
        assert_eq!(read_all(&packs, "other"), v1);
    }

    #[test]
    fn pushes_build_on_pushes_and_drop_what_is_gone() {
        let base = Arc::new(MemoryPackSource::new());
        base.insert("shared", pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 2), asset(3, 9_000, 3)]));
        let packs = LivePacks::new(base.clone());
        let v2 = pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 7), asset(3, 9_000, 3)]);
        let (_, pushed) = push(&packs, "shared", &v2, 4096);
        packs.install("shared", pushed.version);

        // The third version keeps 2's pushed bytes, drops 3 and adds 5: only 5 is sent, and what
        // it keeps comes from memory and the app's file alike.
        let v3 = pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 7), asset(5, 2_000, 5)]);
        let (needed, pushed) = push(&packs, "shared", &v3, 100_000);
        let toc = PackReader::new(&v3).unwrap().toc().clone();
        let new = toc.stored_range(toc.find(id(5)).unwrap());
        assert_eq!(pushed.received(), needed.iter().map(|range| range.len).sum::<u64>());
        let sent = pushed.received();
        assert!(sent < new.end - new.start + 16, "only asset 5 and the padding before it: {needed:?}");
        packs.install("shared", pushed.version);
        assert_eq!(read_all(&packs, "shared"), v3);

        // An identical push sends nothing.
        let (needed, pushed) = push(&packs, "shared", &v3, 100_000);
        assert!(needed.is_empty() && pushed.received() == 0);
    }

    #[test]
    fn a_pack_the_app_lacks_is_sent_whole() {
        let packs = LivePacks::new(MemoryPackSource::new());
        // Larger than a read copies in place: reading it whole copies on a worker.
        let bytes = pack(vec![noise(1, 400_000)]);
        assert!(bytes.len() > COPY_IN_PLACE);
        let (needed, pushed) = push(&packs, "zone", &bytes, 1024);
        let data = PackHeader::read(&bytes).unwrap().data_offset;
        assert_eq!(needed, [ByteRange { offset: data, len: bytes.len() as u64 - data }]);
        packs.install("zone", pushed.version);
        assert_eq!(read_all(&packs, "zone"), bytes);
        assert_eq!(packs.pushed(), [("zone".to_owned(), bytes.len() as u64)]);
    }

    #[test]
    fn chunks_must_come_in_order_and_complete_the_ranges() {
        let packs = LivePacks::new(MemoryPackSource::new());
        let bytes = pack(vec![asset(1, 5_000, 1)]);
        let plan = || packs.plan("zone", bytes.len() as u64, head(&bytes)).unwrap();
        let range = plan().needed()[0];
        let at = |offset: u64, len: usize| bytes[offset as usize..offset as usize + len].to_vec();

        let mut skipped = plan();
        let error = skipped.receive(range.offset + 10, at(range.offset + 10, 10)).unwrap_err();
        assert!(
            matches!(error, PushError::OutOfOrder { expected, found } if expected == range.offset && found == range.offset + 10)
        );

        let mut overlong = plan();
        let error = overlong.receive(range.offset, vec![0; range.len as usize + 1]).unwrap_err();
        assert!(matches!(error, PushError::Unrequested(_)), "{error}");

        let mut short = plan();
        short.receive(range.offset, at(range.offset, 100)).unwrap();
        let error = short.finish().unwrap_err();
        assert!(matches!(error, PushError::Incomplete(missing) if missing == range.len - 100), "{error}");

        let mut extra = plan();
        extra.receive(range.offset, at(range.offset, range.len as usize)).unwrap();
        assert!(matches!(extra.receive(range.end(), vec![1]), Err(PushError::Unrequested(_))));
    }

    #[test]
    fn invalid_heads_are_refused() {
        let packs = LivePacks::new(MemoryPackSource::new());
        let bytes = pack(vec![asset(1, 5_000, 1)]);
        let len = bytes.len() as u64;
        let mut corrupt = head(&bytes);
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(matches!(packs.plan("zone", len, corrupt), Err(PushError::Pack(PackError::TocHash))));
        assert!(matches!(packs.plan("zone", len, bytes[..200].to_vec()), Err(PushError::Pack(_))));
        assert!(matches!(packs.plan("zone", len, bytes[..300].to_vec()), Err(PushError::Head { .. })));
        assert!(
            matches!(packs.plan("zone", len + 1, head(&bytes)), Err(PushError::Pack(_))),
            "the wrong length"
        );
    }

    /// Plans, sends and serves `bytes` as pack `name` (as applying it does, but for the remount).
    fn push_and_serve(packs: &LivePacks, name: &str, bytes: &[u8]) -> Result<(), PushError> {
        let mut plan = packs.plan(name, bytes.len() as u64, head(bytes))?;
        for range in plan.needed().to_vec() {
            plan.receive(range.offset, bytes[range.offset as usize..range.end() as usize].to_vec())?;
        }
        let pushed = plan.finish()?;
        packs.install(&pushed.name, pushed.version.clone());
        Ok(())
    }

    /// The bytes a version of a pack holds in memory when the device lacks all of it: its head
    /// and its data.
    fn whole(bytes: &[u8]) -> u64 {
        bytes.len() as u64
    }

    #[test]
    fn pushes_of_new_names_stop_at_the_limit() {
        // A tool naming each push afresh, none of which the app mounts.
        let packs = LivePacks::new(MemoryPackSource::new());
        let bytes = pack(vec![asset(1, 5_000, 1)]);
        let results: Vec<_> = (0..40).map(|n| push_and_serve(&packs, &format!("zone{n}"), &bytes)).collect();
        assert!(results[..16].iter().all(Result::is_ok));
        for refused in &results[16..] {
            let error = refused.as_ref().unwrap_err();
            assert!(matches!(error, PushError::TooManyPacks(16)), "{error}");
            assert!(error.to_string().contains("restart the app"), "{error}");
        }
        assert_eq!(packs.pushed().len(), 16, "nothing pushed was dropped to make room");
        assert!(
            packs
                .pushed()
                .iter()
                .all(|(name, _)| name.starts_with("zone") && name[4..].parse::<u32>().unwrap() < 16)
        );
        // A name served can be pushed again.
        push_and_serve(&packs, "zone3", &pack(vec![asset(1, 5_000, 2)])).unwrap();
        assert_eq!(packs.pushed().len(), 16);
    }

    #[test]
    fn pushes_being_received_hold_their_names() {
        let limits = PushLimits { packs: 2, ..PushLimits::DEFAULT };
        let packs = LivePacks::with_limits(MemoryPackSource::new(), limits);
        let bytes = pack(vec![asset(1, 5_000, 1)]);
        push_and_serve(&packs, "a", &bytes).unwrap();
        // A push of "b" begun holds the last place, and another of "b" shares it.
        let plan = packs.plan("b", bytes.len() as u64, head(&bytes)).unwrap();
        let again = packs.plan("b", bytes.len() as u64, head(&bytes)).unwrap();
        assert!(matches!(packs.plan("c", bytes.len() as u64, head(&bytes)), Err(PushError::TooManyPacks(2))));
        // Dropped, the pushes give their place back.
        drop((plan, again));
        push_and_serve(&packs, "c", &bytes).unwrap();
        assert!(matches!(push_and_serve(&packs, "b", &bytes), Err(PushError::TooManyPacks(2))));
        // As does a pushed pack received whole but dropped before it was applied (a link closed
        // with it queued).
        let packs = LivePacks::with_limits(MemoryPackSource::new(), limits);
        let mut plan = packs.plan("a", bytes.len() as u64, head(&bytes)).unwrap();
        let range = plan.needed()[0];
        plan.receive(range.offset, bytes[range.offset as usize..range.end() as usize].to_vec()).unwrap();
        let queued = plan.finish().unwrap();
        push_and_serve(&packs, "b", &bytes).unwrap();
        assert!(matches!(push_and_serve(&packs, "c", &bytes), Err(PushError::TooManyPacks(2))));
        drop(queued);
        push_and_serve(&packs, "c", &bytes).unwrap();
    }

    #[test]
    fn pushes_stop_at_the_memory_budget() {
        // Two whole packs' worth, not three.
        let bytes = |seed| pack(vec![noise(seed, 40_000)]);
        let limits = PushLimits { resident: 2 * whole(&bytes(1)) + 1_000, ..PushLimits::DEFAULT };
        let packs = LivePacks::with_limits(MemoryPackSource::new(), limits);
        push_and_serve(&packs, "a", &bytes(1)).unwrap();
        push_and_serve(&packs, "b", &bytes(2)).unwrap();
        assert_eq!(packs.held(), whole(&bytes(1)) + whole(&bytes(2)));
        let error = push_and_serve(&packs, "c", &bytes(3)).unwrap_err();
        assert!(
            matches!(error, PushError::OverBudget { needs, held, budget } if needs == whole(&bytes(3)) && held == packs.held() && budget == limits.resident),
            "{error}"
        );
        assert!(error.to_string().contains("restart the app"), "{error}");
        // Replacing a pack is refused too while both versions would be held at once.
        assert!(matches!(push_and_serve(&packs, "a", &bytes(4)), Err(PushError::OverBudget { .. })));
        assert_eq!(packs.pushed().len(), 2, "what is served stays");
        // Served again whole, the same bytes are held once: pushing it again sends nothing.
        push_and_serve(&packs, "a", &bytes(1)).unwrap();
        assert_eq!(packs.held(), whole(&bytes(1)) + whole(&bytes(2)), "only the new head, the old one freed");
    }

    #[test]
    fn versions_sharing_received_bytes_count_them_once() {
        let base = Arc::new(MemoryPackSource::new());
        base.insert("shared", pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 2), asset(3, 9_000, 3)]));
        let packs = LivePacks::new(base);
        // Asset 2 changes: its bytes and the head are held, assets 1 and 3 read from the app's file.
        let v2 = pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 7), asset(3, 9_000, 3)]);
        let (needed, pushed) = push(&packs, "shared", &v2, 4096);
        let changed: u64 = needed.iter().map(|range| range.len).sum();
        let head2 = head(&v2).len() as u64;
        packs.install("shared", pushed.version);
        assert_eq!(packs.held(), head2 + changed);
        // The next version keeps 2's received bytes and adds 5: 2's are counted once, though
        // both versions hold them while a file reads the older.
        let (file, _) = packs.open("shared").unwrap();
        let v3 = pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 7), asset(5, 2_000, 5)]);
        let (needed, pushed) = push(&packs, "shared", &v3, 100_000);
        let added: u64 = needed.iter().map(|range| range.len).sum();
        let head3 = head(&v3).len() as u64;
        packs.install("shared", pushed.version);
        assert_eq!(packs.held(), head2 + head3 + changed + added, "the old head, open, too");
        packs.close(file);
        assert_eq!(packs.held(), head3 + changed + added);
        // Going back to the app's own version frees them all.
        packs.restore("shared", None);
        assert_eq!(packs.held(), 0);
    }

    #[test]
    fn reads_across_pieces_of_the_apps_file_gather() {
        // Two unchanged assets around a changed one: a read of the whole data spans file, memory
        // and file pieces.
        let base = Arc::new(MemoryPackSource::new());
        base.insert("shared", pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 2), asset(3, 9_000, 3)]));
        let packs = LivePacks::new(base);
        let v2 = pack(vec![asset(1, 9_000, 1), asset(2, 9_000, 8), asset(3, 9_000, 3)]);
        let (_, pushed) = push(&packs, "shared", &v2, 512);
        assert!(pushed.version.pieces.len() >= 4, "head, 1, 2 and 3");
        packs.install("shared", pushed.version);
        let (file, len) = packs.open("shared").unwrap();
        let data = PackHeader::read(&v2).unwrap().data_offset;
        assert_eq!(read_blocking(&packs, file, data, len - data).unwrap(), v2[data as usize..]);
        let error = read_blocking(&packs, file, len - 4, 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        packs.close(file);
        let error = read_blocking(&packs, file, 0, 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "a closed file");
    }
}
