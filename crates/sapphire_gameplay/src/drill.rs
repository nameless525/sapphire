//! The drill: the smallest set of behaviors that exercises the simulation until M2's G packages
//! replace it.
//!
//! This isn't a controller or a move system. It turns button presses into the things those will
//! do, so determinism, pools and clocks have something real to hold: [`Buttons::LIGHT`] follows the
//! actor's table, [`Buttons::HEAVY`] freezes it, [`Buttons::DODGE`] slows it,
//! [`Buttons::ULTIMATE`] slows the world, [`Buttons::SKILL`] spawns a transient actor at a random
//! yaw, and the stick walks an actor that isn't following a table.

use std::f32::consts::TAU;

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_time::ScaleSource;

use crate::actions::{Buttons, Controls};
use crate::actors::{ActorRequests, ActorSpawn, ActorTable};
use crate::motion::{ActorTransform, RootMotion, TableId};
use crate::simulation::SimRng;

/// Ticks a [`Buttons::HEAVY`] press freezes its own actor's clock.
pub const DRILL_HITSTOP_TICKS: u16 = 6;

/// The actor's speed, in per-mille, that [`Buttons::DODGE`] toggles.
pub const DRILL_LOCAL_SCALE: u32 = 500;

/// The world's speed, in per-mille, that [`Buttons::ULTIMATE`] toggles.
pub const DRILL_WORLD_SCALE: u32 = 200;

/// Metres per second of the actor's own time at a full stick.
pub const DRILL_WALK_SPEED: f32 = 3.0;

/// How an actor takes part in the drill.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Drill {
    /// The table a press follows, and what a spawned actor follows.
    pub(crate) table: Option<TableId>,
    /// Follows `table` from its first step, and despawns at its end.
    pub(crate) transient: bool,
}

/// [`crate::CombatSet::Intents`]: this tick's presses, in ID order, so every actor gets the same
/// random numbers on every run.
pub(crate) fn apply_presses(
    table: Res<'_, ActorTable>,
    mut requests: ResMut<'_, ActorRequests>,
    mut rng: ResMut<'_, SimRng>,
    mut world_clock: ResMut<'_, WorldClock>,
    mut actors: Query<'_, '_, (&Controls, &mut EntityClock, &mut RootMotion, &ActorTransform, &Drill)>,
) {
    for (_, entity) in table.iter() {
        let Ok((controls, mut clock, mut motion, transform, drill)) = actors.get_mut(entity) else {
            continue;
        };
        let pressed = controls.pressed();
        if pressed.is_empty() {
            continue;
        }
        if pressed.contains(Buttons::LIGHT)
            && let Some(table) = drill.table
        {
            let now = clock.0.now();
            motion.start(table, now, transform.yaw);
        }
        if pressed.contains(Buttons::HEAVY) {
            clock.0.apply_hitstop(DRILL_HITSTOP_TICKS);
        }
        if pressed.contains(Buttons::DODGE) && clock.0.remove_source(ScaleSource::Gameplay) == 0 {
            // Toggled, so the stack never fills: there's nothing to do with the error.
            let _ = clock.0.push_scale(ScaleSource::Gameplay, DRILL_LOCAL_SCALE);
        }
        if pressed.contains(Buttons::ULTIMATE) && world_clock.0.remove_source(ScaleSource::PerfectDodge) == 0
        {
            let _ = world_clock.0.push_scale(ScaleSource::PerfectDodge, DRILL_WORLD_SCALE);
        }
        if pressed.contains(Buttons::SKILL) {
            let yaw = rng.0.unit_f32() * TAU;
            // A full pool refuses and counts it; the drill has nothing else to do about it.
            let _ = requests.spawn(ActorSpawn {
                position: transform.position,
                yaw,
                table: drill.table,
                transient: true,
            });
        }
    }
}

/// [`crate::CombatSet::Movement`], before root motion: the stick moves an actor not following a
/// table, by its own clock's time, in world axes. A real controller lands in M2 G1.
pub(crate) fn walk(
    table: Res<'_, ActorTable>,
    mut actors: Query<'_, '_, (&Controls, &EntityClock, &RootMotion, &mut ActorTransform)>,
) {
    for (_, entity) in table.iter() {
        let Ok((controls, clock, motion, mut transform)) = actors.get_mut(entity) else { continue };
        if motion.table().is_some() {
            continue;
        }
        let stick = controls.current.stick.to_vec2();
        let seconds = clock.0.last_delta().as_secs_f32();
        transform.position += Vec3::new(stick.x, 0.0, stick.y) * (DRILL_WALK_SPEED * seconds);
    }
}
