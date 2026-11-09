//! The real geometry the navigation bake's tests and example read.

#![allow(dead_code, reason = "each test and the example use their own part")]

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_colliders::{ColliderDocument, ColliderSet};
use sapphire_pipeline::assembly::{self, Detail};
use sapphire_pipeline::import::gltf;
use sapphire_pipeline::nav::{
    BakeSettings, Baked, IslandStats, Seed, Triangle, bake_colliders, colliders_to_triangles, solid_groups,
};
use sapphire_zone::ZoneDocument;

pub fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// The arena's collider set, as the simulation reads it.
pub fn arena_colliders() -> ColliderSet {
    let text = fs::read_to_string(content().join("arena/square.colliders.ron")).expect("the document");
    ColliderDocument::from_ron(&text).expect("it reads").cook().expect("it cooks")
}

/// The proving ground's colliders and its navmesh, as a cook bakes it from its zone document.
pub fn proving_ground() -> (ColliderSet, Baked, IslandStats) {
    let dir = content().join("zones/proving_ground");
    let read = |name: &str| fs::read_to_string(dir.join(name)).expect("the zone's file");
    let zone = ZoneDocument::from_ron(&read("proving_ground.zone.ron")).expect("the zone reads");
    let navigation = zone.navigation.as_ref().expect("it asks for a navmesh");
    let colliders = ColliderDocument::from_ron(&read("proving_ground.colliders.ron"))
        .expect("it reads")
        .cook()
        .expect("it cooks");
    let cooked = colliders.to_cooked().expect("it writes");
    // As the cook seeds it: spawners, then checkpoints.
    let spawners = zone.spawners.iter().map(|spawner| (&spawner.name, spawner.position));
    let checkpoints = zone.checkpoints.iter().map(|checkpoint| (&checkpoint.name, checkpoint.position));
    let seeds: Vec<Seed> =
        spawners.chain(checkpoints).map(|(name, position)| Seed { field: name.clone(), position }).collect();
    let area = (navigation.min, navigation.max);
    let (baked, islands) =
        bake_colliders(&cooked, area, &seeds, &BakeSettings::CHARACTER, None).expect("it bakes");
    (colliders, baked, islands)
}

/// What stops a character in the arena.
pub fn arena_triangles() -> Vec<Triangle> {
    colliders_to_triangles(&arena_colliders(), solid_groups())
}

/// The whole look-dev town, as the cook assembles it: every triangle anyone drew, roofs and all, with
/// nothing to say what a body would really meet. The larger case, for what a bake costs and how paths
/// run: no zone bakes from drawn triangles, whose low bits aren't the same on every platform.
pub fn town_triangles() -> Vec<Triangle> {
    let dir = content().join("lookdev/town");
    let bytes = fs::read(dir.join("town.assembly.ron")).expect("the town's document");
    let town = assembly::read(&bytes).expect("it reads");
    let mesh = assembly::assemble(&town, Detail::Full, &mut |file: &str| {
        let path = dir.join(file);
        let bytes = fs::read(&path).expect("the piece");
        Ok(gltf::import_slice(&bytes, path.parent()).expect("it imports"))
    })
    .expect("it assembles");
    let (corners, _) = mesh.indices.as_chunks::<3>();
    corners.iter().map(|corner| corner.map(|index| mesh.vertices[index as usize].position)).collect()
}

/// Each of the town's top-level placements, by name, with a digest of its triangles. A diff of two
/// platforms' lines says which pieces of the town come out with other low bits.
pub fn town_part_digests() -> Vec<(String, u32, blake3::Hash)> {
    let dir = content().join("lookdev/town");
    let path = dir.join("town.assembly.ron");
    let town = assembly::read(&fs::read(&path).expect("the town's document")).expect("it reads");
    let parts = assembly::import_parts(&path, &town, Detail::Full).expect("it imports");
    let (corners, _) = parts.mesh.indices.as_chunks::<3>();
    let mut hashes: Vec<(blake3::Hasher, u32)> =
        std::iter::repeat_with(|| (blake3::Hasher::new(), 0)).take(town.place.len()).collect();
    for (corner, owner) in corners.iter().zip(&parts.owners) {
        let (hash, count) = &mut hashes[*owner as usize];
        *count += 1;
        for index in corner {
            for value in parts.mesh.vertices[*index as usize].position {
                hash.update(&value.to_bits().to_le_bytes());
            }
        }
    }
    let names = town.place.iter().map(|entry| entry.name().to_owned());
    names.zip(hashes).map(|(name, (hash, count))| (name, count, hash.finalize())).collect()
}

/// A digest of triangles by their floats' bits, to tell input that moved from a bake that did.
pub fn triangles_digest(triangles: &[Triangle]) -> blake3::Hash {
    let mut hash = blake3::Hasher::new();
    for corner in triangles.iter().flatten() {
        for value in corner {
            hash.update(&value.to_bits().to_le_bytes());
        }
    }
    hash.finalize()
}
