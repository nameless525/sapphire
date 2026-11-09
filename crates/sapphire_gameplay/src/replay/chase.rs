//! The navigation gates (M3 P2, P3, spec §9's gates 2 and 4), each on the arena's real navmesh, through the
//! same stick anyone has and the real controller, by what [`crate::brain`] decides.
//!
//! - **`nav-chase`:** a player and 64 brains after them. A replay of it checks a real bake, the search,
//!   the corridor, the funnel, the budget and who waits for it, a straight line and a crowd, on every
//!   architecture, at no allocation in any step.
//! - **`nav-patrol`:** a player and two real warriors, who walk the document's patrols, notice them, chase,
//!   and go back once they've lost them. Which adds who notices whom, walking a patrol and giving up.

use sapphire_core::Name;
use sapphire_math::Vec3;
use sapphire_moves::{EnemyDocument, EnemyMind, MoveSet, MoveSetDocument};
use sapphire_nav::NavGraph;
use sapphire_physics::{ColliderSet, PhysicsWorld};

use super::scenario::{
    CHASER_ENEMY, CHASER_MOVES, PLAYERS, ScenarioError, ULTIMATE_CINEMATIC, WARRIORS, add_warrior,
    player_moves, test_moves, ultimate_cinematic,
};
use crate::actors::{ActorId, ActorSpawn};
use crate::simulation::Simulation;

/// How many chase the player: the most P0 measured.
pub const CHASERS: usize = 64;

/// How many walk the arena's patrols, one on each.
pub(crate) const GUARDS: usize = 2;

/// How far off the ground's height a polygon's middle may be to start someone on: within a row of the
/// navmesh's. Its height is the bake's, and a character stands on the paving.
const FLAT: f32 = 0.05;

/// Where the chasers start: on the middles of polygons at ground level, spread through the mesh in its
/// order, so some have the length of the square to come. In `+ / *` over a mesh whose bytes are committed.
fn spots(graph: &NavGraph, count: usize) -> Vec<Vec3> {
    let flat: Vec<Vec3> = (0..graph.polygon_count() as u32)
        .map(|polygon| graph.centre(polygon))
        .filter(|centre| centre.y.abs() <= FLAT)
        .collect();
    (0..count)
        .map(|index| flat[index * flat.len() / count])
        .map(|spot| Vec3::new(spot.x, 0.0, spot.z))
        .collect()
}

/// The chasers' documents, as a set and the kind of enemy on it (M3 B3).
fn chaser() -> Result<(MoveSet, EnemyMind), ScenarioError> {
    let moves = MoveSetDocument::parse(CHASER_MOVES)?;
    let mind = EnemyDocument::parse(CHASER_ENEMY)?.compile(&moves)?.mind;
    Ok((moves.compile()?, mind))
}

/// The `nav-chase` world, with `moves` and their kind's `mind` for the chasers: the player, who's the one input drives, and the
/// chasers, each with a brain, in ID order. `in_reverse` starts them on each other's spots, for a test of
/// what depends on who's first.
pub fn chase_world(
    sim: &mut Simulation,
    (moves, mind): (MoveSet, EnemyMind),
    in_reverse: bool,
) -> Result<(ActorId, Vec<ActorId>), ScenarioError> {
    let colliders = crate::arena::cooked_colliders()?;
    let set = ColliderSet::read(&colliders)?;
    sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
    sim.set_navigation(&crate::arena::cooked_navmesh())?;
    let controller = Some(sim.add_controller_profile(&crate::arena::profile())?);
    let player_set = Some(sim.add_move_set(test_moves()?));
    let chaser_set = sim.add_move_set(moves);
    sim.add_enemy(chaser_set, mind)?;
    let chaser_set = Some(chaser_set);

    let start = crate::arena::spawns().player;
    let player = sim.spawn(ActorSpawn {
        position: start.position(),
        yaw: start.yaw(),
        moves: player_set,
        controller,
        faction: PLAYERS,
    })?;
    let mut spots = spots(sim.navigation().expect("just given").navigation().graph(), CHASERS);
    if in_reverse {
        spots.reverse();
    }
    let mut chasers = Vec::with_capacity(CHASERS);
    for position in spots {
        let chaser =
            sim.spawn(ActorSpawn { position, yaw: 0.0, moves: chaser_set, controller, faction: WARRIORS })?;
        sim.give_brain(chaser);
        chasers.push(chaser);
    }
    Ok((player, chasers))
}

/// The `nav-chase` scenario's setup: the cast is the player alone, since the chasers think for themselves.
pub(super) fn nav_chase(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    Ok(vec![chase_world(sim, chaser()?, false)?.0])
}

/// The `nav-patrol` scenario's setup: the cast is the player, then the guards, each on the first point of
/// one of the document's patrols. A patrol here is whatever the document says: this names no point.
pub(super) fn nav_patrol(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    let colliders = crate::arena::cooked_colliders()?;
    let set = ColliderSet::read(&colliders)?;
    sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
    sim.set_navigation(&crate::arena::cooked_navmesh())?;
    let controller = Some(sim.add_controller_profile(&crate::arena::profile())?);
    // The player's moves name their ultimate's cinematic, which loads before them, as `fight_on` does.
    sim.add_cinematic(ULTIMATE_CINEMATIC, ultimate_cinematic()?)?;
    let player_set = Some(sim.add_move_set(player_moves()?));
    let warrior_set = Some(add_warrior(sim)?);

    let spawns = crate::arena::spawns();
    let player = sim.spawn(ActorSpawn {
        position: spawns.player.position(),
        yaw: spawns.player.yaw(),
        moves: player_set,
        controller,
        faction: PLAYERS,
    })?;
    sim.give_soft_lock(player);
    let mut cast = vec![player];
    for patrol in spawns.patrols.iter().take(GUARDS) {
        let walk = sim.add_patrol(Name::intern(&patrol.name), &patrol.points, patrol.closed)?;
        let guard = sim.spawn(ActorSpawn {
            position: Vec3::from_array(patrol.points[0]),
            yaw: 0.0,
            moves: warrior_set,
            controller,
            faction: WARRIORS,
        })?;
        sim.give_brain(guard);
        sim.give_patrol(guard, walk);
        cast.push(guard);
    }
    Ok(cast)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use sapphire_nav::NODES_PER_TICK;

    use super::super::{SCENARIOS, find_scenario, find_script};
    use super::*;
    use crate::actions::TickActions;
    use crate::brain::BrainState;

    fn start(chaser: (MoveSet, EnemyMind), in_reverse: bool) -> (Simulation, ActorId, Vec<ActorId>) {
        let scenario = find_scenario(&SCENARIOS, "nav-chase").expect("listed");
        let mut sim = Simulation::new(scenario.config);
        let (player, chasers) = chase_world(&mut sim, chaser, in_reverse).expect("it sets up");
        (sim, player, chasers)
    }

    /// What a crowd came to, round whoever it was after.
    #[derive(Debug)]
    struct Spread {
        /// Of 16 bearings round the target, how many someone stands in.
        bearings: usize,
        /// The nearest any two stand, in metres.
        closest: f32,
        /// How far off the furthest of them is, in metres.
        furthest: f32,
        /// How far the crowd is from having arrived: the mean of how far off they stand.
        mean: f32,
        /// How many have a neighbour within a capsule's width of touching.
        touching: usize,
    }

    /// Metres across the ground.
    fn ground(a: Vec3, b: Vec3) -> f32 {
        let (x, z) = (a.x - b.x, a.z - b.z);
        (x * x + z * z).sqrt()
    }

    fn spread(sim: &Simulation, player: ActorId, chasers: &[ActorId]) -> Spread {
        let centre = sim.actor(player).expect("alive").position;
        let at: Vec<Vec3> =
            chasers.iter().map(|chaser| sim.actor(*chaser).expect("alive").position).collect();
        let mut bearings = [false; 16];
        let (mut closest, mut furthest, mut total) = (f32::MAX, 0.0_f32, 0.0);
        let mut touching = 0;
        for (index, place) in at.iter().enumerate() {
            let (x, z) = (place.x - centre.x, place.z - centre.z);
            let metres = (x * x + z * z).sqrt();
            furthest = furthest.max(metres);
            total += metres;
            let bearing = sapphire_math::det::atan2(x, z) / std::f32::consts::TAU + 0.5;
            bearings[((bearing * 16.0) as usize).min(15)] = true;
            let nearest = at
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, other)| ground(*place, *other))
                .fold(f32::MAX, f32::min);
            closest = closest.min(nearest);
            touching += usize::from(nearest < 0.65);
        }
        Spread {
            bearings: bearings.iter().filter(|seen| **seen).count(),
            closest,
            furthest,
            mean: total / at.len() as f32,
            touching,
        }
    }

    /// Runs `chase-laps` to its end. Returns what it came to, and how many searches it took.
    fn chase(sim: &mut Simulation, player: ActorId, chasers: &[ActorId]) -> (Spread, u32) {
        let script = find_script("chase-laps").expect("listed");
        let cast = [player];
        let mut searches = 0;
        for tick in 0..u64::from(script.ticks) {
            sim.step(&(script.actions)(tick, &cast));
            let stats = sim.navigation_mut().expect("it has one").take_stats();
            assert!(stats.worst_tick <= NODES_PER_TICK, "tick {tick}: {stats:?}");
            searches += stats.searches;
        }
        (spread(sim, player, chasers), searches)
    }

    fn chasers_apart(apart: f32) -> (MoveSet, EnemyMind) {
        let moves = MoveSetDocument::parse(CHASER_MOVES).expect("it parses");
        let mut kind = EnemyDocument::parse(CHASER_ENEMY).expect("it parses");
        kind.brains.values_mut().for_each(|brain| brain.apart = apart);
        let mind = kind.compile(&moves).expect("it compiles").mind;
        (moves.compile().expect("it compiles"), mind)
    }

    /// What P3 is for: the old queue, with `apart` at 0, against a crowd, on one real run.
    #[test]
    fn a_crowd_spreads_round_who_it_chases_and_doesnt_queue() {
        let (mut sim, player, chasers) = start(chasers_apart(0.0), false);
        let (queue, _) = chase(&mut sim, player, &chasers);
        let (mut sim, player, chasers) = start(chasers_apart(1.2), false);
        let (crowd, searches) = chase(&mut sim, player, &chasers);
        let mut out = std::io::stdout();
        writeln!(out, "NAV-CROWD queue {queue:?}").expect("it prints");
        writeln!(out, "NAV-CROWD crowd {crowd:?}, {searches} searches").expect("it prints");
        assert!(crowd.bearings > queue.bearings, "it's round them, not behind them: {crowd:?} {queue:?}");
        assert!(crowd.bearings >= 12, "{crowd:?}");
        assert!(crowd.closest > queue.closest, "{crowd:?} {queue:?}");
        assert!(crowd.closest > 0.6, "capsules 0.3 m in radius don't touch: {crowd:?}");
        assert_eq!(crowd.touching, 0, "{crowd:?}");
        assert!(queue.touching > 16, "with nothing keeping them apart they do: {queue:?}");
        // 64 people at 1.2 m apart at most fill a circle about this wide.
        assert!(crowd.furthest < 8.5, "and it's a crowd round them, not a scatter: {crowd:?}");

        let follows = find_script("chase-laps").expect("listed").ticks * CHASERS as u32;
        assert!(searches * 10 < follows, "{searches} searches in {follows} asks: a path should last");
    }

    /// Who's first in the list isn't what makes the crowd. The chasers' spots dealt in the other order
    /// come to the same measures, near enough for it to be the same crowd. The sum's bits and who gets the
    /// budget do depend on the order, as they're allowed to.
    #[test]
    fn a_crowd_comes_to_the_same_in_either_order() {
        let (mut sim, player, chasers) = start(chasers_apart(1.2), true);
        let (reversed, _) = chase(&mut sim, player, &chasers);
        let (mut sim, player, chasers) = start(chasers_apart(1.2), false);
        let (forward, _) = chase(&mut sim, player, &chasers);
        assert!(reversed.closest > 0.6, "{reversed:?}");
        assert!(reversed.bearings.abs_diff(forward.bearings) <= 2, "{reversed:?} {forward:?}");
        assert!((reversed.mean - forward.mean).abs() < 0.5, "{reversed:?} {forward:?}");
    }

    /// A crowd that has got there stands. Nothing goes on walking round for ever, as a rule that always
    /// stepped one way would have it.
    #[test]
    fn a_crowd_that_has_arrived_stands_still() {
        let (mut sim, player, chasers) = start(chasers_apart(1.2), false);
        chase(&mut sim, player, &chasers);
        let places: Vec<Vec3> = chasers.iter().map(|id| sim.actor(*id).expect("alive").position).collect();
        for _ in 0..60 {
            sim.step(&TickActions::NEUTRAL);
        }
        let moved: f32 = chasers
            .iter()
            .zip(&places)
            .map(|(id, was)| sim.actor(*id).expect("alive").position.distance(*was))
            .sum();
        assert!(moved / (CHASERS as f32) < 0.3, "the mean of how far they went in a second: {moved}");
    }

    /// A replay tells one navmesh from another, and a simulation with none hashes what it always did.
    #[test]
    fn the_tables_hash_holds_the_navmesh_and_only_if_there_is_one() {
        let scenario = find_scenario(&SCENARIOS, "arena").expect("listed");
        let (mut sim, _) = scenario.start(scenario.config).expect("it sets up");
        let without = sim.tables_hash();
        let mut bytes = crate::arena::cooked_navmesh();
        sim.set_navigation(&bytes).expect("it reads");
        let with = sim.tables_hash();
        assert_ne!(without, with);
        // The bytes are checked as well as hashed, so change one no check minds: a vertex's height.
        let mesh = sapphire_navmesh::NavMesh::read(&bytes).expect("it reads");
        let vertex = mesh.vertices.as_ptr() as usize - bytes.as_ptr() as usize;
        bytes[vertex + 4] ^= 1;
        sim.set_navigation(&bytes).expect("it reads");
        assert_ne!(sim.tables_hash(), with);
    }

    /// A path holds what comes next, so the checksum reads it: it moves with one forgotten, before any
    /// character has moved differently.
    #[test]
    fn the_checksum_reads_every_path() {
        let (mut sim, _, chasers) = start(chasers_apart(1.2), false);
        for _ in 0..8 {
            sim.step(&TickActions::NEUTRAL);
        }
        let walker = *chasers
            .iter()
            .find(|chaser| sim.path_of(**chaser).is_some_and(|path| path.route_end().is_some()))
            .expect("someone has a wall in the way");
        let before = sim.checksum();
        let world = sim.ecs_mut().world_mut();
        let entity = world.resource::<crate::actors::ActorTable>().entity(walker).expect("alive");
        world.get_mut::<crate::navigation::WalkPath>(entity).expect("it has one").0.clear();
        assert_ne!(sim.checksum(), before);
    }

    /// Everyone starts on the ground, clear of each other and of the walls: the replay checks what
    /// follows, not what a bad start did.
    #[test]
    fn the_chasers_start_apart_and_on_the_ground() {
        let (mut sim, _, chasers) = start(chasers_apart(1.2), false);
        sim.step(&TickActions::NEUTRAL);
        let mut seen: Vec<Vec3> = Vec::new();
        for chaser in chasers {
            let view = sim.actor(chaser).expect("alive");
            assert_eq!(view.controller.state.mode, crate::controller::GroundMode::Grounded);
            assert!(view.position.y.abs() < 1e-3, "{}", view.position);
            assert!(seen.iter().all(|other| other.distance(view.position) > 0.6), "{}", view.position);
            seen.push(view.position);
        }
    }

    /// The whole of `nav-patrol`: a guard walking its patrol, then after the player when they come near,
    /// and back to walking it once they've run off past what it'll chase.
    #[test]
    fn a_guard_walks_notices_chases_and_goes_back() {
        let scenario = find_scenario(&SCENARIOS, "nav-patrol").expect("listed");
        let (mut sim, cast) = scenario.start(scenario.config).expect("it sets up");
        let script = find_script("patrol-pass").expect("listed");
        let guards = &cast[1..];
        assert_eq!(guards.len(), GUARDS);
        let mut noticed = [false; GUARDS];
        let mut gave_up = [false; GUARDS];
        let mut points = [[false; 4]; GUARDS];
        for tick in 0..u64::from(script.ticks) {
            sim.step(&(script.actions)(tick, &cast));
            for (index, guard) in guards.iter().enumerate() {
                let brain = sim.actor(*guard).expect("alive").brain;
                if brain.target().is_some() {
                    noticed[index] = true;
                }
                if noticed[index] && brain.state() == BrainState::Watching && brain.target().is_none() {
                    gave_up[index] = true;
                }
                if let Some(walk) = brain.patrol() {
                    points[index][usize::from(walk.next()).min(3)] = true;
                }
            }
        }
        assert!(noticed.iter().any(|did| *did), "someone notices the player");
        assert!(gave_up.iter().any(|did| *did), "and someone gives up: {noticed:?} {gave_up:?}");
        for (index, seen) in points.iter().enumerate() {
            assert!(seen.iter().filter(|seen| **seen).count() >= 2, "guard {index} walks on: {seen:?}");
        }
        assert!(!sim.actor(cast[0]).expect("alive").fighter.is_defeated(), "the player gets away");
    }
}
