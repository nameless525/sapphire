use std::collections::BTreeSet;
use std::f32::consts::{PI, TAU};

use glam::Vec3;
use std::collections::BTreeMap;

use sapphire_assets::mesh::{Mesh, PositionFormat, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags};

use super::*;
use crate::mesh::{ImportedDelta, ImportedMorphTarget, ImportedSkin, SkinWeights};
use crate::test_util::material;

const TOLERANCE: f32 = bake::DEFAULT_WELD_TOLERANCE;

/// A unit sphere of `rings` × `segments` quads, split at the equator into two submeshes with
/// materials 0 (north) and 1 (south). The equator's vertices are duplicated, one copy per
/// submesh, as an exporter splits a mesh by material.
fn two_material_sphere(rings: u32, segments: u32) -> ImportedMesh {
    let mut mesh = ImportedMesh {
        materials: vec![material("north"), material("south")],
        lods: vec![Lod::default()],
        ..ImportedMesh::default()
    };
    for (material, ring_range) in [(0, 0..=rings / 2), (1, rings / 2..=rings)] {
        let base = mesh.vertices.len() as u32;
        let first_ring = *ring_range.start();
        for ring in ring_range.clone() {
            let theta = PI * ring as f32 / rings as f32;
            for segment in 0..=segments {
                let phi = TAU * segment as f32 / segments as f32;
                let n = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
                mesh.vertices.push(Vertex {
                    position: n.to_array(),
                    normal: n.to_array(),
                    outline_normal: n.to_array(),
                    uv: [segment as f32 / segments as f32, ring as f32 / rings as f32],
                    color: VERTEX_COLOR_NEUTRAL,
                    flags: VertexFlags::NONE,
                });
            }
        }
        let first_index = mesh.indices.len() as u32;
        let stride = segments + 1;
        for ring in 0..(ring_range.end() - first_ring) {
            for segment in 0..segments {
                let a = base + ring * stride + segment;
                let (b, c, d) = (a + stride, a + 1, a + stride + 1);
                mesh.indices.extend([a, c, b, c, d, b]);
            }
        }
        let index_count = mesh.indices.len() as u32 - first_index;
        mesh.lods[0].submeshes.push(Submesh { first_index, index_count, material });
    }
    mesh.authored_outline = vec![false; mesh.vertices.len()];
    mesh.lods[0].vertex_count = mesh.vertices.len() as u32;
    mesh
}

fn triangles(mesh: &ImportedMesh, lod: usize) -> usize {
    mesh.lods[lod].submeshes.iter().map(|submesh| submesh.index_count as usize / 3).sum()
}

fn lod_indices(mesh: &ImportedMesh, lod: usize) -> impl Iterator<Item = (&Submesh, &[u32])> {
    mesh.lods[lod].submeshes.iter().map(|submesh| {
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        (submesh, &mesh.indices[range])
    })
}

/// The extent meshoptimizer measures relative errors against: the bounds' longest side.
fn extent(mesh: &ImportedMesh) -> f32 {
    let bounds = sapphire_assets::mesh::Bounds::enclosing(mesh.vertices.iter().map(|v| v.position));
    (0..3).map(|axis| bounds.max[axis] - bounds.min[axis]).fold(0.0, f32::max)
}

#[test]
fn lods_shrink_monotonically_within_their_error_targets() {
    let mut mesh = two_material_sphere(48, 64);
    let lod0_triangles = mesh.indices.len() / 3;
    let settings = LodSettings::default();
    optimize(&mut mesh, &settings, TOLERANCE, Cook::Desktop);
    // A dense sphere reaches every level.
    assert_eq!(mesh.lods.len(), 1 + settings.levels().len());
    assert_eq!((triangles(&mesh, 0), mesh.lods[0].error), (lod0_triangles, 0.0));
    let extent = extent(&mesh);
    for (lod, level) in mesh.lods.windows(2).enumerate().map(|(i, pair)| (i + 1, pair)) {
        let [previous, current] = level else { unreachable!() };
        let limit = settings.levels()[lod - 1];
        assert!(triangles(&mesh, lod) < triangles(&mesh, lod - 1), "LOD{lod} has fewer triangles");
        assert!(
            triangles(&mesh, lod) as f32 <= (1.0 - MIN_LOD_REDUCTION) * triangles(&mesh, lod - 1) as f32,
            "LOD{lod} saves enough to be worth a switch"
        );
        assert!(current.vertex_count <= previous.vertex_count);
        assert!(current.error >= previous.error, "errors never decrease");
        assert!(current.error <= limit.max_error * extent * 1.0001, "LOD{lod} error {} m", current.error);
        // The triangle target is met unless the error budget stopped it first.
        let target = (lod0_triangles as f32 * limit.triangle_ratio).ceil() as usize;
        assert!(triangles(&mesh, lod) <= target + 2 || current.error >= 0.5 * limit.max_error * extent);
    }
    eprintln!(
        "sphere LODs: triangles {:?}, vertices {:?}, errors {:?} m",
        (0..mesh.lods.len()).map(|lod| triangles(&mesh, lod)).collect::<Vec<_>>(),
        mesh.lods.iter().map(|lod| lod.vertex_count).collect::<Vec<_>>(),
        mesh.lods.iter().map(|lod| lod.error).collect::<Vec<_>>(),
    );
    // And the result is a valid cooked mesh: every LOD inside its prefix, and so on.
    Mesh::read(&mesh.pack(PositionFormat::Unorm16, &BTreeMap::new()).unwrap().to_cooked().unwrap()).unwrap();
}

#[test]
fn parts_that_simplify_away_keep_the_errors_monotonic() {
    // The sphere plus a 1 mm triangle of its own material, far smaller than any LOD's budget,
    // which the coarser levels drop altogether.
    let mut mesh = two_material_sphere(48, 64);
    mesh.materials.push(material("speck"));
    let base = mesh.vertices.len() as u32;
    for position in [[2.0, 0.0, 0.0], [2.001, 0.0, 0.0], [2.0, 0.001, 0.0]] {
        mesh.vertices.push(Vertex { position, normal: [0.0, 0.0, 1.0], ..mesh.vertices[0] });
        mesh.authored_outline.push(false);
    }
    let first_index = mesh.indices.len() as u32;
    mesh.indices.extend([base, base + 1, base + 2]);
    mesh.lods[0].submeshes.push(Submesh { first_index, index_count: 3, material: 2 });
    mesh.lods[0].vertex_count = mesh.vertices.len() as u32;

    optimize(&mut mesh, &LodSettings::default(), TOLERANCE, Cook::Desktop);
    let has_speck = |lod: usize| mesh.lods[lod].submeshes.iter().any(|submesh| submesh.material == 2);
    assert!(has_speck(0), "LOD0 keeps everything");
    assert!((1..mesh.lods.len()).any(|lod| !has_speck(lod)), "the speck is simplified away");
    assert!(mesh.lods.windows(2).all(|pair| pair[1].error >= pair[0].error), "{:?}", mesh.lods);
    assert!(mesh.lods.iter().all(|lod| !lod.submeshes.is_empty()), "no LOD is empty");
    Mesh::read(&mesh.pack(PositionFormat::Unorm16, &BTreeMap::new()).unwrap().to_cooked().unwrap())
        .expect("cooks");
}

#[test]
fn every_lod_reads_a_prefix_of_the_vertices() {
    let mut mesh = two_material_sphere(32, 48);
    optimize(&mut mesh, &LodSettings::default(), TOLERANCE, Cook::Desktop);
    for lod in 0..mesh.lods.len() {
        let used: BTreeSet<u32> =
            lod_indices(&mesh, lod).flat_map(|(_, indices)| indices.iter().copied()).collect();
        // Nested levels: exactly the prefix, no gaps.
        assert_eq!(used, (0..mesh.lods[lod].vertex_count).collect(), "LOD{lod}");
    }
    assert_eq!(mesh.lods[0].vertex_count as usize, mesh.vertices.len());
}

#[test]
fn material_seams_never_open() {
    let mut mesh = two_material_sphere(32, 48);
    let equator = |mesh: &ImportedMesh, lod: usize, material: u32| -> BTreeSet<[i32; 3]> {
        lod_indices(mesh, lod)
            .filter(|(submesh, _)| submesh.material == material)
            .flat_map(|(_, indices)| indices.iter().map(|&index| mesh.vertices[index as usize].position))
            .filter(|position| position[1].abs() < 1e-5)
            .map(|position| position.map(|c| (c * 1e4).round() as i32))
            .collect()
    };
    optimize(&mut mesh, &LodSettings::default(), TOLERANCE, Cook::Desktop);
    let seam = equator(&mesh, 0, 0);
    assert_eq!(seam.len(), 48, "the seam's points (the 0/1 seam column shares a point)");
    for lod in 1..mesh.lods.len() {
        // Both sides keep every seam vertex where it was, so their edges still meet.
        assert_eq!(equator(&mesh, lod, 0), seam, "north side of LOD{lod}");
        assert_eq!(equator(&mesh, lod, 1), seam, "south side of LOD{lod}");
    }
}

#[test]
fn submeshes_merge_by_material() {
    let mut mesh = two_material_sphere(8, 12);
    // Split each hemisphere into two submeshes and interleave them: 0, 1, 0, 1.
    let [north, south] = [mesh.lods[0].submeshes[0], mesh.lods[0].submeshes[1]];
    let half = |submesh: Submesh, part: u32| {
        let first = submesh.index_count / 6 * 3;
        if part == 0 {
            Submesh { index_count: first, ..submesh }
        } else {
            Submesh {
                first_index: submesh.first_index + first,
                index_count: submesh.index_count - first,
                ..submesh
            }
        }
    };
    mesh.lods[0].submeshes = vec![half(north, 0), half(south, 0), half(north, 1), half(south, 1)];
    let triangles_of = |mesh: &ImportedMesh, material: u32| -> BTreeSet<[[i32; 3]; 3]> {
        lod_indices(mesh, 0)
            .filter(|(submesh, _)| submesh.material == material)
            .flat_map(|(_, indices)| indices.as_chunks::<3>().0.to_vec())
            .map(|triangle| {
                // By position, rotated to start at the smallest corner: order-independent.
                let corners = triangle
                    .map(|index| mesh.vertices[index as usize].position.map(|c| (c * 1e4).round() as i32));
                let start = (0..3).min_by_key(|&k| corners[k]).unwrap();
                [corners[start], corners[(start + 1) % 3], corners[(start + 2) % 3]]
            })
            .collect()
    };
    let before = [triangles_of(&mesh, 0), triangles_of(&mesh, 1)];
    optimize(&mut mesh, &LodSettings::none(), TOLERANCE, Cook::Desktop);
    let materials: Vec<u32> = mesh.lods[0].submeshes.iter().map(|submesh| submesh.material).collect();
    assert_eq!(materials, [0, 1], "one draw per material");
    assert_eq!(
        [triangles_of(&mesh, 0), triangles_of(&mesh, 1)],
        before,
        "the same triangles, wound the same"
    );
}

#[test]
fn the_mobile_cook_drops_levels_and_their_vertices() {
    let source = two_material_sphere(48, 64);
    let mut desktop = source.clone();
    let mut mobile = source.clone();
    optimize(&mut desktop, &LodSettings::default(), TOLERANCE, Cook::Desktop);
    optimize(&mut mobile, &LodSettings::default(), TOLERANCE, Cook::Mobile);
    assert_eq!(mobile.lods.len(), desktop.lods.len() - 1);
    assert_eq!(triangles(&mobile, 0), triangles(&desktop, 1));
    assert_eq!(mobile.vertices.len() as u32, desktop.lods[1].vertex_count);
    assert!(mobile.lods[0].error > 0.0);

    // Skipping more levels than there are leaves the coarsest.
    let all = LodSettings::new(LodSettings::default().levels().to_vec(), 3).unwrap();
    let mut coarsest = source.clone();
    optimize(&mut coarsest, &all, TOLERANCE, Cook::Mobile);
    assert_eq!((coarsest.lods.len(), triangles(&coarsest, 0)), (1, triangles(&desktop, 3)));
    // Without levels both cooks keep LOD0 alone.
    let mut plain = source;
    optimize(&mut plain, &LodSettings::none(), TOLERANCE, Cook::Mobile);
    assert_eq!(plain.lods.len(), 1);
}

#[test]
fn skin_weights_and_morph_deltas_follow_their_vertices() {
    let mut mesh = two_material_sphere(32, 48);
    // Tag every vertex by its position: the weight of palette entry 1 is its height, and the
    // morph target moves exactly the northern vertices along their normal.
    let weights = mesh
        .vertices
        .iter()
        .map(|v| {
            let up = (v.position[1] + 1.0) / 2.0;
            SkinWeights { binds: [1, 0, 0, 0], weights: [up, 1.0 - up, 0.0, 0.0] }
        })
        .collect();
    mesh.skin = Some(ImportedSkin { weights, ..ImportedSkin::default() });
    let deltas = (0..mesh.vertices.len() as u32)
        .filter(|&i| mesh.vertices[i as usize].position[1] > 0.1)
        .map(|vertex| ImportedDelta {
            vertex,
            position: mesh.vertices[vertex as usize].normal,
            normal: [0.0; 3],
        })
        .collect();
    mesh.morph_targets = vec![ImportedMorphTarget { name: "puff".into(), default_weight: 0.0, deltas }];

    for cook in Cook::ALL {
        let mut optimized = mesh.clone();
        optimize(&mut optimized, &LodSettings::default(), TOLERANCE, cook);
        let skin = optimized.skin.as_ref().unwrap();
        assert_eq!(skin.weights.len(), optimized.vertices.len());
        for (vertex, weights) in optimized.vertices.iter().zip(&skin.weights) {
            assert_eq!(weights.weights[0], (vertex.position[1] + 1.0) / 2.0, "{cook}: {vertex:?}");
        }
        let deltas = &optimized.morph_targets[0].deltas;
        let northern = optimized.vertices.iter().filter(|v| v.position[1] > 0.1).count();
        assert_eq!(deltas.len(), northern, "{cook}: dropped vertices lose their deltas, kept ones keep them");
        assert!(deltas.windows(2).all(|pair| pair[0].vertex < pair[1].vertex), "{cook}: sorted by vertex");
        for delta in deltas {
            assert_eq!(delta.position, optimized.vertices[delta.vertex as usize].normal, "{cook}");
        }
    }
}

#[test]
fn optimizing_is_deterministic() {
    let run = || {
        let mut mesh = two_material_sphere(32, 48);
        optimize(&mut mesh, &LodSettings::default(), TOLERANCE, Cook::Desktop);
        mesh
    };
    assert_eq!(run(), run());
}

#[test]
fn settings_are_checked() {
    let level = |triangle_ratio, max_error| LodLevel { triangle_ratio, max_error };
    assert!(LodSettings::new(vec![level(0.5, 0.01), level(0.25, 0.02)], 2).is_some());
    assert!(LodSettings::new(vec![], 0).is_some());
    for (levels, skip) in [
        (vec![level(1.0, 0.01)], 0),
        (vec![level(0.0, 0.01)], 0),
        (vec![level(0.5, 0.01), level(0.5, 0.02)], 0),
        (vec![level(0.5, 0.02), level(0.25, 0.01)], 0),
        (vec![level(0.5, f32::NAN)], 0),
        (vec![level(0.5, -0.1)], 0),
        (vec![level(0.5, 0.01)], 2),
    ] {
        assert!(LodSettings::new(levels.clone(), skip).is_none(), "{levels:?} skipping {skip}");
    }
}
