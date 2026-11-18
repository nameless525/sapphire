//! What a zone's documents hold each other to (M6 T2): a placement, or an instance, and the colliders it names.
//!
//! A zone's colliders are a document of their own, and a cook refuses a placement that moved without the
//! colliders that stand for it, or a collider without its placement (M4 X1). So an edit of a zone goes through
//! [`edit_with_colliders`]: whatever the edit is (a gizmo's drag of one number, a whole position typed, three
//! placements set at once), each placement or instance it moved takes the colliders it names with it, by as
//! far, and the zone's step and the colliders' are one [`crate::SetStep`], or one step of the set's gesture.
//!
//! It carries a move and nothing else: a turn or a scale leaves the colliders where they are, for the cook to
//! judge, and so does an edit that adds or takes out placements or instances (their places shift, so a place
//! before and after needn't be one thing) or reorders them. A collider the colliders' document doesn't have is
//! the cook's to name, and stays out of it. A step that moved colliders is bound ([`crate::SetStep::bound`]):
//! undo and redo take it whole or refuse it, never half.

use std::collections::BTreeMap;
use std::path::Path;

use sapphire_assets::AssetId;
use sapphire_colliders::ColliderDocument;
use sapphire_zone::ZoneDocument;

use crate::document::{Outcome, Refused};
use crate::kinds::Kind;
use crate::path::{PropertyPath, Step};
use crate::set::{AnyDocument, DocumentSet, SetError};
use crate::write;

/// Why a zone's edit didn't land. Every text is as it was.
#[derive(Debug, thiserror::Error)]
pub enum ZoneEditError {
    #[error(transparent)]
    Refused(#[from] Refused),
    /// The zone's colliders' document couldn't be opened to move with it.
    #[error("the zone's colliders can't be opened to move with it: {0}")]
    Colliders(#[from] SetError),
    /// It isn't open, or isn't a zone.
    #[error("no zone with the ID {0} is open")]
    NotAZone(AssetId),
    /// Two placements naming one collider moved apart, so there's no telling where it goes.
    #[error("collider `{0}` is named by things that moved apart: move them together")]
    MovedApart(String),
}

/// Where something a zone places stands, and the colliders it names.
#[derive(Clone, Debug, PartialEq)]
struct Placed {
    at: [f32; 3],
    colliders: Vec<String>,
}

/// Each placement and instance, by its list and its place in it.
fn placed(zone: &ZoneDocument) -> BTreeMap<(&'static str, usize), Placed> {
    let placements = zone.placements.iter().enumerate().map(|(index, placement)| {
        (("placements", index), Placed { at: placement.at, colliders: placement.colliders.clone() })
    });
    let instances = zone.instances.iter().enumerate().map(|(index, instance)| {
        (("instances", index), Placed { at: instance.at, colliders: instance.colliders.clone() })
    });
    placements.chain(instances).collect()
}

/// How many placements and instances a zone has.
const fn lengths(zone: &ZoneDocument) -> (usize, usize) {
    (zone.placements.len(), zone.instances.len())
}

/// Runs `edit` on the zone `zone`, and moves each collider a placement or instance it moved names by as far,
/// as one edit (see the [module docs](self)): a step of its own under `label`, or part of the set's gesture.
/// Refused, nothing changes.
pub fn edit_with_colliders(
    set: &mut DocumentSet,
    zone: AssetId,
    label: &str,
    edit: impl FnOnce(&mut dyn AnyDocument) -> Result<Outcome, Refused>,
) -> Result<Outcome, ZoneEditError> {
    let before = set
        .get::<ZoneDocument>(zone)
        .ok_or(ZoneEditError::NotAZone(zone))?
        .value()
        .map(|zone| (placed(zone), lengths(zone)));
    set.together(label, |set| {
        let document = set.edit(zone).ok_or(ZoneEditError::NotAZone(zone))?;
        let outcome = edit(document)?;
        let Some((before, was_long)) = before else { return Ok(outcome) };
        let Some(value) = set.get::<ZoneDocument>(zone).and_then(|document| document.value()) else {
            return Ok(outcome);
        };
        // A placement added or taken out shifts the rest, so one place before and after needn't be one thing.
        if lengths(value) != was_long {
            return Ok(outcome);
        }
        let (after, colliders) = (placed(value), value.colliders);
        let mut moves: BTreeMap<String, [f64; 3]> = BTreeMap::new();
        for (place, was) in &before {
            let Some(now) = after.get(place) else { continue };
            if now.at == was.at || now.colliders != was.colliders {
                continue;
            }
            let by: [f64; 3] = std::array::from_fn(|axis| f64::from(now.at[axis]) - f64::from(was.at[axis]));
            for name in &now.colliders {
                if moves.insert(name.clone(), by).is_some_and(|other| other != by) {
                    return Err(ZoneEditError::MovedApart(name.clone()));
                }
            }
        }
        let Some(colliders) = colliders.filter(|_| !moves.is_empty()) else { return Ok(outcome) };
        let id = open_colliders(set, colliders)?;
        let document = set.edit(id).ok_or(ZoneEditError::NotAZone(id))?;
        for (name, by) in moves {
            let Some(position) = position_of(&*document, &name) else { continue };
            // In f64, so a collider standing where its placement does lands where it does, to the bit.
            let moved: [f32; 3] = std::array::from_fn(|axis| (f64::from(position[axis]) + by[axis]) as f32);
            // A number at a time, as a person edits one: the bytes of the others stay as they were.
            for axis in (0..3).filter(|axis| moved[*axis].to_bits() != position[*axis].to_bits()) {
                let text = write::canonical(&moved[axis], document.registry()).map_err(Refused::from)?;
                document.set_ron(label, &collider_position(&name, axis), &text)?;
            }
        }
        // A placement and its colliders are one step that never goes apart.
        set.bind_gesture();
        Ok(Outcome::Changed)
    })
}

/// The colliders' document `id`, opened if it isn't.
fn open_colliders(set: &mut DocumentSet, id: AssetId) -> Result<AssetId, SetError> {
    if set.any(id).is_some() {
        return Ok(id);
    }
    let path = set.find(id).ok_or(SetError::NotOpen(id))?;
    Kind::Colliders.open(set, Path::new(&path))
}

/// One number of where the collider `name` stands.
fn collider_position(name: &str, axis: usize) -> PropertyPath {
    PropertyPath::root()
        .join(Step::Field("colliders".to_owned()))
        .join(Step::Key(name.to_owned()))
        .join(Step::Field("position".to_owned()))
        .join(Step::Index(axis))
}

/// Where the collider `name` stands, if the document has one of that name.
fn position_of(document: &dyn AnyDocument, name: &str) -> Option<[f32; 3]> {
    let colliders: &ColliderDocument =
        document.as_any().downcast_ref::<crate::Document<ColliderDocument>>()?.value()?;
    let mut named = colliders.colliders.iter().filter(|entry| entry.name == name);
    let entry = named.next()?;
    named.next().is_none().then_some(entry.position)
}
