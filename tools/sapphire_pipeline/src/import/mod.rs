//! Importers: source formats → engine-neutral [`MeshAsset`](sapphire_assets::mesh::MeshAsset).
//!
//! Importers only translate; derived data (outline normals, later LODs and vertex-cache
//! optimization) is the job of [`bake`](crate::bake). Each importer has a
//! [`Stage`](crate::cache::Stage) whose version is part of the content cache key, and lists
//! the other files it reads, which are too. FBX via `ufbx` joins glTF in M1.

pub mod gltf;
