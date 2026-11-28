use std::cell::Cell;

use sapphire_serialize::cooked::AlignedVec;

use super::*;
use crate::{Zone, ZoneDocument, ZoneGeometry};

/// A pond, a river that falls along two stretches, a frozen rink, and the sea under them all.
const WATER: &str = r#"
    water: [
        (name: "pond", shape: Lake(outline: [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], height: 1.0)),
        (name: "race", shape: River(points: [
            (at: (20.0, 3.0, 5.0), width: 4.0, speed: 2.0),
            (at: (40.0, 1.0, 5.0), width: 6.0, speed: 2.0),
            (at: (40.0, 1.0, 25.0), width: 6.0, speed: 1.0),
        ]), swim: false),
        (name: "rink", shape: Lake(outline: [(-20.0, -20.0), (-10.0, -20.0), (-10.0, -10.0), (-20.0, -10.0)], height: 0.5), surface: Frozen),
        (name: "sea", shape: Ocean(height: -2.0)),
    ],
"#;

fn document(data: &str) -> ZoneDocument {
    ZoneDocument::from_ron(&format!(
        r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), {data}))"#
    ))
    .expect("a good document")
}

/// The reflection probes a cook would bake, by some ID: the cook places one for each lake and river that names
/// none.
const REFLECTIONS: AssetId = AssetId::from_bytes([0x77; 16]);

fn cooked(data: &str) -> AlignedVec {
    let document = document(data);
    let reflections = (!document.reflection_probes().is_empty()).then_some(REFLECTIONS);
    let geometry = ZoneGeometry { reflections, ..ZoneGeometry::default() };
    document.cook(geometry).expect("cooks").to_cooked().expect("writes")
}

/// The ground under it all: the pond's bed a metre down, a hill west of x = −30 standing over the sea, nothing
/// at all past x = 100, and elsewhere the seabed at −5. A ray that starts inside the hill meets it at once.
fn floor(x: f32, from_y: f32, _z: f32, reach: f32) -> Option<f32> {
    let ground = if x > 100.0 {
        return None;
    } else if x < -30.0 {
        3.0
    } else if (0.0..=10.0).contains(&x) {
        -1.0
    } else {
        -5.0
    };
    let hit = if ground > from_y { from_y } else { ground };
    (from_y - hit <= reach).then_some(hit)
}

fn body_named(volumes: &WaterVolumes<'_, impl Floor>, sample: &WaterSample) -> String {
    volumes.body(sample.body).expect("a body").name.as_str().to_owned()
}

/// The query's table: inside and outside each kind, at the shore, along a river's flow, frozen, under a hill,
/// and the floor's depth, with the floor's ray as `floor` above casts it.
#[test]
fn the_query_answers_where_the_water_is() {
    let bytes = cooked(WATER);
    let zone = Zone::read(&bytes).expect("reads");
    let volumes = WaterVolumes::new(&zone.simulation, floor);
    assert!(!volumes.is_empty());

    /// The body's name, its kind, the surface's height, the flow and the depth.
    type Expected = (&'static str, WaterKind, f32, [f32; 2], Option<f32>);
    struct Row {
        what: &'static str,
        at: [f32; 3],
        expect: Option<Expected>,
    }
    let rows = [
        Row {
            what: "the pond's middle",
            at: [5.0, 0.5, 5.0],
            expect: Some(("pond", WaterKind::Lake, 1.0, [0.0; 2], Some(2.0))),
        },
        Row {
            what: "on the pond's surface",
            at: [5.0, 1.0, 5.0],
            expect: Some(("pond", WaterKind::Lake, 1.0, [0.0; 2], Some(2.0))),
        },
        Row { what: "over the pond", at: [5.0, 1.01, 5.0], expect: None },
        Row { what: "under the pond's bed", at: [5.0, -1.5, 5.0], expect: None },
        Row {
            what: "just inside the pond's shore",
            at: [9.99, 0.5, 5.0],
            expect: Some(("pond", WaterKind::Lake, 1.0, [0.0; 2], Some(2.0))),
        },
        Row { what: "just outside it, over the sea", at: [10.01, 0.5, 5.0], expect: None },
        Row {
            what: "the sea",
            at: [15.0, -3.0, 15.0],
            expect: Some(("sea", WaterKind::Ocean, -2.0, [0.0; 2], Some(3.0))),
        },
        Row {
            what: "the sea with no floor in reach",
            at: [200.0, -3.0, 0.0],
            expect: Some(("sea", WaterKind::Ocean, -2.0, [0.0; 2], None)),
        },
        Row { what: "under the hill", at: [-40.0, -3.0, 0.0], expect: None },
        Row {
            what: "halfway down the race's fall",
            at: [30.0, 1.5, 5.0],
            expect: Some(("race", WaterKind::River, 2.0, [2.0, 0.0], Some(7.0))),
        },
        Row { what: "over the race there", at: [30.0, 2.5, 5.0], expect: None },
        Row {
            what: "by the race's bank",
            at: [30.0, 1.5, 7.4],
            expect: Some(("race", WaterKind::River, 2.0, [2.0, 0.0], Some(7.0))),
        },
        Row { what: "past its bank", at: [30.0, 1.5, 7.6], expect: None },
        Row {
            what: "the race's second stretch",
            at: [40.0, 0.5, 15.0],
            expect: Some(("race", WaterKind::River, 1.0, [0.0, 1.5], Some(6.0))),
        },
        Row {
            what: "round the race's bend",
            at: [42.5, 0.5, 4.0],
            expect: Some(("race", WaterKind::River, 1.0, [2.0, 0.0], Some(6.0))),
        },
        Row { what: "past the race's mouth", at: [40.0, 0.5, 28.5], expect: None },
        Row {
            what: "the rink",
            at: [-15.0, 0.0, -15.0],
            expect: Some(("rink", WaterKind::Lake, 0.5, [0.0; 2], Some(5.5))),
        },
    ];
    for row in rows {
        let [x, y, z] = row.at;
        let found = volumes.at(x, y, z);
        match (found, row.expect) {
            (None, None) => {}
            (Some(sample), Some((name, kind, surface_y, flow, depth))) => {
                assert_eq!(body_named(&volumes, &sample), name, "{}", row.what);
                assert_eq!(sample.kind, kind, "{}", row.what);
                assert_eq!(sample.surface_y, surface_y, "{}", row.what);
                let off = (0..2).map(|axis| (sample.flow[axis] - flow[axis]).abs()).fold(0.0, f32::max);
                assert!(off <= 1e-6, "{}: flow {:?}, expected {flow:?}", row.what, sample.flow);
                assert_eq!(sample.depth, depth, "{}", row.what);
            }
            (found, expect) => panic!("{}: found {found:?}, expected {expect:?}", row.what),
        }
    }

    let rink = volumes.at(-15.0, 0.0, -15.0).expect("the rink");
    assert_eq!((rink.surface, rink.swim), (WaterSurface::Frozen, true), "ice reports the water under it");
    let race = volumes.at(30.0, 1.5, 5.0).expect("the race");
    assert_eq!((race.surface, race.swim), (WaterSurface::Liquid, false));
    let pond = volumes.at(5.0, 0.5, 5.0).expect("the pond");
    assert!(pond.swim);
}

/// With no floor at all, every column is as deep as its water goes, and the sea's covers the zone.
#[test]
fn with_no_floor_the_water_is_bottomless() {
    let bytes = cooked(WATER);
    let zone = Zone::read(&bytes).expect("reads");
    let volumes = WaterVolumes::new(&zone.simulation, |_, _, _, _| None);
    let deep = volumes.at(-40.0, -100.0, 3000.0).expect("the sea");
    assert_eq!((deep.kind, deep.depth), (WaterKind::Ocean, None));
    assert_eq!(volumes.at(5.0, -50.0, 5.0).expect("the pond").kind, WaterKind::Lake);
}

/// Where two bodies hold a point, the higher surface answers, the first in the zone's order on a tie; and the
/// floor is asked from that surface, once, where its floor holds the point.
#[test]
fn where_bodies_overlap_the_higher_surface_answers() {
    let square = |x: f32| format!("[({x}, 0.0), ({}, 0.0), ({}, 10.0), ({x}, 10.0)]", x + 10.0, x + 10.0);
    let bytes = cooked(&format!(
        r#"water: [
            (name: "low", shape: Lake(outline: {}, height: 1.0)),
            (name: "high", shape: Lake(outline: {}, height: 2.0)),
            (name: "level", shape: Lake(outline: {}, height: 2.0)),
        ],"#,
        square(0.0),
        square(5.0),
        square(8.0),
    ));
    let zone = Zone::read(&bytes).expect("reads");
    let asked = Cell::new(0);
    let floor = |_: f32, from_y: f32, _: f32, _: f32| {
        asked.set(asked.get() + 1);
        Some(from_y - 3.0)
    };
    let volumes = WaterVolumes::new(&zone.simulation, floor);
    let name = |x: f32, y: f32| volumes.at(x, y, 5.0).map(|sample| body_named(&volumes, &sample));
    assert_eq!(name(6.0, 0.5).as_deref(), Some("high"));
    assert_eq!(name(6.0, 1.5).as_deref(), Some("high"));
    assert_eq!(name(2.0, 0.5).as_deref(), Some("low"));
    // "high" and "level" are one height over x = 9: the first by ID answers, whichever that is.
    let first = zone.simulation.water.iter().find(|body| body.name != "low").expect("there").name.as_str();
    assert_eq!(name(9.0, 0.5).as_deref(), Some(first));
    assert_eq!(volumes.at(6.0, 0.5, 5.0).expect("high").depth, Some(3.0), "from the higher surface");
    asked.set(0);
    let _ = volumes.at(9.0, 0.5, 5.0);
    assert_eq!(asked.get(), 1, "one ray where the highest's floor holds the point, however many bodies do");
    let _ = volumes.at(30.0, 0.5, 5.0);
    assert_eq!(asked.get(), 1, "none where no body does");
    // Under the floor "high" and "level" share at 2.0, but over the floor of "low" at 1.0: one ray for the
    // two at one height, cached, and one for the lower, whose water answers.
    let low = |depth: f32| Some(("low".to_owned(), 1.0, Some(depth)));
    assert_eq!(answers(&volumes, &asked, [9.0, -1.5, 5.0]), (low(3.0), 2));
    assert_eq!(answers(&volumes, &asked, [9.0, -2.5, 5.0]), (None, 2), "under every floor: no water");
    assert_eq!(answers(&volumes, &asked, [6.0, -1.5, 5.0]), (low(3.0), 2), "two heights, two rays");
}

/// The floor is asked once for each distinct surface height over a point, however many bodies stand there:
/// at most [`MAX_BODIES`] rays where every body has its own height and every floor refuses the point, and one
/// where they all share a height.
#[test]
fn the_floor_is_asked_once_for_each_height_over_a_point() {
    let outline = "[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]";
    let lakes = |height: &dyn Fn(usize) -> f32| {
        let lakes: String = (0..MAX_BODIES)
            .map(|index| {
                let height = height(index);
                format!(r#"(name: "lake_{index}", shape: Lake(outline: {outline}, height: {height:.1})),"#)
            })
            .collect();
        cooked(&format!("water: [{lakes}],"))
    };
    let all = MAX_BODIES as u32;
    let asked = Cell::new(0);
    let floor = |_: f32, from_y: f32, _: f32, _: f32| {
        asked.set(asked.get() + 1);
        Some(from_y - 1.0)
    };
    let apart = lakes(&|index| index as f32);
    let zone = Zone::read(&apart).expect("reads");
    let volumes = WaterVolumes::new(&zone.simulation, &floor);
    assert_eq!(answers(&volumes, &asked, [5.0, -10.0, 5.0]), (None, all), "one ray a height");
    let lowest = Some(("lake_0".to_owned(), 0.0, Some(1.0)));
    assert_eq!(answers(&volumes, &asked, [5.0, -0.5, 5.0]), (lowest, all), "the lowest answers last");
    let level = lakes(&|_| 2.0);
    let zone = Zone::read(&level).expect("reads");
    let volumes = WaterVolumes::new(&zone.simulation, &floor);
    assert_eq!(answers(&volumes, &asked, [5.0, -10.0, 5.0]), (None, 1), "one ray for one height");
    assert_eq!(answers(&volumes, &asked, [5.0, 1.5, 5.0]).1, 1);
}

/// A point that isn't a number is in no water, and asks no floor, wherever the bodies are.
#[test]
fn a_point_that_isnt_a_number_is_in_no_water() {
    let bytes = cooked(WATER);
    let zone = Zone::read(&bytes).expect("reads");
    let asked = Cell::new(0);
    let counted = |x: f32, from_y: f32, z: f32, reach: f32| {
        asked.set(asked.get() + 1);
        floor(x, from_y, z, reach)
    };
    let volumes = WaterVolumes::new(&zone.simulation, &counted);
    assert!(volumes.at(5.0, 0.5, 5.0).is_some(), "the pond holds a number");
    for at in [[5.0, f32::NAN, 5.0], [-25.0, f32::NAN, 0.0], [60.0, f32::NAN, 60.0]] {
        assert_eq!(answers(&volumes, &asked, at), (None, 0), "{at:?}");
    }
}

/// The body that answers at a point, its surface and its depth, if any does, and how many floor rays it cast.
type Answer = (Option<(String, f32, Option<f32>)>, u32);

/// Asks `volumes` at a point, counting the rays its floor cast in `asked`.
fn answers(volumes: &WaterVolumes<'_, impl Floor>, asked: &Cell<u32>, at: [f32; 3]) -> Answer {
    asked.set(0);
    let [x, y, z] = at;
    let found =
        volumes.at(x, y, z).map(|sample| (body_named(volumes, &sample), sample.surface_y, sample.depth));
    (found, asked.get())
}

/// In the body named, under its surface at `surface_y`, `depth` over its floor, by `rays` floor rays.
fn in_body(name: &str, surface_y: f32, depth: f32, rays: u32) -> Answer {
    (Some((name.to_owned(), surface_y, Some(depth))), rays)
}

/// A solid block over the ground plane, from x0 to x1 and z0 to z1, standing from under everything to `top`.
type Solid = ([f32; 2], [f32; 2], f32);

/// What a ray from `from_y` down meets first: the top of a solid it starts over, its start if it starts inside
/// one, or else the bed at `bed`.
fn ground(solids: &[Solid], bed: f32, x: f32, from_y: f32, z: f32, reach: f32) -> Option<f32> {
    let mut hit = bed;
    for &([x0, x1], [z0, z1], top) in solids {
        if (x0..=x1).contains(&x) && (z0..=z1).contains(&z) && hit < top {
            hit = top.min(from_y);
        }
    }
    (from_y - hit <= reach).then_some(hit)
}

/// A river on an aqueduct over a lake: a swimmer in the lake under the aqueduct is in the lake, though the
/// aqueduct's channel is the higher surface over them, since the channel's ray stops at its deck. Between the
/// deck and the lake they're in no water, and in the channel the channel answers by one ray.
#[test]
fn a_lake_under_an_aqueduct_holds_a_swimmer_under_it() {
    let bytes = cooked(
        r#"water: [
            (name: "lake", shape: Lake(outline: [(-30.0, -30.0), (30.0, -30.0), (30.0, 30.0), (-30.0, 30.0)], height: 0.0)),
            (name: "aqueduct", shape: River(points: [
                (at: (-40.0, 10.0, 0.0), width: 4.0, speed: 1.0),
                (at: (40.0, 10.0, 0.0), width: 4.0, speed: 1.0),
            ]), swim: false),
        ],"#,
    );
    let zone = Zone::read(&bytes).expect("reads");
    let asked = Cell::new(0);
    // The channel's bed is a deck 6 m across with its top at 8 m, open underneath: a ray from under it misses
    // it. The lake's bed is at −5 m.
    let deck = [([-40.0, 40.0], [-3.0, 3.0], 8.0)];
    let floor = |x: f32, from_y: f32, z: f32, reach: f32| {
        asked.set(asked.get() + 1);
        let solids: &[Solid] = if from_y >= 8.0 { &deck } else { &[] };
        ground(solids, -5.0, x, from_y, z, reach)
    };
    let volumes = WaterVolumes::new(&zone.simulation, floor);
    let at = |point| answers(&volumes, &asked, point);
    assert_eq!(at([0.0, -1.0, 0.0]), in_body("lake", 0.0, 5.0, 2), "in the lake under the aqueduct");
    assert_eq!(at([0.0, -1.0, 10.0]), in_body("lake", 0.0, 5.0, 1), "in the lake beside it");
    assert_eq!(at([0.0, 9.0, 0.0]), in_body("aqueduct", 10.0, 2.0, 1), "in the aqueduct's channel");
    assert_eq!(at([0.0, 4.0, 0.0]), (None, 1), "in the air under the deck");
    assert_eq!(at([35.0, 9.0, 0.0]), in_body("aqueduct", 10.0, 2.0, 1), "in the channel past the lake");
}

/// A river running off a ledge that overhangs its pool, then falling into it: a swimmer in the pool under the
/// ledge is in the pool, though the river's surface is the higher over them. Under the fall itself, where
/// nothing stands between its surface and the pool's bed, the fall's surface is the higher and answers.
#[test]
fn a_pool_under_a_fall_holds_a_swimmer_under_its_ledge() {
    let bytes = cooked(
        r#"water: [
            (name: "pool", shape: Lake(outline: [(-4.0, -6.0), (12.0, -6.0), (12.0, 6.0), (-4.0, 6.0)], height: 0.0)),
            (name: "falls", shape: River(points: [
                (at: (-20.0, 5.0, 0.0), width: 4.0, speed: 1.0),
                (at: (0.0, 5.0, 0.0), width: 4.0, speed: 2.0),
                (at: (1.0, 0.0, 0.0), width: 4.0, speed: 4.0),
            ])),
        ],"#,
    );
    let zone = Zone::read(&bytes).expect("reads");
    let asked = Cell::new(0);
    // The cliff is solid to 4.5 m west of x = −4, and its ledge reaches over the pool to the lip at x = 0.2,
    // open underneath. The pool's bed is at −3 m.
    let (cliff, ledge) = (([-100.0, -4.0], [-100.0, 100.0], 4.5), ([-4.0, 0.2], [-100.0, 100.0], 4.5));
    let (over, under) = ([cliff, ledge], [cliff]);
    let floor = |x: f32, from_y: f32, z: f32, reach: f32| {
        asked.set(asked.get() + 1);
        let solids: &[Solid] = if from_y >= 4.5 { &over } else { &under };
        ground(solids, -3.0, x, from_y, z, reach)
    };
    let volumes = WaterVolumes::new(&zone.simulation, floor);
    let at = |point| answers(&volumes, &asked, point);
    assert_eq!(at([-2.0, -0.5, 0.0]), in_body("pool", 0.0, 3.0, 2), "in the pool under the ledge");
    assert_eq!(at([-2.0, 4.75, 0.0]), in_body("falls", 5.0, 0.5, 1), "in the river on the ledge");
    assert_eq!(at([-2.0, 2.0, 0.0]), (None, 1), "in the air under the ledge, over the pool");
    assert_eq!(at([6.0, -0.5, 0.0]), in_body("pool", 0.0, 3.0, 1), "in the pool past the fall");
    let (foot, rays) = at([0.9, -0.5, 0.0]);
    assert_eq!(foot.map(|(name, ..)| name), Some("falls".to_owned()), "under the fall's foot");
    assert_eq!(rays, 1);
}

#[test]
fn a_zone_with_no_water_has_none_anywhere() {
    let bytes = cooked("");
    let zone = Zone::read(&bytes).expect("reads");
    let volumes = WaterVolumes::new(&zone.simulation, floor);
    assert!(volumes.is_empty());
    assert_eq!(volumes.at(0.0, -100.0, 0.0), None);
}

/// A lake's outline may run either way round, and its shape is the same.
#[test]
fn an_outline_reads_the_same_either_way_round() {
    let forward = cooked(
        r#"water: [(name: "a", shape: Lake(outline: [(0.0, 0.0), (8.0, 0.0), (4.0, 6.0)], height: 0.0))],"#,
    );
    let backward = cooked(
        r#"water: [(name: "a", shape: Lake(outline: [(4.0, 6.0), (8.0, 0.0), (0.0, 0.0)], height: 0.0))],"#,
    );
    let (forward, backward) = (Zone::read(&forward).expect("reads"), Zone::read(&backward).expect("reads"));
    let (forward, backward) =
        (WaterVolumes::new(&forward.simulation, floor), WaterVolumes::new(&backward.simulation, floor));
    for (x, z) in [(4.0, 2.0), (1.0, 1.0), (7.5, 0.5), (4.0, 5.9), (0.5, 3.0), (7.5, 3.0), (4.0, -0.1)] {
        assert_eq!(forward.at(x, -0.5, z).is_some(), backward.at(x, -0.5, z).is_some(), "({x}, {z})");
    }
    assert!(forward.at(4.0, -0.5, 2.0).is_some());
    assert!(forward.at(0.5, -0.5, 3.0).is_none());
}

/// Each thing a document can get wrong about its water names its field.
#[test]
fn every_water_check_names_the_field() {
    type Tamper = fn(&mut ZoneDocument);
    let cases: [(&str, Tamper); 16] = [
        ("water[0].name", |d| d.water[0].name.clear()),
        ("water[1].name", |d| d.water[1].name = "pond".to_owned()),
        ("water[0].shape.outline", |d| {
            d.water[0].shape = WaterShape::Lake { outline: vec![[0.0; 2], [1.0, 0.0]], height: 0.0 }
        }),
        ("water[0].shape.outline", |d| {
            let outline = vec![[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
            d.water[0].shape = WaterShape::Lake { outline, height: 0.0 };
        }),
        ("water[0].shape.outline", |d| {
            let outline = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]];
            d.water[0].shape = WaterShape::Lake { outline, height: 0.0 };
        }),
        ("water[0].shape.outline", |d| {
            let outline = vec![[0.0, 0.0], [0.0, 0.0], [2.0, 0.0], [0.0, 2.0]];
            d.water[0].shape = WaterShape::Lake { outline, height: 0.0 };
        }),
        ("water[0].shape.outline", |d| {
            let outline = vec![[0.0, 0.0], [f32::NAN, 0.0], [0.0, 2.0]];
            d.water[0].shape = WaterShape::Lake { outline, height: 0.0 };
        }),
        ("water[0].shape.height", |d| {
            let outline = vec![[0.0, 0.0], [2.0, 0.0], [0.0, 2.0]];
            d.water[0].shape = WaterShape::Lake { outline, height: f32::INFINITY };
        }),
        ("water[1].shape.points", |d| {
            let WaterShape::River { points } = &mut d.water[1].shape else { unreachable!() };
            points.truncate(1);
        }),
        ("water[1].shape.points[1].width", |d| {
            let WaterShape::River { points } = &mut d.water[1].shape else { unreachable!() };
            points[1].width = 0.0;
        }),
        ("water[1].shape.points[0].speed", |d| {
            let WaterShape::River { points } = &mut d.water[1].shape else { unreachable!() };
            points[0].speed = -1.0;
        }),
        ("water[1].shape.points[2].at", |d| {
            let WaterShape::River { points } = &mut d.water[1].shape else { unreachable!() };
            points[2].at = [40.0, -5.0, 5.0];
        }),
        ("water[3].shape", |d| d.water[0].shape = WaterShape::Ocean { height: 0.0 }),
        ("water[2].shore_margin", |d| d.water[2].shore_margin = -1.0),
        ("water[2].probe", |d| d.water[2].probe = Some("nowhere".to_owned())),
        ("water", |d| {
            let mut extra = d.water[0].clone();
            for index in 0..MAX_BODIES {
                extra.name = format!("pool_{index}");
                d.water.push(extra.clone());
            }
        }),
    ];
    for (field, tamper) in cases {
        let mut tampered = document(WATER);
        tamper(&mut tampered);
        let message = tampered.check().expect_err(field).to_string();
        assert!(message.contains(&format!("`{field}`")), "`{message}` must name `{field}`");
    }
}

/// The module's example reads, and a document's water round-trips through its text, the defaults left out.
#[test]
fn water_round_trips_through_its_text() {
    let reflections = r#"reflections: [(name: "harbour", position: (0.0, 1.6, 0.0), parallax: (min: (-8.0, 0.0, -8.0), max: (8.0, 8.0, 8.0)))],"#;
    let with_probe = WATER.replace(
        r#"shape: Ocean(height: -2.0))"#,
        r#"shape: Ocean(height: -2.0), probe: Some("harbour"), shore_margin: 4.0)"#,
    );
    let document = document(&format!("{reflections} {with_probe}"));
    assert_eq!(document.water[0].shore_margin, DEFAULT_SHORE_MARGIN);
    assert!(document.water[0].swim && !document.water[1].swim);
    let text = sapphire_serialize::to_ron(&document).expect("write");
    assert!(!text.contains("Liquid") && !text.contains("swim: true"), "defaults are left out: {text}");
    assert_eq!(ZoneDocument::from_ron(&text).expect("read"), document);

    let zone = document.cook(ZoneGeometry {
        reflections: Some(AssetId::from_text("0b3f6c1d-7a52-4e98-b4d1-93c2e8f05a67")),
        ..ZoneGeometry::default()
    });
    let zone = zone.expect("cooks");
    let sea = zone.simulation.water.iter().position(|body| body.name == "sea").expect("there");
    let surface = &zone.presentation.water[sea];
    assert_eq!((surface.probe, surface.shore_margin), (Some(0), 4.0));
    assert_eq!(surface.id, zone.simulation.water[sea].id);
    assert!(zone.simulation.water.is_sorted_by_key(|body| body.id));
}

/// The probe a cook places for each lake and river that names none (M6 W3a): named the body's name, over the
/// middle of a lake's outline or halfway along a river, its box past the body's surface; never the sea's. The
/// zone's probes are its document's then these, in its water's order, and a cooked surface reflects its own.
#[test]
fn each_lake_and_river_without_a_probe_has_one_placed() {
    let document = document(WATER);
    let pond = document.water[0].placed_probe().expect("the pond names none");
    assert_eq!(pond.name, "pond");
    assert_eq!(pond.position, [5.0, 1.0 + PROBE_LIFT, 5.0], "over the outline's middle");
    let reach = DEFAULT_SHORE_MARGIN + PROBE_REACH;
    assert_eq!(pond.parallax.min, [-reach, 1.0 - PROBE_DEPTH, -reach]);
    assert_eq!(pond.parallax.max, [10.0 + reach, 1.0 + PROBE_HEIGHT, 10.0 + reach]);
    let race = document.water[1].placed_probe().expect("the race names none");
    assert_eq!(race.position, [40.0, 1.0 + PROBE_LIFT, 5.0], "halfway along its 40 m");
    assert_eq!((race.parallax.min[1], race.parallax.max[1]), (1.0 - PROBE_DEPTH, 3.0 + PROBE_HEIGHT));
    assert_eq!(document.water[3].placed_probe(), None, "the sea reflects the sky");
    // An L's bounds' middle, (5, 5), is land: its probe stands over the middle of the water across that line.
    let ell = WaterBody {
        shape: WaterShape::Lake {
            outline: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 4.0], [4.0, 4.0], [4.0, 10.0], [0.0, 10.0]],
            height: 0.0,
        },
        ..document.water[0].clone()
    };
    assert_eq!(ell.placed_probe().expect("placed").position, [2.0, PROBE_LIFT, 5.0]);
    let names: Vec<String> = document.reflection_probes().into_iter().map(|probe| probe.name).collect();
    assert_eq!(names, ["pond", "race", "rink"]);

    let zone = Zone::read(&cooked(WATER)).map(|zone| {
        let by_name = |name: &str| {
            let index = zone.simulation.water.iter().position(|body| body.name == name).expect("there");
            zone.presentation.water[index].probe.as_ref().map(|probe| probe.to_native())
        };
        [by_name("pond"), by_name("race"), by_name("rink"), by_name("sea")]
    });
    assert_eq!(zone.expect("reads"), [Some(0), Some(1), Some(2), None]);

    // A document's own probe named for a body that names none is refused: the placed one would take its name.
    let reflections = r#"reflections: [(name: "rink", position: (0.0, 1.6, 0.0), parallax: (min: (-8.0, 0.0, -8.0), max: (8.0, 8.0, 8.0)))],"#;
    let text = format!(
        r#"(type: "sapphire::zone::ZoneDocument", version: 1, data: (grid: (cell_size: 32.0), {reflections} {WATER}))"#
    );
    let message = ZoneDocument::from_ron(&text).expect_err("refused").to_string();
    assert!(message.contains("water[2].probe") && message.contains("rink"), "{message}");
}
