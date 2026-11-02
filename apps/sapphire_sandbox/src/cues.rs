//! `--cues`: the cues of a `--replay` or a `--record`ed fight fired as a game fires them, through
//! `sapphire_cues` (M2 E3): effects, the scene's screen effects, a shake on its camera, sound and rumble.
//!
//! - **Where it lands** ([`CueStage`]). The arena draws the simulation's actors, so a cue lands where
//!   the simulation has them. The look-dev stage draws none of them, and its character stands at the
//!   origin in a held pose, so [`on_stage`] carries a fight to it: what lands on an actor lands on the
//!   scene's character, wherever the fight has walked to, at that place on its own surface.
//! - **The camera.** With a `sapphire_camera::CameraDirector` on, shakes are its own. Any other view is
//!   `OrbitCamera`, so a [`ShakeStack`] plays shakes on it, from the director's own tuning.
//! - **Sound and rumble are the windowed app's.** A capture or a bench never mounts `audio.spack`
//!   (`audio.rs`), and has no pad. Their frames just forget what the steps asked for.
//! - **It reads the simulation and changes nothing in it,** so a replay's checksums still hold, and a
//!   capture's check still refuses another fight.
//! - **Its clocks are the simulation's,** for effects and sound, which is what freezes them. The scene's
//!   effect stack is on the scene's own world clock, which [`Scene::follow_simulation`] keeps with the
//!   fight's slow motion, so a screen effect and a spark take the same time.
//! - **A frame reads them all as it reads a pose** (review WA-3): `alpha` of the way from the step before
//!   the latest to the latest, each by its own clock, and what began on the latest step from where it
//!   begins. Shakes stay on the real clock, as the director's do, and start when what's drawn reaches
//!   their step, so none of it leads the blow.
//!
//! [`Scene::follow_simulation`]: crate::scene::Scene::follow_simulation

use std::time::Duration;

use sapphire_audio::{CueRequest, Emitter};
use sapphire_camera::{CameraDirector, CameraTuning, ShakeId, ShakeStack};
use sapphire_cues::{
    CueDispatcher, CueStats, CueStep, CueTable, CueTableError, RumbleSink, ShakeSink, actor_emitter,
};
use sapphire_ecs::prelude::WorldClock;
use sapphire_gameplay::{CueEvent, Simulation};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_platform::{GamepadId, Platform};
use sapphire_render::{PostVariants, VfxFrame};
use sapphire_time::{Stopwatch, TICK_HZ};
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

/// Seeded, so a capture's sparks land the same way in every run, and so do its shakes.
const SEED: u64 = 0x5EED;

/// How a hurtbox's surface maps onto the character's: the test moves' capsule is 0.7 m across and 1.75 m
/// tall, and the character about half as wide and 1.55 m tall, so a hit on one's edge lands on the other's.
const ON_THE_CHARACTER: Vec3 = Vec3::new(0.5, 0.88, 0.5);

/// How many seconds a frame `alpha` of the way between its last two steps draws behind the latest:
/// what a shake fired since the last frame starts from. Not a number draws the step before, as
/// `presented_ticks` does.
pub(crate) fn behind_latest(alpha: f32) -> f32 {
    (1.0 - if alpha >= 0.0 { alpha.min(1.0) } else { 0.0 }) / TICK_HZ as f32
}

/// Where a fight's cues land.
pub(crate) enum CueStage<'a> {
    /// On the one character that stands here, whichever actor they name ([`on_stage`]).
    OneCharacter { stand: Vec3 },
    /// Where the simulation has its actors, which are drawn there: the arena. With a director on, it plays
    /// their shakes.
    Actors { director: Option<&'a mut CameraDirector> },
}

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
    /// The sounds of the table it started with: what an app loads for it, once.
    #[cfg(any(test, feature = "dev"))]
    loaded_sounds: Vec<sapphire_assets::AssetId>,
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
        Self::with_table(CueTable::default_table(), VfxLibrary::default_effects(), budget, presets)
    }

    /// `table`, firing `effects` within `budget`. An error names a row it can't resolve.
    pub(crate) fn with_table(
        table: CueTable,
        effects: VfxLibrary,
        budget: VfxBudget,
        presets: &ScreenLibrary,
    ) -> Result<Self, CueTableError> {
        let tuning = CameraTuning::default_tuning();
        let dispatcher = CueDispatcher::new(&table, &effects, presets, &tuning)?;
        Ok(Self {
            #[cfg(any(test, feature = "dev"))]
            loaded_sounds: table.sound_cues(),
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
    /// after each step. Allocates nothing once warmed.
    pub fn step(
        &mut self,
        sim: &Simulation,
        screen: &mut EffectStack,
        presets: &ScreenLibrary,
        view_projection: Mat4,
        stage: CueStage<'_>,
    ) {
        let tick = sim.tick();
        if self.tick == Some(tick) {
            return;
        }
        self.tick = Some(tick);
        let stopwatch = Stopwatch::start();
        let (stand, shakes): (Option<Vec3>, &mut dyn ShakeSink) = match stage {
            CueStage::OneCharacter { stand } => (Some(stand), &mut self.shakes),
            CueStage::Actors { director: Some(director) } => (None, director),
            CueStage::Actors { director: None } => (None, &mut self.shakes),
        };
        let root_of = |id| stand.or_else(|| sim.actor(id).map(|actor| actor.position));
        let mut step =
            CueStep { vfx: &mut self.world, screen, presets, shakes, view_projection, root_of: &root_of };
        let cues = sim.cues().map(|cue| stand.map_or(cue, |stand| on_stage(cue, sim, stand)));
        self.dispatcher.read_step(tick, cues, &mut step);
        self.dispatcher.advance_effects(sim, &mut self.world);
        self.emitters.clear();
        self.emitters.extend(sim.actors().map(|actor| {
            let emitter = actor_emitter(&actor);
            Emitter { position: stand.unwrap_or(emitter.position), ..emitter }
        }));
        self.world_time_scale = sim.world().resource::<WorldClock>().0.effective_scale();
        self.stepping += stopwatch.elapsed();
    }

    /// Writes the effects to `frame`, `alpha` of the way from the step before the latest to the latest,
    /// with the time this frame's steps and the write took.
    pub fn write(&mut self, frame: &mut VfxFrame, alpha: f32) {
        let stopwatch = Stopwatch::start();
        self.world.write_frame(frame, alpha);
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

    /// Ends the frame's steps, `dt` real seconds after the last frame's, for a frame drawn at `alpha`:
    /// forgets its sounds, tells `rumble` what changed, and returns where its shakes turn and move the
    /// view, having played them by `view_dt`, which a pause holds at zero. A pad's rumble runs on: it's in
    /// the player's hands, not on the screen.
    pub fn end_frame(
        &mut self,
        dt: f32,
        view_dt: f32,
        alpha: f32,
        rumble: &mut dyn RumbleSink,
    ) -> (Vec3, Vec3) {
        self.dispatcher.end_frame(dt, rumble);
        self.shakes.stack.advance(view_dt, behind_latest(alpha));
        self.shakes.stack.sample(&self.shakes.tuning.shakes)
    }

    /// Stops any rumble at once: the app lost focus or the foreground.
    pub fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        self.dispatcher.stop_rumble(rumble);
    }

    pub fn stats(&self) -> CueStats {
        self.dispatcher.stats()
    }

    /// A new fight: what the last one set going ends here, rather than playing out over ground its
    /// actors have left. Every screen effect goes, `--post-effect`'s among them. A rumble stops at the
    /// next [`Self::end_frame`], which has a pad to tell. A sound rings out: the mixer has no way to stop
    /// one, and the longest is a second or so.
    pub fn start_over(&mut self, screen: &mut EffectStack) {
        self.world.clear();
        self.shakes.stack.clear();
        self.dispatcher.release_rumble();
        screen.clear();
    }

    /// Takes `presets`, or says why not: the table names a preset they don't have. Nothing changes then,
    /// and the caller keeps its presets too.
    #[cfg(any(test, feature = "dev"))]
    pub fn take_presets(&mut self, presets: &ScreenLibrary) -> Result<(), CueTableError> {
        self.dispatcher.resolve(&self.table, self.world.library(), presets, &self.shakes.tuning)
    }

    /// Takes new camera tuning, or says why not: the table names a shake it doesn't have. A shake's ID is
    /// its place in the tuning's list, so the table's are resolved again, through the dispatcher's one
    /// route for that. Shakes playing finish as they were fired.
    #[cfg(any(test, feature = "dev"))]
    pub fn take_camera_tuning(
        &mut self,
        tuning: CameraTuning,
        presets: &ScreenLibrary,
    ) -> Result<(), CueTableError> {
        self.dispatcher.resolve(&self.table, self.world.library(), presets, &tuning)?;
        self.shakes.tuning = tuning;
        Ok(())
    }

    /// Takes a new table, or says why not: it names what its libraries don't have, or a sound that wasn't
    /// loaded as the app started, which would otherwise be dropped each time it fired.
    #[cfg(any(test, feature = "dev"))]
    pub fn take_table(&mut self, table: CueTable, presets: &ScreenLibrary) -> Result<(), CueTableError> {
        table.check_sounds(&self.loaded_sounds)?;
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

    /// In the arena a cue lands where the simulation has its actor, no shake goes near the orbit camera's
    /// stack while a director's on, and the sound is heard from there too.
    #[test]
    fn a_fights_cues_land_where_its_actors_are() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        let run = |director: Option<&mut CameraDirector>| {
            let presets = ScreenLibrary::default_effects();
            let mut screen = EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
            let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
            let scenario = find_scenario(SCENARIOS, "arena-fight").expect("listed");
            let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
            let mut director = director;
            for _ in 0..140 {
                sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
                let stage = CueStage::Actors { director: director.as_deref_mut() };
                show.step(&sim, &mut screen, &presets, Mat4::IDENTITY, stage);
            }
            let places: Vec<Vec3> = sim.actors().map(|actor| actor.position).collect();
            let heard: Vec<Vec3> = show.emitters.iter().map(|emitter| emitter.position).collect();
            assert_eq!(heard, places, "heard from where they stand");
            let stats = show.stats();
            assert!(stats.effects > 0 && stats.shakes > 0, "the overhead landed: {stats:?}");
            show
        };

        let mut director = CameraDirector::new(CameraTuning::default_tuning(), SEED).expect("it's good");
        let mut with_director = run(Some(&mut director));
        assert!(director.take_stats().shakes_fired > 0, "its shakes are the director's");
        let shaken = |show: &mut CueShow| {
            // A shake reads from where it begins on the frame it fires, and moves the view from the next.
            let _ = show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ());
            show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ())
        };
        assert_eq!(shaken(&mut with_director), (Vec3::ZERO, Vec3::ZERO));

        let mut plain = run(None);
        let moved = shaken(&mut plain);
        assert_ne!(moved, (Vec3::ZERO, Vec3::ZERO), "the plain view's shake");
        assert_eq!(plain.end_frame(1.0 / 60.0, 0.0, 1.0, &mut ()), moved, "a pause holds it");
        assert_ne!(
            plain.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ()),
            moved,
            "and it plays on from there"
        );
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

    /// Sounds load once, as the app starts, so a new table playing another would be silent while its
    /// reply said it had been taken. One the app did load is fine, even after a table that dropped it.
    #[test]
    fn a_reload_playing_a_sound_nobody_loaded_is_refused() {
        let presets = ScreenLibrary::default_effects();
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let rows = show.table().rows().len();
        let loaded = show.table().sound_cues()[0];
        let playing = |sound: sapphire_assets::AssetId| {
            CueTable::from_ron(&format!(
                r#"(type: "sapphire::cues::CueTable", version: 1, data: (rows: [
                    (cue: "hit_landed", sounds: ["{sound}"]),
                ]))"#
            ))
            .expect("valid on its own")
        };
        let unloaded = sapphire_assets::AssetId::from_text("99999999-2222-4333-8444-555555555555");
        let error = show.take_table(playing(unloaded), &presets).expect_err("nothing loaded it");
        assert!(matches!(error, CueTableError::UnloadedSound { .. }), "{error}");
        assert_eq!(show.table().rows().len(), rows);

        let silent = CueTable::from_ron(
            r#"(type: "sapphire::cues::CueTable", version: 1, data: (rows: [(cue: "hit_landed")]))"#,
        )
        .expect("valid on its own");
        show.take_table(silent, &presets).expect("it plays nothing");
        show.take_table(playing(loaded), &presets).expect("the app still holds this one");
    }

    /// What a fired row asks of a camera.
    #[derive(Default)]
    struct FiredShakes(Vec<ShakeId>);

    impl ShakeSink for FiredShakes {
        fn fire_shake(&mut self, id: ShakeId, _strength: f32) {
            self.0.push(id);
        }
    }

    /// Fires the table's first row with a shake, read on `tick`, and returns what it asked for.
    fn shakes_fired(show: &mut CueShow, presets: &ScreenLibrary, tick: u64) -> Vec<ShakeId> {
        let row = show.table.rows().iter().find(|row| row.shake.is_some()).expect("a row with a shake");
        let actor = ActorId::from_raw(1).expect("not zero");
        let hit = |weight| HitCue {
            target: actor,
            hurtbox: 0,
            point: Vec3::ZERO,
            normal: Vec3::Z,
            weight,
            damage: 1,
            poise: 1,
            hitstop: 1,
            outcome: HitOutcome::Flinched,
        };
        let event = CueEvent {
            actor,
            cue: sapphire_core::Name::intern(&row.cue),
            tick,
            local_time: SimTime::ZERO,
            move_time: SimDuration::ZERO,
            hit: row.weight.map(hit),
        };
        let mut clock = sapphire_time::ScaledClock::new();
        let mut screen = EffectStack::new(clock.new_local_clock());
        let mut fired = FiredShakes::default();
        let root_of = |_| Some(Vec3::ZERO);
        let mut step = CueStep {
            vfx: &mut show.world,
            screen: &mut screen,
            presets,
            shakes: &mut fired,
            view_projection: Mat4::IDENTITY,
            root_of: &root_of,
        };
        show.dispatcher.read_step(tick, [event], &mut step);
        fired.0
    }

    /// New camera tuning moves shake IDs, which the table's are resolved against again, so a row still
    /// fires the shake it names. Tuning that loses one the table names is refused, and what fires stays.
    #[test]
    fn new_camera_tuning_resolves_the_tables_shakes_again() {
        let presets = ScreenLibrary::default_effects();
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let shipped = CameraTuning::default_tuning();
        let row = show.table.rows().iter().find_map(|row| row.shake.as_ref()).expect("a row with a shake");
        let named = row.preset.clone();
        let was = shipped.shake_id(&named).expect("in the shipped tuning");
        assert_eq!(shakes_fired(&mut show, &presets, 1), [was]);

        let mut moved = shipped.clone();
        moved.shakes.presets.rotate_left(1);
        let now = moved.shake_id(&named).expect("still there");
        assert_ne!(now, was, "the edit has to move its ID");
        show.take_camera_tuning(moved.clone(), &presets).expect("every name is there");
        assert_eq!(show.shakes.tuning, moved);
        assert_eq!(shakes_fired(&mut show, &presets, 2), [now]);

        let mut losing = moved.clone();
        losing.shakes.presets.retain(|preset| preset.name != named);
        let error = show.take_camera_tuning(losing, &presets).expect_err("the table fires it");
        assert!(matches!(error, CueTableError::UnknownShake { .. }), "{error}");
        assert_eq!(show.shakes.tuning, moved, "it keeps what it has");
        assert_eq!(shakes_fired(&mut show, &presets, 3), [now], "and it still fires");
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
