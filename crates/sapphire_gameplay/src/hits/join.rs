//! The systems that join moves to hit queries: an actor's hurtboxes in
//! [`CombatSet::Movement`](crate::CombatSet::Movement), and its volumes in
//! [`CombatSet::Hits`](crate::CombatSet::Hits).

use sapphire_ecs::prelude::*;
use sapphire_moves::MoveSet;

use super::HitQuery;
use super::hurtbox::HurtboxId;
use super::published::{PublishedMoves, hit_shape, published_hurtbox};
use super::set_dodge_volume;
use crate::actors::ActorTable;
use crate::controller::CharacterWorld;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveOutput, MoveRunner};
use crate::party::Party;
use crate::resolve::{Fighter, ResolveStats};

/// [`CombatSet::Movement`](crate::CombatSet::Movement), after every actor has moved: makes each
/// fighter's hurtboxes what its moves say they are this tick, where its root now stands. A defeated
/// actor's leave the world. Each actor's list is its set's current revision's, in order, so a hurtbox's
/// number is its place there. A perfect-dodge window open this step ([`MoveOutput::perfect_window`])
/// puts its volume in the world too, and only then, so nothing is spent on a dodge that can't fire.
pub(crate) fn place_hurtboxes(
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    mut characters: ResMut<'_, CharacterWorld>,
    mut stats: ResMut<'_, ResolveStats>,
    party: Res<'_, Party>,
    actors: Query<'_, '_, (&MoveRunner, &MoveOutput, &ActorTransform, &Fighter)>,
) {
    let physics = characters.physics_mut();
    // Off the field an actor has no bodies: leaving took them out, and nothing here puts them back.
    for (id, entity) in table.iter().filter(|&(id, _)| !party.is_off_field(id)) {
        let Ok((runner, output, root, fighter)) = actors.get(entity) else { continue };
        let Some(def) = runner.set().and_then(|set| library.current(set)).and_then(MoveSet::fighter) else {
            continue;
        };
        for (index, own) in def.hurtboxes.iter().enumerate() {
            let shape = (!fighter.is_defeated())
                .then(|| published_hurtbox(own.name(), hit_shape(own.shape), output))
                .flatten();
            // A set holds at most as many as an ID has room for, and only a run's 2²⁷th actor has none.
            let placed = HurtboxId::new(id, index as u8)
                .is_some_and(|hurtbox| hurtbox.set(physics, shape, *root, own.local).is_ok());
            stats.hurtboxes_refused += u64::from(!placed);
        }
        let volume = output.perfect_window.filter(|_| !fighter.is_defeated()).and_then(|window| {
            let at = runner.running?.at;
            let volume = library.get(at)?.perfect_dodge.get(window as usize)?.volume?;
            Some((hit_shape(volume.shape), volume.local))
        });
        let placed = set_dodge_volume(physics, id, volume, *root).is_ok();
        stats.hurtboxes_refused += u64::from(!placed);
    }
}

/// [`CombatSet::Hits`](crate::CombatSet::Hits): sweeps everything each actor's moves published, in ID
/// order, and finishes the step, with or without anything swept. A defeated actor's volumes stop with
/// it.
pub(crate) fn sweep_hits(
    mut hits: ResMut<'_, HitQuery>,
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    characters: Res<'_, CharacterWorld>,
    party: Res<'_, Party>,
    actors: Query<'_, '_, (&EntityClock, &MoveRunner, &MoveOutput, &ActorTransform, &Fighter)>,
) {
    for (attacker, entity) in table.iter().filter(|&(attacker, _)| !party.is_off_field(attacker)) {
        let Ok((clock, runner, output, root, fighter)) = actors.get(entity) else { continue };
        if fighter.is_defeated() {
            continue;
        }
        let moves = PublishedMoves { attacker, delta: clock.0.last_delta(), runner, output, root: *root };
        hits.sweep_published(characters.physics(), &library, &moves);
    }
    hits.finish_step();
}
