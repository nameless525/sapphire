//! Packs coming and going: [`AssetServer::mount`], which waits on nothing, and [`AssetServer::unmount`].
//! [`AssetServer`]'s docs ("Mounting", "Unmounting") say what an app sees; this file holds the server's
//! half.

use std::collections::HashSet;
use std::io;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use sapphire_core::frame_loads::{self, LoadWork};

use super::handle::SlotKey;
use super::{
    AssetServer, Completion, LOG_TARGET, LoadError, LoadPriority, MountError, PackFile, PackSource, SlotState,
};
use crate::AssetId;
use crate::manifest::ReleasedPack;
use crate::pack::{Entry, HEADER_LEN, PackError, PackHeader, PackMetadataRead, Toc, part_name};

slotmap::new_key_type! {
    /// A mounted pack. A slot's mount outlasts the pack only while the slot has no entry, which no read
    /// follows; the key's generation means the next pack mounted can't be taken for it.
    pub(super) struct MountKey;
}

/// A mounted pack.
pub(super) struct Mount {
    pub name: String,
    /// Each part's file, in part order.
    pub files: Vec<OpenPack>,
    pub parts: Arc<Parts>,
}

/// A pack's parts' tables of contents, read as one (see "Parts" in [`crate::pack`]): an asset's place in the
/// pack, which [`AssetServer`]'s index and slots hold, runs on through the parts in order.
pub(super) struct Parts {
    tocs: Vec<Arc<Toc>>,
    /// Where each part's assets start among the pack's.
    starts: Vec<u32>,
}

impl Parts {
    /// The parts whose tables are `tocs`, in part order.
    pub fn new(tocs: Vec<Toc>) -> Self {
        let mut starts = Vec::with_capacity(tocs.len());
        let mut next = 0u32;
        for toc in &tocs {
            starts.push(next);
            // Each part holds fewer than 2^32 entries, and a pack at most 4,096 parts of 128 MiB or so.
            next = next.saturating_add(toc.entries().len() as u32);
        }
        Self { tocs: tocs.into_iter().map(Arc::new).collect(), starts }
    }

    /// Each part's table, in part order.
    pub fn tocs(&self) -> &[Arc<Toc>] {
        &self.tocs
    }

    /// The part that holds the pack's asset `index`, and the asset's entry there.
    pub fn locate(&self, index: u32) -> (usize, &Entry) {
        let part = self.starts.partition_point(|&start| start <= index) - 1;
        (part, &self.tocs[part].entries()[(index - self.starts[part]) as usize])
    }

    /// The pack's asset `index`.
    pub fn entry(&self, index: u32) -> &Entry {
        self.locate(index).1
    }

    /// What the pack's asset `index` depends on.
    pub fn dependencies(&self, index: u32) -> &[AssetId] {
        let (part, entry) = self.locate(index);
        self.tocs[part].dependencies(entry)
    }

    /// Where asset `id` is in the pack, if it holds it.
    pub fn position(&self, id: AssetId) -> Option<u32> {
        self.tocs.iter().zip(&self.starts).find_map(|(toc, &start)| {
            let index = toc.entries().binary_search_by(|entry| entry.id.cmp(&id)).ok()?;
            Some(start + index as u32)
        })
    }

    /// Asset `id`'s entry, if the pack holds it.
    pub fn find(&self, id: AssetId) -> Option<&Entry> {
        self.position(id).map(|index| self.entry(index))
    }

    /// Every asset, part by part, each part's in ID order: the pack's asset `index` is the `index`th.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.tocs.iter().flat_map(|toc| toc.entries())
    }

    /// Whether `other` is the same pack, laid out the same: its parts' headers hold their tables' hashes.
    pub fn same_as(&self, other: &Self) -> bool {
        self.tocs.len() == other.tocs.len()
            && self.tocs.iter().zip(&other.tocs).all(|(a, b)| a.header() == b.header())
    }

    /// The bytes of every part's file.
    pub fn file_len(&self) -> u64 {
        self.tocs.iter().map(|toc| toc.header().file_len()).sum()
    }
}

/// A pack a worker is opening.
pub(super) struct PendingMount {
    name: String,
    request: u64,
}

/// Where a pack is, from [`AssetServer::mount_state`].
#[derive(Debug)]
pub enum MountState<'a> {
    /// Never asked for, or unmounted.
    Unmounted,
    /// Its header and table of contents are being read. Its assets can't load yet.
    Mounting,
    /// Its assets can load.
    Mounted,
    /// It couldn't be mounted. Mounting it again tries again.
    Failed(&'a MountError),
}

/// What one [`AssetServer::unmount`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnmountReport {
    /// Assets of the pack unloaded by the call, none using them.
    pub unloaded: u32,
    /// Assets of the pack kept for what's holding them, until that lets go.
    pub held: u32,
    /// Assets of the pack that were still loading, and failed.
    pub cancelled: u32,
}

/// A pack file open in a source, closed when this goes, whichever way it goes: a mount's file, or a
/// mount that was cancelled or failed.
pub(super) struct OpenPack {
    source: Arc<dyn PackSource>,
    file: PackFile,
}

impl OpenPack {
    pub fn file(&self) -> PackFile {
        self.file
    }
}

impl Drop for OpenPack {
    fn drop(&mut self) {
        self.source.close(self.file);
    }
}

/// Reads a range of `file` and waits for it.
fn read_now(source: &dyn PackSource, file: PackFile, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    // The calling thread waits for it, wherever it's read.
    frame_loads::count(LoadWork::Read);
    let (sender, receiver) = mpsc::sync_channel(1);
    let done = Box::new(move |result| {
        // The receiver waits below, so the send can't fail.
        let _ = sender.send(result);
    });
    source.read(file, offset, len, LoadPriority::Critical, done);
    receiver.recv().unwrap_or_else(|_| Err(io::Error::other("the pack source dropped a read")))
}

/// Opens the pack called `name` and reads and checks each of its parts' header and table of contents, waiting
/// for them on the calling thread: its first part, which says how many there are, then the rest. Each must say
/// it is the part its name says, of as many as the first.
pub(super) fn read_pack(
    source: &Arc<dyn PackSource>,
    name: &str,
    release: Option<&[ReleasedPack]>,
    metadata: &mut PackMetadataRead,
) -> Result<(Vec<OpenPack>, Parts), MountError> {
    let (first_file, first) = read_part(source, name, release, metadata)?;
    let parts = first.header().parts;
    let check = |part_name: &str, toc: &Toc, expected: u32| {
        let header = toc.header();
        if header.part == expected && header.parts == parts {
            Ok(())
        } else {
            Err(MountError::Pack {
                name: part_name.to_owned(),
                source: PackError::Part {
                    part: header.part,
                    parts: header.parts,
                    expected,
                    expected_parts: parts,
                },
            })
        }
    };
    check(name, &first, 0)?;
    let cut = first.header().cut;
    let (mut files, mut tocs) = (vec![first_file], vec![first]);
    for part in 1..parts {
        let part_name = part_name(name, part);
        let (file, toc) = read_part(source, &part_name, release, metadata)?;
        check(&part_name, &toc, part)?;
        if toc.header().cut != cut {
            return Err(MountError::Pack { name: part_name, source: PackError::OtherCut { part } });
        }
        files.push(file);
        tocs.push(toc);
    }
    Ok((files, Parts::new(tocs)))
}

/// Opens the file called `name`, a pack's part, and reads and checks its header and table of contents. With a
/// `release`, the part must be one it names, with the length and header it names, before its table of contents
/// is read.
fn read_part(
    source: &Arc<dyn PackSource>,
    name: &str,
    release: Option<&[ReleasedPack]>,
    metadata: &mut PackMetadataRead,
) -> Result<(OpenPack, Toc), MountError> {
    let io_error = |source| MountError::Io { name: name.to_owned(), source };
    let pack_error = |source| MountError::Pack { name: name.to_owned(), source };
    let not_as_released = |reason| MountError::NotAsReleased { name: name.to_owned(), reason };
    let released = match release {
        Some(packs) => Some(
            packs
                .binary_search_by(|pack| pack.name.as_str().cmp(name))
                .map(|index| &packs[index])
                .map_err(|_| MountError::NotReleased(name.to_owned()))?,
        ),
        None => None,
    };
    let (file, len) = source.open(name).map_err(io_error)?;
    let open = OpenPack { source: source.clone(), file };
    if released.is_some_and(|released| released.size != len) {
        return Err(not_as_released("its length isn't the release's"));
    }
    let header = read_now(&**source, file, 0, len.min(HEADER_LEN as u64) as usize).map_err(io_error)?;
    let header = PackHeader::read(&header).map_err(pack_error)?;
    if released.is_some_and(|released| released.header_hash != header.hash()) {
        return Err(not_as_released("its header isn't the release's"));
    }
    let toc_len = metadata.toc_read_len(&header, len).map_err(pack_error)?;
    let toc = read_now(&**source, file, HEADER_LEN as u64, toc_len).map_err(io_error)?;
    let toc = Toc::read(header, &toc, len).map_err(pack_error)?;
    Ok((open, toc))
}

impl AssetServer {
    /// Starts mounting the pack called `name` ([`crate::pack::file_name`] in the source, and its other parts'
    /// files, [`crate::pack::part_name`]) and returns at once, with nothing read: a worker opens each part and
    /// reads and checks its header and table of contents.
    /// Its assets can load from the [`Self::update`] that finds that done, which
    /// [`Self::mount_state`] shows, and until then they're in no mounted pack. An asset may be in one
    /// mounted pack only. Loaded assets that lost their pack and that this one holds move to it, and
    /// failed assets that need what it holds load again (see [hot reload](AssetServer#hot-reload)).
    ///
    /// Fails here if the pack is mounted or mounting already, or the metadata budget cannot admit
    /// its header. A refusal leaves the request unqueued. Later failures show in [`Self::mount_state`].
    pub fn mount(&mut self, name: &str) -> Result<(), MountError> {
        profiling::scope!("AssetServer::mount");
        if self.is_mounted(name) || self.pending_mounts.iter().any(|pending| pending.name == name) {
            return Err(MountError::AlreadyMounted(name.to_owned()));
        }
        let mut metadata = self
            .metadata_budget
            .begin()
            .map_err(|source| MountError::Pack { name: name.to_owned(), source })?;
        self.failed_mounts.retain(|(failed, _)| failed != name);
        self.mount_requests += 1;
        let request = self.mount_requests;
        self.pending_mounts.push(PendingMount { name: name.to_owned(), request });
        self.reads_in_flight += 1;
        let (source, sender, name) = (self.source.clone(), self.sender.clone(), name.to_owned());
        let release = self.release.clone();
        sapphire_jobs::spawn_async(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                read_pack(&source, &name, release.as_deref(), &mut metadata)
            }))
            .unwrap_or_else(|_| {
                Err(MountError::Io { name, source: io::Error::other("reading the pack panicked") })
            });
            // A closed channel means the server is gone: what was opened closes with the message.
            let _ = sender.send(Completion::Mount { request, result, metadata });
        });
        Ok(())
    }

    /// Mounts the pack called `name` as [`Self::mount`] does, and waits for it: a load screen's call, and
    /// how tools mount. What it fails with is what [`Self::mount_state`] would have said.
    pub fn mount_now(&mut self, name: &str) -> Result<(), MountError> {
        self.mount(name)?;
        while matches!(self.mount_state(name), MountState::Mounting) {
            self.wait(Duration::from_millis(100));
        }
        match self.failed_mounts.iter().position(|(failed, _)| failed == name) {
            Some(failed) => Err(self.failed_mounts.swap_remove(failed).1),
            None => Ok(()),
        }
    }

    /// Where the pack called `name` is.
    pub fn mount_state(&self, name: &str) -> MountState<'_> {
        if self.is_mounted(name) {
            MountState::Mounted
        } else if self.pending_mounts.iter().any(|pending| pending.name == name) {
            MountState::Mounting
        } else if let Some((_, error)) = self.failed_mounts.iter().find(|(failed, _)| failed == name) {
            MountState::Failed(error)
        } else {
            MountState::Unmounted
        }
    }

    /// Whether the pack called `name` is mounted.
    pub fn is_mounted(&self, name: &str) -> bool {
        self.mounts.values().any(|mount| mount.name == name)
    }

    /// A worker finished opening a pack: it joins the mounted packs, unless it's been unmounted since.
    pub(super) fn complete_mount(
        &mut self,
        request: u64,
        result: Result<(Vec<OpenPack>, Parts), MountError>,
    ) {
        let Some(pending) = self.pending_mounts.iter().position(|pending| pending.request == request) else {
            return;
        };
        let PendingMount { name, .. } = self.pending_mounts.remove(pending);
        let attached = result.and_then(|(files, parts)| self.attach(&name, files, parts));
        if let Err(error) = attached {
            tracing::warn!(target: LOG_TARGET, pack = name, %error, "pack didn't mount");
            self.failed_mounts.push((name, error));
        }
    }

    /// Makes pack `name`'s assets loadable, if no other mounted pack holds them.
    fn attach(&mut self, name: &str, files: Vec<OpenPack>, parts: Parts) -> Result<(), MountError> {
        self.check_duplicates(name, &parts, None)?;
        let parts = Arc::new(parts);
        let mount = self.mounts.insert(Mount { name: name.to_owned(), files, parts: parts.clone() });
        self.index
            .extend(parts.entries().enumerate().map(|(index, entry)| (entry.id, (mount, index as u32))));
        let reloading = self.settle_mount(mount, &parts);
        tracing::info!(
            target: LOG_TARGET,
            pack = name,
            parts = parts.tocs().len(),
            assets = parts.entries().count(),
            bytes = parts.file_len(),
            reloading,
            "pack mounted"
        );
        Ok(())
    }

    /// Fails if pack `name`'s `parts` hold an asset twice, or one another mounted pack holds, besides mount
    /// `except` (the one a remount replaces).
    pub(super) fn check_duplicates(
        &self,
        name: &str,
        parts: &Parts,
        except: Option<MountKey>,
    ) -> Result<(), MountError> {
        let duplicate =
            |id, first: &str| MountError::Duplicate { id, first: first.to_owned(), second: name.to_owned() };
        for entry in parts.entries() {
            if let Some(&(other, _)) = self.index.get(&entry.id)
                && Some(other) != except
            {
                return Err(duplicate(entry.id, &self.mounts[other].name));
            }
        }
        // Each part's entries are distinct and sorted; another part may still hold one of them.
        if parts.tocs().len() > 1 {
            let mut seen = HashSet::with_capacity(parts.entries().count());
            if let Some(entry) = parts.entries().find(|entry| !seen.insert(entry.id)) {
                return Err(duplicate(entry.id, name));
            }
        }
        Ok(())
    }

    /// Unmounts the pack called `name` and closes its file: see [unmounting](AssetServer#unmounting) for
    /// what becomes of its assets and of the handles to them. A pack still mounting is cancelled, and one
    /// that failed to is forgotten. Fails only if neither that nor mounted describes it.
    pub fn unmount(&mut self, name: &str) -> Result<UnmountReport, MountError> {
        profiling::scope!("AssetServer::unmount");
        if let Some(pending) = self.pending_mounts.iter().position(|pending| pending.name == name) {
            // Its job finds no one waiting, and what it opened closes with its message.
            self.pending_mounts.remove(pending);
            return Ok(UnmountReport::default());
        }
        if let Some(failed) = self.failed_mounts.iter().position(|(failed, _)| failed == name) {
            self.failed_mounts.swap_remove(failed);
            return Ok(UnmountReport::default());
        }
        let key = self
            .mounts
            .iter()
            .find(|(_, mount)| mount.name == name)
            .map(|(key, _)| key)
            .ok_or_else(|| MountError::NotMounted(name.to_owned()))?;
        // Its file closes at the end of this call. Reads queued on it may fail, and every one of them
        // is about to be for a version nothing waits for.
        let mount = self.mounts.remove(key).expect("found above");
        self.index.retain(|_, &mut (other, _)| other != key);

        let held: Vec<SlotKey> = self
            .slots
            .iter_mut()
            .filter(|(_, slot)| slot.mount == key && slot.entry.is_some())
            .map(|(slot_key, slot)| {
                slot.entry = None;
                slot_key
            })
            .collect();
        let mut report = UnmountReport::default();
        for &slot_key in &held {
            self.cancel_next(slot_key);
            let slot = &self.slots[slot_key];
            if matches!(slot.state, SlotState::Loading | SlotState::Waiting | SlotState::Queued) {
                report.cancelled += 1;
                let id = slot.handle.id();
                self.fail(slot_key, LoadError::Removed(id));
            }
        }
        // New versions waiting on one that was dropped go on with the version it has.
        for &slot_key in &held {
            if self.slots.get(slot_key).is_some_and(|slot| slot.next.is_none()) {
                for parent in self.parents(slot_key) {
                    self.try_queue_next(parent);
                }
            }
        }
        // Over and over, as removing a mesh can leave its material with no users.
        loop {
            let unused: Vec<SlotKey> = held
                .iter()
                .copied()
                .filter(|&slot_key| self.slots.get(slot_key).is_some_and(Self::evictable))
                .collect();
            if unused.is_empty() {
                break;
            }
            report.unloaded += unused.len() as u32;
            for slot_key in unused {
                self.remove(slot_key);
            }
        }
        report.held = held.iter().filter(|&&slot_key| self.slots.contains_key(slot_key)).count() as u32;
        tracing::info!(
            target: LOG_TARGET,
            pack = name,
            unloaded = report.unloaded,
            held = report.held,
            cancelled = report.cancelled,
            "pack unmounted"
        );
        drop(mount);
        Ok(report)
    }
}
