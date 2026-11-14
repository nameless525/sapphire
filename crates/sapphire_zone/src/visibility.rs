//! Visible sets by cell: M5 D0's prototype of cooked visibility, which D7 may build into the zone.
//!
//! A cook samples the space a camera can stand in, cell by cell, and writes for each cell the objects some
//! sample saw: a cell's merged mesh, or one of its instances. A camera in a cell then draws, of what its
//! frustum passes, only that cell's set. The sets are conservative (an object a camera in the cell could see
//! is always in its set, `sapphire_pipeline`'s `zone::visibility` says why), so a frame drawn with them is
//! the frame drawn without them, pixel for pixel, and a set is only ever a saving.
//!
//! **A camera outside the sampled space draws by the frustum alone** ([`Outside`]): outside the zone's cells,
//! under or over the heights sampled, or inside an occluder's bounds, whose walls are seen from behind there.
//! Only the main view and its prepass may read a set: a shadow cascade, an inset or a cutscene's other
//! camera sees from somewhere else.
//!
//! This is not the cooked zone's format and changes nothing a shipping build draws: the prototype's sets are
//! a file beside a cook (`target/`), written by `sapphire_pipeline cell-visibility` and read by a dev
//! sandbox's `--cell-visibility`. Its bytes are little-endian and its order fixed, so two cooks that agree
//! write the same file, and [`CellVisibility::digest`] pins it.

use sapphire_serialize::AssetId;
use xxhash_rust::xxh3::xxh3_64;

use crate::ArchivedZonePresentation;
use crate::grid::{Bounds, CellCoord, Grid};

/// The file's first bytes.
const MAGIC: [u8; 4] = *b"SZVS";
/// Its layout's version.
const VERSION: u32 = 1;
/// The most cells and objects a file may hold, so a bad header can't ask for gigabytes.
const MAX_ENTRIES: usize = 1 << 20;

/// Visible sets by cell. See the [module docs](self).
#[derive(Clone, Debug, PartialEq)]
pub struct CellVisibility {
    /// A cell's side, in metres: the zone's grid.
    pub cell_size: f32,
    /// The camera heights sampled, least and most, in metres in the zone's space.
    pub heights: [f32; 2],
    /// The cooked zone's cells, in its order, each with the objects it draws.
    pub cells: Vec<VisibleCell>,
    /// Boxes a camera must stand outside of for a set to hold: each occluder's whole bounds.
    pub keep_out: Vec<Bounds>,
    /// Per cell, a bit per object, [`Self::words`] words a cell: bit `o % 64` of word `o / 64` is object `o`,
    /// numbered over every cell's objects in order.
    pub bits: Vec<u64>,
}

/// One cell of a zone, as its sets number what it draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleCell {
    /// `[x, z]`, as the cooked zone's.
    pub coord: [i32; 2],
    /// The mesh of each object it draws: its merged mesh if it has one, then its instances', in the cooked
    /// zone's order. Lights aren't objects.
    pub objects: Vec<AssetId>,
}

/// Why a camera draws by its frustum alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outside {
    /// It isn't over any of the zone's cells.
    NoCell,
    /// It's lower or higher than the heights sampled.
    Height,
    /// It stands inside an occluder's bounds.
    InOccluder,
}

/// Why a file of sets can't be read, or doesn't fit a zone.
#[derive(Debug, thiserror::Error)]
pub enum VisibilityError {
    #[error("not a visible sets file, or version {0} rather than {VERSION}")]
    Header(u32),
    #[error("the file ends early")]
    Short,
    #[error("the file is invalid: {0}")]
    Invalid(String),
    #[error("the sets don't fit this zone: {0}")]
    Mismatch(String),
}

impl CellVisibility {
    /// Words of [`Self::bits`] each cell takes.
    #[must_use]
    pub fn words(&self) -> usize {
        self.object_count().div_ceil(64)
    }

    /// Objects over every cell.
    #[must_use]
    pub fn object_count(&self) -> usize {
        self.cells.iter().map(|cell| cell.objects.len()).sum()
    }

    /// The set a camera at `eye` (in the zone's space) draws by, as the index of the cell it stands in, or why
    /// it has none.
    pub fn cell_for(&self, eye: [f32; 3]) -> Result<usize, Outside> {
        if !(self.heights[0]..=self.heights[1]).contains(&eye[1]) {
            return Err(Outside::Height);
        }
        let inside = |bounds: &Bounds| {
            (0..3).all(|axis| bounds.min[axis] <= eye[axis] && eye[axis] <= bounds.max[axis])
        };
        if self.keep_out.iter().any(inside) {
            return Err(Outside::InOccluder);
        }
        let CellCoord { x, z } = Grid { cell_size: self.cell_size }.cell_of(eye);
        self.cells.binary_search_by_key(&[x, z], |cell| cell.coord).map_err(|_| Outside::NoCell)
    }

    /// Whether a camera in cell `from` may see object `object`.
    #[must_use]
    pub fn sees(&self, from: usize, object: usize) -> bool {
        let word = self.bits[from * self.words() + object / 64];
        word & (1 << (object % 64)) != 0
    }

    /// The number of each cell's first object: object `firsts[cell] + item` is that cell's `item`th.
    #[must_use]
    pub fn firsts(&self) -> Vec<usize> {
        self.cells
            .iter()
            .scan(0, |next, cell| {
                let first = *next;
                *next += cell.objects.len();
                Some(first)
            })
            .collect()
    }

    /// How many objects each cell's set holds.
    #[must_use]
    pub fn set_sizes(&self) -> Vec<usize> {
        let words = self.words();
        self.bits
            .chunks(words.max(1))
            .take(self.cells.len())
            .map(|set| set.iter().map(|word| word.count_ones() as usize).sum())
            .collect()
    }

    /// The same sets a cell coarser: a cell sees every object of each cell it sees anything of. What a set of
    /// cells per cell would draw, in this file's form.
    #[must_use]
    pub fn by_cells(&self) -> Self {
        let words = self.words();
        let firsts = self.firsts();
        let mut bits = vec![0_u64; self.bits.len()];
        for from in 0..self.cells.len() {
            for (cell, first) in self.cells.iter().zip(&firsts) {
                let objects = *first..*first + cell.objects.len();
                if objects.clone().any(|object| self.sees(from, object)) {
                    for object in objects {
                        bits[from * words + object / 64] |= 1 << (object % 64);
                    }
                }
            }
        }
        Self { bits, ..self.clone() }
    }

    /// Checks the sets were made for `zone`'s cells and objects, naming the first difference.
    pub fn check_fits(&self, zone: &ArchivedZonePresentation) -> Result<(), VisibilityError> {
        let mismatch = |why: String| Err(VisibilityError::Mismatch(why));
        if zone.cells.len() != self.cells.len() {
            return mismatch(format!("{} cells, the zone has {}", self.cells.len(), zone.cells.len()));
        }
        if zone.cell_size.to_native() != self.cell_size {
            return mismatch(format!("{} m cells, the zone's are {} m", self.cell_size, zone.cell_size));
        }
        for (ours, theirs) in self.cells.iter().zip(zone.cells.iter()) {
            let coord = theirs.coord();
            let contents = &theirs.contents;
            let drawn: Vec<AssetId> = contents
                .mesh
                .as_ref()
                .copied()
                .into_iter()
                .chain(contents.instances().map(|index| zone.instances[index].mesh))
                .collect();
            if ours.coord != [coord.x, coord.z] || ours.objects != drawn {
                return mismatch(format!(
                    "cell {:?} draws other things than the zone's ({}, {})",
                    ours.coord, coord.x, coord.z
                ));
            }
        }
        Ok(())
    }

    /// The file's bytes: the same for the same sets on every machine.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        let u32_ = |out: &mut Vec<u8>, value: u32| out.extend_from_slice(&value.to_le_bytes());
        u32_(&mut out, VERSION);
        let f32_ = |out: &mut Vec<u8>, value: f32| out.extend_from_slice(&value.to_bits().to_le_bytes());
        f32_(&mut out, self.cell_size);
        f32_(&mut out, self.heights[0]);
        f32_(&mut out, self.heights[1]);
        u32_(&mut out, self.cells.len() as u32);
        for cell in &self.cells {
            out.extend_from_slice(&cell.coord[0].to_le_bytes());
            out.extend_from_slice(&cell.coord[1].to_le_bytes());
            u32_(&mut out, cell.objects.len() as u32);
            for mesh in &cell.objects {
                out.extend_from_slice(&mesh.to_bytes());
            }
        }
        u32_(&mut out, self.keep_out.len() as u32);
        for bounds in &self.keep_out {
            for value in bounds.min.iter().chain(&bounds.max) {
                f32_(&mut out, *value);
            }
        }
        for word in &self.bits {
            out.extend_from_slice(&word.to_le_bytes());
        }
        out
    }

    /// Reads [`Self::to_bytes`]'s bytes, refusing anything that isn't a whole, consistent file.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, VisibilityError> {
        let mut reader = Reader { bytes, at: 0 };
        if reader.take(4)? != MAGIC {
            return Err(VisibilityError::Header(0));
        }
        let version = reader.u32()?;
        if version != VERSION {
            return Err(VisibilityError::Header(version));
        }
        let cell_size = reader.f32()?;
        let heights = [reader.f32()?, reader.f32()?];
        let invalid = |why: &str| VisibilityError::Invalid(why.to_owned());
        if !(cell_size.is_finite() && cell_size > 0.0 && heights[0] <= heights[1]) {
            return Err(invalid("its cell size or heights"));
        }
        let cell_count = reader.count()?;
        let mut cells = Vec::with_capacity(cell_count);
        let mut objects = 0;
        for _ in 0..cell_count {
            let coord = [reader.i32()?, reader.i32()?];
            let count = reader.count()?;
            objects += count;
            let objects = (0..count).map(|_| reader.id()).collect::<Result<_, _>>()?;
            cells.push(VisibleCell { coord, objects });
        }
        if objects > MAX_ENTRIES || !cells.windows(2).all(|pair| pair[0].coord < pair[1].coord) {
            return Err(invalid("its cells, which must ascend"));
        }
        let keep_count = reader.count()?;
        let mut keep_out = Vec::with_capacity(keep_count);
        for _ in 0..keep_count {
            let mut values = [0.0; 6];
            for value in &mut values {
                *value = reader.f32()?;
            }
            let bounds =
                Bounds { min: [values[0], values[1], values[2]], max: [values[3], values[4], values[5]] };
            if !bounds.is_valid() {
                return Err(invalid("a box to keep out of"));
            }
            keep_out.push(bounds);
        }
        let words = cell_count * objects.div_ceil(64);
        let bits = (0..words).map(|_| reader.u64()).collect::<Result<Vec<_>, _>>()?;
        if reader.at != bytes.len() {
            return Err(invalid("bytes after its sets"));
        }
        let read = Self { cell_size, heights, cells, keep_out, bits };
        // No bit past the last object.
        let spare = read.words() * 64 - objects;
        if spare > 0 && read.bits.chunks(read.words()).any(|set| set[set.len() - 1] >> (64 - spare) != 0) {
            return Err(invalid("a bit past its objects"));
        }
        Ok(read)
    }

    /// The file's digest, which a test pins.
    #[must_use]
    pub fn digest(&self) -> u64 {
        xxh3_64(&self.to_bytes())
    }
}

/// Reads a file of sets in order.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], VisibilityError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(VisibilityError::Short)?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], VisibilityError> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }

    fn u32(&mut self) -> Result<u32, VisibilityError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, VisibilityError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, VisibilityError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32, VisibilityError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn id(&mut self) -> Result<AssetId, VisibilityError> {
        Ok(AssetId::from_bytes(self.array()?))
    }

    fn count(&mut self) -> Result<usize, VisibilityError> {
        let count = self.u32()? as usize;
        if count > MAX_ENTRIES {
            return Err(VisibilityError::Invalid(format!("a count of {count}")));
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> AssetId {
        AssetId::from_bytes([n; 16])
    }

    /// Two cells side by side, (0, 0) drawing three objects and (1, 0) one; one building's bounds kept out of.
    fn sets() -> CellVisibility {
        CellVisibility {
            cell_size: 32.0,
            heights: [0.5, 8.0],
            cells: vec![
                VisibleCell { coord: [0, 0], objects: vec![id(1), id(2), id(2)] },
                VisibleCell { coord: [1, 0], objects: vec![id(3)] },
            ],
            keep_out: vec![Bounds { min: [4.0, -1.0, 4.0], max: [10.0, 9.0, 10.0] }],
            // From (0, 0): objects 0 and 1. From (1, 0): object 3 alone.
            bits: vec![0b0011, 0b1000],
        }
    }

    #[test]
    fn a_camera_finds_its_cell_or_why_it_has_none() {
        let sets = sets();
        assert_eq!(sets.cell_for([1.0, 2.0, 1.0]), Ok(0));
        assert_eq!(sets.cell_for([40.0, 2.0, 31.0]), Ok(1));
        assert_eq!(sets.cell_for([40.0, 2.0, 33.0]), Err(Outside::NoCell));
        assert_eq!(sets.cell_for([-0.1, 2.0, 1.0]), Err(Outside::NoCell));
        assert_eq!(sets.cell_for([1.0, 8.5, 1.0]), Err(Outside::Height), "over the roofs");
        assert_eq!(sets.cell_for([1.0, 0.2, 1.0]), Err(Outside::Height));
        assert_eq!(sets.cell_for([5.0, 2.0, 5.0]), Err(Outside::InOccluder));
        assert!(sets.sees(0, 1) && !sets.sees(0, 2) && sets.sees(1, 3) && !sets.sees(1, 0));
        assert_eq!(sets.firsts(), [0, 3]);
        assert_eq!(sets.set_sizes(), [2, 1]);
    }

    #[test]
    fn by_cells_sees_every_object_of_a_cell_it_sees_anything_of() {
        let coarse = sets().by_cells();
        assert_eq!(coarse.bits, [0b0111, 0b1000]);
    }

    #[test]
    fn its_bytes_read_back_and_anything_else_is_refused() {
        let sets = sets();
        let bytes = sets.to_bytes();
        assert_eq!(CellVisibility::from_bytes(&bytes).unwrap(), sets);
        assert!(matches!(CellVisibility::from_bytes(&bytes[..bytes.len() - 1]), Err(VisibilityError::Short)));
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(CellVisibility::from_bytes(&longer), Err(VisibilityError::Invalid(_))));
        let mut past = bytes.clone();
        let last = past.len() - 1;
        past[last] = 0x80;
        assert!(
            matches!(CellVisibility::from_bytes(&past), Err(VisibilityError::Invalid(_))),
            "a bit past the objects"
        );
        let mut version = bytes;
        version[4] = 9;
        assert!(matches!(CellVisibility::from_bytes(&version), Err(VisibilityError::Header(9))));
        assert!(matches!(CellVisibility::from_bytes(b"SZ"), Err(VisibilityError::Short)));
    }
}
