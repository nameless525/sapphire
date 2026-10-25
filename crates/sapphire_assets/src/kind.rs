//! What kind of data an asset is. Packs record it in their table of contents, so the runtime
//! knows how to check, budget and upload an asset before reading it.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::AssetId;
use crate::material::{Material, MaterialError};
use crate::mesh::{Mesh, MeshError};
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
}

impl AssetKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 4] = [Self::Mesh, Self::Texture, Self::Material, Self::Rig];

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
        }
    }

    /// Whether assets of this kind go to the GPU, through the asset server's upload queue. A rig
    /// is ready once it and its dependencies are decoded.
    pub const fn uploads(self) -> bool {
        !matches!(self, Self::Rig)
    }

    /// Whether the asset server keeps an asset's decoded bytes once it is ready. Meshes and
    /// textures are GPU data, so their bytes go once uploaded (a new renderer has them read
    /// again). Materials are small and CPU code reads them (live link, the editor), and rigs are
    /// read on the CPU only, so they stay.
    pub const fn keeps_bytes(self) -> bool {
        matches!(self, Self::Material | Self::Rig)
    }

    /// Whether an asset of this kind may depend on one of `kind`: meshes on materials, materials
    /// on textures, textures and rigs on nothing. Every dependency is of a lower kind, so chains
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
            _ => None,
        }
    }

    /// Checks cooked bytes of this kind exactly as the runtime reads them (`Mesh::read`,
    /// `Texture::from_ktx2`, `Material::read`, `CharacterRig::read`), and returns the assets they
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
        };
        references.sort_unstable();
        references.dedup();
        Ok(references)
    }
}

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
}
