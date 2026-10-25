//! Importers: source formats → engine-neutral intermediates: an
//! [`ImportedMesh`](crate::mesh::ImportedMesh) for meshes, a [`SourceImage`](image::SourceImage)
//! for textures, and for VRM characters a mesh plus a
//! [`CharacterRig`](sapphire_assets::rig::CharacterRig).
//!
//! Importers only translate what the source authored (geometry, materials, skins, morph
//! targets and outline normals in one object space; pixels; a VRM's humanoid, expressions,
//! spring bones and constraints). Derived data is later stages' job: outline normals in
//! [`bake`](crate::bake), vertex order and LODs in [`optimize`](crate::optimize), mip chains in
//! the texture cook ([`texture`](crate::texture)). Each importer has a
//! [`Stage`](crate::cache::Stage) whose version is part of the content cache key, and lists the
//! other files it reads, which are too. FBX via `ufbx` joins glTF later.

pub mod gltf;
pub mod image;
pub mod vrm;
