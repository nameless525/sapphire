//! An enemy's brain (spec §5.22, "Enemy AI", M2 G6): what presses an enemy's intents and tilts its
//! stick, so nothing past this file knows it isn't a player. Its moves are documents like anyone's, the
//! move system runs them, and a brain's tuning is a section of the same document
//! ([`sapphire_moves::BrainDef`]), so it reaches a replay's `tables_hash` and a set's replacement.
//!
//! It has two faces, as the controller does. [`step_brain`] is one brain for one tick, plain values in
//! and out with no ECS. [`Brain`] is the component every pooled actor carries, off until
//! [`crate::Simulation::give_brain`], and `think` runs it in [`crate::CombatSet::Intents`], in ID order,
//! after the buttons have gone in.
//!
//! # The rhythm
//!
//! A first guess at Zenless Zone Zero's common enemies, for the maintainer's Feel review 3 to move:
//!
//! 1. **Keep a distance** ([`BrainState::Pacing`]): walk in from past
//!    [`Spacing::far`](sapphire_moves::Spacing), back off from inside `near`, and between them stand or
//!    circle, by a drawn `circle_share`, facing the target throughout.
//! 2. **Pick an attack:** once its drawn `patience` has run out, each attack scores
//!    [`BrainAttack::score`](sapphire_moves::BrainAttack::score), from the distance, what the target is
//!    doing and how lately it was used, and one is drawn in proportion. If nothing scores, it keeps
//!    walking and asks again next tick, drawing nothing.
//! 3. **Commit** ([`BrainState::Attacking`]): the press starts the move on the same step, and the move's
//!    own telegraph and warp do the rest.
//! 4. **Rest** ([`BrainState::Resting`], for a drawn `rest`): the player's opening, with a new patience to
//!    follow.
//!
//! Over all of them, a move it didn't press is a **reaction** ([`BrainState::Reacting`]): it presses
//! nothing and pushes nothing until that's over, but `rise`, drawn when a knockdown starts. Whatever hits
//! it next, a poise break lying down included, is a new reaction, which draws for itself. It then stands
//! for a drawn `recover`, shorter than a rest, and **keeps the patience it had:** a reaction doesn't put
//! off its next attack, or a string of light hits landing every second would hold it off for good, as long
//! as a hit lands before it's ready to try again. **Defeat** ([`BrainState::Defeated`]) ends everything. A
//! target that's gone or defeated leaves it standing.
//!
//! # What it reads
//!
//! Only what a player reads off a screen: where things stand, what's playing and how far through
//! ([`TargetDoing`]). It reads no hit volume, no ring and no timer of its target's, and it writes
//! nothing but its own actor's ring, stick, aim and facing, so which brain goes first changes nothing
//! but the order of draws, which is ID order.
//!
//! # Time
//!
//! Its clock is its owner's, in whole sub-ticks. A frozen clock (`last_delta` zero) stops it: it decides
//! nothing and no timer runs down, because they're all readings of that clock. So hitstop spends none of
//! its patience and none of its rest.
//!
//! # Why a plain `enum` and not `statig`
//!
//! The spec named `statig`, and it was tried: it runs without allocating and its generated `State` can
//! be `Copy`. What it would bring is a macro, five build-time crates (one, `proc-macro-error2`, with a
//! future-incompatibility warning on every build) and a machine that isn't `Copy`, whose state can be read
//! but not written and has a private `initialized` flag beside it, so the checksum's per-field flip test
//! can't flip it. What it's for, nested states and entry and exit actions, has nothing to do here: what
//! overrides everything is two `if`s ahead of a `match`, and the states are five.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Pcg32, Vec3, det};
use sapphire_moves::{BrainDef, MAX_BRAIN_ATTACKS, MoveSet, Phase, TargetDoing, TickRange};
use sapphire_time::{SimDuration, SimTime};

use crate::actions::{Controls, Stick};
use crate::actors::{ActorId, ActorTable};
use crate::controller::{CharacterController, ControllerDrive};
use crate::intents::IntentRing;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveOutput, MoveRunner, MoveStats, WarpTarget};
use crate::resolve::Fighter;
use crate::simulation::SimRng;

/// Under this many metres across the ground, two roots give no direction: it stands where it is.
const MIN_SEPARATION: f32 = 1e-4;

/// What a brain is doing. A plain value in the checksum: the [module docs](self) say why it's this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrainState {
    /// Keeping its distance, and looking for an attack once its patience has run out.
    Pacing,
    /// Playing the attack it pressed: `serial` says which playing, of the actor's moves.
    Attacking { serial: u32 },
    /// Standing after an attack or a reaction, until `until`.
    Resting { until: SimTime },
    /// Playing a move it didn't press. It presses `rise` at `rise_at` ticks into it, if that's set.
    Reacting { serial: u32, rise_at: Option<u32> },
    /// Out of health. Nothing ends this.
    Defeated,
}

impl Default for BrainState {
    /// Resting with nothing to wait for, so its first tick draws its first patience.
    fn default() -> Self {
        Self::Resting { until: SimTime::ZERO }
    }
}

/// An actor's brain: all it carries from tick to tick. Every pooled actor has one, off until
/// [`crate::Simulation::give_brain`]. See the [module docs](self).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Brain {
    pub(crate) target: Option<ActorId>,
    pub(crate) state: BrainState,
    /// When it next looks for an attack, on its own clock. A reaction leaves this where it is.
    pub(crate) patience_until: SimTime,
    /// Which way it's circling: 1 is to its left as it faces its target, −1 its right, and 0 it stands.
    pub(crate) circle: i8,
    /// What it's pushing: frozen steps push it too, so `Controls` doesn't flicker.
    pub(crate) stick: Stick,
    /// When it last chose each attack, in [`BrainDef::attacks`]' order, on its own clock.
    pub(crate) last_used: [Option<SimTime>; MAX_BRAIN_ATTACKS],
}

impl Brain {
    /// Whether it's thinking at all.
    #[must_use]
    pub const fn is_on(&self) -> bool {
        self.target.is_some()
    }

    /// Who it's fighting.
    #[must_use]
    pub const fn target(&self) -> Option<ActorId> {
        self.target
    }

    /// What it's doing.
    #[must_use]
    pub const fn state(&self) -> BrainState {
        self.state
    }

    /// What it's pushing.
    #[must_use]
    pub const fn stick(&self) -> Stick {
        self.stick
    }

    /// A brain that's pacing, patient until `until`, for timing [`step_brain`] by itself: nothing a step
    /// reaches.
    #[doc(hidden)]
    #[must_use]
    pub fn bench(until: SimTime) -> Self {
        Self { state: BrainState::Pacing, patience_until: until, ..Self::default() }
    }

    /// A brain fighting `target`, from its next tick, with its first patience drawn.
    pub(crate) fn fighting(target: ActorId, def: &BrainDef, now: SimTime, rng: &mut Pcg32) -> Self {
        Self { target: Some(target), patience_until: now + draw_ticks(def.patience, rng), ..Self::default() }
    }
}

/// A move as a brain reads one of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeenMove {
    pub name: Name,
    /// Which playing this is, of the actor's moves.
    pub serial: u32,
    /// Whole ticks into it, as of the last step.
    pub tick: u64,
}

/// Its target, as a brain reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeenTarget {
    pub position: Vec3,
    pub doing: TargetDoing,
    pub defeated: bool,
}

/// What a brain reads for one tick. See the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    /// Its clock's reading this step.
    pub now: SimTime,
    /// How far its clock moved this step: zero is frozen.
    pub delta: SimDuration,
    pub position: Vec3,
    pub yaw: f32,
    pub defeated: bool,
    /// Moves it has started, all told: the next to start is one more.
    pub started: u32,
    /// What's playing, as of the last step.
    pub running: Option<SeenMove>,
    /// A reaction starts this step: a hit that froze no one forced it, and it hasn't started yet.
    pub reaction_due: bool,
    /// Its target, if that's alive.
    pub target: Option<SeenTarget>,
}

/// What a brain asks of its actor for one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrainOutput {
    /// An intent to press, as its place in the set's intents.
    pub press: Option<u32>,
    pub stick: Stick,
    /// The yaw to turn toward this tick, already limited to the brain's turn rate.
    pub face: Option<f32>,
}

/// A length drawn from `range`, from a generator. Draws nothing if it's one length.
fn draw_ticks(range: TickRange, rng: &mut Pcg32) -> SimDuration {
    let extra = if range.max > range.min { rng.below(range.max - range.min + 1) } else { 0 };
    SimDuration::from_ticks(u64::from(range.min + extra))
}

/// The attack to press out of `def`, drawn in proportion to its score, or `None` if none scores, which
/// draws nothing.
fn choose_attack(
    brain: &Brain,
    def: &BrainDef,
    now: SimTime,
    distance: f32,
    doing: TargetDoing,
    rng: &mut Pcg32,
) -> Option<usize> {
    let mut scores = [0.0_f32; MAX_BRAIN_ATTACKS];
    let mut total = 0.0;
    let mut underflowed = false;
    for (index, (attack, score)) in def.attacks.iter().zip(&mut scores).enumerate() {
        let since = brain.last_used[index].map(|used| now.since(used));
        *score = attack.score(distance, doing, since);
        total += *score;
        underflowed |= *score == 0.0 && attack.score_wide(distance, doing, since) > 0.0;
    }
    // Keep ordinary replay arithmetic unchanged. Exceptional authored magnitudes need a wider
    // product and total: infinity minus infinity would otherwise always fall through to the end,
    // while a subnormal total rounds the random draw to too few distinct positions.
    if !total.is_finite() || total.is_subnormal() || underflowed {
        let mut wide = [0.0_f64; MAX_BRAIN_ATTACKS];
        let mut total_wide = 0.0;
        for (index, attack) in def.attacks.iter().enumerate() {
            let since = brain.last_used[index].map(|used| now.since(used));
            wide[index] = attack.score_wide(distance, doing, since);
            total_wide += wide[index];
        }
        if total_wide <= 0.0 {
            return None;
        }
        let mut left = f64::from(rng.unit_f32()) * total_wide;
        let mut chosen = None;
        for (index, &score) in wide.iter().enumerate().take(def.attacks.len()) {
            if score > 0.0 {
                chosen = Some(index);
                left -= score;
                if left < 0.0 {
                    break;
                }
            }
        }
        return chosen;
    }
    if total <= 0.0 {
        return None;
    }
    let mut left = rng.unit_f32() * total;
    let mut chosen = None;
    for (index, &score) in scores.iter().enumerate().take(def.attacks.len()) {
        if score > 0.0 {
            chosen = Some(index);
            left -= score;
            if left < 0.0 {
                break;
            }
        }
    }
    chosen
}

/// Which way it goes while it keeps its distance: toward or away from the target past the band, and
/// round it within, leaning back to the band's middle so a circle stays one. `to_target` is unit length.
fn pacing_stick(def: &BrainDef, circle: i8, to_target: Vec3, distance: f32) -> Stick {
    let (near, far) = (def.spacing.near, def.spacing.far);
    let (x, z) = if distance > far {
        (to_target.x * def.pace.approach, to_target.z * def.pace.approach)
    } else if distance < near {
        (-to_target.x * def.pace.retreat, -to_target.z * def.pace.retreat)
    } else if circle != 0 {
        let sum = near + far;
        let middle = if sum.is_finite() { sum * 0.5 } else { near * 0.5 + far * 0.5 };
        let lean = (distance - middle) / (far - near) * 2.0;
        let round = f32::from(circle);
        // To the left of the way to the target, with 0 yaw facing +Z and +X on its left: (z, −x).
        let tangent = Vec3::new(to_target.z * round, 0.0, -to_target.x * round);
        let along = tangent + to_target * lean;
        (along.x * def.pace.circle, along.z * def.pace.circle)
    } else {
        (0.0, 0.0)
    };
    Stick::from_axes(x, z)
}

/// Runs one brain for one tick of its owner's clock: see the [module docs](self). **Frozen is
/// [`Seen::delta`] zero,** which reaches none of this: the caller doesn't call it.
pub fn step_brain(brain: &mut Brain, def: &BrainDef, seen: &Seen, rng: &mut Pcg32) -> BrainOutput {
    if seen.defeated {
        brain.state = BrainState::Defeated;
    }
    if let BrainState::Defeated = brain.state {
        brain.stick = Stick::CENTER;
        return BrainOutput::default();
    }
    // Pressing now would read the reaction as the attack it pressed, by its serial.
    if seen.reaction_due {
        brain.stick = Stick::CENTER;
        return BrainOutput::default();
    }

    let pressed = match brain.state {
        BrainState::Attacking { serial } => Some(serial),
        _ => None,
    };
    if let Some(running) = seen.running
        && Some(running.serial) != pressed
    {
        let known = matches!(brain.state, BrainState::Reacting { serial, .. } if serial == running.serial);
        if !known {
            let rise_at = def.rise.filter(|rise| rise.during() == running.name).map(|rise| {
                let span = rise.to - rise.from;
                rise.from + if span > 1 { rng.below(span) } else { 0 }
            });
            brain.state = BrainState::Reacting { serial: running.serial, rise_at };
        }
    }

    let mut output = BrainOutput::default();
    match brain.state {
        BrainState::Defeated => {}
        BrainState::Reacting { serial, rise_at } => {
            let Some(running) = seen.running else {
                return stand_for(brain, def, seen, def.recover, rng);
            };
            if let Some(at) = rise_at
                && running.tick + 1 >= u64::from(at)
            {
                output.press = def.rise.map(|rise| rise.intent());
                brain.state = BrainState::Reacting { serial, rise_at: None };
            }
        }
        BrainState::Attacking { .. } => {
            if seen.running.is_none() {
                let output = stand_for(brain, def, seen, def.rest, rng);
                if let BrainState::Resting { until } = brain.state {
                    brain.patience_until = until + draw_ticks(def.patience, rng);
                }
                return output;
            }
        }
        BrainState::Resting { until } => {
            if seen.now >= until {
                brain.state = BrainState::Pacing;
                brain.circle = if rng.unit_f32() < def.pace.circle_share {
                    if rng.below(2) == 0 { 1 } else { -1 }
                } else {
                    0
                };
                return pace(brain, def, seen, rng);
            }
            output.face = facing(def, seen);
        }
        BrainState::Pacing => return pace(brain, def, seen, rng),
    }
    brain.stick = output.stick;
    output
}

/// Starts standing for a length drawn from `range`, and says what that asks of this tick.
fn stand_for(
    brain: &mut Brain,
    def: &BrainDef,
    seen: &Seen,
    range: TickRange,
    rng: &mut Pcg32,
) -> BrainOutput {
    brain.state = BrainState::Resting { until: seen.now + draw_ticks(range, rng) };
    brain.stick = Stick::CENTER;
    BrainOutput { face: facing(def, seen), ..BrainOutput::default() }
}

/// Where a target stands from here across the ground, as a unit direction and metres, if it's alive and
/// not on top of this actor.
fn way_to_target(seen: &Seen) -> Option<(Vec3, f32)> {
    let target = seen.target.filter(|target| !target.defeated)?;
    let apart = target.position - seen.position;
    let distance = (apart.x * apart.x + apart.z * apart.z).sqrt();
    (distance > MIN_SEPARATION).then(|| (Vec3::new(apart.x / distance, 0.0, apart.z / distance), distance))
}

/// The yaw to turn toward this tick, to keep facing the target at the brain's own rate.
fn facing(def: &BrainDef, seen: &Seen) -> Option<f32> {
    let (to_target, _) = way_to_target(seen)?;
    let rate = def.pace.turn_rate_degrees.to_radians();
    Some(det::turn_towards(seen.yaw, det::yaw_of_direction(to_target), rate * seen.delta.as_secs_f32()))
}

/// One tick of [`BrainState::Pacing`]: the attack, if patience has run out and one scores, and otherwise
/// the distance.
fn pace(brain: &mut Brain, def: &BrainDef, seen: &Seen, rng: &mut Pcg32) -> BrainOutput {
    let way = way_to_target(seen);
    if seen.now >= brain.patience_until
        && let (Some((_, distance)), Some(target)) = (way, seen.target)
        && let Some(index) = choose_attack(brain, def, seen.now, distance, target.doing, rng)
    {
        brain.last_used[index] = Some(seen.now);
        brain.state = BrainState::Attacking { serial: seen.started.wrapping_add(1) };
        brain.stick = Stick::CENTER;
        return BrainOutput { press: Some(def.attacks[index].intent()), stick: Stick::CENTER, face: None };
    }
    brain.stick = way
        .map_or(Stick::CENTER, |(to_target, distance)| pacing_stick(def, brain.circle, to_target, distance));
    BrainOutput { press: None, stick: brain.stick, face: facing(def, seen) }
}

/// What a target is doing, by what a player would read off a screen.
fn what_it_is_doing(runner: &MoveRunner, output: &MoveOutput, library: &MoveLibrary) -> TargetDoing {
    if output.windows.invulnerable {
        return TargetDoing::Untouchable;
    }
    let Some(running) = runner.running else { return TargetDoing::Free };
    let Some(mv) = library.get(running.at) else { return TargetDoing::Free };
    let def = runner.set().and_then(|set| library.current(set)).and_then(MoveSet::fighter);
    if def.is_some_and(|def| def.reactions.contains(mv.name())) {
        return TargetDoing::Reacting;
    }
    match mv.phase_at(runner.cursor.since(running.started_at)) {
        Phase::Startup | Phase::Active => TargetDoing::Swinging,
        Phase::Recovery => TargetDoing::Opening,
    }
}

/// [`crate::CombatSet::Intents`], after the buttons have gone in: each brain, in ID order, on its own
/// clock. Everyone reads where everyone stood at the end of the last step, so the only thing that goes in
/// order is its draws.
pub(crate) fn think(
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    mut rng: ResMut<'_, SimRng>,
    mut stats: ResMut<'_, MoveStats>,
    mut minds: Query<
        '_,
        '_,
        (&mut Brain, &mut IntentRing, &mut Controls, &mut WarpTarget, &mut CharacterController, &EntityClock),
    >,
    bodies: Query<'_, '_, (&ActorTransform, &MoveRunner, &MoveOutput, &Fighter)>,
) {
    for (_, entity) in table.iter() {
        let Ok((mut brain, mut ring, mut controls, mut aim, mut controller, clock)) = minds.get_mut(entity)
        else {
            continue;
        };
        let Some(target) = brain.target else { continue };
        let Ok((transform, runner, _, fighter)) = bodies.get(entity) else { continue };
        let Some(set) = runner.set().and_then(|set| library.current(set)) else { continue };
        let Some(def) = set.brain() else { continue };

        let delta = clock.0.last_delta();
        if delta == SimDuration::ZERO {
            controls.current.stick = brain.stick;
            continue;
        }

        let seen_target = table.entity(target).and_then(|entity| bodies.get(entity).ok()).map(
            |(transform, runner, output, fighter)| SeenTarget {
                position: transform.position,
                doing: what_it_is_doing(runner, output, &library),
                defeated: fighter.is_defeated(),
            },
        );
        let running = runner.running.and_then(|running| {
            Some(SeenMove {
                name: library.get(running.at)?.name(),
                serial: running.serial,
                tick: runner.cursor.since(running.started_at).whole_ticks(),
            })
        });
        let seen = Seen {
            now: clock.0.now(),
            delta,
            position: transform.position,
            yaw: transform.yaw,
            defeated: fighter.is_defeated(),
            started: runner.started(),
            running,
            reaction_due: runner.forced.is_some(),
            target: seen_target,
        };
        let output = step_brain(&mut brain, def, &seen, &mut rng.0);

        controls.current.stick = output.stick;
        *aim = WarpTarget::Actor(target);
        if let Some(face) = output.face {
            controller.drive = ControllerDrive { face: Some(face), ..ControllerDrive::NONE };
        }
        if let Some(intent) = output.press.and_then(|at| set.intents().get(at as usize)) {
            stats.intents_overwritten += u64::from(ring.press(intent, &clock.0));
            stats.intents_pressed += 1;
        }
    }
}

#[cfg(test)]
mod numeric_tests {
    use super::*;
    use sapphire_moves::{MoveSetDocument, Reach};

    #[test]
    fn a_finite_spacing_band_keeps_its_circling_midpoint_finite() {
        let mut document =
            MoveSetDocument::parse(include_str!("../../../content/combat/enemy/warrior.moves.ron"))
                .expect("parses");
        document.brain.as_mut().expect("brain").spacing =
            sapphire_moves::Spacing { near: 1.0e38, far: 3.0e38 };
        let set = document.compile().expect("finite ordered spacing is valid");
        let def = set.brain().expect("brain");
        assert_eq!(pacing_stick(def, 1, Vec3::Z, 2.0e38), Stick::from_axes(def.pace.circle, 0.0));
    }

    #[test]
    fn extreme_finite_weights_sample_in_proportion_with_the_same_rng_draw() {
        let mut document =
            MoveSetDocument::parse(include_str!("../../../content/combat/enemy/warrior.moves.ron"))
                .expect("parses");
        let brain = document.brain.as_mut().expect("brain");
        brain.attacks.truncate(2);
        for attack in &mut brain.attacks {
            attack.weight = 1.0;
            attack.reach = Reach { near: 0.0, best: 2.0, far: 4.0 };
            attack.against = sapphire_moves::Against::default();
        }
        let ordinary = document.compile().expect("ordinary weights");
        for (weight, multiplier) in [
            (f32::MAX, 1.0),           // finite scores overflow only when summed
            (f32::MAX, f32::MAX),      // individual score products overflow
            (f32::from_bits(1), 0.25), // individual products underflow
            (f32::from_bits(1), 1.0),  // positive subnormal total quantizes the draw
        ] {
            for attack in &mut document.brain.as_mut().expect("brain").attacks {
                attack.weight = weight;
                attack.against.free = multiplier;
            }
            let extreme = document.compile().expect("valid finite weights");
            let mut selected = [false; 2];
            for seed in 0..64 {
                let mut a = Pcg32::new(seed, 7);
                let mut b = a;
                let mut one_draw = a;
                one_draw.unit_f32();
                let choose = |def, rng: &mut Pcg32| {
                    choose_attack(&Brain::default(), def, SimTime::ZERO, 2.0, TargetDoing::Free, rng)
                };
                let expected = choose(ordinary.brain().expect("brain"), &mut a);
                if seed == 1 {
                    assert_eq!(expected, Some(0), "fixed replayable selection");
                }
                assert_eq!(a.state(), one_draw.state(), "ordinary selection draws once");
                let actual = choose(extreme.brain().expect("brain"), &mut b);
                assert_eq!(actual, expected, "weight={weight}, multiplier={multiplier}, seed={seed}");
                assert_eq!(a.state(), b.state(), "exactly one draw");
                selected[actual.expect("both score")] = true;
            }
            assert_eq!(selected, [true, true]);
        }
    }
}
