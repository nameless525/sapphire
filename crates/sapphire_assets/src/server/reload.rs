//! Hot reload (M1 A8): remounting a changed pack and swapping new versions in behind the same
//! handles. [`AssetServer`]'s docs ("Hot reload") describe what an app sees; this file holds the
//! server's half.

use std::collections::HashSet;
use std::sync::Arc;

use sapphire_serialize::cooked::AlignedVec;

use super::handle::{SlotKey, UntypedHandle};
use super::{
    AssetServer, AssetSlot, Handle, LOG_TARGET, LoadError, MountError, MountKey, Slot, SlotState, Version,
    error_chain, mounts,
};
use crate::pack::{Entry, Toc};
use crate::{Asset, AssetId, AssetKind};

/// A change hot reload made to a loaded asset, from [`AssetServer::take_changes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetChange {
    /// A new version replaced the asset behind its handles, as `generation`
    /// ([`AssetServer::generation`]). An asset that goes to the GPU reached the app's uploader
    /// as this generation first.
    Reloaded { id: AssetId, kind: AssetKind, slot: AssetSlot, generation: u32 },
    /// Its new version didn't load: the asset keeps the version it has.
    ReloadFailed { id: AssetId, kind: AssetKind, error: LoadError },
    /// Its pack no longer holds it, or holds an asset of another kind under its ID. It keeps the
    /// version it has until nothing uses it, but can't be read again until a pack holds it again:
    /// a new renderer gets it only if its kind keeps its bytes.
    Removed { id: AssetId, kind: AssetKind },
}

/// What one [`AssetServer::remount`] found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RemountReport {
    /// Assets the pack holds with other bytes than before, or didn't hold before.
    pub changed: u32,
    /// Assets the pack held before and doesn't now.
    pub removed: u32,
    /// Loaded assets now loading a new version, or loading again from the new pack, and failed
    /// assets of other packs loading again because of it.
    pub reloading: u32,
}

/// A new version of a ready asset on its way (see [`Slot::next`](super::Slot)).
pub(super) struct Next {
    pub stage: NextStage,
    /// Decoded bytes, once read.
    pub data: Option<Arc<AlignedVec>>,
    /// Its dependencies, from the new table of contents; linked as the asset's children too, so
    /// a failing one fails this version.
    pub children: Vec<UntypedHandle>,
    pub size: u64,
    pub hash: [u8; 32],
}

/// Where a [`Next`] version is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NextStage {
    /// Being read or decoded.
    Loading,
    /// Decoded; waiting for its dependencies, and for the asset's current version to be ready.
    Waiting,
    /// In the upload queue.
    Queued,
}

/// The dependency the asset in `slot` failed on, if that is why it failed.
fn failed_dependency(slot: &Slot) -> Option<AssetId> {
    match slot.state {
        SlotState::Failed(
            LoadError::Dependency { dependency, .. } | LoadError::DependencyKind { dependency, .. },
        ) => Some(dependency),
        _ => None,
    }
}

impl AssetServer {
    /// Reads mounted pack `name` again, after its file changed (see
    /// [hot reload](AssetServer#hot-reload)): checks it as [`Self::mount`] does, makes its table of
    /// contents the index of its assets, and reloads every loaded asset whose bytes changed behind
    /// its handles.
    /// Assets still loading start again from the new pack; unused cached assets that changed are
    /// unloaded instead; assets it no longer holds keep the version they have. The pack's earlier
    /// file is closed. A pack that fails to mount keeps the version that was mounted, and an
    /// unchanged one changes nothing.
    ///
    /// **Order:** an asset may be in one mounted pack only, so when an asset moves from one mounted
    /// pack to another, remount the pack it left first. Remounting the pack it moved to first
    /// fails with [`MountError::Duplicate`], and has to be done again after the other.
    pub fn remount(&mut self, name: &str) -> Result<RemountReport, MountError> {
        profiling::scope!("AssetServer::remount");
        let mount = self
            .mounts
            .iter()
            .find(|(_, mount)| mount.name == name)
            .map(|(key, _)| key)
            .ok_or_else(|| MountError::NotMounted(name.to_owned()))?;
        let mut metadata = self
            .metadata_budget
            .begin()
            .map_err(|source| MountError::Pack { name: name.to_owned(), source })?;
        let (file, toc) = mounts::read_pack(&self.source, name, &mut metadata)?;
        let old = &self.mounts[mount];
        // The header holds the table of contents' hash, so an equal one is the same pack, laid out
        // the same: reads in flight from the earlier file stay good.
        if toc.header() == old.toc.header() {
            return Ok(RemountReport::default());
        }
        self.check_duplicates(name, &toc, Some(mount))?;
        let mut report = RemountReport::default();
        for entry in toc.entries() {
            report.changed += u32::from(old.toc.find(entry.id).is_none_or(|old| old.hash != entry.hash));
        }
        report.removed = old.toc.entries().iter().filter(|entry| toc.find(entry.id).is_none()).count() as u32;

        let toc = Arc::new(toc);
        let old_file = std::mem::replace(&mut self.mounts[mount].file, file);
        self.mounts[mount].toc = toc.clone();
        self.index.retain(|_, &mut (other, _)| other != mount);
        self.index
            .extend(toc.entries().iter().enumerate().map(|(index, entry)| (entry.id, (mount, index as u32))));
        report.reloading = self.settle_mount(mount, &toc);
        // Reads still queued for the earlier file are superseded, so their failures are ignored.
        drop(old_file);
        tracing::info!(
            target: LOG_TARGET,
            pack = name,
            changed = report.changed,
            removed = report.removed,
            reloading = report.reloading,
            "pack remounted"
        );
        Ok(report)
    }

    /// Brings loaded assets in line with mount `mount`, whose table of contents is now `toc`:
    /// the mount's assets move to their entries there, assets that left their packs and that `toc`
    /// holds join them, and what changed starts loading. Returns how many assets are now loading
    /// a new version, or loading again.
    pub(super) fn settle_mount(&mut self, mount: MountKey, toc: &Toc) -> u32 {
        // An ID under another kind isn't this asset: `gone` reported it when it left.
        let keys: Vec<SlotKey> = self
            .slots
            .iter()
            .filter(|(_, slot)| {
                slot.mount == mount
                    || (slot.entry.is_none()
                        && toc.find(slot.handle.id()).is_some_and(|entry| entry.kind == slot.handle.kind()))
            })
            .map(|(key, _)| key)
            .collect();
        // Every asset of the pack moves to its new entry before any load starts, so a load that
        // reaches a dependency (a failed one, tried again) finds it where the new pack holds it.
        let mut moved = Vec::with_capacity(keys.len());
        for key in keys {
            let slot = &mut self.slots[key];
            let (id, kind) = (slot.handle.id(), slot.handle.kind());
            let was_present = slot.entry.is_some();
            slot.mount = mount;
            let found = toc.entries().binary_search_by(|entry| entry.id.cmp(&id)).ok();
            let found_kind = found.map(|index| toc.entries()[index].kind);
            slot.entry = found.filter(|_| found_kind == Some(kind)).map(|index| index as u32);
            moved.push((key, found_kind, was_present));
        }
        // Found before any load starts: the loads below retry some of them, but not all they hold up.
        let failed_on_this_pack: Vec<SlotKey> = self
            .slots
            .iter()
            .filter(|(_, slot)| failed_dependency(slot).is_some_and(|id| toc.find(id).is_some()))
            .map(|(key, _)| key)
            .collect();
        let mut reloading = 0;
        for &(key, found_kind, was_present) in &moved {
            // Loading one asset never unloads another, but stay safe against it.
            let Some(slot) = self.slots.get(key) else { continue };
            if slot.entry.is_none() {
                if was_present {
                    self.gone(key, found_kind);
                }
            } else if self.relocate(key, toc) {
                reloading += 1;
            }
        }
        // New versions waiting on one that was dropped (an edit undone, an asset gone from the
        // pack, a reload that couldn't start) go on with the version it has.
        for &(key, _, _) in &moved {
            if self.slots.get(key).is_some_and(|slot| slot.next.is_none()) {
                for parent in self.parents(key) {
                    self.try_queue_next(parent);
                }
            }
        }
        reloading + self.retry_dependents(mount, failed_on_this_pack)
    }

    /// Tries again the assets in `pending`, which failed on a dependency mount `mount` now holds,
    /// and those that failed above them: only assets of other packs that are still in use, since
    /// the mount's own were tried as they moved. Returns how many it tried.
    fn retry_dependents(&mut self, mount: MountKey, mut pending: Vec<SlotKey>) -> u32 {
        let mut seen = HashSet::new();
        let mut retried = 0;
        while let Some(key) = pending.pop() {
            if !seen.insert(key) {
                continue;
            }
            let Some(slot) = self.slots.get(key) else { continue };
            // On through assets no longer failed too: a load may have retried one, not those above.
            pending.extend_from_slice(&slot.parents);
            if slot.mount != mount
                && slot.entry.is_some()
                && slot.users() > 0
                && failed_dependency(slot).is_some()
            {
                self.resolve(key);
                retried += 1;
            }
        }
        retried
    }

    /// How many new versions replaced `handle`'s asset since it loaded (hot reload), and times a
    /// dependency it binds was replaced: its [`Upload::generation`](super::Upload::generation).
    /// Systems that derive data from an asset compare it with the generation they derived from.
    /// 0 for a handle of another server.
    pub fn generation<T: Asset>(&self, handle: &Handle<T>) -> u32 {
        self.untyped_generation(handle.untyped())
    }

    /// [`Self::generation`] for an untyped handle.
    pub fn untyped_generation(&self, handle: &UntypedHandle) -> u32 {
        self.slot(handle).map_or(0, |slot| slot.generation)
    }

    /// Hot reload's changes since the last call: new versions swapped in, reloads that failed and
    /// assets removed from their packs, in the order they happened.
    pub fn take_changes(&mut self) -> Vec<AssetChange> {
        std::mem::take(&mut self.changes)
    }

    /// Starts loading what changed of the asset in `key`, which has moved to its entry in `toc`,
    /// its mount's new table of contents. Returns whether it is now loading a new version, or
    /// loading again.
    fn relocate(&mut self, key: SlotKey, toc: &Toc) -> bool {
        let slot = &self.slots[key];
        let entry = toc.entries()[slot.entry.expect("only assets the new pack holds are relocated") as usize];
        let changed = entry.hash != slot.hash;
        match slot.state {
            SlotState::Loading | SlotState::Waiting | SlotState::Queued => {
                if changed {
                    // Its dependencies may differ too.
                    self.resolve(key);
                } else if slot.state == SlotState::Loading {
                    // The same bytes, but the read in flight is of the file about to close.
                    self.read(key);
                }
                changed || self.slots[key].state == SlotState::Loading
            }
            SlotState::Ready => {
                let next = slot.next.as_ref().map(|next| (next.hash, next.stage));
                if let Some((hash, stage)) = next
                    && hash == entry.hash
                {
                    // Already loading these bytes (or rebinding them): only its read moves.
                    if stage == NextStage::Loading {
                        self.read(key);
                    }
                    return changed;
                }
                // A new version of other bytes is superseded: by this one, or by the bytes the asset
                // has (an edit undone).
                self.cancel_next(key);
                if !changed {
                    return false;
                }
                let slot = &self.slots[key];
                if slot.users() == 0 && slot.pins == 0 {
                    // Cached and unused: the next load reads the new version.
                    self.remove(key);
                    return false;
                }
                self.start_reload(key, toc.dependencies(&entry), &entry);
                true
            }
            SlotState::Failed(_) => {
                // A failed asset that something still wants loads again: the new pack may have
                // fixed it or what it depends on, or hold it again.
                let retry = slot.users() > 0;
                if retry {
                    self.resolve(key);
                }
                retry
            }
        }
    }

    /// The asset in `key` is no longer in its pack, or is now of kind `found` there (its entry is
    /// gone already): it keeps the version it has, if any, and can't be read again.
    fn gone(&mut self, key: SlotKey, found: Option<AssetKind>) {
        let slot = &self.slots[key];
        let (id, kind) = (slot.handle.id(), slot.handle.kind());
        let error = match found {
            Some(found) => LoadError::WrongKind { id, expected: kind, found },
            None => LoadError::Removed(id),
        };
        self.cancel_next(key);
        match self.slots[key].state {
            SlotState::Loading | SlotState::Waiting | SlotState::Queued => self.fail(key, error),
            SlotState::Ready => {
                tracing::warn!(target: LOG_TARGET, %id, %kind, %error, "asset left its pack; keeping the loaded version");
                self.changes.push(AssetChange::Removed { id, kind });
            }
            SlotState::Failed(_) => {}
        }
    }

    /// Starts loading the new version `entry` of the ready asset in `key`, after its
    /// dependencies `dependencies`, while it keeps the version it has.
    fn start_reload(&mut self, key: SlotKey, dependencies: &[AssetId], entry: &Entry) {
        match self.load_dependencies(key, dependencies) {
            Ok(children) => {
                self.link(key, &children);
                let next = Next {
                    stage: NextStage::Loading,
                    data: None,
                    children,
                    size: entry.size,
                    hash: entry.hash,
                };
                self.slots[key].next = Some(next);
                self.read(key);
            }
            Err(error) => self.reload_failed(key, error),
        }
    }

    /// Queues the next version of the asset in `key` for the GPU once its dependencies are ready
    /// with no newer versions of their own on the way, so dependencies are replaced first, and
    /// once the asset's current version is ready (a new renderer gets that first). Fails it if a
    /// dependency failed. A kind that doesn't go to the GPU is swapped in at once.
    pub(super) fn try_queue_next(&mut self, key: SlotKey) {
        let Some(slot) = self.slots.get(key).filter(|slot| slot.state == SlotState::Ready) else { return };
        let Some(next) = slot.next.as_ref().filter(|next| next.stage == NextStage::Waiting) else { return };
        let mut failed = None;
        for child in &next.children {
            let child_slot = &self.slots[child.key()];
            match child_slot.state {
                SlotState::Ready if child_slot.next.is_none() => {}
                SlotState::Failed(_) => {
                    failed = Some(child.id());
                    break;
                }
                _ => return,
            }
        }
        let (id, uploads) = (slot.handle.id(), slot.handle.kind().uploads());
        match failed {
            Some(dependency) => self.fail_next(key, LoadError::Dependency { id, dependency }),
            None if !uploads => self.swap(key),
            None => {
                self.slots[key].next.as_mut().expect("checked above").stage = NextStage::Queued;
                self.upload_queue.push_back((key, Version::Next));
            }
        }
    }

    /// The next version of the asset in `key` is on the GPU, or needs no GPU: it replaces the
    /// current one behind every handle, and the dependencies only the old version had are
    /// released.
    pub(super) fn swap(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        let next = slot.next.take().expect("only a slot with a next version swaps");
        slot.data = if slot.handle.kind().keeps_bytes() { next.data } else { None };
        (slot.size, slot.hash) = (next.size, next.hash);
        let old = std::mem::replace(&mut slot.children, next.children);
        self.unlink(key, &old);
        self.replaced(key);
    }

    /// A new version of the asset in `key` is current: it is a new generation, reported, and the
    /// assets that depend on it go on with their own reloads or bind it again.
    pub(super) fn replaced(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        slot.generation += 1;
        let (id, kind, generation) = (slot.handle.id(), slot.handle.kind(), slot.generation);
        tracing::info!(target: LOG_TARGET, %id, %kind, generation, "asset reloaded");
        self.reloads += 1;
        self.changes.push(AssetChange::Reloaded { id, kind, slot: AssetSlot(key), generation });
        for parent in self.parents(key) {
            self.dependency_replaced(parent);
        }
    }

    /// A dependency of the asset in `parent` was replaced: a new version of `parent` waiting on
    /// it may go on, and an asset that binds its dependencies is sent again as a version of its
    /// own, so the app binds the new dependency: a ready one from its kept bytes, one on its way
    /// to a new renderer ([`Self::reupload_all`]) as the next generation.
    fn dependency_replaced(&mut self, parent: SlotKey) {
        let Some(slot) = self.slots.get_mut(parent) else { return };
        if slot.next.is_none() && slot.loaded && slot.handle.kind().binds_dependencies() {
            match slot.state {
                SlotState::Ready => {
                    let Some(data) = slot.data.clone() else { return };
                    slot.next = Some(Next {
                        stage: NextStage::Waiting,
                        data: Some(data),
                        children: slot.children.clone(),
                        size: slot.size,
                        hash: slot.hash,
                    });
                }
                SlotState::Loading | SlotState::Waiting | SlotState::Queued => slot.replaces = true,
                SlotState::Failed(_) => {}
            }
        }
        self.try_queue_next(parent);
    }

    /// Drops the next version of the asset in `key`, which failed: the asset keeps the version it
    /// has, and new versions of assets waiting on this one go on with it.
    pub(super) fn fail_next(&mut self, key: SlotKey, error: LoadError) {
        if self.slots[key].next.is_none() {
            return;
        }
        self.cancel_next(key);
        self.reload_failed(key, error);
        for parent in self.parents(key) {
            self.try_queue_next(parent);
        }
    }

    /// Reports that a new version of the asset in `key` failed.
    fn reload_failed(&mut self, key: SlotKey, error: LoadError) {
        let handle = &self.slots[key].handle;
        let (id, kind) = (handle.id(), handle.kind());
        tracing::warn!(target: LOG_TARGET, %id, %kind, error = error_chain(&error), "reload failed; keeping the loaded version");
        self.reload_failures += 1;
        self.changes.push(AssetChange::ReloadFailed { id, kind, error });
    }

    /// Drops the next version of the asset in `key`, if any, releasing the dependencies only it
    /// had. A read of it in flight is ignored when it completes.
    pub(super) fn cancel_next(&mut self, key: SlotKey) {
        if let Some(next) = self.slots[key].next.take() {
            self.unlink(key, &next.children);
        }
    }

    /// Makes the next version of the asset in `key` its main state, for a new renderer
    /// ([`Self::reupload_all`]): its pack may no longer hold the version the asset has. Uploading
    /// it makes the next generation.
    pub(super) fn promote(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        let next = slot.next.take().expect("only a slot with a next version is promoted");
        (slot.size, slot.hash) = (next.size, next.hash);
        slot.replaces = true;
        match next.stage {
            // Its read in flight now completes the main state.
            NextStage::Loading => {
                slot.state = SlotState::Loading;
                slot.data = None;
            }
            NextStage::Waiting | NextStage::Queued => {
                slot.state = SlotState::Waiting;
                slot.data = next.data;
            }
        }
        let old = std::mem::replace(&mut slot.children, next.children);
        self.unlink(key, &old);
    }
}
