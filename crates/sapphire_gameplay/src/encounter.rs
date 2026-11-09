//! A zone's triggers, and the director of its encounters (spec §5.22, M3 B2).
//!
//! [`crate::Simulation::set_zone`] takes a cooked zone and copies what it reads into [`SimZone`], so nothing
//! here reads an archive again and nothing here streams: a zone's triggers and encounters are all there
//! before the first step, and they stay. Then, each step, ahead of the brains:
//!
//! - **Triggers** ([`SimZone`]'s, from `.szone` data) watch whoever has a soft lock on, a player by any other
//!   name. One fires each time someone goes in with nobody inside, and an encounter whose `starts_on` is its
//!   event starts.
//! - **The director** (`direct`) brings a running encounter's waves on when their rules say. A spawn goes
//!   through [`crate::ActorRequests`], so it stands from the end of this step, and the step after, the new
//!   actor gets its brain, which draws from the simulation's RNG in spawn order like any other's.
//! - **Tokens** decide who may press. A brain of an encounter's has to hold one of its tokens to start an
//!   attack: without one its patience stays run out and it keeps pacing. It gives the token back when its
//!   attack is over for whatever reason, and that token rests for `token_rest` ticks. Slots are taken from the
//!   lowest and brains ask in ID order, so nothing is drawn and nothing depends on an address.
//!
//! An actor nobody spawned here has no director, so a fight without a zone runs as it did and hashes nothing
//! more. Everything here is sized when the zone is given, so a step allocates nothing.
//!
//! **Who moves whom:** a spawner's `spawns` is a move set's name, which [`ZoneCast`] says the ID of. The zone's
//! digest, those names and the cooked encounters go in [`crate::Simulation::tables_hash`], and the director's
//! state in [`crate::Simulation::checksum`].
//!
//! A trigger's test is the actor's root in the volume, axis-aligned, with no rotation: a zone's document
//! has none to give. It costs a soft lock's test against every trigger, each step.

use sapphire_colliders::ColliderSet;
use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::MoveSet;
use sapphire_physics::{PhysicsWorld, WorldError};
use sapphire_serialize::AssetId;
use sapphire_zone::{
    ArchivedEncounter, ArchivedWaveStart, ArchivedZone, CookedVolume, Encounter, MAX_TOKENS, ZoneError,
};

use crate::actors::{ActorId, ActorRequests, ActorSpawn, ActorTable};
use crate::brain::Brain;
use crate::checksum::StateHasher;
use crate::controller::{PhysicsWorldError, ProfileId};
use crate::faction::Faction;
use crate::movement::ActorTransform;
use crate::moves::{MoveLibrary, MoveRunner, MoveSetId};
use crate::party::Party;
use crate::resolve::Fighter;
use crate::simulation::SimRng;
use crate::targeting::SoftLock;

/// What a zone's spawners and encounters stand on, from whoever gives a simulation the zone.
#[derive(Clone, Copy, Debug)]
pub struct ZoneCast<'a> {
    /// The move sets a spawner's `spawns` may name: the name, and the ID it has here.
    pub moves: &'a [(&'a str, MoveSetId)],
    /// What an encounter's fighters move by, when their documents don't say.
    pub controller: Option<ProfileId>,
}

/// Why a simulation couldn't take a zone. It stays as it was.
#[derive(Debug, thiserror::Error)]
pub enum ZoneStartError {
    #[error(transparent)]
    Zone(#[from] ZoneError),
    #[error(transparent)]
    Colliders(#[from] WorldError),
    #[error(transparent)]
    ColliderSet(#[from] sapphire_colliders::ColliderError),
    #[error(transparent)]
    World(#[from] PhysicsWorldError),
    /// The zone's spawner spawns a set of moves nobody said the ID of.
    #[error("spawner `{spawner}`: nothing says what `{spawns}` is")]
    UnknownMoves { spawner: String, spawns: String },
    /// A director can't give an actor a brain whose moves don't say one.
    #[error("spawner `{spawner}`: `{spawns}` has no brain, and an encounter's fighters need one")]
    NoBrain { spawner: String, spawns: String },
    /// One more than a brain's place in a list of them can say.
    #[error("{count} encounters are more than a zone can hold")]
    TooManyEncounters { count: usize },
}

/// Where a zone's trigger stands: what starts an encounter, for presentation to warm ahead of (M3 W5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StartVolume {
    /// Its centre, in the zone's space.
    pub position: [f32; 3],
    /// Axis-aligned about [`Self::position`], as a trigger tests it.
    pub volume: CookedVolume,
}

/// How far along an encounter is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncounterPhase {
    /// Its trigger hasn't fired.
    #[default]
    Waiting,
    /// Its fighters are on their way, or there.
    Running,
    /// Every wave has come and all its fighters are down. Nothing ends this.
    Cleared,
}

/// What a zone's triggers and encounters have done in a run, for a bench and a test. Not in the checksum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneStats {
    pub triggers_fired: u64,
    pub encounters_started: u64,
    pub encounters_cleared: u64,
    /// Fighters spawned, and those a full pool refused.
    pub spawned: u64,
    pub spawns_refused: u64,
    pub tokens_taken: u64,
    /// Brain-ticks spent ready to attack with no token to do it with.
    pub tokens_waited: u64,
    /// The most tokens any one encounter has had out at once.
    pub most_tokens_held: u32,
}

#[derive(Clone, Copy, Debug)]
struct SimTrigger {
    position: [f32; 3],
    volume: CookedVolume,
    event: u64,
    inside: bool,
    /// Scratch for a step: whether someone's been found in it. Never read across steps.
    found: bool,
    fires: u32,
}

impl SimTrigger {
    /// Whether a root stands in it. Plain comparisons, in scalars.
    fn holds(&self, at: Vec3) -> bool {
        let (x, y, z) = (at.x - self.position[0], at.y - self.position[1], at.z - self.position[2]);
        match self.volume {
            CookedVolume::Box { half_extents } => {
                x.abs() <= half_extents[0] && y.abs() <= half_extents[1] && z.abs() <= half_extents[2]
            }
            CookedVolume::Sphere { radius } => x * x + y * y + z * z <= radius * radius,
        }
    }
}

/// When a wave comes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaveRule {
    Start,
    Remaining(u32),
    After(u64),
}

#[derive(Clone, Copy, Debug)]
struct SimWave {
    rule: WaveRule,
    gap: u64,
    /// One past its last spawn, in [`SimEncounter::spawns`].
    end: u32,
}

/// A token's place: who has it, or when it's ready for the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TokenSlot {
    holder: Option<ActorId>,
    ready_at: u64,
}

/// One encounter: what its document said, then where it's got to.
#[derive(Clone, Debug)]
pub struct SimEncounter {
    starts_on: u64,
    tokens: u8,
    token_rest: u64,
    waves: Box<[SimWave]>,
    spawns: Box<[ActorSpawn]>,

    phase: EncounterPhase,
    /// How many waves have come.
    waves_begun: u32,
    /// How many of [`Self::spawns`] have been asked for.
    spawned: u32,
    /// When the wave last begun began.
    wave_at: u64,
    /// When the next of this wave's spawns may go.
    spawn_at: u64,
    /// Who each of [`Self::spawns`] became: `None` for one not yet asked for, or that a full pool refused.
    fighters: Box<[Option<ActorId>]>,
    /// How many of [`Self::fighters`] have been given their brains.
    woken: u32,
    slots: [TokenSlot; MAX_TOKENS as usize],
}

impl SimEncounter {
    fn new(
        cooked: &ArchivedEncounter,
        zone: &ArchivedZone,
        cast: ZoneCast<'_>,
        library: &MoveLibrary,
    ) -> Result<Self, ZoneStartError> {
        cooked.check_in(&zone.simulation)?;
        let faction = Faction::new(cooked.faction).expect("`Encounter::read` checked its range");
        let mut waves = Vec::with_capacity(cooked.waves.len());
        let mut spawns = Vec::new();
        for wave in cooked.waves.iter() {
            for name in wave.spawners.iter() {
                let spawner = zone.simulation.spawner(Name::new(name.as_str())).expect("`check_in` found it");
                let spawns_name = spawner.spawns.as_str();
                let describe = || (name.as_str().to_owned(), spawns_name.to_owned());
                let moves = cast.moves.iter().find(|(kind, _)| *kind == spawns_name).map(|&(_, id)| id);
                let Some(moves) = moves else {
                    let (spawner, spawns) = describe();
                    return Err(ZoneStartError::UnknownMoves { spawner, spawns });
                };
                if library.current(moves).and_then(MoveSet::brain).is_none() {
                    let (spawner, spawns) = describe();
                    return Err(ZoneStartError::NoBrain { spawner, spawns });
                }
                spawns.push(ActorSpawn {
                    position: Vec3::from_array(spawner.position()),
                    yaw: spawner.yaw_degrees.to_native().to_radians(),
                    moves: Some(moves),
                    controller: cast.controller,
                    faction,
                });
            }
            waves.push(SimWave {
                rule: match wave.when {
                    ArchivedWaveStart::Start => WaveRule::Start,
                    ArchivedWaveStart::Remaining(left) => WaveRule::Remaining(u32::from(left.to_native())),
                    ArchivedWaveStart::After(ticks) => WaveRule::After(u64::from(ticks.to_native())),
                },
                gap: u64::from(wave.gap.to_native()),
                end: spawns.len() as u32,
            });
        }
        Ok(Self {
            starts_on: Name::new(cooked.starts_on.as_str()).as_u64(),
            tokens: cooked.tokens,
            token_rest: u64::from(cooked.token_rest.to_native()),
            waves: waves.into_boxed_slice(),
            fighters: vec![None; spawns.len()].into_boxed_slice(),
            spawns: spawns.into_boxed_slice(),
            phase: EncounterPhase::Waiting,
            waves_begun: 0,
            spawned: 0,
            wave_at: 0,
            spawn_at: 0,
            woken: 0,
            slots: [TokenSlot::default(); MAX_TOKENS as usize],
        })
    }

    /// How far along it is.
    #[must_use]
    pub const fn phase(&self) -> EncounterPhase {
        self.phase
    }

    /// Who it's spawned so far, in the order they were asked for.
    pub fn fighters(&self) -> impl Iterator<Item = ActorId> + '_ {
        self.fighters.iter().flatten().copied()
    }

    /// How many fighters all its waves bring.
    #[must_use]
    pub fn planned(&self) -> usize {
        self.spawns.len()
    }

    /// How many of [`Self::planned`] have been asked for: those still to come are the rest.
    #[must_use]
    pub const fn spawned(&self) -> u32 {
        self.spawned
    }

    /// How many waves have come.
    #[must_use]
    pub const fn waves_begun(&self) -> u32 {
        self.waves_begun
    }

    /// How many of its tokens are out now.
    #[must_use]
    pub fn tokens_held(&self) -> u32 {
        self.slots[..usize::from(self.tokens)].iter().filter(|slot| slot.holder.is_some()).count() as u32
    }

    fn holds(&self, actor: ActorId) -> bool {
        self.slots[..usize::from(self.tokens)].iter().any(|slot| slot.holder == Some(actor))
    }

    fn free_slot(&self, tick: u64) -> Option<usize> {
        self.slots[..usize::from(self.tokens)]
            .iter()
            .position(|slot| slot.holder.is_none() && tick >= slot.ready_at)
    }

    /// Whether `actor` could start an attack now.
    fn may_attack(&self, actor: ActorId, tick: u64) -> bool {
        self.holds(actor) || self.free_slot(tick).is_some()
    }

    /// Brings the slots up to what a brain did: one that's attacking has a token, and one that isn't, none.
    fn settle(&mut self, actor: ActorId, attacking: bool, tick: u64, stats: &mut ZoneStats) {
        let tokens = usize::from(self.tokens);
        if attacking {
            if !self.holds(actor)
                && let Some(free) = self.free_slot(tick)
            {
                self.slots[free].holder = Some(actor);
                stats.tokens_taken += 1;
                stats.most_tokens_held = stats.most_tokens_held.max(self.tokens_held());
            }
        } else if let Some(slot) = self.slots[..tokens].iter_mut().find(|slot| slot.holder == Some(actor)) {
            *slot = TokenSlot { holder: None, ready_at: tick + self.token_rest };
        }
    }

    /// Whether the wave after the last begun may come. It waits for the one before to have finished arriving.
    fn next_wave_is_due(&self, tick: u64, standing: u32) -> bool {
        let Some(wave) = self.waves.get(self.waves_begun as usize) else { return false };
        let last_end = self.waves_begun.checked_sub(1).map_or(0, |last| self.waves[last as usize].end);
        if self.spawned < last_end {
            return false;
        }
        match wave.rule {
            WaveRule::Start => true,
            WaveRule::Remaining(left) => standing <= left,
            WaveRule::After(ticks) => tick >= self.wave_at + ticks,
        }
    }

    /// Takes back a token whose holder's gone, whom no brain's turn will ever settle.
    fn take_back_tokens(&mut self, table: &ActorTable, tick: u64) {
        let rest = self.token_rest;
        for slot in &mut self.slots[..usize::from(self.tokens)] {
            if slot.holder.is_some_and(|holder| table.entity(holder).is_none()) {
                *slot = TokenSlot { holder: None, ready_at: tick + rest };
            }
        }
    }

    fn write_state(&self, hasher: &mut StateHasher) {
        hasher.u8(self.phase as u8);
        hasher.u32(self.waves_begun);
        hasher.u32(self.spawned);
        hasher.u64(self.wave_at);
        hasher.u64(self.spawn_at);
        for fighter in &self.fighters {
            hasher.u32(fighter.map_or(0, ActorId::raw));
        }
        hasher.u32(self.woken);
        for slot in &self.slots {
            hasher.u32(slot.holder.map_or(0, ActorId::raw));
            hasher.u64(slot.ready_at);
        }
    }
}

/// A zone as a simulation holds it. See the [module docs](self).
#[derive(Resource, Clone, Debug)]
pub struct SimZone {
    tables: u64,
    triggers: Box<[SimTrigger]>,
    encounters: Box<[SimEncounter]>,
    stats: ZoneStats,
}

impl SimZone {
    /// What the zone says, for a simulation to hold. Its colliders are [`Self::physics_of`]'s.
    pub(crate) fn new<'a>(
        zone: &ArchivedZone,
        cast: ZoneCast<'_>,
        library: &MoveLibrary,
        mut read: impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<Self, ZoneStartError> {
        let simulation = &zone.simulation;
        let digest = zone.digest_without_navigation(&mut read)?;
        if simulation.encounters.len() > usize::from(u8::MAX) {
            return Err(ZoneStartError::TooManyEncounters { count: simulation.encounters.len() });
        }
        let triggers = simulation
            .triggers
            .iter()
            .map(|trigger| SimTrigger {
                position: trigger.position(),
                volume: trigger.volume.to_native(),
                event: Name::new(trigger.event.as_str()).as_u64(),
                inside: false,
                found: false,
                fires: 0,
            })
            .collect();
        let mut encounters = Vec::with_capacity(simulation.encounters.len());
        for id in simulation.encounters.iter() {
            let bytes = read(*id).ok_or(ZoneError::NotLoaded { what: "encounter", id: *id })?;
            encounters.push(SimEncounter::new(Encounter::read(bytes)?, zone, cast, library)?);
        }
        // Which names mean which sets is part of what a run depends on, and the digest can't know it.
        let mut hasher = StateHasher::new();
        hasher.u64(digest);
        for (name, id) in cast.moves {
            hasher.bytes(name.as_bytes());
            hasher.u32(id.0);
        }
        Ok(Self {
            tables: hasher.finish(),
            triggers,
            encounters: encounters.into_boxed_slice(),
            stats: ZoneStats::default(),
        })
    }

    /// The physics world a zone's `colliders` make, if it names any.
    pub(crate) fn physics_of<'a>(
        zone: &ArchivedZone,
        capacity: usize,
        read: &mut impl FnMut(AssetId) -> Option<&'a [u8]>,
    ) -> Result<Option<PhysicsWorld>, ZoneStartError> {
        let Some(id) = zone.simulation.colliders.as_ref().copied() else { return Ok(None) };
        let bytes = read(id).ok_or(ZoneError::NotLoaded { what: "collider set", id })?;
        Ok(Some(PhysicsWorld::from_cooked(ColliderSet::read(bytes)?, capacity)?))
    }

    /// Its encounters, in the zone's order, which is by ID.
    #[must_use]
    pub fn encounters(&self) -> &[SimEncounter] {
        &self.encounters
    }

    /// Where encounter `index` starts: the first trigger whose event is its `starts_on`, the trigger that
    /// fires it first. `None` for an encounter that no trigger here starts, or past the end.
    #[must_use]
    pub fn start_volume(&self, index: usize) -> Option<StartVolume> {
        let encounter = self.encounters.get(index)?;
        let trigger = self.triggers.iter().find(|trigger| trigger.event == encounter.starts_on)?;
        Some(StartVolume { position: trigger.position, volume: trigger.volume })
    }

    /// How many fighters all its encounters bring: room a pool has to keep.
    #[must_use]
    pub fn planned(&self) -> usize {
        self.encounters.iter().map(SimEncounter::planned).sum()
    }

    #[must_use]
    pub const fn stats(&self) -> ZoneStats {
        self.stats
    }

    /// What goes in a simulation's `tables_hash`.
    pub(crate) const fn tables(&self) -> u64 {
        self.tables
    }

    /// Whether a brain of encounter `index` may start an attack now.
    pub(crate) fn may_attack(&self, index: u8, actor: ActorId, tick: u64) -> bool {
        self.encounters[usize::from(index)].may_attack(actor, tick)
    }

    /// What a brain of encounter `index` did with its tick.
    pub(crate) fn settle(&mut self, index: u8, actor: ActorId, attacking: bool, waiting: bool, tick: u64) {
        self.stats.tokens_waited += u64::from(waiting);
        self.encounters[usize::from(index)].settle(actor, attacking, tick, &mut self.stats);
    }

    pub(crate) fn write_state(&self, hasher: &mut StateHasher) {
        for trigger in &self.triggers {
            hasher.u8(u8::from(trigger.inside));
            hasher.u32(trigger.fires);
        }
        for encounter in &self.encounters {
            encounter.write_state(hasher);
        }
    }

    /// Runs each trigger against who's in it, and starts what its firing starts.
    fn watch_triggers(
        &mut self,
        tick: u64,
        table: &ActorTable,
        party: &Party,
        watched: &Query<'_, '_, (&ActorTransform, &SoftLock)>,
    ) {
        for trigger in &mut self.triggers {
            trigger.found = false;
        }
        for (id, entity) in table.iter() {
            let Ok((transform, lock)) = watched.get(entity) else { continue };
            if !lock.on || party.is_off_field(id) {
                continue;
            }
            for trigger in &mut self.triggers {
                trigger.found |= trigger.holds(transform.position);
            }
        }
        for trigger in &mut self.triggers {
            let entered = trigger.found && !trigger.inside;
            trigger.inside = trigger.found;
            if !entered {
                continue;
            }
            trigger.fires = trigger.fires.saturating_add(1);
            self.stats.triggers_fired += 1;
            for encounter in &mut self.encounters {
                if encounter.phase == EncounterPhase::Waiting && encounter.starts_on == trigger.event {
                    encounter.phase = EncounterPhase::Running;
                    encounter.wave_at = tick;
                    self.stats.encounters_started += 1;
                }
            }
        }
    }
}

/// [`crate::CombatSet::Intents`], ahead of the soft locks and the brains: the triggers, then each running
/// encounter, as the [module docs](self) say.
#[allow(clippy::too_many_arguments, reason = "a system's parameters read as what it touches")]
pub(crate) fn direct(
    zone: Option<ResMut<'_, SimZone>>,
    table: Res<'_, ActorTable>,
    party: Res<'_, Party>,
    library: Res<'_, MoveLibrary>,
    clock: Res<'_, WorldClock>,
    mut requests: ResMut<'_, ActorRequests>,
    mut rng: ResMut<'_, SimRng>,
    watched: Query<'_, '_, (&ActorTransform, &SoftLock)>,
    fighters: Query<'_, '_, &Fighter>,
    mut minds: Query<'_, '_, (&mut Brain, &EntityClock, &MoveRunner)>,
) {
    let Some(mut zone) = zone else { return };
    let zone = &mut *zone;
    let tick = clock.0.ticks();
    zone.watch_triggers(tick, &table, &party, &watched);

    for (index, encounter) in zone.encounters.iter_mut().enumerate() {
        if encounter.phase != EncounterPhase::Running {
            continue;
        }
        encounter.take_back_tokens(&table, tick);
        // Last step's spawns stand now. Their brains come in spawn order, which is the order of their draws.
        while encounter.woken < encounter.spawned {
            let fighter = encounter.fighters[encounter.woken as usize];
            encounter.woken += 1;
            let Some(entity) = fighter.and_then(|actor| table.entity(actor)) else { continue };
            let Ok((mut brain, actor_clock, runner)) = minds.get_mut(entity) else { continue };
            let def = runner.set().and_then(|set| library.current(set)).and_then(MoveSet::brain);
            if let Some(def) = def {
                *brain = Brain::fighting(def, actor_clock.0.now(), &mut rng.0);
                brain.encounter = Some(index as u8);
            }
        }

        let standing = encounter
            .fighters()
            .filter_map(|actor| table.entity(actor))
            .filter(|&entity| fighters.get(entity).is_ok_and(|fighter| !fighter.is_defeated()))
            .count() as u32;
        if encounter.next_wave_is_due(tick, standing) {
            encounter.waves_begun += 1;
            encounter.wave_at = tick;
            encounter.spawn_at = tick;
        }
        let end = encounter.waves_begun.checked_sub(1).map_or(0, |last| encounter.waves[last as usize].end);
        let gap = encounter.waves_begun.checked_sub(1).map_or(0, |last| encounter.waves[last as usize].gap);
        while encounter.spawned < end && tick >= encounter.spawn_at {
            let at = encounter.spawned as usize;
            encounter.spawned += 1;
            match requests.spawn(encounter.spawns[at]) {
                Ok(actor) => {
                    encounter.fighters[at] = Some(actor);
                    zone.stats.spawned += 1;
                }
                Err(_) => zone.stats.spawns_refused += 1,
            }
            encounter.spawn_at = tick + gap;
            if gap > 0 {
                break;
            }
        }

        let all_came = encounter.waves_begun as usize == encounter.waves.len()
            && encounter.spawned as usize == encounter.spawns.len();
        if all_came && encounter.woken == encounter.spawned && standing == 0 {
            encounter.phase = EncounterPhase::Cleared;
            zone.stats.encounters_cleared += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{SCENARIOS, find_scenario};
    use crate::simulation::Simulation;

    /// What's changed, and how.
    type Flip = (&'static str, fn(&mut SimZone));

    /// A fight well into its waves, with a token out.
    fn running() -> Simulation {
        let scenario = find_scenario(&SCENARIOS, "arena-waves").expect("listed");
        let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
        while sim.zone().expect("it has one").encounters()[0].tokens_held() == 0 {
            sim.step(&crate::TickActions::NEUTRAL);
        }
        sim
    }

    /// Every field of what a director does goes in the checksum, and what only reports on it doesn't.
    #[test]
    fn the_checksum_sees_what_the_director_is_doing() {
        let flips: [Flip; 12] = [
            ("a trigger's inside", |zone| zone.triggers[0].inside ^= true),
            ("a trigger's fires", |zone| zone.triggers[0].fires += 1),
            ("the phase", |zone| zone.encounters[0].phase = EncounterPhase::Cleared),
            ("waves begun", |zone| zone.encounters[0].waves_begun += 1),
            ("spawned", |zone| zone.encounters[0].spawned += 1),
            ("the wave's start", |zone| zone.encounters[0].wave_at += 1),
            ("the next spawn", |zone| zone.encounters[0].spawn_at += 1),
            ("who a fighter is", |zone| zone.encounters[0].fighters[6] = ActorId::from_raw(99)),
            ("woken", |zone| zone.encounters[0].woken += 1),
            ("a token's holder", |zone| zone.encounters[0].slots[1].holder = ActorId::from_raw(99)),
            ("a token's rest", |zone| zone.encounters[0].slots[1].ready_at += 1),
            ("a token past the tokens", |zone| zone.encounters[0].slots[7].ready_at += 1),
        ];
        let plain = running().checksum();
        assert_eq!(plain, running().checksum());
        for (what, flip) in flips {
            let mut sim = running();
            flip(&mut sim.resource_mut::<SimZone>());
            assert_ne!(sim.checksum(), plain, "{what}");
        }
        let mut sim = running();
        sim.resource_mut::<SimZone>().stats.tokens_waited += 1;
        assert_eq!(sim.checksum(), plain, "the stats report");
    }

    /// A brain's place in the zone's encounters goes with it.
    #[test]
    fn the_checksum_sees_whose_a_brain_is() {
        let sim = running();
        let plain = sim.checksum();
        let fighter = sim.zone().expect("it has one").encounters()[0].fighters().next().expect("came");
        let mut sim = running();
        let entity = sim.world().resource::<ActorTable>().entity(fighter).expect("stands");
        sim.ecs_mut().world_mut().get_mut::<Brain>(entity).expect("thinks").encounter = Some(1);
        assert_ne!(sim.checksum(), plain);
    }
}
