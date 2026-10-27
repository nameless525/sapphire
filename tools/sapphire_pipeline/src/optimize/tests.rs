use std::collections::BTreeSet;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use std::collections::BTreeMap;

use sapphire_assets::mesh::{
    Joint, JointTransform, Mesh, PositionFormat, SkinBind, VERTEX_COLOR_NEUTRAL, Vertex, VertexFlags,
};

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

/// A rolling heightfield of `cells` × `cells` quads, one material: a tile cut from a larger
/// surface, its edges open.
fn heightfield_tile(cells: u32) -> ImportedMesh {
    let mut mesh = ImportedMesh {
        materials: vec![material("ground")],
        lods: vec![Lod::default()],
        ..ImportedMesh::default()
    };
    for row in 0..=cells {
        for column in 0..=cells {
            let (x, z) = (column as f32 / cells as f32, row as f32 / cells as f32);
            let y = 0.1 * (x * 9.0).sin() * (z * 7.0).cos();
            mesh.vertices.push(Vertex {
                position: [x, y, z],
                normal: [0.0, 1.0, 0.0],
                outline_normal: [0.0, 1.0, 0.0],
                uv: [x, z],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
        }
    }
    let stride = cells + 1;
    for row in 0..cells {
        for column in 0..cells {
            let a = row * stride + column;
            mesh.indices.extend([a, a + stride, a + 1, a + 1, a + stride, a + stride + 1]);
        }
    }
    mesh.lods[0].submeshes.push(Submesh {
        first_index: 0,
        index_count: mesh.indices.len() as u32,
        material: 0,
    });
    mesh.authored_outline = vec![false; mesh.vertices.len()];
    mesh.lods[0].vertex_count = mesh.vertices.len() as u32;
    mesh
}

#[test]
fn locked_borders_stay_whole_at_every_level() {
    let border = |mesh: &ImportedMesh, lod: usize| -> BTreeSet<[u32; 3]> {
        lod_indices(mesh, lod)
            .flat_map(|(_, indices)| indices.iter().map(|&index| mesh.vertices[index as usize].position))
            .filter(|p| [p[0], p[2]].iter().any(|&c| c == 0.0 || c == 1.0))
            .map(|p| p.map(f32::to_bits))
            .collect()
    };
    let settings = LodSettings::default();
    let mut free = heightfield_tile(32);
    optimize(&mut free, &settings, TOLERANCE, Cook::Desktop);
    let whole = border(&free, 0);
    assert_eq!(whole.len(), 4 * 32, "the tile's border");
    assert!((1..free.lods.len()).any(|lod| border(&free, lod) != whole), "free borders simplify");
    let mut locked = heightfield_tile(32);
    optimize(&mut locked, &settings.clone().lock_border(true), TOLERANCE, Cook::Desktop);
    assert!(locked.lods.len() > 1, "the inside still simplifies");
    for lod in 1..locked.lods.len() {
        assert_eq!(border(&locked, lod), whole, "LOD{lod} keeps every border vertex");
        assert!(triangles(&locked, lod) < triangles(&locked, 0));
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

/// A shipped mesh source (under `content/`), imported and baked as its cook does, with its
/// sidecar's settings.
fn shipped(source: &str) -> (ImportedMesh, crate::meta::MeshSettings) {
    use crate::meta::ImportSettings;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content").join(source);
    let importer = crate::meta::read(&crate::meta::sidecar_path(&path)).unwrap().importer;
    let bytes = std::fs::read(&path).unwrap();
    let (mut mesh, settings) = match importer {
        ImportSettings::Gltf(settings) => (crate::import::gltf::import(&path).unwrap(), settings),
        ImportSettings::Vrm(settings) => {
            (crate::import::vrm::import(&bytes, settings.version).unwrap().mesh, settings.mesh)
        }
        other => panic!("{source} isn't a glTF or VRM mesh: {other:?}"),
    };
    bake::outline_normals(&mut mesh, &settings.outline);
    (mesh, settings)
}

/// Per morph target: every (rest position, position delta) the mesh's kept vertices carry.
fn morph_shapes(mesh: &ImportedMesh) -> Vec<BTreeSet<([u32; 3], [u32; 3])>> {
    mesh.morph_targets
        .iter()
        .map(|target| {
            target
                .deltas
                .iter()
                .map(|delta| {
                    let position = mesh.vertices[delta.vertex as usize].position;
                    (position.map(f32::to_bits), delta.position.map(f32::to_bits))
                })
                .collect()
        })
        .collect()
}

/// Triangles per material slot name in `lod`.
fn triangles_by_slot(mesh: &ImportedMesh, lod: usize) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (submesh, indices) in lod_indices(mesh, lod) {
        *counts.entry(mesh.materials[submesh.material as usize].name.clone()).or_default() +=
            indices.len() / 3;
    }
    counts
}

/// The review's AP-005 on shipped content: the look-dev character and the faces sample hold
/// expressions on phones, whose cook starts at a simplified level, so every vertex a morph
/// target moves must still be there, with its delta, and phones must still save triangles.
#[test]
fn shipped_morph_meshes_keep_every_morph_on_phones() {
    let mut failures = Vec::new();
    for source in [
        "third_party/vrm1_constraint_twist_sample/VRM1_Constraint_Twist_Sample.vrm",
        "samples/face/face_head.glb",
    ] {
        let (mesh, settings) = shipped(source);
        let tolerance = settings.outline.weld_tolerance();
        let [mut desktop, mut mobile] = [mesh.clone(), mesh];
        optimize(&mut desktop, &settings.lods, tolerance, Cook::Desktop);
        optimize(&mut mobile, &settings.lods, tolerance, Cook::Mobile);
        for (label, cooked) in [("desktop", &desktop), ("mobile", &mobile)] {
            for lod in 0..cooked.lods.len() {
                eprintln!(
                    "{source} {label} LOD{lod}: {} triangles, {} vertices, error {:.5} m, {:?}",
                    triangles(cooked, lod),
                    cooked.lods[lod].vertex_count,
                    cooked.lods[lod].error,
                    triangles_by_slot(cooked, lod),
                );
            }
        }
        let (full, light) = (morph_shapes(&desktop), morph_shapes(&mobile));
        let lost: Vec<(&str, usize)> = full
            .iter()
            .zip(&light)
            .zip(&desktop.morph_targets)
            .filter(|((full, light), _)| full != light)
            .map(|((full, light), target)| (target.name.as_str(), full.difference(light).count()))
            .collect();
        if !lost.is_empty() {
            failures.push(format!("{source}: phones lose morph deltas (target, deltas lost): {lost:?}"));
        }
        // Phones still save triangles, and keep within the spec's hero character budget (§3.4).
        if triangles(&mobile, 0) >= triangles(&desktop, 0) {
            failures.push(format!("{source}: phones draw as many triangles as desktop"));
        }
        if triangles(&mobile, 0) > 30_000 {
            failures.push(format!("{source}: {} triangles on phones, over 30k", triangles(&mobile, 0)));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The review's AP-005 witness: a 9 × 9 plane, flat to the simplifier, whose middle vertex
/// alone carries a 10 m morph delta and black ambient occlusion. Before deformation-aware
/// simplification the phone cook kept 10 of its 81 vertices, none of them that one, and
/// reported an error under a millimetre.
fn morph_witness() -> ImportedMesh {
    const N: u32 = 9;
    let middle = N * N / 2;
    let mut mesh = ImportedMesh {
        materials: vec![material("surface")],
        lods: vec![Lod::default()],
        ..ImportedMesh::default()
    };
    for z in 0..N {
        for x in 0..N {
            let middle = z * N + x == middle;
            mesh.vertices.push(Vertex {
                position: [x as f32, 0.0, z as f32],
                normal: [0.0, 1.0, 0.0],
                outline_normal: [0.0, 1.0, 0.0],
                uv: [x as f32, z as f32],
                color: if middle { [0, 0, 0] } else { [255, 255, 255] },
                flags: VertexFlags::NONE,
            });
        }
    }
    for z in 0..N - 1 {
        for x in 0..N - 1 {
            let a = z * N + x;
            mesh.indices.extend([a, a + N, a + 1, a + 1, a + N, a + N + 1]);
        }
    }
    mesh.lods[0] = Lod {
        vertex_count: N * N,
        error: 0.0,
        submeshes: vec![Submesh { first_index: 0, index_count: mesh.indices.len() as u32, material: 0 }],
    };
    mesh.authored_outline = vec![false; mesh.vertices.len()];
    mesh.morph_targets = vec![ImportedMorphTarget {
        name: "bump".into(),
        default_weight: 1.0,
        deltas: vec![ImportedDelta { vertex: middle, position: [0.0, 10.0, 0.0], normal: [0.0; 3] }],
    }];
    mesh
}

/// LOD `lod`'s triangles by corner positions, rotated to start at the smallest corner, so two
/// meshes' triangles compare whatever their vertex numbering.
fn triangle_positions(mesh: &ImportedMesh, lod: usize) -> BTreeSet<[[u32; 3]; 3]> {
    lod_indices(mesh, lod)
        .flat_map(|(_, indices)| indices.as_chunks::<3>().0.to_vec())
        .map(|triangle| {
            let corners = triangle.map(|index| mesh.vertices[index as usize].position.map(f32::to_bits));
            let start = (0..3).min_by_key(|&k| corners[k]).unwrap();
            [corners[start], corners[(start + 1) % 3], corners[(start + 2) % 3]]
        })
        .collect()
}

#[test]
fn simplification_keeps_what_morph_targets_move() {
    let source = morph_witness();
    let settings = LodSettings::new(vec![LodLevel { triangle_ratio: 0.1, max_error: 0.001 }], 1).unwrap();
    let mut desktop = source.clone();
    optimize(&mut desktop, &settings, TOLERANCE, Cook::Desktop);
    let mut mobile = source;
    optimize(&mut mobile, &settings, TOLERANCE, Cook::Mobile);
    assert_eq!(mobile.lods.len(), 1, "the phone cook starts at the simplified level");
    eprintln!(
        "witness on phones: {} of 81 vertices, {} triangles of 128, error {} m",
        mobile.vertices.len(),
        triangles(&mobile, 0),
        mobile.lods[0].error
    );
    assert!(triangles(&mobile, 0) < triangles(&desktop, 0), "the rest of the plane still simplifies");
    assert!(mobile.lods[0].error <= 0.001 * extent(&desktop), "within the level's budget");

    // The vertex the target moves is still there, with its whole delta and its color.
    let deltas = &mobile.morph_targets[0].deltas;
    assert_eq!(deltas.len(), 1, "the delta survives");
    let moved = mobile.vertices[deltas[0].vertex as usize];
    assert_eq!((moved.position, deltas[0].position), ([4.0, 0.0, 4.0], [0.0, 10.0, 0.0]));
    assert_eq!(moved.color, [0, 0, 0], "and so does its black ambient occlusion");

    // Every triangle the target deforms is LOD0's own, so at any weight the deformed level
    // differs from the deformed LOD0 only where neither moves: by the error at rest.
    let deforming = |mesh: &ImportedMesh, lod: usize| -> BTreeSet<[[u32; 3]; 3]> {
        let moved = moved.position.map(f32::to_bits);
        triangle_positions(mesh, lod).into_iter().filter(|triangle| triangle.contains(&moved)).collect()
    };
    assert_eq!(deforming(&mobile, 0), deforming(&desktop, 0));
    assert_eq!(deforming(&desktop, 0).len(), 6, "the six triangles round the middle of the grid");
}

/// A tube 2 m long and 0.2 m across along Y, in rings 2.5 cm apart, skinned to a lower joint at
/// the origin and an upper one at 1 m whose weight rises linearly from 0.9 to 1.1 m: an elbow.
/// At rest it is a straight cylinder, so positions and normals alone let the simplifier merge
/// its rings freely, the blend's among them.
fn elbow() -> ImportedMesh {
    const RINGS: u32 = 80;
    const SIDES: u32 = 16;
    let mut mesh = ImportedMesh {
        materials: vec![material("sleeve")],
        lods: vec![Lod::default()],
        ..ImportedMesh::default()
    };
    let mut weights = Vec::new();
    for ring in 0..=RINGS {
        let y = 2.0 * ring as f32 / RINGS as f32;
        let upper = ((y - 0.9) / 0.2).clamp(0.0, 1.0);
        for side in 0..SIDES {
            let angle = TAU * side as f32 / SIDES as f32;
            let normal = [angle.cos(), 0.0, angle.sin()];
            mesh.vertices.push(Vertex {
                position: [0.1 * normal[0], y, 0.1 * normal[2]],
                normal,
                outline_normal: normal,
                uv: [side as f32 / SIDES as f32, y / 2.0],
                color: VERTEX_COLOR_NEUTRAL,
                flags: VertexFlags::NONE,
            });
            // Heaviest first.
            let [(heavy, heavy_weight), (light, light_weight)] =
                if upper >= 0.5 { [(1, upper), (0, 1.0 - upper)] } else { [(0, 1.0 - upper), (1, upper)] };
            weights.push(SkinWeights {
                binds: [heavy, light, 0, 0],
                weights: [heavy_weight, light_weight, 0.0, 0.0],
            });
        }
    }
    for ring in 0..RINGS {
        for side in 0..SIDES {
            let (a, b) = (ring * SIDES + side, ring * SIDES + (side + 1) % SIDES);
            mesh.indices.extend([a, a + SIDES, b, b, a + SIDES, b + SIDES]);
        }
    }
    mesh.lods[0] = Lod {
        vertex_count: mesh.vertices.len() as u32,
        error: 0.0,
        submeshes: vec![Submesh { first_index: 0, index_count: mesh.indices.len() as u32, material: 0 }],
    };
    mesh.authored_outline = vec![false; mesh.vertices.len()];
    let joint = |name: &str, parent, y| Joint {
        name: name.into(),
        parent,
        rest: JointTransform { translation: [0.0, y, 0.0], ..JointTransform::IDENTITY },
    };
    let translation = |y: f32| Mat4::from_translation(Vec3::new(0.0, y, 0.0)).to_cols_array_2d();
    mesh.skin = Some(ImportedSkin {
        joints: vec![joint("lower", None, 0.0), joint("upper", Some(0), 1.0)],
        binds: vec![
            SkinBind { joint: 0, inverse_bind_matrix: translation(0.0) },
            SkinBind { joint: 1, inverse_bind_matrix: translation(-1.0) },
        ],
        weights,
    });
    mesh
}

/// The distance from `point` to the triangle `[a, b, c]` (Ericson, Real-Time Collision
/// Detection, 5.1.5).
fn distance_to_triangle(point: Vec3, [a, b, c]: [Vec3; 3]) -> f32 {
    let (ab, ac, ap) = (b - a, c - a, point - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return point.distance(a);
    }
    let bp = point - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return point.distance(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return point.distance(a + ab * (d1 / (d1 - d3)));
    }
    let cp = point - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return point.distance(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return point.distance(a + ac * (d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return point.distance(b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denominator = 1.0 / (va + vb + vc);
    point.distance(a + ab * (vb * denominator) + ac * (vc * denominator))
}

/// How far LOD `lod` of the elbow strays from its LOD0 with the upper joint bent `angle` about
/// X: the farthest any bent LOD0 vertex lies from the bent level's surface.
fn elbow_strays(mesh: &ImportedMesh, lod: usize, angle: f32) -> f32 {
    let skin = mesh.skin.as_ref().unwrap();
    let pivot = Vec3::new(0.0, 1.0, 0.0);
    let bend = Mat3::from_rotation_x(angle);
    let bent: Vec<Vec3> = mesh
        .vertices
        .iter()
        .zip(&skin.weights)
        .map(|(vertex, influences)| {
            let position = Vec3::from(vertex.position);
            let upper = pivot + bend * (position - pivot);
            influences.binds.iter().zip(influences.weights).fold(Vec3::ZERO, |sum, (&bind, weight)| {
                sum + weight * if bind == 1 { upper } else { position }
            })
        })
        .collect();
    let level: Vec<[Vec3; 3]> = lod_indices(mesh, lod)
        .flat_map(|(_, indices)| indices.as_chunks::<3>().0.to_vec())
        .map(|triangle| triangle.map(|index| bent[index as usize]))
        .collect();
    (0..mesh.lods[0].vertex_count as usize)
        .map(|vertex| {
            let point = bent[vertex];
            level.iter().map(|&triangle| distance_to_triangle(point, triangle)).fold(f32::MAX, f32::min)
        })
        .fold(0.0, f32::max)
}

#[test]
fn simplification_keeps_the_shape_of_bent_joints() {
    let mut mesh = elbow();
    optimize(&mut mesh, &LodSettings::default(), TOLERANCE, Cook::Desktop);
    assert_eq!(mesh.lods.len(), 4, "the straight tube still simplifies to every level");
    for lod in 1..mesh.lods.len() {
        let error = mesh.lods[lod].error;
        let [rest, posed, right] = [0.0, POSE_ANGLE, FRAC_PI_2].map(|angle| elbow_strays(&mesh, lod, angle));
        eprintln!(
            "elbow LOD{lod}: {} triangles, error {error:.4} m, strays {rest:.4} m at rest, {posed:.4} m \
             bent {}°, {right:.4} m bent 90°",
            triangles(&mesh, lod),
            POSE_ANGLE.to_degrees(),
        );
        // The error bounds how far the level strays bent as far as the test poses, as at rest.
        assert!(rest <= error, "LOD{lod} at rest: {rest} m against {error} m");
        assert!(posed <= error, "LOD{lod} bent {}°: {posed} m against {error} m", POSE_ANGLE.to_degrees());
    }
}
