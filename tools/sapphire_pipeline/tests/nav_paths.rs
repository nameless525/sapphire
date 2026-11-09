//! Whether a polygon graph is enough, on real geometry: paths across the square, round its stalls,
//! through a doorway and up a stair, for a capsule of 0.3 m by 1.6 m, with no detail mesh and no
//! off-mesh links. Each path is held to the arena's own colliders, by distance, not by eye.

mod nav_support;

use std::io::Write as _;

use glam::Vec3;
use nav_support::{arena_colliders, arena_triangles, proving_ground, town_triangles};
use sapphire_colliders::{
    ColliderDocument, ColliderEntry, ColliderSet, ColliderShape, CookedShape, Group, Rotation,
};
use sapphire_nav::{
    FunnelEnd, Location, NavGraph, Navigation, Path, PathSearch, Reach, RouteEnd, Steer, straighten,
};
use sapphire_navmesh::NavMesh;
use sapphire_pipeline::nav::{
    BakeSettings, Baked, TileBounds, Triangle, bake, bake_tile, colliders_to_triangles, stitch,
};

const ARENA: TileBounds = ([-4, -4], [3, 3]);
const TOWN: TileBounds = ([-6, -6], [5, 5]);
const CAPSULE_RADIUS: f32 = 0.3;
const CAPSULE_HEIGHT: f32 = 1.6;

fn say(line: String) {
    writeln!(std::io::stdout(), "NAV-PATH {line}").expect("it prints");
}

fn graph_of(baked: &Baked) -> NavGraph {
    let bytes = baked.mesh.to_cooked().expect("it writes");
    NavGraph::new(NavMesh::read(&bytes).expect("it reads"))
}

struct Walked {
    end: RouteEnd,
    polygons: Vec<u32>,
    corners: Vec<Vec3>,
    from: Location,
    to: Location,
}

fn walk(graph: &NavGraph, from: [f32; 3], to: [f32; 3]) -> Walked {
    let at = |point: [f32; 3]| {
        graph
            .locate(Vec3::from(point), Reach::CHARACTER)
            .unwrap_or_else(|| panic!("{point:?} isn't on the mesh"))
    };
    let (from, to) = (at(from), at(to));
    let mut search = PathSearch::new(graph, u32::MAX);
    let mut corridor = vec![0; 2048];
    let route = search.find(graph, from, to, &mut corridor);
    let polygons = corridor[..route.polygons as usize].to_vec();
    let mut corners = vec![Vec3::ZERO; 256];
    let (count, funnel) = straighten(graph, &polygons, from.position, to.position, &mut corners);
    assert_eq!(funnel, FunnelEnd::Whole);
    corners.truncate(count as usize);
    Walked { end: route.end, polygons, corners, from, to }
}

fn length(corners: &[Vec3]) -> f32 {
    corners.windows(2).map(|pair| pair[0].distance(pair[1])).sum()
}

/// How far a capsule standing at `foot` is from a collider's sides, on the XZ plane: negative inside.
/// Only what stands in the capsule's height counts, and only the arena's colliders' shapes.
fn gap_to(set: &ColliderSet, index: usize, foot: Vec3) -> Option<f32> {
    let collider = &set.colliders[index];
    let at = Vec3::from(collider.position);
    let (half_height, across): (f32, Result<[f32; 2], f32>) = match collider.shape {
        CookedShape::Box { half_extents: [x, y, z] } => (y, Ok([x, z])),
        CookedShape::Capsule { half_height, radius } => (half_height + radius, Err(radius)),
        CookedShape::Sphere { radius } => (radius, Err(radius)),
        CookedShape::Mesh { .. } => panic!("the arena has no meshes"),
    };
    // A body steps over what's under a climb, so that doesn't stand in its way.
    let (low, high) = (foot.y + BakeSettings::CHARACTER.climb, foot.y + CAPSULE_HEIGHT);
    let [qx, qy, qz, qw] = collider.rotation;
    if qx != 0.0 || qz != 0.0 {
        return Some(tilted_gap(collider.rotation, at, across, half_height, foot, low, high));
    }
    if at.y + half_height < low || at.y - half_height > high {
        return None;
    }
    let (dx, dz) = (foot.x - at.x, foot.z - at.z);
    // The inverse of a turn about y by this quaternion, on the XZ plane.
    let (c, s) = (qw * qw - qy * qy, 2.0 * qw * qy);
    let (lx, lz) = (c * dx - s * dz, s * dx + c * dz);
    Some(match across {
        Ok([x, z]) => {
            let (ox, oz) = (lx.abs() - x, lz.abs() - z);
            let outside = (ox.max(0.0) * ox.max(0.0) + oz.max(0.0) * oz.max(0.0)).sqrt();
            outside + ox.max(oz).min(0.0)
        }
        Err(radius) => (lx * lx + lz * lz).sqrt() - radius,
    })
}

/// A box tilted off upright, as the proving ground's ramp and steep face are: the least distance from
/// the capsule's standing height, `low` to `high` above the foot, to the box, sampling every 2 cm. That
/// is never more than the gap across, so it holds a path to no less than an upright box's would.
fn tilted_gap(
    rotation: [f32; 4],
    at: Vec3,
    across: Result<[f32; 2], f32>,
    half_height: f32,
    foot: Vec3,
    low: f32,
    high: f32,
) -> f32 {
    let [x, z] = across.expect("only boxes are tilted");
    let half = Vec3::new(x, half_height, z);
    let inverse = glam::Quat::from_array(rotation).inverse();
    let steps = ((high - low) / 0.02).ceil() as u32;
    (0..=steps)
        .map(|step| {
            let point = Vec3::new(foot.x, low + (high - low) * step as f32 / steps as f32, foot.z);
            let local = (inverse * (point - at)).abs() - half;
            local.max(Vec3::ZERO).length() + local.max_element().min(0.0)
        })
        .fold(f32::MAX, f32::min)
}

/// The least gap along a path to anything solid in the arena, and which collider, sampling every 2 cm.
fn clearance(set: &ColliderSet, corners: &[Vec3]) -> (f32, String) {
    let solid = Group::World.mask().union(Group::CharacterBlocker.mask()).bits();
    let mut least = (f32::MAX, String::new());
    for pair in corners.windows(2) {
        let steps = (pair[0].distance(pair[1]) / 0.02).ceil().max(1.0) as u32;
        for step in 0..=steps {
            let foot = pair[0].lerp(pair[1], step as f32 / steps as f32);
            for index in 0..set.colliders.len() {
                if set.colliders[index].memberships & solid == 0 {
                    continue;
                }
                if let Some(gap) = gap_to(set, index, foot)
                    && gap < least.0
                {
                    least = (gap, set.colliders[index].name.clone());
                }
            }
        }
    }
    least
}

fn islands(graph: &NavGraph) -> Vec<usize> {
    let mut seen = vec![false; graph.polygon_count()];
    let mut sizes = Vec::new();
    for start in 0..graph.polygon_count() {
        if seen[start] {
            continue;
        }
        let mut stack = vec![start as u32];
        seen[start] = true;
        let mut size = 0;
        while let Some(polygon) = stack.pop() {
            size += 1;
            for next in graph.neighbours_of(polygon) {
                if !seen[next as usize] {
                    seen[next as usize] = true;
                    stack.push(next);
                }
            }
        }
        sizes.push(size);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes
}

/// Across the arena's square, corner to corner, the long way through its stalls. Every path through
/// the clutter here must keep a capsule's radius from anything in its way, less what cells round off.
#[test]
fn a_path_crosses_the_square_round_its_stalls_with_a_capsules_room() {
    let set = arena_colliders();
    let baked = bake(&arena_triangles(), ARENA, &BakeSettings::CHARACTER).expect("it bakes");
    let graph = graph_of(&baked);

    for (name, from, to) in [
        ("front to back through the market", [-14.0, 0.0, 14.0], [13.5, 0.0, -12.5]),
        ("along the stalls", [15.0, 0.0, -2.0], [8.0, 0.0, -13.5]),
        ("past the pavilion", [-15.0, 0.0, -2.0], [-1.0, 0.0, -12.0]),
        ("into the pavilion's opening", [-3.0, 0.0, -3.0], [-4.5, 0.0, -7.0]),
        ("out of the corner behind the cart", [-14.0, 0.0, -16.0], [14.0, 0.0, 14.0]),
    ] {
        let walked = walk(&graph, from, to);
        assert_eq!(walked.end, RouteEnd::Reached, "{name}");
        let (gap, what) = clearance(&set, &walked.corners);
        let straight = walked.from.position.distance(walked.to.position);
        say(format!(
            "arena, {name}: {} polygons, {} corners, {:.2} m for {:.2} m straight, least gap {gap:.3} m to {what}",
            walked.polygons.len(),
            walked.corners.len(),
            length(&walked.corners),
            straight,
        ));
        // Erosion is 3 cells, 0.375 m, from voxels that round outward. Simplification may then cut a
        // corner by its 1.3 cells' error, 0.16 m, though the least here reads 0.302 m.
        assert!(gap > CAPSULE_RADIUS - 0.09, "{name}: {gap} m to {what}");
    }
    say(format!("arena: islands {:?}, seams {:?}", islands(&graph), baked.seams));
}

/// How high a bake's floor stands over paving at 0: a row's height, which `Reach::CHARACTER` allows for.
const FLOOR: f32 = 0.03125;

/// A route that's pinned, by the bits of its corners. Either end's height only has to be near the ground.
struct Golden {
    name: &'static str,
    from: [f32; 3],
    to: [f32; 3],
    corners: &'static [[f32; 3]],
}

/// Routes over the arena's navmesh, as the simulation holds it.
const ARENA_GOLDEN: &[Golden] = &[
    Golden {
        name: "from the player's spawn to the enemy's",
        from: [3.2, 0.0, 5.0],
        to: [3.2, 0.0, -1.0],
        corners: &[[3.2, FLOOR, 5.0], [3.2, FLOOR, -1.0]],
    },
    Golden {
        name: "front to back through the market",
        from: [-14.0, 0.0, 14.0],
        to: [13.5, 0.0, -12.5],
        corners: &[
            [-14.0, FLOOR, 14.0],
            [-4.875, FLOOR, 3.125],
            [11.375, FLOOR, -9.75],
            [13.5, FLOOR, -12.5],
        ],
    },
    Golden {
        name: "along the stalls",
        from: [15.0, 0.0, -2.0],
        to: [8.0, 0.0, -13.5],
        corners: &[
            [15.0, FLOOR, -2.0],
            [14.875, FLOOR, -6.125],
            [11.75, FLOOR, -11.75],
            [11.5, FLOOR, -12.0],
            [8.0, FLOOR, -13.25],
        ],
    },
    Golden {
        name: "past the pavilion",
        from: [-15.0, 0.0, -2.0],
        to: [-1.0, 0.0, -12.0],
        corners: &[
            [-15.0, FLOOR, -2.0],
            [-14.125, FLOOR, -7.125],
            [-13.625, FLOOR, -7.375],
            [-1.0, FLOOR, -12.0],
        ],
    },
    Golden {
        name: "into the pavilion's opening",
        from: [-3.0, 0.0, -3.0],
        to: [-4.5, 0.0, -7.0],
        corners: &[[-3.0, FLOOR, -3.0], [-4.5, FLOOR, -7.0]],
    },
    Golden {
        name: "out of the corner behind the cart",
        from: [-14.0, 0.0, -16.0],
        to: [14.0, 0.0, 14.0],
        corners: &[
            [-14.0, FLOOR, -16.0],
            [-15.125, FLOOR, -16.0],
            [-14.875, FLOOR, -15.75],
            [-12.875, FLOOR, -12.25],
            [-0.875, FLOOR, 3.375],
            [0.0, FLOOR, 5.75],
            [14.0, FLOOR, 14.0],
        ],
    },
];

/// Routes over the proving ground's, as a cook bakes it.
const PROVING_GROUND_GOLDEN: &[Golden] = &[
    Golden {
        name: "from the player's start to the far gate",
        from: [1.0, 0.0, 1.0],
        to: [22.0, 0.0, -2.0],
        corners: &[[1.0, FLOOR, 1.0], [2.125, FLOOR, 0.0], [22.0, FLOOR, -2.0]],
    },
    Golden {
        name: "round the pillar",
        from: [1.0, 0.0, 1.0],
        to: [3.0, 0.0, 3.0],
        corners: &[
            [1.0, FLOOR, 1.0],
            [2.75, FLOOR, 1.125],
            [3.0, FLOOR, 1.375],
            [3.0, FLOOR, 2.625],
            [3.0, FLOOR, 3.0],
        ],
    },
    Golden {
        name: "across the whole ground",
        from: [-30.0, 0.0, -40.0],
        to: [50.0, 0.0, 40.0],
        // East of the walking course's lane, which stands across the straight line at z -20.
        corners: &[[-30.0, FLOOR, -40.0], [0.0, FLOOR, -22.0], [32.0, FLOOR, 16.0], [50.0, FLOOR, 40.0]],
    },
];

/// Follows a route as a brain would, the whole way, a run's 0.1 m a tick. Returns its corners as they
/// stood after the search that started it, having held the searches to next to none, every step
/// to stay on the mesh, and the walk to end where the route said at about the length it said.
fn follow_the_whole_way(bytes: &[u8], route: &Golden) -> Vec<Vec3> {
    let mut navigation = Navigation::new(NavMesh::read(bytes).expect("it reads"));
    let mut path = Path::new();
    let target = Vec3::from(route.to);
    let mut position = Vec3::from(route.from);
    navigation.begin_tick();
    let first = navigation.follow(&mut path, position, target);
    assert!(matches!(first, Steer::Walk(_)), "{}: {first:?}", route.name);
    assert_eq!(path.route_end(), Some(RouteEnd::Reached), "{}", route.name);
    let start = navigation.graph().locate(position, Reach::CHARACTER).expect("on the mesh").position;
    let mut corners = vec![start];
    corners.extend_from_slice(path.corners());

    let mut walked = 0.0;
    for tick in 0.. {
        assert!(tick < 20_000, "{}: it never arrived, at {position}", route.name);
        navigation.begin_tick();
        match navigation.follow(&mut path, position, target) {
            Steer::Walk(direction) => {
                let to = position + direction * 0.1;
                let on = navigation.graph().locate(to, Reach::CHARACTER);
                position = on.unwrap_or_else(|| panic!("{}: off the mesh at {to}", route.name)).position;
                walked += 0.1;
            }
            Steer::Arrived => break,
            Steer::NoPath => panic!("{}: no path at {position}", route.name),
        }
    }
    let stats = navigation.take_stats();
    // Nothing moved, so only this: rounding a corner can clip a polygon off the corridor, which asks again.
    assert!(stats.searches <= 3, "{}: {stats:?}", route.name);
    let goal = navigation.graph().locate(target, Reach::CHARACTER).expect("on the mesh").position;
    assert!(position.distance(goal) < 0.3, "{}: it stopped at {position}, short of {goal}", route.name);
    let promised = length(&corners);
    assert!(walked <= promised * 1.05 + 0.5, "{}: {walked} m walked for a route of {promised} m", route.name,);
    corners
}

/// Pins each route and walks it. A failure prints each route as it now is, to paste back in, after reading
/// why it moved: a route that changes changes how every character in a zone gets anywhere.
#[test]
fn golden_paths_go_where_they_went() {
    let arena_bytes = sapphire_gameplay::arena::cooked_navmesh();
    let (ground_set, ground_baked, _) = proving_ground();
    let ground_bytes = ground_baked.mesh.to_cooked().expect("it writes");
    let arena_set = arena_colliders();
    let mut moved = Vec::new();
    for (bytes, set, routes) in [
        (&arena_bytes[..], &arena_set, ARENA_GOLDEN),
        (&ground_bytes[..], &ground_set, PROVING_GROUND_GOLDEN),
    ] {
        for route in routes {
            let corners = follow_the_whole_way(bytes, route);
            let (gap, what) = clearance(set, &corners);
            say(format!(
                "golden {}: {} corners, {:.2} m, least gap {gap:.3} m to {what}",
                route.name,
                corners.len(),
                length(&corners)
            ));
            assert!(gap > CAPSULE_RADIUS - 0.09, "{}: {gap} m to {what}", route.name);
            let pinned: Vec<Vec3> = route.corners.iter().copied().map(Vec3::from).collect();
            if corners != pinned {
                moved.push(format!(
                    "{}: {:?}",
                    route.name,
                    corners.iter().map(|c| c.to_array()).collect::<Vec<_>>()
                ));
            }
        }
    }
    assert!(moved.is_empty(), "routes that moved:\n{}", moved.join("\n"));
}

/// The town as drawn has no blockers, so its moon gates are open. Through one, from the square to
/// the street, where the arena's colliders say nothing.
#[test]
fn a_path_goes_through_a_moon_gate_in_the_town_as_drawn() {
    let baked = bake(&town_triangles(), TOWN, &BakeSettings::CHARACTER).expect("it bakes");
    let graph = graph_of(&baked);
    let sizes = islands(&graph);
    say(format!(
        "town: {} polygons, {} islands, largest {:?}, seams {:?}",
        graph.polygon_count(),
        sizes.len(),
        &sizes[..sizes.len().min(8)],
        baked.seams,
    ));

    // The street's moon gate is 3.4 m of wall about x = 3.2, at z = -18.
    let walked = walk(&graph, [3.2, 0.0, -12.0], [3.2, 0.0, -30.0]);
    say(format!(
        "town, through the street's moon gate: {:?}, {} polygons, {:.2} m, corners {:?}",
        walked.end,
        walked.polygons.len(),
        length(&walked.corners),
        walked.corners,
    ));
    assert_eq!(walked.end, RouteEnd::Reached);
    let crossing = walked
        .corners
        .windows(2)
        .find(|pair| pair[0].z >= -18.0 && pair[1].z < -18.0)
        .expect("it crosses the wall's line");
    let t = (-18.0 - crossing[0].z) / (crossing[1].z - crossing[0].z);
    let x = crossing[0].x + (crossing[1].x - crossing[0].x) * t;
    assert!((1.5..4.9).contains(&x), "it crosses at x = {x}, in the wall");

    let square = walk(&graph, [-14.0, 0.0, 14.0], [13.5, 0.0, -12.5]);
    say(format!(
        "town, across the square: {:?}, {} polygons, {} corners, {:.2} m",
        square.end,
        square.polygons.len(),
        square.corners.len(),
        length(&square.corners),
    ));
    assert_eq!(square.end, RouteEnd::Reached);
}

fn solid(name: &str, half_extents: [f32; 3], position: [f32; 3]) -> ColliderEntry {
    ColliderEntry {
        name: name.to_owned(),
        shape: ColliderShape::Box { half_extents },
        position,
        rotation: Rotation::None,
        groups: vec![Group::World],
    }
}

/// `steps` boxes rising `rise` each and running 0.3 m, from a floor at y 0 to a landing.
fn stair(rise: f32, steps: u32) -> Vec<Triangle> {
    let mut colliders = vec![solid("floor", [16.0, 0.5, 16.0], [0.0, -0.5, 0.0])];
    let top = rise * steps as f32;
    for step in 0..steps {
        let height = rise * (step + 1) as f32;
        colliders.push(solid(
            &format!("step{step:02}"),
            [1.5, height * 0.5, 0.15],
            [0.0, height * 0.5, 0.15 + 0.3 * step as f32],
        ));
    }
    let run = 0.3 * steps as f32;
    colliders.push(solid("landing", [1.5, top * 0.5, 1.5], [0.0, top * 0.5, run + 1.5]));
    let set = ColliderDocument { colliders }.cook().expect("it cooks");
    colliders_to_triangles(&set, Group::World.mask())
}

/// The town has no stair, so one is built: a typical flight, 18 cm by 30 cm, over 1.8 m.
#[test]
fn a_stair_is_a_ramp_to_polygons_and_its_error_is_within_a_step() {
    let baked = bake(&stair(0.18, 10), ([-1, -1], [0, 0]), &BakeSettings::CHARACTER).expect("it bakes");
    let graph = graph_of(&baked);
    let top = 1.8;
    let walked = walk(&graph, [0.0, 0.0, -4.0], [0.0, top, 4.5]);
    assert_eq!(walked.end, RouteEnd::Reached, "up a stair with no links and no detail");
    assert!(walked.corners.len() <= 4, "straight up it: {:?}", walked.corners);

    // Every step's tread: what the graph says its height is, against where a foot really is.
    let mut worst: f32 = 0.0;
    let mut out_of_reach = 0;
    for step in 0..10 {
        for across in [-0.5, 0.0, 0.5] {
            let tread = 0.18 * (step + 1) as f32;
            let foot = Vec3::new(across, tread, 0.15 + 0.3 * step as f32);
            let far = Reach { across: 0.75, vertical: 8.0 };
            let found = graph.locate(foot, far).expect("every tread is on the mesh");
            worst = worst.max((found.position.y - tread).abs());
            out_of_reach += u32::from(graph.locate(foot, Reach::CHARACTER).is_none());
        }
    }
    say(format!(
        "stair 18 by 30 cm: {} polygons in all, path {} polygons, {} corners, worst height error over treads {worst:.3} m, {out_of_reach} of 30 feet out of a character's reach",
        graph.polygon_count(),
        walked.polygons.len(),
        walked.corners.len(),
    ));
    assert_eq!(out_of_reach, 0, "a character's reach covers what a polygon's height can be off by");
    assert!(worst < Reach::CHARACTER.vertical, "off by {worst} m");
}

/// Past a climb, a step is a wall, and with no links there's no way up it, nor down.
#[test]
fn a_step_past_the_climb_has_no_way_up() {
    let baked = bake(&stair(0.4, 3), ([-1, -1], [0, 0]), &BakeSettings::CHARACTER).expect("it bakes");
    let graph = graph_of(&baked);
    let walked = walk(&graph, [0.0, 0.0, -4.0], [0.0, 1.2, 3.0]);
    assert_eq!(walked.end, RouteEnd::Unreachable);
    let last = *walked.corners.last().expect("it goes somewhere");
    say(format!("a 40 cm stair: unreachable, walks to {last:?}, for lack of a link"));
}

/// A walkway over a street: a point locates to the floor at its own height, and a path under it
/// stays under it.
#[test]
fn two_floors_are_told_apart_by_height() {
    let colliders = vec![
        solid("street", [16.0, 0.5, 16.0], [0.0, -0.5, 0.0]),
        solid("walkway", [1.5, 0.1, 8.0], [0.0, 3.0, 0.0]),
    ];
    let set = ColliderDocument { colliders }.cook().expect("it cooks");
    let triangles = colliders_to_triangles(&set, Group::World.mask());
    let baked = bake(&triangles, ([-1, -1], [0, 0]), &BakeSettings::CHARACTER).expect("it bakes");
    let graph = graph_of(&baked);

    let below = graph.locate(Vec3::new(0.0, 0.0, 0.0), Reach::CHARACTER).expect("the street");
    let above = graph.locate(Vec3::new(0.0, 3.1, 0.0), Reach::CHARACTER).expect("the walkway");
    assert_ne!(below.polygon, above.polygon);
    assert!(below.position.y.abs() < 0.1, "{below:?}");
    assert!((above.position.y - 3.1).abs() < 0.1, "{above:?}");

    let walked = walk(&graph, [0.0, 0.0, -10.0], [0.0, 0.0, 10.0]);
    assert_eq!(walked.end, RouteEnd::Reached);
    assert!(walked.corners.iter().all(|corner| corner.y.abs() < 0.1), "{:?}", walked.corners);
    assert_eq!(islands(&graph).len(), 2, "a street and a walkway no stair joins");
}

/// Two tiles alone, side by side, with their seam the only way across: the arena's own, where it
/// runs through open paving and past a stall.
#[test]
fn two_stitched_tiles_carry_a_path_across_their_seam() {
    let settings = BakeSettings::CHARACTER;
    let triangles = arena_triangles();
    let left = bake_tile(&triangles, [-1, -1], &settings).expect("bakes").expect("has ground");
    let right = bake_tile(&triangles, [0, -1], &settings).expect("bakes").expect("has ground");
    let polygons_left = left.polygons.len() as u32;
    let (mesh, seams) = stitch(&[left, right], &settings).expect("it stitches");
    let bytes = mesh.to_cooked().expect("it writes");
    let graph = NavGraph::new(NavMesh::read(&bytes).expect("every edge is shared from both sides"));

    let walked = walk(&graph, [-14.0, 0.0, -2.0], [4.0, 0.0, -14.0]);
    assert_eq!(walked.end, RouteEnd::Reached);
    let on_left = walked.polygons.iter().filter(|&&p| p < polygons_left).count();
    assert!(on_left > 0 && on_left < walked.polygons.len(), "both tiles: {:?}", walked.polygons);
    say(format!(
        "two tiles: {} polygons, seam {seams:?}; path {} polygons, {} corners, {:.2} m",
        graph.polygon_count(),
        walked.polygons.len(),
        walked.corners.len(),
        length(&walked.corners),
    ));
    // Nothing but the outer sides of two tiles is left over.
    assert!(seams.exact + seams.same_ends_other_height + seams.partial > 0);
    let outer = seams.seam_cells - seams.joined_cells;
    let seam_length = 2 * settings.tile_cells as u64;
    // A bench and a lantern stand on this seam, so a little of it has nothing walkable on either side.
    assert!(seams.joined_cells >= seam_length * 3 / 4, "most of the seam between them joined: {seams:?}");
    assert!(outer > 0, "and the tiles' outer sides have nothing to join");
}
