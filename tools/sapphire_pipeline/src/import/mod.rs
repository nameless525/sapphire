//! Importers: source formats → engine-neutral intermediates: [`MeshAsset`] for meshes,
//! [`SourceImage`](image::SourceImage) for textures.
//!
//! Importers only translate; derived data (outline normals, later LODs and vertex-cache
//! optimization) is the job of [`bake`](crate::bake), and mip chains are the texture cook's
//! ([`texture`](crate::texture)). Each importer has a [`Stage`](crate::cache::Stage) whose
//! version is part of the content cache key, and lists the other files it reads, which are
//! too. FBX via `ufbx` joins glTF in M1.
//!
//! [`MeshAsset`]: sapphire_assets::mesh::MeshAsset

pub mod gltf;
pub mod image;
