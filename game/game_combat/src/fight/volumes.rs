//! Encounter volumes (M3 W5): what a zone's encounter will bring stands by before its trigger fires, and its
//! zone's streaming holds while it runs.
//!
//! **What the stage learns, and from where.** The simulation's copy of the zone, made from its cooked `.szone`
//! and `.sencounter`s before the first step ([`SimZone`]), says each encounter's trigger
//! ([`SimZone::start_volume`]), how many fighters its waves bring and how many have come. That's all a fight
//! reads: nothing here is in a step, and nothing moves a checksum.
//!
//! [`SimZone`]: sapphire_gameplay::SimZone
//! [`SimZone::start_volume`]: sapphire_gameplay::SimZone::start_volume
//!
//! **Warming.** An encounter starts to warm when the player on the field comes within [`WARM_REACH`] of its
//! trigger's volume, or is in it, or it's running already, or no trigger here starts it. Warm means the
//! stage holds a spare warrior of their kind for each fighter its waves have still to bring, beside the
//! spares every other warming encounter wants, so a wave's arrival in a fixed step takes one of its kind and
//! allocates nothing (B2). Spares
//! stand one a frame or more, within what the zone's streaming leaves of its budget that frame
//! ([`sapphire_world::streaming::StreamBudget`]): at least one, and more while time and entities are left.
//! Before a fight's first step, what's in reach then stands all at once, as a load screen would.
//!
//! What a spare needs is a warrior's: its meshes and materials are in the shared pack, whose pipelines the
//! load built (M3 K1), and its moves were in the simulation from its start. Effects are the app's, prewarmed
//! with the fight. Each kind of enemy (M3 B3) has its look loaded with the fight, so an encounter still brings
//! nothing to load or build: warming stands each kind's spares from that kind's stock.
//!
//! **Holding the zone.** While any encounter runs, the scene's zone streams nothing (`Scene::hold_zone_streaming`);
//! when none does, it looks again from where the camera is and catches up.
//!
//! A fight that starts over keeps its spares, so what warms again stands nothing new.

use std::time::Duration;

use sapphire_gameplay::{ActorId, EncounterPhase, Simulation, StartVolume, ZoneId};
use sapphire_math::Vec3;
use sapphire_stage::scene::Scene;
use sapphire_time::Stopwatch;
use sapphire_world::streaming::StreamBudget;
use sapphire_zone::CookedVolume;

use super::{FightShow, Warrior, draw, stand_warrior, undraw};
use crate::LOG_TARGET;

/// How far from an encounter's trigger the player is when its fighters start to stand by, in metres. At a
/// run of 6 m/s that's over a second, and a wave takes a frame or a few to stand on either tier's budget.
pub const WARM_REACH: f32 = 8.0;

/// Entities a warrior costs the frame's budget, at most: itself and its blade.
const ENTITIES_A_WARRIOR: u32 = 2;

/// Where one encounter stands, as the stage sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Warmth {
    /// Nobody's near, and it's asked for nothing.
    #[default]
    Cold,
    /// Its spares are standing.
    Warming,
    /// They stand.
    Warm,
}

/// What the stage has done for a fight's encounters (R9), for a bench and a test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VolumeStats {
    /// Encounters the zone has.
    pub encounters: u32,
    /// Encounters whose trigger fired with their spares all standing, and those it found still warming.
    pub started_warm: u32,
    pub started_cold: u32,
    /// Warriors stood by ahead of a wave, and those that had to stand in a step because no spare was left:
    /// the second is a miss, and allocates there.
    pub stood_ahead: u32,
    pub stood_in_step: u32,
    /// Frames that stood something.
    pub warming_frames: u64,
    /// The time warming took this frame, and its slowest frame so far.
    pub time: Duration,
    pub peak_time: Duration,
    /// Whether an encounter is running, holding the zone's streaming.
    pub holding: bool,
    /// Frames the zone's streaming spent held, and the cells it asked for in them: none, held.
    pub held_frames: u64,
    pub asked_while_held: u64,
}

/// A fight's encounters as the stage follows them.
#[derive(Default)]
pub(super) struct Volumes {
    /// Whether the fight's zone has been looked at, and room made for everyone it'll bring.
    seen: bool,
    /// The shelf's row it looked at, once a trip has entered one: one entered after brings its own
    /// encounters (M3 G1), looked at as it arrives.
    zone: Option<ZoneId>,
    /// Each encounter's, in the zone's order.
    warmth: Vec<Warmth>,
    /// Each encounter's phase as of the last frame, for what's changed.
    phases: Vec<EncounterPhase>,
    stats: VolumeStats,
}

impl Volumes {
    /// A new fight on the same stage: everything's cold again, and the spares stay.
    pub(super) fn start_over(&mut self) {
        self.warmth.fill(Warmth::Cold);
        self.phases.fill(EncounterPhase::Waiting);
    }

    /// Whether the fight's zone is still to be looked at: before its first step.
    pub(super) fn unseen(&self) -> bool {
        !self.seen
    }

    /// A warrior had to stand in a step.
    pub(super) fn missed(&mut self) {
        self.stats.stood_in_step += 1;
    }
}

/// How much warming may do.
#[derive(Clone, Copy, Debug)]
pub(super) enum Pace {
    /// Everything, before a fight's first step.
    AtOnce,
    /// What a frame's budget leaves, at least one.
    Within { budget: StreamBudget, spent: Duration },
}

impl FightShow {
    /// What the stage has done for the fight's encounters.
    #[must_use]
    pub fn volume_stats(&self) -> VolumeStats {
        self.volumes.stats
    }

    /// A frame's look at the fight's encounters: stands what the near ones will bring, within what the
    /// zone's streaming left of the frame's budget, and holds the streaming while one runs. Nothing without
    /// `--arena` or a zone.
    pub fn follow_volumes(&mut self, scene: &mut Scene, simulation: &Simulation, cast: &[ActorId]) {
        let budget = scene.stream_settings().map_or(StreamBudget::DESKTOP, |settings| settings.budget);
        // The zone streamed after the last frame's look: what it did then.
        let streamed = scene.stream_stats().unwrap_or_default();
        if streamed.held {
            self.volumes.stats.held_frames += 1;
            self.volumes.stats.asked_while_held += u64::from(streamed.asked);
        }
        self.warm_volumes(scene, simulation, cast, Pace::Within { budget, spent: streamed.time });
    }

    /// Looks at the fight's encounters and warms what's near, at `pace`.
    pub(super) fn warm_volumes(
        &mut self,
        scene: &mut Scene,
        simulation: &Simulation,
        cast: &[ActorId],
        pace: Pace,
    ) {
        let (Some(arena), Some(zone)) = (&mut self.arena, simulation.zone()) else { return };
        let stopwatch = Stopwatch::start();
        let volumes = &mut self.volumes;
        let encounters = zone.encounters();
        if !volumes.seen || volumes.zone != simulation.current_zone() {
            volumes.seen = true;
            volumes.zone = simulation.current_zone();
            volumes.warmth.clear();
            volumes.warmth.resize(encounters.len(), Warmth::Cold);
            volumes.phases.clear();
            volumes.phases.resize(encounters.len(), EncounterPhase::Waiting);
            volumes.stats.encounters += encounters.len() as u32;
            // Room for everyone, so an arrival grows nothing: a few words a fighter.
            let planned = zone.planned();
            arena.reserve(planned);
            self.warriors.reserve(planned);
            // A warrior standing of another kind than who comes goes to the spares too.
            self.spares.reserve(planned + arena.free_warriors());
        }

        let player = cast
            .first()
            .and_then(|&first| simulation.actor(simulation.party().on_field(first)))
            .map(|actor| actor.position);
        let mut running = false;
        let mut widened = false;
        for (index, encounter) in encounters.iter().enumerate() {
            let phase = encounter.phase();
            running |= phase == EncounterPhase::Running;
            let warmth = &mut volumes.warmth[index];
            if *warmth == Warmth::Cold && phase != EncounterPhase::Cleared {
                let near = match (zone.start_volume(index), player) {
                    (Some(start), Some(at)) => distance_to(start, at) <= WARM_REACH,
                    (None, _) => true,
                    (Some(_), None) => false,
                };
                if near || phase == EncounterPhase::Running {
                    *warmth = Warmth::Warming;
                    widened = true;
                }
            }
            let was = std::mem::replace(&mut volumes.phases[index], phase);
            if was == EncounterPhase::Waiting && phase != EncounterPhase::Waiting {
                let warm = volumes.warmth[index] == Warmth::Warm;
                if warm {
                    volumes.stats.started_warm += 1;
                } else {
                    volumes.stats.started_cold += 1;
                }
                tracing::info!(target: LOG_TARGET, encounter = index, tick = simulation.tick(), warm, "an encounter started");
            }
            if was == EncounterPhase::Running && phase == EncounterPhase::Cleared {
                tracing::info!(target: LOG_TARGET, encounter = index, tick = simulation.tick(), "an encounter cleared");
            }
        }
        if running != volumes.stats.holding {
            scene.hold_zone_streaming(running);
            volumes.stats.holding = running;
            let tick = simulation.tick();
            if running {
                tracing::info!(target: LOG_TARGET, tick, "an encounter holds the zone's streaming");
            } else {
                tracing::info!(target: LOG_TARGET, tick, "the zone streams again");
            }
        }

        // Spares for what every warming or warm encounter has still to bring, of each kind. A fighter whose
        // set no kind here draws comes as the first kind, as the roster has them.
        let kinds = self.stocks.len();
        let kind_of = |set| Some(arena.kind_of_set(set)).filter(|&kind| kind < kinds).unwrap_or(0);
        let wanted_of = |kind: usize| -> usize {
            encounters
                .iter()
                .zip(&volumes.warmth)
                .filter(|(encounter, warmth)| {
                    **warmth != Warmth::Cold && encounter.phase() != EncounterPhase::Cleared
                })
                .flat_map(|(encounter, _)| encounter.sets_to_come())
                .filter(|&set| kind_of(set) == kind)
                .count()
        };
        let spares_of =
            |spares: &[Warrior], kind: usize| spares.iter().filter(|spare| spare.kind == kind).count();
        if widened && let Some(cues) = &mut self.cues {
            let short: usize =
                (0..kinds).map(|kind| wanted_of(kind).saturating_sub(arena.free_warriors_of(kind))).sum();
            cues.stand_by(arena.fighter_count() + short);
        }
        let world = scene.world_mut();
        let mut stood = 0;
        let mut stand = true;
        let mut all_stand = kinds > 0;
        for (kind, stock) in self.stocks.iter().enumerate() {
            let wanted = wanted_of(kind);
            let mut have = arena.free_warriors_of(kind) + spares_of(&self.spares, kind);
            while have < wanted {
                if stood > 0
                    && let Pace::Within { budget, spent } = pace
                {
                    let limit = Duration::from_micros(u64::from(budget.micros)).saturating_sub(spent);
                    if stood * ENTITIES_A_WARRIOR as usize >= budget.entities as usize
                        || stopwatch.elapsed() >= limit
                    {
                        stand = false;
                    }
                }
                if !stand {
                    all_stand = false;
                    break;
                }
                let warrior = stand_warrior(world, stock, kind);
                undraw(world, warrior);
                self.spares.push(warrior);
                stood += 1;
                have += 1;
            }
        }
        if stood > 0 {
            volumes.stats.stood_ahead += stood as u32;
            volumes.stats.warming_frames += 1;
        }

        if all_stand && volumes.warmth.contains(&Warmth::Warming) {
            // All of them drawn, so that what holds drawn warriors has room for every one, then none, then
            // the way back once for each kind, so that the world knows it: an arrival then moves nothing that
            // grows.
            for &warrior in &self.spares {
                if let Some(stock) = self.stocks.get(warrior.kind) {
                    draw(world, warrior, stock.meshes);
                }
            }
            for &warrior in &self.spares {
                undraw(world, warrior);
            }
            for (kind, stock) in self.stocks.iter().enumerate() {
                if let Some(&warrior) = self.spares.iter().find(|spare| spare.kind == kind) {
                    draw(world, warrior, stock.meshes);
                    undraw(world, warrior);
                }
            }
            for (index, warmth) in volumes.warmth.iter_mut().enumerate() {
                if *warmth == Warmth::Warming {
                    *warmth = Warmth::Warm;
                    tracing::info!(
                        target: LOG_TARGET,
                        encounter = index,
                        tick = simulation.tick(),
                        spares = self.spares.len(),
                        frames = volumes.stats.warming_frames,
                        "an encounter's fighters stand by"
                    );
                }
            }
        }
        volumes.stats.time = stopwatch.elapsed();
        volumes.stats.peak_time = volumes.stats.peak_time.max(volumes.stats.time);
    }
}

/// How far `at` is from a trigger's volume, in metres: nothing inside it.
fn distance_to(start: StartVolume, at: Vec3) -> f32 {
    let offset = at - Vec3::from(start.position);
    match start.volume {
        CookedVolume::Box { half_extents } => {
            (offset.abs() - Vec3::from(half_extents)).max(Vec3::ZERO).length()
        }
        CookedVolume::Sphere { radius } => (offset.length() - radius).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_is_as_far_as_its_nearest_surface() {
        let sphere = StartVolume { position: [0.0, 1.0, 0.0], volume: CookedVolume::Sphere { radius: 2.0 } };
        assert_eq!(distance_to(sphere, Vec3::new(0.0, 1.0, 10.0)), 8.0);
        assert_eq!(distance_to(sphere, Vec3::new(0.5, 1.0, 0.5)), 0.0, "inside");
        let cube = StartVolume {
            position: [9.0, 1.0, 10.0],
            volume: CookedVolume::Box { half_extents: [4.0, 2.0, 4.0] },
        };
        assert_eq!(distance_to(cube, Vec3::new(1.0, 0.0, 10.0)), 4.0, "off one face");
        assert_eq!(distance_to(cube, Vec3::new(10.0, 0.0, 11.0)), 0.0, "inside");
        assert!(
            (distance_to(cube, Vec3::new(16.0, 0.0, 17.0)) - 2.0_f32.sqrt() * 3.0).abs() < 1e-5,
            "off a corner"
        );
    }
}
