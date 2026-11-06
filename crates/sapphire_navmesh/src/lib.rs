//! A zone's walkable polygons as plain data: what the cooker writes and the simulation reads.
//!
//! The cooker must never link the simulation, and the simulation must never link a baker (the lesson
//! of #74), so the format they agree on lives below them both, as `sapphire_colliders`' does.
//!
//! # Owns
//!
//! - **The cooked format:** [`NavMesh`], an rkyv archive (`.snav`), one flat polygon graph per zone.
//!   A tile is the cook's unit, not the runtime's: the simulation streams nothing, so a zone's tiles
//!   are one graph before its first step, and a seam is an ordinary shared edge by then.
//! - **Its checks:** [`ArchivedNavMesh::validate`], whatever wrote the bytes.
//!
//! # Does not own
//!
//! - Baking and stitching: `sapphire_pipeline`'s `nav` module, on `rerecast`.
//! - Point location and paths: `sapphire_nav`.
//!
//! # Determinism
//!
//! Vertices are whole cells, so nothing in the graph's shape is a float. A vertex's position is
//! `origin + cell × size`, in plain `+` and `*`, which every architecture rounds alike. Nothing here
//! depends on hash order or addresses.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::CookedError;
use sapphire_serialize::cooked::AlignedVec;

/// Conventional file extension for cooked navigation meshes.
pub const NAVMESH_EXTENSION: &str = "snav";

/// In [`NavMesh::neighbours`]: nothing walkable lies across this edge.
pub const NO_NEIGHBOUR: u32 = u32::MAX;

/// Most corners one polygon may have: `rerecast`'s six, plus what a seam can add along an edge.
pub const MAX_CORNERS: usize = 32;

/// A zone's polygon graph. See the [crate docs](self).
///
/// Corners wind so that `cross(b − a, c − a)` on the XZ plane, written `(b.z − a.z) × (c.x − a.x) −
/// (b.x − a.x) × (c.z − a.z)`, isn't negative for any three in order. A seam can leave three in a line.
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::navmesh"]
pub struct NavMesh {
    /// Where cell `[0, 0, 0]` is, in metres.
    pub origin: [f32; 3],
    /// A cell's side on the XZ plane, in metres.
    pub cell_size: f32,
    /// A cell's height, in metres.
    pub cell_height: f32,
    /// Vertices, in whole cells from [`Self::origin`].
    pub vertices: Vec<[i32; 3]>,
    /// Where each polygon's corners start in [`Self::corners`], with one more entry at the end.
    pub polygon_starts: Vec<u32>,
    /// A polygon's corners, as indices into [`Self::vertices`].
    pub corners: Vec<u32>,
    /// Beside [`Self::corners`]: the polygon across the edge from this corner to the next, or
    /// [`NO_NEIGHBOUR`].
    pub neighbours: Vec<u32>,
}

impl Versioned for NavMesh {
    /// 1: M3 P0.
    const VERSION: u32 = 1;
}

/// Cooked bytes that can't be walked.
#[derive(Debug, thiserror::Error)]
pub enum NavMeshError {
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("cells of {size} by {height} m aren't usable")]
    Cells { size: f32, height: f32 },
    #[error("the origin isn't finite")]
    Origin,
    #[error("vertex {vertex} does not have a finite world position")]
    Position { vertex: usize },
    #[error("polygon {polygon}'s corners don't run in order inside the list")]
    Starts { polygon: usize },
    #[error("polygon {polygon} has {corners} corners, not 3 to {MAX_CORNERS}")]
    Corners { polygon: usize, corners: usize },
    #[error("polygon {polygon} names vertex {vertex}, of {vertices}")]
    Vertex { polygon: usize, vertex: u32, vertices: usize },
    #[error("polygon {polygon} winds the wrong way, or isn't convex")]
    Winding { polygon: usize },
    #[error("polygon {polygon} names neighbour {neighbour}, of {polygons}")]
    Neighbour { polygon: usize, neighbour: u32, polygons: usize },
    #[error("polygon {polygon} names {neighbour} across an edge {neighbour} doesn't share back")]
    OneWay { polygon: usize, neighbour: u32 },
    #[error("{corners} corners with {neighbours} neighbours")]
    Lengths { corners: usize, neighbours: usize },
}

impl NavMesh {
    pub fn to_cooked(&self) -> Result<AlignedVec, NavMeshError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and the graph
    /// ([`ArchivedNavMesh::validate`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedNavMesh, NavMeshError> {
        let mesh = sapphire_serialize::access::<Self>(bytes)?;
        mesh.validate()?;
        Ok(mesh)
    }

    /// How many polygons there are.
    pub fn polygon_count(&self) -> usize {
        self.polygon_starts.len().saturating_sub(1)
    }
}

impl ArchivedNavMesh {
    /// How many polygons there are.
    pub fn polygon_count(&self) -> usize {
        self.polygon_starts.len().saturating_sub(1)
    }

    /// Where cell `[0, 0, 0]` is, in metres.
    pub fn origin(&self) -> [f32; 3] {
        self.origin.map(|v| v.to_native())
    }

    /// A vertex, in whole cells.
    pub fn vertex(&self, index: usize) -> [i32; 3] {
        self.vertices[index].map(|v| v.to_native())
    }

    /// The range of a polygon's corners in the corner lists.
    pub fn corner_range(&self, polygon: usize) -> std::ops::Range<usize> {
        self.polygon_starts[polygon].to_native() as usize
            ..self.polygon_starts[polygon + 1].to_native() as usize
    }

    /// Checks everything the runtime relies on, whatever wrote these bytes: every index in range,
    /// every polygon convex and wound one way, and every shared edge shared from both sides.
    pub fn validate(&self) -> Result<(), NavMeshError> {
        let (size, height) = (self.cell_size.to_native(), self.cell_height.to_native());
        if !(size.is_finite() && height.is_finite() && size > 0.0 && height > 0.0) {
            return Err(NavMeshError::Cells { size, height });
        }
        if !self.origin().iter().all(|v| v.is_finite()) {
            return Err(NavMeshError::Origin);
        }
        let origin = self.origin();
        for index in 0..self.vertices.len() {
            let cell = self.vertex(index);
            if !(0..3).all(|axis| {
                let scale = if axis == 1 { height } else { size };
                (origin[axis] + cell[axis] as f32 * scale).is_finite()
            }) {
                return Err(NavMeshError::Position { vertex: index });
            }
        }
        if self.corners.len() != self.neighbours.len() {
            return Err(NavMeshError::Lengths {
                corners: self.corners.len(),
                neighbours: self.neighbours.len(),
            });
        }
        let polygons = self.polygon_count();
        let mut previous_end = 0;
        for polygon in 0..polygons {
            let start = self.polygon_starts[polygon].to_native() as usize;
            let end = self.polygon_starts[polygon + 1].to_native() as usize;
            if start != previous_end || end < start || end > self.corners.len() {
                return Err(NavMeshError::Starts { polygon });
            }
            previous_end = end;
            let corners = end - start;
            if !(3..=MAX_CORNERS).contains(&corners) {
                return Err(NavMeshError::Corners { polygon, corners });
            }
            for corner in &self.corners[start..end] {
                let vertex = corner.to_native();
                if vertex as usize >= self.vertices.len() {
                    return Err(NavMeshError::Vertex { polygon, vertex, vertices: self.vertices.len() });
                }
            }
            self.check_winding(polygon, start, corners)?;
        }
        if previous_end != self.corners.len()
            || self.polygon_starts.first().is_some_and(|v| v.to_native() != 0)
        {
            return Err(NavMeshError::Starts { polygon: polygons });
        }
        for polygon in 0..polygons {
            for corner in self.corner_range(polygon) {
                let neighbour = self.neighbours[corner].to_native();
                if neighbour == NO_NEIGHBOUR {
                    continue;
                }
                if neighbour as usize >= polygons {
                    return Err(NavMeshError::Neighbour { polygon, neighbour, polygons });
                }
                let range = self.corner_range(polygon);
                let next = if corner + 1 == range.end { range.start } else { corner + 1 };
                let a = self.vertex(self.corners[corner].to_native() as usize);
                let b = self.vertex(self.corners[next].to_native() as usize);
                // A baked step can have different heights on the two sides. Its projected edge
                // still agrees exactly; climb acceptance belongs to the baker.
                let projected = |v: [i32; 3]| [v[0], v[2]];
                let other_range = self.corner_range(neighbour as usize);
                let back = neighbour as usize != polygon
                    && other_range.clone().any(|other| {
                        let next = if other + 1 == other_range.end { other_range.start } else { other + 1 };
                        self.neighbours[other].to_native() == polygon as u32
                            && projected(self.vertex(self.corners[other].to_native() as usize))
                                == projected(b)
                            && projected(self.vertex(self.corners[next].to_native() as usize)) == projected(a)
                    });
                if !back {
                    return Err(NavMeshError::OneWay { polygon, neighbour });
                }
            }
        }
        Ok(())
    }

    fn check_winding(&self, polygon: usize, start: usize, corners: usize) -> Result<(), NavMeshError> {
        let at = |i: usize| self.vertex(self.corners[start + i % corners].to_native() as usize);
        let cross = |a: [i32; 3], b: [i32; 3], c: [i32; 3]| {
            let dx = i128::from(b[0]) - i128::from(a[0]);
            let dz = i128::from(b[2]) - i128::from(a[2]);
            dz * (i128::from(c[0]) - i128::from(a[0])) - dx * (i128::from(c[2]) - i128::from(a[2]))
        };
        let mut area = 0i128;
        for i in 0..corners {
            let (a, b) = (at(i), at(i + 1));
            // Distinct projected corners rule out zero edges and polygons that walk an outline
            // more than once. Checking every corner against every edge also rejects star polygons
            // whose consecutive turns alone can all have the expected sign.
            for j in 0..corners {
                let c = at(j);
                if (j != i && a[0] == c[0] && a[2] == c[2]) || cross(a, b, c) < 0 {
                    return Err(NavMeshError::Winding { polygon });
                }
            }
            area += cross(at(0), a, b);
        }
        if area == 0 {
            return Err(NavMeshError::Winding { polygon });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two unit squares side by side, sharing the edge at x = 1.
    fn two_squares() -> NavMesh {
        NavMesh {
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            cell_height: 1.0,
            vertices: vec![[0, 0, 0], [0, 0, 1], [1, 0, 1], [1, 0, 0], [2, 0, 1], [2, 0, 0]],
            polygon_starts: vec![0, 4, 8],
            corners: vec![0, 1, 2, 3, 3, 2, 4, 5],
            neighbours: vec![
                NO_NEIGHBOUR,
                NO_NEIGHBOUR,
                1,
                NO_NEIGHBOUR,
                0,
                NO_NEIGHBOUR,
                NO_NEIGHBOUR,
                NO_NEIGHBOUR,
            ],
        }
    }

    #[test]
    fn a_neighbour_must_share_this_edge_and_cannot_be_itself() {
        for self_link in [false, true] {
            let mut mesh = two_squares();
            if self_link {
                mesh.neighbours[2] = 0;
            } else {
                mesh.neighbours[4] = NO_NEIGHBOUR;
                mesh.neighbours[5] = 0;
            }
            assert!(matches!(
                NavMesh::read(&mesh.to_cooked().expect("write")),
                Err(NavMeshError::OneWay { .. })
            ));
        }
    }

    #[test]
    fn shared_edges_agree_in_projection_even_across_a_step() {
        let mut mesh = two_squares();
        mesh.vertices.extend([[1, 1, 0], [1, 1, 1]]);
        mesh.corners[4] = 6;
        mesh.corners[5] = 7;
        assert!(NavMesh::read(&mesh.to_cooked().expect("write")).is_ok());
        mesh.vertices[6][0] += 9;
        mesh.vertices[7][0] += 9;
        mesh.vertices[4][0] += 9;
        mesh.vertices[5][0] += 9;
        assert!(matches!(NavMesh::read(&mesh.to_cooked().expect("write")), Err(NavMeshError::OneWay { .. })));
    }

    #[test]
    fn polygons_need_a_distinct_convex_outline_with_area() {
        for vertices in [
            vec![[0, 0, 0], [0, 0, 0], [0, 0, 0]],
            vec![[0, 0, 0], [1, 0, 0], [2, 0, 0]],
            vec![[0, 0, 0], [1, 0, 1], [0, 0, 1], [1, 0, 0]],
            vec![[0, 0, 0], [0, 0, 1], [1, 0, 1], [0, 0, 0], [1, 0, 0]],
        ] {
            let count = vertices.len() as u32;
            let mesh = NavMesh {
                vertices,
                cell_size: 1.0,
                cell_height: 1.0,
                polygon_starts: vec![0, count],
                corners: (0..count).collect(),
                neighbours: vec![NO_NEIGHBOUR; count as usize],
                ..NavMesh::default()
            };
            assert!(matches!(
                NavMesh::read(&mesh.to_cooked().expect("write")),
                Err(NavMeshError::Winding { .. })
            ));
        }
        // A seam may split a straight edge without making the polygon degenerate.
        let mut mesh = two_squares();
        mesh.neighbours.fill(NO_NEIGHBOUR);
        mesh.polygon_starts = vec![0, 5];
        mesh.vertices = vec![[0, 0, 0], [0, 0, 1], [0, 0, 2], [1, 0, 2], [1, 0, 0]];
        mesh.corners = vec![0, 1, 2, 3, 4];
        mesh.neighbours.truncate(5);
        assert!(NavMesh::read(&mesh.to_cooked().expect("write")).is_ok());
    }

    #[test]
    fn exact_cell_orientation_widens_before_subtracting() {
        let mut mesh = two_squares();
        mesh.vertices = vec![
            [i32::MIN, 0, i32::MIN],
            [i32::MIN, 0, i32::MAX],
            [i32::MAX, 0, i32::MAX],
            [i32::MAX, 0, i32::MIN],
        ];
        mesh.cell_size = 1e-8;
        mesh.polygon_starts = vec![0, 4];
        mesh.corners = vec![0, 1, 2, 3];
        mesh.neighbours = vec![NO_NEIGHBOUR; 4];
        assert!(NavMesh::read(&mesh.to_cooked().expect("write")).is_ok());
        mesh.cell_size = f32::MAX;
        assert!(matches!(
            NavMesh::read(&mesh.to_cooked().expect("write")),
            Err(NavMeshError::Position { .. })
        ));
    }

    #[test]
    fn a_cooked_mesh_reads_back_in_place() {
        let bytes = two_squares().to_cooked().expect("write");
        let read = NavMesh::read(&bytes).expect("read");
        assert_eq!(read.polygon_count(), 2);
        assert_eq!(read.vertex(4), [2, 0, 1]);
        assert_eq!(read.corner_range(1), 4..8);
    }

    /// Whatever wrote these, `read` must refuse them: a pack's bytes needn't come from our cooker.
    #[test]
    fn cooked_data_the_runtime_cant_walk_is_refused() {
        type Tamper = fn(&mut NavMesh);
        let cases: [(&str, &str, Tamper); 7] = [
            ("cell", "cells", |mesh| mesh.cell_size = 0.0),
            ("origin", "origin", |mesh| mesh.origin[1] = f32::NAN),
            ("starts", "in order", |mesh| mesh.polygon_starts[1] = 9),
            ("vertex", "names vertex", |mesh| mesh.corners[0] = 99),
            ("winding", "winds", |mesh| mesh.corners[..4].reverse()),
            ("neighbour", "names neighbour", |mesh| mesh.neighbours[2] = 7),
            ("one way", "doesn't share", |mesh| mesh.neighbours[4] = NO_NEIGHBOUR),
        ];
        for (what, expected, tamper) in cases {
            let mut tampered = two_squares();
            tamper(&mut tampered);
            let bytes = tampered.to_cooked().expect("write");
            let message = NavMesh::read(&bytes).expect_err(what).to_string();
            assert!(message.contains(expected), "{what}: `{message}` must say `{expected}`");
        }
    }

    #[test]
    fn the_winding_the_docs_name_is_the_one_checked() {
        let mesh = two_squares();
        let bytes = mesh.to_cooked().expect("write");
        assert!(NavMesh::read(&bytes).is_ok(), "the test's squares wind as documented");
    }
}
