//! One tile through `rerecast`, and a whole set of triangles through tiles and the stitch.

use std::collections::BTreeMap;

use glam::{UVec3, Vec3, Vec3A};
use rerecast::{
    Aabb3d, AreaType, BuildContoursFlags, HeightfieldBuilder, PolygonNavmesh, RegionId, RegionVertexId,
    TriMesh,
};
use sapphire_navmesh::NavMesh;

use super::NavBakeError;
use super::contours::{any_crossed, follow_cells, merge_holes};
use super::geometry::{Triangle, is_walkable};
use super::stitch::{SeamReport, stitch};

/// What a bake is for: the character it's for, and how fine it looks.
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// `rerecast`'s defaults for what's left: 1 m² of voxels to keep a region and 6 m² to merge one at
/// these cells' sizes, and contour simplification in its recommended range.
/// How many cells of height a tile can hold: `rerecast` keeps a span's floor and ceiling in 16 bits.
const MAX_ROWS: u16 = u16::MAX;
const MIN_REGION_AREA: u16 = 64;
const MERGE_REGION_AREA: u16 = 400;
const MAX_SIMPLIFICATION_ERROR: f32 = 1.3;
/// The longest side a tile can have: `rerecast` 0.5 keeps a squared diagonal in 16 bits when it
/// triangulates, which wraps past about 181 cells on a side.
pub const MAX_TILE_CELLS: i32 = 180;
const MAX_EDGE_CELLS_PER_RADIUS: u16 = 8;
const MAX_CORNERS_PER_POLYGON: u16 = 6;

/// One tile's polygons, in whole cells from the bake's origin.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub coord: [i32; 2],
    pub polygons: Vec<TilePolygon>,
    /// Whether its contours follow the cells, since simplifying them left one crossing itself.
    pub cell_exact: bool,
    /// Holes nothing could join, whose insides count as walkable.
    pub holes_lost: u32,
}

#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Across {
    Wall,
    /// A polygon of the same tile.
    Polygon(u32),
    /// The tile's edge on this side: what's there is the next tile's to say.
    Seam(Seam),
}

/// A tile's sides, as `rerecast` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seam {
    LowX = 0,
    HighZ = 1,
    HighX = 2,
    LowZ = 3,
}

/// How far up a bake's rows start, in metres, and how tall they stand: the same for every tile, so
/// a vertex's height means one thing throughout.
pub fn world_rows(triangles: &[Triangle], settings: &BakeSettings) -> Result<(f32, f32), NavBakeError> {
    let mut low = f32::MAX;
    let mut high = f32::MIN;
    for corner in triangles.iter().flatten() {
        low = low.min(corner[1]);
        high = high.max(corner[1]);
    }
    if low > high {
        return Err(NavBakeError::Nothing);
    }
    let origin = (low / settings.cell_height).floor() * settings.cell_height;
    let top = high + settings.agent_height;
    let limit = MAX_ROWS as f32 * settings.cell_height;
    if top - origin > limit {
        return Err(NavBakeError::TooTall { height: top - origin, limit });
    }
    Ok((origin, top))
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
    rows: (f32, f32),
    settings: &BakeSettings,
) -> Result<Option<Tile>, NavBakeError> {
    if settings.tile_cells > MAX_TILE_CELLS {
        return Err(NavBakeError::TileTooLarge { cells: settings.tile_cells, limit: MAX_TILE_CELLS });
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
        min: Vec3::new(corner(coord[0], -reach), rows.0, corner(coord[1], -reach)),
        max: Vec3::new(corner(coord[0] + 1, reach), rows.1, corner(coord[1] + 1, reach)),
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
    /// Tiles whose contours had to follow the cells, and holes lost in spite of that.
    pub tiles_cell_exact: usize,
    pub holes_lost: usize,
}

/// Which tiles to bake, as inclusive coordinates: a zone says where a character can be, since the
/// ground drawn past that can go on for kilometres.
pub type TileBounds = ([i32; 2], [i32; 2]);

/// Bakes triangles into one graph, a tile at a time, in coordinate order, within `bounds`.
pub fn bake(
    triangles: &[Triangle],
    bounds: TileBounds,
    settings: &BakeSettings,
) -> Result<Baked, NavBakeError> {
    let rows = world_rows(triangles, settings)?;
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
    let tiles_read = by_tile.len();
    let mut tiles = Vec::new();
    for ((z, x), triangles) in &by_tile {
        if let Some(tile) = bake_tile(triangles, [*x, *z], rows, settings)? {
            tiles.push(tile);
        }
    }
    let stats = BakeStats {
        triangles: triangles.len(),
        tiles_read,
        tiles_kept: tiles.len(),
        tiles_cell_exact: tiles.iter().filter(|tile| tile.cell_exact).count(),
        holes_lost: tiles.iter().map(|tile| tile.holes_lost as usize).sum(),
    };
    let (mesh, seams) = stitch(&tiles, rows.0, settings)?;
    Ok(Baked { mesh, seams, stats })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_past_what_rerecast_can_triangulate_is_refused() {
        let settings = BakeSettings { tile_cells: MAX_TILE_CELLS + 1, ..BakeSettings::CHARACTER };
        let refused = bake_tile(&[], [0, 0], (0.0, 2.0), &settings);
        assert!(matches!(refused, Err(NavBakeError::TileTooLarge { .. })), "{refused:?}");
    }
}
