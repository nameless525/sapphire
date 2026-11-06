//! What a brain calls: a path kept from tick to tick, and the budget all of a tick's searches share.
//!
//! ```text
//! navigation.begin_tick();                               // once, ahead of the tick's first follow
//! for each actor, in ID order {
//!     match navigation.follow(&mut path, position, target) {
//!         Steer::Walk(direction) => stick = direction,   // a unit vector on the ground
//!         Steer::Arrived => stick = zero,
//!         Steer::NoPath => steer some other way,         // off the mesh, or no search has come yet
//!     }
//! }
//! ```
//!
//! That's all a brain knows of navigation. **What keeps it cheap** is that most ticks search nothing:
//! a [`Path`] holds a corridor of polygons and the corners through it, and `follow` searches again only
//! when the corridor has stopped being any use (see [`Path::wants_search`]). Everything else is finding
//! which polygon the character's on, in a corridor it knows, and walking the next corner.
//!
//! **The budget** is [`NODES_PER_TICK`] polygons expanded, across all actors, in the order they ask, which
//! is actor order. A path that asks with too little left waits for a later tick and walks what it has.
//! Searches last until the target moves, so nobody waits long: an actor served this tick isn't asking
//! next tick, and `NavStats::deferred` counts those who waited. A search that spends all it was given
//! returns a corridor as near the goal as it looked, walks that, and asks again as it goes.
//!
//! **Keeping characters apart** isn't here: P3 does that, where it has every actor's place to hand.

use glam::Vec3;
use sapphire_navmesh::ArchivedNavMesh;

use crate::funnel::{FunnelEnd, straighten};
use crate::graph::{Location, NavGraph, Reach};
use crate::search::{PathSearch, RouteEnd};

/// How many polygons all of a tick's searches may expand: the plan's budget, about 60 µs of a 16.7 ms
/// step at the 58 ns a node P0 measured on an M5.
pub const NODES_PER_TICK: u32 = 1024;

/// Under this many nodes left in a tick, a path waits for the next: the shortest crossing of an arena
/// takes about 50, and a search that can't get anywhere is nodes spent on nothing.
const MIN_SEARCH_NODES: u32 = 64;

/// Polygons a path remembers. A route longer than this is walked this far and searched again.
pub const CORRIDOR_POLYGONS: usize = 256;

/// Corners a path remembers ahead of it. When they run out it straightens again.
pub const PATH_CORNERS: usize = 16;

/// How far a target must move, in metres, before a path searches for it again. At a run of 4 m/s that's
/// every 15 ticks at most.
const REPATH_DISTANCE: f32 = 1.0;

/// How far a target must move, in metres, before a path bends its last leg to it: no search, only the
/// funnel again.
const REGOAL_DISTANCE: f32 = 0.25;

/// How near a corner counts as reached, in metres: a run covers about 0.07 m a tick.
const CORNER_RADIUS: f32 = 0.2;

/// What a character should do this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Steer {
    /// Walk this way: a unit vector on the XZ plane.
    Walk(Vec3),
    /// It's where it was going, or as near as there's any way to get.
    Arrived,
    /// It isn't on the mesh, or it has asked for a search that hasn't had its turn yet.
    NoPath,
}

/// What a [`Path`] came to at its last search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Searched {
    /// It hasn't had one, or what it had no longer holds: it wants one.
    Never,
    Ended(RouteEnd),
}

/// One character's way to somewhere, carried from tick to tick. A fixed size, so holding one allocates
/// nothing however it's stored, and the same for any route.
#[derive(Clone, Debug)]
pub struct Path {
    corridor: [u32; CORRIDOR_POLYGONS],
    corridor_len: u32,
    /// Which of `corridor` the character's on.
    at: u32,
    corners: [Vec3; PATH_CORNERS],
    corner_count: u32,
    /// The corner it's walking to.
    next_corner: u32,
    /// Whether the corners end at the goal, or there's more of the corridor past them.
    whole: bool,
    /// Where it's going, on the mesh, as the corners stand.
    goal: Vec3,
    /// Where its target was at its last search.
    searched_for: Vec3,
    searched: Searched,
}

impl Default for Path {
    fn default() -> Self {
        Self {
            corridor: [0; CORRIDOR_POLYGONS],
            corridor_len: 0,
            at: 0,
            corners: [Vec3::ZERO; PATH_CORNERS],
            corner_count: 0,
            next_corner: 0,
            whole: false,
            goal: Vec3::ZERO,
            searched_for: Vec3::ZERO,
            searched: Searched::Never,
        }
    }
}

impl Path {
    /// A path with no route yet, which wants a search at its first [`Navigation::follow`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the route: a new target, say, for which the old one is no use. Its next
    /// [`Navigation::follow`] searches.
    pub fn clear(&mut self) {
        self.searched = Searched::Never;
        self.corridor_len = 0;
        self.at = 0;
        self.corner_count = 0;
        self.next_corner = 0;
    }

    /// The polygons of its route, from where it is on.
    #[must_use]
    pub fn corridor(&self) -> &[u32] {
        &self.corridor[self.at as usize..self.corridor_len as usize]
    }

    /// The corners ahead of it, the next first.
    #[must_use]
    pub fn corners(&self) -> &[Vec3] {
        &self.corners[self.next_corner as usize..self.corner_count as usize]
    }

    /// How its last search ended, if it has had one that still holds.
    #[must_use]
    pub fn route_end(&self) -> Option<RouteEnd> {
        match self.searched {
            Searched::Never => None,
            Searched::Ended(end) => Some(end),
        }
    }

    /// Whether it needs a search, for a target at `to` on polygon `to_polygon`:
    ///
    /// - It has no route, or the character has left its corridor.
    /// - The target has gone more than a metre from where it was searched for, off the polygon the route
    ///   ends on.
    /// - Its route stops short, because the search ran out of nodes or its corridor filled, and it has
    ///   come to the last of it, or crossed a polygon since if the search ran out.
    ///
    /// A route that found no way doesn't ask again until the target moves, or every tick would search
    /// the whole of an island for nothing.
    #[must_use]
    pub fn wants_search(&self, to: Vec3, to_polygon: Option<u32>) -> bool {
        let Searched::Ended(end) = self.searched else {
            return true;
        };
        let moved = to_polygon.is_some() && self.searched_for.distance(to) > REPATH_DISTANCE;
        let last = self.corridor_len.saturating_sub(1);
        if moved && to_polygon != Some(self.corridor[last as usize]) {
            return true;
        }
        match end {
            RouteEnd::Reached => self.corridor_len as usize == CORRIDOR_POLYGONS && self.at == last,
            RouteEnd::Unreachable => false,
            RouteEnd::OutOfNodes => self.at > 0 || self.corridor_len <= 1,
        }
    }

    /// Feeds what it holds to `write`, every field that decides what it does next, in a fixed order, for a
    /// state checksum to read. Only the slots in use: what's past them means nothing.
    pub fn write_state(&self, write: &mut impl FnMut(&[u8])) {
        for count in [self.corridor_len, self.at, self.corner_count, self.next_corner] {
            write(&count.to_le_bytes());
        }
        for polygon in &self.corridor[..self.corridor_len as usize] {
            write(&polygon.to_le_bytes());
        }
        let mut point = |point: Vec3| {
            for value in point.to_array() {
                write(&value.to_bits().to_le_bytes());
            }
        };
        for corner in &self.corners[..self.corner_count as usize] {
            point(*corner);
        }
        point(self.goal);
        point(self.searched_for);
        let searched: u8 = match self.searched {
            Searched::Never => 0,
            Searched::Ended(RouteEnd::Reached) => 1,
            Searched::Ended(RouteEnd::Unreachable) => 2,
            Searched::Ended(RouteEnd::OutOfNodes) => 3,
        };
        write(&[searched, u8::from(self.whole)]);
    }

    /// Straightens what's left of the corridor from where the character is.
    fn straighten(&mut self, graph: &NavGraph, from: Vec3) {
        let corridor = &self.corridor[self.at as usize..self.corridor_len as usize];
        let (count, end) = straighten(graph, corridor, from, self.goal, &mut self.corners);
        self.corner_count = count;
        // The first corner is where it stands.
        self.next_corner = u32::from(count > 1);
        self.whole = end == FunnelEnd::Whole;
    }
}

/// What a tick's paths cost, since the last [`Navigation::take_stats`], for the stat counters (R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NavStats {
    pub searches: u32,
    pub expanded: u64,
    pub out_of_nodes: u32,
    /// Times a path wanted a search and the tick's budget had gone.
    pub deferred: u32,
    /// The most nodes any one tick spent: never past [`NODES_PER_TICK`].
    pub worst_tick: u32,
}

/// A zone's graph, the pool its searches share and what's left of this tick's budget. Built once at
/// load, and it allocates nothing after that.
#[derive(Clone, Debug)]
pub struct Navigation {
    graph: NavGraph,
    search: PathSearch,
    scratch: Box<[u32; CORRIDOR_POLYGONS]>,
    budget: u32,
    left: u32,
    stats: NavStats,
}

impl Navigation {
    /// From a checked mesh ([`NavMesh::read`](sapphire_navmesh::NavMesh::read)), with the plan's budget.
    #[must_use]
    pub fn new(mesh: &ArchivedNavMesh) -> Self {
        Self::with_budget(mesh, NODES_PER_TICK)
    }

    /// With another budget. Whatever's chosen is part of what a replay depends on.
    #[must_use]
    pub fn with_budget(mesh: &ArchivedNavMesh, budget: u32) -> Self {
        let graph = NavGraph::new(mesh);
        let search = PathSearch::new(&graph, budget);
        Self {
            graph,
            search,
            scratch: Box::new([0; CORRIDOR_POLYGONS]),
            budget,
            left: budget,
            stats: NavStats::default(),
        }
    }

    #[must_use]
    pub fn graph(&self) -> &NavGraph {
        &self.graph
    }

    /// What paths have cost since the last call.
    pub fn take_stats(&mut self) -> NavStats {
        std::mem::take(&mut self.stats)
    }

    /// Refills the budget. Once a tick, ahead of that tick's first [`Self::follow`].
    pub fn begin_tick(&mut self) {
        self.left = self.budget;
    }

    /// Which way a character at `position` should walk to get to `target`, keeping `path` up to date, and
    /// searching if it needs to and the tick has the nodes. Call it in actor order: who gets the budget
    /// is part of what a replay checks.
    pub fn follow(&mut self, path: &mut Path, position: Vec3, target: Vec3) -> Steer {
        let Some(here) = self.graph.locate(position, Reach::CHARACTER) else {
            return Steer::NoPath;
        };
        let goal = self.graph.locate(target, Reach::CHARACTER);

        // Where it's got to along its corridor.
        let mut moved_on = false;
        let on_route = path.searched != Searched::Never && {
            let ahead = &path.corridor[path.at as usize..path.corridor_len as usize];
            match ahead.iter().position(|&polygon| polygon == here.polygon) {
                Some(offset) => {
                    moved_on = offset > 0;
                    path.at += offset as u32;
                    true
                }
                None => false,
            }
        };
        if !on_route {
            path.clear();
        }

        if path.wants_search(target, goal.map(|goal| goal.polygon))
            && let Some(goal) = goal
        {
            if self.left < MIN_SEARCH_NODES.min(self.budget) {
                self.stats.deferred += 1;
            } else {
                self.search_for(path, here, goal, target);
                moved_on = false;
            }
        } else if let (Some(goal), true) = (goal, path.route_end() == Some(RouteEnd::Reached))
            && goal.polygon == path.corridor[path.corridor_len as usize - 1]
            && path.goal.distance(goal.position) > REGOAL_DISTANCE
        {
            path.goal = goal.position;
            moved_on = true;
        }
        if path.searched == Searched::Never {
            return Steer::NoPath;
        }
        if moved_on {
            path.straighten(&self.graph, here.position);
        }

        // Past the corners it has come to, straightening again if they run out, at most once a tick.
        let mut refilled = false;
        loop {
            while path.next_corner < path.corner_count {
                let corner = path.corners[path.next_corner as usize];
                if let Some(direction) = walk_toward(here.position, corner) {
                    return Steer::Walk(direction);
                }
                path.next_corner += 1;
            }
            if path.whole || refilled {
                return Steer::Arrived;
            }
            path.straighten(&self.graph, here.position);
            refilled = true;
        }
    }

    fn search_for(&mut self, path: &mut Path, here: Location, goal: Location, target: Vec3) {
        let limit = self.left;
        let route = self.search.find_within(&self.graph, here, goal, &mut self.scratch[..], limit);
        self.left -= route.expanded;
        self.stats.searches += 1;
        self.stats.expanded += u64::from(route.expanded);
        self.stats.worst_tick = self.stats.worst_tick.max(self.budget - self.left);
        if route.end == RouteEnd::OutOfNodes {
            self.stats.out_of_nodes += 1;
        }
        path.corridor_len = route.polygons;
        path.corridor[..route.polygons as usize].copy_from_slice(&self.scratch[..route.polygons as usize]);
        path.at = 0;
        path.searched = Searched::Ended(route.end);
        path.searched_for = target;
        // A route that stops short walks as far as it goes: to the middle of its last polygon.
        let arrives = route.end == RouteEnd::Reached && route.polygons as usize != CORRIDOR_POLYGONS;
        path.goal = if arrives {
            goal.position
        } else {
            self.graph.centre(self.scratch[route.polygons as usize - 1])
        };
        path.straighten(&self.graph, here.position);
    }
}

/// The unit direction from `from` to `to` on the ground, unless they're within [`CORNER_RADIUS`]. In
/// `*`, `+`, `/` and `sqrt`, as everything here is.
fn walk_toward(from: Vec3, to: Vec3) -> Option<Vec3> {
    let (x, z) = (to.x - from.x, to.z - from.z);
    let length = (x * x + z * z).sqrt();
    if length.is_finite() {
        return (length > CORNER_RADIUS).then(|| Vec3::new(x / length, 0.0, z / length));
    }
    let (x, z) = (f64::from(to.x) - f64::from(from.x), f64::from(to.z) - f64::from(from.z));
    let length = (x * x + z * z).sqrt();
    (length > f64::from(CORNER_RADIUS)).then(|| Vec3::new((x / length) as f32, 0.0, (z / length) as f32))
}

#[cfg(test)]
mod tests {
    use sapphire_navmesh::{NO_NEIGHBOUR, NavMesh};
    use sapphire_serialize::cooked::AlignedVec;

    use super::*;

    /// A corridor of `squares` squares a metre wide along +X, each a polygon, its cells a metre.
    fn strip(squares: u32) -> AlignedVec {
        let mut mesh =
            NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        for square in 0..squares {
            let x = square as i32;
            let first = mesh.vertices.len() as u32;
            mesh.vertices.extend([[x, 0, 0], [x, 0, 1], [x + 1, 0, 1], [x + 1, 0, 0]]);
            mesh.corners.extend([first, first + 1, first + 2, first + 3]);
            mesh.neighbours.extend([
                if square > 0 { square - 1 } else { NO_NEIGHBOUR },
                NO_NEIGHBOUR,
                if square + 1 < squares { square + 1 } else { NO_NEIGHBOUR },
                NO_NEIGHBOUR,
            ]);
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
        mesh.to_cooked().expect("it writes")
    }

    fn navigation(bytes: &AlignedVec) -> Navigation {
        Navigation::new(NavMesh::read(bytes).expect("it reads"))
    }

    fn at(x: f32) -> Vec3 {
        Vec3::new(x, 0.0, 0.5)
    }

    #[test]
    fn finite_far_endpoints_still_produce_a_unit_steering_direction() {
        assert_eq!(walk_toward(Vec3::ZERO, Vec3::new(1e20, 0.0, 0.0)), Some(Vec3::X));
        assert_eq!(
            walk_toward(Vec3::new(f32::MAX, 0.0, 0.0), Vec3::new(f32::MIN, 0.0, 0.0)),
            Some(Vec3::NEG_X)
        );
    }

    #[test]
    fn clearing_a_progressed_path_forgets_every_cursor() {
        let mut nav = navigation(&strip(3));
        let mut path = Path::new();
        nav.begin_tick();
        nav.follow(&mut path, at(0.5), at(2.5));
        nav.begin_tick();
        nav.follow(&mut path, at(1.5), at(2.5));
        assert_eq!(path.corridor(), &[1, 2]);
        for _ in 0..2 {
            path.clear();
            assert!(path.corridor().is_empty());
            assert!(path.corners().is_empty());
            assert_eq!(path.route_end(), None);
            assert!(path.wants_search(at(2.5), Some(2)));
        }
        nav.begin_tick();
        assert!(matches!(nav.follow(&mut path, at(1.5), at(2.5)), Steer::Walk(_)));
        assert_eq!(path.route_end(), Some(RouteEnd::Reached));
    }

    #[test]
    fn it_searches_once_walks_and_arrives() {
        let bytes = strip(10);
        let mut nav = navigation(&bytes);
        let mut path = Path::new();
        let target = at(8.5);
        let mut position = at(0.5);
        for _ in 0..1000 {
            nav.begin_tick();
            match nav.follow(&mut path, position, target) {
                Steer::Walk(direction) => {
                    assert!((direction.length() - 1.0).abs() < 1e-5 && direction.y == 0.0, "{direction}");
                    position += direction * 0.1;
                }
                Steer::Arrived => break,
                Steer::NoPath => panic!("it's on the mesh"),
            }
        }
        assert!(position.distance(target) <= CORNER_RADIUS, "{position}");
        let stats = nav.take_stats();
        assert_eq!(stats.searches, 1, "nothing moved, so nothing searched again");
        assert_eq!(path.route_end(), Some(RouteEnd::Reached));
    }

    #[test]
    fn a_target_moving_within_its_polygon_or_a_little_way_doesnt_search() {
        let bytes = strip(10);
        let mut nav = navigation(&bytes);
        let mut path = Path::new();
        nav.begin_tick();
        nav.follow(&mut path, at(0.5), at(8.5));
        let searches = nav.take_stats().searches;

        // Within the polygon: only the last leg bends.
        nav.begin_tick();
        nav.follow(&mut path, at(0.5), at(8.9));
        assert_eq!(nav.take_stats().searches + searches, 1);
        assert_eq!(path.corners().last().copied(), Some(at(8.9)));

        // Under a metre, onto another polygon: not yet.
        nav.begin_tick();
        nav.follow(&mut path, at(0.5), at(9.2));
        assert_eq!(nav.take_stats().searches, 0);

        // Past it: yes.
        nav.begin_tick();
        nav.follow(&mut path, at(0.5), at(9.6));
        assert_eq!(nav.take_stats().searches, 1);
        assert_eq!(path.corners().last().copied(), Some(at(9.6)));
    }

    #[test]
    fn leaving_the_corridor_searches_again() {
        let bytes = strip(10);
        let mut nav = navigation(&bytes);
        let mut path = Path::new();
        nav.begin_tick();
        nav.follow(&mut path, at(4.5), at(8.5));
        nav.take_stats();
        nav.begin_tick();
        // Knocked back, behind where its corridor starts.
        assert!(matches!(nav.follow(&mut path, at(1.5), at(8.5)), Steer::Walk(_)));
        assert_eq!(nav.take_stats().searches, 1);
        assert_eq!(path.corridor().first().copied(), Some(1));
    }

    #[test]
    fn off_the_mesh_it_has_no_path_and_a_target_off_it_keeps_the_route_it_has() {
        let bytes = strip(10);
        let mut nav = navigation(&bytes);
        let mut path = Path::new();
        nav.begin_tick();
        assert_eq!(nav.follow(&mut path, at(50.0), at(8.5)), Steer::NoPath);
        assert_eq!(nav.follow(&mut path, at(0.5), at(50.0)), Steer::NoPath, "nothing to search for yet");
        assert!(matches!(nav.follow(&mut path, at(0.5), at(8.5)), Steer::Walk(_)));
        nav.take_stats();
        assert!(matches!(nav.follow(&mut path, at(1.5), at(50.0)), Steer::Walk(_)), "the target jumped");
        assert_eq!(nav.take_stats().searches, 0);
    }

    /// Nothing leads from one strip to the other: it asks once, walks as near as it can, and stands.
    #[test]
    fn with_no_way_there_it_searches_once_and_goes_as_near_as_it_can() {
        let mut mesh =
            NavMesh { cell_size: 1.0, cell_height: 1.0, polygon_starts: vec![0], ..NavMesh::default() };
        for x in [0, 2] {
            let first = mesh.vertices.len() as u32;
            mesh.vertices.extend([[x, 0, 0], [x, 0, 1], [x + 1, 0, 1], [x + 1, 0, 0]]);
            mesh.corners.extend([first, first + 1, first + 2, first + 3]);
            mesh.neighbours.extend([NO_NEIGHBOUR; 4]);
            mesh.polygon_starts.push(mesh.corners.len() as u32);
        }
        let bytes = mesh.to_cooked().expect("it writes");
        let mut nav = navigation(&bytes);
        let mut path = Path::new();
        let mut position = at(0.1);
        for _ in 0..100 {
            nav.begin_tick();
            match nav.follow(&mut path, position, at(2.5)) {
                Steer::Walk(direction) => position += direction * 0.1,
                Steer::Arrived => break,
                Steer::NoPath => panic!("it's on the mesh"),
            }
        }
        assert_eq!(path.route_end(), Some(RouteEnd::Unreachable));
        assert_eq!(nav.take_stats().searches, 1, "the whole island once, and not every tick");
    }

    /// Who gets the budget, and who waits, and that nobody waits for long.
    #[test]
    fn the_budget_holds_across_a_tick_and_the_last_in_line_get_their_turn() {
        let bytes = strip(100);
        let mesh = NavMesh::read(&bytes).expect("it reads");
        let mut nav = Navigation::with_budget(mesh, 200);
        let mut paths: Vec<Path> = (0..8).map(|_| Path::new()).collect();
        let from = at(0.5);
        let target = at(90.5);

        nav.begin_tick();
        let first: Vec<Steer> = paths.iter_mut().map(|path| nav.follow(path, from, target)).collect();
        // Each search spends 90: two go, and the third hasn't enough to start on.
        assert!(matches!(first[0], Steer::Walk(_)) && matches!(first[1], Steer::Walk(_)), "{first:?}");
        assert!(first[3..].iter().all(|steer| *steer == Steer::NoPath), "{first:?}");
        let stats = nav.take_stats();
        assert!(stats.deferred >= 5, "{stats:?}");
        assert!(stats.worst_tick <= 200, "{stats:?}");

        let mut ticks = 1;
        while paths.iter().any(|path| path.route_end().is_none()) {
            nav.begin_tick();
            for path in &mut paths {
                nav.follow(path, from, target);
            }
            assert!(nav.take_stats().worst_tick <= 200);
            ticks += 1;
            assert!(ticks < 8, "the last in line waits only for those ahead of it");
        }
    }

    /// A search short of nodes walks what it found and asks again, and gets there.
    #[test]
    fn a_route_longer_than_a_search_can_afford_is_walked_in_stages() {
        let bytes = strip(100);
        let mesh = NavMesh::read(&bytes).expect("it reads");
        let mut nav = Navigation::with_budget(mesh, 16);
        let mut path = Path::new();
        let mut position = at(0.5);
        let target = at(90.5);
        for _ in 0..10_000 {
            nav.begin_tick();
            match nav.follow(&mut path, position, target) {
                Steer::Walk(direction) => position += direction * 0.25,
                Steer::Arrived if path.route_end() == Some(RouteEnd::Reached) => break,
                other => panic!("{other:?} at {position}, {:?}", path.route_end()),
            }
        }
        assert!(position.distance(target) <= CORNER_RADIUS, "{position}");
        let stats = nav.take_stats();
        assert!(stats.out_of_nodes > 1 && stats.worst_tick <= 16, "{stats:?}");
    }

    #[test]
    fn a_corridor_longer_than_a_path_holds_is_walked_and_searched_again() {
        let bytes = strip(600);
        let mesh = NavMesh::read(&bytes).expect("it reads");
        let mut nav = Navigation::with_budget(mesh, 4096);
        let mut path = Path::new();
        let mut position = at(0.5);
        let target = at(590.5);
        for _ in 0..10_000 {
            nav.begin_tick();
            match nav.follow(&mut path, position, target) {
                Steer::Walk(direction) => position += direction * 0.5,
                Steer::Arrived if position.distance(target) <= CORNER_RADIUS => break,
                other => panic!("{other:?} at {position}"),
            }
        }
        assert!(position.distance(target) <= CORNER_RADIUS, "{position}");
        assert!(nav.take_stats().searches >= 3);
    }

    /// Every field a path decides by is one a checksum reads.
    #[test]
    fn its_state_says_what_it_decides_by() {
        let bytes = strip(10);
        let mut nav = navigation(&bytes);
        let mut path = Path::new();
        nav.begin_tick();
        nav.follow(&mut path, at(0.5), at(8.5));
        let state = |path: &Path| {
            let mut bytes = Vec::new();
            path.write_state(&mut |more| bytes.extend_from_slice(more));
            bytes
        };
        let held = state(&path);
        assert_eq!(held, state(&path.clone()));
        let changes: [fn(&mut Path); 10] = [
            |path| path.corridor[0] ^= 1,
            |path| path.corridor_len -= 1,
            |path| path.at += 1,
            |path| path.corners[0].x += 1.0,
            |path| path.corner_count -= 1,
            |path| path.next_corner += 1,
            |path| path.whole = !path.whole,
            |path| path.goal.x += 1.0,
            |path| path.searched_for.x += 1.0,
            |path| path.searched = Searched::Ended(RouteEnd::Unreachable),
        ];
        for (index, change) in changes.iter().enumerate() {
            let mut changed = path.clone();
            change(&mut changed);
            assert_ne!(state(&changed), held, "change {index}");
        }
    }
}
