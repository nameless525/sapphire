//! Navmesh baking (M3 P0, P1): a zone's colliders in, `sapphire_navmesh`'s plain graph out.
//!
//! `rerecast` bakes one tile at a time ([`bake_tile`]) and knows nothing of its neighbours;
//! [`stitch()`] joins tiles into the one flat graph the simulation reads, and [`keep_seeded`] drops what
//! a character can't get to. Tiles are this module's unit alone, so that an edit can rebake only what it
//! touched ([`bake_cached`]), and they sit on a grid anchored at the world's origin, in rows fixed for
//! every bake ([`ROWS`]), so adding a building doesn't shift every other tile.
//!
//! **What it bakes is what the simulation collides with:** a zone's cooked colliders, world and
//! character blockers, read from the same `.scol` bytes the simulation reads ([`bake_colliders`]). Two
//! things follow. A saved kit piece doesn't touch a navmesh at all, since a prefab carries no colliders.
//! And its input is bit-exact on every platform by construction: the collider cook goes through
//! `sapphire_math::det`, and its triangles from there in `+ - *`. A mesh's triangles wouldn't be, since
//! `assembly` shapes its geometry with `std`'s transcendentals, whose low bits follow each platform's
//! C library, and a bake's bytes go in a replay's digest. `nav_bake`'s pinned digests hold this.
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
//! (`MAX_TILE_CELLS`). A new version needs `nav_attack` and the digests read again: `nav_bake`'s
//! `rerecast_is_the_version_this_module_was_read_against` fails until someone has.
//!
//! **A bake that stops is a failed cook.** Tiles bake on threads of their own, and nothing finishing for
//! two minutes fails the cook, naming the tile (the `limit` module). A stalled CI job says nothing.
//!
//! **Choosing cell sizes:** both are whole fractions of two, so a tile's corner is exact in `f32`
//! wherever it is, and two tiles voxelize what they share into the same cells.

mod bake;
mod contours;
mod geometry;
mod islands;
mod limit;
mod stitch;

pub use bake::{
    Across, BakeSettings, BakeStats, Baked, MAX_TILE_CELLS, ROWS, Seam, TILE_BAKER, Tile, TileBounds,
    TilePolygon, bake, bake_cached, bake_tile, tile_range,
};
pub use contours::merge_holes;
pub use geometry::{Triangle, colliders_to_triangles, is_walkable};
pub use islands::{IslandStats, Seed, keep_seeded};
pub use stitch::{SeamReport, stitch};

use sapphire_colliders::{ColliderError, ColliderSet, Group};
use sapphire_navmesh::NavMeshError;

use crate::cache::{Cache, CacheError, Stage};

/// Reads colliders into triangles, bakes, stitches and prunes. Bump the version whenever that changes
/// a `.snav` for the same colliders and document, a tile's bake aside ([`TILE_BAKER`]).
pub const IMPORTER: Stage = Stage { name: "navigation", version: 1 };

/// The `.snav` writer. Bump the version whenever it changes output for the same graph.
pub const COOKER: Stage = Stage { name: "snav", version: 1 };

/// Most tiles a zone's area may cover. Past this it's a slip of the pen rather than a zone: at 16 m a
/// tile, this is 2 km on a side, and a bake of about a minute.
pub const MAX_TILES: u64 = 16_384;

/// The tiles over an area on the XZ plane, in metres, as a zone says it.
pub fn tiles_over(min: [f32; 2], max: [f32; 2], settings: &BakeSettings) -> Result<TileBounds, NavBakeError> {
    let size = settings.tile_cells as f32 * settings.cell_size;
    let low = min.map(|value| (value / size).floor() as i32);
    let high = max.map(|value| (value / size).floor() as i32);
    let side = |axis: usize| u64::from(high[axis].abs_diff(low[axis])) + 1;
    let tiles = side(0).saturating_mul(side(1));
    if tiles > MAX_TILES {
        return Err(NavBakeError::TooManyTiles { tiles, limit: MAX_TILES });
    }
    Ok((low, high))
}

/// What stops a character, of a collider set: the groups a bake reads.
pub fn solid_groups() -> sapphire_colliders::GroupMask {
    Group::World.mask().union(Group::CharacterBlocker.mask())
}

/// A zone's navmesh from its cooked colliders, the bytes the simulation reads: baked over the tiles of
/// `min` to `max`, and pruned to what `seeds` can walk to.
pub fn bake_colliders(
    colliders: &[u8],
    area: ([f32; 2], [f32; 2]),
    seeds: &[Seed],
    settings: &BakeSettings,
    cache: Option<&Cache>,
) -> Result<(Baked, IslandStats), NavBakeError> {
    let set: ColliderSet = ColliderSet::from_cooked(colliders)?;
    let bounds = tiles_over(area.0, area.1, settings)?;
    let triangles = colliders_to_triangles(&set, solid_groups());
    let mut baked = bake_cached(&triangles, bounds, settings, cache)?;
    let (mesh, islands) = keep_seeded(&baked.mesh, seeds)?;
    baked.mesh = mesh;
    Ok((baked, islands))
}

/// A bake that can't finish.
#[derive(Debug, thiserror::Error)]
pub enum NavBakeError {
    #[error("rerecast's {stage}: {message}")]
    Rerecast { stage: &'static str, message: String },
    #[error("there's no walkable ground to bake")]
    Nothing,
    #[error("a triangle at {y} m is past the {low} to {high} m a navmesh's rows hold")]
    OutOfRows { y: f32, low: f32, high: f32 },
    #[error("rows of {cell_height} m don't fit the height a bake covers in 16 bits")]
    RowsTooFine { cell_height: f32 },
    #[error("a seam left a polygon with {corners} corners")]
    TooManyCorners { corners: usize },
    #[error("a tile of {cells} cells a side is past `rerecast`'s limit of {limit}")]
    TileTooLarge { cells: i32, limit: i32 },
    #[error("tile {coord:?}'s contours cross or wind backward, even following the cells")]
    Tangled { coord: [i32; 2] },
    #[error("the area covers {tiles} tiles, more than the {limit} a zone may have")]
    TooManyTiles { tiles: u64, limit: u64 },
    #[error("`{field}` at {position:?} isn't on the navmesh: nothing a character could stand on lies there")]
    OffMesh { field: String, position: [f32; 3] },
    #[error("baking tile {coord:?} stopped: no tile finished for {seconds} s")]
    Stalled { coord: [i32; 2], seconds: u64 },
    #[error("baking tile {coord:?} panicked: {message}")]
    Panicked { coord: [i32; 2], message: String },
    #[error(transparent)]
    Colliders(#[from] ColliderError),
    #[error(transparent)]
    Graph(#[from] NavMeshError),
    #[error(transparent)]
    Cache(#[from] CacheError),
}
