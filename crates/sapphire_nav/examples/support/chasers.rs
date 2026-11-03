//! Chasers with no physics: the smallest thing that moves on what [`Navigation::follow`] says, as a brain
//! would, for the allocation test and the bench to share. Each walks straight where it's told and stands
//! on the mesh. There's nobody to bump into, so all that a tick costs here is paths.

use glam::Vec3;
use sapphire_nav::{NavStats, Navigation, Path, PathSearch, Reach, RouteEnd, Steer};
use sapphire_navmesh::ArchivedNavMesh;

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

struct Chaser {
    position: Vec3,
    speed: f32,
    path: Path,
}

/// `N` chasers and the navigation they share. Sized once, and nothing in [`Chasers::step`] allocates.
pub struct Chasers {
    chasers: Vec<Chaser>,
    navigation: Navigation,
    /// FNV-1a over every position and path, by bits, every tick.
    pub checksum: u64,
    /// Everything spent so far, `worst_tick` the worst of them.
    pub stats: NavStats,
    /// How many times, over all ticks, a chaser stood on a route that found no way.
    pub no_way: u64,
}

impl Chasers {
    /// `count` chasers, on polygons a seeded generator picks from those a search from `home` reaches,
    /// at speeds that differ, because identical actors hide an ordering leak.
    pub fn new(mesh: &ArchivedNavMesh, count: usize, budget: u32, home: Vec3, seed: u64) -> Self {
        let navigation = Navigation::with_budget(mesh, budget);
        let graph = navigation.graph();
        let mut search = PathSearch::new(graph, u32::MAX);
        let mut corridor = vec![0; graph.polygon_count()];
        let home = graph.locate(home, Reach::CHARACTER).expect("the chasers' home is on the mesh");
        let mut random = Lcg(seed);
        let mut chasers = Vec::with_capacity(count);
        while chasers.len() < count {
            let position = graph.centre(random.next(graph.polygon_count() as u32));
            let Some(start) = graph.locate(position, Reach::CHARACTER) else { continue };
            if search.find(graph, start, home, &mut corridor).end != RouteEnd::Reached {
                continue;
            }
            let speed = 3.0 + chasers.len() as f32 * 0.037;
            chasers.push(Chaser { position, speed, path: Path::new() });
        }
        Self { chasers, navigation, checksum: 0xcbf2_9ce4_8422_2325, stats: NavStats::default(), no_way: 0 }
    }

    /// One fixed step: each chaser, in order, follows its path toward `target`, and walks.
    pub fn step(&mut self, target: Vec3) {
        self.navigation.begin_tick();
        for chaser in &mut self.chasers {
            if let Steer::Walk(direction) = self.navigation.follow(&mut chaser.path, chaser.position, target)
            {
                let moved = chaser.position + direction * (chaser.speed * TICK);
                if let Some(on) = self.navigation.graph().locate(moved, Reach::CHARACTER) {
                    chaser.position = on.position;
                }
            }
            if chaser.path.route_end() == Some(RouteEnd::Unreachable) {
                self.no_way += 1;
            }
            let checksum = &mut self.checksum;
            let mut write = |bytes: &[u8]| {
                for byte in bytes {
                    *checksum = (*checksum ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
                }
            };
            for value in chaser.position.to_array() {
                write(&value.to_bits().to_le_bytes());
            }
            chaser.path.write_state(&mut write);
        }
        let stats = self.navigation.take_stats();
        self.stats.searches += stats.searches;
        self.stats.expanded += stats.expanded;
        self.stats.out_of_nodes += stats.out_of_nodes;
        self.stats.deferred += stats.deferred;
        self.stats.worst_tick = self.stats.worst_tick.max(stats.worst_tick);
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
