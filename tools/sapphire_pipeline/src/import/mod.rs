//! Importers: source formats → engine-neutral intermediates: an
//! [`ImportedMesh`](crate::mesh::ImportedMesh) for meshes (glTF, FBX), a
//! [`SourceImage`](image::SourceImage) for textures, and for VRM characters a mesh plus a
//! [`CharacterRig`](sapphire_assets::rig::CharacterRig).
//!
//! Importers only translate what the source authored (geometry, materials, skins, morph
//! targets and outline normals in one object space; pixels; a VRM's humanoid, expressions,
//! spring bones and constraints). Derived data is later stages' job: outline normals in
//! [`bake`](crate::bake), vertex order and LODs in [`optimize`](crate::optimize), mip chains in
//! the texture cook ([`texture`](crate::texture)). Each importer has a
//! [`Stage`](crate::cache::Stage) whose version is part of the content cache key, and lists the
//! other files it reads, which are too. The mesh importers agree on what a mesh is: one object
//! space (+Y up, meters, facing +Z), a material slot per source material named the same way
//! (`slot_names`), the same generated material family, and the same skin and morph limits, so
//! a scene exported from Blender as glTF and as FBX imports to the same mesh.

pub mod fbx;
pub mod gltf;
pub mod image;
pub mod vrm;

/// The material slot of faces without a material: glTF's default material, or an FBX mesh with
/// none.
pub(crate) const DEFAULT_SLOT: &str = "#default";

/// The slot name of each of a source's materials, by index: its own name unless another material
/// shares it (`<name>#<index>`), `#<index>` for an unnamed one, and unique against each other and
/// [`DEFAULT_SLOT`].
pub(crate) fn slot_names(names: &[Option<&str>]) -> Vec<String> {
    let mut slots: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| match name {
            Some(name) if names.iter().filter(|other| *other == &Some(*name)).count() == 1 => {
                (*name).to_owned()
            }
            Some(name) => format!("{name}#{index}"),
            None => format!("#{index}"),
        })
        .collect();
    // A literal name can still match a made-up one (`Plain#1`, `#default`): suffix the clashing
    // names with their index until every name, and the default material's, is unique. Each pass
    // makes the clashing names longer and distinct from one another, so this ends.
    loop {
        let clashing: Vec<usize> = (0..slots.len())
            .filter(|&index| {
                slots[index] == DEFAULT_SLOT || slots.iter().filter(|name| **name == slots[index]).count() > 1
            })
            .collect();
        if clashing.is_empty() {
            return slots;
        }
        for index in clashing {
            slots[index] = format!("{}#{index}", slots[index]);
        }
    }
}
