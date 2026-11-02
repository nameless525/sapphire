//! A fixed-size, sorted book of what happened to each move instance: who it hit, and which of its
//! volumes a wall stopped.

use crate::actors::ActorId;
use crate::checksum::StateHasher;

/// What a book says about one thing, in one volley of one move instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry<K> {
    pub(super) attacker: ActorId,
    pub(super) instance: u32,
    pub(super) key: K,
    /// The latest volley this happened in.
    pub(super) volley: u32,
}

/// Entries sorted by attacker, instance and key, so the bytes it hashes never depend on the order
/// anything was written or removed in. Never grows past what it reserved: a full book refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Book<K> {
    pub(super) entries: Vec<Entry<K>>,
    capacity: usize,
}

impl<K: Copy + Ord> Book<K> {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { entries: Vec::with_capacity(capacity), capacity }
    }

    fn find(&self, attacker: ActorId, instance: u32, key: K) -> Result<usize, usize> {
        self.entries.binary_search_by_key(&(attacker, instance, key), |e| (e.attacker, e.instance, e.key))
    }

    /// The volley last written for this, if any.
    pub(super) fn volley_of(&self, attacker: ActorId, instance: u32, key: K) -> Option<u32> {
        self.find(attacker, instance, key).ok().map(|slot| self.entries[slot].volley)
    }

    /// Writes this down, keeping the later volley, and returns `false` if the book had no room.
    pub(super) fn write(&mut self, attacker: ActorId, instance: u32, key: K, volley: u32) -> bool {
        match self.find(attacker, instance, key) {
            Ok(slot) => {
                let entry = &mut self.entries[slot];
                entry.volley = entry.volley.max(volley);
                true
            }
            Err(_) if self.entries.len() >= self.capacity => false,
            Err(slot) => {
                self.entries.insert(slot, Entry { attacker, instance, key, volley });
                true
            }
        }
    }

    /// Forgets an attacker's instances before `instance`.
    pub(super) fn retire(&mut self, attacker: ActorId, instance: u32) {
        let start = self.entries.partition_point(|e| e.attacker < attacker);
        let end = self.entries.partition_point(|e| (e.attacker, e.instance) < (attacker, instance));
        if start < end {
            self.entries.drain(start..end);
        }
    }

    /// Forgets whatever `forget` picks.
    pub(super) fn forget(&mut self, forget: impl Fn(&Entry<K>) -> bool) {
        self.entries.retain(|entry| !forget(entry));
    }

    pub(super) fn write_canonical(&self, hasher: &mut StateHasher, write_key: impl Fn(&mut StateHasher, K)) {
        hasher.u32(self.entries.len() as u32);
        for entry in &self.entries {
            hasher.u32(entry.attacker.raw());
            hasher.u32(entry.instance);
            write_key(hasher, entry.key);
            hasher.u32(entry.volley);
        }
    }
}
