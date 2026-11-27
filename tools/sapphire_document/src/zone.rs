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
//!
//! **IDs (M6 T8a):** a zone's placements, instances, lights and atmosphere volumes each have an ID, which a
//! path calls the entry by (`placements["3fa91c07"]`), so an editor's selection names the same entry however
//! the lists shift. An entry added here takes a fresh one ([`new_id`]). A version 1 document's IDs are its
//! entries' places, which an entry added or taken out would shift under the selection, so an edit of one that
//! adds or takes out such an entry is refused until the document is migrated ([`migrate`]): its text written
//! at version 2 with the IDs it already reads with, comments and layout kept, and nothing else changed.

use std::collections::BTreeMap;
use std::path::Path;

use bevy_reflect::PartialReflect;
use sapphire_assets::AssetId;
use sapphire_colliders::ColliderDocument;
use sapphire_reflect::Versioned;
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
        document.as_any().downcast_ref::<Document<ColliderDocument>>()?.value()?;
    let mut named = colliders.colliders.iter().filter(|entry| entry.name == name);
    let entry = named.next()?;
    named.next().is_none().then_some(entry.position)
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
