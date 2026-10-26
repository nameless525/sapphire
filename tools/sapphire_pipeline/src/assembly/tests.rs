use std::collections::BTreeMap;

use glam::Vec3;
use sapphire_assets::mesh::{Lod, PositionFormat, Submesh, Vertex, VertexFlags};

use super::*;
use crate::mesh::{ImportedMaterial, ImportedMesh};

/// A piece: one upright unit square in the XY plane facing +Z (`0..1` across, `0..1` up), its
/// left triangle in palette cell `[0, 0]` (glTF material `paint`) and its right one in cell
/// `[1, 0]`, the right one's top corner at the cell's top and its bottom ones at its bottom.
fn wall_piece() -> ImportedMesh {
    let vertex = |position: [f32; 3], uv: [f32; 2]| Vertex {
        position,
        normal: [0.0, 0.0, 1.0],
        outline_normal: [0.0, 0.0, 1.0],
        uv,
        color: [255; 3],
        flags: VertexFlags::NONE,
    };
    // Palette 2 × 1: cell 0 is u in 0..0.5, cell 1 u in 0.5..1.
    let vertices = vec![
        vertex([0.0, 0.0, 0.0], [0.25, 0.5]),
        vertex([1.0, 0.0, 0.0], [0.25, 0.5]),
        vertex([0.0, 1.0, 0.0], [0.25, 0.5]),
        vertex([1.0, 0.0, 0.0], [0.75, 1.0]),
        vertex([1.0, 1.0, 0.0], [0.75, 0.0]),
        vertex([0.0, 1.0, 0.0], [0.75, 0.0]),
    ];
    ImportedMesh {
        authored_outline: vec![false; vertices.len()],
        vertices,
        indices: vec![0, 1, 2, 3, 4, 5],
        lods: vec![Lod {
            vertex_count: 6,
            error: 0.0,
            submeshes: vec![Submesh { first_index: 0, index_count: 6, material: 0 }],
        }],
        materials: vec![ImportedMaterial { name: "paint".to_owned(), generated: Default::default() }],
        ..ImportedMesh::default()
    }
}

/// An assembly over [`wall_piece`] (`kit/wall`), with slots `Plaster`, `Timber` and `Stone`.
fn assembly(text: &str) -> Result<Assembly, AssemblyError> {
    let document = format!(
        r#"(
            type: "sapphire::pipeline::Assembly",
            version: 1,
            data: (
                kits: {{ "kit": "pieces" }},
                palette: (columns: 2, rows: 1, gradient_columns: [1]),
                slots: [
                    (name: "Plaster", tile: (2.0, 4.0)),
                    (name: "Timber"),
                    (name: "Stone", projection: Box),
                ],
                {text}
            ),
        )"#
    );
    read(document.as_bytes())
}

const RULES: &str = r#"surfaces: [
    (cell: Some((0, 0)), slot: "Plaster"),
    (cell: Some((1, 0)), slot: "Timber"),
    (pieces: ["wa*"], cell: Some((0, 0)), variant: Some("ground"), slot: "Stone"),
],"#;

fn assemble_text(text: &str) -> Result<ImportedMesh, AssemblyError> {
    let assembly = assembly(text)?;
    let mut imported = Vec::new();
    let mesh = assemble(&assembly, &mut |file: &str| {
        imported.push(file.to_owned());
        Ok(wall_piece())
    })?;
    assert_eq!(imported, ["pieces/wall.glb"], "each piece file is imported once");
    Ok(mesh)
}

/// Each slot's triangles: their three positions, in submesh order.
fn triangles(mesh: &ImportedMesh) -> BTreeMap<String, Vec<[Vec3; 3]>> {
    let mut by_slot = BTreeMap::new();
    for submesh in &mesh.lods[0].submeshes {
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        let list: &mut Vec<_> =
            by_slot.entry(mesh.materials[submesh.material as usize].name.clone()).or_default();
        for corners in mesh.indices[range].as_chunks::<3>().0 {
            list.push(corners.map(|index| Vec3::from(mesh.vertices[index as usize].position)));
        }
    }
    by_slot
}

#[test]
fn sources_are_known_by_their_suffix_and_cook_beside_themselves() {
    assert!(is_source(Path::new("lookdev/town/Town.Assembly.RON")));
    assert!(!is_source(Path::new(".assembly.ron")), "a bare suffix names nothing");
    assert!(!is_source(Path::new("town.ron")));
    assert_eq!(
        output_path(Path::new("lookdev/town/town.assembly.ron"), "smesh"),
        Path::new("lookdev/town/town.smesh")
    );
}

#[test]
fn rules_pick_slots_by_cell_and_variant_and_later_rules_win() {
    let mesh = assemble_text(&format!(
        r#"{RULES}
        place: [
            (piece: "kit/wall"),
            (piece: "kit/wall", at: (0.0, 1.0, 0.0), variant: Some("ground")),
        ],"#
    ))
    .unwrap();
    let names: Vec<&str> = mesh.materials.iter().map(|material| material.name.as_str()).collect();
    assert_eq!(names, ["Plaster", "Timber", "Stone"], "every declared slot, in order");
    let slots = triangles(&mesh);
    assert_eq!(slots["Plaster"].len(), 1);
    assert_eq!(slots["Timber"].len(), 2, "the variant changes only the cell its rule names");
    assert_eq!(slots["Stone"].len(), 1);
    assert!(slots["Stone"][0].iter().all(|p| p.y >= 1.0), "the raised placement's");
    sapphire_assets::mesh::Mesh::read(
        &mesh.pack(PositionFormat::Float32, &BTreeMap::new()).unwrap().to_cooked().unwrap(),
    )
    .expect("a valid mesh");
}

#[test]
fn placements_turn_scale_and_nest() {
    let mesh = assemble_text(&format!(
        r#"{RULES}
        groups: {{ "bay": [(piece: "kit/wall", at: (1.0, 0.0, 0.0))] }},
        place: [(piece: "@bay", at: (10.0, 0.0, 0.0), turn: 90.0, scale: 2.0)],"#
    ))
    .unwrap();
    let slots = triangles(&mesh);
    // The wall's corners at x 1..2 in the group, scaled by 2 and turned a quarter about +Y:
    // +X goes to −Z.
    let all: Vec<Vec3> = slots.values().flatten().flatten().copied().collect();
    for p in &all {
        assert!((p.x - 10.0).abs() < 1e-5, "{p}");
        assert!((-4.0 - 1e-5..=-2.0 + 1e-5).contains(&p.z), "{p}");
        assert!((0.0..=2.0 + 1e-5).contains(&p.y), "{p}");
    }
    // Facing +Z turned to face +X, and still wound counter-clockwise from the front.
    for [a, b, c] in slots.values().flatten() {
        assert!((*b - *a).cross(*c - *a).normalize().abs_diff_eq(Vec3::X, 1e-5));
    }
    for vertex in &mesh.vertices {
        assert!(Vec3::from(vertex.normal).abs_diff_eq(Vec3::X, 1e-5));
    }
}

#[test]
fn mirrored_placements_keep_triangles_facing_out() {
    let mesh =
        assemble_text(&format!(r#"{RULES} place: [(piece: "kit/wall", scale: (-1.0, 1.0, 1.0))],"#)).unwrap();
    for [a, b, c] in triangles(&mesh).values().flatten() {
        assert!((*b - *a).cross(*c - *a).normalize().abs_diff_eq(Vec3::Z, 1e-5), "wound toward its normal");
    }
}

#[test]
fn identical_triangles_are_kept_once() {
    let mesh =
        assemble_text(&format!(r#"{RULES} place: [(piece: "kit/wall"), (piece: "kit/wall", turn: 360.0)],"#))
            .unwrap();
    assert_eq!(triangles(&mesh).values().flatten().count(), 2, "the second wall coincides with the first");
}

#[test]
fn texture_coordinates_measure_meters_on_each_face() {
    // Plaster tiles 2 m across and 4 m down. The wall, scaled 4×, faces +Z: U runs along +X,
    // V down.
    let mesh =
        assemble_text(&format!(r#"{RULES} place: [(piece: "kit/wall", at: (1.0, 0.0, 0.0), scale: 4.0)],"#))
            .unwrap();
    let plaster = mesh.lods[0].submeshes.iter().find(|submesh| submesh.material == 0).unwrap();
    for &index in
        &mesh.indices[plaster.first_index as usize..(plaster.first_index + plaster.index_count) as usize]
    {
        let vertex = &mesh.vertices[index as usize];
        assert_eq!(vertex.uv, [vertex.position[0] / 2.0, -vertex.position[1] / 4.0]);
    }
    // Boxed: a wall turned 30° maps onto the nearest axis plane, X up the side seen from +Z.
    let (u, v) = projection_axes(Projection::Box, Vec3::new(0.5, 0.0, 0.866));
    assert_eq!((u, v), (Vec3::X, -Vec3::Y));
    // A roof facing +Z and up: U level, V down the slope; a floor from above.
    let slope = std::f32::consts::FRAC_1_SQRT_2;
    let (u, v) = projection_axes(Projection::Planar, Vec3::new(0.0, slope, slope));
    assert!(u.abs_diff_eq(Vec3::X, 1e-5) && v.abs_diff_eq(Vec3::new(0.0, -slope, slope), 1e-4), "{u} {v}");
    assert_eq!(projection_axes(Projection::Planar, Vec3::Y), (Vec3::X, Vec3::Z));
}

#[test]
fn occlusion_darkens_walls_toward_the_ground_and_follows_the_kits_gradients() {
    let mesh = assemble_text(&format!(
        r#"{RULES}
        occlusion: (ground: 0.5, ground_height: 1.0, gradient: 0.2),
        place: [(piece: "kit/wall")],"#
    ))
    .unwrap();
    let mut colors: Vec<([f32; 3], u8)> =
        mesh.vertices.iter().map(|vertex| (vertex.position, vertex.color[0])).collect();
    colors.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (on_ground, top) = (unorm8(0.5), 255);
    assert_eq!(
        colors,
        [
            ([0.0, 0.0, 0.0], on_ground),
            ([0.0, 1.0, 0.0], top),
            ([0.0, 1.0, 0.0], top),
            // The right triangle's bottom corner is at its gradient's dark end; the left one's
            // flat cell takes the ground's alone.
            ([1.0, 0.0, 0.0], unorm8(0.5 * 0.8)),
            ([1.0, 0.0, 0.0], on_ground),
            ([1.0, 1.0, 0.0], top),
        ]
    );
    let darkest = mesh.vertices.iter().map(|vertex| vertex.color[0]).min().unwrap();
    assert_eq!(darkest, unorm8(0.5 * 0.8), "the gradient's dark end on the ground");
}

#[test]
fn placement_slots_override_the_rules_and_builtins_take_them() {
    let mesh = assemble_text(&format!(
        r##"{RULES}
        place: [
            (piece: "kit/wall", slot: Some("Stone")),
            (piece: "#box", at: (5.0, 0.5, 0.0), slot: Some("Timber")),
            (piece: "#quad", at: (0.0, -1.0, 0.0), scale: (8.0, 1.0, 8.0), slot: Some("Plaster")),
        ],"##
    ))
    .unwrap();
    let slots = triangles(&mesh);
    // The 8 m quad in 2 m cells: 4 × 4 of them, two triangles each.
    assert_eq!((slots["Stone"].len(), slots["Timber"].len(), slots["Plaster"].len()), (2, 12, 32));
    for [a, b, c] in &slots["Timber"] {
        let center = (*a + *b + *c) / 3.0 - Vec3::new(5.0, 0.5, 0.0);
        assert!((*b - *a).cross(*c - *a).dot(center) > 0.0, "the box's faces point out");
    }
    for [a, b, c] in &slots["Plaster"] {
        assert!((*b - *a).cross(*c - *a).normalize().abs_diff_eq(Vec3::Y, 1e-6), "the quad faces up");
    }
}

#[test]
fn ground_builtins_come_in_small_cells_near_their_center_without_t_junctions() {
    let builtins = |place: &str| {
        let assembly = assembly(&format!("{RULES} place: [{place}],"))?;
        assemble(&assembly, &mut |file: &str| unreachable!("no pieces, not {file}"))
    };
    let mesh = builtins(
        r##"(piece: "#quad", at: (0.0, 0.0, 0.0), scale: (37.2, 1.0, 5.0), slot: Some("Stone")),
            (piece: "#ground", at: (0.0, -0.03, 0.0), scale: (4000.0, 1.0, 4000.0), slot: Some("Plaster"))"##,
    )
    .unwrap();
    let slots = triangles(&mesh);
    // 19 columns (37.2 m) by 3 rows (5 m), two triangles each.
    assert_eq!(slots["Stone"].len(), 19 * 3 * 2);
    let extent = |[a, b, c]: &[Vec3; 3]| a.distance(*b).max(b.distance(*c)).max(c.distance(*a));
    for triangle in slots["Stone"].iter().chain(&slots["Plaster"]) {
        let [a, b, c] = *triangle;
        assert!((b - a).cross(c - a).normalize().abs_diff_eq(Vec3::Y, 1e-5), "faces up: {triangle:?}");
    }
    assert!(slots["Stone"].iter().all(|triangle| extent(triangle) <= GROUND_CELL * 1.5));

    let ground = &slots["Plaster"];
    let reach =
        |triangle: &[Vec3; 3]| triangle.iter().map(|p| Vec3::new(p.x, 0.0, p.z).length()).fold(0.0, f32::max);
    assert!(
        ground.iter().filter(|t| reach(t) <= GROUND_FINE).all(|t| extent(t) <= GROUND_CELL * 1.5),
        "cells near the center are small"
    );
    // Texture coordinates wrapped by blocks: a 2 km radius in 2 m tiles would reach ±1000, where
    // a block and the widest far cell (355 m, 178 tiles) stay under 256.
    assert!(
        mesh.vertices.iter().flat_map(|vertex| vertex.uv).all(|c| (0.0..4.0 * WRAP_TILES).contains(&c)),
        "wrapped texture coordinates"
    );
    let rim = ground.iter().map(reach).fold(0.0, f32::max);
    assert!((rim - 2000.0).abs() < 0.5, "out to its rim: {rim}");
    // No T-junctions: every edge inside the disc is shared by exactly two triangles, and the rim's
    // by one.
    let mut edges: HashMap<[[i32; 3]; 2], u32> = HashMap::new();
    let key = |p: Vec3| (p * 1000.0).round().as_ivec3().to_array();
    for [a, b, c] in ground {
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let mut edge = [key(*p), key(*q)];
            edge.sort_unstable();
            *edges.entry(edge).or_default() += 1;
        }
    }
    let on_rim = |edge: &[[i32; 3]; 2]| {
        edge.iter().all(|p| (Vec3::new(p[0] as f32, 0.0, p[2] as f32).length() / 1000.0 - 2000.0).abs() < 0.5)
    };
    for (edge, count) in &edges {
        assert_eq!(*count, if on_rim(edge) { 1 } else { 2 }, "{edge:?}");
    }

    let open_land = builtins(r##"(piece: "#quad", scale: (600.0, 1.0, 10.0), slot: Some("Stone"))"##);
    assert!(
        matches!(&open_land, Err(AssemblyError::Invalid(message)) if message.contains("#ground")),
        "{open_land:?}"
    );
}

#[test]
fn documents_that_cant_assemble_fail_naming_why() {
    let unclaimed =
        assemble_text(r#"surfaces: [(cell: Some((0, 0)), slot: "Plaster")], place: [(piece: "kit/wall")],"#);
    assert!(
        matches!(&unclaimed, Err(AssemblyError::Unclaimed { piece, cell: [1, 0], .. }) if piece == "kit/wall"),
        "{unclaimed:?}"
    );
    let errors = [
        (r#"surfaces: [(slot: "Brick")], place: []"#, "Brick"),
        (r#"surfaces: [], place: [(piece: "@nowhere")]"#, "nowhere"),
        (r#"surfaces: [], place: [(piece: "other/wall")]"#, "other/wall"),
        (r#"surfaces: [], place: [(piece: "kit/wall", scale: 0.0)]"#, "nonzero"),
        (r#"surfaces: [(cell: Some((2, 0)), slot: "Stone")], place: []"#, "outside"),
    ];
    for (text, needle) in errors {
        let error = assembly(text).expect_err(text).to_string();
        assert!(error.contains(needle), "{text}: {error}");
    }
    let unslotted = assemble_text(r##"surfaces: [], place: [(piece: "#box")]"##);
    assert!(
        matches!(&unslotted, Err(AssemblyError::BuiltinWithoutSlot(piece)) if piece == "#box"),
        "{unslotted:?}"
    );
    let cycle = assemble_text(r#"surfaces: [], groups: { "a": [(piece: "@a")] }, place: [(piece: "@a")],"#);
    assert!(matches!(cycle, Err(AssemblyError::Cycle(_))), "{cycle:?}");
    assert!(matches!(assemble_text(r#"surfaces: [], place: [],"#), Err(AssemblyError::Empty)));
}

#[test]
fn patterns_match_runs_of_characters() {
    for (pattern, name, expected) in [
        ("wall-*", "wall-door", true),
        ("wall-*", "wall", false),
        ("*-door", "wall-wood-door", true),
        ("wall*door", "wall-wood-door", true),
        ("wall*wood*", "wall-door", false),
        ("roof", "roof", true),
        ("roof", "roof-high", false),
        ("*", "anything", true),
    ] {
        assert_eq!(matches_pattern(pattern, name), expected, "{pattern} {name}");
    }
}

#[test]
fn files_are_the_pieces_placed_anywhere() {
    let assembly = assembly(
        r##"surfaces: [], groups: { "g": [(piece: "kit/b"), (piece: "kit/a")] }, place: [(piece: "kit/b"), (piece: "@g"), (piece: "#box", slot: Some("Stone"))],"##,
    )
    .unwrap();
    assert_eq!(assembly.files().unwrap(), ["pieces/a.glb", "pieces/b.glb"]);
}
