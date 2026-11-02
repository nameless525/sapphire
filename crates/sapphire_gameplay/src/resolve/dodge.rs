//! [`fire_perfect_dodges`]: what a swing through an open perfect-dodge window does. See the
//! [module docs](super#a-perfect-dodge).

use sapphire_ecs::prelude::*;
use sapphire_time::ScaleSource;

use super::time::counter_scale;
use super::{ResolveStats, SlowMotion};
use crate::actors::ActorTable;
use crate::cues::{CueEvent, CueQueue, PERFECT_DODGE};
use crate::hits::HitQuery;
use crate::moves::{MoveLibrary, MoveOutput, MoveRunner, OpenCounter};

/// [`crate::CombatSet::Resolve`], after hits: fires each dodge a swing went through this step. A volume
/// is in the world only on a step its window can fire, so the swing's dodger has that window published,
/// and firing marks the playing, so it can't fire again.
#[allow(clippy::too_many_arguments, reason = "a system's parameters read as what it touches")]
pub(crate) fn fire_perfect_dodges(
    hits: Res<'_, HitQuery>,
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    world_clock: Res<'_, WorldClock>,
    mut slow: ResMut<'_, SlowMotion>,
    mut cues: ResMut<'_, CueQueue>,
    mut stats: ResMut<'_, ResolveStats>,
    mut actors: Query<'_, '_, (&mut EntityClock, &mut MoveRunner, &MoveOutput)>,
) {
    for swing in hits.dodged() {
        let Some(entity) = table.entity(swing.dodger) else { continue };
        let Ok((mut clock, mut runner, output)) = actors.get_mut(entity) else { continue };
        let (Some(index), Some(mut running)) = (output.perfect_window, runner.running) else { continue };
        let Some(window) = library.get(running.at).and_then(|mv| mv.perfect_dodge.get(index as usize)) else {
            continue;
        };

        let now = clock.0.now();
        running.dodged = true;
        runner.running = Some(running);
        if let Some(counter) = &window.counter {
            runner.counter = Some(OpenCounter {
                at: running.at,
                window: index,
                opens: now,
                closes: now + counter.duration(),
            });
        }
        if let Some(asked) = window.slow_motion {
            slow.request(asked.per_mille, asked.steps);
            clock.0.remove_source(ScaleSource::PerfectDodge);
            let pushed = clock.0.push_scale(ScaleSource::PerfectDodge, counter_scale(asked.per_mille));
            slow.refused += u64::from(pushed.is_err());
        }
        cues.push(CueEvent {
            actor: swing.dodger,
            cue: PERFECT_DODGE,
            tick: world_clock.0.ticks(),
            local_time: now,
            move_time: now.since(running.started_at),
            hit: None,
        });
        stats.perfect_dodges += 1;
    }
}
