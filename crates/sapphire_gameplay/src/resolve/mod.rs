//! Resolve (spec §5.22, steps 6 to 8, M2 G3): what a landed hit does. The feel it's after is Zenless Zone
//! Zero's: hits that stop both fighters for a beat, a stagger meter, and reactions a heavier hit
//! overrides. It's the smallest set of attributes and effects that fight uses, not a framework: a
//! [`Fighter`]'s health and poise, instant damage and poise damage, one tag ([`Fighter::is_defeated`]),
//! hitstop and a push.
//!
//! Owns [`Fighter`], how a hit lands ([`HitOutcome`]), the cues a hit publishes and the hitstop it asks
//! for, and the seam for world-clock slow motion ([`SlowMotion`]). Reads what `sapphire_moves` authors
//! ([`Attack`], [`sapphire_moves::FighterDef`], [`sapphire_moves::ArmourWindow`]) and what the `hits`
//! module found.
//!
//! Also owns what a perfect dodge does when a swing goes through one ([`fire_perfect_dodges`], M2 G4).
//!
//! Doesn't own: which volumes touch what (the `hits` module), a parry (M2 has none), who's allowed to hit
//! whom (everyone can hit everyone: M2 has one player and one enemy), or what a cue looks and sounds
//! like (M2 E3).
//!
//! # A step
//!
//! 1. [`crate::CombatSet::Movement`]: [`push_knockback`] adds a sliding actor's push to what its move
//!    wants of its root, on its own clock, and the `hits` module sets its hurtboxes afterwards.
//! 2. [`crate::CombatSet::Hits`]: `HitQuery` fills its events.
//! 3. [`crate::CombatSet::Resolve`]: [`apply_hits`] applies them, nearest first in the order
//!    `HitQuery::events` states, and forces reactions. [`fire_perfect_dodges`] then fires what
//!    `HitQuery::dodged` found.
//! 4. [`crate::CombatSet::TimeScale`]: [`apply_time`] freezes clocks and moves slow motion along, with
//!    what a perfect dodge has scaled back up following it.
//! 5. [`crate::CombatSet::Cues`]: [`publish_hit_cues`] tells presentation.
//!
//! So a hit lands everywhere in one step (pillar 3). Clocks tick at the start of a step, so a hit's
//! freeze covers the next `hitstop` steps. Its reaction starts on the first of them, because a forced
//! start is the one thing a frozen clock fires: the target shows its first frame of reacting for the whole
//! freeze, and the attacker holds where the hit found it.
//!
//! # What a hit does
//!
//! In order, on a target that isn't already defeated:
//!
//! 1. Its damage comes off health, and its poise off the meter, whatever else happens. An invulnerable
//!    target has no hurtboxes to touch, so it never gets this far.
//! 2. If that took the last of its health, it's **defeated:** its `defeat` reaction plays, its hurtboxes
//!    leave the world, its capsule stops meeting anyone (`CharacterWorld::fell`), it starts nothing more
//!    and hits nothing more.
//! 3. If that took the last of its poise, its **poise breaks:** its `poise_break` reaction plays, whatever
//!    armour says, and the meter fills again.
//! 4. If the hit's poise is below the threshold of an armour window open on its running move, its armour
//!    **absorbs** it: no reaction and no push, and only the attacker freezes, so the armoured
//!    swing comes through while the attacker is stopped. That's what makes armour worth having; a
//!    freeze on both would let a stream of light hits hold an armoured move still.
//! 5. Otherwise it **flinches:** the reaction for the hit's weight plays.
//!
//! All but an absorbed hit push the target, away from the attacker across the ground, on the target's
//! own clock, so hitstop delays the slide and costs it nothing. A slide runs its course whatever the
//! target does next, and a later reaction's replaces it.
//!
//! # When hits overlap
//!
//! - **On one tick:** every hit in `HitQuery::events` lands, in that order: nearest first, then by
//!   attacker. Two actors hitting each other on one tick trade: both take their damage and both
//!   react. A blow from an attacker defeated earlier in the same step still lands, since it was already
//!   swung. A target defeated by an earlier hit of the step takes no more.
//! - **Reactions:** the strongest of the step plays, and its push: a defeat, then a poise break, then
//!   the heaviest flinch, the first among equals. A later step's reaction replaces what's playing,
//!   however light, unless armour or a hurtbox window says otherwise: that's a move's to author. A
//!   knockdown that shouldn't be flinched out of says so with `invulnerable`.
//! - **Hitstop:** an actor's freeze is the longest asked of it. Two hits don't add up, and a shorter
//!   freeze doesn't cut a longer one (`LocalClock::apply_hitstop`).
//!
//! # A finish freeze needs no clock of its own
//!
//! M2's decision (spec §5.21's P3 note left it open): a hit with `finish_freeze` that defeats its target
//! freezes every actor's clock, as hitstop does, and leaves the world clock running. Everything worth
//! playing through a finish lives on the world clock or on real time: P3's `EffectStack` has `World` and
//! `Real` layers, which both run while the world does, and the camera has its own clock. So screen
//! effects play on over a stopped fight, and nothing needs a third kind of time. Stopping the world
//! clock instead is what would need one: `Real` layers and hitstop's own count both wait for a running
//! world, so a stopped world would hold the very effects a finish is for, with nothing left running to
//! end it.
//!
//! # World-clock slow motion
//!
//! [`SlowMotion`]: a request from any system before [`crate::CombatSet::TimeScale`] slows the world for an
//! exact number of real steps.
//!
//! # A perfect dodge
//!
//! Zenless Zone Zero's rhythm: a dodge just as a blow would land stops the fight's clock for everyone
//! but the dodger, and the answer is one button away. A move's `perfect_dodge` window authors it
//! ([`sapphire_moves::PerfectDodgeWindow`]). While one is open, invulnerable as a document has to make it,
//! the actor has a volume in the world, and a sweep of anyone's live volume that touches it is a
//! `DodgedSwing`, which lands nothing. What fires, in that step, once per playing of the dodge:
//!
//! - **The swing is spent on the dodger:** its hit group has the dodger on record in that volley, as a
//!   landed hit would leave it, so it can't land on the dodger, however long the slowdown stretches it,
//!   or fire the dodger's next dodge. The fight's rhythm depends on trusting a perfect dodge, and a swing
//!   that landed anyway at 0.2× would feel like a bug.
//! - **Slow motion, if authored:** the world at its `per_mille` for its `steps` real steps from the next
//!   step, the dodger's own clock scaled back up over exactly those steps, so it moves at normal speed
//!   in a slowed world. A later request, from another dodge or anything else, replaces what's running, and
//!   clocks already scaled back follow it until it ends.
//! - **A counter window, if authored:** see the `moves` module. It's a cancel window on the dodger's own
//!   clock, so a press goes through what any cancel does, with its buffer, its priority and its exact
//!   instant, and a counter starts on its press's own step. A forced move would start a step later,
//!   fire through a frozen clock and need all of that again by hand.
//! - **[`crate::PERFECT_DODGE`],** the cue presentation hangs its flash and sound on.
//!
//! When things meet:
//!
//! - **Who can fire one:** the volume has to be live, so one not yet open or already closed can't, and it
//!   has to do something ([`Attack`]) and be able to land, so a sensor can't, and nor can a swing
//!   whose hit group already has the dodger in this volley, or that a wall has stopped. A frozen attacker
//!   sweeps nothing, so it can't either.
//! - **A frozen dodger** reads no new ticks and fires nothing: its window waits, and so does its counter
//!   window, on its own clock, and a swing still touching when it thaws fires then.
//! - **A hit on the tick the window opens:** the window wins. Hurtboxes leave the world in
//!   [`crate::CombatSet::Movement`], before any sweep, on the step that first reads the window, so
//!   on that step the swing touches the perfect-dodge volume, and fires it, and lands nothing. On the step
//!   before, the window isn't open: a hit lands and its reaction replaces the dodge. A step never has
//!   both, because the window only counts on a step the actor is invulnerable.
//! - **Plain i-frames spend nothing:** a swing still live when an actor's invulnerability ends lands as if
//!   it had just arrived, whether the actor had no window or had one that fired nothing. Only a fired
//!   perfect dodge spends, and only the swing it credits, for that dodger. A second attacker's swing on the
//!   same step, or another hit group of the same move, lands when the i-frames end, on whatever reaction
//!   that plays, which closes the counter window. So a counter still wants i-frames of its own.
//! - **A swing that re-arms** comes back as a landed one does: the record covers the volley it went
//!   through, so from the group's next re-arm it can land on the dodger, or fire its next dodge.
//!
//! # Determinism
//!
//! Numbers are integers, a slide's plain float arithmetic on whole sub-ticks. [`Fighter`] goes in the
//! checksum. So do cues, where everything a hit decided shows, and [`AppliedHits`] doesn't: it's rebuilt
//! every step and nothing reads it after the step ends.

mod dodge;
mod fighter;
mod time;

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};
use sapphire_moves::{Attack, MoveSet, Reactions, Weight};
use sapphire_time::SimTime;

pub(crate) use self::dodge::fire_perfect_dodges;
pub(crate) use self::fighter::Slide;
pub use self::fighter::{Fighter, HitOutcome};
pub use self::time::SlowMotion;
pub(crate) use self::time::{apply_time, counter_scale};
use crate::actors::ActorTable;
use crate::controller::CharacterWorld;
use crate::cues::{CueEvent, CueQueue, DEFEATED, HitCue, POISE_BROKEN};
use crate::hits::{HitEvent, HitQuery};
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveOutput, MoveRunner};

/// Under this many metres apart across the ground, two roots give no direction to push along, and a
/// push goes the way the attacker faces.
const MIN_PUSH_SEPARATION: f32 = 1e-4;

/// What resolve has done so far: deterministic, and not state, since nothing reads it back.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolveStats {
    pub(crate) hits_applied: u64,
    pub(crate) hits_absorbed: u64,
    pub(crate) reactions_forced: u64,
    pub(crate) poise_breaks: u64,
    pub(crate) defeats: u64,
    pub(crate) hurtboxes_refused: u64,
    pub(crate) perfect_dodges: u64,
}

/// A hit that did something this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AppliedHit {
    pub(crate) event: HitEvent,
    pub(crate) attack: Attack,
    pub(crate) outcome: HitOutcome,
    /// When the attacker's move began, on its clock.
    pub(crate) move_started_at: SimTime,
}

/// This step's applied hits, in the order they applied, for the rest of the step to read. Never grows
/// past `HitQuery`'s own limit, which it reserves.
#[derive(Resource, Debug)]
pub(crate) struct AppliedHits(pub(crate) Vec<AppliedHit>);

impl Default for AppliedHits {
    fn default() -> Self {
        Self(Vec::with_capacity(crate::hits::MAX_HIT_EVENTS))
    }
}

/// How strong a reaction is: the strongest of a step plays.
const fn strength(outcome: HitOutcome, weight: Weight) -> u8 {
    match outcome {
        HitOutcome::Absorbed => 0,
        HitOutcome::Flinched => 1 + weight as u8,
        HitOutcome::PoiseBroken => 1 + Weight::Heavy as u8 + 1,
        HitOutcome::Defeated => 1 + Weight::Heavy as u8 + 2,
    }
}

/// The move a hit's outcome plays, if the fighter authored one.
fn reaction(reactions: &Reactions, outcome: HitOutcome, weight: Weight) -> Option<Name> {
    match outcome {
        HitOutcome::Absorbed => None,
        HitOutcome::Flinched => reactions.flinch(weight),
        HitOutcome::PoiseBroken => reactions.poise_break(),
        HitOutcome::Defeated => reactions.defeat(),
    }
}

/// Which way a hit pushes its target: away from the attacker across the ground, or the way it faces if
/// they stand on one spot.
fn push_direction(attacker: ActorTransform, target: ActorTransform) -> Vec3 {
    let apart = target.position - attacker.position;
    let length = (apart.x * apart.x + apart.z * apart.z).sqrt();
    if length > MIN_PUSH_SEPARATION {
        Vec3::new(apart.x / length, 0.0, apart.z / length)
    } else {
        det::rotate_y(Vec3::Z, attacker.yaw)
    }
}

/// [`crate::CombatSet::Movement`], before root motion applies: each actor's slide adds to what its move
/// wants of its root, by what its clock did this step.
pub(crate) fn push_knockback(
    table: Res<'_, ActorTable>,
    mut actors: Query<'_, '_, (&EntityClock, &mut Fighter, &mut MoveOutput)>,
) {
    for (_, entity) in table.iter() {
        let Ok((clock, mut fighter, mut output)) = actors.get_mut(entity) else { continue };
        if fighter.is_sliding() {
            output.root_delta.translation += fighter.advance_slide(clock.0.last_delta());
        }
    }
}

/// [`crate::CombatSet::Resolve`]: refills poise that's due, then applies this step's hits. See the
/// [module docs](self).
#[allow(clippy::type_complexity, reason = "a system's query reads as what it touches")]
#[allow(clippy::too_many_arguments, reason = "a system's parameters read as what it touches")]
pub(crate) fn apply_hits(
    table: Res<'_, ActorTable>,
    hits: Res<'_, HitQuery>,
    library: Res<'_, MoveLibrary>,
    mut applied: ResMut<'_, AppliedHits>,
    mut stats: ResMut<'_, ResolveStats>,
    attackers: Query<'_, '_, (&MoveOutput, &ActorTransform)>,
    mut targets: Query<'_, '_, (&EntityClock, &mut Fighter, &mut MoveRunner, &MoveOutput, &ActorTransform)>,
    mut characters: ResMut<'_, CharacterWorld>,
) {
    applied.0.clear();
    for (_, entity) in table.iter() {
        let Ok((clock, mut fighter, ..)) = targets.get_mut(entity) else { continue };
        if fighter.refill_is_due(clock.0.now()) {
            fighter.refill_poise();
        }
    }
    for event in hits.events() {
        let Some((attack, move_started_at, attacker_root)) =
            attack_of(event, &hits, &table, &library, &attackers)
        else {
            continue;
        };
        let Some(entity) = table.entity(event.target) else { continue };
        let Ok((clock, mut fighter, mut runner, output, target_root)) = targets.get_mut(entity) else {
            continue;
        };
        if fighter.is_defeated() || !fighter.is_fighter() {
            continue;
        }
        let outcome = fighter.take_hit(&attack, output.windows.armour, clock.0.now());

        let strength_of = |hit: &AppliedHit| strength(hit.outcome, hit.attack.weight);
        let same_target = applied.0.iter().filter(|hit| hit.event.target == event.target);
        let stronger_this_step = same_target.map(strength_of).max().unwrap_or(0);
        if strength(outcome, attack.weight) > stronger_this_step {
            let set = runner.set().and_then(|set| library.current(set)).and_then(MoveSet::fighter);
            if let Some(name) = set.and_then(|def| reaction(&def.reactions, outcome, attack.weight)) {
                runner.force(name);
                stats.reactions_forced += 1;
            }
            fighter.start_slide(attack.knockback, push_direction(attacker_root, *target_root));
        }

        stats.hits_applied += 1;
        stats.hits_absorbed += u64::from(outcome == HitOutcome::Absorbed);
        stats.poise_breaks += u64::from(outcome == HitOutcome::PoiseBroken);
        stats.defeats += u64::from(outcome == HitOutcome::Defeated);
        if outcome == HitOutcome::Defeated {
            characters.fell(event.target);
        }
        applied.0.push(AppliedHit { event: *event, attack, outcome, move_started_at });
        // The attacker's meter fills for it (M3 Q3c), as the target's did as it took it.
        if let Some(attacker) = table.entity(event.attacker)
            && let Ok((_, mut attacker, ..)) = targets.get_mut(attacker)
        {
            attacker.landed_a_hit();
        }
    }
}

/// What the volume behind a hit authored, when its move began, and where its attacker stands. `None` for
/// a volume with no [`Attack`], which is an event and nothing more.
fn attack_of(
    event: &HitEvent,
    hits: &HitQuery,
    table: &ActorTable,
    library: &MoveLibrary,
    attackers: &Query<'_, '_, (&MoveOutput, &ActorTransform)>,
) -> Option<(Attack, SimTime, ActorTransform)> {
    let (output, root) = attackers.get(table.entity(event.attacker)?).ok()?;
    if event.volume == HitEvent::PRIVATE_VOLUME {
        let source = hits.private_attack_source(event)?;
        let [original] = &library.get(source.movement)?.hitboxes[..] else { return None };
        return Some((original.attack?, source.started_at, *root));
    }
    let volume = output.volumes.get(usize::from(event.volume))?;
    let attack = library.get(volume.at)?.hitboxes.get(volume.hitbox as usize)?.attack?;
    Some((attack, volume.started_at, *root))
}

/// [`crate::CombatSet::Cues`]: each applied hit's cue, in the order they applied, and another for what it
/// broke or ended. E3 hangs effects, sound, shake, rumble and P3's post presets on these, and the app
/// turns them into pushes, since this crate has no edge to the world.
pub(crate) fn publish_hit_cues(
    applied: Res<'_, AppliedHits>,
    world_clock: Res<'_, WorldClock>,
    mut cues: ResMut<'_, CueQueue>,
) {
    let tick = world_clock.0.ticks();
    for hit in &applied.0 {
        let event = &hit.event;
        let cue = |name: Name| CueEvent {
            actor: event.attacker,
            cue: name,
            tick,
            local_time: hit.move_started_at + event.at,
            move_time: event.at,
            hit: Some(HitCue {
                target: event.target,
                hurtbox: event.hurtbox,
                point: event.point,
                normal: event.normal,
                weight: hit.attack.weight,
                damage: hit.attack.damage,
                poise: hit.attack.poise,
                hitstop: hit.attack.hitstop,
                outcome: hit.outcome,
            }),
        };
        cues.push(cue(hit.attack.cue()));
        match hit.outcome {
            HitOutcome::PoiseBroken => cues.push(cue(POISE_BROKEN)),
            HitOutcome::Defeated => cues.push(cue(DEFEATED)),
            HitOutcome::Flinched | HitOutcome::Absorbed => {}
        }
    }
}
