//! The real geometry the navigation bake's tests and example read.

#![allow(dead_code, reason = "each test and the example use their own part")]

use std::fs;
use std::path::{Path, PathBuf};

use sapphire_colliders::{ColliderDocument, ColliderSet, Group};
use sapphire_pipeline::assembly::{self, Detail};
use sapphire_pipeline::import::gltf;
use sapphire_pipeline::nav::{Triangle, colliders_to_triangles, mesh_to_triangles};

pub fn content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
}

/// The arena's collider set, as the simulation reads it.
pub fn arena_colliders() -> ColliderSet {
    let text = fs::read_to_string(content().join("arena/square.colliders.ron")).expect("the document");
    ColliderDocument::from_ron(&text).expect("it reads").cook().expect("it cooks")
}

/// What stops a character in the arena.
pub fn arena_triangles() -> Vec<Triangle> {
    let solid = Group::World.mask().union(Group::CharacterBlocker.mask());
    colliders_to_triangles(&arena_colliders(), solid)
}

/// The whole look-dev town, as the cook assembles it: every triangle anyone drew, roofs and all, with
/// nothing to say what a body would really meet. The larger case.
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
    mesh_to_triangles(&mesh)
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
