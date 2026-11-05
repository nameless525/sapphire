//! Progress: two rotating slots, the newest whole one wins.

use std::io;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sapphire_reflect::{TypePath, Versioned};
use sapphire_serialize::{SaveError, from_save, to_save};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::aside::keep_aside;
use crate::lock::WriteLock;
use crate::slot::{self, FORMAT, Frame, Slot, SlotFault, SlotRead};
use crate::{LOG_TARGET, Replace, Snapshot, check_name};

/// A game's progress on disk, as two files that take turns.
///
/// A write replaces the slot that doesn't hold the newest save, so a write that goes wrong in any
/// way leaves the newest save as it was. "Newest" is a counter in the file, the generation, so a
/// clock going backward means nothing here.
///
/// **What each case does:**
///
/// | On disk | [`load`](Self::load) | [`write`](Self::write) |
/// |---|---|---|
/// | No slots | `Ok(None)`: a new game | Generation 1, to slot A |
/// | One whole slot, the other missing | That slot | To the missing one |
/// | Both whole | The higher generation | Over the lower |
/// | One damaged | The other, with [`OtherSlot::Damaged`] | Over the damaged one, kept aside first |
/// | Both damaged | [`LoadError::AllSlotsDamaged`] | Generation 1, each kept aside as it's replaced |
/// | Any whole slot from a newer build | [`LoadError::Newer`] | [`WriteError::Newer`]; the file stays byte for byte |
/// | A slot from an older schema | Upgraded by the type's [`Versioned`] chain | As for any whole slot |
/// | A whole slot at generation `u64::MAX` | That slot, as usual | [`WriteError::GenerationExhausted`]; both slots stay byte for byte |
///
/// A damaged slot can't say how new it was, so a fall-back may have cost the last save or nothing.
/// A newer build's slot refuses even when the other slot would load: a player who plays on with
/// the older build would rotate over it within two saves. An error from `load` leaves the choice
/// with the game, since a write after [`LoadError::AllSlotsDamaged`] is how a player starts again.
///
/// A clone is cheap and can go to a job. See the crate docs for keeping a write off the frame's
/// path.
pub struct ProgressStore<T> {
    dir: PathBuf,
    name: String,
    replace: Replace,
    of: PhantomData<fn() -> T>,
}

impl<T> Clone for ProgressStore<T> {
    fn clone(&self) -> Self {
        Self {
            dir: self.dir.clone(),
            name: self.name.clone(),
            replace: self.replace.clone(),
            of: PhantomData,
        }
    }
}

/// A save read from a slot.
#[derive(Debug)]
pub struct LoadedSave<T> {
    pub value: T,
    pub slot: Slot,
    pub generation: u64,
    /// The older schema version the save was upgraded from, if it was.
    pub upgraded_from: Option<u32>,
    /// What's in the slot that wasn't used.
    pub other: OtherSlot,
}

impl<T> LoadedSave<T> {
    /// Whether the other slot was damaged, so this may be the save before the last. It stays true
    /// on every load until the next write replaces the damaged slot, so a game tells a player
    /// once by remembering it has.
    pub fn fell_back(&self) -> bool {
        matches!(self.other, OtherSlot::Damaged(_))
    }
}

/// The slot a load didn't use.
#[derive(Debug)]
pub enum OtherSlot {
    /// Never written, as before a store's second write, or gone since.
    Missing,
    /// Whole, and older.
    Older { generation: u64 },
    /// Couldn't be used.
    Damaged(SlotFault),
}

/// What a write did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub slot: Slot,
    pub generation: u64,
    /// The file's size.
    pub bytes: usize,
}

/// A whole slot this build must neither read nor overwrite.
#[derive(Debug, thiserror::Error)]
#[error(
    "slot {slot} was written by a newer build (format {format}, schema {schema}; this build has format \
     {FORMAT}, schema {current}): the game needs updating, and the file is left alone"
)]
pub struct NewerSave {
    pub slot: Slot,
    pub format: u32,
    pub schema: u32,
    pub current: u32,
}

/// A load that found saves and gave none.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error(transparent)]
    Newer(#[from] NewerSave),
    /// Neither slot can be used. A slot that's missing is `None`.
    #[error("no save slot can be used. Slot a: {}. Slot b: {}.", describe(a), describe(b))]
    AllSlotsDamaged { a: Option<SlotFault>, b: Option<SlotFault> },
}

fn describe(fault: &Option<SlotFault>) -> String {
    fault.as_ref().map_or_else(|| "missing".to_owned(), ToString::to_string)
}

/// A write that didn't happen. Whatever was on disk still loads.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error(transparent)]
    Newer(#[from] NewerSave),
    /// A valid slot already has the largest generation; neither slot was changed.
    #[error("save generation is exhausted; the existing slots are left alone")]
    GenerationExhausted,
    #[error("writing {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
}

impl<T: Versioned + Serialize + DeserializeOwned + TypePath> ProgressStore<T> {
    /// The store named `name` in `dir`. `replace` writes a file whole: an app passes
    /// `sapphire_platform::durable_replace`.
    ///
    /// # Panics
    ///
    /// If `name` isn't a plain file stem.
    pub fn new(
        dir: impl Into<PathBuf>,
        name: &str,
        replace: impl Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync + 'static,
    ) -> Self {
        check_name(name);
        Self { dir: dir.into(), name: name.to_owned(), replace: Arc::new(replace), of: PhantomData }
    }

    /// The path of `slot`'s file.
    pub fn slot_path(&self, slot: Slot) -> PathBuf {
        self.dir.join(format!("{}.{}.sav", self.name, slot.letter()))
    }

    /// Reads the newest usable save, or `None` if nothing has been written. It only reads: a
    /// damaged slot stays until a write replaces it.
    pub fn load(&self) -> Result<Option<LoadedSave<T>>, LoadError> {
        let [a, b] = self.read_slots();
        refuse_newer(&[(Slot::A, &a), (Slot::B, &b)], T::VERSION)?;
        // The higher generation first; on a tie, which only copied files can make, slot A.
        let newest = match (&a, &b) {
            (SlotRead::Whole(a), SlotRead::Whole(b)) if b.generation > a.generation => Slot::B,
            (SlotRead::Whole(_), _) => Slot::A,
            (_, SlotRead::Whole(_)) => Slot::B,
            (SlotRead::Missing, SlotRead::Missing) => return Ok(None),
            _ => Slot::A,
        };
        let [newest_read, other_read] = match newest {
            Slot::A => [a, b],
            Slot::B => [b, a],
        };
        match self.decode(newest, newest_read) {
            Ok(frame_value) => Ok(Some(loaded(newest, frame_value, other_of(other_read)))),
            Err(newest_fault) => match self.decode(newest.other(), other_read) {
                Ok(frame_value) => {
                    let other = newest_fault.map_or(OtherSlot::Missing, OtherSlot::Damaged);
                    Ok(Some(loaded(newest.other(), frame_value, other)))
                }
                Err(other_fault) => {
                    let (a, b) = match newest {
                        Slot::A => (newest_fault, other_fault),
                        Slot::B => (other_fault, newest_fault),
                    };
                    let error = LoadError::AllSlotsDamaged { a, b };
                    tracing::error!(target: LOG_TARGET, store = %self.name, %error, "no save loaded");
                    Err(error)
                }
            },
        }
    }

    /// Serializes `value` into plain bytes, for [`write`](Self::write) to put on disk later and
    /// elsewhere.
    pub fn snapshot(&self, value: &T) -> Result<Snapshot<T>, SaveError> {
        to_save(value).map(Snapshot::new)
    }

    /// Writes a snapshot over the slot that doesn't hold the newest save. It reads both slots
    /// first, with no memory of an earlier load, so what's on disk is what it decides by. It can
    /// stall for as long as the storage does: see the crate docs.
    pub fn write(&self, snapshot: &Snapshot<T>) -> Result<Written, WriteError> {
        let lock_path = self.dir.join(format!("{}.lock", self.name));
        let _lock = WriteLock::take(&lock_path);
        let [a, b] = self.read_slots();
        refuse_newer(&[(Slot::A, &a), (Slot::B, &b)], T::VERSION)?;

        let (a_generation, b_generation) = (a.generation(), b.generation());
        let (target, target_read) = match (a_generation, b_generation) {
            (Some(a_generation), Some(b_generation)) if a_generation > b_generation => (Slot::B, &b),
            (Some(_), None) => (Slot::B, &b),
            _ => (Slot::A, &a),
        };
        let generation = a_generation
            .max(b_generation)
            .map_or(Some(1), |newest| newest.checked_add(1))
            .ok_or(WriteError::GenerationExhausted)?;

        let path = self.slot_path(target);
        if let SlotRead::Damaged(fault) = target_read {
            keep_aside(&path, &fault.to_string());
        }

        let file = slot::encode(generation, T::VERSION, &snapshot.bytes);
        (self.replace)(&path, &file).map_err(|source| WriteError::Io { path, source })?;
        Ok(Written { slot: target, generation, bytes: file.len() })
    }

    fn read_slots(&self) -> [SlotRead; 2] {
        Slot::BOTH.map(|slot| slot::read(&self.slot_path(slot)))
    }

    /// Decodes a slot's read, or gives its fault, `None` if it's missing.
    fn decode(&self, slot: Slot, read: SlotRead) -> Result<Decoded<T>, Option<SlotFault>> {
        let frame = match read {
            SlotRead::Missing => return Err(None),
            SlotRead::Damaged(fault) => {
                tracing::warn!(target: LOG_TARGET, store = %self.name, %slot, %fault, "a save slot is damaged");
                return Err(Some(fault));
            }
            SlotRead::Whole(frame) => frame,
        };
        match from_save::<T>(&frame.body) {
            Ok(value) => Ok(Decoded { value, generation: frame.generation, schema: frame.schema }),
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, store = %self.name, %slot, %error, "a save slot won't decode");
                Err(Some(SlotFault::Undecodable(error)))
            }
        }
    }
}

struct Decoded<T> {
    value: T,
    generation: u64,
    schema: u32,
}

fn loaded<T: Versioned>(slot: Slot, decoded: Decoded<T>, other: OtherSlot) -> LoadedSave<T> {
    let Decoded { value, generation, schema } = decoded;
    let upgraded_from = (schema != T::VERSION).then_some(schema);
    LoadedSave { value, slot, generation, upgraded_from, other }
}

fn other_of(read: SlotRead) -> OtherSlot {
    match read {
        SlotRead::Missing => OtherSlot::Missing,
        SlotRead::Damaged(fault) => OtherSlot::Damaged(fault),
        SlotRead::Whole(Frame { generation, .. }) => OtherSlot::Older { generation },
    }
}

/// The first whole slot from a newer build, as an error. Only a good checksum gets here, so a
/// flipped byte can't fake one.
fn refuse_newer(slots: &[(Slot, &SlotRead)], current: u32) -> Result<(), NewerSave> {
    for (slot, read) in slots {
        if let SlotRead::Whole(frame) = read
            && (frame.format != FORMAT || frame.schema > current)
        {
            let newer = NewerSave { slot: *slot, format: frame.format, schema: frame.schema, current };
            tracing::error!(target: LOG_TARGET, error = %newer, "refusing a newer save");
            return Err(newer);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
