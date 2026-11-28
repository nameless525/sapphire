//! What a zone's documents hold each other to (M6 T2): a placement, or an instance, and the colliders it names.
//!
//! A zone's colliders are a document of their own, and a cook refuses a placement that moved without the
//! colliders that stand for it, or a collider without its placement (M4 X1). So an edit of a zone goes through
//! [`edit_with_colliders`]: whatever the edit is (a gizmo's drag of one number, a whole position typed, three
//! placements set at once), each placement or instance it moved takes the colliders it names with it, by as
//! far, and the zone's step and the colliders' are one [`crate::SetStep`], or one step of the set's gesture.
//!
//! It carries a move and a turn (M6 T9b): a collider turns about where its placement stands by as much as the
//! placement turned, and its own rotation with it, so a placement turned with a gizmo's ring stays held to what
//! stands for it. A scale leaves the colliders as they are, for the cook to judge (it refuses a placement shrunk
//! inside its colliders' sides), and so does an edit that adds or takes out placements or instances (their places
//! shift, so a place before and after needn't be one thing) or reorders them: [`duplicate`] and [`remove`] are
//! how those keep the colliders in step, a copy naming copies of its colliders and an entry taken out taking the
//! colliders only it named. A collider the colliders' document doesn't have is the cook's to name, and stays out
//! of it. A step that moved, made or took out colliders is bound ([`crate::SetStep::bound`]): undo and redo take
//! it whole or refuse it, never half.
//!
//! **IDs (M6 T8a):** a zone's placements, instances, lights and atmosphere volumes each have an ID, which a
//! path calls the entry by (`placements["3fa91c07"]`), so an editor's selection names the same entry however
//! the lists shift. An entry added here takes a fresh one ([`new_id`]). A version 1 document's IDs are its
//! entries' places, which an entry added or taken out would shift under the selection, so an edit of one that
//! adds or takes out such an entry is refused until the document is migrated ([`migrate`]): its text written
//! at version 2 with the IDs it already reads with, comments and layout kept, and nothing else changed.

use std::collections::BTreeMap;
use std::path::Path;

use bevy_reflect::{PartialReflect, Reflect, ReflectFromReflect, ReflectRef, TypeRegistry};
use sapphire_assets::AssetId;
use sapphire_colliders::{ColliderDocument, ColliderEntry, Rotation};
use sapphire_math::{Quat, Vec3, det};
use sapphire_reflect::Versioned;
use sapphire_zone::document::orientation;
use sapphire_zone::{AtmosphereVolume, IdList, Instance, Light, Placement, ZoneDocument, ZoneError};

use crate::document::{Document, Outcome, Refused};
use crate::kinds::Kind;
use crate::layout::{self, Splice};
use crate::path::{PropertyPath, Step};
use crate::set::{AnyDocument, DocumentSet, SetError};
use crate::{blank, text, walk, write};

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
    /// It would add, take out, reorder or rename (`what`) an entry of a version 1 zone's `list`, whose IDs are
    /// its entries' places.
    #[error(
        "the zone is at version 1, whose IDs are its entries' places, which an edit that {what} its `{list}` \
         can't keep: migrate it to version 2 first (MigrateZone)"
    )]
    PlacesShift { list: &'static str, what: &'static str },
    /// An entry asked for by an ID another entry of the zone has.
    #[error("the zone has an entry called `{0}` already, and an ID names one thing in the zone")]
    Taken(String),
    /// No random number came for a new ID.
    #[error("no new ID: {0}")]
    Entropy(String),
    /// What a duplicate or a removal was asked of isn't an entry of one of the zone's lists, by its key.
    #[error("`{0}` isn't an entry of one of the zone's lists, by its ID or name")]
    NotAnEntry(String),
}

/// Where something a zone places stands, how it's turned, and the colliders it names.
#[derive(Clone, Debug, PartialEq)]
struct Placed {
    at: [f32; 3],
    turn: f32,
    rotation: Option<[f32; 3]>,
    colliders: Vec<String>,
}

impl Placed {
    /// Its yaw in degrees, if it's turned about +Y alone.
    fn yaw(&self) -> Option<f32> {
        match self.rotation {
            None => Some(self.turn),
            Some([pitch, yaw, roll]) if pitch == 0.0 && roll == 0.0 => Some(yaw),
            Some(_) => None,
        }
    }
}

/// Each placement and instance, by its list and its place in it.
fn placed(zone: &ZoneDocument) -> BTreeMap<(&'static str, usize), Placed> {
    let placements = zone.placements.iter().enumerate().map(|(index, placement)| {
        let placed = Placed {
            at: placement.at,
            turn: placement.turn,
            rotation: placement.rotation,
            colliders: placement.colliders.clone(),
        };
        (("placements", index), placed)
    });
    let instances = zone.instances.iter().enumerate().map(|(index, instance)| {
        let placed = Placed {
            at: instance.at,
            turn: instance.turn,
            rotation: instance.rotation,
            colliders: instance.colliders.clone(),
        };
        (("instances", index), placed)
    });
    placements.chain(instances).collect()
}

/// How a collider goes with what names it: from where that stood to where it stands, turned by `turn`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Carry {
    from: [f32; 3],
    to: [f32; 3],
    turn: Option<Turn>,
}

/// A turn a collider takes with what names it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Turn {
    by: Quat,
    /// In degrees, when what names it is turned about +Y alone before and after: how a collider's rotation about
    /// +Y says it.
    yaw: Option<f32>,
}

impl Carry {
    /// How `was` going to `now` carries what it names, if it moved or turned.
    fn of(was: &Placed, now: &Placed) -> Option<Self> {
        let turned = was.turn != now.turn || was.rotation != now.rotation;
        if now.at == was.at && !turned {
            return None;
        }
        let turn = turned.then(|| match (was.yaw(), now.yaw()) {
            (Some(from), Some(to)) => {
                let yaw = to - from;
                Turn { by: det::quat_from_axis_angle(Vec3::Y, yaw.to_radians()), yaw: Some(yaw) }
            }
            _ => {
                let (from, to) = (orientation(was.turn, was.rotation), orientation(now.turn, now.rotation));
                Turn { by: det::quat_normalize(det::quat_mul(to, det::quat_conjugate(from))), yaw: None }
            }
        });
        Some(Self { from: was.at, to: now.at, turn })
    }

    /// Where a collider at `position` goes.
    fn position(&self, position: [f32; 3]) -> [f32; 3] {
        match self.turn {
            // In f64, so a collider standing where its placement does lands where it does, to the bit.
            None => std::array::from_fn(|axis| {
                (f64::from(position[axis]) + f64::from(self.to[axis]) - f64::from(self.from[axis])) as f32
            }),
            Some(turn) => {
                let offset = Vec3::from(position) - Vec3::from(self.from);
                (Vec3::from(self.to) + det::quat_rotate(turn.by, offset)).to_array()
            }
        }
    }

    /// How a collider turned by `rotation` is turned after, if this turns it: about +Y in degrees where both
    /// turns are, as a person writes one, and otherwise as a quaternion.
    fn rotation(&self, rotation: Rotation) -> Option<Rotation> {
        let turn = self.turn?;
        let about_y = |axis: [f32; 3]| axis[0] == 0.0 && axis[2] == 0.0 && axis[1] != 0.0;
        Some(match (turn.yaw, rotation) {
            (Some(yaw), Rotation::None) => {
                Rotation::AxisAngle { axis: [0.0, 1.0, 0.0], degrees: wrapped(yaw) }
            }
            // About -Y, a turn about +Y takes its degrees down.
            (Some(yaw), Rotation::AxisAngle { axis, degrees }) if about_y(axis) => {
                Rotation::AxisAngle { axis, degrees: wrapped(degrees + yaw * axis[1].signum()) }
            }
            (_, rotation) => {
                let was = match rotation {
                    Rotation::None => Quat::IDENTITY,
                    Rotation::AxisAngle { axis, degrees } => {
                        det::quat_from_axis_angle(Vec3::from(axis).normalize_or_zero(), degrees.to_radians())
                    }
                    Rotation::Quaternion(q) => Quat::from_array(q),
                };
                Rotation::Quaternion(det::quat_normalize(det::quat_mul(turn.by, was)).to_array())
            }
        })
    }

    /// Whether `other` takes a collider where this does.
    fn agrees(&self, other: &Self) -> bool {
        self.position([0.0; 3]) == other.position([0.0; 3])
            && self.turn.map(|turn| turn.by) == other.turn.map(|turn| turn.by)
    }
}

/// `degrees` into (-180, 180], as a collider's turn is written.
fn wrapped(degrees: f32) -> f32 {
    let wrapped = (degrees + 180.0).rem_euclid(360.0) - 180.0;
    if wrapped == -180.0 { 180.0 } else { wrapped }
}

/// The zone's placements and instances and its colliders as they were when a gesture began (M6 T9b): what a drag
/// writes each frame's colliders from, so where they land depends on where the drag ends, never on how many
/// frames it took or the way the pointer went. Made by [`hold`], given to [`edit_with_colliders_from`].
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    placed: BTreeMap<(&'static str, usize), Placed>,
    entries: BTreeMap<String, ([f32; 3], Rotation)>,
}

/// The zone `zone` and its colliders as they are now, for a gesture to write from ([`Held`]); its colliders'
/// document opened if it isn't. None if the zone doesn't read.
pub fn hold(set: &mut DocumentSet, zone: AssetId) -> Option<Held> {
    let value = set.get::<ZoneDocument>(zone)?.value()?;
    let (placed, colliders) = (placed(value), value.colliders);
    let mut entries = BTreeMap::new();
    if let Some(colliders) = colliders
        && let Ok(id) = open_colliders(set, colliders)
        && let Some(document) = set.get::<ColliderDocument>(id).and_then(Document::value)
    {
        for entry in &document.colliders {
            entries.insert(entry.name.clone(), (entry.position, entry.rotation));
        }
    }
    Some(Held { placed, entries })
}

/// How many placements and instances a zone has.
const fn lengths(zone: &ZoneDocument) -> (usize, usize) {
    (zone.placements.len(), zone.instances.len())
}

/// How many entries each list with IDs has, in [`IdList::ALL`]'s order.
fn id_lengths(zone: &ZoneDocument) -> [usize; 4] {
    [zone.placements.len(), zone.instances.len(), zone.lights.len(), zone.volumes.len()]
}

/// What an edit of a version 1 zone (`by_places`) was refused for: an entry of a list with IDs added or taken
/// out before its last, moved, or given an ID reads back with its place's ID, which isn't what the edit made,
/// so that's the reason given, by what the edit was.
fn shifted_places(refused: Refused, by_places: bool) -> ZoneEditError {
    let shift = match &refused {
        Refused::Disagrees { path } if by_places => path.parse::<PropertyPath>().ok().and_then(|path| {
            let list = match path.steps().first() {
                Some(Step::Field(field)) => IdList::of_field(field)?,
                _ => return None,
            };
            let what = match path.steps() {
                [.., Step::Field(last)] if last == "id" => "sets an ID in",
                _ => "adds to, takes from or reorders",
            };
            Some(ZoneEditError::PlacesShift { list: list.field(), what })
        }),
        _ => None,
    };
    shift.unwrap_or(ZoneEditError::Refused(refused))
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
    edit_with_colliders_from(set, zone, label, None, edit)
}

/// [`edit_with_colliders`], each collider of a placement or instance the edit moves written from `held`, where
/// the gesture began, rather than from where the last frame left it (M6 T9b): a frame of a drag.
pub fn edit_with_colliders_from(
    set: &mut DocumentSet,
    zone: AssetId,
    label: &str,
    held: Option<&Held>,
    edit: impl FnOnce(&mut dyn AnyDocument) -> Result<Outcome, Refused>,
) -> Result<Outcome, ZoneEditError> {
    let opened = set.get::<ZoneDocument>(zone).ok_or(ZoneEditError::NotAZone(zone))?;
    let by_places = opened.written_version() == Some(1);
    let before = opened.value().map(|zone| (placed(zone), lengths(zone), id_lengths(zone)));
    set.together(label, |set| {
        let document = set.edit(zone).ok_or(ZoneEditError::NotAZone(zone))?;
        let outcome = edit(document).map_err(|refused| shifted_places(refused, by_places))?;
        let Some((before, was_long, had)) = before else { return Ok(outcome) };
        let Some(value) = set.get::<ZoneDocument>(zone).and_then(|document| document.value()) else {
            return Ok(outcome);
        };
        let shifted = IdList::ALL
            .into_iter()
            .zip(id_lengths(value).into_iter().zip(had))
            .find(|(_, (now, was))| now != was);
        if let (true, Some((list, _))) = (by_places, shifted) {
            return Err(ZoneEditError::PlacesShift { list: list.field(), what: "adds to or takes from" });
        }
        // A placement added or taken out shifts the rest, so one place before and after needn't be one thing.
        if lengths(value) != was_long {
            return Ok(outcome);
        }
        let (after, colliders) = (placed(value), value.colliders);
        let mut moves: BTreeMap<String, Carry> = BTreeMap::new();
        for (place, was) in &before {
            let Some(now) = after.get(place) else { continue };
            if now.colliders != was.colliders {
                continue;
            }
            // From where the gesture began, if it's held there, so a drag back to the start puts them back.
            let start =
                held.and_then(|held| held.placed.get(place)).filter(|start| start.colliders == now.colliders);
            let carry = match (start, Carry::of(was, now)) {
                (_, None) => continue,
                (Some(start), Some(_)) => {
                    Carry::of(start, now).unwrap_or(Carry { from: now.at, to: now.at, turn: None })
                }
                (None, Some(carry)) => carry,
            };
            for name in &now.colliders {
                if moves.insert(name.clone(), carry).is_some_and(|other| !other.agrees(&carry)) {
                    return Err(ZoneEditError::MovedApart(name.clone()));
                }
            }
        }
        let Some(colliders) = colliders.filter(|_| !moves.is_empty()) else { return Ok(outcome) };
        let id = open_colliders(set, colliders)?;
        let document = set.edit(id).ok_or(ZoneEditError::NotAZone(id))?;
        for (name, carry) in moves {
            let Some((position, rotation)) = entry_of(&*document, &name) else { continue };
            let started = held.and_then(|held| held.entries.get(&name).copied());
            let (from, turned) = match started {
                Some((from, was)) => (from, Some(carry.rotation(was).unwrap_or(was))),
                None => (position, carry.rotation(rotation)),
            };
            let moved = carry.position(from);
            // A number at a time, as a person edits one: the bytes of the others stay as they were.
            for axis in (0..3).filter(|axis| moved[*axis].to_bits() != position[*axis].to_bits()) {
                let text = write::canonical(&moved[axis], document.registry()).map_err(Refused::from)?;
                document.set_ron(label, &collider_field(&name, "position").join(Step::Index(axis)), &text)?;
            }
            if let Some(turned) = turned.filter(|turned| *turned != rotation) {
                let text = write::canonical(&turned, document.registry()).map_err(Refused::from)?;
                document.set_ron(label, &collider_field(&name, "rotation"), &text)?;
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

/// The path of the collider `name`.
fn collider_path(name: &str) -> PropertyPath {
    PropertyPath::root().join(Step::Field("colliders".to_owned())).join(Step::Key(name.to_owned()))
}

/// A field of the collider `name`.
fn collider_field(name: &str, field: &str) -> PropertyPath {
    collider_path(name).join(Step::Field(field.to_owned()))
}

/// Where the collider `name` stands and how it's turned, if the document has one of that name.
fn entry_of(document: &dyn AnyDocument, name: &str) -> Option<([f32; 3], Rotation)> {
    let colliders: &ColliderDocument =
        document.as_any().downcast_ref::<Document<ColliderDocument>>()?.value()?;
    let mut named = colliders.colliders.iter().filter(|entry| entry.name == name);
    let entry = named.next()?;
    named.next().is_none().then_some((entry.position, entry.rotation))
}

/// The zone `zone`'s colliders' document, opened if it isn't, if the zone names one: what a ray for the ground
/// meets (M6 T9b), as the editor has it now.
pub fn colliders_of(set: &mut DocumentSet, zone: AssetId) -> Result<Option<AssetId>, ZoneEditError> {
    let opened = set.get::<ZoneDocument>(zone).ok_or(ZoneEditError::NotAZone(zone))?;
    let Some(colliders) = opened.value().and_then(|value| value.colliders) else { return Ok(None) };
    Ok(Some(open_colliders(set, colliders)?))
}

/// What [`duplicate`] made: each copy's path, in the order asked, and the IDs it gave them, in that order, for a
/// recording to make the same copies again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Duplicated {
    pub paths: Vec<PropertyPath>,
    pub ids: Vec<String>,
}

/// Copies each entry of the zone `zone` at `entries`, keyed paths of its lists' entries (`placements["3fa91c07"]`,
/// `spawners["gate"]`, `weather_plan?.regions["rain"]`), to the end of its list, as one edit (M6 T9b): the copy of
/// an entry of a list with IDs takes a fresh one (the next of `ids`, or one drawn at random once those run out),
/// and one with a name a fresh name (`gate-2`, the first number free). A copy of a placement or an instance names
/// copies of its colliders, each with a fresh name and standing where its original does, so moving the copy takes
/// its own. Refused for a path that isn't an entry, an ID of `ids` the zone has, and, in a version 1 zone, an
/// entry of a list with IDs.
pub fn duplicate(
    set: &mut DocumentSet,
    zone: AssetId,
    label: &str,
    entries: &[PropertyPath],
    ids: &[String],
) -> Result<Duplicated, ZoneEditError> {
    let opened = set.get::<ZoneDocument>(zone).ok_or(ZoneEditError::NotAZone(zone))?;
    let by_places = opened.written_version() == Some(1);
    let colliders = opened.value().and_then(|value| value.colliders);
    set.together(label, |set| {
        let mut made = Duplicated::default();
        let mut ids = ids.iter();
        let mut copies: Vec<ColliderEntry> = Vec::new();
        for path in entries {
            let list = list_of(path)?;
            let with_ids = match list.steps() {
                [Step::Field(field)] => IdList::of_field(field),
                _ => None,
            };
            if let (true, Some(with_ids)) = (by_places, with_ids) {
                return Err(ZoneEditError::PlacesShift { list: with_ids.field(), what: "adds to" });
            }
            let document = zone_document(set, zone)?;
            let value =
                document.value().ok_or_else(|| Refused::ReadOnly("the zone doesn't read".to_owned()))?;
            let registry = document.registry();
            let entry = walk::get(value, path).map_err(Refused::from)?;
            let mut copy = concrete(entry, registry).ok_or_else(|| not_an_entry(path))?;
            let siblings = walk::get(value, &list).map_err(Refused::from)?;
            let at = match siblings.reflect_ref() {
                ReflectRef::List(items) => items.len(),
                _ => return Err(not_an_entry(path)),
            };
            let mut key = None;
            if with_ids.is_some() {
                let id = match ids.next() {
                    Some(id) if value.has_id(id) => return Err(ZoneEditError::Taken(id.clone())),
                    Some(id) => id.clone(),
                    None => new_id(value)?,
                };
                assign_field(copy.as_mut(), "id", &id, registry)?;
                made.ids.push(id.clone());
                key = Some(id);
            }
            if let Some(name) = text_field(copy.as_partial_reflect(), "name") {
                let fresh = fresh_name(&name, &names_in(siblings, "name"));
                assign_field(copy.as_mut(), "name", &fresh, registry)?;
                key.get_or_insert(fresh);
            }
            let key = key.ok_or_else(|| not_an_entry(path))?;
            let named: Vec<String> = field_of(copy.as_partial_reflect(), "colliders")
                .and_then(|field| field.try_downcast_ref::<Vec<String>>().cloned())
                .unwrap_or_default();
            if let (Some(colliders), false) = (colliders, named.is_empty()) {
                let renamed = copy_colliders(set, zone, colliders, &named, &mut copies)?;
                let document = zone_document(set, zone)?;
                assign_field(copy.as_mut(), "colliders", &renamed, document.registry())?;
            }
            zone_document(set, zone)?.insert(label, &list, at, copy.as_partial_reflect())?;
            made.paths.push(list.join(Step::Key(key)));
        }
        if let (Some(colliders), false) = (colliders, copies.is_empty()) {
            let id = open_colliders(set, colliders)?;
            let document = colliders_document(set, id)?;
            let list = PropertyPath::root().join(Step::Field("colliders".to_owned()));
            for entry in copies {
                let at = document.value().map_or(0, |value| value.colliders.len());
                document.insert(label, &list, at, &entry)?;
            }
            set.bind_gesture();
        }
        Ok(made)
    })
}

/// Copies of the colliders `named` in the zone's colliders' document `colliders`, each with a name none has, put
/// in `copies`: the names a copy of what names them names, in their order. A name the document doesn't have stays
/// as it is, the cook's to name.
fn copy_colliders(
    set: &mut DocumentSet,
    zone: AssetId,
    colliders: AssetId,
    named: &[String],
    copies: &mut Vec<ColliderEntry>,
) -> Result<Vec<String>, ZoneEditError> {
    // A name some placement or instance claims is taken, even where the document hasn't it.
    let claimed: Vec<String> = set
        .get::<ZoneDocument>(zone)
        .and_then(Document::value)
        .map(|zone| {
            let placements = zone.placements.iter().flat_map(|placement| placement.colliders.iter());
            placements
                .chain(zone.instances.iter().flat_map(|instance| instance.colliders.iter()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let id = open_colliders(set, colliders)?;
    let document = set.get::<ColliderDocument>(id).ok_or(ZoneEditError::NotAZone(id))?;
    let value =
        document.value().ok_or_else(|| Refused::ReadOnly("the zone's colliders don't read".to_owned()))?;
    let mut taken: Vec<String> = value.colliders.iter().map(|entry| entry.name.clone()).collect();
    taken.extend(copies.iter().map(|entry| entry.name.clone()));
    taken.extend(claimed);
    let mut renamed = Vec::with_capacity(named.len());
    for name in named {
        let Some(entry) = value.colliders.iter().find(|entry| entry.name == *name) else {
            renamed.push(name.clone());
            continue;
        };
        let fresh = fresh_name(name, &taken);
        taken.push(fresh.clone());
        copies.push(ColliderEntry { name: fresh.clone(), ..entry.clone() });
        renamed.push(fresh);
    }
    Ok(renamed)
}

/// Takes out each entry of the zone `zone` at `entries`, keyed paths as [`duplicate`] takes, as one edit (M6
/// T9b), and each collider that only they named, so the colliders' document holds nothing for what's gone.
/// Refused for a path that isn't an entry, and, in a version 1 zone, an entry of a list with IDs.
pub fn remove(
    set: &mut DocumentSet,
    zone: AssetId,
    label: &str,
    entries: &[PropertyPath],
) -> Result<(), ZoneEditError> {
    let opened = set.get::<ZoneDocument>(zone).ok_or(ZoneEditError::NotAZone(zone))?;
    let by_places = opened.written_version() == Some(1);
    let colliders = opened.value().and_then(|value| value.colliders);
    set.together(label, |set| {
        let mut named: Vec<String> = Vec::new();
        for path in entries {
            let list = list_of(path)?;
            if let ([Step::Field(field)], true) = (list.steps(), by_places)
                && let Some(with_ids) = IdList::of_field(field)
            {
                return Err(ZoneEditError::PlacesShift { list: with_ids.field(), what: "takes from" });
            }
            let document = zone_document(set, zone)?;
            let value =
                document.value().ok_or_else(|| Refused::ReadOnly("the zone doesn't read".to_owned()))?;
            let entry = walk::get(value, path).map_err(Refused::from)?;
            if let Some(names) =
                field_of(entry, "colliders").and_then(|field| field.try_downcast_ref::<Vec<String>>())
            {
                named.extend(names.iter().cloned());
            }
            document.remove(label, path)?;
        }
        let Some(colliders) = colliders.filter(|_| !named.is_empty()) else { return Ok(()) };
        // What something that stays still names, stays.
        let left = zone_document(set, zone)?.value().cloned();
        let still_named = |name: &String| {
            left.as_ref().is_some_and(|zone| {
                zone.placements.iter().any(|placement| placement.colliders.contains(name))
                    || zone.instances.iter().any(|instance| instance.colliders.contains(name))
            })
        };
        named.sort_unstable();
        named.dedup();
        let id = open_colliders(set, colliders)?;
        let document = colliders_document(set, id)?;
        let mut took = false;
        for name in named.iter().filter(|name| !still_named(name)) {
            if entry_of(&*document, name).is_some() {
                document.remove(label, &collider_path(name))?;
                took = true;
            }
        }
        if took {
            set.bind_gesture();
        }
        Ok(())
    })
}

/// The list `path`, an entry of it by its key, is in.
fn list_of(path: &PropertyPath) -> Result<PropertyPath, ZoneEditError> {
    match path.split_last() {
        Some((list, Step::Key(_))) => Ok(list),
        _ => Err(not_an_entry(path)),
    }
}

fn not_an_entry(path: &PropertyPath) -> ZoneEditError {
    ZoneEditError::NotAnEntry(path.to_string())
}

/// The zone `zone`, to edit as part of the set's step.
fn zone_document(set: &mut DocumentSet, zone: AssetId) -> Result<&mut Document<ZoneDocument>, ZoneEditError> {
    set.edit(zone)
        .and_then(|open| open.as_any_mut().downcast_mut::<Document<ZoneDocument>>())
        .ok_or(ZoneEditError::NotAZone(zone))
}

/// The colliders' document `id`, to edit as part of the set's step.
fn colliders_document(
    set: &mut DocumentSet,
    id: AssetId,
) -> Result<&mut Document<ColliderDocument>, ZoneEditError> {
    set.edit(id)
        .and_then(|open| open.as_any_mut().downcast_mut::<Document<ColliderDocument>>())
        .ok_or(ZoneEditError::NotAZone(id))
}

/// `entry` as a value of its own type, every field of it: what a copy is made from, whole, so a type whose
/// conversion needs every field converts.
fn concrete(entry: &dyn PartialReflect, registry: &TypeRegistry) -> Option<Box<dyn Reflect>> {
    let info = entry.get_represented_type_info()?;
    registry.get_type_data::<ReflectFromReflect>(info.type_id())?.from_reflect(entry)
}

/// The field `name` of a struct.
fn field_of<'v>(entry: &'v dyn PartialReflect, name: &str) -> Option<&'v dyn PartialReflect> {
    match entry.reflect_ref() {
        ReflectRef::Struct(fields) => fields.field(name),
        _ => None,
    }
}

/// A struct's text field `name`.
fn text_field(entry: &dyn PartialReflect, name: &str) -> Option<String> {
    field_of(entry, name)?.try_downcast_ref::<String>().cloned()
}

/// Puts `value` in the field `name` of `entry`.
fn assign_field(
    entry: &mut dyn Reflect,
    name: &str,
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<(), ZoneEditError> {
    let path = PropertyPath::root().join(Step::Field(name.to_owned()));
    let slot = walk::get_mut(entry.as_partial_reflect_mut(), &path).map_err(Refused::from)?;
    walk::assign(slot, value, registry).map_err(|why| Refused::Document(why).into())
}

/// The text field `name` of each entry of `list`.
fn names_in(list: &dyn PartialReflect, name: &str) -> Vec<String> {
    match list.reflect_ref() {
        ReflectRef::List(items) => items.iter().filter_map(|item| text_field(item, name)).collect(),
        _ => Vec::new(),
    }
}

/// `name` with the first number from 2 after it that makes it none of `taken`: `gate-2`, `gate-3`. A name that
/// ends in such a number already counts on from it.
fn fresh_name(name: &str, taken: &[String]) -> String {
    let (stem, from) = match name.rsplit_once('-').map(|(stem, number)| (stem, number.parse::<u32>())) {
        Some((stem, Ok(number))) if !stem.is_empty() => (stem, number.saturating_add(1)),
        _ => (name, 2),
    };
    (from..=u32::MAX)
        .map(|number| format!("{stem}-{number}"))
        .find(|fresh| !taken.iter().any(|taken| taken == fresh))
        .unwrap_or_else(|| format!("{name}-copy"))
}

/// The schema version a document's text says it's written at, if it says.
#[must_use]
pub fn version_of(source: &str) -> Option<u32> {
    let tree = text::scan(source).ok()?;
    tree.field("version").filter(|_| tree.field("type").is_some())?.text(source).parse().ok()
}

/// An ID no entry of `zone` has: eight hexadecimal digits, drawn at random, and again while one has it (the
/// `sapphire_zone::ids` module).
pub fn new_id(zone: &ZoneDocument) -> Result<String, ZoneEditError> {
    loop {
        let mut bytes = [0_u8; 4];
        getrandom::fill(&mut bytes).map_err(|error| ZoneEditError::Entropy(error.to_string()))?;
        let id = format!("{:08x}", u32::from_le_bytes(bytes));
        if !zone.has_id(&id) {
            return Ok(id);
        }
    }
}

/// Adds a new entry to the end of `list`, one of the zone's lists with IDs, called `id`, or by an ID of its own
/// ([`new_id`]) when that's `None`: what an inspector's "add" does there, and what a recorded one replays, with
/// the ID it drew. A placement, an instance or a light starts as its type's `Default` (at the origin, unscaled,
/// a placement naming no prefab yet); an atmosphere volume, which has no look to start from, as its type's blank
/// ([`crate::blank`]). What else it starts as is for a person to fill in, and the zone's checks may refuse it,
/// as they would any edit. Returns the entry's ID.
pub fn add_entry(
    document: &mut Document<ZoneDocument>,
    label: &str,
    list: IdList,
    id: Option<&str>,
) -> Result<(Outcome, String), ZoneEditError> {
    let zone = document.value().ok_or_else(|| Refused::ReadOnly("the zone doesn't read".to_owned()))?;
    let id = match id {
        Some(id) if zone.has_id(id) => return Err(ZoneEditError::Taken(id.to_owned())),
        Some(id) => id.to_owned(),
        None => new_id(zone)?,
    };
    let given = id.clone();
    let path = PropertyPath::root().join(Step::Field(list.field().to_owned()));
    let at = zone.ids().filter(|(of, _, _)| *of == list).count();
    let entry: Box<dyn PartialReflect> = match list {
        IdList::Placements => Box::new(Placement { id, ..Placement::default() }),
        IdList::Instances => Box::new(Instance { id, ..Instance::default() }),
        IdList::Lights => Box::new(Light { id, ..Light::default() }),
        IdList::Volumes => {
            let registry = document.registry();
            let info = registry
                .get_type_info(std::any::TypeId::of::<AtmosphereVolume>())
                .ok_or_else(|| Refused::Document(format!("`{path}` has entries of no registered type")))?;
            let mut entry =
                blank::blank(info, registry).map_err(|error| Refused::Document(error.to_string()))?;
            let field = PropertyPath::root().join(Step::Field("id".to_owned()));
            let slot = walk::get_mut(entry.as_mut(), &field).map_err(Refused::from)?;
            walk::assign(slot, &id, registry).map_err(Refused::Document)?;
            entry
        }
    };
    Ok((document.insert(label, &path, at, entry.as_ref())?, given))
}

/// Why a zone's text couldn't be migrated. Nothing's written.
#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error(transparent)]
    Zone(#[from] ZoneError),
    #[error("the zone can't be migrated: {0}")]
    Text(String),
}

/// `source`, a zone's text at version 1, at version 2: each placement, instance, light and atmosphere volume
/// given the ID it reads with (its place, `placement-3`) as its first field, in the file's layout, and the
/// version raised, every other byte as it was. `None` for a zone already at version 2. What it writes reads as
/// the very document `source` does, or it's refused.
pub fn migrate(source: &str) -> Result<Option<String>, MigrateError> {
    let zone = ZoneDocument::from_ron(source)?;
    match version_of(source) {
        Some(ZoneDocument::VERSION) => return Ok(None),
        Some(1) => {}
        found => return Err(MigrateError::Text(format!("it says version {found:?}, and only 1 migrates"))),
    }
    let problem = |what: &str| MigrateError::Text(what.to_owned());
    let tree = text::scan(source).map_err(|error| problem(error.what))?;
    let version = tree.field("version").ok_or_else(|| problem("it names no version"))?;
    let mut splices = vec![Splice { range: version.span.clone(), text: ZoneDocument::VERSION.to_string() }];
    let body = tree.field("data").ok_or_else(|| problem("it has no data"))?;
    for list in IdList::ALL {
        let Some(entries) = body.field(list.field()) else { continue };
        for (index, entry) in entries.items().iter().enumerate() {
            let id = zone
                .id_at(list, index)
                .ok_or_else(|| problem("an entry the scan has and the value hasn't"))?;
            let quoted = format!("{id:?}");
            if let Some(written) = entry.field("id") {
                // A version 1 text that wrote one read with its place all the same, and now says so.
                splices.push(Splice { range: written.span.clone(), text: quoted });
                continue;
            }
            let inside = layout::inside(source, entry).ok_or_else(|| problem("an entry with no brackets"))?;
            let field = format!("id: {quoted}");
            splices.push(match entry.fields().first() {
                Some(first) => layout::insert_first(source, &inside, first.start, &field),
                None => layout::into_empty(source, inside, &field),
            });
        }
    }
    let migrated = layout::apply(source, &splices).ok_or_else(|| problem("its splices overlap"))?;
    if ZoneDocument::from_ron(&migrated)? != zone {
        return Err(problem("what it wrote doesn't read as what it read"));
    }
    Ok(Some(migrated))
}
