//! The navigation gate (M3 P2, spec §9's gates 2 and 4): `nav-chase`, a player and 64 chasers on the
//! arena's real navmesh, steering by what [`SimNavigation::follow`] says through the same stick anyone
//! has, in the real controller.
//!
//! So a replay of it checks a real bake, the search, the corridor, the funnel, the budget and who waits
//! for it, on every architecture, at no allocation in any step. They run into each other, as nothing keeps
//! them apart yet: that's P3, as are the brains that would do this for a reason.

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_nav::{NavGraph, Steer};
use sapphire_physics::{ColliderSet, PhysicsWorld};

use super::scenario::{ScenarioError, test_moves};
use crate::actions::{Controls, Stick};
use crate::actors::{ActorId, ActorSpawn, ActorTable};
use crate::movement::ActorTransform;
use crate::navigation::{PathId, SimNavigation};
use crate::simulation::{CombatSet, Simulation};

/// How many chase the player: the most P0 measured.
pub(super) const CHASERS: usize = 64;

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

/// The `nav-chase` world: the player, who's the one input drives, and the chasers and their paths, in ID
/// order.
pub(super) fn set_up(sim: &mut Simulation) -> Result<(ActorId, Vec<(ActorId, PathId)>), ScenarioError> {
    let colliders = crate::arena::cooked_colliders()?;
    let set = ColliderSet::read(&colliders)?;
    sim.set_physics_world(PhysicsWorld::from_cooked(set, sim.body_capacity())?)?;
    sim.set_navigation(&crate::arena::cooked_navmesh())?;
    let controller = Some(sim.add_controller_profile(&crate::arena::profile())?);
    let moves = Some(sim.add_move_set(test_moves()?));

    let start = crate::arena::spawns().player;
    let player = sim.spawn(ActorSpawn {
        position: start.position(),
        yaw: start.yaw(),
        moves,
        controller,
        ..ActorSpawn::default()
    })?;
    let mut navigation = sim.navigation_mut().expect("just given");
    let spots = spots(navigation.navigation().graph(), CHASERS);
    let paths: Vec<PathId> = spots.iter().map(|_| navigation.add_path()).collect();
    let mut chasers = Vec::with_capacity(CHASERS);
    for position in spots {
        chasers.push(sim.spawn(ActorSpawn {
            position,
            yaw: 0.0,
            moves,
            controller,
            ..ActorSpawn::default()
        })?);
    }

    let followers: Vec<(ActorId, PathId)> = chasers.into_iter().zip(paths).collect();
    let chasing = followers.clone();
    sim.add_combat_system(
        CombatSet::Intents,
        (move |table: Res<'_, ActorTable>,
               mut navigation: ResMut<'_, SimNavigation>,
               mut actors: Query<'_, '_, (&ActorTransform, &mut Controls)>| {
            let seen = table.entity(player).and_then(|entity| actors.get(entity).ok());
            let Some(target) = seen.map(|(transform, _)| transform.position) else { return };
            navigation.begin_tick();
            for &(actor, path) in &chasing {
                let Some(entity) = table.entity(actor) else { continue };
                let Ok((transform, mut controls)) = actors.get_mut(entity) else { continue };
                controls.current.stick = match navigation.follow(path, transform.position, target) {
                    Steer::Walk(direction) => Stick::from_axes(direction.x, direction.z),
                    Steer::Arrived | Steer::NoPath => Stick::CENTER,
                };
            }
        })
        .after(crate::brain::think),
    );
    Ok((player, followers))
}

/// The scenario's setup: the cast is the player alone, since the chasers' sticks are the system's.
pub(super) fn nav_chase(sim: &mut Simulation) -> Result<Vec<ActorId>, ScenarioError> {
    Ok(vec![set_up(sim)?.0])
}

#[cfg(test)]
mod tests {
    use sapphire_nav::{NODES_PER_TICK, RouteEnd};

    use super::super::{SCENARIOS, find_scenario, find_script};
    use super::*;
    use crate::actions::TickActions;

    fn start() -> (Simulation, ActorId, Vec<(ActorId, PathId)>) {
        let scenario = find_scenario(&SCENARIOS, "nav-chase").expect("listed");
        let mut sim = Simulation::new(scenario.config);
        let (player, chasers) = set_up(&mut sim).expect("it sets up");
        (sim, player, chasers)
    }

    /// What the replay is for: everyone gets where they're going, on the budget, with no search at all
    /// on most ticks. The player runs laps round the square and stands, and within the script they've
    /// all arrived, in a queue, since nothing keeps them apart yet and the front of it stops the rest.
    #[test]
    fn the_chasers_catch_a_player_who_stops() {
        let (mut sim, player, chasers) = start();
        let script = find_script("chase-laps").expect("listed");
        let cast = [player];
        let mut searches = 0;
        let mut deferred_at_first = 0;
        for tick in 0..u64::from(script.ticks) {
            sim.step(&(script.actions)(tick, &cast));
            let stats = sim.navigation_mut().expect("it has one").take_stats();
            assert!(stats.worst_tick <= NODES_PER_TICK, "tick {tick}: {stats:?}");
            searches += stats.searches;
            if tick < 4 {
                deferred_at_first += stats.deferred;
            }
        }
        let end = sim.actor(player).expect("alive").position;
        let start = crate::arena::spawns().player.position();
        assert!(start.distance(end) > 4.0, "the player went somewhere: {end}");
        for (index, (chaser, path)) in chasers.iter().enumerate() {
            let at = sim.actor(*chaser).expect("alive").position;
            let path = sim.navigation().expect("it has one").path(*path);
            assert_eq!(path.route_end(), Some(RouteEnd::Reached), "chaser {index} at {at}");
            assert!(at.distance(end) < 8.0, "chaser {index} stands at {at}, and the player at {end}");
        }
        assert!(deferred_at_first > 0, "64 first searches don't fit a tick, or the budget isn't tested");
        let follows = script.ticks * CHASERS as u32;
        assert!(searches * 10 < follows, "{searches} searches in {follows} follows: a path should last");
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
        let (mut sim, _, chasers) = start();
        for _ in 0..8 {
            sim.step(&TickActions::NEUTRAL);
        }
        let before = sim.checksum();
        sim.navigation_mut().expect("it has one").clear_path(chasers[63].1);
        assert_ne!(sim.checksum(), before);
    }

    /// Everyone starts on the ground, clear of each other and of the walls: the replay checks what
    /// follows, not what a bad start did.
    #[test]
    fn the_chasers_start_apart_and_on_the_ground() {
        let (mut sim, _, chasers) = start();
        sim.step(&TickActions::NEUTRAL);
        let mut seen: Vec<Vec3> = Vec::new();
        for (chaser, _) in chasers {
            let view = sim.actor(chaser).expect("alive");
            assert_eq!(view.controller.state.mode, crate::controller::GroundMode::Grounded);
            assert!(view.position.y.abs() < 1e-3, "{}", view.position);
            assert!(seen.iter().all(|other| other.distance(view.position) > 0.6), "{}", view.position);
            seen.push(view.position);
        }
    }
}
