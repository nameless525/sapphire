//! Load admission (M5 M2): the cap on bytes read or decoded and not yet on the GPU, the reads held
//! back under it, and raising a load's priority. [`AssetServer`]'s docs ("Admission") describe what an
//! app sees; this file holds the server's half.

use super::handle::SlotKey;
use super::{Asset, AssetServer, Handle, LoadPriority, SlotState};

/// A held read's place in the queue: its priority, then its age.
pub(super) type ReadOrder = (LoadPriority, u64);

impl AssetServer {
    /// Raises `handle`'s asset to `priority`, if that's more urgent than it loads at, and every dependency
    /// it has with it: a load the app now wants sooner (a texture's level the streamer needs this frame,
    /// say). A read held back for admission moves up the queue, keeping its age within its new priority.
    /// A read already asked of the source keeps the priority it went with, and an asset decoded and queued
    /// for the GPU keeps its place: see [admission](AssetServer#admission). Nothing for a ready asset but
    /// the priority it reads at again, after a new renderer or a remount.
    pub fn raise_priority<T: Asset>(&mut self, handle: &Handle<T>, priority: LoadPriority) {
        if self.slot(handle.untyped()).is_some() {
            self.raise(handle.key(), priority);
            self.admit();
        }
    }

    /// Raises the asset in `key` and its dependencies, at any depth, to `priority` where they're less
    /// urgent. Dependencies always load at least as urgently as what depends on them, so one already as
    /// urgent has nothing below it to raise.
    pub(super) fn raise(&mut self, key: SlotKey, priority: LoadPriority) {
        let mut stack = vec![key];
        while let Some(key) = stack.pop() {
            let slot = &mut self.slots[key];
            if slot.priority <= priority {
                continue;
            }
            slot.priority = priority;
            if let Some(order) = slot.held {
                let moved = (priority, order.1);
                slot.held = Some(moved);
                self.held_back.remove(&order);
                self.held_back.insert(moved, key);
            }
            let slot = &self.slots[key];
            let next_children = slot.next.iter().flat_map(|next| &next.children);
            stack.extend(slot.children.iter().chain(next_children).map(|child| child.key()));
        }
    }

    /// Starts reading and decoding the asset in `key` from where its pack holds it, superseding any read
    /// of it in flight, once [admission](AssetServer#admission) lets it. The caller says which version
    /// waits for it: the main state while it's loading, else the next version.
    pub(super) fn read(&mut self, key: SlotKey) {
        let slot = &mut self.slots[key];
        slot.request = slot.request.wrapping_add(1);
        if !slot.handle.kind().uploads() {
            // Its decoded bytes are what stays resident: nothing waits on the GPU to take them.
            return self.issue(key);
        }
        let (size, admitted) = self.reading(key);
        if let Some(admitted) = admitted {
            // Read again (its pack was remounted): it keeps its admission, at its new size.
            self.admitted = self.admitted - admitted + size;
            self.admitted_peak = self.admitted_peak.max(self.admitted);
            *self.reading_admission(key) = Some(size);
            self.issue(key);
            // Its dependencies may have changed with its bytes, and it waits for them.
            return self.admit_dependencies(key);
        }
        if self.slots[key].held.is_some() {
            // Already waiting its turn, now for this read.
            return;
        }
        if self.awaited(key) {
            // Something admitted waits for it, and gives nothing back until it's in.
            self.reserve(key, size);
            self.issue(key);
            return self.admit_dependencies(key);
        }
        let order = (self.slots[key].priority, self.read_sequence);
        self.read_sequence += 1;
        self.slots[key].held = Some(order);
        self.held_back.insert(order, key);
        self.admit();
    }

    /// Admits the reads held back that fit, most urgent first, oldest first within a priority, skipping
    /// any whose asset still has a dependency held back, until one that's free to go doesn't fit. One
    /// always goes when nothing is admitted.
    pub(super) fn admit(&mut self) {
        if self.held_back.is_empty() {
            return;
        }
        let mut chosen = std::mem::take(&mut self.admission_scratch);
        chosen.clear();
        let mut admitted = self.admitted;
        for (&order, &key) in &self.held_back {
            if self.holds_back_a_dependency(key) {
                continue;
            }
            let (size, _) = self.reading(key);
            if admitted > 0 && admitted.saturating_add(size) > self.budgets.admission {
                break;
            }
            admitted = admitted.saturating_add(size);
            chosen.push(order);
        }
        for &order in &chosen {
            let key = self.held_back.remove(&order).expect("chosen from the reads held back");
            self.slots[key].held = None;
            let (size, _) = self.reading(key);
            self.reserve(key, size);
            self.issue(key);
        }
        self.admission_scratch = chosen;
    }

    /// Admits every read held back of the asset in `key`'s dependencies, at any depth, over the cap if
    /// it must: the asset holds an admission it was given without waiting for them (read again with
    /// other dependencies, or awaited itself), and gives nothing back until they're in. A dependency
    /// that already held back a read for another asset is one this asset now waits on too. Keeps
    /// [admission](AssetServer#admission)'s rule that nothing admitted waits on a read held back.
    fn admit_dependencies(&mut self, key: SlotKey) {
        let mut stack = vec![key];
        while let Some(key) = stack.pop() {
            let slot = &self.slots[key];
            let next_children = slot.next.iter().flat_map(|next| &next.children);
            for child in slot.children.iter().chain(next_children) {
                stack.push(child.key());
            }
            if let Some(order) = self.slots[key].held.take() {
                self.held_back.remove(&order);
                let (size, _) = self.reading(key);
                self.reserve(key, size);
                self.issue(key);
            }
        }
    }

    /// Drops the read held back for the asset in `key`, if one is: the version it was for failed or went.
    pub(super) fn unhold(&mut self, key: SlotKey) {
        if let Some(order) = self.slots[key].held.take() {
            self.held_back.remove(&order);
        }
    }

    /// Gives back bytes a version held against admission.
    pub(super) fn release(&mut self, admitted: Option<u64>) {
        self.admitted -= admitted.unwrap_or(0);
    }

    /// Holds `size` bytes against admission for the version of the asset in `key` that's reading.
    fn reserve(&mut self, key: SlotKey, size: u64) {
        *self.reading_admission(key) = Some(size);
        self.admitted += size;
        self.admitted_peak = self.admitted_peak.max(self.admitted);
    }

    /// The size of the version of the asset in `key` that's reading, and what it's admitted with.
    fn reading(&self, key: SlotKey) -> (u64, Option<u64>) {
        let slot = &self.slots[key];
        if slot.state == SlotState::Loading {
            (slot.size, slot.admitted)
        } else {
            let next = slot.next.as_ref().expect("an asset that isn't loading reads its next version");
            (next.size, next.admitted)
        }
    }

    fn reading_admission(&mut self, key: SlotKey) -> &mut Option<u64> {
        let slot = &mut self.slots[key];
        if slot.state == SlotState::Loading {
            &mut slot.admitted
        } else {
            &mut slot.next.as_mut().expect("an asset that isn't loading reads its next version").admitted
        }
    }

    /// Whether a dependency of the asset in `key`, at any depth, has a read held back. Chains are at most
    /// a few deep, since a dependency is of a lower kind.
    fn holds_back_a_dependency(&self, key: SlotKey) -> bool {
        let slot = &self.slots[key];
        let next_children = slot.next.iter().flat_map(|next| &next.children);
        slot.children.iter().chain(next_children).any(|child| {
            let child = child.key();
            self.slots[child].held.is_some() || self.holds_back_a_dependency(child)
        })
    }

    /// Whether an asset that depends on the one in `key`, at any depth, holds an admission: it decodes or
    /// waits with its bytes, and can need this read before it gives them back.
    fn awaited(&self, key: SlotKey) -> bool {
        self.slots[key].parents.iter().any(|&parent| {
            let slot = &self.slots[parent];
            slot.admitted.is_some()
                || slot.next.as_ref().is_some_and(|next| next.admitted.is_some())
                || self.awaited(parent)
        })
    }
}
