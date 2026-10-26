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

use std::io;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sapphire_assets::pack::{Block, HEADER_LEN, PackError, PackHeader, Toc};
use sapphire_assets::{AssetServer, LoadPriority, MountError, PackFile, PackSource, ReadDone};

use crate::protocol::{ByteRange, PackOutcome};

/// The most bytes one push may ask for: a pack's changed assets, or a whole pack the device lacks.
pub const MAX_PUSH: u64 = 1 << 30;

/// The most received bytes a read copies on the thread that asks: headers, tables of contents
/// and small assets. Larger ones (a pushed texture) are copied on a worker.
const COPY_IN_PLACE: usize = 256 << 10;

/// Packs pushed over live link, served in place of the app's own (see the [module docs](self)).
///
/// The app's asset server reads through it (it is a [`PackSource`]); the live link's connections
/// read the versions it serves, to find what a push needs to send. Packs nobody pushed are read
/// from the app's source as they are. A pushed pack is served from then on, to every later open,
/// until another push replaces it or the app restarts: a desktop app whose pack file changes
/// after a push keeps serving the pushed version.
pub struct LivePacks {
    base: Arc<dyn PackSource>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Pushed packs by name, each the version later opens get.
    pushed: Vec<(String, Arc<Version>)>,
    /// Files open through this source, by [`PackFile`] number.
    files: Vec<Option<Open>>,
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
    /// Serves the packs of `base`, the app's own source, until packs are pushed.
    pub fn new(base: impl PackSource + 'static) -> Self {
        Self { base: Arc::new(base), state: Mutex::default() }
    }

    /// The packs pushed so far, by name, with their lengths.
    pub fn pushed(&self) -> Vec<(String, u64)> {
        self.lock().pushed.iter().map(|(name, version)| (name.clone(), version.len)).collect()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
        let received = needed.iter().map(|_| Vec::new()).collect();
        Ok(PackPlan { name: name.to_owned(), len, toc, pieces, needed, received, filling: 0 })
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
        Ok(PushedPack { name: self.name, version: Arc::new(version), received })
    }
}

/// A pack pushed whole, for the app to [apply](Self::apply).
pub struct PushedPack {
    name: String,
    version: Arc<Version>,
    received: u64,
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
