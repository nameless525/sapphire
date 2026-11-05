//! Packs coming and going: [`AssetServer::mount`], which waits on nothing, and [`AssetServer::unmount`].
//! [`AssetServer`]'s docs ("Mounting", "Unmounting") say what an app sees; this file holds the server's
//! half.

use std::io;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::handle::SlotKey;
use super::{
    AssetServer, Completion, LOG_TARGET, LoadError, LoadPriority, MountError, PackFile, PackSource, SlotState,
};
use crate::pack::{HEADER_LEN, PackError, PackHeader, Toc};

slotmap::new_key_type! {
    /// A mounted pack. A slot's mount outlasts the pack only while the slot has no entry, which no read
    /// follows; the key's generation means the next pack mounted can't be taken for it.
    pub(super) struct MountKey;
}

/// A mounted pack.
pub(super) struct Mount {
    pub name: String,
    pub file: OpenPack,
    pub toc: Arc<Toc>,
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
    let (sender, receiver) = mpsc::sync_channel(1);
    let done = Box::new(move |result| {
        // The receiver waits below, so the send can't fail.
        let _ = sender.send(result);
    });
    source.read(file, offset, len, LoadPriority::Critical, done);
    receiver.recv().unwrap_or_else(|_| Err(io::Error::other("the pack source dropped a read")))
}

/// Opens the pack called `name` and reads and checks its header and table of contents, waiting for them
/// on the calling thread.
pub(super) fn read_pack(source: &Arc<dyn PackSource>, name: &str) -> Result<(OpenPack, Toc), MountError> {
    let io_error = |source| MountError::Io { name: name.to_owned(), source };
    let pack_error = |source| MountError::Pack { name: name.to_owned(), source };
    let (file, len) = source.open(name).map_err(io_error)?;
    let open = OpenPack { source: source.clone(), file };
    let header = read_now(&**source, file, 0, len.min(HEADER_LEN as u64) as usize).map_err(io_error)?;
    let header = PackHeader::read(&header).map_err(pack_error)?;
    // Reject an inconsistent extent before asking the source to allocate the TOC buffer.
    header.check_file_len(len).map_err(pack_error)?;
    let toc_range = header.toc_range();
    let toc_len = usize::try_from(toc_range.end - toc_range.start)
        .map_err(|_| pack_error(PackError::Header("the table of contents is too large for this platform")))?;
    let toc = read_now(&**source, file, toc_range.start, toc_len).map_err(io_error)?;
    let toc = Toc::read(header, &toc, len).map_err(pack_error)?;
    Ok((open, toc))
}

impl AssetServer {
    /// Starts mounting the pack called `name` ([`crate::pack::file_name`] in the source) and returns at
    /// once, with nothing read: a worker opens it and reads and checks its header and table of contents.
    /// Its assets can load from the [`Self::update`] that finds that done, which
    /// [`Self::mount_state`] shows, and until then they're in no mounted pack. An asset may be in one
    /// mounted pack only. Loaded assets that lost their pack and that this one holds move to it, and
    /// failed assets that need what it holds load again (see [hot reload](AssetServer#hot-reload)).
    ///
    /// Fails here only if the pack is mounted or mounting already. Anything else that stops it shows in
    /// [`Self::mount_state`].
    pub fn mount(&mut self, name: &str) -> Result<(), MountError> {
        profiling::scope!("AssetServer::mount");
        if self.is_mounted(name) || self.pending_mounts.iter().any(|pending| pending.name == name) {
            return Err(MountError::AlreadyMounted(name.to_owned()));
        }
        self.failed_mounts.retain(|(failed, _)| failed != name);
        self.mount_requests += 1;
        let request = self.mount_requests;
        self.pending_mounts.push(PendingMount { name: name.to_owned(), request });
        self.reads_in_flight += 1;
        let (source, sender, name) = (self.source.clone(), self.sender.clone(), name.to_owned());
        sapphire_jobs::spawn_async(move || {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| read_pack(&source, &name)))
                .unwrap_or_else(|_| {
                    Err(MountError::Io { name, source: io::Error::other("reading the pack panicked") })
                });
            // A closed channel means the server is gone: what was opened closes with the message.
            let _ = sender.send(Completion::Mount { request, result });
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
    pub(super) fn complete_mount(&mut self, request: u64, result: Result<(OpenPack, Toc), MountError>) {
        let Some(pending) = self.pending_mounts.iter().position(|pending| pending.request == request) else {
            return;
        };
        let PendingMount { name, .. } = self.pending_mounts.remove(pending);
        let attached = result.and_then(|(file, toc)| self.attach(&name, file, toc));
        if let Err(error) = attached {
            tracing::warn!(target: LOG_TARGET, pack = name, %error, "pack didn't mount");
            self.failed_mounts.push((name, error));
        }
    }

    /// Makes pack `name`'s assets loadable, if no other mounted pack holds them.
    fn attach(&mut self, name: &str, file: OpenPack, toc: Toc) -> Result<(), MountError> {
        self.check_duplicates(name, &toc, None)?;
        let toc = Arc::new(toc);
        let mount = self.mounts.insert(Mount { name: name.to_owned(), file, toc: toc.clone() });
        self.index
            .extend(toc.entries().iter().enumerate().map(|(index, entry)| (entry.id, (mount, index as u32))));
        let reloading = self.settle_mount(mount, &toc);
        tracing::info!(
            target: LOG_TARGET,
            pack = name,
            assets = toc.entries().len(),
            bytes = toc.header().file_len(),
            reloading,
            "pack mounted"
        );
        Ok(())
    }

    /// Fails if pack `name`'s `toc` holds an asset another mounted pack holds, besides mount
    /// `except` (the one a remount replaces).
    pub(super) fn check_duplicates(
        &self,
        name: &str,
        toc: &Toc,
        except: Option<MountKey>,
    ) -> Result<(), MountError> {
        for entry in toc.entries() {
            if let Some(&(other, _)) = self.index.get(&entry.id)
                && Some(other) != except
            {
                let first = self.mounts[other].name.clone();
                return Err(MountError::Duplicate { id: entry.id, first, second: name.to_owned() });
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
