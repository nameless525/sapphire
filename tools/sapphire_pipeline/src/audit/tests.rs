use sapphire_assets::character_maps::MaterialId;
use sapphire_assets::mesh::{Lod, Submesh, Vertex};

use super::*;
use crate::import::image::ChannelLayout;
use crate::test_util::material;

/// A `width` × `height` map, each texel `texel(x, y)`.
fn image(width: u32, height: u32, texel: impl Fn(u32, u32) -> [f32; 4]) -> SourceImage {
    let pixels =
        (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).map(|(x, y)| texel(x, y)).collect();
    SourceImage::new(width, height, ChannelLayout::Rgba, pixels).unwrap()
}

fn encoded(id: u8) -> f32 {
    ilm::encode_material_id(MaterialId::new(id).unwrap())
}

/// A control map of two regions side by side, ID 0 on the left half and 3 on the right: R constant, G
/// constant in each region, B a gradient down the right one.
fn control() -> SourceImage {
    image(8, 4, |x, y| {
        let right = x >= 4;
        let b = if right { y as f32 / 4.0 } else { 0.5 };
        [0.25, if right { 0.9 } else { 0.1 }, b, encoded(if right { 3 } else { 0 })]
    })
}

#[test]
fn channels_read_as_constant_per_region_or_varying() {
    let control = control();
    let regions = Regions::of(&control);
    let audit = channels(&control, Some(&regions), &[0, 1, 2]);
    assert_eq!(
        audit.regions,
        vec![RegionTexels { id: 0, texels: 16 }, RegionTexels { id: 3, texels: 16 }],
        "each half's texels, by the alpha's IDs"
    );
    let shapes: Vec<ChannelShape> = audit.channels.iter().map(|channel| channel.shape).collect();
    assert_eq!(shapes, [ChannelShape::Constant, ChannelShape::PerRegion, ChannelShape::Varies]);
    let g = &audit.channels[1];
    assert_eq!(
        g.regions,
        vec![RegionRange { id: 0, min: 0.1, max: 0.1 }, RegionRange { id: 3, min: 0.9, max: 0.9 }]
    );
    let b = &audit.channels[2];
    assert_eq!(b.regions[0], RegionRange { id: 0, min: 0.5, max: 0.5 }, "the left half holds one value");
    assert_eq!((b.regions[1].min, b.regions[1].max), (0.0, 0.75), "the right half's gradient");

    // Without regions, a channel is constant or it varies.
    let alone = channels(&control, None, &[1]);
    assert!(alone.regions.is_empty() && alone.channels[0].regions.is_empty());
    assert_eq!(alone.channels[0].shape, ChannelShape::Varies);
}

#[test]
fn a_map_of_another_size_reads_the_control_maps_regions_by_nearest_texel() {
    let control = control();
    let regions = Regions::of(&control);
    // Twice as wide and tall: its left half is region 0, as the control map's is.
    let layers = image(16, 8, |x, _| [if x < 8 { 0.0 } else { 1.0 }, 0.0, 0.0, 0.0]);
    let audit = channels(&layers, Some(&regions), &[0]);
    assert_eq!(audit.regions, vec![RegionTexels { id: 0, texels: 64 }, RegionTexels { id: 3, texels: 64 }]);
    assert_eq!(audit.channels[0].shape, ChannelShape::PerRegion);
}

#[test]
fn values_within_half_an_8_bit_step_are_one() {
    let near =
        image(2, 1, |x, _| [0.5 + x as f32 * SAME_VALUE * 0.9, 0.5 + x as f32 * SAME_VALUE * 1.5, 0.0, 0.0]);
    let audit = channels(&near, None, &[0, 1]);
    assert_eq!(audit.channels[0].shape, ChannelShape::Constant);
    assert_eq!(audit.channels[1].shape, ChannelShape::Varies);
}

/// A mesh of the triangles `uvs`, each vertex with the ID beside its UV.
fn mesh(triangles: &[[([f32; 2], u8); 3]]) -> ImportedMesh {
    let mut mesh = ImportedMesh {
        materials: vec![material("body")],
        lods: vec![Lod::default()],
        ..ImportedMesh::default()
    };
    for (index, triangle) in triangles.iter().enumerate() {
        for &(uv, id) in triangle {
            mesh.vertices.push(Vertex {
                position: [index as f32, uv[0], uv[1]],
                uv,
                flags: VertexFlags::with_material_id(MaterialId::new(id).unwrap()),
                ..Vertex::default()
            });
        }
        mesh.indices.extend((0..3).map(|corner| (index * 3 + corner) as u32));
    }
    mesh.lods[0].submeshes.push(Submesh {
        first_index: 0,
        index_count: mesh.indices.len() as u32,
        material: 0,
    });
    mesh
}

#[test]
fn triangles_disagree_where_they_cover_another_regions_texels_or_split_and_straddle_two_regions() {
    let regions = Regions::of(&control());
    let mesh = mesh(&[
        // Inside the left half, ID 0: agrees.
        [([0.05, 0.1], 0), ([0.4, 0.1], 0), ([0.05, 0.9], 0)],
        // Inside the right half with ID 0: disagrees.
        [([0.6, 0.1], 0), ([0.95, 0.1], 0), ([0.6, 0.9], 0)],
        // Across the halves with ID 3: covers left texels, disagrees.
        [([0.3, 0.1], 3), ([0.95, 0.1], 3), ([0.95, 0.9], 3)],
        // Smaller than a texel, in the right half: the texel under its centroid agrees.
        [([0.80, 0.50], 3), ([0.81, 0.50], 3), ([0.80, 0.51], 3)],
        // Its vertices split between IDs.
        [([0.05, 0.1], 0), ([0.4, 0.1], 0), ([0.05, 0.9], 3)],
        // Wrapped one repeat to the right of the left half: agrees, as a repeating sampler reads it.
        [([1.05, 0.1], 0), ([1.4, 0.1], 0), ([1.05, 0.9], 0)],
    ]);
    let triangles: Vec<[u32; 3]> = mesh.indices.as_chunks::<3>().0.to_vec();
    // Only the third covers both regions' texels: the second disagrees with its vertices' ID alone.
    assert_eq!(
        agreement(&mesh, &triangles, Some(&regions)),
        Agreement { split: 1, disagreeing: 3, straddling: 1, held: true, tiling: None }
    );
    assert_eq!(
        agreement(&mesh, &triangles, None),
        Agreement { split: 1, disagreeing: 1, straddling: 0, held: false, tiling: None },
        "split alone"
    );
}

#[test]
fn a_slot_tiling_past_its_wraps_isnt_held_to_its_control_map() {
    // One triangle over eight repeats of a 2048² map: some 2.7e8 texel centres to test, were it held.
    let wide =
        SourceImage::new(2048, 2048, ChannelLayout::Rgba, vec![[0.0, 0.0, 0.0, encoded(0)]; 2048 * 2048])
            .unwrap();
    let regions = Regions::of(&wide);
    let tiling = mesh(&[[([0.0, 0.0], 0), ([8.0, 0.0], 0), ([0.0, 8.0], 3)]]);
    let triangles: Vec<[u32; 3]> = tiling.indices.as_chunks::<3>().0.to_vec();
    assert_eq!(
        agreement(&tiling, &triangles, Some(&regions)),
        Agreement { split: 1, disagreeing: 1, straddling: 0, held: false, tiling: Some([8.0, 8.0]) },
        "split still counts; the map's regions aren't read, and its span is said"
    );
    assert_eq!(agreement(&tiling, &triangles, None).tiling, None, "without a map, nothing to tile");
    // Two repeats exactly is held: a slot spans at most `MAX_WRAPS` on each axis.
    let small = Regions::of(&control());
    let within = mesh(&[[([0.0, 0.0], 0), ([MAX_WRAPS, 0.0], 0), ([0.0, 0.5], 0)]]);
    let triangles: Vec<[u32; 3]> = within.indices.as_chunks::<3>().0.to_vec();
    let held = agreement(&within, &triangles, Some(&small));
    assert!(held.held && held.tiling.is_none());
    let past = mesh(&[[([0.0, 0.0], 0), ([0.0, 0.25 + MAX_WRAPS], 0), ([0.5, 0.0], 0)]]);
    let triangles: Vec<[u32; 3]> = past.indices.as_chunks::<3>().0.to_vec();
    let past = agreement(&past, &triangles, Some(&small));
    assert!(!past.held, "past it on v alone");
    assert_eq!(past.tiling, Some([0.5, 0.25 + MAX_WRAPS]));
    let slot = TilingSlot { source: "a/body.glb".to_owned(), slot: "skin".to_owned(), span: [0.5, 2.25] };
    assert_eq!(slot.to_string(), "a/body.glb#skin spans 0.50 x 2.25 repeats");
}

/// The cooked levels are held to the audit's chain by triangles, vertices and error to the bit, on each
/// cook's levels: the mobile cook's skip drops LOD0 but never the last level.
#[test]
fn cooked_levels_are_held_to_the_chain_to_the_bit() {
    let level =
        |setting, triangles, vertices, error| optimize::ChainLevel { setting, triangles, vertices, error };
    let chain = ChainAudit {
        vertices: 40,
        seam: 0,
        morph: 0,
        border: 0,
        pinned: 0,
        locked: 0,
        cooked: vec![level(1, 50, 30, 0.002), level(2, 20, 14, 0.01)],
        relaxed: Vec::new(),
    };
    let cooked = |levels: &[(u32, u32, f32)]| -> Vec<CookedLevel> {
        levels
            .iter()
            .map(|&(triangles, vertices, error)| CookedLevel { triangles, vertices, error })
            .collect()
    };
    let desktop = cooked(&[(100, 40, 0.0), (50, 30, 0.002), (20, 14, 0.01)]);
    assert_eq!(chain_levels(100, &chain, 1, Cook::Desktop), desktop);
    assert!(same_levels(&desktop, &chain_levels(100, &chain, 1, Cook::Desktop)));
    assert_eq!(chain_levels(100, &chain, 1, Cook::Mobile), desktop[1..]);
    assert_eq!(chain_levels(100, &chain, 9, Cook::Mobile), desktop[2..], "never the last level");
    // The same triangles with a vertex more, or an error one step off, aren't the chain.
    let built = chain_levels(100, &chain, 0, Cook::Desktop);
    let more = cooked(&[(100, 40, 0.0), (50, 31, 0.002), (20, 14, 0.01)]);
    let off = cooked(&[(100, 40, 0.0), (50, 30, f32::from_bits(0.002_f32.to_bits() + 1)), (20, 14, 0.01)]);
    assert!(!same_levels(&more, &built) && !same_levels(&off, &built));
    assert!(!same_levels(&desktop[1..], &built), "a level fewer");
}

#[test]
fn the_report_reads_back_as_written() {
    let audit = CookAudit {
        cook: "desktop".to_owned(),
        textures: BTreeMap::from([("ILM".to_owned(), ClassBytes { maps: 1, bytes: 1234 })]),
        alpha: AlphaCensus {
            families: BTreeMap::from([(
                "anime character".to_owned(),
                AlphaCounts { opaque: 3, alpha_tested: 1, blended: 0 },
            )]),
            alpha_tested: vec!["a/eye.material.ron".to_owned()],
            blended: Vec::new(),
        },
        maps: vec![MapBytes {
            source: "a/body.ilm.ron".to_owned(),
            output: "a/body.ktx2".to_owned(),
            class: "ILM".to_owned(),
            bytes: 1234,
            levels: 0,
        }],
        materials: Vec::new(),
        meshes: vec![MeshAudit {
            source: "a/body.glb".to_owned(),
            output: "a/body.smesh".to_owned(),
            levels: vec![CookedLevel { triangles: 12, vertices: 8, error: 0.0 }],
            chain: None,
            slots: Vec::new(),
        }],
        tiling: Vec::new(),
    };
    let text = audit.to_ron().unwrap();
    assert!(text.starts_with("(\n    type: \"sapphire::pipeline::CookAudit\",\n    version: 1,"), "{text}");
    assert!(!text.contains("tiling"), "written only when a slot tiles: {text}");
    assert_eq!(CookAudit::from_ron(&text).unwrap(), audit);
    let tiling = CookAudit {
        tiling: vec![TilingSlot {
            source: "a/body.glb".to_owned(),
            slot: "skin".to_owned(),
            span: [0.5, 2.25],
        }],
        ..audit
    };
    let text = tiling.to_ron().unwrap();
    assert!(text.contains("tiling:"), "{text}");
    assert_eq!(CookAudit::from_ron(&text).unwrap(), tiling);
}

#[test]
fn names_are_relative_with_forward_slashes() {
    let root = Path::new("root");
    assert_eq!(relative(&root.join("a").join("b.png"), root), "a/b.png");
}
