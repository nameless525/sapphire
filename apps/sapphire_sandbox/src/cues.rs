//! `--cues`: the cues of a `--replay` or a `--record`ed fight fired as a game fires them, through
//! `sapphire_cues` (M2 E3): effects, the scene's screen effects, a shake on its camera, sound and rumble.
//!
//! - **On this stage.** The simulation's actors draw nothing here until N3 and O5 put them on stage, and
//!   the look-dev stage's character stands at the origin in a held pose. So [`on_stage`] carries a fight
//!   to it: what lands on an actor lands on the scene's character, wherever the fight has walked to, at
//!   that place on its own surface. A fight with its actors drawn takes the simulation's places as they
//!   are, and needs none of this.
//! - **A camera that isn't the director's.** The scene frames its stage with [`OrbitCamera`], so a
//!   [`ShakeStack`] plays shakes on it, from the director's own tuning. A stage with a
//!   `sapphire_camera::CameraDirector` hands that to the dispatcher instead, with no other change.
//! - **Sound and rumble are the windowed app's.** A capture or a bench never mounts `audio.spack`
//!   (`audio.rs`), and has no pad. Their frames just forget what the steps asked for.
//! - **It reads the simulation and changes nothing in it,** so a replay's checksums still hold, and a
//!   capture's check still refuses another fight.
//! - **Its clocks are the simulation's,** for effects and sound, which is what freezes them. The scene's
//!   effect stack stays on the scene's own world clock, which a fight's slow motion doesn't reach.

use std::time::Duration;

use sapphire_audio::{CueRequest, Emitter};
use sapphire_camera::{CameraTuning, ShakeId, ShakeStack};
use sapphire_cues::{
    CueDispatcher, CueStats, CueStep, CueTable, CueTableError, RumbleSink, ShakeSink, actor_emitter,
};
use sapphire_ecs::prelude::WorldClock;
use sapphire_gameplay::{CueEvent, Simulation};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::{GamepadId, Platform};
use sapphire_render::{PostVariants, VfxFrame};
use sapphire_time::Stopwatch;
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

/// Seeded, so a capture's sparks land the same way in every run, and so do its shakes.
const SEED: u64 = 0x5EED;

/// How a hurtbox's surface maps onto the character's: the test moves' capsule is 0.7 m across and 1.75 m
/// tall, and the character about half as wide and 1.55 m tall, so a hit on one's edge lands on the other's.
const ON_THE_CHARACTER: Vec3 = Vec3::new(0.5, 0.88, 0.5);

/// The camera's shakes, for a view that plays them itself.
struct OrbitShakes {
    stack: ShakeStack,
    tuning: CameraTuning,
}

impl ShakeSink for OrbitShakes {
    fn fire_shake(&mut self, id: ShakeId, strength: f32) {
        if let Some(preset) = self.tuning.shakes.presets.get(usize::from(id.0)) {
            self.stack.fire(preset.shake, strength);
        }
    }
}

/// The pad in the player's hands, if there is one: nothing on the keyboard, on touch, or on a pad with
/// no motors, which SDL tells [`Platform::rumble_gamepad`] and nothing needs to hear. No phone's haptics
/// here, by choice: they aren't SDL's, and a fight on touch has enough to prove first.
pub(crate) struct PadRumble<'a> {
    pub platform: &'a mut Platform,
    pub pad: Option<GamepadId>,
}

impl RumbleSink for PadRumble<'_> {
    fn rumble(&mut self, low: f32, high: f32, duration: Duration) {
        if let Some(pad) = self.pad {
            self.platform.rumble_gamepad(pad, low, high, duration);
        }
    }
}

/// What a fight asks of the audio system this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CueSounds<'a> {
    pub requests: &'a [CueRequest],
    pub emitters: &'a [Emitter],
    /// The simulation's world clock's speed, which its slow motion turns down.
    pub world_time_scale: f32,
}

impl CueSounds<'_> {
    /// Nothing: there's no fight.
    pub const NONE: CueSounds<'static> = CueSounds { requests: &[], emitters: &[], world_time_scale: 1.0 };
}

/// What's firing, and what it fires into that the scene doesn't hold itself.
pub(crate) struct CueShow {
    table: CueTable,
    dispatcher: CueDispatcher,
    world: VfxWorld,
    shakes: OrbitShakes,
    emitters: Vec<Emitter>,
    world_time_scale: f32,
    /// The simulation's tick when it last stepped, so a replay past its end stops as it does.
    tick: Option<u64>,
    /// The time spent in steps since the last frame was written.
    stepping: Duration,
}

impl CueShow {
    /// The shipped table, within `budget`. An error names a row it can't resolve, and a test holds the
    /// shipped libraries to it.
    pub fn new(budget: VfxBudget, presets: &ScreenLibrary) -> Result<Self, CueTableError> {
        let table = CueTable::default_table();
        let effects = VfxLibrary::default_effects();
        let tuning = CameraTuning::default_tuning();
        let dispatcher = CueDispatcher::new(&table, &effects, presets, &tuning)?;
        Ok(Self {
            table,
            dispatcher,
            world: VfxWorld::new(effects, budget, SEED),
            shakes: OrbitShakes { stack: ShakeStack::new(SEED), tuning },
            emitters: Vec::new(),
            world_time_scale: 1.0,
            tick: None,
            stepping: Duration::ZERO,
        })
    }

    /// The post variants the table's screen effects draw through.
    pub fn post_variants(&self, presets: &ScreenLibrary) -> PostVariants {
        self.dispatcher.post_variants(presets)
    }

    /// The table, for what an app loads for it.
    pub fn table(&self) -> &CueTable {
        &self.table
    }

    /// Fires what the simulation's latest step published, and moves the effects by its clocks: call it
    /// after each step. `stand` is where the scene's character stands. Allocates nothing once warmed.
    pub fn step(
        &mut self,
        sim: &Simulation,
        screen: &mut EffectStack,
        presets: &ScreenLibrary,
        view_projection: Mat4,
        stand: Vec3,
    ) {
        let tick = sim.tick();
        if self.tick == Some(tick) {
            return;
        }
        self.tick = Some(tick);
        let stopwatch = Stopwatch::start();
        let root_of = |_| Some(stand);
        let mut step = CueStep {
            vfx: &mut self.world,
            screen,
            presets,
            shakes: &mut self.shakes,
            view_projection,
            root_of: &root_of,
        };
        self.dispatcher.read_step(tick, sim.cues().map(|cue| on_stage(cue, sim, stand)), &mut step);
        self.dispatcher.advance_effects(sim, &mut self.world);
        self.emitters.clear();
        self.emitters.extend(sim.actors().map(|actor| Emitter { position: stand, ..actor_emitter(&actor) }));
        self.world_time_scale = sim.world().resource::<WorldClock>().0.effective_scale();
        self.stepping += stopwatch.elapsed();
    }

    /// Writes the moment's effects to `frame`, with the time this frame's steps and the write took.
    pub fn write(&mut self, frame: &mut VfxFrame) {
        let stopwatch = Stopwatch::start();
        self.world.write_frame(frame);
        frame.stats.simulation = self.stepping + stopwatch.elapsed();
        self.stepping = Duration::ZERO;
    }

    /// What this frame's steps asked of the audio system.
    pub fn sounds(&self) -> CueSounds<'_> {
        CueSounds {
            requests: self.dispatcher.sounds(),
            emitters: &self.emitters,
            world_time_scale: self.world_time_scale,
        }
    }

    /// Ends the frame, `dt` real seconds after the last: forgets its sounds, tells `rumble` what changed,
    /// and returns where its shakes turn and move the view.
    pub fn end_frame(&mut self, dt: f32, rumble: &mut dyn RumbleSink) -> (Vec3, Vec3) {
        self.dispatcher.end_frame(dt, rumble);
        self.shakes.stack.advance(dt);
        self.shakes.stack.sample(&self.shakes.tuning.shakes)
    }

    /// Stops any rumble at once: the app lost focus or the foreground.
    pub fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        self.dispatcher.stop_rumble(rumble);
    }

    pub fn stats(&self) -> CueStats {
        self.dispatcher.stats()
    }

    /// Takes `presets`, or says why not: the table names a preset they don't have. Nothing changes then,
    /// and the caller keeps its presets too.
    #[cfg(any(test, feature = "dev"))]
    pub fn take_presets(&mut self, presets: &ScreenLibrary) -> Result<(), CueTableError> {
        self.dispatcher.resolve(&self.table, self.world.library(), presets, &self.shakes.tuning)
    }

    /// Takes a new table, or says why not: it names what its libraries don't have.
    #[cfg(any(test, feature = "dev"))]
    pub fn take_table(&mut self, table: CueTable, presets: &ScreenLibrary) -> Result<(), CueTableError> {
        self.dispatcher.resolve(&table, self.world.library(), presets, &self.shakes.tuning)?;
        self.table = table;
        Ok(())
    }
}

/// `event` on the scene's character: a hit lands where it would on its target's own surface, with the
/// target turned to face the way the character does. See the [module docs](self).
fn on_stage(event: CueEvent, sim: &Simulation, stand: Vec3) -> CueEvent {
    let Some(mut hit) = event.hit else { return event };
    let Some(target) = sim.actor(hit.target) else { return event };
    let turn = Quat::from_rotation_y(-target.yaw);
    hit.point = stand + (turn * (hit.point - target.position)) * ON_THE_CHARACTER;
    hit.normal = turn * hit.normal;
    CueEvent { hit: Some(hit), ..event }
}

#[cfg(test)]
mod tests {
    use sapphire_gameplay::{ActorId, HitCue, HitOutcome};
    use sapphire_moves::Weight;
    use sapphire_time::{SimDuration, SimTime};

    use super::*;

    /// A hit on the target's chest, a little off its middle, lands there on the character, whichever
    /// way and wherever the fight had the target stand.
    #[test]
    fn a_hit_lands_on_the_character_as_on_its_target() {
        let scenario = sapphire_gameplay::replay::find_scenario(sapphire_gameplay::replay::SCENARIOS, "duel")
            .expect("a scenario");
        let (sim, cast) = scenario.start(scenario.config).expect("sets up");
        // The opponent stands 4 m along +Z, half a turn round, facing the player.
        let target = sim.actor(cast[1]).expect("there");
        assert!((target.yaw.abs() - std::f32::consts::PI).abs() < 1e-5);
        let event = CueEvent {
            actor: ActorId::from_raw(cast[0].raw()).expect("not zero"),
            cue: sapphire_core::Name::new("hit_landed"),
            tick: 0,
            local_time: SimTime::ZERO,
            move_time: SimDuration::ZERO,
            hit: Some(HitCue {
                target: cast[1],
                hurtbox: 0,
                // In front of it, as it faces, a little to its left, and turned toward its front.
                point: target.position + Vec3::new(0.25, 1.4, -0.3),
                normal: Vec3::NEG_Z,
                weight: Weight::Heavy,
                damage: 1,
                poise: 1,
                hitstop: 1,
                outcome: HitOutcome::Flinched,
            }),
        };
        let stand = Vec3::new(1.0, 0.0, 2.0);
        let hit = on_stage(event, &sim, stand).hit.expect("still a hit");
        // The character faces +Z: its front is toward +Z.
        let on_it = stand + Vec3::new(-0.25, 1.4, 0.3) * ON_THE_CHARACTER;
        assert!(hit.point.abs_diff_eq(on_it, 1e-4), "{}", hit.point);
        assert!(hit.normal.abs_diff_eq(Vec3::Z, 1e-4), "{}", hit.normal);
    }

    #[test]
    fn a_cue_with_no_hit_is_left_as_it_is() {
        let scenario = sapphire_gameplay::replay::find_scenario(sapphire_gameplay::replay::SCENARIOS, "duel")
            .expect("a scenario");
        let (sim, cast) = scenario.start(scenario.config).expect("sets up");
        let event = CueEvent {
            actor: cast[0],
            cue: sapphire_core::Name::new("swing"),
            tick: 0,
            local_time: SimTime::ZERO,
            move_time: SimDuration::ZERO,
            hit: None,
        };
        assert_eq!(on_stage(event, &sim, Vec3::ONE), event);
    }

    /// The shipped table resolves against the libraries a scene starts with.
    #[test]
    fn a_show_starts_on_the_shipped_libraries() {
        let presets = ScreenLibrary::default_effects();
        let show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("every name's there");
        assert_ne!(show.post_variants(&presets), PostVariants::NONE);
        assert!(!show.table().sound_cues().is_empty());
    }

    /// A new table naming what the libraries lack is refused, and the one firing stays.
    #[test]
    fn a_reload_naming_what_is_not_there_is_refused() {
        let presets = ScreenLibrary::default_effects();
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let rows = show.table().rows().len();
        let naming = CueTable::from_ron(
            r#"(type: "sapphire::cues::CueTable", version: 1, data: (rows: [
                (cue: "hit_landed", screen: Some((preset: "no_such_preset"))),
            ]))"#,
        )
        .expect("valid on its own");
        let error = show.take_table(naming, &presets).expect_err("no such preset");
        assert!(matches!(error, CueTableError::UnknownPreset { .. }), "{error}");
        assert_eq!(show.table().rows().len(), rows);
        show.take_table(CueTable::default_table(), &presets).expect("the shipped one again");
    }

    /// Presets that lose one the table pushes are refused, so what's firing keeps working.
    #[test]
    fn a_reload_that_loses_a_preset_is_refused() {
        let presets = ScreenLibrary::default_effects();
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let losing = ScreenLibrary::from_ron(
            r#"(type: "sapphire::world::EffectLibrary", version: 1, data: (presets: [
                (name: "only", layers: [(inversion: (amount: 1.0), envelope: Keys([(0.0, 1.0), (0.05, 1.0)]))]),
            ]))"#,
        )
        .expect("valid");
        let error = show.take_presets(&losing).expect_err("the table pushes others");
        assert!(matches!(error, CueTableError::UnknownPreset { .. }), "{error}");
        show.take_presets(&presets).expect("the same again");
    }
}
