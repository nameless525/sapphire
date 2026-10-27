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
