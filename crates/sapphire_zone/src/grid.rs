//! The grid a zone's content is cut along.
//!
//! **The rule:** a whole placed thing belongs to the cell its anchor is in, its position as placed, and
//! is never cut. So where a thing is decides its cell, which a cook can tell from the document, with no
//! geometry read, and a save to a piece's shape never moves it to another cell. What's placed may stick
//! out of its square, by up to one cell a side ([`Grid::overreach`]): that bounds how far a cell's
//! bounds lie from its square, and sends anything larger to the [backdrop](Place::Backdrop), where it
//! can't make a cell's bounding sphere enter every view.

use bevy_reflect::Reflect;
use sapphire_math::det;
use serde::{Deserialize, Serialize};

/// The smallest cell side, in metres. A smaller grid buys nothing: a building would span many cells.
pub const MIN_CELL_SIZE: f32 = 4.0;
/// The largest cell side, in metres.
pub const MAX_CELL_SIZE: f32 = 1024.0;
/// How far from the zone's origin anything may lie or reach, in metres: `sapphire_colliders`' limit,
/// for the same reason. A zone stays within about 4 km of its origin (spec §5.4), and `f32` loses its
/// millimetres beyond this.
pub const MAX_COORDINATE: f32 = 16_384.0;

/// A zone's grid: square cells in XZ, one corner at the zone's origin.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::zone"]
#[serde(deny_unknown_fields)]
pub struct Grid {
    /// A cell's side, in metres.
    pub cell_size: f32,
}

/// A cell of a [`Grid`]: `x * cell_size..(x + 1) * cell_size` in X, and likewise in Z.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct CellCoord {
    pub x: i32,
    pub z: i32,
}

/// Where a whole placed thing belongs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Place {
    /// What stays for as long as the zone does: open ground, hills, far mountains, whatever is too
    /// large for a cell.
    Backdrop,
    Cell(CellCoord),
}

/// An axis-aligned box in the zone's space, in metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    /// The smallest box holding both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|axis| det::min(self.min[axis], other.min[axis])),
            max: std::array::from_fn(|axis| det::max(self.max[axis], other.max[axis])),
        }
    }

    /// Whether it's finite, runs from its least corner to its greatest and lies within
    /// [`MAX_COORDINATE`].
    #[must_use]
    pub fn is_valid(self) -> bool {
        let finite = self.min.iter().chain(&self.max).all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE);
        finite && self.min.iter().zip(&self.max).all(|(min, max)| min <= max)
    }
}

impl Grid {
    /// Whether the side is one a grid may have, or what's wrong with it.
    pub(crate) fn check(self) -> Result<(), String> {
        if !self.cell_size.is_finite() || !(MIN_CELL_SIZE..=MAX_CELL_SIZE).contains(&self.cell_size) {
            return Err(format!("{} isn't from {MIN_CELL_SIZE} to {MAX_CELL_SIZE} m", self.cell_size));
        }
        Ok(())
    }

    /// The cell `position` is in. A position on a line between cells is in the one with the greater
    /// coordinate.
    #[must_use]
    pub fn cell_of(self, position: [f32; 3]) -> CellCoord {
        CellCoord {
            x: (position[0] / self.cell_size).floor() as i32,
            z: (position[2] / self.cell_size).floor() as i32,
        }
    }

    /// How far past a cell's square `bounds` reach, on its farthest side in XZ, in metres: 0 inside it.
    #[must_use]
    pub fn reach(self, cell: CellCoord, bounds: Bounds) -> f32 {
        let past = |least: i32, min: f32, max: f32| {
            let start = least as f32 * self.cell_size;
            (start - min).max(max - (start + self.cell_size)).max(0.0)
        };
        past(cell.x, bounds.min[0], bounds.max[0]).max(past(cell.z, bounds.min[2], bounds.max[2]))
    }

    /// How much further than one cell's side `bounds` reach past a cell's square, if they do: such
    /// content belongs in the backdrop, or in pieces.
    #[must_use]
    pub fn overreach(self, cell: CellCoord, bounds: Bounds) -> Option<f32> {
        let reach = self.reach(cell, bounds);
        (reach > self.cell_size).then_some(reach)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRID: Grid = Grid { cell_size: 32.0 };

    #[test]
    fn a_position_is_in_the_cell_below_it_on_each_axis() {
        assert_eq!(GRID.cell_of([0.0, 99.0, 0.0]), CellCoord { x: 0, z: 0 });
        assert_eq!(GRID.cell_of([31.9, 0.0, -0.1]), CellCoord { x: 0, z: -1 });
        assert_eq!(GRID.cell_of([32.0, 0.0, -32.0]), CellCoord { x: 1, z: -1 });
        assert_eq!(GRID.cell_of([-32.1, 0.0, 64.0]), CellCoord { x: -2, z: 2 });
    }

    #[test]
    fn content_may_stick_out_by_one_cell_and_no_more() {
        let cell = CellCoord { x: 1, z: -1 };
        let boxed =
            |min_x, max_x, min_z, max_z| Bounds { min: [min_x, -500.0, min_z], max: [max_x, 500.0, max_z] };
        // The square runs 32..64 in X, −32..0 in Z. Height doesn't count.
        assert_eq!(GRID.reach(cell, boxed(32.0, 64.0, -32.0, 0.0)), 0.0);
        assert_eq!(GRID.reach(cell, boxed(30.0, 64.0, -32.0, 5.0)), 5.0);
        assert_eq!(GRID.overreach(cell, boxed(0.0, 96.0, -64.0, 32.0)), None, "one cell on every side");
        assert_eq!(GRID.overreach(cell, boxed(-1.0, 64.0, -32.0, 0.0)), Some(33.0));
        assert_eq!(GRID.overreach(cell, boxed(32.0, 64.0, -32.0, 40.0)), Some(40.0));
    }

    #[test]
    fn bounds_are_checked_and_joined() {
        let a = Bounds { min: [0.0; 3], max: [1.0; 3] };
        let b = Bounds { min: [-1.0, 0.5, 0.5], max: [0.5, 2.0, 0.5] };
        assert_eq!(a.union(b), Bounds { min: [-1.0, 0.0, 0.0], max: [1.0, 2.0, 1.0] });
        assert!(a.is_valid());
        assert!(!Bounds { min: [2.0; 3], max: [1.0; 3] }.is_valid(), "inside out");
        assert!(!Bounds { min: [f32::NAN; 3], max: [1.0; 3] }.is_valid());
        assert!(!Bounds { min: [0.0; 3], max: [20_000.0; 3] }.is_valid());
    }
}
