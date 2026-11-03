//! What a mesh's material slot draws through, as data (M3 K1, spec §5.12's permutation inventory).
//!
//! The renderer builds a mesh's pipelines from three things: its material's family and flags, and the
//! position format its draws read. [`PipelineKey`] is that triple, here rather than in the renderer because
//! the cooker writes it into a pack's table ([`Toc::pipeline_keys`](crate::pack::Toc::pipeline_keys)) and must
//! not link the renderer, and this crate already owns all three parts. Render state stays the renderer's: it
//! derives its variants from a key at load, through the one function its uploads go through, and a test there
//! holds real cooked keys to what the same meshes' uploads build.
//!
//! The format is the one draws read, not the one the mesh is cooked in: a skinned or morphed mesh draws its
//! skinning outputs, which are f32 whatever its stream holds
//! ([`ArchivedMesh::draw_format`](crate::mesh::ArchivedMesh::draw_format)).

use std::cmp::Ordering;
use std::fmt;

use crate::AssetId;
use crate::material::{MaterialFamily, MaterialFlags};
use crate::mesh::{ArchivedMesh, ArchivedSlotMaterial, PositionFormat};

/// What one material slot of a mesh draws through. Ordered, so a list of them has one spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub family: MaterialFamily,
    pub flags: MaterialFlags,
    /// What its draws read ([`ArchivedMesh::draw_format`]).
    pub format: PositionFormat,
}

impl PipelineKey {
    /// The bytes one key takes in a pack's table: a multiple of 16 keeps the data after it aligned.
    pub const ROW_LEN: usize = 16;

    /// One key per material slot of `mesh`, in slot order, with `material` telling the family and flags of
    /// the material assets its slots bind: every slot, as an upload builds every slot's, and a submesh draws
    /// exactly one. The asset a slot binds that `material` doesn't know, otherwise.
    pub fn of_mesh(
        mesh: &ArchivedMesh,
        mut material: impl FnMut(AssetId) -> Option<(MaterialFamily, MaterialFlags)>,
    ) -> Result<Vec<Self>, AssetId> {
        let format = mesh.draw_format();
        mesh.materials
            .iter()
            .map(|slot| {
                let (family, flags) = match &slot.material {
                    ArchivedSlotMaterial::Embedded(material) => (material.family, material.flags()),
                    ArchivedSlotMaterial::Asset(id) => material(*id).ok_or(*id)?,
                };
                Ok(Self { family, flags, format })
            })
            .collect()
    }

    /// The key as a pack's table stores it: its family and format (a byte each, their positions in
    /// [`MaterialFamily::ALL`] and [`PositionFormat::ALL`]), 2 reserved bytes, its flags (little-endian) and 8
    /// reserved bytes.
    pub fn to_row(self) -> [u8; Self::ROW_LEN] {
        let mut row = [0; Self::ROW_LEN];
        row[0] = self.family.index() as u8;
        row[1] = self.format.index() as u8;
        row[4..8].copy_from_slice(&self.flags.bits().to_le_bytes());
        row
    }

    /// The key a pack's table stores as `row`; why it can't be one, otherwise.
    pub fn from_row(row: &[u8; Self::ROW_LEN]) -> Result<Self, &'static str> {
        let family = *MaterialFamily::ALL.get(usize::from(row[0])).ok_or("unknown material family")?;
        let format = *PositionFormat::ALL.get(usize::from(row[1])).ok_or("unknown position format")?;
        if row[2..4] != [0; 2] || row[8..] != [0; 8] {
            return Err("reserved bytes aren't zero");
        }
        let flags = u32::from_le_bytes(*row[4..8].first_chunk().expect("4 bytes"));
        let flags = MaterialFlags::from_bits(flags).ok_or("unknown material flags")?;
        Ok(Self { family, flags, format })
    }

    fn order(self) -> (usize, u32, usize) {
        (self.family.index(), self.flags.bits(), self.format.index())
    }
}

impl PartialOrd for PipelineKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PipelineKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order().cmp(&other.order())
    }
}

impl fmt::Display for PipelineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} flags {:#x} {:?}", self.family, self.flags.bits(), self.format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> PipelineKey {
        PipelineKey {
            family: MaterialFamily::AnimeCharacter,
            flags: MaterialFlags::ALPHA_TEST | MaterialFlags::OUTLINE,
            format: PositionFormat::Float32,
        }
    }

    #[test]
    fn a_row_says_the_key_it_was_made_from() {
        for family in MaterialFamily::ALL {
            for format in PositionFormat::ALL {
                let key = PipelineKey { family, format, ..key() };
                assert_eq!(PipelineKey::from_row(&key.to_row()), Ok(key));
            }
        }
    }

    #[test]
    fn a_row_with_anything_unknown_is_refused() {
        let good = key().to_row();
        let with = |index: usize, value: u8| {
            let mut row = good;
            row[index] = value;
            PipelineKey::from_row(&row)
        };
        assert_eq!(with(0, 5), Err("unknown material family"));
        assert_eq!(with(1, 2), Err("unknown position format"));
        assert_eq!(with(2, 1), Err("reserved bytes aren't zero"));
        assert_eq!(with(15, 1), Err("reserved bytes aren't zero"));
        assert_eq!(with(5, 0xff), Err("unknown material flags"));
    }

    #[test]
    fn keys_sort_by_family_then_flags_then_format() {
        let a = PipelineKey { flags: MaterialFlags::empty(), format: PositionFormat::Float32, ..key() };
        let b = PipelineKey { flags: MaterialFlags::OUTLINE, format: PositionFormat::Unorm16, ..key() };
        let c = PipelineKey { family: MaterialFamily::Environment, ..a };
        let mut keys = [c, b, a];
        keys.sort();
        assert_eq!(keys, [a, b, c]);
    }
}
