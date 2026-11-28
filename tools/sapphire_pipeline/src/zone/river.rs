//! A zone's rivers read along their own polylines (M6 H3a): each stretch's current and slope, its falls and its
//! effect sites, cooked into the `.szone`'s presentation ([`sapphire_zone::water::river`] says what it holds).
//! Never the zone's camera splines, which are rails, and never the simulation's: nothing here is in the digest.
//!
//! # The rules
//!
//! - **A stretch** runs from one point to the next: its way is the unit vector between them across the ground,
//!   its speeds its points', its slope its surface's fall over its length across the ground, and its place
//!   along the river the sum of the stretches' lengths before it, as the drawn ribbon's `v` sums them
//!   ([`super::water`]).
//! - **A fall** is a run of neighbouring stretches each falling more than [`FALL_SLOPE`] for each metre across
//!   the ground (past 50°), its lip the first one's upstream point and its foot the last one's downstream point.
//!   A **fall's foot** is a site, its strength the fall's drop over [`FULL_FALL`], at most 1.
//! - **Rapids** need the zone's colliders. Every [`RAPIDS_SPACING`] along the river, at the middle of each such
//!   span, the cook looks at [`RAPIDS_LANES`] points across it, evenly inside its banks, and scores each as the
//!   product of three shares, each running straight between its two ends and held to 0 to 1: its current's
//!   speed (0 at [`RAPIDS_SLOW`], 1 at [`RAPIDS_FAST`]), its shallowness (the bed's depth under the surface by
//!   W3a's rule, [`ColliderGround::under`]: 0 at [`RAPIDS_DEEP`], 1 at [`RAPIDS_SHALLOW`]; no bed is deep, and
//!   land scores nothing) and its nearness to an obstacle (a closed solid standing through the surface with
//!   some of its footprint inside the river's banks there, the span's half width either side of its axis, so
//!   a rock or a pier in it and never a bank or a wall beside it, [`ColliderGround::obstacle`]: 0 at
//!   [`RAPIDS_FAR`] or none, 1 at [`RAPIDS_NEAR`]). The thresholds are the zone's
//!   ([`sapphire_zone::water::river`]), so the renderer reads the same ones. The span's best point,
//!   the first of the best, is a rapids site if it scores at least [`RAPIDS_MIN`], its strength its score. A
//!   fall's stretches have none: its foot speaks for it. Fast, shallow water with nothing to break on throws no
//!   spray, though H3b's foam still reads it from the info texture.
//! - **Too many sites:** a river keeps at most [`MAX_SITES`], every fall's foot and then the strongest rapids,
//!   the furthest upstream of those as strong; sites run downstream.
//!
//! # Determinism
//!
//! Every byte is from `f64` sums, differences, products, quotients and square roots of `f32` values, each
//! correctly rounded, comparisons and `clamp`, and the ground's own (its triangles' heights and its solids'
//! distances, the same arithmetic): no transcendental, no hash order, nothing of threads. So the same bytes on
//! every machine; a test pins them.
//!
//! # Refusals
//!
//! A river whose surface climbs more steeply than a fall falls, where its water couldn't run, fails the zone's
//! cook at the point it climbs to (`water[i].shape.points[j].at`): most likely its points run from its mouth.

use sapphire_zone::water::river::{
    FALL_SLOPE, MAX_SITES, RAPIDS_DEEP, RAPIDS_FAR, RAPIDS_FAST, RAPIDS_NEAR, RAPIDS_SHALLOW, RAPIDS_SLOW,
};
use sapphire_zone::water::{CookedFall, CookedStretch, MAX_RIVER_POINTS};
use sapphire_zone::{CookedRiver, RiverPoint, WaterSite, WaterSiteKind};

use super::scatter::{Banks, ColliderGround};
use super::water::{right, stretches, xz};
use crate::cache::Stage;

/// What reads a river (M6 H3a). Bump the version whenever it changes a reading for the same points and colliders,
/// [`ColliderGround::obstacle`] included: the `.szone`'s key holds it, with `WATER_COOKER`'s for the bed's rule.
pub const RIVER_READING: Stage = Stage { name: "river reading", version: 1 };

/// How far apart along a river the cook looks for rapids, in metres.
pub const RAPIDS_SPACING: f64 = 4.0;
/// How many points across a river the cook looks at for rapids.
pub const RAPIDS_LANES: u32 = 5;
/// The least score a rapids site has.
pub const RAPIDS_MIN: f64 = 0.2;
/// The drop, in metres, at which a fall's foot is at its strongest.
pub const FULL_FALL: f64 = 8.0;

/// What `cook --report` says of a river's reading, a line a river.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RiverLine {
    /// Its zone's document.
    pub zone: std::path::PathBuf,
    /// Its body's name.
    pub name: String,
    pub stretches: usize,
    pub falls: usize,
    /// Its rapids sites.
    pub rapids: usize,
    /// Its falls' feet's sites.
    pub feet: usize,
}

impl RiverLine {
    /// Each river's line in a cooked zone's `bytes`, in its cooked water's order (by ID).
    pub fn of_zone(zone: &std::path::Path, bytes: &[u8]) -> Result<Vec<Self>, sapphire_zone::ZoneError> {
        let cooked = sapphire_zone::Zone::read(bytes)?;
        let volumes = cooked.simulation.water.iter();
        Ok((volumes.zip(cooked.presentation.water.iter()))
            .filter_map(|(volume, surface)| {
                let river = surface.river.as_ref()?;
                let count = |kind: WaterSiteKind| {
                    river.sites.iter().filter(|site| site.kind.to_native() == kind).count()
                };
                Some(Self {
                    zone: zone.to_owned(),
                    name: volume.name.to_string(),
                    stretches: river.stretches.len(),
                    falls: river.falls.len(),
                    rapids: count(WaterSiteKind::Rapids),
                    feet: count(WaterSiteKind::FallFoot),
                })
            })
            .collect())
    }
}

// A fall's stretches are numbered in `u16`s, and a river's falls' feet (a fall at most every other stretch)
// always leave its sites room.
const _: () = assert!(MAX_RIVER_POINTS <= u16::MAX as usize && MAX_RIVER_POINTS / 2 <= MAX_SITES);

/// Reads a river of `points` (W2's), its rapids on `ground` if the zone has colliders: or what's wrong with it, by
/// its field under `water[i]`.
pub fn cook(points: &[RiverPoint], ground: Option<&ColliderGround>) -> Result<CookedRiver, (String, String)> {
    let ways = stretches(points);
    let mut stretch_list = Vec::with_capacity(ways.len());
    let mut along = 0.0f64;
    for (index, &(way, length)) in ways.iter().enumerate() {
        let (from, to) = (points[index], points[index + 1]);
        let drop = f64::from(from.at[1]) - f64::from(to.at[1]);
        if -drop > length * FALL_SLOPE {
            let problem = format!(
                "the river climbs {:.2} m over the {length:.2} m here, more steeply than a fall falls, where its \
                 water couldn't run; list its points from its source to its mouth",
                -drop
            );
            return Err((format!("shape.points[{}].at", index + 1), problem));
        }
        let start = along;
        along += length;
        stretch_list.push(CookedStretch {
            way: way.map(|value| value as f32),
            speed: [from.speed, to.speed],
            slope: (drop / length) as f32,
            along: [start as f32, along as f32],
            fall: drop > length * FALL_SLOPE,
        });
    }

    let mut falls = Vec::new();
    let mut index = 0;
    while index < stretch_list.len() {
        if !stretch_list[index].fall {
            index += 1;
            continue;
        }
        let first = index;
        while index + 1 < stretch_list.len() && stretch_list[index + 1].fall {
            index += 1;
        }
        let (lip, foot) = (points[first], points[index + 1]);
        falls.push(CookedFall {
            stretches: [first as u16, index as u16],
            lip: lip.at,
            foot: foot.at,
            along: [stretch_list[first].along[0], stretch_list[index].along[1]],
            width: [lip.width, foot.width],
        });
        index += 1;
    }

    // Each site with its place along the river, which orders them.
    let mut sites: Vec<(f32, WaterSite)> = falls
        .iter()
        .map(|fall| {
            let drop = f64::from(fall.lip[1]) - f64::from(fall.foot[1]);
            let strength = (drop / FULL_FALL).min(1.0) as f32;
            (fall.along[1], WaterSite { kind: WaterSiteKind::FallFoot, position: fall.foot, strength })
        })
        .collect();
    if let Some(ground) = ground {
        let mut rapids = rapids(points, &ways, &stretch_list, ground);
        let room = MAX_SITES.saturating_sub(sites.len());
        if rapids.len() > room {
            // The strongest, then the furthest upstream: a stable sort keeps their order on a tie.
            rapids.sort_by(|one, other| other.1.strength.total_cmp(&one.1.strength));
            rapids.truncate(room);
        }
        sites.extend(rapids);
    }
    sites.sort_by(|one, other| one.0.total_cmp(&other.0).then(one.1.kind.cmp(&other.1.kind)));
    Ok(CookedRiver {
        stretches: stretch_list,
        falls,
        sites: sites.into_iter().map(|(_, site)| site).collect(),
    })
}

/// From 0 at `zero` to 1 at `one`, straight between, held to 0 to 1.
fn share(value: f64, zero: f64, one: f64) -> f64 {
    ((value - zero) / (one - zero)).clamp(0.0, 1.0)
}

/// A river's rapids sites on `ground` (see the [module docs](self#the-rules)), each with its place along it, in
/// order downstream.
fn rapids(
    points: &[RiverPoint],
    ways: &[([f64; 2], f64)],
    stretches: &[CookedStretch],
    ground: &ColliderGround,
) -> Vec<(f32, WaterSite)> {
    let mut sites = Vec::new();
    let (mut stretch, mut start) = (0usize, 0.0f64);
    let mut span = 0u32;
    loop {
        let at = (f64::from(span) + 0.5) * RAPIDS_SPACING;
        span += 1;
        while stretch < ways.len() && at > start + ways[stretch].1 {
            start += ways[stretch].1;
            stretch += 1;
        }
        let Some(&(way, length)) = ways.get(stretch) else { break };
        if stretches[stretch].fall {
            continue;
        }
        let (from, to) = (points[stretch], points[stretch + 1]);
        let t = (at - start) / length;
        let between = |a: f32, b: f32| f64::from(a) + (f64::from(b) - f64::from(a)) * t;
        let ([ax, az], [bx, bz]) = (xz(points, stretch), xz(points, stretch + 1));
        let middle = [ax + (bx - ax) * t, az + (bz - az) * t];
        let surface = between(from.at[1], to.at[1]) as f32;
        let speed = share(between(from.speed, to.speed), RAPIDS_SLOW, RAPIDS_FAST);
        let half = between(from.width, to.width) * 0.5;
        let across = right(way);
        let banks = Banks { centre: middle, across, half };
        let mut best: Option<(f64, [f32; 3])> = None;
        for lane in 0..RAPIDS_LANES {
            let out = half * f64::from(2 * lane + 1) / f64::from(RAPIDS_LANES) - half;
            let (x, z) = ((middle[0] + across[0] * out) as f32, (middle[1] + across[1] * out) as f32);
            let depth = match ground.under(x, z, surface) {
                (Some(bed), _) if bed < f64::from(surface) => f64::from(surface) - bed,
                _ => continue,
            };
            let Some(distance) = ground.obstacle(x, z, surface, RAPIDS_FAR, &banks) else { continue };
            let score =
                speed * share(depth, RAPIDS_DEEP, RAPIDS_SHALLOW) * share(distance, RAPIDS_FAR, RAPIDS_NEAR);
            if best.is_none_or(|(highest, _)| score > highest) {
                best = Some((score, [x, surface, z]));
            }
        }
        if let Some((score, position)) = best
            && score >= RAPIDS_MIN
        {
            let site = WaterSite { kind: WaterSiteKind::Rapids, position, strength: score as f32 };
            sites.push((at as f32, site));
        }
    }
    sites
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::Triangle;

    /// A flat rectangle's two triangles at `height`, facing up.
    fn rectangle(min: [f32; 2], max: [f32; 2], height: f32) -> [Triangle; 2] {
        let corner = |x: f32, z: f32| [x, height, z];
        [
            [corner(min[0], min[1]), corner(min[0], max[1]), corner(max[0], max[1])],
            [corner(min[0], min[1]), corner(max[0], max[1]), corner(max[0], min[1])],
        ]
    }

    /// A box's faces between `min` and `max`: its top facing up, its bottom down, and four walls.
    fn block(min: [f32; 3], max: [f32; 3]) -> Vec<Triangle> {
        let top = rectangle([min[0], min[2]], [max[0], max[2]], max[1]);
        let bottom = rectangle([min[0], min[2]], [max[0], max[2]], min[1]).map(|[a, b, c]| [a, c, b]);
        let wall = |a: [f32; 2], b: [f32; 2]| -> [Triangle; 2] {
            let (low, high) = (min[1], max[1]);
            [
                [[a[0], low, a[1]], [b[0], low, b[1]], [b[0], high, b[1]]],
                [[a[0], low, a[1]], [b[0], high, b[1]], [a[0], high, a[1]]],
            ]
        };
        let corners = [[min[0], min[2]], [max[0], min[2]], [max[0], max[2]], [min[0], max[2]]];
        let walls = (0..4).flat_map(|k| wall(corners[k], corners[(k + 1) % 4]));
        top.into_iter().chain(bottom).chain(walls).collect()
    }

    /// A bed at `bed` under a river running along +X, and a rock from under it to 2 m over its surface, its
    /// footprint `rock`: its solid among the ground's.
    fn bed_with_rock(bed: f32, rock: ([f32; 2], [f32; 2])) -> ColliderGround {
        let rock = block([rock.0[0], -4.0, rock.0[1]], [rock.1[0], 4.0, rock.1[1]]);
        let faces: Vec<Triangle> = rectangle([-100.0, -100.0], [100.0, 100.0], bed)
            .into_iter()
            .chain(rock.iter().copied())
            .collect();
        ColliderGround::with_solids(&faces, &[rock]).expect("small")
    }

    fn point(x: f32, y: f32, width: f32, speed: f32) -> RiverPoint {
        RiverPoint { at: [x, y, 0.0], width, speed }
    }

    /// The tangent of 60°, as a fixture's drop for each metre across the ground.
    const SIXTY: f32 = 1.732_050_8;

    /// A river over level ground, then a stretch dropping at 60° over 2 m, then level again: one fall, its lip and
    /// foot that stretch's ends, and a site at its foot as strong as its drop over 8 m.
    #[test]
    fn a_sixty_degree_drop_is_one_fall_with_a_site_at_its_foot() {
        let drop = 2.0 * SIXTY;
        let points = [
            point(0.0, 10.0, 4.0, 1.0),
            point(10.0, 10.0, 4.0, 1.0),
            point(12.0, 10.0 - drop, 5.0, 3.0),
            point(30.0, 10.0 - drop, 6.0, 1.0),
        ];
        let river = cook(&points, None).expect("reads");
        assert_eq!(river.stretches.len(), 3);
        let flags: Vec<bool> = river.stretches.iter().map(|stretch| stretch.fall).collect();
        assert_eq!(flags, [false, true, false]);
        assert!((river.stretches[1].slope - SIXTY).abs() < 1e-5, "{}", river.stretches[1].slope);
        assert_eq!(river.stretches[1].way, [1.0, 0.0]);
        assert_eq!(river.stretches[1].speed, [1.0, 3.0]);
        assert_eq!(river.stretches[1].along, [10.0, 12.0], "the ribbon's v across the ground");
        assert_eq!(river.stretches[2].along, [12.0, 30.0]);
        let [fall] = river.falls.as_slice() else { panic!("one fall: {:?}", river.falls) };
        assert_eq!(fall.stretches, [1, 1]);
        assert_eq!((fall.lip, fall.foot), (points[1].at, points[2].at));
        assert_eq!((fall.along, fall.width), ([10.0, 12.0], [4.0, 5.0]));
        let [site] = river.sites.as_slice() else { panic!("one site: {:?}", river.sites) };
        assert_eq!((site.kind, site.position), (WaterSiteKind::FallFoot, points[2].at));
        assert!((f64::from(site.strength) - f64::from(drop) / FULL_FALL).abs() < 1e-6, "{}", site.strength);
    }

    /// Steep neighbours are one fall; a drop of 45° isn't one; and a fall at the river's end has its foot there.
    #[test]
    fn steep_neighbours_are_one_fall_and_a_gentler_drop_none() {
        let points = [
            point(0.0, 20.0, 4.0, 1.0),
            point(1.0, 20.0 - SIXTY, 4.0, 1.0),
            point(2.0, 20.0 - 2.0 * SIXTY, 4.0, 1.0),
            point(10.0, 20.0 - 2.0 * SIXTY, 4.0, 1.0),
            point(14.0, 16.0 - 2.0 * SIXTY, 4.0, 1.0),
            point(15.0, 14.0 - 2.0 * SIXTY, 4.0, 1.0),
        ];
        let river = cook(&points, None).expect("reads");
        let flags: Vec<bool> = river.stretches.iter().map(|stretch| stretch.fall).collect();
        assert_eq!(flags, [true, true, false, false, true], "45° is no fall; 63° is");
        let spans: Vec<[u16; 2]> = river.falls.iter().map(|fall| fall.stretches).collect();
        assert_eq!(spans, [[0, 1], [4, 4]]);
        assert_eq!(river.falls[1].foot, points[5].at, "its foot is the river's end");
        assert_eq!(river.sites.len(), 2, "a site at each foot");
        assert!((f64::from(river.sites[0].strength) - 2.0 * f64::from(SIXTY) / FULL_FALL).abs() < 1e-6);
        assert_eq!(river.sites[1].strength, 0.25, "a 2 m drop");
    }

    /// Fast, shallow water past a rock throws rapids beside it, and nowhere far from it; the same river slow, or
    /// deep, or with no rock, throws none.
    #[test]
    fn fast_shallow_water_past_a_rock_is_rapids_and_gentle_water_none() {
        let river = |speed: f32| [point(0.0, 1.0, 6.0, speed), point(40.0, 1.0, 6.0, speed)];
        let rock = ([19.0, -1.0], [21.0, 1.0]);
        let ground = bed_with_rock(0.6, rock);
        let fast = cook(&river(5.0), Some(&ground)).expect("reads");
        assert!(!fast.sites.is_empty(), "rapids by the rock");
        for site in &fast.sites {
            assert_eq!(site.kind, WaterSiteKind::Rapids);
            let [x, y, _] = site.position;
            assert!((16.0..=24.0).contains(&x), "beside the rock: {site:?}");
            assert_eq!(y, 1.0, "on the surface");
            assert!(site.strength >= RAPIDS_MIN as f32 && site.strength <= 1.0);
        }
        // At 18 m, the middle lane is a metre short of the rock: full speed, the bed 0.4 m down, nearly near.
        let expected = share(0.4, RAPIDS_DEEP, RAPIDS_SHALLOW) * share(1.0, RAPIDS_FAR, RAPIDS_NEAR);
        assert_eq!(fast.sites[0].position, [18.0, 1.0, 0.0]);
        assert!((f64::from(fast.sites[0].strength) - expected).abs() < 1e-3, "{:?}", fast.sites[0]);

        let gentle = cook(&river(0.5), Some(&ground)).expect("reads");
        assert_eq!(gentle.sites, [], "slow water");
        let deep = cook(&river(5.0), Some(&bed_with_rock(-2.0, rock))).expect("reads");
        assert_eq!(deep.sites, [], "deep water");
        let open = cook(&river(5.0), Some(&bed_with_rock(0.6, ([60.0, -1.0], [62.0, 1.0])))).expect("reads");
        assert_eq!(open.sites, [], "nothing to break on");
        assert_eq!(cook(&river(5.0), None).expect("reads").sites, [], "no colliders, no rapids");
    }

    /// A rock wholly under the surface is the bed, not an obstacle; one wholly over it is a bridge.
    #[test]
    fn only_a_solid_through_the_surface_is_an_obstacle() {
        let rock = block([19.0, -4.0, -1.0], [21.0, 0.9, 1.0]);
        let deck = block([19.0, 3.0, -4.0], [21.0, 4.0, 4.0]);
        let faces: Vec<Triangle> = rectangle([-100.0, -100.0], [100.0, 100.0], 0.6)
            .into_iter()
            .chain(rock.iter().copied())
            .chain(deck.iter().copied())
            .collect();
        let ground = ColliderGround::with_solids(&faces, &[rock, deck]).expect("small");
        let wide = Banks { centre: [0.0, 0.0], across: [0.0, 1.0], half: 100.0 };
        assert_eq!(ground.obstacle(18.0, 0.0, 1.0, RAPIDS_FAR, &wide), None);
        let standing = bed_with_rock(0.6, ([19.0, -1.0], [21.0, 1.0]));
        assert_eq!(standing.obstacle(18.0, 0.0, 1.0, RAPIDS_FAR, &wide), Some(1.0));
        assert_eq!(standing.obstacle(20.0, 0.5, 1.0, RAPIDS_FAR, &wide), Some(0.0), "over it");
        assert_eq!(
            standing.obstacle(18.0, 3.0, 1.0, RAPIDS_FAR, &wide),
            Some(5.0_f64.sqrt()),
            "to its corner"
        );
        assert_eq!(standing.obstacle(10.0, 0.0, 1.0, RAPIDS_FAR, &wide), None, "out of reach");
        // A river 4 m wide whose axis runs 3 m north of the rock: its south bank only touches it, from outside.
        let beside = Banks { centre: [18.0, 3.0], across: [0.0, 1.0], half: 2.0 };
        assert_eq!(standing.obstacle(18.0, 1.5, 1.0, RAPIDS_FAR, &beside), None, "a bank, not in the river");
        let over = Banks { half: 2.1, ..beside };
        assert_eq!(standing.obstacle(18.0, 1.5, 1.0, RAPIDS_FAR, &over), Some(1.25_f64.sqrt()), "in it");
    }

    /// A fast, shallow river between two walls along its banks throws no rapids: they're its banks, not rocks
    /// in it. With a rock in it too, it throws them by the rock and only there.
    #[test]
    fn walls_along_a_rivers_banks_are_no_obstacle() {
        let river = [point(0.0, 1.0, 6.0, 5.0), point(40.0, 1.0, 6.0, 5.0)];
        let walls =
            [block([-10.0, -4.0, 3.0], [50.0, 4.0, 4.0]), block([-10.0, -4.0, -4.0], [50.0, 4.0, -3.0])];
        let rock = block([19.0, -4.0, -1.0], [21.0, 4.0, 1.0]);
        let ground = |solids: &[Vec<Triangle>]| {
            let faces: Vec<Triangle> = rectangle([-100.0, -100.0], [100.0, 100.0], 0.6)
                .into_iter()
                .chain(solids.iter().flatten().copied())
                .collect();
            ColliderGround::with_solids(&faces, solids).expect("small")
        };
        let channel = cook(&river, Some(&ground(&walls))).expect("reads");
        assert_eq!(channel.sites, [], "its banks throw no spray, though its outer lanes are 0.6 m from them");
        let [north, south] = walls;
        let rocky = cook(&river, Some(&ground(&[north, south, rock]))).expect("reads");
        assert!(!rocky.sites.is_empty(), "rapids by the rock");
        for site in &rocky.sites {
            assert_eq!(site.kind, WaterSiteKind::Rapids);
            assert!((16.0..=24.0).contains(&site.position[0]), "by the rock, not the walls: {site:?}");
        }
    }

    /// A river climbing more steeply than a fall falls is refused at the point it climbs to.
    #[test]
    fn a_river_climbing_like_a_fall_is_refused() {
        let points = [point(0.0, 1.0, 4.0, 1.0), point(10.0, 1.0, 4.0, 1.0), point(11.0, 3.0, 4.0, 1.0)];
        let (field, problem) = cook(&points, None).expect_err("climbs");
        assert_eq!(field, "shape.points[2].at");
        assert!(problem.contains("climbs 2.00 m"), "{problem}");
        // A gentle rise is the author's to make.
        let rising = [point(0.0, 1.0, 4.0, 1.0), point(10.0, 2.0, 4.0, 1.0)];
        assert!(cook(&rising, None).expect("reads").stretches[0].slope < 0.0);
    }

    /// A river keeps at most `MAX_SITES` sites: every fall's foot, then its strongest rapids, and of those as
    /// strong as the weakest it keeps, the furthest upstream.
    #[test]
    fn a_long_river_keeps_its_strongest_sites() {
        let length = (MAX_SITES as f32 + 40.0) * RAPIDS_SPACING as f32;
        let points =
            [point(0.0, 1.0, 6.0, 5.0), point(length, 1.0, 6.0, 5.0), point(length + 1.0, -1.0, 6.0, 5.0)];
        // A ridge of rock along the river's north bank, from end to end: rapids all the way, nearest at its east.
        let rock = block([-10.0, -4.0, 2.0], [length + 10.0, 4.0, 2.5]);
        let mut faces: Vec<Triangle> = rock.clone();
        let step = 64.0;
        let mut x = -10.0;
        while x < length + 20.0 {
            // The bed shoals eastward, so the rapids grow stronger downstream.
            let bed = 0.0 + 0.6 * (x / (length + 20.0));
            faces.extend(rectangle([x, -100.0], [x + step, 100.0], bed));
            x += step;
        }
        let ground = ColliderGround::with_solids(&faces, &[rock]).expect("small");
        let river = cook(&points, Some(&ground)).expect("reads");
        assert_eq!(river.sites.len(), MAX_SITES);
        assert_eq!(
            river.sites.last().map(|site| site.kind),
            Some(WaterSiteKind::FallFoot),
            "the foot is kept"
        );
        let rapids: Vec<&WaterSite> =
            river.sites.iter().filter(|site| site.kind == WaterSiteKind::Rapids).collect();
        assert!(rapids.windows(2).all(|pair| pair[0].position[0] < pair[1].position[0]), "downstream");
        assert!(rapids[0].position[0] > 100.0, "the weakest, upstream, dropped: {:?}", rapids[0]);

        // Against every rapids site the river would have held with room for them all.
        let reading = cook(&points, None).expect("reads");
        let every = super::rapids(&points, &stretches(&points), &reading.stretches, &ground);
        let (kept, dropped): (Vec<(f32, WaterSite)>, Vec<_>) = every
            .iter()
            .copied()
            .partition(|(_, site)| rapids.iter().any(|kept| kept.position == site.position));
        assert_eq!(
            (kept.len(), kept.len() + dropped.len()),
            (rapids.len(), every.len()),
            "each kept one is one"
        );
        let weakest = kept.iter().map(|(_, site)| site.strength).fold(f32::INFINITY, f32::min);
        let strongest = dropped.iter().map(|(_, site)| site.strength).fold(f32::NEG_INFINITY, f32::max);
        assert!(weakest >= strongest, "kept {weakest}, dropped {strongest}");
        let tied = |sites: &[(f32, WaterSite)]| -> Vec<f32> {
            sites.iter().filter(|(_, site)| site.strength == weakest).map(|&(along, _)| along).collect()
        };
        let (kept_tied, dropped_tied) = (tied(&kept), tied(&dropped));
        assert!(!dropped_tied.is_empty(), "the cut falls among sites as strong as each other");
        let upstream = kept_tied.iter().fold(f32::NEG_INFINITY, |far, &along| far.max(along));
        assert!(dropped_tied.iter().all(|&along| upstream < along), "of those as strong, the upstream ones");
    }

    /// A river's reading is the same bytes every time, pinned: a fall and rapids past a rock, over the ground's
    /// triangles and solid.
    #[test]
    fn a_rivers_reading_is_the_same_bytes_and_is_pinned() {
        let drop = 2.0 * SIXTY;
        let points = [
            point(-30.0, 1.0 + drop, 6.0, 2.0),
            point(-12.0, 1.0 + drop, 6.0, 3.0),
            point(-10.0, 1.0, 7.0, 4.5),
            point(10.0, 1.0, 6.0, 5.0),
            point(40.0, 1.0, 6.5, 2.5),
        ];
        let ground = bed_with_rock(0.6, ([19.0, -1.0], [21.0, 1.0]));
        // Every value's bits in its order, as the `.szone` holds them.
        let digest = || {
            let river = cook(&points, Some(&ground)).expect("reads");
            let mut hasher = blake3::Hasher::new();
            let mut floats = |values: &[f32]| {
                for value in values {
                    hasher.update(&value.to_bits().to_le_bytes());
                }
            };
            for stretch in &river.stretches {
                floats(&[stretch.way[0], stretch.way[1], stretch.speed[0], stretch.speed[1], stretch.slope]);
                floats(&[stretch.along[0], stretch.along[1], f32::from(u8::from(stretch.fall))]);
            }
            for fall in &river.falls {
                floats(&[f32::from(fall.stretches[0]), f32::from(fall.stretches[1])]);
                floats(&fall.lip);
                floats(&fall.foot);
                floats(&[fall.along[0], fall.along[1], fall.width[0], fall.width[1]]);
            }
            for site in &river.sites {
                floats(&[f32::from(site.kind as u8)]);
                floats(&site.position);
                floats(&[site.strength]);
            }
            hasher.finalize().to_hex().to_string()
        };
        assert_eq!(digest(), digest());
        assert_eq!(
            digest(),
            "efb5b6171a6805d1a960b7ed85e7d772bed1649669b7b64c21a726c92c83ccdf",
            "the river's reading moved"
        );
    }
}
