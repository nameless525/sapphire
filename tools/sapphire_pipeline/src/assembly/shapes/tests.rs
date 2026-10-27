//! Tests of the parametric builtins: each shape's geometry (finite, no slivers, faces wound to
//! their normals, within its bounds, in its parts) and the features that make it what it is.

use std::collections::BTreeSet;

use glam::{Vec2, Vec3};

use super::*;
use crate::assembly::coplanar;

fn read(text: &str) -> Shape {
    sapphire_serialize::ron::from_str(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

/// A shape's triangles, checked: finite, not degenerate, each corner's normal unit and on its
/// front side (within 80°: smooth normals lean across creases), and the parts it draws in.
fn build(shape: &Shape) -> (Vec<PieceTriangle>, BTreeSet<&'static str>) {
    build_in(shape, Detail::Full)
}

/// [`build`] in `detail`.
fn build_in(shape: &Shape, detail: Detail) -> (Vec<PieceTriangle>, BTreeSet<&'static str>) {
    let triangles = shape.triangles(detail).unwrap();
    assert!(!triangles.is_empty());
    let mut parts = BTreeSet::new();
    for (index, triangle) in triangles.iter().enumerate() {
        let [a, b, c] = triangle.positions;
        assert!(
            a.is_finite() && b.is_finite() && c.is_finite(),
            "{}: triangle {index} {:?}",
            shape.name(),
            triangle.positions
        );
        let face = (b - a).cross(c - a);
        assert!(
            face.length() > 1e-7,
            "{}: triangle {index} is a sliver: {:?}",
            shape.name(),
            triangle.positions
        );
        let face = face.normalize();
        for normal in triangle.normals {
            if normal == Vec3::ZERO {
                continue;
            }
            assert!((normal.length() - 1.0).abs() < 1e-3, "{}: a unit normal", shape.name());
            assert!(
                normal.dot(face) > 0.17,
                "{}: triangle {index} ({}) faces {face} against its normal {normal}",
                shape.name(),
                Part::ALL[triangle.material].name()
            );
        }
        assert!(triangle.occlusion.iter().all(|o| (0.0..=1.0).contains(o)));
        parts.insert(Part::ALL[triangle.material].name());
    }
    (triangles, parts)
}

fn bounds(triangles: &[PieceTriangle]) -> (Vec3, Vec3) {
    triangles
        .iter()
        .flat_map(|t| t.positions)
        .fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(p), hi.max(p)))
}

/// Faces of one shape that overlap in one plane, in different parts or shading differently: a
/// fight inside the shape, which the cook's check (between placements) can't see.
fn self_fights(triangles: &[PieceTriangle]) -> usize {
    let faces: Vec<coplanar::Face> = triangles
        .iter()
        .enumerate()
        .map(|(index, t)| {
            let normal = (t.positions[1] - t.positions[0]).cross(t.positions[2] - t.positions[0]).normalize();
            let uv = match t.uv {
                ShapeUv::Project => [Vec2::ZERO; 3],
                ShapeUv::Meters(uv) | ShapeUv::Tiles(uv) => uv,
            };
            coplanar::Face {
                corners: t.positions,
                normal,
                slot: t.material,
                placement: index,
                shading: [0, 1, 2].map(|k| coplanar::Shading {
                    normal,
                    uv: uv[k],
                    occlusion: t.occlusion[k],
                }),
            }
        })
        .collect();
    coplanar::overlaps(&faces).len()
}

fn hip_gable() -> Shape {
    read("Roof((size: (7.0, 5.0), kind: HipGable(0.5), rise: 2.3, overhang: 0.9, barge: 0.35))")
}

#[test]
fn every_roof_kind_builds_clean_and_in_its_bounds() {
    for text in [
        "Roof((size: (8.0, 6.0), kind: Hip))",
        "Roof((size: (4.6, 4.6), kind: Hip, rise: 2.8, overhang: 1.0, sweep: 0.5, ridge: (height: 0.3, width: 0.28, ends: 1.1)))",
        "Roof((size: (7.0, 5.0), kind: HipGable(0.5), rise: 2.3, overhang: 0.9, barge: 0.35))",
        "Roof((size: (6.4, 5.0), kind: Gable, rise: 2.0, overhang: 0.8, barge: 0.5, sweep: 0.3))",
        "Roof((size: (8.0, 6.0), kind: Skirt(1.3), rise: 0.9, overhang: 0.8, sweep: 0.25, sweep_reach: 1.6))",
    ] {
        let shape = read(text);
        let Shape::Roof(roof) = &shape else { unreachable!() };
        let (triangles, parts) = build(&shape);
        for part in ["tiles", "ridge", "eaves"] {
            assert!(parts.contains(part), "{text}: {part}");
        }
        let gabled = matches!(roof.kind, RoofKind::Gable | RoofKind::HipGable(_));
        assert_eq!(parts.contains("gable"), gabled, "{text}");
        let (lo, hi) = bounds(&triangles);
        // Within the eaves (and the hips' tips past them), over the walls' top less the eaves'
        // drop, and under the rise, the tiles, the ridge and its ornaments.
        let reach = Vec2::from(roof.size) * 0.5 + Vec2::splat(roof.overhang.max(roof.barge) + 0.6);
        assert!(
            lo.x >= -reach.x && hi.x <= reach.x && lo.z >= -reach.y && hi.z <= reach.y,
            "{text}: {lo} {hi}"
        );
        assert!(
            lo.y > -roof.rise && hi.y < roof.rise + roof.ridge.height * 2.0 + roof.ridge.ends + 0.6,
            "{text}: {lo} {hi}"
        );
        assert_eq!(self_fights(&triangles), 0, "{text}");
    }
}

#[test]
fn eaves_sweep_up_toward_the_corners_and_the_soffit_meets_the_walls() {
    let Shape::Roof(roof) = hip_gable() else { unreachable!() };
    let (triangles, _) = build(&hip_gable());
    // The front eave's tile edge: its highest point near the middle and near the corner.
    let eave_z = 0.5 * roof.size[1] + roof.overhang;
    let edge = |x0: f32, x1: f32| {
        triangles
            .iter()
            .filter(|t| Part::ALL[t.material] == Part::Tiles)
            .flat_map(|t| t.positions)
            .filter(|p| (p.z - eave_z).abs() < 1e-3 && p.x >= x0 && p.x <= x1)
            .map(|p| p.y)
            .fold(f32::MIN, f32::max)
    };
    let middle = edge(-0.5, 0.5);
    let corner = edge(eave_z.max(0.5 * roof.size[0] + roof.overhang) - 0.3, 100.0);
    assert!(corner > middle + 0.6 * roof.sweep, "the corner sweeps up: {corner} against {middle}");
    // The soffit reaches the walls' line at the walls' top.
    let at_wall = triangles
        .iter()
        .filter(|t| Part::ALL[t.material] == Part::Eaves)
        .flat_map(|t| t.positions)
        .filter(|p| (p.z - 0.5 * roof.size[1]).abs() < 1e-3 && p.x.abs() < 1.0)
        .map(|p| p.y)
        .collect::<Vec<_>>();
    assert!(!at_wall.is_empty() && at_wall.iter().all(|y| y.abs() < 1e-3), "{at_wall:?}");
}

#[test]
fn tiles_run_in_rows_with_channels_and_crests() {
    let Shape::Roof(roof) = hip_gable() else { unreachable!() };
    let (triangles, _) = build(&hip_gable());
    // Along a line across the front slope, the tiles' heights rise and fall once a period.
    let z = 0.5 * roof.size[1];
    let mut heights: Vec<(f32, f32)> = triangles
        .iter()
        .filter(|t| Part::ALL[t.material] == Part::Tiles)
        .flat_map(|t| t.positions)
        .filter(|p| (p.z - z).abs() < 1e-3 && p.x.abs() < 1.5)
        .map(|p| (p.x, p.y))
        .collect();
    heights.sort_by(|a, b| a.0.total_cmp(&b.0));
    heights.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-4);
    let (low, high) = heights.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(_, y)| (lo.min(y), hi.max(y)));
    assert!((high - low - roof.tiles.depth).abs() < 0.01, "crests stand {} over channels", high - low);
    let crests = heights.iter().filter(|&&(_, y)| y > high - 1e-3).count();
    let expected = (3.0 / roof.tiles.period).round() as usize;
    assert!(crests.abs_diff(expected) <= 1, "{crests} crests in 3 m, {expected} expected");
}

#[test]
fn halls_cut_their_openings_and_stand_on_their_plinth() {
    let text = "Hall((
        size: (8.0, 6.0, 3.6),
        posts: Some((width: 0.24, proud: 0.04, bays: (4, 2))),
        beams: [(at: 3.6, height: 0.28, proud: 0.07)],
        openings: [
            (kind: Window, at: (-3.0, 1.1), size: (1.3, 1.3), lattice: (3, 2)),
            (kind: Door, at: (-1.0, 0.0), size: (1.3, 2.5)),
            (side: Right, kind: Moon, at: (1.5, 1.1), size: (1.2, 1.2), lattice: (2, 2)),
            (side: Back, kind: Gate, at: (-1.0, 0.0), size: (1.4, 1.35)),
        ],
        flat_top: true,
    ))";
    let shape = read(text);
    let (triangles, parts) = build(&shape);
    assert_eq!(
        parts,
        ["door", "lattice", "plaster", "plinth", "timber", "trim", "window"].into_iter().collect(),
        "every part a hall has"
    );
    let (lo, hi) = bounds(&triangles);
    assert!((lo.y).abs() < 1e-5 && (hi.y - 3.6).abs() < 1e-5, "{lo} {hi}");
    assert!(
        (hi.x - (4.0 + 0.07)).abs() < 1e-4 && (hi.z - (3.0 + 0.07)).abs() < 1e-4,
        "the beam stands proudest: {hi}"
    );
    // No plaster crosses an opening's middle on the front face: rays in at the window's and
    // the door's middles meet paper and a door first.
    let first = |origin: Vec3, direction: Vec3| {
        let mut best: Option<(f32, &str)> = None;
        for t in &triangles {
            if let Some(distance) = ray_triangle(origin, direction, t.positions)
                && best.is_none_or(|(d, _)| distance < d)
            {
                best = Some((distance, Part::ALL[t.material].name()));
            }
        }
        best.map(|(_, part)| part)
    };
    // Between the window's lattice bars, and beside the door's stile.
    assert_eq!(first(Vec3::new(-3.0 + 0.16, 1.1 + 0.2, 5.0), -Vec3::Z), Some("window"));
    assert_eq!(first(Vec3::new(-1.0 + 0.3, 1.0, 5.0), -Vec3::Z), Some("door"));
    assert_eq!(first(Vec3::new(2.6, 2.0, 5.0), -Vec3::Z), Some("plaster"));
    assert_eq!(first(Vec3::new(2.6, 0.25, 5.0), -Vec3::Z), Some("plinth"));
    // The round window in the right side, between its bars, and the gate in the back.
    assert_eq!(first(Vec3::new(5.0, 1.7, -1.5), -Vec3::X), Some("window"));
    assert_eq!(first(Vec3::new(1.0, 0.6, -5.0), Vec3::Z), Some("door"));
    assert_eq!(self_fights(&triangles), 0);
}

#[test]
fn walls_let_their_gates_and_windows_through() {
    let text = "Wall((
        size: (6.0, 2.8, 0.34),
        openings: [
            (kind: Gate, size: (2.2, 2.1)),
            (kind: Window, at: (-2.1, 1.1), size: (0.8, 0.8), lattice: (2, 2)),
        ],
    ))";
    let shape = read(text);
    let (triangles, parts) = build(&shape);
    for part in ["plaster", "plinth", "trim", "timber", "lattice", "tiles", "ridge", "gable"] {
        assert!(parts.contains(part), "{part}");
    }
    let through =
        |origin: Vec3| triangles.iter().all(|t| ray_triangle(origin, -Vec3::Z, t.positions).is_none());
    assert!(through(Vec3::new(0.0, 1.0, 2.0)), "the moon gate is a way through");
    assert!(through(Vec3::new(-2.1, 1.5, 2.0)), "the lattice window is seen through");
    assert!(!through(Vec3::new(2.0, 2.0, 2.0)), "the wall is solid elsewhere");
    let (lo, hi) = bounds(&triangles);
    assert!(hi.y > 2.8 + 0.1 && hi.y < 2.8 + 0.6, "a coping on top: {hi}");
    assert!(lo.z < -0.17 - 0.1 && hi.z > 0.17 + 0.1, "the coping overhangs both faces: {lo} {hi}");
    assert_eq!(self_fights(&triangles), 0);
}

#[test]
fn columns_and_lanterns_build_in_their_parts() {
    let (triangles, parts) =
        build(&read("Column((height: 5.15, radius: 0.2, base: (0.36, 0.5), cap: (0.5, 0.25)))"));
    assert_eq!(parts, ["plinth", "timber"].into_iter().collect());
    let (lo, hi) = bounds(&triangles);
    assert!((hi.y - 5.15).abs() < 1e-4 && lo.y.abs() < 1e-5 && (hi.x - 0.36).abs() < 1e-3, "{lo} {hi}");
    assert_eq!(self_fights(&triangles), 0);
    for (text, top, bottom) in [
        ("Lantern((size: (0.42, 0.5), mount: Hang(0.3)))", 0.0, -1.2),
        ("Lantern((kind: Drum, size: (0.4, 0.6), mount: Bare))", 0.75, 0.0),
        ("Lantern((kind: Square, size: (0.34, 0.46), mount: Post(1.25)))", 2.2, 0.0),
    ] {
        let (triangles, parts) = build(&read(text));
        assert!(parts.contains("paper") && parts.contains("frame"), "{text}: {parts:?}");
        let (lo, hi) = bounds(&triangles);
        assert!(hi.y <= top + 1e-4 && lo.y >= bottom - 1e-4, "{text}: {lo} {hi}");
        // The paper takes its map a rib a tile.
        let paper = triangles.iter().find(|t| Part::ALL[t.material] == Part::Paper).unwrap();
        assert!(matches!(paper.uv, ShapeUv::Tiles(_)), "{text}");
        assert_eq!(self_fights(&triangles), 0, "{text}");
    }
}

#[test]
fn bad_parameters_fail_naming_why() {
    for (text, expected) in [
        ("Roof((size: (5.0, 7.0), kind: Hip))", "ridge runs along its width"),
        ("Roof((size: (8.0, 6.0), kind: HipGable(0.1)))", "gable must start above"),
        ("Roof((size: (8.0, 6.0), kind: Skirt(0.5), overhang: 0.8))", "skirt's run"),
        ("Roof((size: (8.0, 6.0), curve: 1.5))", "curve"),
        (
            "Hall((openings: [(kind: Window, at: (2.8, 1.0), size: (1.0, 1.0))]))",
            "reaches past its side's ends",
        ),
        ("Hall((openings: [(kind: Window, at: (0.0, 0.2), size: (1.0, 1.0))]))", "above the plinth"),
        (
            "Hall((openings: [(kind: Window, at: (0.0, 1.0), size: (1.0, 1.0)), (kind: Door, at: (0.5, 0.0), size: (1.0, 2.2))]))",
            "overlaps opening 0",
        ),
        ("Hall((openings: [(kind: Door, at: (0.0, 0.3), size: (1.0, 2.0))]))", "stands on the ground"),
        ("Hall((openings: [(kind: Moon, at: (0.0, 1.0), size: (1.0, 1.2))]))", "is round"),
        ("Hall((openings: [(kind: Gate, size: (1.6, 2.4))]))", "circle must reach the ground"),
        ("Hall((posts: Some((proud: 0.05)), beams: [(at: 3.0, proud: 0.02)]))", "prouder than posts"),
        ("Wall((openings: [(side: Back, kind: Gate, size: (2.0, 1.9))]))", "from its front"),
        (
            "Hall((posts: Some((bays: (2, 1))), openings: [(kind: Window, at: (0.3, 1.0), size: (1.0, 1.0))]))",
            "crosses a post",
        ),
        (
            "Hall((posts: Some(()), openings: [(kind: Window, at: (2.2, 1.0), size: (1.0, 1.0))]))",
            "crosses a post",
        ),
        ("Column((radius: 0.3, base: (0.2, 0.3)))", "drum must be wider"),
        ("Lantern((size: (0.0, 0.5)))", "must be positive"),
        ("Stall((size: (0.4, 1.6)))", "leave no room"),
        ("Stall((counter: Some((height: 2.0))))", "well under the beams"),
        ("Stall((shelves: (depth: 0.9)))", "aisle"),
        ("Stall((awning: Tiles((kind: Skirt(0.5)))))", "hip, hip-and-gable or gable"),
        ("Stall((awning: Cloth((rise: 0.4, sag: 0.2))))", "sags at most"),
        ("Stall((awning: Cloth((valance: (1.6, 0.2)))))", "upper half"),
        ("Stall((sign: Some((size: (3.0, 0.3)))))", "along the front beam"),
        ("Stall((goods: [(kind: Jar, on: Shelf(2))]))", "no shelf 2"),
        ("Stall((goods: [(kind: Crate(Fruit), at: (1.0, 0.0), size: 0.5)]))", "reach past"),
        (
            "Stall((goods: [(kind: Jar, size: 0.3), (kind: Basket(Greens), at: (0.2, 0.0), size: 0.3)]))",
            "overlap goods 0",
        ),
        ("Stall((shelves: (count: 6), goods: [(kind: Jar, on: Shelf(0), size: 0.28)]))", "too tall"),
        ("Stall((goods: [(kind: Jar, on: Ground, size: 0.3)]))", "outside the frame"),
        ("Stall((goods: [(on: Bed)]))", "no bed"),
        ("Cart((wheels: (spokes: 2)))", "3 to 24 spokes"),
        ("Cart((goods: [(on: Counter)]))", "bed or the ground"),
    ] {
        let error = read(text).validate().unwrap_err();
        assert!(error.contains(expected), "{text}: {error}");
    }
}

/// The mobile cook's light detail builds as clean as full detail, in the same parts and without
/// fights of its own, and lighter: roofs (tile rows) by a third at least.
#[test]
fn light_detail_builds_clean_and_lighter() {
    for text in [
        "Roof((size: (7.0, 5.0), kind: HipGable(0.5), rise: 2.3, overhang: 0.9, barge: 0.35))",
        "Roof((size: (8.0, 6.0), kind: Skirt(1.3), rise: 0.9, overhang: 0.8, sweep: 0.25, sweep_reach: 1.6))",
        "Roof((size: (6.4, 5.0), kind: Gable, rise: 2.0, overhang: 0.8, barge: 0.5, sweep: 0.3))",
        "Roof((size: (5.0, 5.0), kind: Hip, rise: 2.0))",
        "Wall((size: (6.0, 2.8, 0.34), openings: [(kind: Gate, size: (2.2, 2.1))]))",
        "Lantern((size: (0.42, 0.5), mount: Hang(0.3)))",
        "Lantern((kind: Drum, size: (0.4, 0.5), mount: Bare))",
        "Column((height: 5.15, radius: 0.2, base: (0.36, 0.5), cap: (0.5, 0.25)))",
        STALL,
        "Stall((awning: Cloth(()), goods: [(kind: Basket(Greens)), (kind: Crate(Fruit), on: Ground, at: (0.0, 1.3))]))",
        "Stall((awning: Cloth((kind: Shed)), goods: [(kind: Bolts, at: (-0.5, 0.0), size: 0.3, count: 2)]))",
        CART,
    ] {
        let shape = read(text);
        let (full, full_parts) = build_in(&shape, Detail::Full);
        let (light, light_parts) = build_in(&shape, Detail::Light);
        assert_eq!(light_parts, full_parts, "{text}");
        assert_eq!(self_fights(&light), 0, "{text}");
        let ceiling = if matches!(shape, Shape::Roof(_)) { 2 * full.len() / 3 } else { full.len() };
        assert!(light.len() <= ceiling, "{text}: {} light triangles of {}", light.len(), full.len());
        let ((lo, hi), (light_lo, light_hi)) = (bounds(&full), bounds(&light));
        assert!(lo.distance(light_lo) < 0.1 && hi.distance(light_hi) < 0.1, "{text}: the same size");
    }
}

/// A stall with a little of everything: a tiled hip roof, a sign, goods on the counter, both
/// shelves and the ground.
const STALL: &str = "Stall((
    sign: Some((size: (0.9, 0.3))),
    goods: [
        (kind: Basket(Fruit), at: (-0.7, 0.0), size: 0.4),
        (kind: Crate(Greens), at: (0.0, 0.0), size: 0.45),
        (kind: Jar, at: (0.75, 0.0), size: 0.3),
        (kind: Jar, on: Shelf(1), size: 0.22, count: 4),
        (kind: Bolts, on: Shelf(0), at: (-0.6, 0.0), size: 0.28),
        (kind: Basket(Empty), on: Ground, at: (0.0, 1.3), size: 0.45),
    ],
))";

/// A cart with goods in its bed and a jar beside it.
const CART: &str = "Cart((
    goods: [
        (kind: Crate(Fruit), on: Bed, at: (-0.2, -0.3), size: 0.5),
        (kind: Basket(Greens), on: Bed, at: (0.25, 0.3), size: 0.4),
        (kind: Jar, on: Ground, at: (1.1, 0.4), size: 0.35),
    ],
))";

#[test]
fn every_stall_builds_clean_and_in_its_bounds() {
    for text in [
        STALL,
        "Stall((awning: Tiles((kind: Gable)), sign: Some((at: 0.5, size: (0.4, 0.6))), shelves: (count: 0)))",
        "Stall((size: (3.2, 1.8), awning: Tiles((kind: HipGable(0.55))), counter: None))",
        "Stall((awning: Cloth(()), goods: [(kind: Basket(Greens), size: 0.3, count: 3)]))",
        "Stall((awning: Cloth((kind: Shed, valance: (0.0, 0.2))), shelves: (back: false)))",
    ] {
        let shape = read(text);
        let Shape::Stall(stall) = &shape else { unreachable!() };
        let (triangles, parts) = build(&shape);
        let (cloth, valance) = match stall.awning {
            Awning::Cloth(cloth) => (true, cloth.valance[0] > 0.0),
            Awning::Tiles(_) => (false, false),
        };
        for part in ["timber", "plinth"] {
            assert!(parts.contains(part), "{text}: {part}");
        }
        let awning: &[&str] = if cloth { &["cloth"] } else { &["tiles", "ridge", "eaves"] };
        for part in awning {
            assert!(parts.contains(part), "{text}: {part}");
        }
        let bolts = stall.goods.iter().any(|goods| goods.kind == GoodsKind::Bolts);
        assert_eq!(parts.contains("valance"), valance || bolts, "{text}");
        assert_eq!(parts.contains("sign"), stall.sign.is_some(), "{text}");
        assert_eq!(parts.contains("boards"), stall.counter.is_some(), "{text}");
        assert_eq!(parts.contains("back"), stall.shelves.back, "{text}");
        let (lo, hi) = bounds(&triangles);
        // On the ground, within the awning's reach (and the goods' before it), and under its top.
        let reach = Vec2::from(stall.size) * 0.5 + Vec2::splat(0.9);
        assert!(lo.y.abs() < 1e-5, "{text}: {lo}");
        assert!(
            lo.x >= -reach.x && hi.x <= reach.x && lo.z >= -reach.y && hi.z <= reach.y + 0.8,
            "{text}: {lo} {hi}"
        );
        assert!(hi.y > stall.height + 0.3 && hi.y < stall.height + 2.0, "{text}: {hi}");
        assert_eq!(self_fights(&triangles), 0, "{text}");
    }
}

#[test]
fn goods_stand_on_their_surfaces_under_the_roof() {
    let (triangles, parts) = build(&read(STALL));
    for part in ["wicker", "pottery", "fruit", "produce", "cloth", "valance", "boards", "sign"] {
        assert!(parts.contains(part), "{part}");
    }
    let down = -Vec3::Y;
    let hit = |origin: Vec3, direction: Vec3| first(&triangles, origin, direction).map(|(_, part)| part);
    // The counter's goods stand along its middle, half a meter in: into the basket's heap and
    // the crate's, onto the jar's lid, and onto the counter's bare top at 0.9 m.
    assert_eq!(hit(Vec3::new(-0.7, 2.0, 0.5), down), Some("fruit"));
    assert_eq!(hit(Vec3::new(0.0, 2.0, 0.5), down), Some("produce"));
    assert_eq!(hit(Vec3::new(0.75, 2.0, 0.5), down), Some("pottery"));
    let (distance, part) = first(&triangles, Vec3::new(1.05, 2.0, 0.5), down).unwrap();
    assert!(part == "timber" && (2.0 - distance - 0.9).abs() < 1e-4, "the counter's top: {part} {distance}");
    // The roof's ridge over it all; from the front, the empty basket on the ground and the
    // counter's boards behind it.
    assert_eq!(hit(Vec3::new(0.0, 5.0, 0.0), down), Some("ridge"));
    assert_eq!(hit(Vec3::new(0.1, 0.1, 3.0), -Vec3::Z), Some("wicker"));
    assert_eq!(hit(Vec3::new(0.6, 0.4, 3.0), -Vec3::Z), Some("boards"));
    // Every basket's and jar's lowest point is on a surface: the ground, the counter or a shelf.
    let surfaces = [0.0, 0.9, 0.72, 1.44];
    for part in ["wicker", "pottery"] {
        let lowest = triangles
            .iter()
            .filter(|t| Part::ALL[t.material].name() == part)
            .flat_map(|t| t.positions)
            .map(|p| p.y)
            .fold(f32::MAX, f32::min);
        assert!(surfaces.iter().any(|s| (lowest - s).abs() < 1e-4), "{part} stands at {lowest}");
    }
}

#[test]
fn cloth_awnings_sweep_up_at_their_corners_and_lie_over_their_poles() {
    let shape = read("Stall((awning: Cloth(())))");
    let Shape::Stall(stall) = &shape else { unreachable!() };
    let Awning::Cloth(cloth) = stall.awning else { unreachable!() };
    let (triangles, _) = build(&shape);
    // Along the front eave's edge: highest near the corners.
    let eave = 0.5 * stall.size[1] + cloth.overhang;
    let edge = |x0: f32, x1: f32| {
        triangles
            .iter()
            .filter(|t| Part::ALL[t.material] == Part::Cloth)
            .flat_map(|t| t.positions)
            .filter(|p| (p.z - eave).abs() < 1e-4 && p.x.abs() >= x0 && p.x.abs() <= x1)
            .map(|p| p.y)
            .fold(f32::MIN, f32::max)
    };
    let (middle, corner) = (edge(0.0, 0.3), edge(1.3, 10.0));
    assert!(corner > middle + 0.6 * cloth.sweep, "the corners sweep up: {corner} against {middle}");
    let hits = |origin: Vec3| {
        let mut hits: Vec<(f32, &str)> = triangles
            .iter()
            .filter_map(|t| {
                ray_triangle(origin, -Vec3::Y, t.positions).map(|d| (d, Part::ALL[t.material].name()))
            })
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits
    };
    // Down between the ridge and the eaves: the cloth's top and underside, over the frame's
    // top, then the counter.
    let through = hits(Vec3::new(0.3, 5.0, 0.4));
    assert_eq!((through[0].1, through[1].1), ("cloth", "cloth"), "{through:?}");
    assert!(through[2].1 != "cloth" && 5.0 - through[1].0 > stall.height, "{through:?}");
    // Down beside the ridge: the cloth, then the ridge pole just under it.
    let ridge = hits(Vec3::new(0.3, 5.0, 0.01));
    assert_eq!((ridge[0].1, ridge[1].1, ridge[2].1), ("cloth", "cloth", "timber"), "{ridge:?}");
    assert!(ridge[2].0 - ridge[1].0 < 0.03, "the pole holds the cloth: {ridge:?}");
}

#[test]
fn carts_stand_level_on_spoked_wheels() {
    let shape = read(CART);
    let Shape::Cart(cart) = &shape else { unreachable!() };
    let (triangles, parts) = build(&shape);
    for part in ["timber", "boards", "iron", "fruit", "produce", "wicker", "pottery"] {
        assert!(parts.contains(part), "{part}");
    }
    let radius = cart.wheels.radius;
    // The tyres reach the ground and no further from the axle than the wheels' radius.
    let axle_z = -0.12 * cart.bed[1];
    let tyre: Vec<Vec3> =
        triangles.iter().filter(|t| Part::ALL[t.material] == Part::Iron).flat_map(|t| t.positions).collect();
    assert!(tyre.iter().all(|p| Vec2::new(p.y - radius, p.z - axle_z).length() < radius + 1e-4));
    let lowest = tyre.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    assert!(lowest.abs() < 1e-4, "the tyres meet the ground: {lowest}");
    let (lo, hi) = bounds(&triangles);
    assert!(lo.y.abs() < 1e-4 && hi.z <= 0.5 * cart.bed[1] + cart.shafts + 1e-4, "{lo} {hi}");
    // Along the axle into the left wheel, a quarter meter out from its middle: a spoke at a
    // spoke's angle, and between spokes through the wheel.
    let wheel_x = 0.5 * cart.bed[0] + 0.05 + 0.5 * cart.wheels.rim[1];
    let at = |angle: f32| {
        let out = Vec3::new(0.0, -angle.cos(), -angle.sin());
        first(&triangles, Vec3::new(-3.0, radius, axle_z) + 0.25 * out, Vec3::X)
    };
    let pitch = std::f32::consts::TAU / cart.wheels.spokes as f32;
    let (distance, part) = at(0.5 * pitch).unwrap();
    assert!(part == "timber" && (distance - (3.0 - wheel_x)).abs() < 0.05, "a spoke: {part} at {distance}");
    let between = at(pitch).map_or(f32::MAX, |(distance, _)| distance);
    assert!(between > 3.0 - wheel_x + 0.1, "between spokes, through the wheel: {between}");
    assert_eq!(self_fights(&triangles), 0);
}

/// The nearest triangle along `direction` from `origin`: its distance and part.
fn first(triangles: &[PieceTriangle], origin: Vec3, direction: Vec3) -> Option<(f32, &'static str)> {
    triangles
        .iter()
        .filter_map(|t| {
            ray_triangle(origin, direction, t.positions).map(|d| (d, Part::ALL[t.material].name()))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
}

/// The distance along `direction` (unit) from `origin` to where it meets `triangle` from its
/// front or back, if it does.
fn ray_triangle(origin: Vec3, direction: Vec3, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = direction.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let t = origin - a;
    let u = t.dot(p) / det;
    let q = t.cross(e1);
    let v = direction.dot(q) / det;
    let distance = e2.dot(q) / det;
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && distance > 0.0).then_some(distance)
}
