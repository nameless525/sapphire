//! Which of a bake's ground is real.
//!
//! A bake finds every flat top a character could stand on, and a stall's roof is as flat as a street.
//! A zone says what's real by where things start: its spawners, and the seeds it lists for ground no
//! spawner reaches. What a character can walk to from one of those stays, and every other island goes,
//! so that a search for somewhere on a roof fails at the nearest street rather than spending a tick's
//! nodes on finding no way.

use glam::Vec3;
use sapphire_nav::{NavGraph, Reach};
use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};

use super::NavBakeError;

/// Somewhere a zone says is on the ground: a character stands here at some point.
#[derive(Clone, Debug, PartialEq)]
pub struct Seed {
    /// The document's field it comes from, for what a refusal says.
    pub field: String,
    pub position: [f32; 3],
}

/// What pruning kept and dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IslandStats {
    pub polygons_kept: usize,
    pub polygons_dropped: usize,
    pub islands_dropped: usize,
}

/// `mesh` with only what can be walked to from a seed. Polygons and vertices keep their relative
/// order, so this adds no order of its own. A seed nothing lies under fails, by its field, so a spawner
/// off the mesh is a failed cook rather than an enemy that stands still in a shipped build.
pub fn keep_seeded(mesh: &NavMesh, seeds: &[Seed]) -> Result<(NavMesh, IslandStats), NavBakeError> {
    if mesh.polygon_count() == 0 {
        return Err(NavBakeError::Nothing);
    }
    let cooked = mesh.to_cooked()?;
    let graph = NavGraph::new(NavMesh::read(&cooked)?);

    let count = mesh.polygon_count();
    let mut kept = vec![false; count];
    let mut stack = Vec::new();
    for seed in seeds {
        let Some(location) = graph.locate(Vec3::from(seed.position), Reach::CHARACTER) else {
            return Err(NavBakeError::OffMesh { field: seed.field.clone(), position: seed.position });
        };
        flood(&graph, location.polygon, &mut kept, &mut stack);
    }

    let mut seen = kept.clone();
    let mut islands_dropped = 0;
    for polygon in 0..count {
        if !seen[polygon] {
            islands_dropped += 1;
            flood(&graph, polygon as u32, &mut seen, &mut stack);
        }
    }

    let mut new_id = vec![NO_NEIGHBOUR; count];
    let mut polygons_kept = 0;
    for (polygon, _) in kept.iter().enumerate().filter(|(_, kept)| **kept) {
        new_id[polygon] = polygons_kept;
        polygons_kept += 1;
    }
    let mut new_vertex = vec![NO_NEIGHBOUR; mesh.vertices.len()];
    let mut pruned = NavMesh {
        origin: mesh.origin,
        cell_size: mesh.cell_size,
        cell_height: mesh.cell_height,
        vertices: Vec::new(),
        polygon_starts: vec![0],
        corners: Vec::new(),
        neighbours: Vec::new(),
    };
    for (polygon, _) in kept.iter().enumerate().filter(|(_, kept)| **kept) {
        let range = mesh.polygon_starts[polygon] as usize..mesh.polygon_starts[polygon + 1] as usize;
        for corner in range {
            let vertex = mesh.corners[corner] as usize;
            if new_vertex[vertex] == NO_NEIGHBOUR {
                new_vertex[vertex] = pruned.vertices.len() as u32;
                pruned.vertices.push(mesh.vertices[vertex]);
            }
            pruned.corners.push(new_vertex[vertex]);
            let neighbour = mesh.neighbours[corner];
            // What's across an edge of a kept polygon was flooded with it.
            pruned.neighbours.push(if neighbour == NO_NEIGHBOUR {
                NO_NEIGHBOUR
            } else {
                new_id[neighbour as usize]
            });
        }
        pruned.polygon_starts.push(pruned.corners.len() as u32);
    }
    let stats = IslandStats {
        polygons_kept: polygons_kept as usize,
        polygons_dropped: count - polygons_kept as usize,
        islands_dropped,
    };
    Ok((pruned, stats))
}

/// Marks in `seen` what's reachable from `from`.
fn flood(graph: &NavGraph, from: u32, seen: &mut [bool], stack: &mut Vec<u32>) {
    if std::mem::replace(&mut seen[from as usize], true) {
        return;
    }
    stack.push(from);
    while let Some(polygon) = stack.pop() {
        for neighbour in graph.neighbours_of(polygon) {
            if !std::mem::replace(&mut seen[neighbour as usize], true) {
                stack.push(neighbour);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A strip of `squares` unit squares from `x`, at height `y`, each cell a metre.
    fn strip(mesh: &mut NavMesh, x: i32, y: i32, squares: i32) {
        let first_polygon = mesh.polygon_count() as u32;
        for square in 0..squares {
            let first = mesh.vertices.len() as u32;
            let left = x + square;
            mesh.vertices.extend([[left, y, 0], [left, y, 1], [left + 1, y, 1], [left + 1, y, 0]]);
            mesh.corners.extend([first, first + 1, first + 2, first + 3]);
            let id = first_polygon + square as u32;
            mesh.neighbours.extend([
                if square > 0 { id - 1 } else { NO_NEIGHBOUR },
                NO_NEIGHBOUR,
                if square + 1 < squares { id + 1 } else { NO_NEIGHBOUR },
                NO_NEIGHBOUR,
            ]);
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
    }

    /// Three strips: two squares at x 0, three at x 10, one at x 20.
    fn three_islands() -> NavMesh {
        let mut mesh =
            NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        strip(&mut mesh, 0, 0, 2);
        strip(&mut mesh, 10, 0, 3);
        strip(&mut mesh, 20, 0, 1);
        mesh
    }

    fn seed(x: f32) -> Seed {
        Seed { field: format!("spawners[{x}]"), position: [x, 0.0, 0.5] }
    }

    #[test]
    fn what_a_seed_cant_walk_to_goes_and_what_stays_still_reads() {
        let (kept, stats) = keep_seeded(&three_islands(), &[seed(11.5)]).expect("it prunes");
        assert_eq!(stats, IslandStats { polygons_kept: 3, polygons_dropped: 3, islands_dropped: 2 });
        assert_eq!(kept.vertices.len(), 12, "no vertex of what went is left");
        assert_eq!(kept.neighbours[2], 1, "the first square's neighbour, by its new ID");
        let cooked = kept.to_cooked().expect("it writes");
        NavMesh::read(&cooked).expect("it still reads");
    }

    #[test]
    fn two_seeds_keep_two_islands_in_their_first_order() {
        let (kept, stats) = keep_seeded(&three_islands(), &[seed(20.5), seed(0.5)]).expect("it prunes");
        assert_eq!(stats, IslandStats { polygons_kept: 3, polygons_dropped: 3, islands_dropped: 1 });
        assert_eq!(kept.vertices[0], [0, 0, 0], "polygons keep their order, whichever seed came first");
        assert_eq!(kept.vertices[8], [20, 0, 0]);
    }

    #[test]
    fn a_seed_off_the_mesh_says_which() {
        let refused = keep_seeded(&three_islands(), &[seed(0.5), seed(15.5)]);
        let Err(NavBakeError::OffMesh { field, position }) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!((field.as_str(), position), ("spawners[15.5]", [15.5, 0.0, 0.5]));
    }

    #[test]
    fn a_seed_a_floor_away_is_off_the_mesh() {
        let high = Seed { field: "navigation.seeds[0]".to_owned(), position: [0.5, 5.0, 0.5] };
        assert!(matches!(keep_seeded(&three_islands(), &[high]), Err(NavBakeError::OffMesh { .. })));
    }
}
