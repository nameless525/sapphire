//! P0's chasers: the smallest thing that moves on what `sapphire_nav` returns, as a brain would, for
//! the allocation test and the bench to share. A chaser walks its corners in a straight line and
//! searches again when its turn comes. There's no separation and no corridor to reuse, so every
//! search is a whole search, which is the worst a path can cost.

use glam::Vec3;
use sapphire_nav::{FunnelEnd, NavGraph, PathSearch, Reach, RouteEnd, straighten};

const CORRIDOR: usize = 256;
const CORNERS: usize = 32;

/// 60 Hz.
const TICK: f32 = 1.0 / 60.0;

/// A small generator with no platform in it.
pub struct Lcg(pub u64);

impl Lcg {
    pub fn next(&mut self, range: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (((self.0 >> 33) * u64::from(range)) >> 31) as u32
    }
}

/// A polygon's corners' mean, which a convex polygon holds.
pub fn centre(graph: &NavGraph, polygon: u32) -> Vec3 {
    let mut sum = Vec3::ZERO;
    let mut count = 0.0;
    for corner in graph.polygon(polygon) {
        sum += corner;
        count += 1.0;
    }
    sum / count
}

#[derive(Clone)]
struct Chaser {
    position: Vec3,
    speed: f32,
    corners: [Vec3; CORNERS],
    count: u32,
    next: u32,
}

/// `N` chasers and what their searches share. Sized once, and nothing in [`Chasers::step`] allocates.
pub struct Chasers {
    chasers: Vec<Chaser>,
    search: PathSearch,
    corridor: Box<[u32; CORRIDOR]>,
    /// Whose search is next, in ID order.
    turn: usize,
    /// FNV-1a over every position, by bits, every tick.
    pub checksum: u64,
    pub searches: u64,
    pub unreached: u64,
}

impl Chasers {
    /// `count` chasers, on polygons a seeded generator picks from those a search from `home` reaches,
    /// at speeds that differ, because identical actors hide an ordering leak.
    pub fn new(graph: &NavGraph, count: usize, node_limit: u32, home: Vec3, seed: u64) -> Self {
        let mut search = PathSearch::new(graph, node_limit);
        let mut corridor = Box::new([0; CORRIDOR]);
        let home = graph.locate(home, Reach::CHARACTER).expect("the chasers' home is on the mesh");
        let mut random = Lcg(seed);
        let mut chasers = Vec::with_capacity(count);
        while chasers.len() < count {
            let polygon = random.next(graph.polygon_count() as u32);
            let position = centre(graph, polygon);
            let Some(start) = graph.locate(position, Reach::CHARACTER) else { continue };
            let route = search.find(graph, start, home, &mut corridor[..]);
            if route.end != RouteEnd::Reached {
                continue;
            }
            let speed = 3.0 + chasers.len() as f32 * 0.037;
            chasers.push(Chaser { position, speed, corners: [position; CORNERS], count: 1, next: 0 });
        }
        Self {
            chasers,
            search,
            corridor,
            turn: 0,
            checksum: 0xcbf2_9ce4_8422_2325,
            searches: 0,
            unreached: 0,
        }
    }

    /// One fixed step: `budget` chasers search for `target`, then all of them walk.
    pub fn step(&mut self, graph: &NavGraph, target: Vec3, budget: usize) {
        if let Some(goal) = graph.locate(target, Reach::CHARACTER) {
            for _ in 0..budget.min(self.chasers.len()) {
                let index = self.turn;
                self.turn = (self.turn + 1) % self.chasers.len();
                let chaser = &mut self.chasers[index];
                let Some(from) = graph.locate(chaser.position, Reach::CHARACTER) else { continue };
                let route = self.search.find(graph, from, goal, &mut self.corridor[..]);
                self.searches += 1;
                if route.end != RouteEnd::Reached {
                    self.unreached += 1;
                }
                let corridor = &self.corridor[..route.polygons as usize];
                let (count, end) =
                    straighten(graph, corridor, from.position, goal.position, &mut chaser.corners);
                debug_assert!(end == FunnelEnd::Whole || count as usize == CORNERS);
                chaser.count = count;
                chaser.next = 1;
            }
        }
        for chaser in &mut self.chasers {
            let mut left = chaser.speed * TICK;
            while chaser.next < chaser.count && left > 0.0 {
                let to = chaser.corners[chaser.next as usize];
                let gap = chaser.position.distance(to);
                if gap <= left {
                    chaser.position = to;
                    chaser.next += 1;
                    left -= gap;
                } else {
                    chaser.position += (to - chaser.position) * (left / gap);
                    left = 0.0;
                }
            }
            for value in chaser.position.to_array() {
                for byte in value.to_bits().to_le_bytes() {
                    self.checksum = (self.checksum ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
    }
}

/// Where the target walks at `tick`: straight between `points` at 4 m/s, round and round.
pub fn target_at(points: &[Vec3], tick: u32) -> Vec3 {
    let mut walked = tick as f32 * TICK * 4.0;
    let mut total = 0.0;
    for (index, from) in points.iter().enumerate() {
        total += from.distance(points[(index + 1) % points.len()]);
    }
    walked %= total;
    for (index, &from) in points.iter().enumerate() {
        let to = points[(index + 1) % points.len()];
        let gap = from.distance(to);
        if walked <= gap {
            return from + (to - from) * (walked / gap);
        }
        walked -= gap;
    }
    points[0]
}
