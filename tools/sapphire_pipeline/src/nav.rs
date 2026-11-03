//! Navmesh baking (M3 P0): triangles in, `sapphire_navmesh`'s plain graph out.
//!
//! `rerecast` bakes one tile at a time ([`bake_tile`]) and knows nothing of its neighbours;
//! [`stitch()`] joins tiles into the one flat graph the simulation reads. Tiles are this module's unit
//! alone, so that an edit can rebake only what it touched, and they sit on a grid anchored at the
//! world's origin, so adding a building doesn't shift every other tile.
//!
//! **Polygons only:** no detail mesh. `DetailNavmesh` does its arithmetic in `Vec3A` and needs no
//! place in a graph whose heights only tell floors apart.
//!
//! **The same on every architecture, as far as this module can make it:**
//! - Triangles are marked walkable here, from a written-out cosine and in `f64` scalars, rather than
//!   through `TriMesh::mark_walkable_triangles`, which calls `std`'s `cos` and normalizes in `Vec3A`.
//! - `rerecast`'s rasterizer does `Vec3A` arithmetic, but only `+ - * /` lane by lane, which rounds
//!   alike on NEON and SSE2, and it reduces everything to whole cells at once. All later stages are
//!   integers.
//! - **Never enable `rerecast`'s `libm` or `nostd-libm` feature.** Each names glam's own, and Cargo
//!   unifies features on the workspace's one glam. Without them, `rerecast`'s only transcendental on
//!   this path is the `cos` above, which this module doesn't call.
//!
//! **`rerecast` is pinned to one version,** since this module works around what 0.5.0 does: it has no
//! hole merging and fails on a crossed contour ([`merge_holes`], `contours`' docs), its vertex removal
//! can hang (`bake_tile` clears the flag that leads there), and it wraps a 16-bit diagonal on big tiles
//! (`MAX_TILE_CELLS`). A new version needs `nav_attack` and the digests read again.
//!
//! **Choosing cell sizes:** both are whole fractions of two, so a tile's corner is exact in `f32`
//! wherever it is, and two tiles voxelize what they share into the same cells.

mod bake;
mod contours;
mod geometry;
mod stitch;

pub use bake::{
    Across, BakeSettings, BakeStats, Baked, MAX_TILE_CELLS, Seam, Tile, TileBounds, TilePolygon, bake,
    bake_tile, tile_range, world_rows,
};
pub use contours::merge_holes;
pub use geometry::{Triangle, colliders_to_triangles, is_walkable, mesh_to_triangles};
pub use stitch::{SeamReport, stitch};

/// A bake that can't finish.
#[derive(Debug, thiserror::Error)]
pub enum NavBakeError {
    #[error("rerecast's {stage}: {message}")]
    Rerecast { stage: &'static str, message: String },
    #[error("there's no walkable ground to bake")]
    Nothing,
    #[error("{height} m from lowest to highest is more than {limit} m of cells of this height")]
    TooTall { height: f32, limit: f32 },
    #[error("a seam left a polygon with {corners} corners")]
    TooManyCorners { corners: usize },
    #[error("a tile of {cells} cells a side is past `rerecast`'s limit of {limit}")]
    TileTooLarge { cells: i32, limit: i32 },
    #[error("tile {coord:?}'s contours cross or wind backward, even following the cells")]
    Tangled { coord: [i32; 2] },
}
