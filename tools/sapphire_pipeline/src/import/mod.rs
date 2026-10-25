//! Importers: source formats → engine-neutral intermediates: an
//! [`ImportedMesh`](crate::mesh::ImportedMesh) for meshes, a [`SourceImage`](image::SourceImage)
//! for textures.
//!
//! Importers only translate what the source authored (geometry, materials, skins, morph
//! targets and outline normals in one object space; pixels). Derived data is later stages'
//! job: outline normals in [`bake`](crate::bake), vertex order and LODs in
//! [`optimize`](crate::optimize), mip chains in the texture cook ([`texture`](crate::texture)).
//! Each importer has a [`Stage`](crate::cache::Stage) whose version is part of the content
//! cache key, and lists the other files it reads, which are too. FBX via `ufbx` joins glTF
//! later.

pub mod gltf;
pub mod image;
