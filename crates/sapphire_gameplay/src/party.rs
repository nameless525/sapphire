//! The party (spec §5.22, M2 G7): up to three characters, one on the field, the others off it and still
//! ticking. Swapping is an ability, and the swap-in assist is an ordinary move.
//!
//! Owns [`Party`], which slot is on the field and each slot's readiness; [`PartyTuning`], the ability's two
//! numbers; and [`settle_swaps`], the system that decides a swap and carries it out. It's a resource rather
//! than a component of each actor, because one thing is true of all three: whoever's on the field.
//!
//! Doesn't own: what a swap looks like (`sapphire_cues` hangs effects on [`crate::PARTY_SWAP`], and the app
//! changes what it draws), what the assist does (a move in each member's own set, as any other), how the
//! player names a slot (a press means the next one: see below), or a second party (one a simulation).
//!
//! # A swap
//!
//! A member's set has an intent named [`SWAP_INTENT`], and a move named [`SWAP_IN_MOVE`]. The button reaches
//! the ring as any other press does. What's different is who reads it: [`crate::CombatSet::Party`], between
//! [`crate::CombatSet::Intents`] and [`crate::CombatSet::Cancels`], and not a cancel window. So it can fire
//! out of anything, a move's recovery or a hit's reaction, which is the point of it, and the press has
//! gone through everything an intent does:
//!
//! - **On a frozen clock it waits.** Time here is the owner's, so hitstop can't eat a press, and it goes on
//!   the first step that moves, as a cancel's would.
//! - **While the ability can't fire it waits too,** to its buffer's end and no further: on cooldown, or with
//!   nobody ready to come in. A press a few ticks early still swaps, and one too early is gone.
//! - **It names no slot.** It means the next member in slot order who's standing and rested. `TickActions`'
//!   layout has one swap button and no room for more, and a stick's direction already means where to go.
//!
//! When it fires, on the press's own step:
//!
//! - **Who leaves** stops whatever it's doing, with nothing more of it read: its move is over, its buffered
//!   presses and any counter window are gone, a push stops, and its capsule and hurtboxes leave the world.
//! - **Who comes in** stands where that one stood, facing as it faced and in the same state on the ground or
//!   in the air, gets its capsule at that place, takes over its [`Controls`] so a button held across the
//!   swap isn't a press, and its [`WarpTarget`]. Whoever was aiming at the one who left aims at this one,
//!   and a [`Brain`] fighting it fights this one.
//!   A perfect dodge's scale-back comes along, so a swap out of a dodge's slow motion runs at full speed.
//! - **The assist starts,** forced as a hit's reaction is, at this step's reading: tick 0 on the press's
//!   own step, as a neutral start would be. From there it's a move: its hit volumes land, its armour
//!   absorbs, its i-frames take its hurtboxes out and its cancels take presses.
//! - [`crate::PARTY_SWAP`] goes out, with the one who came in as its actor.
//!
//! # The two numbers
//!
//! Both on the owner's clock, so each is read against the clock of the character it belongs to, and both
//! live in one place, [`PartySlot::ready_at`], since a slot is only ever doing one of these:
//!
//! - [`PartyTuning::swap_cooldown`]: how long whoever came in must stay. Hitstop on it holds this back.
//! - [`PartyTuning::rest`]: how long whoever left must stay off. It's what the off-field clock runs down.
//!
//! # Off the field
//!
//! An off-field member stays a live actor with a clock that ticks. Its poise refills and its timers run
//! down, every system that walks actors still reaches it, and a finish freeze passes it by: that stops
//! a fight it isn't in. What it doesn't have is any effect on the fight:
//!
//! - **No bodies.** Its capsule and hurtboxes aren't in the world, in the spirit of invulnerability, so
//!   nothing can hit it, it blocks nothing and a cast never sees it.
//! - **No input.** Input names the party by its first member's ID ([`Party::input_actor`]), whoever's on the
//!   field, so a script or a live player's bindings need no idea who that is. A tick's entry for
//!   another member's ID is ignored. Its [`Controls`] stay neutral.
//! - **No moves.** [`crate::CombatSet::Cancels`] drops what would start one: a force, a press a brain made.
//!
//! # When the one on the field falls
//!
//! A defeat **forces a swap** to the next member standing, rested or not and with no regard for the
//! cooldown, once the fallen character's reaction has played out and its clock is moving, so the fall shows
//! and a finish freeze holds. How long that takes is the `defeat` move's to author. With nobody left the
//! **party is down**: [`Party::is_down`] says so, [`crate::PARTY_DEFEATED`] goes out once, and whoever fell
//! last stays where it fell. That's the fight's end, as the simulation sees it: what comes after is the app's.
//!
//! # Order
//!
//! `CombatSet::Party` runs after `Intents`, so a press is in the ring, and a brain has pressed, and before
//! `Cancels`, so the assist starts this step and the one who left starts nothing. So a hit on this step's
//! `Hits` finds the one who came in, where it now stands. `tests/party.rs` holds all of it.
//!
//! # Determinism
//!
//! Plain integers and whole sub-ticks, walked in slot order, and no allocation: a capsule comes and goes
//! within the world's body capacity. [`Party`] goes in the checksum, and writes nothing while no party is
//! formed, so a fight without one has the checksums it always had but for the layout's number.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_moves::MoveSet;
use sapphire_time::{ScaleSource, SimDuration, SimTime};

use crate::actions::Controls;
use crate::actors::{ActorId, ActorTable};
use crate::brain::Brain;
use crate::checksum::StateHasher;
use crate::controller::{
    CharacterController, CharacterWorld, ControllerDrive, ControllerEvents, ControllerState,
};
use crate::cues::{CueEvent, CueQueue, PARTY_DEFEATED, PARTY_SWAP};
use crate::intents::IntentRing;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveRunner, WarpTarget};
use crate::resolve::{Fighter, counter_scale};

/// How many members a party can have.
pub const PARTY_SLOTS: usize = 3;

/// The intent a member's set names for the swap button: a press of it is what `settle_swaps` reads.
pub const SWAP_INTENT: Name = Name::new("swap");

/// The move a member's set names for its swap-in assist.
pub const SWAP_IN_MOVE: Name = Name::new("swap_in");

/// The ability's numbers, in ticks of each character's own clock: the `party` module's docs say what each
/// does. Plain data the scenario or the game supplies, and in `Simulation::tables_hash`, so a replay can
/// tell changed tuning from a broken simulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PartyTuning {
    /// How long whoever came in must stay.
    pub swap_cooldown: u32,
    /// How long whoever left must stay off.
    pub rest: u32,
}

/// A party that can't be formed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PartyError {
    #[error("this simulation already has a party")]
    AlreadyFormed,
    #[error("a party has from 1 to {PARTY_SLOTS} members, and this one has {count}")]
    BadSize { count: usize },
    #[error("actor {} is in the party twice", .actor.raw())]
    Twice { actor: ActorId },
    #[error("actor {} isn't alive", .actor.raw())]
    NotAlive { actor: ActorId },
    #[error("actor {}'s set of moves has no `swap` intent, or it has no set", .actor.raw())]
    NoSwapIntent { actor: ActorId },
    #[error("actor {}'s set of moves has no `swap_in` move", .actor.raw())]
    NoSwapInMove { actor: ActorId },
}

/// One slot of a party.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PartySlot {
    /// Who's in it.
    pub actor: ActorId,
    /// When it can next change place, on its own clock: leave, on the field, or come back, off it.
    pub ready_at: SimTime,
}

/// A formed party's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Formed {
    pub(crate) slots: [Option<PartySlot>; PARTY_SLOTS],
    /// Which slot is on the field. Always a filled one.
    pub(crate) active: u8,
    pub(crate) tuning: PartyTuning,
    /// Swaps made so far, those it was forced into among them.
    pub(crate) swaps: u32,
    /// Of those, the ones a defeat forced.
    pub(crate) forced_swaps: u32,
    /// Nobody's left standing.
    pub(crate) down: bool,
}

impl Formed {
    fn active_slot(&self) -> PartySlot {
        self.slots[usize::from(self.active)].expect("the active slot is always a filled one")
    }

    /// The slots after the active one, in order, round to the one before it.
    fn others(&self) -> impl Iterator<Item = (u8, PartySlot)> + '_ {
        (1..PARTY_SLOTS as u8).filter_map(|offset| {
            let index = (self.active + offset) % PARTY_SLOTS as u8;
            Some((index, self.slots[usize::from(index)]?))
        })
    }

    fn holds(&self, actor: ActorId) -> bool {
        self.slots.iter().flatten().any(|slot| slot.actor == actor)
    }
}

/// The simulation's party, if it has one: `Simulation::form_party` makes it. See the `party` module's docs.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Party {
    pub(crate) formed: Option<Formed>,
}

impl Party {
    /// Whether this simulation has a party.
    #[must_use]
    pub const fn is_formed(&self) -> bool {
        self.formed.is_some()
    }

    /// Whose ID a tick's input names to reach whoever's on the field: the first member's, for good.
    #[must_use]
    pub fn input_actor(&self) -> Option<ActorId> {
        self.formed?.slots[0].map(|slot| slot.actor)
    }

    /// Who's on the field.
    #[must_use]
    pub fn active(&self) -> Option<ActorId> {
        self.formed.map(|formed| formed.active_slot().actor)
    }

    /// Which slot is on the field, for what depends on who it is, such as what to draw.
    #[must_use]
    pub fn active_slot(&self) -> Option<usize> {
        self.formed.map(|formed| usize::from(formed.active))
    }

    /// The party's slots, in order.
    pub fn slots(&self) -> impl Iterator<Item = PartySlot> + '_ {
        self.formed.iter().flat_map(|formed| formed.slots.iter().flatten().copied())
    }

    /// Who `id` means on the field: whoever's there if it's the party's [`Self::input_actor`], and itself
    /// otherwise. What follows the player reads this, so a cast's first actor means the party.
    #[must_use]
    pub fn on_field(&self, id: ActorId) -> ActorId {
        if self.input_actor() == Some(id) { self.active().unwrap_or(id) } else { id }
    }

    /// Whether `id` is a member who isn't on the field. Off the field, nothing it does reaches the fight.
    #[must_use]
    pub fn is_off_field(&self, id: ActorId) -> bool {
        self.formed.is_some_and(|formed| formed.active_slot().actor != id && formed.holds(id))
    }

    /// Whether every member has fallen: the fight's over, as far as the simulation can tell.
    #[must_use]
    pub fn is_down(&self) -> bool {
        self.formed.is_some_and(|formed| formed.down)
    }

    /// Swaps made so far, those a defeat forced among them.
    #[must_use]
    pub fn swaps(&self) -> u32 {
        self.formed.map_or(0, |formed| formed.swaps)
    }

    /// Of those, the ones a defeat forced.
    #[must_use]
    pub fn forced_swaps(&self) -> u32 {
        self.formed.map_or(0, |formed| formed.forced_swaps)
    }

    /// The ability's numbers.
    #[must_use]
    pub fn tuning(&self) -> Option<PartyTuning> {
        self.formed.map(|formed| formed.tuning)
    }

    /// Whose input `id` reads this tick: what a tick names for it, or nothing, off the field.
    pub(crate) fn input_of(&self, id: ActorId) -> Option<ActorId> {
        let Some(formed) = &self.formed else { return Some(id) };
        if formed.active_slot().actor == id {
            self.input_actor()
        } else if formed.holds(id) {
            None
        } else {
            Some(id)
        }
    }

    /// Writes nothing with no party, so a fight without one has the checksums it always had but for the
    /// layout's number.
    pub(crate) fn write_canonical(&self, hasher: &mut StateHasher) {
        let Some(formed) = &self.formed else { return };
        let Formed { slots, active, tuning, swaps, forced_swaps, down } = *formed;
        for slot in slots {
            // 0 stands for none, which no actor's ID is.
            hasher.u32(slot.map_or(0, |slot| slot.actor.raw()));
            hasher.u64(slot.map_or(0, |slot| slot.ready_at.as_subticks()));
        }
        hasher.u8(active);
        tuning.write_canonical(hasher);
        hasher.u32(swaps);
        hasher.u32(forced_swaps);
        hasher.u8(u8::from(down));
    }

    /// What a run depends on: for `Simulation::tables_hash`, and nothing with no party, so a replay of a
    /// fight without one is stamped as it always was.
    pub(crate) fn write_tuning(&self, hasher: &mut StateHasher) {
        if let Some(formed) = &self.formed {
            formed.tuning.write_canonical(hasher);
        }
    }
}

impl PartyTuning {
    fn write_canonical(self, hasher: &mut StateHasher) {
        let Self { swap_cooldown, rest } = self;
        hasher.u32(swap_cooldown);
        hasher.u32(rest);
    }
}

/// What a swap reads and writes of a member.
type Member<'w> = (
    &'w mut EntityClock,
    &'w mut Controls,
    &'w mut ActorTransform,
    &'w mut MoveRunner,
    &'w mut IntentRing,
    &'w mut WarpTarget,
    &'w mut CharacterController,
    &'w mut Fighter,
);

/// What a query for [`Member`] hands over.
type MemberItem<'w> = (
    Mut<'w, EntityClock>,
    Mut<'w, Controls>,
    Mut<'w, ActorTransform>,
    Mut<'w, MoveRunner>,
    Mut<'w, IntentRing>,
    Mut<'w, WarpTarget>,
    Mut<'w, CharacterController>,
    Mut<'w, Fighter>,
);

/// What's due this step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Due {
    Nothing,
    /// A swap into this slot: a press's, or with `forced` a defeat's.
    Swap {
        to: u8,
        forced: bool,
    },
    /// The one on the field has fallen, and nobody's left to come in.
    PartyDown,
}

/// What's due this step, by the [module docs](self).
fn due(formed: &Formed, table: &ActorTable, actors: &Query<'_, '_, Member<'_>>) -> Due {
    let leaving = formed.active_slot();
    let member = table.entity(leaving.actor).and_then(|entity| actors.get(entity).ok());
    let forced = match member {
        // Fallen: when its reaction has played out, and whatever freeze it's under is over.
        Some((clock, _, _, runner, _, _, _, fighter)) if fighter.is_defeated() => {
            let frozen = clock.0.last_delta() == SimDuration::ZERO;
            if frozen || runner.is_running() || runner.forced.is_some() {
                return Due::Nothing;
            }
            true
        }
        Some((clock, _, _, _, ring, ..)) => {
            let now = clock.0.now();
            let can_leave = clock.0.last_delta() != SimDuration::ZERO && leaving.ready_at <= now;
            // The ring lets go of what's expired later in the step, so a press can still be here on the
            // first tick it can't fire.
            if !can_leave || !ring.iter().any(|press| press.name == SWAP_INTENT && press.expires_at > now) {
                return Due::Nothing;
            }
            false
        }
        // It's gone, which only a despawn does.
        None => true,
    };
    let can_come_in = |slot: PartySlot| {
        let (clock, .., fighter) = actors.get(table.entity(slot.actor)?).ok()?;
        Some(!fighter.is_defeated() && (forced || slot.ready_at <= clock.0.now()))
    };
    match formed.others().find(|&(_, slot)| can_come_in(slot) == Some(true)) {
        Some((to, _)) => Due::Swap { to, forced },
        None if forced => Due::PartyDown,
        None => Due::Nothing,
    }
}

/// Takes a member off the field: see the [module docs](self). It's the world's bodies that make it so.
fn leave_field(
    (_, mut controls, _, mut runner, mut ring, mut target, mut controller, mut fighter): MemberItem<'_>,
) {
    *controls = Controls::default();
    runner.running = None;
    runner.forced = None;
    runner.counter = None;
    ring.reset();
    *target = WarpTarget::None;
    controller.state = ControllerState::default();
    controller.drive = ControllerDrive::NONE;
    fighter.stop_slide();
}

/// What whoever comes in takes over from whoever leaves.
#[derive(Clone, Copy)]
struct Handover {
    place: ActorTransform,
    state: ControllerState,
    aim: WarpTarget,
    controls: Controls,
    /// A perfect dodge had scaled its clock back up.
    scaled_back: bool,
    /// Its clock's reading as it left.
    left_at: SimTime,
}

impl Handover {
    /// Takes what comes in from a member about to leave.
    fn from(member: &mut MemberItem<'_>) -> Self {
        let (clock, controls, transform, _, _, target, controller, _) = member;
        Self {
            place: **transform,
            state: ControllerState { events: ControllerEvents::default(), ..controller.state },
            aim: **target,
            controls: **controls,
            scaled_back: clock.0.remove_source(ScaleSource::PerfectDodge) > 0,
            left_at: clock.0.now(),
        }
    }

    /// Gives it to a member coming in.
    fn apply(&self, world_clock: &WorldClock, member: &mut MemberItem<'_>) {
        let (clock, controls, transform, _, _, target, controller, _) = member;
        **transform = self.place;
        controller.state = self.state;
        **target = self.aim;
        **controls = self.controls;
        clock.0.remove_source(ScaleSource::PerfectDodge);
        let slowed = world_clock.0.scales().find(|scale| scale.source == ScaleSource::PerfectDodge);
        if let (true, Some(slowed)) = (self.scaled_back, slowed) {
            // It has just taken any out, so only what else fills the stack can refuse this.
            let _ = clock.0.push_scale(ScaleSource::PerfectDodge, counter_scale(slowed.per_mille));
        }
    }
}

/// [`crate::CombatSet::Party`]: fires what's due. See the [module docs](self).
#[allow(clippy::too_many_arguments, reason = "a system's parameters read as what it touches")]
pub(crate) fn settle_swaps(
    mut party: ResMut<'_, Party>,
    table: Res<'_, ActorTable>,
    world_clock: Res<'_, WorldClock>,
    mut cues: ResMut<'_, CueQueue>,
    library: Res<'_, MoveLibrary>,
    mut characters: ResMut<'_, CharacterWorld>,
    mut actors: Query<'_, '_, Member<'_>>,
    mut brains: Query<'_, '_, &mut Brain>,
) {
    let Some(formed) = party.formed.filter(|formed| !formed.down) else { return };
    let tick = world_clock.0.ticks();
    let leaving = formed.active_slot().actor;
    let (to, forced) = match due(&formed, &table, &actors) {
        Due::Nothing => return,
        Due::PartyDown => {
            if let Some(formed) = &mut party.formed {
                formed.down = true;
            }
            let local_time = table
                .entity(leaving)
                .and_then(|entity| actors.get(entity).ok())
                .map_or(SimTime::ZERO, |(clock, ..)| clock.0.now());
            let cue = PARTY_DEFEATED;
            let move_time = SimDuration::ZERO;
            cues.push(CueEvent { actor: leaving, cue, tick, local_time, move_time, hit: None });
            return;
        }
        Due::Swap { to, forced } => (to, forced),
    };
    let Some(coming) = formed.slots[usize::from(to)] else { return };
    let Some(incoming) = table.entity(coming.actor) else { return };

    let handover = table.entity(leaving).and_then(|entity| actors.get_mut(entity).ok()).map(|mut member| {
        let handover = Handover::from(&mut member);
        leave_field(member);
        handover
    });
    characters.despawned(leaving);

    let Ok(mut member) = actors.get_mut(incoming) else { return };
    if let Some(handover) = &handover {
        handover.apply(&world_clock, &mut member);
    }
    let (clock, _, transform, mut runner, _, _, mut controller, _) = member;
    let now = clock.0.now();
    let state = controller.state;
    // Its own capsule and numbers, wherever it's standing: only the place and the motion carry over.
    let movement = runner.set().and_then(|set| library.movement(set));
    let spawned = controller.profile();
    *controller = characters.spawned(coming.actor, spawned, movement, &transform);
    controller.state = state;
    if let Some(numbers) = movement.or_else(|| spawned.and_then(|profile| characters.profile(profile))) {
        controller.state.velocity = numbers.carried(state.velocity);
    }
    // The assist starts at this step's reading: what's between that and the last one belongs to no move.
    runner.cursor = now;
    runner.force(SWAP_IN_MOVE);

    // Whoever aimed at the one who left, or was thinking about it, turns on this one now.
    for (_, entity) in table.iter() {
        if let Ok((.., mut target, _, _)) = actors.get_mut(entity)
            && *target == WarpTarget::Actor(leaving)
        {
            *target = WarpTarget::Actor(coming.actor);
        }
        if let Ok(mut brain) = brains.get_mut(entity)
            && brain.target == Some(leaving)
        {
            brain.target = Some(coming.actor);
        }
    }

    if let Some(formed) = &mut party.formed {
        let active = usize::from(formed.active);
        let rest = SimDuration::from_ticks(u64::from(formed.tuning.rest));
        let cooldown = SimDuration::from_ticks(u64::from(formed.tuning.swap_cooldown));
        if let (Some(slot), Some(handover)) = (&mut formed.slots[active], &handover) {
            slot.ready_at = handover.left_at + rest;
        }
        if let Some(slot) = &mut formed.slots[usize::from(to)] {
            slot.ready_at = now + cooldown;
        }
        formed.active = to;
        formed.swaps += 1;
        formed.forced_swaps += u32::from(forced);
    }
    let move_time = SimDuration::ZERO;
    cues.push(CueEvent { actor: coming.actor, cue: PARTY_SWAP, tick, local_time: now, move_time, hit: None });
}

/// Forms the party: its first member on the field, where it stands, and the others off it. See
/// `Simulation::form_party`.
pub(crate) fn form(world: &mut World, members: &[ActorId], tuning: PartyTuning) -> Result<(), PartyError> {
    if world.resource::<Party>().is_formed() {
        return Err(PartyError::AlreadyFormed);
    }
    if members.is_empty() || members.len() > PARTY_SLOTS {
        return Err(PartyError::BadSize { count: members.len() });
    }
    let mut slots = [None; PARTY_SLOTS];
    for (index, &actor) in members.iter().enumerate() {
        if members[..index].contains(&actor) {
            return Err(PartyError::Twice { actor });
        }
        let entity = world.resource::<ActorTable>().entity(actor).ok_or(PartyError::NotAlive { actor })?;
        let library = world.resource::<MoveLibrary>();
        let set: Option<&MoveSet> =
            world.get::<MoveRunner>(entity).and_then(MoveRunner::set).and_then(|set| library.current(set));
        let set = set.ok_or(PartyError::NoSwapIntent { actor })?;
        set.intent(SWAP_INTENT).ok_or(PartyError::NoSwapIntent { actor })?;
        set.find(SWAP_IN_MOVE).ok_or(PartyError::NoSwapInMove { actor })?;
        slots[index] = Some(PartySlot { actor, ready_at: SimTime::ZERO });
    }
    let mut query = world.query::<Member<'_>>();
    for &actor in &members[1..] {
        let entity = world.resource::<ActorTable>().entity(actor).ok_or(PartyError::NotAlive { actor })?;
        leave_field(query.get_mut(world, entity).expect("every pooled entity has all of these"));
        world.resource_mut::<CharacterWorld>().despawned(actor);
    }
    let formed = Formed { slots, active: 0, tuning, swaps: 0, forced_swaps: 0, down: false };
    world.resource_mut::<Party>().formed = Some(formed);
    Ok(())
}
