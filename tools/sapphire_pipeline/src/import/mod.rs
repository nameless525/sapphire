//! Importers: source formats → engine-neutral [`MeshAsset`](sapphire_assets::mesh::MeshAsset).
//!
//! Importers only translate; derived data (outline normals, later LODs and vertex-cache
//! optimization) is the job of [`bake`](crate::bake). FBX via `ufbx` joins glTF in M1.

pub mod gltf;
