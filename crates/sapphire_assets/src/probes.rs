//! The ambient probe grid (`.sprobe`, M1 W5): a zone's baked ambient light, which the renderer
//! relights every frame (spec §5.12 "Lighting", `sapphire_render`'s `ambient` module).
//!
//! # Layout
//!
//! Probes stand on a regular grid, [`ProbeGrid::spacing`] meters apart from
//! [`ProbeGrid::origin`], grouped in **bricks** of [`BRICK_CELLS`]³ cells. Only bricks near
//! geometry are kept (the cooker's bake says how it chooses them): [`ProbeGrid::brick_table`]
//! maps every brick position of the grid's extent to a kept brick, or to [`EMPTY_BRICK`]. A
//! kept brick lists the [`BRICK_PROBES`] probes on its corners and faces, [`BRICK_SIDE`]³ of
//! them including the ones it shares with its neighbors, so the renderer can lay each brick out
//! on its own and filter within it; each probe's data is stored once
//! ([`ProbeGrid::brick_probes`] indexes it).
//!
//! # Per probe
//!
//! - **Sky visibility**: which directions see the sky from the probe, 1 where a ray escapes and
//!   0 where it hits geometry, as SH (`sapphire_math::sh`): L2 (9 coefficients) or L1 (4),
//!   [`ProbeGrid::visibility_coefficients`]. The renderer multiplies it by the frame's sky.
//! - **Bounce light** per time-of-day key ([`ProbeGrid::key_hours`]): the irradiance the lit
//!   environment sends the probe under that key's sun or moon, sky and shadow tint, after the
//!   bake's bounces, as L1 per color channel (12 coefficients, renderer normalization: a uniform
//!   radiance of 1 is an irradiance of 1). The renderer blends the keys the frame's hour lies
//!   between.
//! - **Probe data** ([`ProbeData`]): how far the probe sees along each axis before geometry, for
//!   the renderer's leak test, and whether it lies in open space at all.
//!
//! Coefficients are IEEE half floats (`half`'s bits), which is what the GPU reads.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

/// Conventional file extension for cooked probe grids.
pub const PROBE_GRID_EXTENSION: &str = "sprobe";

/// Cells along each edge of a brick.
pub const BRICK_CELLS: u32 = 4;
/// Probes along each edge of a brick, the ones it shares with its neighbors included.
pub const BRICK_SIDE: u32 = BRICK_CELLS + 1;
/// Probes a brick lists.
pub const BRICK_PROBES: u32 = BRICK_SIDE * BRICK_SIDE * BRICK_SIDE;
/// A brick table entry for a brick with no probes.
pub const EMPTY_BRICK: u32 = u32::MAX;
/// Bounce coefficients per probe per key: L1 for red, green and blue.
pub const BOUNCE_COEFFICIENTS: u32 = 12;
/// Most bricks along an axis: the renderer lays bricks out in a 3D texture of at most 2048 texels
/// a side.
pub const MAX_BRICKS_PER_AXIS: u32 = 256;
/// Most time-of-day keys a grid is baked for.
pub const MAX_KEYS: u32 = 32;

/// A zone's ambient probe grid. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct ProbeGrid {
    /// World position of the grid's first probe: brick (0, 0, 0)'s corner.
    pub origin: [f32; 3],
    /// Meters between neighboring probes.
    pub spacing: f32,
    /// Brick positions along x, y and z.
    pub bricks: [u32; 3],
    /// Per brick position (x fastest, then y, then z): its kept brick, or [`EMPTY_BRICK`].
    pub brick_table: Vec<u32>,
    /// Per kept brick, [`BRICK_PROBES`] probe indices, x fastest, then y, then z.
    pub brick_probes: Vec<u32>,
    /// The hours of the time-of-day keys bounce was baked under, ascending, each in `0..24`.
    pub key_hours: Vec<f32>,
    /// Sky visibility coefficients per probe: 4 (L1) or 9 (L2).
    pub visibility_coefficients: u32,
    /// Sky visibility, half floats, [`Self::visibility_coefficients`] per probe.
    pub visibility: Vec<u16>,
    /// Bounce irradiance, half floats: per probe, per key, [`BOUNCE_COEFFICIENTS`] (red's L1, then
    /// green's, then blue's).
    pub bounce: Vec<u16>,
    /// Per probe, [`ProbeData`]'s bits.
    pub probe_data: Vec<u32>,
}

impl Versioned for ProbeGrid {
    /// 1: M1 W5.
    const VERSION: u32 = 1;
}

/// What a probe knows of the geometry around it: how far it sees along each axis before
/// geometry, in 31sts of the grid's spacing (31: a full spacing or more, so its neighbor), and
/// whether it lies in open space (a probe inside geometry sees backfaces; its light is filled in
/// from its neighbors, and the renderer weighs it down).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProbeData {
    /// Along +x, −x, +y, −y, +z, −z.
    pub free: [u8; 6],
    pub valid: bool,
}

impl ProbeData {
    /// The largest free distance, a full spacing.
    pub const FULL: u8 = 31;
    const VALID_BIT: u32 = 1 << 30;
    /// Bits a valid grid leaves clear (the renderer marks probes with them at upload).
    pub const UNUSED_BITS: u32 = !((1 << 31) - 1);

    /// The free distance `meters` as a step of a grid `spacing` apart, rounded down (so the leak
    /// test errs toward seeing geometry), at most [`Self::FULL`].
    pub fn quantize(meters: f32, spacing: f32) -> u8 {
        let steps = (meters / spacing * f32::from(Self::FULL)).floor();
        if steps.is_nan() { 0 } else { steps.clamp(0.0, f32::from(Self::FULL)) as u8 }
    }

    pub fn to_bits(self) -> u32 {
        let free = self
            .free
            .iter()
            .enumerate()
            .fold(0, |bits, (axis, &steps)| bits | (u32::from(steps.min(Self::FULL)) << (5 * axis)));
        free | if self.valid { Self::VALID_BIT } else { 0 }
    }

    pub fn from_bits(bits: u32) -> Self {
        Self {
            free: std::array::from_fn(|axis| ((bits >> (5 * axis)) & 31) as u8),
            valid: bits & Self::VALID_BIT != 0,
        }
    }
}

impl ProbeGrid {
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every count and index
    /// ([`ArchivedProbeGrid::validate`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedProbeGrid, ProbeGridError> {
        let grid = sapphire_serialize::access::<Self>(bytes)?;
        grid.validate()?;
        Ok(grid)
    }

    /// Probes the grid stores.
    pub fn probe_count(&self) -> usize {
        self.probe_data.len()
    }
}

impl ArchivedProbeGrid {
    /// Probes the grid stores.
    pub fn probe_count(&self) -> usize {
        self.probe_data.len()
    }

    /// Kept bricks.
    pub fn brick_count(&self) -> usize {
        self.brick_probes.len() / BRICK_PROBES as usize
    }

    pub fn origin(&self) -> [f32; 3] {
        self.origin.map(|value| value.to_native())
    }

    pub fn bricks(&self) -> [u32; 3] {
        self.bricks.map(|value| value.to_native())
    }

    /// The time-of-day keys' hours.
    pub fn key_hours(&self) -> impl ExactSizeIterator<Item = f32> + '_ {
        self.key_hours.iter().map(|hours| hours.to_native())
    }

    /// Checks what the renderer relies on: a finite placement, the brick table's size and
    /// entries, every probe index, the coefficient counts, and finite coefficients.
    pub fn validate(&self) -> Result<(), ProbeGridError> {
        let invalid = |what: &'static str, reason: String| Err(ProbeGridError::Invalid { what, reason });
        let spacing = self.spacing.to_native();
        if !(spacing.is_finite() && spacing > 0.0) || !self.origin().iter().all(|v| v.is_finite()) {
            return invalid("placement", format!("origin {:?}, spacing {spacing}", self.origin()));
        }
        let bricks = self.bricks();
        if bricks.iter().any(|&n| n == 0 || n > MAX_BRICKS_PER_AXIS) {
            return invalid("brick extent", format!("{bricks:?}, each 1..={MAX_BRICKS_PER_AXIS}"));
        }
        let positions = bricks.iter().map(|&n| n as usize).product::<usize>();
        if self.brick_table.len() != positions {
            return invalid(
                "brick table",
                format!("{} entries for {positions} bricks", self.brick_table.len()),
            );
        }
        if !self.brick_probes.len().is_multiple_of(BRICK_PROBES as usize) {
            return invalid("brick probes", format!("{} indices", self.brick_probes.len()));
        }
        let kept = self.brick_count();
        let mut used = vec![false; kept];
        for entry in self.brick_table.iter().map(|entry| entry.to_native()) {
            if entry == EMPTY_BRICK {
                continue;
            }
            match used.get_mut(entry as usize) {
                Some(seen) if !*seen => *seen = true,
                _ => {
                    return invalid(
                        "brick table",
                        format!("brick {entry} of {kept} is missing or listed twice"),
                    );
                }
            }
        }
        if used.iter().any(|seen| !seen) {
            return invalid("brick table", "a kept brick has no position".to_owned());
        }
        let probes = self.probe_count();
        // A grid of nothing would bind empty buffers and textures: an error, not a no-op.
        if kept == 0 || probes == 0 {
            return invalid("bricks", format!("{kept} bricks and {probes} probes kept, at least one"));
        }
        if self.probe_data.iter().any(|bits| bits.to_native() & ProbeData::UNUSED_BITS != 0) {
            return invalid("probe data", "bits past the validity bit are set".to_owned());
        }
        if let Some(index) = self.brick_probes.iter().map(|i| i.to_native()).find(|&i| i as usize >= probes) {
            return invalid("brick probes", format!("probe {index} of {probes}"));
        }
        let keys = self.key_hours.len();
        if keys == 0 || keys > MAX_KEYS as usize {
            return invalid("keys", format!("{keys}, 1..={MAX_KEYS}"));
        }
        let hours: Vec<f32> = self.key_hours().collect();
        if !hours.iter().all(|h| (0.0..24.0).contains(h)) || !hours.is_sorted_by(|a, b| a < b) {
            return invalid("key hours", format!("{hours:?}"));
        }
        let order = self.visibility_coefficients.to_native();
        if !matches!(order, 4 | 9) {
            return invalid("visibility", format!("{order} coefficients, 4 or 9"));
        }
        if self.visibility.len() != probes * order as usize {
            return invalid(
                "visibility",
                format!("{} coefficients for {probes} probes", self.visibility.len()),
            );
        }
        if self.bounce.len() != probes * keys * BOUNCE_COEFFICIENTS as usize {
            return invalid("bounce", format!("{} coefficients for {probes} probes", self.bounce.len()));
        }
        let finite = |bits: &rend::u16_le| half::f16::from_bits(bits.to_native()).is_finite();
        if !self.visibility.iter().all(finite) || !self.bounce.iter().all(finite) {
            return invalid("coefficients", "not all finite".to_owned());
        }
        Ok(())
    }
}

/// A cooked probe grid that isn't valid.
#[derive(Debug, thiserror::Error)]
pub enum ProbeGridError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("{what}: {reason}")]
    Invalid { what: &'static str, reason: String },
}

/// A grid of one brick at the origin, every probe its own, `keys` keys: for tests here and in
/// the server's.
#[cfg(test)]
pub(crate) fn one_brick(keys: usize) -> ProbeGrid {
    let probes = BRICK_PROBES as usize;
    let half = |value: f32| half::f16::from_f32(value).to_bits();
    ProbeGrid {
        origin: [-2.0, 0.0, -2.0],
        spacing: 1.0,
        bricks: [1, 1, 1],
        brick_table: vec![0],
        brick_probes: (0..BRICK_PROBES).collect(),
        key_hours: (0..keys).map(|key| key as f32 * 3.0).collect(),
        visibility_coefficients: 9,
        visibility: (0..probes * 9).map(|i| half(i as f32 / 100.0)).collect(),
        bounce: (0..probes * keys * 12).map(|i| half(i as f32 / 1000.0)).collect(),
        probe_data: (0..probes)
            .map(|i| ProbeData { free: [ProbeData::FULL; 6], valid: i % 7 != 0 }.to_bits())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooked_grids_round_trip() {
        let grid = one_brick(3);
        let bytes = grid.to_cooked().expect("cook");
        let read = ProbeGrid::read(&bytes).expect("valid");
        assert_eq!(read.probe_count(), BRICK_PROBES as usize);
        assert_eq!(read.brick_count(), 1);
        assert_eq!(read.key_hours().collect::<Vec<_>>(), [0.0, 3.0, 6.0]);
        let back: ProbeGrid = rkyv::deserialize::<ProbeGrid, rkyv::rancor::Error>(read).expect("deserialize");
        assert_eq!(back, grid);
    }

    #[test]
    fn invalid_grids_are_refused() {
        let check = |edit: &dyn Fn(&mut ProbeGrid)| {
            let mut grid = one_brick(2);
            edit(&mut grid);
            ProbeGrid::read(&grid.to_cooked().expect("cook")).map(|_| ())
        };
        assert!(check(&|_| {}).is_ok());
        assert!(check(&|grid| grid.spacing = 0.0).is_err(), "spacing");
        assert!(check(&|grid| grid.brick_table = vec![1]).is_err(), "a brick that doesn't exist");
        assert!(check(&|grid| grid.brick_table = vec![EMPTY_BRICK]).is_err(), "a brick with no position");
        assert!(check(&|grid| grid.brick_probes[3] = 999).is_err(), "a probe that doesn't exist");
        assert!(
            check(&|grid| {
                grid.brick_table = vec![EMPTY_BRICK];
                grid.brick_probes.clear();
                grid.probe_data.clear();
                grid.visibility.clear();
                grid.bounce.clear();
            })
            .is_err(),
            "no bricks"
        );
        assert!(check(&|grid| grid.probe_data[2] |= 1 << 31).is_err(), "an unused bit");
        assert!(check(&|grid| grid.key_hours = vec![6.0, 6.0]).is_err(), "keys out of order");
        assert!(check(&|grid| grid.key_hours = vec![]).is_err(), "no keys");
        assert!(check(&|grid| grid.visibility_coefficients = 4).is_err(), "coefficient count");
        assert!(check(&|grid| grid.bounce.pop().map(drop).unwrap_or(())).is_err(), "bounce count");
        assert!(
            check(&|grid| grid.visibility[5] = half::f16::INFINITY.to_bits()).is_err(),
            "a coefficient that isn't finite"
        );
        let mut l1 = one_brick(1);
        l1.visibility_coefficients = 4;
        l1.visibility.truncate(BRICK_PROBES as usize * 4);
        assert!(ProbeGrid::read(&l1.to_cooked().unwrap()).is_ok(), "L1 visibility");
    }

    #[test]
    fn probe_data_packs_distances_and_validity() {
        let data = ProbeData { free: [0, 1, 15, 30, 31, 7], valid: true };
        assert_eq!(ProbeData::from_bits(data.to_bits()), data);
        assert_eq!(ProbeData::from_bits(0), ProbeData::default());
        assert_eq!(ProbeData::quantize(0.5, 1.0), 15, "rounded down");
        assert_eq!(ProbeData::quantize(3.0, 1.0), ProbeData::FULL, "a full spacing at most");
        assert_eq!(ProbeData::quantize(-1.0, 1.0), 0);
        assert_eq!(ProbeData::quantize(f32::NAN, 1.0), 0);
    }
}
