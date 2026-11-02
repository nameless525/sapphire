//! [`crate::CombatSet::TimeScale`]: hitstop from this step's hits, and world-clock slow motion. Both take
//! effect from the next step's clocks. See the [module docs](super).

use sapphire_ecs::prelude::*;
use sapphire_time::{ScaleSource, ScaledClock};

use super::{AppliedHits, HitOutcome};
use crate::actors::ActorTable;
use crate::checksum::StateHasher;

/// World-clock slow motion: the seam M2 G4's perfect dodge builds on. A request slows the world for an
/// exact number of real fixed steps, ends by itself and is in the checksum. Hitstop counts real steps
/// too, so it lasts as long as authored under this.
///
/// It uses the `PerfectDodge` scale source, whose counter-scale on the player's own clock is G4's to
/// push and pop ([`Self::is_running`] says when). Anything else that wants the world slower for a while
/// asks here.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlowMotion {
    /// `(per mille, steps)`, taken up by the next [`crate::CombatSet::TimeScale`].
    pub(crate) requested: Option<(u32, u16)>,
    /// Steps it has left to run.
    pub(crate) left: u16,
    /// Requests the world's scale stack had no room for: non-zero means something else fills it.
    pub(crate) refused: u64,
}

impl SlowMotion {
    /// Slows the world to this many thousandths of its speed for the next `steps` fixed steps, from any
    /// system before [`crate::CombatSet::TimeScale`], or between steps. The latest request replaces
    /// whatever's running, and a request for no steps ends it.
    pub fn request(&mut self, per_mille: u32, steps: u16) {
        self.requested = Some((per_mille, steps));
    }

    /// Whether the world is slowed.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.left > 0
    }

    /// Requests the world's scale stack had no room for.
    #[must_use]
    pub const fn refused(&self) -> u64 {
        self.refused
    }

    /// Moves it on by one real step, or takes up a request. A stopped world holds it, as it holds hitstop.
    fn settle(&mut self, world: &mut ScaledClock) {
        if let Some((per_mille, steps)) = self.requested.take() {
            if self.left > 0 {
                world.remove_source(ScaleSource::PerfectDodge);
            }
            self.left = 0;
            if steps > 0 {
                match world.push_scale(ScaleSource::PerfectDodge, per_mille) {
                    Ok(_) => self.left = steps,
                    Err(_) => self.refused += 1,
                }
            }
        } else if self.left > 0 && !world.is_stopped() {
            self.left -= 1;
            if self.left == 0 {
                world.remove_source(ScaleSource::PerfectDodge);
            }
        }
    }

    pub(crate) fn write_canonical(&self, hasher: &mut StateHasher) {
        let (per_mille, steps) = self.requested.unwrap_or((u32::MAX, u16::MAX));
        hasher.u8(u8::from(self.requested.is_some()));
        hasher.u32(per_mille);
        hasher.u16(steps);
        hasher.u16(self.left);
        hasher.u64(self.refused);
    }
}

/// The system. Each applied hit freezes its attacker and, unless armour took it, its target; a defeat
/// with a finish freeze stops every actor. An actor's freeze is the longest asked of it, so the order
/// of these changes nothing. See the [module docs](super#when-hits-overlap).
pub(crate) fn apply_time(
    table: Res<'_, ActorTable>,
    applied: Res<'_, AppliedHits>,
    mut slow: ResMut<'_, SlowMotion>,
    mut world_clock: ResMut<'_, WorldClock>,
    mut clocks: Query<'_, '_, &mut EntityClock>,
) {
    for hit in &applied.0 {
        let mut freeze = |actor, ticks| {
            let Some(entity) = table.entity(actor) else { return };
            if let Ok(mut clock) = clocks.get_mut(entity) {
                clock.0.apply_hitstop(ticks);
            }
        };
        freeze(hit.event.attacker, hit.attack.hitstop);
        if hit.outcome != HitOutcome::Absorbed {
            freeze(hit.event.target, hit.attack.hitstop);
        }
        if hit.outcome == HitOutcome::Defeated && hit.attack.finish_freeze > 0 {
            for (actor, _) in table.iter() {
                freeze(actor, hit.attack.finish_freeze);
            }
        }
    }
    slow.settle(&mut world_clock.0);
}
