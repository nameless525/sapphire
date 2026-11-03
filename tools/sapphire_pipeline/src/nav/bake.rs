//! One tile through `rerecast`, and a whole set of triangles through tiles and the stitch.

use std::collections::BTreeMap;

use glam::{UVec3, Vec3, Vec3A};
use rerecast::{
    Aabb3d, AreaType, BuildContoursFlags, HeightfieldBuilder, PolygonNavmesh, RegionId, RegionVertexId,
    TriMesh,
};
use sapphire_navmesh::NavMesh;
use sapphire_serialize::postcard;
use serde::{Deserialize, Serialize};

use super::NavBakeError;
use super::contours::{any_crossed, follow_cells, merge_holes};
use super::geometry::{Triangle, is_walkable};
use super::limit::{STALL_LIMIT, Stopped, with_limit};
use super::stitch::{SeamReport, stitch};
use crate::cache::{Cache, CacheKey, Stage};

/// One tile's bake, for its entry in the cache. Bump the version whenever a tile's bake changes for the
/// same triangles and settings. `rerecast`'s version is in here by being pinned (`nav_bake`'s test).
pub const TILE_BAKER: Stage = Stage { name: "nav tile", version: 1 };

/// How many threads a bake's tiles take at most, as a cook's own assets do.
const WORKERS: usize = 4;

/// What a bake is for: the character it's for, and how fine it looks.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BakeSettings {
    /// A cell's side on the XZ plane, in metres. A power of two, so tiles' corners are exact.
    pub cell_size: f32,
    /// A cell's height, in metres. A power of two for the same reason.
    pub cell_height: f32,
    /// A tile's side, in cells.
    pub tile_cells: i32,
    pub agent_radius: f32,
    pub agent_height: f32,
    /// The highest step a character climbs, in metres.
    pub climb: f32,
    /// The cosine of the steepest walkable slope.
    pub slope_cos: f32,
}

impl BakeSettings {
    /// The controller's default character: a capsule 0.3 m by 1.6 m, stepping 0.3 m, on slopes to 50°
    /// (`ControllerProfile`), in 16 m tiles.
    pub const CHARACTER: Self = Self {
        cell_size: 0.125,
        cell_height: 0.03125,
        tile_cells: 128,
        agent_radius: 0.3,
        agent_height: 1.6,
        climb: 0.3,
        // cos 50°, written out: `std`'s `cos` isn't pinned.
        slope_cos: 0.642_787_6,
    };

    fn tile_size(&self) -> f32 {
        self.tile_cells as f32 * self.cell_size
    }

    fn walkable_radius(&self) -> u16 {
        (self.agent_radius / self.cell_size).ceil() as u16
    }

    fn walkable_height(&self) -> u16 {
        (self.agent_height / self.cell_height).ceil() as u16
    }

    /// In whole cells of height.
    pub(super) fn walkable_climb(&self) -> u16 {
        (self.climb / self.cell_height).floor() as u16
    }

    /// How far past a tile's edge it reads, in cells: enough for erosion to see what's beyond, as
    /// Recast's own tile samples do.
    fn border(&self) -> u16 {
        self.walkable_radius() + 3
    }
}

/// The rows of cells every tile of every bake has, in metres: a vertex's height is counted from the first.
/// Fixed, rather than taken from what's being baked, so a tile's bake is a function of what reaches
/// that tile alone: a tower going up in one corner of a zone leaves every other tile's cells where they
/// were. Rows cost nothing until a span stands in one, as `rerecast` keeps columns of spans.
pub const ROWS: (f32, f32) = (-1000.0, 1000.0);
/// How many cells of height a tile can hold: `rerecast` keeps a span's floor and ceiling in 16 bits.
const MAX_ROWS: f32 = u16::MAX as f32;
/// `rerecast`'s defaults for what's left: 1 m² of voxels to keep a region and 6 m² to merge one at
/// these cells' sizes, and contour simplification in its recommended range.
const MIN_REGION_AREA: u16 = 64;
const MERGE_REGION_AREA: u16 = 400;
const MAX_SIMPLIFICATION_ERROR: f32 = 1.3;
/// The longest side a tile can have: `rerecast` 0.5 keeps a squared diagonal in 16 bits when it
/// triangulates, which wraps past about 181 cells on a side.
pub const MAX_TILE_CELLS: i32 = 180;
const MAX_EDGE_CELLS_PER_RADIUS: u16 = 8;
const MAX_CORNERS_PER_POLYGON: u16 = 6;

/// One tile's polygons, in whole cells from the bake's origin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub coord: [i32; 2],
    pub polygons: Vec<TilePolygon>,
    /// Whether its contours follow the cells, since simplifying them left one crossing itself.
    pub cell_exact: bool,
    /// Holes nothing could join, whose insides count as walkable.
    pub holes_lost: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TilePolygon {
    pub corners: Vec<[i32; 3]>,
    /// Beside `corners`: what's across the edge from this corner to the next.
    pub across: Vec<Across>,
}

impl TilePolygon {
    /// Whether no three corners in a row turn backward, as `sapphire_navmesh` will insist.
    fn winds_right(&self) -> bool {
        let n = self.corners.len();
        (0..n).all(|i| {
            let (a, b, c) = (self.corners[i], self.corners[(i + 1) % n], self.corners[(i + 2) % n]);
            i64::from(b[2] - a[2]) * i64::from(c[0] - a[0]) - i64::from(b[0] - a[0]) * i64::from(c[2] - a[2])
                >= 0
        })
    }
}

/// What a tile's polygon edge meets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Across {
    Wall,
    /// A polygon of the same tile.
    Polygon(u32),
    /// The tile's edge on this side: what's there is the next tile's to say.
    Seam(Seam),
}

/// A tile's sides, as `rerecast` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seam {
    LowX = 0,
    HighZ = 1,
    HighX = 2,
    LowZ = 3,
}

/// Which tiles a triangle's footprint reaches, borders included, as inclusive coordinates.
pub fn tile_range(triangle: &Triangle, settings: &BakeSettings) -> ([i32; 2], [i32; 2]) {
    let size = settings.tile_size();
    let border = f32::from(settings.border()) * settings.cell_size;
    let span = |axis: usize| {
        let low = triangle.iter().map(|c| c[axis]).fold(f32::MAX, f32::min);
        let high = triangle.iter().map(|c| c[axis]).fold(f32::MIN, f32::max);
        (((low - border) / size).floor() as i32, ((high + border) / size).floor() as i32)
    };
    let (x, z) = (span(0), span(2));
    ([x.0, z.0], [x.1, z.1])
}

/// Bakes one tile from the triangles that reach it. `None` if nothing in it can be walked on.
pub fn bake_tile(
    triangles: &[Triangle],
    coord: [i32; 2],
    settings: &BakeSettings,
) -> Result<Option<Tile>, NavBakeError> {
    if settings.tile_cells > MAX_TILE_CELLS {
        return Err(NavBakeError::TileTooLarge { cells: settings.tile_cells, limit: MAX_TILE_CELLS });
    }
    if (ROWS.1 - ROWS.0) / settings.cell_height > MAX_ROWS {
        return Err(NavBakeError::RowsTooFine { cell_height: settings.cell_height });
    }
    let failed = |stage: &'static str| {
        move |error: &dyn std::fmt::Display| NavBakeError::Rerecast { stage, message: error.to_string() }
    };

    let mut trimesh = TriMesh::default();
    let mut any_walkable = false;
    for (index, triangle) in triangles.iter().enumerate() {
        let first = index as u32 * 3;
        trimesh.vertices.extend(triangle.iter().map(|&corner| Vec3A::from_array(corner)));
        trimesh.indices.push(UVec3::new(first, first + 1, first + 2));
        let walkable = is_walkable(triangle, settings.slope_cos);
        any_walkable |= walkable;
        trimesh.area_types.push(if walkable { AreaType::DEFAULT_WALKABLE } else { AreaType::NOT_WALKABLE });
    }
    if !any_walkable {
        return Ok(None);
    }

    let border = settings.border();
    let cell = settings.cell_size;
    let corner = |tile: i32, border: i32| (tile * settings.tile_cells + border) as f32 * cell;
    let reach = i32::from(border);
    let aabb = Aabb3d {
        min: Vec3::new(corner(coord[0], -reach), ROWS.0, corner(coord[1], -reach)),
        max: Vec3::new(corner(coord[0] + 1, reach), ROWS.1, corner(coord[1] + 1, reach)),
    };

    let (height, climb, radius) =
        (settings.walkable_height(), settings.walkable_climb(), settings.walkable_radius());
    let mut heightfield = HeightfieldBuilder { aabb, cell_size: cell, cell_height: settings.cell_height }
        .build()
        .map_err(|error| failed("heightfield")(&error))?;
    heightfield.rasterize_triangles(&trimesh, climb).map_err(|error| failed("rasterizer")(&error))?;
    heightfield.filter_low_hanging_walkable_obstacles(climb);
    heightfield.filter_ledge_spans(height, climb);
    heightfield.filter_walkable_low_height_spans(height);

    let mut compact =
        heightfield.into_compact(height, climb).map_err(|error| failed("compact heightfield")(&error))?;
    compact.erode_walkable_area(radius);
    compact.build_distance_field();
    compact
        .build_regions(border, MIN_REGION_AREA, MERGE_REGION_AREA)
        .map_err(|error| failed("regions")(&error))?;
    let max_edge = radius * MAX_EDGE_CELLS_PER_RADIUS;
    let mut with_holes_lost = None;
    let mut last_error = None;
    // Simplified first. `rerecast` keeps an edge between two regions straight whatever the limit, so
    // one that bends can cross its own contour, and only following the cells gets it back.
    for cell_exact in [false, true] {
        let mut contours =
            compact.build_contours(MAX_SIMPLIFICATION_ERROR, max_edge, BuildContoursFlags::DEFAULT);
        if cell_exact {
            follow_cells(&mut contours);
        }
        if any_crossed(&contours) {
            continue;
        }
        let (_, holes_lost) = merge_holes(&mut contours);
        // Keeps `rerecast` 0.5 out of its vertex removal for these, which can hang (upstream's open #62).
        // A stitch cuts edges where its neighbour has corners, so the extra corners cost nothing there.
        for contour in &mut contours.contours {
            for (_, flags) in &mut contour.vertices {
                *flags &= !RegionVertexId::BORDER_VERTEX.bits();
            }
        }
        match contours.into_polygon_mesh(MAX_CORNERS_PER_POLYGON) {
            Err(error) => last_error = Some(failed("polygon mesh")(&error)),
            Ok(mesh) => {
                if mesh.polygon_count() == 0 {
                    return Ok(None);
                }
                let tile = tile_from(&mesh, coord, cell_exact, holes_lost as u32, settings);
                if !tile.polygons.iter().all(TilePolygon::winds_right) {
                    continue;
                }
                if holes_lost == 0 {
                    return Ok(Some(tile));
                }
                with_holes_lost.get_or_insert(tile);
            }
        }
    }
    match (with_holes_lost, last_error) {
        (Some(tile), _) => Ok(Some(tile)),
        (None, Some(error)) => Err(error),
        (None, None) => Err(NavBakeError::Tangled { coord }),
    }
}

/// `rerecast`'s tile in whole cells from the bake's origin, with its sides named.
fn tile_from(
    mesh: &PolygonNavmesh,
    coord: [i32; 2],
    cell_exact: bool,
    holes_lost: u32,
    settings: &BakeSettings,
) -> Tile {
    let nvp = mesh.max_vertices_per_polygon as usize;
    let border = RegionId::BORDER_REGION.bits();
    let mut polygons = Vec::with_capacity(mesh.polygon_count());
    for polygon in 0..mesh.polygon_count() {
        let vertices = &mesh.polygons[polygon * nvp..(polygon + 1) * nvp];
        let neighbours = &mesh.polygon_neighbors[polygon * nvp..(polygon + 1) * nvp];
        let count = vertices.iter().take_while(|&&v| v != PolygonNavmesh::NO_INDEX).count();
        let corners = vertices[..count]
            .iter()
            .map(|&v| {
                let at = mesh.vertices[v as usize];
                [
                    coord[0] * settings.tile_cells + i32::from(at.x),
                    i32::from(at.y),
                    coord[1] * settings.tile_cells + i32::from(at.z),
                ]
            })
            .collect();
        let across = neighbours[..count]
            .iter()
            .map(|&neighbour| {
                if neighbour == PolygonNavmesh::NO_CONNECTION {
                    Across::Wall
                } else if neighbour & border != 0 {
                    Across::Seam(match neighbour & 3 {
                        0 => Seam::LowX,
                        1 => Seam::HighZ,
                        2 => Seam::HighX,
                        _ => Seam::LowZ,
                    })
                } else {
                    Across::Polygon(u32::from(neighbour))
                }
            })
            .collect();
        polygons.push(TilePolygon { corners, across });
    }
    Tile { coord, polygons, cell_exact, holes_lost }
}

/// A finished bake.
#[derive(Clone, Debug)]
pub struct Baked {
    pub mesh: NavMesh,
    pub seams: SeamReport,
    pub stats: BakeStats,
}

/// What a bake read and made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BakeStats {
    pub triangles: usize,
    /// Tiles something reached, and tiles that came out with polygons.
    pub tiles_read: usize,
    pub tiles_kept: usize,
    /// Tiles the cache had, whose triangles and settings hadn't changed since they baked.
    pub tiles_cached: usize,
    /// Tiles whose contours had to follow the cells, and holes lost in spite of that.
    pub tiles_cell_exact: usize,
    pub holes_lost: usize,
}

/// Which tiles to bake, as inclusive coordinates: a zone says where a character can be, since the
/// ground drawn past that can go on for kilometres.
pub type TileBounds = ([i32; 2], [i32; 2]);

/// What a tile's bake is keyed by: its place, what it's for and every bit of what reaches it.
fn tile_key(coord: [i32; 2], triangles: &[Triangle], settings: &BakeSettings) -> CacheKey {
    let mut hash = blake3::Hasher::new();
    hash.update(&coord[0].to_le_bytes());
    hash.update(&coord[1].to_le_bytes());
    hash.update(&postcard::to_allocvec(settings).expect("settings always encode"));
    for value in triangles.iter().flatten().flatten() {
        hash.update(&value.to_bits().to_le_bytes());
    }
    CacheKey::tile(&hash.finalize(), TILE_BAKER)
}

/// Bakes triangles into one graph, a tile at a time, within `bounds`, baking every tile.
pub fn bake(
    triangles: &[Triangle],
    bounds: TileBounds,
    settings: &BakeSettings,
) -> Result<Baked, NavBakeError> {
    bake_cached(triangles, bounds, settings, None)
}

/// A tile for the workers.
struct TileJob {
    coord: [i32; 2],
    triangles: Vec<Triangle>,
}

/// [`bake`], taking from `cache` every tile whose triangles it has seen, and filling it with the rest. The
/// stitch always runs, and takes a millisecond or so.
pub fn bake_cached(
    triangles: &[Triangle],
    bounds: TileBounds,
    settings: &BakeSettings,
    cache: Option<&Cache>,
) -> Result<Baked, NavBakeError> {
    // By (z, x), and each tile's triangles in the input's order, so nothing depends on a hash.
    let mut by_tile: BTreeMap<(i32, i32), Vec<Triangle>> = BTreeMap::new();
    for triangle in triangles {
        let (low, high) = tile_range(triangle, settings);
        for z in low[1].max(bounds.0[1])..=high[1].min(bounds.1[1]) {
            for x in low[0].max(bounds.0[0])..=high[0].min(bounds.1[0]) {
                by_tile.entry((z, x)).or_default().push(*triangle);
            }
        }
    }
    if by_tile.is_empty() {
        return Err(NavBakeError::Nothing);
    }
    let tiles_read = by_tile.len();
    let mut found: BTreeMap<(i32, i32), Option<Tile>> = BTreeMap::new();
    let mut jobs = Vec::new();
    for ((z, x), triangles) in by_tile {
        if let Some(&y) =
            triangles.iter().flatten().map(|corner| &corner[1]).find(|y| !(ROWS.0..=ROWS.1).contains(*y))
        {
            return Err(NavBakeError::OutOfRows { y, low: ROWS.0, high: ROWS.1 });
        }
        if let Some(cache) = cache
            && let Some(bytes) = cache.get(&tile_key([x, z], &triangles, settings))?
            && let Ok(tile) = postcard::from_bytes(&bytes)
        {
            found.insert((z, x), tile);
            continue;
        }
        jobs.push(TileJob { coord: [x, z], triangles });
    }
    let tiles_cached = found.len();
    let keys: Option<Vec<CacheKey>> =
        cache.map(|_| jobs.iter().map(|job| tile_key(job.coord, &job.triangles, settings)).collect());
    let coords: Vec<[i32; 2]> = jobs.iter().map(|job| job.coord).collect();
    let workers = std::thread::available_parallelism().map_or(1, |count| count.get()).min(WORKERS);
    let on_threads = *settings;
    let baked = with_limit(jobs, workers, STALL_LIMIT, move |job: &TileJob| {
        bake_tile(&job.triangles, job.coord, &on_threads)
    })
    .map_err(|stopped| match stopped {
        Stopped::Stalled { index } => {
            NavBakeError::Stalled { coord: coords[index], seconds: STALL_LIMIT.as_secs() }
        }
        Stopped::Panicked { index, message } => NavBakeError::Panicked { coord: coords[index], message },
    })?;
    // The lowest tile's error, whichever thread came to it first.
    for (index, tile) in baked.into_iter().enumerate() {
        let tile = tile?;
        if let (Some(cache), Some(keys)) = (cache, &keys) {
            cache.put(&keys[index], &postcard::to_allocvec(&tile).expect("a tile always encodes"))?;
        }
        found.insert((coords[index][1], coords[index][0]), tile);
    }
    let tiles: Vec<Tile> = found.into_values().flatten().collect();
    let stats = BakeStats {
        triangles: triangles.len(),
        tiles_read,
        tiles_kept: tiles.len(),
        tiles_cached,
        tiles_cell_exact: tiles.iter().filter(|tile| tile.cell_exact).count(),
        holes_lost: tiles.iter().map(|tile| tile.holes_lost as usize).sum(),
    };
    let (mesh, seams) = stitch(&tiles, settings)?;
    Ok(Baked { mesh, seams, stats })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_past_what_rerecast_can_triangulate_is_refused() {
        let settings = BakeSettings { tile_cells: MAX_TILE_CELLS + 1, ..BakeSettings::CHARACTER };
        let refused = bake_tile(&[], [0, 0], &settings);
        assert!(matches!(refused, Err(NavBakeError::TileTooLarge { .. })), "{refused:?}");
    }

    #[test]
    fn rows_too_fine_for_16_bits_are_refused() {
        let settings = BakeSettings { cell_height: 1.0 / 64.0, ..BakeSettings::CHARACTER };
        let refused = bake_tile(&[], [0, 0], &settings);
        assert!(matches!(refused, Err(NavBakeError::RowsTooFine { .. })), "{refused:?}");
    }

    /// A square of ground, `half` metres to each side of the origin, at height `y`, wound up.
    fn ground(half: f32, y: f32) -> Vec<Triangle> {
        let (a, b, c, d) = ([-half, y, -half], [-half, y, half], [half, y, half], [half, y, -half]);
        vec![[a, b, c], [a, c, d]]
    }

    #[test]
    fn ground_past_the_rows_is_refused_and_only_where_it_counts() {
        let settings = BakeSettings::CHARACTER;
        let mountain = ground(1.0, ROWS.1 + 100.0).into_iter().map(|t| t.map(|[x, y, z]| [x + 500.0, y, z]));
        let triangles: Vec<Triangle> = ground(8.0, 0.0).into_iter().chain(mountain).collect();
        let near = ([-1, -1], [0, 0]);
        bake(&triangles, near, &settings).expect("past the bounds, a mountain's height doesn't matter");
        let refused = bake(&triangles, ([-1, -1], [40, 0]), &settings);
        assert!(matches!(refused, Err(NavBakeError::OutOfRows { .. })), "{refused:?}");
    }

    /// Two bakes of what differs in one tile's worth of triangles: the second bakes only tiles those reach,
    /// and the mesh is what a whole bake makes, which is the promise a cache can break quietly.
    #[test]
    fn a_change_rebakes_only_the_tiles_it_reaches_and_comes_to_the_same_bytes() {
        let dir = crate::test_util::TempDir::new("nav-tile-cache");
        let cache = Cache::new(dir.path());
        let settings = BakeSettings::CHARACTER;
        let bounds = ([-4, -4], [3, 3]);
        let mut triangles = ground(64.0, 0.0);
        let first = bake_cached(&triangles, bounds, &settings, Some(&cache)).expect("it bakes");
        assert_eq!(first.stats.tiles_cached, 0);
        assert_eq!(first.stats.tiles_read, 64);

        let again = bake_cached(&triangles, bounds, &settings, Some(&cache)).expect("it bakes");
        assert_eq!(again.stats.tiles_cached, 64, "nothing changed, and nothing bakes");
        assert_eq!(again.mesh, first.mesh);

        // A platform well inside tile (0, 0), its borders staying in it, with room to walk under.
        let (low, high) = (4.0, 8.0);
        let top = 2.0;
        let (a, b, c, d) = ([low, top, low], [low, top, high], [high, top, high], [high, top, low]);
        triangles.extend([[a, b, c], [a, c, d]]);
        let edited = bake_cached(&triangles, bounds, &settings, Some(&cache)).expect("it bakes");
        assert_eq!(edited.stats.tiles_cached, 63, "only what the platform reaches");
        assert_eq!(edited.mesh, bake(&triangles, bounds, &settings).expect("it bakes").mesh);
        assert_ne!(edited.mesh, first.mesh);
    }

    /// Nothing but a tile's own triangles and settings say what's in it, or a cache would hand out a
    /// tile baked for something else.
    #[test]
    fn a_tiles_key_holds_its_place_its_settings_and_its_triangles() {
        let settings = BakeSettings::CHARACTER;
        let triangles = ground(1.0, 0.0);
        let key = tile_key([0, 0], &triangles, &settings);
        assert_eq!(key, tile_key([0, 0], &triangles, &settings));
        assert_ne!(key, tile_key([1, 0], &triangles, &settings));
        assert_ne!(key, tile_key([0, 1], &triangles, &settings));
        assert_ne!(key, tile_key([0, 0], &ground(1.0, 0.5), &settings));
        assert_ne!(key, tile_key([0, 0], &triangles, &BakeSettings { climb: 0.2, ..settings }));
    }

    #[test]
    fn a_tile_survives_the_cache() {
        let tile = Tile {
            coord: [-3, 4],
            polygons: vec![TilePolygon {
                corners: vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]],
                across: vec![Across::Wall, Across::Polygon(7), Across::Seam(Seam::HighZ)],
            }],
            cell_exact: true,
            holes_lost: 2,
        };
        let some = Some(tile);
        let bytes = postcard::to_allocvec(&some).expect("it encodes");
        assert_eq!(postcard::from_bytes::<Option<Tile>>(&bytes).expect("it decodes"), some);
    }
}
