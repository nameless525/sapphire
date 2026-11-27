//! A zone's colliders with its trees' trunks joined (M6 G1b): what the simulation reads of its foliage.
//!
//! A zone whose foliage document has a rule placing a kind with a [`Trunk`](sapphire_zone::foliage::Trunk)
//! makes a `.scol` of its own, a sub-asset (`grove.trunks.scol`): the collider set its document names, every
//! collider as it was, and one capsule a tree its scatter sets hold, in the `World` group, named by the tree's
//! stable ID ([`sapphire_zone::foliage::trunk_name`]) and sorted with the rest by name, as every collider set
//! is. The `.szone`'s `simulation.colliders` names it in place of the document's, so the physics world, the
//! digest and the navmesh bake ([`crate::nav`]) all read it, and nothing that loads a zone's colliders needs to
//! know trees exist. It does so even if no tree of the kind stands, moving the digest. A zone whose foliage
//! places no kind with a trunk makes none and names its document's, keeping its digest; the document's set
//! ships with its zone through the trunks ([`crate::cook::CookedAsset::ships_with`]).
//!
//! What stands on what: the scatter sets stand on the document's colliders, never on this set, so a tree's
//! roots don't rest on another's trunk; the navmesh bakes from this one, so it routes round every trunk.
//!
//! # The same bytes everywhere
//!
//! A trunk's capsule is [`Trunk::capsule`](sapphire_zone::foliage::Trunk::capsule) of its instance: its root's
//! three `f32`s from the set, its scale from the instance's byte through `det::exp2`, and the kind's two numbers,
//! multiplied, subtracted and halved. Its name is the ID in hex. The order is a sort by name. So the bytes are the
//! scatter sets' bytes and the document's numbers on every machine, and the sets are already the same on all.
//!
//! # Keys
//!
//! Its key holds the document's colliders, every scatter set's bytes and the foliage document, and the cells it
//! joins ([`super::ZonePlan::foliage_recipe`]): a moved collider, a saved foliage document or a set that
//! cooked to other bytes joins it again; a save to a light, or a set cooked again to the same bytes, leaves it
//! alone, and so the navmesh baked from it, which holds its bytes too.

use sapphire_colliders::document::MAX_COLLIDERS;
use sapphire_colliders::{ColliderSet, CookedCollider, CookedShape, Group};
use sapphire_zone::FoliageDocument;
use sapphire_zone::foliage::{ScatterSet, trunk_name};

use super::ZoneCookError;
use crate::cache::Stage;

/// The joined `.scol`'s writer. Bump the version whenever it changes output for the same colliders, sets and
/// document, as a change to [`Trunk::capsule`](sapphire_zone::foliage::Trunk::capsule) does.
pub const TRUNKS_COOKER: Stage = Stage { name: "trunk colliders", version: 1 };

/// The part a zone's joined colliders are among its sub-assets. Never changed: its ID is derived from it.
pub const TRUNKS_PART: &str = "trunk colliders";

/// The cooked collider set `colliders` with a capsule for each tree of `foliage`'s kinds with a trunk in the
/// cooked scatter `sets`, read back through the runtime's check (see the [module docs](self)).
pub fn join(colliders: &[u8], foliage: &FoliageDocument, sets: &[&[u8]]) -> Result<Vec<u8>, ZoneCookError> {
    let mut set = ColliderSet::from_cooked(colliders)
        .map_err(|error| ZoneCookError::invalid("colliders", format!("they don't read: {error}")))?;
    let trunks: Vec<_> = foliage.kinds.iter().map(|kind| kind.trunk).collect();
    for bytes in sets {
        let scatter = ScatterSet::read(bytes)?;
        for run in scatter.runs.iter() {
            let Some(Some(trunk)) = trunks.get(run.kind.to_native() as usize) else { continue };
            let first = run.first_chunk.to_native() as usize;
            let chunks = &scatter.chunks[first..first + run.chunk_count.to_native() as usize];
            let (start, end) = match (chunks.first(), chunks.last()) {
                (Some(first), Some(last)) => (
                    first.first.to_native() as usize,
                    (last.first.to_native() + last.count.to_native()) as usize,
                ),
                _ => continue,
            };
            for index in start..end {
                let capsule = trunk.capsule(scatter.instance(index));
                set.colliders.push(CookedCollider {
                    name: trunk_name(scatter.ids[index].to_native()),
                    shape: CookedShape::Capsule { half_height: capsule.half_height, radius: capsule.radius },
                    position: capsule.centre,
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    memberships: Group::World.mask().bits(),
                });
            }
        }
    }
    if set.colliders.len() > MAX_COLLIDERS {
        let problem = format!(
            "with its trees' trunks they're {} colliders, more than the {MAX_COLLIDERS} a set may hold",
            set.colliders.len()
        );
        return Err(ZoneCookError::invalid("foliage", problem));
    }
    set.colliders.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    if let Some(pair) = set.colliders.windows(2).find(|pair| pair[0].name == pair[1].name) {
        let problem = format!("a collider is named `{}`, as a tree's trunk is", pair[0].name);
        return Err(ZoneCookError::invalid("colliders", problem));
    }
    let cooked = set.to_cooked().map_err(|error| ZoneCookError::invalid("colliders", error.to_string()))?;
    ColliderSet::read(&cooked)
        .map_err(|error| ZoneCookError::invalid("foliage", format!("a trunk: {error}")))?;
    Ok(cooked.into_vec())
}

#[cfg(test)]
mod tests {
    use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderShape, Rotation};
    use sapphire_serialize::AssetId;
    use sapphire_zone::ScatterInstance;
    use sapphire_zone::foliage::{FoliageKind, FoliageModel, Placed, Trunk};

    use super::*;

    fn floor() -> Vec<u8> {
        let entry = ColliderEntry {
            name: "floor".to_owned(),
            shape: ColliderShape::Box { half_extents: [10.0, 0.5, 10.0] },
            position: [0.0, -0.5, 0.0],
            rotation: Rotation::default(),
            groups: vec![Group::World],
        };
        ColliderDocument { colliders: vec![entry] }.cook().unwrap().to_cooked().unwrap().into_vec()
    }

    fn kinds() -> FoliageDocument {
        let kind = |name: &str, model, trunk| FoliageKind { name: name.to_owned(), model, trunk };
        let mesh = AssetId::from_bytes([7; 16]);
        FoliageDocument {
            kinds: vec![
                kind("oak", FoliageModel::Tree(mesh), Some(Trunk { radius: 0.25, height: 3.0 })),
                kind("fern", FoliageModel::Cover(mesh), None),
            ],
            ..FoliageDocument::default()
        }
    }

    fn set(placed: Vec<Placed>) -> Vec<u8> {
        ScatterSet::from_placed([0, 0], placed).to_cooked().unwrap().into_vec()
    }

    #[test]
    fn a_trees_trunk_joins_the_colliders_and_nothing_else_does() {
        // An oak and a fern at about scale 1 (byte 128).
        let oak = ScatterInstance::new([2.0, 0.0, 3.0], 0, 128, 0, 0);
        let fern = ScatterInstance::new([4.0, 0.0, 3.0], 0, 128, 0, 0);
        let sets = [
            set(vec![
                Placed { kind: 0, id: 0xabc, instance: oak },
                Placed { kind: 1, id: 0xdef, instance: fern },
            ]),
            set(Vec::new()),
        ];
        let sets: Vec<&[u8]> = sets.iter().map(Vec::as_slice).collect();
        let joined = join(&floor(), &kinds(), &sets).unwrap();
        // Pinned, so the capsules' numbers and their names' order are the same bytes on every architecture CI
        // runs.
        let hex = blake3::hash(&joined).to_hex();
        assert_eq!(&hex[..16], "3e27ef99963d2a07", "if the join changed on purpose, pin this");
        let read = ColliderSet::from_cooked(&joined).unwrap();
        assert_eq!(read.colliders.len(), 2, "the floor and the oak's trunk, not the fern");
        let trunk = &read.colliders[read.colliders.iter().position(|c| c.name == trunk_name(0xabc)).unwrap()];
        let scale = oak.scale();
        let CookedShape::Capsule { half_height, radius } = trunk.shape else { panic!("{trunk:?}") };
        assert_eq!(radius, 0.25 * scale);
        assert_eq!(half_height, (3.0 * scale - radius) * 0.5);
        assert_eq!(trunk.position, [2.0, half_height, 3.0], "from its radius under the root to its height");
        assert_eq!(trunk.memberships, Group::World.mask().bits());
        let names: Vec<&str> = read.colliders.iter().map(|collider| collider.name.as_str()).collect();
        assert_eq!(names, ["floor", trunk_name(0xabc).as_str()], "sorted by name, so the IDs are the names'");
    }

    #[test]
    fn a_collider_named_as_a_trunk_is_refused() {
        let oak = ScatterInstance::new([2.0, 0.0, 3.0], 0, 128, 0, 0);
        let entry = |name: String| ColliderEntry {
            name,
            shape: ColliderShape::Box { half_extents: [1.0; 3] },
            position: [0.0; 3],
            rotation: Rotation::default(),
            groups: vec![Group::World],
        };
        let colliders =
            ColliderDocument { colliders: vec![entry(trunk_name(5))] }.cook().unwrap().to_cooked().unwrap();
        let sets = [set(vec![Placed { kind: 0, id: 5, instance: oak }])];
        let error = join(&colliders, &kinds(), &[&sets[0]]).unwrap_err().to_string();
        assert!(error.contains("as a tree's trunk is"), "{error}");
    }
}
