//! Going from one zone to another on a fight's stage (M3 W4): the load under a wipe, the swap between two
//! steps, and the hub's prefetch after.
//!
//! A trip starts from a replay's due entry ([`Combat::due_zone_entry`]) or from the game asking
//! ([`Fight::take_exit`]), and goes through [`TripPhase`]'s phases, one frame or more each:
//!
//! 1. **Covering.** The next zone is asked for, hurried, and the simulation holds: no tick runs from here
//!    until the swap, so how long the load takes reaches nothing the fixed step reads. A wipe drawn over the
//!    stage says when it covers the screen ([`FightShow::say_trip_wipe`]); with none, the swap comes at once.
//! 2. **Swapping.** When the zone is ready, its pack mounted, its pipelines built and the meshes around
//!    where the traveller arrives on the renderer (`Scene::zone_load`), the simulation enters it
//!    ([`Combat::enter_zone`]), the stage arrives in it (`Scene::arrive_in_zone`), which gives back what the
//!    last zone held, and the fighters stand where the simulation put them, the camera cut there.
//! 3. **Arriving.** Until the new zone's first cells are placed and the renderer's mesh pool has given back
//!    the room the last zone held, down to its floor (M5 D9: `Scene::zone_arrived`). Then the zone's own
//!    `prefetch` exits come in behind it, which is what makes it a hub.
//! 4. **Uncovering.** Steps run again, and the trip's over when the wipe says the screen's clear.
//!
//! The enemy, who a zone entry leaves behind, is drawn nowhere after one. The zone arrived in brings its look
//! (M5 M6): its day, light rigs, probe grid, sky, wind, weather and fog, which came in with its prefetch, or the
//! stage's own for what it names none of.
//!
//! A game's restart after a trip ([`Fight::start_run`]) starts the new run in its own zone, where the stage
//! no longer is: a trip home takes the stage back there, its simulation held at its first tick until the
//! stage has arrived. That trip is the stage's alone: the simulation is already there. A run stood in the
//! town's square can't be taken back to it, since a zone's arrival leaves the town.
//!
//! [`Combat::due_zone_entry`]: crate::combat::Combat::due_zone_entry
//! [`Combat::enter_zone`]: crate::combat::Combat::enter_zone

use std::sync::Arc;
use std::time::Duration;

use sapphire_assets::{AssetId, UnmountReport};
use sapphire_colliders::ColliderSet;
use sapphire_core::Name;
use sapphire_gameplay::zone::ZoneId;
use sapphire_physics::PhysicsWorld;
use sapphire_stage::content::Content;
use sapphire_stage::scene::{Scene, ZoneLoad, ZoneTarget};
use sapphire_time::Stopwatch;
use sapphire_world::streaming::StreamSettings;

use super::{Fight, FightShow};
use crate::LOG_TARGET;

/// Where a trip has got: see the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TripPhase {
    Covering,
    Swapping,
    Arriving,
    Uncovering,
}

impl TripPhase {
    /// Whether the simulation waits.
    #[must_use]
    pub fn holds_simulation(self) -> bool {
        self != Self::Uncovering
    }
}

/// What a trip came to, as the log says it: the load's cost, cold or warm, for Z1's budgets.
#[derive(Clone, Debug)]
pub struct TripReport {
    pub zone: AssetId,
    /// The zone was in, ready to arrive in, before anyone went: a hub had it coming.
    pub warm: bool,
    /// From asking to the zone being ready to arrive in: its pack mounted, its pipelines built, it and the
    /// meshes around where it's arrived at loaded. Nothing for a warm one.
    pub load: Duration,
    /// Frames drawn while it loaded.
    pub frames: u32,
    /// From asking to the swap, which also waits for the wipe to cover.
    pub swapped_after: Duration,
    /// From the swap to the first cells placed.
    pub settle: Duration,
    /// What the last zone gave back.
    pub left: Option<UnmountReport>,
    /// What went wrong, if anything did: the stage kept the last zone, or the replay's zone isn't its own.
    pub error: Option<String>,
}

/// A trip under way.
struct Trip {
    /// Where it goes. One a replay's entry starts has no pack named yet: see [`Self::entered`].
    target: ZoneTarget,
    /// The simulation's zone a replay's entry started it for, until its first frame names the target's pack
    /// from that zone's ground: a fixed step allocates nothing, and a pack's name is a `String`.
    entered: Option<ZoneId>,
    phase: TripPhase,
    /// It's been asked of the stage.
    asked: bool,
    /// It goes to the zone already on stage, as a replay played again over a stage does: the stage only
    /// stands the travellers anew.
    same: bool,
    /// Only the stage goes: a restarted run is already in the zone, which the simulation doesn't enter.
    stage_only: bool,
    warm: bool,
    started: Stopwatch,
    frames: u32,
    /// When it was ready, from asking.
    ready: Option<Duration>,
    load: Duration,
    swapped: Option<Stopwatch>,
    left: Option<UnmountReport>,
    error: Option<String>,
}

/// What a fight's stage knows of trips: the one under way, and what a wipe over it says.
#[derive(Default)]
pub(super) struct Trips {
    trip: Option<Trip>,
    /// A wipe is drawn over the stage, and says when it covers and when it's clear. With none, a trip doesn't
    /// wait for either.
    wiped: bool,
    covered: bool,
    clear: bool,
    /// How a zone streams on this stage's tier.
    pub(super) settings: Option<StreamSettings>,
    /// The real time given to the camera during a trip since the wipe last took it.
    pending: f32,
    last: Option<TripReport>,
    /// Where a run of this fight starts on the stage, as [`Fight::stand_prepared`] stood it.
    pub(super) home: Option<Home>,
}

/// Where a run of a fight starts on its stage: what a restart's trip home goes back to.
#[derive(Clone, Debug)]
pub(super) enum Home {
    /// A zone, arrived at as its first run stood.
    Zone(ZoneTarget),
    /// The town's square, and the zone the stage streamed there, which no trip comes back to.
    Town(Option<AssetId>),
}

impl Trips {
    pub(super) fn holds_simulation(&self) -> bool {
        self.trip.as_ref().is_some_and(|trip| trip.phase.holds_simulation())
    }

    pub(super) fn add_time(&mut self, dt: f32) {
        if self.wiped && self.trip.is_some() {
            self.pending += dt;
        }
    }

    /// Starts a trip to `target`, unless one to the same spawner, of the same kind, is under way. `entered` is
    /// the simulation's zone a replay's entry names, whose pack `target` doesn't have yet.
    fn begin(&mut self, target: ZoneTarget, stage_only: bool, entered: Option<ZoneId>) {
        let same = |trip: &Trip| {
            (trip.target.zone, trip.target.arrive, trip.stage_only)
                == (target.zone, target.arrive, stage_only)
        };
        if self.trip.as_ref().is_some_and(|trip| same(trip) && trip.phase.holds_simulation()) {
            return;
        }
        self.trip = Some(Trip {
            target,
            entered,
            phase: TripPhase::Covering,
            asked: false,
            same: false,
            stage_only,
            warm: false,
            started: Stopwatch::start(),
            frames: 0,
            ready: None,
            load: Duration::ZERO,
            swapped: None,
            left: None,
            error: None,
        });
    }
}

impl FightShow {
    /// Where the trip under way has got, if there is one.
    #[must_use]
    pub fn trip_phase(&self) -> Option<TripPhase> {
        self.trips.trip.as_ref().map(|trip| trip.phase)
    }

    /// What the last trip came to.
    #[must_use]
    pub fn last_trip(&self) -> Option<&TripReport> {
        self.trips.last.as_ref()
    }

    /// How a zone streams on this stage's tier, for the zones a trip brings in: what [`Fight::stand`] sets.
    pub fn set_zone_streaming(&mut self, settings: StreamSettings) {
        self.trips.settings = Some(settings);
    }

    /// Says a wipe is drawn over this stage from now on, and trips wait for it to cover and to clear.
    pub fn take_trip_wipe(&mut self) {
        self.trips.wiped = true;
    }

    /// The real time the camera's been given since the last call: what a wipe's clock runs on, which in a
    /// capture is the steps' time, so a frame at a tick shows the same wipe every run.
    pub fn take_trip_time(&mut self) -> f32 {
        std::mem::take(&mut self.trips.pending)
    }

    /// Says whether the wipe covers the screen, and whether it's clear, as of its last frame.
    pub fn say_trip_wipe(&mut self, covered: bool, clear: bool) {
        self.trips.covered = covered;
        self.trips.clear = clear;
    }
}

impl Fight {
    /// Whether this step of the simulation waits: a timeline, a trip or an open conversation holds it. A
    /// replay's zone entry due before the step starts a trip, which holds it. Every loop that steps a fight
    /// asks this first. Allocates nothing: the trip's first frame names the zone's pack.
    pub fn holds_step(&mut self) -> bool {
        if !self.show.trips.holds_simulation()
            && let Some(combat) = &mut self.combat
            && let Some(entry) = combat.due_zone_entry()
        {
            let simulation = combat.simulation();
            let target = simulation.zone_ground(entry.zone).and_then(|zone| {
                let arrive = zone.spawner_id(entry.spawner)?;
                Some(ZoneTarget { zone: zone.asset(), pack: String::new(), arrive })
            });
            match target {
                Some(target) => self.begin_trip(target, false, Some(entry.zone)),
                // A crafted file: the step enters it, or refuses, as a headless run does.
                None => tracing::error!(target: LOG_TARGET, ?entry, "a zone entry names no zone"),
            }
        }
        self.show.holds_simulation() || self.dialogue.is_open()
    }

    /// Whether this fight goes between zones: its simulation has a zone on its shelf, as a replay of the
    /// `zone-trip` scenario does. What decides whether a stage loads a wipe.
    #[must_use]
    pub fn travels(&self) -> bool {
        self.combat
            .as_ref()
            .is_some_and(|combat| combat.simulation().zone_ground(ZoneId::from_raw(0)).is_some())
    }

    /// Goes through the zone on stage's exit `name`, as the game's flow asks (G1): a trip to where it leads.
    /// A replay takes its own, and nothing else.
    pub fn take_exit(&mut self, scene: &Scene, content: &mut Content, name: &str) -> Result<(), String> {
        if self.combat.as_ref().is_some_and(|combat| combat.ticks_left().is_some()) {
            return Err("a replay goes where it recorded".to_owned());
        }
        let target = scene
            .zone_exit(Name::new(name).as_u64(), content)
            .ok_or_else(|| format!("the zone on stage has no exit `{name}`"))?;
        self.begin_trip(target, false, None);
        Ok(())
    }

    fn begin_trip(&mut self, target: ZoneTarget, stage_only: bool, entered: Option<ZoneId>) {
        self.show.trips.begin(target, stage_only, entered);
        self.show.draws_latest = true;
    }

    /// Takes the stage back to where a run of this fight starts, for the run just attached, whose simulation is
    /// there and hasn't stepped ([`Fight::start_run`]). A trip under way is the last run's, and goes no
    /// further. One home starts from the latest checkpoint the run stands at, or where its first run stood.
    /// Nothing if the stage is there already, or nothing's known of where that is.
    pub(super) fn go_home(&mut self, scene: &Scene) {
        if self.show.trips.trip.take().is_some() {
            self.show.draws_latest = false;
        }
        let on_stage = scene.zone_on_stage();
        match self.show.trips.home.clone() {
            None => {}
            Some(Home::Zone(home)) if on_stage == Some(home.zone) => {}
            Some(Home::Zone(mut home)) => {
                let checkpoint = self
                    .combat
                    .as_ref()
                    .and_then(crate::combat::Combat::zone_start)
                    .and_then(|start| start.checkpoint);
                if let Some(name) = checkpoint {
                    home.arrive = Name::new(&name).as_u64();
                }
                tracing::info!(target: LOG_TARGET, zone = %home.zone, "a restart takes the stage home");
                self.begin_trip(home, true, None);
            }
            Some(Home::Town(zone)) if on_stage == zone => {}
            Some(Home::Town(_)) => tracing::warn!(
                target: LOG_TARGET,
                "a run in the town's square restarts on the stage a trip took it from: the town doesn't come back"
            ),
        }
    }

    /// A frame of the trip under way, after the content's update and before the zone streams.
    pub(super) fn advance_trip(&mut self, scene: &mut Scene, content: &mut Content) {
        let trips = &mut self.show.trips;
        let Some(trip) = &mut trips.trip else { return };
        let settings = trips.settings.unwrap_or(StreamSettings::DESKTOP);
        if !trip.asked {
            trip.asked = true;
            let simulation = self.combat.as_ref().map(crate::combat::Combat::simulation);
            if let Some(entered) = trip.entered.take()
                && let Some(zone) = simulation.and_then(|simulation| simulation.zone_ground(entered))
            {
                zone.pack().clone_into(&mut trip.target.pack);
            }
            trip.same = scene.zone_on_stage() == Some(trip.target.zone);
            trip.warm = trip.same || scene.zone_load(trip.target.zone) == ZoneLoad::Ready;
            if !trip.same {
                scene.prefetch_zone(trip.target.clone(), settings);
                scene.hurry_zone(trip.target.zone);
            }
        }
        if trip.ready.is_none() && trip.phase.holds_simulation() {
            if trip.same || scene.zone_load(trip.target.zone) == ZoneLoad::Ready {
                trip.ready = Some(trip.started.elapsed());
            } else {
                trip.frames += 1;
            }
        }
        let mut swap = false;
        match trip.phase {
            TripPhase::Covering => {
                if !trips.wiped || trips.covered {
                    trip.phase = TripPhase::Swapping;
                }
            }
            TripPhase::Swapping if trip.same => swap = true,
            TripPhase::Swapping => match scene.zone_load(trip.target.zone) {
                ZoneLoad::Waiting => {}
                ZoneLoad::Ready => swap = true,
                ZoneLoad::Failed(error) => {
                    // The simulation goes all the same, so a replay stays on its track: the stage keeps
                    // what it had.
                    trip.error = Some(error);
                    swap = true;
                }
            },
            TripPhase::Arriving => {
                if scene.zone_arrived(content) || trip.error.is_some() {
                    scene.end_arrival(content);
                    scene.prefetch_exits(content, settings);
                    trip.phase = TripPhase::Uncovering;
                    let report = TripReport {
                        zone: trip.target.zone,
                        warm: trip.warm,
                        load: if trip.warm { Duration::ZERO } else { trip.ready.unwrap_or(trip.load) },
                        frames: trip.frames,
                        swapped_after: trip.load,
                        settle: trip.swapped.as_ref().map_or(Duration::ZERO, Stopwatch::elapsed),
                        left: trip.left.take(),
                        error: trip.error.take(),
                    };
                    match &report.error {
                        None => tracing::info!(target: LOG_TARGET, ?report, "arrived in a zone"),
                        Some(error) => {
                            tracing::error!(target: LOG_TARGET, error, ?report, "a trip went wrong")
                        }
                    }
                    trips.last = Some(report);
                }
            }
            TripPhase::Uncovering => {
                if !trips.wiped || trips.clear {
                    trips.trip = None;
                    self.show.draws_latest = false;
                }
            }
        }
        if swap {
            self.swap(scene, content);
        }
    }

    /// The swap, between two steps: see the [module docs](self).
    fn swap(&mut self, scene: &mut Scene, content: &mut Content) {
        let Some(trip) = &mut self.show.trips.trip else { return };
        trip.load = trip.started.elapsed();
        trip.swapped = Some(Stopwatch::start());
        trip.phase = TripPhase::Arriving;
        let target = trip.target.clone();
        if !trip.stage_only
            && let Some(combat) = &mut self.combat
            && let Err(error) =
                combat.enter_zone(target.zone, &target.pack, target.arrive, |id| content.cooked_bytes(id))
        {
            trip.error.get_or_insert(error);
        }
        let same = trip.same;
        if trip.error.is_some() && !same && scene.zone_load(target.zone) != ZoneLoad::Ready {
            return;
        }
        let ground = match zone_ground(content, target.zone) {
            Ok(ground) => ground,
            Err(error) => {
                trip.error.get_or_insert(error);
                return;
            }
        };
        if !same {
            let Some(arrived) = scene.arrive_in_zone(target.zone, content) else { return };
            trip.left = arrived.left.and_then(|left| match left {
                Ok(report) => Some(report),
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "the last zone's pack stays mounted");
                    None
                }
            });
        }
        scene.stand_on(Arc::clone(&ground));
        let show = &mut self.show;
        if let Some(combat) = &self.combat {
            // Whoever stayed behind leaves the stage as a defeated warrior does, then everyone left stands
            // where they came in.
            show.follow_roster(scene, combat.simulation(), combat.cast());
            if let Some(arena) = &mut show.arena {
                arena.arrive(combat.simulation(), combat.cast(), ground);
            }
        }
        if let Some(cues) = &mut show.cues {
            scene.with_screen_effects(|stack, _| cues.start_over(stack));
        }
        scene.cut_view();
    }
}

/// What the stage stands on in zone `zone`: its colliders, as the simulation's are, or none.
fn zone_ground(content: &Content, zone: AssetId) -> Result<Arc<PhysicsWorld>, String> {
    let bytes = content.cooked_bytes(zone).ok_or_else(|| format!("zone {zone} isn't loaded"))?;
    let read = sapphire_zone::Zone::read(bytes).map_err(|error| error.to_string())?;
    let world = match read.simulation.colliders.as_ref().copied() {
        Some(id) => {
            let bytes =
                content.cooked_bytes(id).ok_or_else(|| format!("zone {zone}'s colliders aren't loaded"))?;
            let set = ColliderSet::read(bytes).map_err(|error| error.to_string())?;
            PhysicsWorld::from_cooked(set, 0).map_err(|error| error.to_string())?
        }
        None => PhysicsWorld::new(Vec::new(), 0).map_err(|error| error.to_string())?,
    };
    Ok(Arc::new(world))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::combat::{Combat, CombatOption, no_bindings};

    /// A replay's zone entry starts a trip inside the fixed step, and the step allocates nothing for it (M3
    /// Z1): the trip's first frame names the zone's pack from the simulation's ground.
    #[test]
    fn a_replays_trip_starts_in_a_step_with_no_allocation() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/zone-trip.sreplay");
        let combat = Combat::new(&CombatOption::Replay { path, to_end: true }).expect("reads");
        let mut fight = Fight::default();
        fight.attach_combat(Some(combat));
        assert!(sapphire_core::alloc::is_installed(), "this regression requires counting");
        loop {
            let scope = sapphire_core::alloc::AllocScope::begin();
            let held = fight.holds_step();
            let counts = scope.counts();
            if held {
                assert_eq!((counts.allocations, counts.bytes), (0, 0), "starting a trip");
                break;
            }
            let combat = fight.combat.as_mut().expect("attached");
            assert!(combat.ticks_left() != Some(0), "the replay took no trip");
            combat.step(&mut no_bindings);
        }
        let trip = fight.show.trips.trip.as_mut().expect("under way");
        assert_eq!(trip.phase, TripPhase::Covering);
        assert!(trip.target.pack.is_empty(), "named on its first frame");
        let entered = trip.entered.take().expect("a replay's entry");
        let simulation = fight.combat.as_ref().expect("attached").simulation();
        let pack = simulation.zone_ground(entered).expect("on the shelf").pack();
        assert!(pack.starts_with("zone_"), "{pack}");
    }
}
