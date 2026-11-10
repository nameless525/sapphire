//! Where a cue's mark stands, and the cue that says a perfect dodge is open (M4 A5, "cues by shape").
//!
//! A fight's warnings are marks over a fighter's head, each a shape of its own, so they read whatever
//! colours a player sees (m4-plan §3, "Colour"): the table's rows place them at [`EffectPlace::Crown`],
//! which [`Stand`] gives. One comes from the simulation's own cues, `telegraph` as a heavy's wind-up
//! starts. [`CueMarks`] fires the other two on the actor they're about, as cues of their own that no
//! document has to name:
//!
//! - [`DODGE_WINDOW`], read from the moves themselves, when a dodge pressed now would meet a foe's next
//!   strike inside the player's perfect window;
//! - [`NAMED_PHASE`], when an enemy says one of its named phases' names as a hit crosses it, so a new
//!   boss's phases are marked with no row of their own: its documents stay under `content/` alone (exit
//!   test 2). A row for the phase's own name still fires as well, for anything else it should show.
//!
//! Everything here reads the simulation and changes nothing in it, so no checksum moves.
//!
//! [`EffectPlace::Crown`]: crate::EffectPlace::Crown

use sapphire_core::Name;
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_math::Vec3;
use sapphire_moves::Shape;

use crate::{CueDispatcher, CueStep};

/// The cue [`CueMarks`] fires on the foe whose strike a dodge pressed now would perfectly meet.
pub const DODGE_WINDOW: &str = "dodge_window";

/// The cue [`CueMarks`] fires on an enemy whose named phase a hit just crossed.
pub const NAMED_PHASE: &str = "named_phase";

/// How far over the top of its body a fighter's crown mark stands, in metres: clear of a raised blade
/// or arm, and close enough to read as its.
pub const CROWN_CLEARANCE: f32 = 0.45;

/// Where a fighter stands, for a cue's effects: its feet, and the top of its body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    pub feet: Vec3,
    /// Over its feet, as high as its highest hurtbox reaches as it stands.
    pub crown: Vec3,
}

impl Stand {
    /// Where the simulation has `actor`: its feet at its position, its crown over them as high as the
    /// highest of its fighter's hurtboxes reaches. With no fighter, its crown is its feet.
    #[must_use]
    pub fn of(sim: &Simulation, actor: ActorId) -> Option<Self> {
        let view = sim.actor(actor)?;
        let set = view.moves.set.and_then(|set| sim.move_library().current(set));
        let hurtboxes =
            set.and_then(|set| set.fighter()).map(|fighter| &fighter.hurtboxes[..]).unwrap_or_default();
        let top = hurtboxes
            .iter()
            .map(|hurtbox| hurtbox.local.translation[1] + reach(hurtbox.shape))
            .fold(0.0, f32::max);
        Some(Self { feet: view.position, crown: view.position + Vec3::Y * top })
    }

    /// The same stand moved to `feet`, its crown as high over them.
    #[must_use]
    pub fn moved_to(self, feet: Vec3) -> Self {
        Self { feet, crown: feet + (self.crown - self.feet) }
    }
}

/// How far a shape reaches from its centre, whichever way it's turned.
fn reach(shape: Shape) -> f32 {
    match shape {
        Shape::Sphere { radius } => radius,
        Shape::Capsule { radius, length } => 0.5 * length + radius,
        Shape::Box { half_extents } => Vec3::from(half_extents).length(),
    }
}

/// Fires the cues no document names (`dodge_window` and `named_phase`) through a [`CueDispatcher`]'s table,
/// after each step. Allocates nothing once [`Self::reserve_actors`] made room.
#[derive(Clone, Debug, Default)]
pub struct CueMarks {
    windows: DodgeWindows,
}

impl CueMarks {
    /// Reserves room for the simulation's whole actor pool, so no step allocates.
    pub fn reserve_actors(&mut self, capacity: usize) {
        self.windows.reserve_actors(capacity);
    }

    /// Forgets what it announced: a fight began again.
    pub fn start_over(&mut self) {
        self.windows.start_over();
    }

    /// After a step, and after the dispatcher read its cues: fires [`NAMED_PHASE`] on each enemy that said
    /// a named phase in it, and [`DODGE_WINDOW`] on each foe of `player` whose strike just came within a
    /// perfect dodge's reach. Without a player, no dodge windows.
    pub fn read_step(
        &mut self,
        sim: &Simulation,
        player: Option<ActorId>,
        dispatcher: &mut CueDispatcher,
        step: &mut CueStep<'_>,
    ) {
        self.read(sim, player, |cue, actor| dispatcher.fire_on_actor(cue, actor, step));
    }

    /// [`Self::read_step`], calling `fire` with each cue and its actor instead of a dispatcher.
    pub fn read(&mut self, sim: &Simulation, player: Option<ActorId>, mut fire: impl FnMut(Name, ActorId)) {
        for actor in named_phases(sim) {
            fire(Name::new(NAMED_PHASE), actor);
        }
        if let Some(player) = player {
            self.windows.read_step(sim, player, |foe| fire(Name::new(DODGE_WINDOW), foe));
        }
    }
}

/// The actors whose cues in the latest step name one of their own named phases: what resolve says as a hit
/// crosses one. A move's timed cue of the same name counts too, so a phase's name is best its own.
pub fn named_phases(sim: &Simulation) -> impl Iterator<Item = ActorId> + '_ {
    let library = sim.move_library();
    let is_phase = move |actor: ActorId, cue: Name| {
        let set = sim.actor(actor).and_then(|view| view.moves.set);
        let mind = set.and_then(|set| library.mind(set));
        mind.is_some_and(|mind| mind.phases().iter().any(|phase| phase.name() == Some(cue)))
    };
    sim.cues().filter(move |cue| cue.hit.is_none() && is_phase(cue.actor, cue.cue)).map(|cue| cue.actor)
}

/// Fires [`DODGE_WINDOW`] on each foe of the player as its next strike comes within reach of a perfect
/// dodge: as early as a dodge pressed then would still meet the strike's first tick inside the player's
/// perfect-dodge window, which is that window's end on the player's set (the latest of its moves').
/// Once per strike, so a frozen or slowed foe isn't announced again.
///
/// It reads the windows the simulation runs on, so a run whose tables widen them (A6's timing assist)
/// moves the cue with them, and a player with no perfect dodge gets none.
#[derive(Clone, Debug, Default)]
struct DodgeWindows {
    /// Each foe announced, with the move's serial and its strike's start in sub-ticks.
    told: Vec<(ActorId, u32, u64)>,
}

impl DodgeWindows {
    /// Reserves room for the simulation's whole actor pool, so no step allocates.
    pub fn reserve_actors(&mut self, capacity: usize) {
        self.told.reserve(capacity.saturating_sub(self.told.len()));
    }

    /// Forgets what it announced: a fight began again.
    pub fn start_over(&mut self) {
        self.told.clear();
    }

    /// After a step: calls `fire` with each foe of `player` whose next strike just came within reach.
    /// Allocates nothing once [`Self::reserve_actors`] made room.
    pub fn read_step(&mut self, sim: &Simulation, player: ActorId, mut fire: impl FnMut(ActorId)) {
        self.told.retain(|&(actor, ..)| sim.actor(actor).is_some());
        let Some(player) = sim.actor(player) else { return };
        let library = sim.move_library();
        let Some(lead) = player.moves.set.and_then(|set| perfect_reach(library.current(set)?)) else {
            return;
        };
        let faction = player.fighter.faction();
        for foe in sim.actors() {
            if foe.id == player.id
                || foe.fighter.is_defeated()
                || !foe.fighter.faction().is_hostile_to(faction)
            {
                continue;
            }
            let Some(current) = foe.moves.current else { continue };
            let Some(mv) = library.get(current.at) else { continue };
            let now = current.elapsed.as_subticks();
            let strikes = mv.hitboxes.iter().filter(|hitbox| hitbox.attack.is_some());
            let next =
                strikes.map(|hitbox| hitbox.span.start().as_subticks()).filter(|&start| start > now).min();
            let Some(start) = next.filter(|&start| start - now <= lead) else { continue };
            let strike = (foe.id, current.serial, start);
            match self.told.iter().position(|(actor, ..)| *actor == foe.id) {
                Some(index) if self.told[index] == strike => continue,
                Some(index) => self.told[index] = strike,
                None if self.told.len() < self.told.capacity() => self.told.push(strike),
                // Past the pool it was sized for: announced every step rather than allocate.
                None => {}
            }
            fire(foe.id);
        }
    }
}

/// How far ahead of a strike a dodge can start and still meet it in a perfect window, in sub-ticks: the
/// latest end of any perfect-dodge window of `set` that can fire. `None` if none can.
fn perfect_reach(set: &sapphire_moves::MoveSet) -> Option<u64> {
    let windows = set.moves().iter().flat_map(|mv| &mv.perfect_dodge);
    windows.filter(|window| window.volume.is_some()).map(|window| window.span.end().as_subticks()).max()
}
