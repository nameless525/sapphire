//! What kind of data an asset is. Packs record it in their table of contents, so the runtime
//! knows how to check, budget and upload an asset before reading it.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::material::{Material, MaterialError};
use crate::mesh::{Mesh, MeshError};
use crate::probes::{ProbeGrid, ProbeGridError};
use crate::rig::{CharacterRig, RigError};
use crate::texture::{Texture, TextureFormatError};

/// The kinds of cooked asset. Each is also a residency category with a budget of its own
/// ([`AssetBudgets`](crate::AssetBudgets)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum AssetKind {
    /// A cooked mesh (`.smesh`): [`Mesh`].
    Mesh = 1,
    /// A cooked texture (KTX2): [`Texture`].
    Texture = 2,
    /// A cooked material (`.smat`): [`Material`].
    Material = 3,
    /// A character rig (`.srig`, M1 A6): [`CharacterRig`]. Animation's CPU data, never
    /// uploaded.
    Rig = 4,
    /// An ambient probe grid (`.sprobe`, M1 W5): [`ProbeGrid`], uploaded to the renderer.
    ProbeGrid = 5,
}

impl AssetKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 5] = [Self::Mesh, Self::Texture, Self::Material, Self::Rig, Self::ProbeGrid];

    /// The kind's position in [`Self::ALL`].
    pub const fn index(self) -> usize {
        self as usize - 1
    }

    /// The name used in messages.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Texture => "texture",
            Self::Material => "material",
            Self::Rig => "rig",
            Self::ProbeGrid => "probe grid",
        }
    }

    /// Whether assets of this kind go to the GPU, through the asset server's upload queue. A rig
    /// is ready once it and its dependencies are decoded.
    pub const fn uploads(self) -> bool {
        !matches!(self, Self::Rig)
    }

    /// Whether the asset server keeps an asset's decoded bytes once it is ready. Meshes,
    /// textures and probe grids are GPU data, so their bytes go once uploaded (a new renderer
    /// has them read again). Materials are small and CPU code reads them (live link, the editor), and rigs are
    /// read on the CPU only, so they stay.
    pub const fn keeps_bytes(self) -> bool {
        matches!(self, Self::Material | Self::Rig)
    }

    /// Whether uploading an asset of this kind binds its dependencies' GPU objects, so it must be
    /// sent again when hot reload replaces one: a material binds its textures. A mesh names its
    /// materials by the renderer's handles, which a material reloaded in place keeps. Such kinds
    /// keep their bytes, which they are sent again from.
    pub const fn binds_dependencies(self) -> bool {
        matches!(self, Self::Material)
    }

    /// Whether an asset of this kind may depend on one of `kind`: meshes on materials, materials
    /// on textures, textures, rigs and probe grids on nothing. Every dependency is of a lower kind, so chains
    /// are at most three long and cycles can't exist, whatever a pack claims. A new kind takes
    /// its place in this order.
    pub const fn may_depend_on(self, kind: AssetKind) -> bool {
        matches!((self, kind), (Self::Mesh, Self::Material) | (Self::Material, Self::Texture))
    }

    /// The kind a pack stores as `value`, if any.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Mesh),
            2 => Some(Self::Texture),
            3 => Some(Self::Material),
            4 => Some(Self::Rig),
            5 => Some(Self::ProbeGrid),
            _ => None,
        }
    }

    /// Checks cooked bytes of this kind exactly as the runtime reads them (`Mesh::read`,
    /// `Texture::from_ktx2`, `Material::read`, `CharacterRig::read`, `ProbeGrid::read`), and
    /// returns the assets they
    /// reference, sorted and without repeats: a mesh's material slots bound to material assets, a
    /// material's textures. The packer records these as the asset's dependencies, and decode jobs
    /// check that a decoded asset references exactly what its pack says it does.
    ///
    /// Meshes, materials and rigs must start 16-byte aligned, as decoded assets do.
    pub fn references(self, bytes: &[u8]) -> Result<Vec<AssetId>, AssetError> {
        let mut references: Vec<AssetId> = match self {
            Self::Mesh => Mesh::read(bytes)?
                .materials
                .iter()
                .filter_map(|slot| match &slot.material {
                    crate::mesh::ArchivedSlotMaterial::Asset(id) => Some(*id),
                    crate::mesh::ArchivedSlotMaterial::Embedded(_) => None,
                })
                .collect(),
            Self::Texture => {
                Texture::from_ktx2(bytes)?;
                Vec::new()
            }
            Self::Material => {
                Material::read(bytes)?.textures.iter().filter_map(|binding| binding.texture()).collect()
            }
            // A rig refers to its mesh's joints, morph targets and slots by index, not to assets.
            Self::Rig => {
                CharacterRig::read(bytes)?;
                Vec::new()
            }
            // A probe grid is baked from meshes and materials, but reads none of them.
            Self::ProbeGrid => {
                ProbeGrid::read(bytes)?;
                Vec::new()
            }
        };
        references.sort_unstable();
        references.dedup();
        Ok(references)
    }
}

// Kinds sent again when a dependency is replaced are sent from their kept bytes.
const _: () = {
    let mut index = 0;
    while index < AssetKind::ALL.len() {
        let kind = AssetKind::ALL[index];
        assert!(!kind.binds_dependencies() || kind.keeps_bytes());
        index += 1;
    }
};

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Cooked bytes the runtime would refuse, by the check of their kind.
#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("invalid mesh")]
    Mesh(#[from] MeshError),
    #[error("invalid texture")]
    Texture(#[from] TextureFormatError),
    #[error("invalid material")]
    Material(#[from] MaterialError),
    #[error("invalid rig")]
    Rig(#[from] RigError),
    #[error("invalid probe grid")]
    ProbeGrid(#[from] ProbeGridError),
}
