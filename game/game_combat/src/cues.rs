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
//! - **Sword trails** have no row, because a row keeps no handle to stop one with. [`BladeTrails`] runs
//!   them in the same world of effects, from what the blades are doing.
//!
//! [`Scene::follow_simulation`]: crate::fight::FightShow::follow_simulation

use std::time::Duration;

use sapphire_audio::Emitter;
use sapphire_camera::{CameraDirector, CameraTuning, ShakeId, ShakeStack};
use sapphire_core::Name;
use sapphire_cues::{
    CueDispatcher, CueMarks, CueStats, CueStep, CueTable, CueTableError, RumbleSink, ShakeSink, Stand,
    actor_emitter,
};
use sapphire_ecs::bevy_ecs::world::WorldId;
use sapphire_ecs::prelude::WorldClock;
use sapphire_gameplay::{ActorView, CueEvent, EncounterPhase, Simulation};
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_moves::EnemyPhase;
use sapphire_platform::{GamepadId, Platform};
use sapphire_render::{PostVariants, VfxFrame};
use sapphire_sequencer::FiredCue;
use sapphire_time::{Stopwatch, TICK_HZ};
use sapphire_vfx::{EffectLibrary as VfxLibrary, OwnerSteps, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

use crate::trails::BladeTrails;
use sapphire_stage::audio::{CueSounds, RUNNING, ShowSounds};

/// Seeded, so a capture's sparks land the same way in every run, and so do its shakes.
const SEED: u64 = 0x5EED;

/// What a fight's voice line is captioned by (M4 A2): its key under this, a string the HUD's subtitle shows
/// as it shows a cutscene's line (`content/loc/*/cutscenes.ftl`).
pub const CAPTION_PREFIX: &str = "subtitle-";
/// How long a caption stays up after its line is said, in real seconds: long enough to read the fight's
/// short lines twice over, since nothing says how long each one sounds.
pub const CAPTION_SECONDS: f32 = 2.5;

/// How a hurtbox's surface maps onto the character's: the test moves' capsule is 0.7 m across and 1.75 m
/// tall, and the character about half as wide and 1.55 m tall, so a hit on one's edge lands on the other's.
const ON_THE_CHARACTER: Vec3 = Vec3::new(0.5, 0.88, 0.5);

/// How many seconds a frame `alpha` of the way between its last two steps draws behind the latest:
/// what a shake fired since the last frame starts from. Not a number draws the step before, as
/// `presented_ticks` does.
pub(crate) fn behind_latest(alpha: f32) -> f32 {
    (1.0 - if alpha >= 0.0 { alpha.min(1.0) } else { 0.0 }) / TICK_HZ as f32
}

/// The phase a fight's score plays (M3 A1): `running` while any of its zone's encounters runs, `cleared`
/// once every one has cleared, and `waiting` before and between. A fight with no zone is a duel, which
/// always runs. A boss's named phase stands in for `running` ([`boss_phase`]).
fn music_phase(sim: &Simulation) -> &'static str {
    let Some(zone) = sim.zone() else { return RUNNING };
    let encounters = zone.encounters();
    if encounters.iter().any(|encounter| encounter.phase() == EncounterPhase::Running) {
        RUNNING
    } else if !encounters.is_empty()
        && encounters.iter().all(|encounter| encounter.phase() == EncounterPhase::Cleared)
    {
        "cleared"
    } else {
        "waiting"
    }
}

/// The named phase of a boss in the fight (M3 Z5a): of the first still up, in the simulation's order, whose
/// health is past a phase with a name, the last such phase its kind's document lists. The score plays it in
/// place of `running` if it has a phase of that name ([`CueSounds::boss_phase`]), so a boss's music is two
/// documents, its kind's `phases` and the score's, and no code. Allocates nothing.
fn boss_phase(sim: &Simulation) -> Option<Name> {
    let library = sim.move_library();
    sim.actors().find_map(|actor| {
        let set = actor.moves.set.filter(|_| !actor.fighter.is_defeated())?;
        let mind = library.mind(set).filter(|mind| mind.boss().is_some())?;
        let past = library.phases(Some(set)).phase_of(actor.fighter.health(), actor.fighter.max_health())?;
        let crossed = mind.phases().get(..usize::from(past)).unwrap_or_default();
        crossed.iter().rev().find_map(EnemyPhase::name)
    })
}

/// Where a fight's cues land.
pub(crate) enum CueStage<'a> {
    /// On the one character that stands here, whichever actor they name ([`on_stage`]).
    OneCharacter { stand: Vec3 },
    /// Where the simulation has its actors, which are drawn there: the arena. With a director on, it plays
    /// their shakes. `fighters` are those drawn, the player first, each a warrior's blade to leave a trail,
    /// and the player only with `player_blade`.
    Actors { director: Option<&'a mut CameraDirector>, fighters: &'a [Option<ActorView>], player_blade: bool },
}

/// A fight's voice line on screen (M4 A2): its caption's key, whether the player's side said it, and how
/// long it has left.
#[derive(Debug, Default)]
struct Caption {
    /// Under [`CAPTION_PREFIX`], written in place, so a line no longer than the last allocates nothing.
    key: String,
    by_player: bool,
    left: f32,
}

/// What a caption shows: the string key of the line, and whether the player's side said it, which names
/// the player's character as its speaker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Captioned<'a> {
    pub key: &'a str,
    pub by_player: bool,
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
pub struct PadRumble<'a> {
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

/// What's firing, and what it fires into that the scene doesn't hold itself.
pub struct CueShow {
    table: CueTable,
    /// The sounds of the table it started with: what an app loads for it, once.
    #[cfg(any(test, feature = "dev"))]
    loaded_sounds: Vec<sapphire_assets::AssetId>,
    dispatcher: CueDispatcher,
    /// The cues no document names: a perfect dodge's window and a named phase, for their marks (M4 A5).
    marks: CueMarks,
    world: VfxWorld,
    trails: BladeTrails,
    shakes: OrbitShakes,
    emitters: Vec<Emitter>,
    world_time_scale: f32,
    /// The world, retained-storage run and tick last presented: one boundary is read once.
    tick: Option<(WorldId, u64, u64)>,
    /// The time spent in steps since the last frame was written.
    stepping: Duration,
    /// A new fight began since the last frame ended.
    started_over: bool,
    /// The phase the score plays, as of the latest step: none before one.
    music_phase: Option<&'static str>,
    /// A boss's named phase, as of the latest step, which the score plays in its place if it has it.
    boss_phase: Option<Name>,
    /// The line said last, while its caption's up.
    caption: Caption,
}

impl CueShow {
    /// The shipped table, within `budget`. An error names a row it can't resolve, and a test holds the
    /// shipped libraries to it.
    pub(crate) fn new(budget: VfxBudget, presets: &ScreenLibrary) -> Result<Self, CueTableError> {
        Self::with_table(CueTable::default_table(), VfxLibrary::default_effects(), budget, presets)
    }

    /// The shipped table, firing `effects` within `budget`. An error names a row they don't have.
    pub(crate) fn with_effects(
        effects: VfxLibrary,
        budget: VfxBudget,
        presets: &ScreenLibrary,
    ) -> Result<Self, CueTableError> {
        Self::with_table(CueTable::default_table(), effects, budget, presets)
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
            marks: CueMarks::default(),
            trails: BladeTrails::new(&effects),
            world: VfxWorld::new(effects, budget, SEED),
            shakes: OrbitShakes { stack: ShakeStack::new(SEED), tuning },
            emitters: Vec::new(),
            world_time_scale: 1.0,
            tick: None,
            stepping: Duration::ZERO,
            started_over: false,
            music_phase: None,
            boss_phase: None,
            caption: Caption::default(),
        })
    }

    /// Reserves actor-owned presentation storage before the first fixed step.
    pub(crate) fn prepare(&mut self, sim: &Simulation) {
        let capacity = sim.world().resource::<sapphire_gameplay::ActorRequests>().capacity() as usize;
        self.emitters.reserve(capacity.saturating_sub(self.emitters.len()));
        self.dispatcher.reserve_actors(capacity);
        self.marks.reserve_actors(capacity);
    }

    /// The post variants the table's screen effects draw through.
    pub(crate) fn post_variants(&self, presets: &ScreenLibrary) -> PostVariants {
        self.dispatcher.post_variants(presets)
    }

    /// The table, for what an app loads for it.
    pub fn table(&self) -> &CueTable {
        &self.table
    }

    /// What an app that plays sound loads for the table: its cues, its score and its voice lines.
    pub fn sounds_to_load(&self) -> ShowSounds {
        ShowSounds {
            cues: self.table.sound_cues(),
            score: self.table.music(),
            voices: self.table.voices().to_vec(),
        }
    }

    /// Fires what the simulation's latest step published, and moves the effects by its clocks: call it
    /// after each step. Allocates nothing once warmed.
    pub(crate) fn step(
        &mut self,
        sim: &Simulation,
        screen: &mut EffectStack,
        presets: &ScreenLibrary,
        view_projection: Mat4,
        stage: CueStage<'_>,
    ) {
        let tick = sim.tick();
        let run = (sim.world().id(), sim.run_generation(), tick);
        if self
            .tick
            .is_some_and(|(world, generation, tick)| world != run.0 || generation != run.1 || run.2 < tick)
        {
            self.start_over(screen);
        }
        if self.tick == Some(run) {
            return;
        }
        self.tick = Some(run);
        let stopwatch = Stopwatch::start();
        let (stand, shakes, fighters, player_blade): (Option<Vec3>, &mut dyn ShakeSink, _, _) = match stage {
            CueStage::OneCharacter { stand } => (Some(stand), &mut self.shakes, &[][..], false),
            CueStage::Actors { director: Some(director), fighters, player_blade } => {
                (None, director, fighters, player_blade)
            }
            CueStage::Actors { director: None, fighters, player_blade } => {
                (None, &mut self.shakes, fighters, player_blade)
            }
        };
        // On one character's stage everyone stands where it does, as tall as they are.
        let stand_of = |id| match stand {
            Some(feet) => {
                Some(Stand::of(sim, id).map_or(Stand { feet, crown: feet }, |at| at.moved_to(feet)))
            }
            None => Stand::of(sim, id),
        };
        let mut step =
            CueStep { vfx: &mut self.world, screen, presets, shakes, view_projection, stand_of: &stand_of };
        let cues = sim.cues().map(|cue| stand.map_or(cue, |stand| on_stage(cue, sim, stand)));
        let said_before = self.dispatcher.said().len();
        self.dispatcher.read_step(tick, cues, &mut step);
        let player = fighters.first().and_then(|player| Some(player.as_ref()?.id));
        // The step's last line is the one to read: on the one character every line is the player's.
        if let Some(said) = self.dispatcher.said().skip(said_before).last() {
            let by_player = stand.is_some()
                || said.actor.is_some_and(|actor| {
                    Some(actor) == player || sim.party().slots().any(|slot| slot.actor == actor)
                });
            let caption = &mut self.caption;
            caption.key.clear();
            caption.key.push_str(CAPTION_PREFIX);
            caption.key.push_str(said.key);
            (caption.by_player, caption.left) = (by_player, CAPTION_SECONDS);
        }
        self.marks.read_step(sim, player, &mut self.dispatcher, &mut step);
        let blades = fighters.iter().enumerate();
        let blades = blades.map(|(place, view)| view.as_ref().filter(|_| place > 0 || player_blade));
        self.trails.read_step(blades, sim.move_library(), &mut self.world);
        self.dispatcher.advance_effects(sim, &mut self.world);
        self.emitters.clear();
        self.emitters.extend(sim.actors().map(|actor| {
            let emitter = actor_emitter(&actor);
            Emitter { position: stand.unwrap_or(emitter.position), ..emitter }
        }));
        self.world_time_scale = sim.world().resource::<WorldClock>().0.effective_scale();
        self.music_phase = Some(music_phase(sim));
        self.boss_phase = boss_phase(sim);
        self.stepping += stopwatch.elapsed();
    }

    /// Fires what a timeline's playhead reached this frame, at the places it says: effects into this show's
    /// world, and sounds with the next frame's audio, since a frame's audio runs before its draw. Allocates
    /// nothing.
    pub(crate) fn fire_placed(
        &mut self,
        fired: impl Iterator<Item = FiredCue>,
        screen: &mut EffectStack,
        presets: &ScreenLibrary,
        director: Option<&mut CameraDirector>,
    ) {
        let shakes: &mut dyn ShakeSink = match director {
            Some(director) => director,
            None => &mut self.shakes,
        };
        let mut step = CueStep {
            vfx: &mut self.world,
            screen,
            presets,
            shakes,
            view_projection: Mat4::IDENTITY,
            stand_of: &|_| None,
        };
        for cue in fired {
            self.dispatcher.fire_at(cue.cue, cue.place, &mut step);
        }
    }

    /// Moves the effects by `dt` seconds with the simulation held under a cutscene, which has no step to
    /// move them, and which holds no one's clock: what's running plays out.
    pub(crate) fn advance_held(&mut self, dt: f32) {
        self.world.advance(&OwnerSteps { world: dt, owners: &[] });
    }

    /// Whether a frame has a blade's trail to tell of where it's drawn.
    pub(crate) fn trails_are_running(&self) -> bool {
        self.trails.are_running()
    }

    /// Whether the trail of the fighter at `place`, the player first, plays now.
    #[cfg(test)]
    pub(crate) fn trail_runs_at(&self, place: usize) -> bool {
        self.trails.runs_at(place)
    }

    /// Takes where this frame draws each blade, the player's first, `alpha` of the way from the step
    /// before the latest to the latest, for the trails: before [`Self::write`].
    pub(crate) fn follow_blades(&mut self, drawn: impl IntoIterator<Item = Option<Mat4>>, alpha: f32) {
        let stopwatch = Stopwatch::start();
        self.trails.follow_frame(drawn, alpha, &mut self.world);
        self.stepping += stopwatch.elapsed();
    }

    /// Writes the effects to `frame`, `alpha` of the way from the step before the latest to the latest,
    /// with the time this frame's steps and the write took.
    pub(crate) fn write(&mut self, frame: &mut VfxFrame, alpha: f32) {
        let stopwatch = Stopwatch::start();
        self.world.write_frame(frame, alpha);
        frame.stats.simulation = self.stepping + stopwatch.elapsed();
        self.stepping = Duration::ZERO;
    }

    /// What this frame's steps asked of the audio system.
    pub(crate) fn sounds(&self) -> CueSounds<'_> {
        CueSounds {
            requests: self.dispatcher.sounds(),
            lines: self.dispatcher.lines(),
            emitters: &self.emitters,
            world_time_scale: self.world_time_scale,
            started_over: self.started_over,
            music_phase: self.music_phase,
            boss_phase: self.boss_phase,
        }
    }

    /// Ends the frame's steps, `dt` real seconds after the last frame's, for a frame drawn at `alpha`:
    /// forgets its sounds, tells `rumble` what changed, and returns where its shakes turn and move the
    /// view, having played them by `view_dt`, which a pause holds at zero. A pad's rumble runs on: it's in
    /// the player's hands, not on the screen.
    pub(crate) fn end_frame(
        &mut self,
        dt: f32,
        view_dt: f32,
        alpha: f32,
        rumble: &mut dyn RumbleSink,
    ) -> (Vec3, Vec3) {
        self.dispatcher.end_frame(dt, rumble);
        self.started_over = false;
        self.caption.left = (self.caption.left - view_dt).max(0.0);
        self.shakes.stack.advance(view_dt, behind_latest(alpha));
        self.shakes.stack.sample(&self.shakes.tuning.shakes)
    }

    /// The fight's voice line that's still to be read, if any: up for [`CAPTION_SECONDS`] of the view's time
    /// after it's said, so a pause holds it.
    pub(crate) fn caption(&self) -> Option<Captioned<'_>> {
        let caption = &self.caption;
        (caption.left > 0.0).then_some(Captioned { key: &caption.key, by_player: caption.by_player })
    }

    /// The player's shake setting, 0 to 1: the shakes this plays on a view of its own, and every rumble.
    pub(crate) fn set_shake(&mut self, share: f32) {
        self.shakes.stack.set_setting(share);
        self.dispatcher.set_rumble_setting(share);
    }

    /// Stops any rumble at once: the app lost focus or the foreground.
    pub(crate) fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        self.dispatcher.stop_rumble(rumble);
    }

    pub fn stats(&self) -> CueStats {
        self.dispatcher.stats()
    }

    /// Gets ready for this many fighters, the player among them: see [`BladeTrails::stand_by`].
    pub(crate) fn stand_by(&mut self, fighters: usize) {
        self.trails.stand_by(fighters);
        self.emitters.reserve(fighters);
        self.dispatcher.stand_by(fighters);
    }

    /// A new fight: what the last one set going ends here, rather than playing out over ground its
    /// actors have left. Every screen effect goes, `--post-effect`'s among them. A rumble stops at the
    /// next [`Self::end_frame`], which has a pad to tell, and sounds at the audio system's next update,
    /// which [`Self::sounds`] tells, along with any the last fight had yet to start.
    pub(crate) fn start_over(&mut self, screen: &mut EffectStack) {
        self.world.start_over();
        self.trails.start_over();
        self.shakes.stack = ShakeStack::new(SEED);
        self.dispatcher.start_over();
        self.marks.start_over();
        self.emitters.clear();
        self.world_time_scale = 1.0;
        self.tick = None;
        self.started_over = true;
        self.caption.left = 0.0;
        screen.start_over();
    }

    /// Takes `presets`, or says why not: the table names a preset they don't have. Nothing changes then,
    /// and the caller keeps its presets too.
    #[cfg(any(test, feature = "dev"))]
    pub(crate) fn take_presets(&mut self, presets: &ScreenLibrary) -> Result<(), CueTableError> {
        self.dispatcher.resolve(&self.table, self.world.library(), presets, &self.shakes.tuning)
    }

    /// Takes new effects, or says why not: the table names one they don't have. Every effect playing ends,
    /// since their places in the library may have moved, and a sword trail begins again at the next cut.
    #[cfg(any(test, feature = "dev"))]
    pub(crate) fn take_effects(
        &mut self,
        effects: VfxLibrary,
        presets: &ScreenLibrary,
    ) -> Result<(), CueTableError> {
        self.dispatcher.resolve(&self.table, &effects, presets, &self.shakes.tuning)?;
        self.trails.take_effects(&effects);
        self.world.set_library(effects);
        Ok(())
    }

    /// Takes new camera tuning, or says why not: the table names a shake it doesn't have. A shake's ID is
    /// its place in the tuning's list, so the table's are resolved again, through the dispatcher's one
    /// route for that. Shakes playing finish as they were fired.
    #[cfg(any(test, feature = "dev"))]
    pub(crate) fn take_camera_tuning(
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
    pub(crate) fn take_table(
        &mut self,
        table: CueTable,
        presets: &ScreenLibrary,
    ) -> Result<(), CueTableError> {
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

    /// Restarting at the same public tick must fire the new fight's cues and use its actors.
    #[test]
    fn restarting_at_the_same_tick_matches_a_fresh_show() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        let presets = ScreenLibrary::default_effects();
        let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
        let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
        for _ in 0..600 {
            sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
            if sim.cues().any(|cue| cue.hit.is_some()) {
                break;
            }
        }
        assert!(sim.cues().any(|cue| cue.hit.is_some()), "the fixture reaches a hit");
        let screen = || EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        let (mut old_screen, mut fresh_screen) = (screen(), screen());
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let mut fresh = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        show.prepare(&sim);
        fresh.prepare(&sim);
        let fire = |show: &mut CueShow, screen: &mut EffectStack, stand| {
            show.step(&sim, screen, &presets, Mat4::IDENTITY, CueStage::OneCharacter { stand });
        };
        fire(&mut show, &mut old_screen, Vec3::ZERO);
        assert!(!show.sounds().requests.is_empty(), "the old fight published sound");
        show.start_over(&mut old_screen);
        assert!(show.sounds().emitters.is_empty(), "no positions from the old fight");
        assert!(show.sounds().requests.is_empty());
        assert!(old_screen.is_empty());
        fire(&mut show, &mut old_screen, Vec3::X);
        fire(&mut fresh, &mut fresh_screen, Vec3::X);
        assert_eq!(show.sounds().requests, fresh.sounds().requests);
        assert!(!show.sounds().requests.is_empty(), "the same tick fired again after reset");
        assert!(show.emitters.iter().all(|emitter| emitter.position == Vec3::X));
        let (mut a, mut b) = (VfxFrame::default(), VfxFrame::default());
        show.write(&mut a, 1.0);
        fresh.write(&mut b, 1.0);
        assert!(!a.is_empty(), "the hit made effects");
        assert_eq!(a.sprites, b.sprites, "the reset uses the original random stream");
        assert_eq!(a.ribbon_points, b.ribbon_points);
        assert_eq!(old_screen.resolve(1.0), fresh_screen.resolve(1.0));
        for _ in 0..2 {
            assert_eq!(
                show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ()),
                fresh.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ())
            );
        }
    }

    #[test]
    fn an_independent_simulation_at_the_same_tick_starts_fresh_cues() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        let presets = ScreenLibrary::default_effects();
        let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
        let (mut previous, _) = scenario.start(scenario.config).expect("sets up");
        for _ in 0..600 {
            previous.step(&sapphire_gameplay::TickActions::NEUTRAL);
            if previous.cues().any(|cue| cue.hit.is_some()) {
                break;
            }
        }
        assert!(previous.cues().any(|cue| cue.hit.is_some()));
        let boundary = (previous.world().id(), previous.run_generation(), previous.tick());
        let screen = || EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        let (mut old_screen, mut fresh_screen) = (screen(), screen());
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let mut fresh = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        show.prepare(&previous);
        show.step(
            &previous,
            &mut old_screen,
            &presets,
            Mat4::IDENTITY,
            CueStage::OneCharacter { stand: Vec3::ZERO },
        );
        assert!(!show.sounds().requests.is_empty());
        drop(previous);

        let (mut replacement, _) = scenario.start(scenario.config).expect("sets up");
        for _ in 0..boundary.2 {
            replacement.step(&sapphire_gameplay::TickActions::NEUTRAL);
        }
        assert_ne!(replacement.world().id(), boundary.0);
        assert_eq!((replacement.run_generation(), replacement.tick()), (boundary.1, boundary.2));
        fresh.prepare(&replacement);
        let fire = |show: &mut CueShow, screen: &mut EffectStack| {
            show.step(
                &replacement,
                screen,
                &presets,
                Mat4::IDENTITY,
                CueStage::OneCharacter { stand: Vec3::X },
            );
        };
        fire(&mut show, &mut old_screen);
        fire(&mut fresh, &mut fresh_screen);
        assert!(show.sounds().started_over);
        assert!(!show.sounds().requests.is_empty());
        assert_eq!(show.sounds().requests, fresh.sounds().requests);
        assert!(show.emitters.iter().all(|emitter| emitter.position == Vec3::X));
        let fired = show.stats();
        fire(&mut show, &mut old_screen);
        assert_eq!(show.stats(), fired, "the replacement's first cues fire exactly once");
        let (mut a, mut b) = (VfxFrame::default(), VfxFrame::default());
        show.write(&mut a, 1.0);
        fresh.write(&mut b, 1.0);
        assert!(!a.is_empty());
        assert_eq!(a.sprites, b.sprites);
        assert_eq!(a.ribbon_points, b.ribbon_points);
        assert_eq!(old_screen.resolve(1.0), fresh_screen.resolve(1.0));
    }

    #[test]
    fn a_rewound_simulation_clears_cues_without_an_arena() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        let presets = ScreenLibrary::default_effects();
        let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
        let (mut previous, _) = scenario.start(scenario.config).expect("sets up");
        let (mut next, _) = scenario.start(scenario.config).expect("sets up");
        for _ in 0..140 {
            previous.step(&sapphire_gameplay::TickActions::NEUTRAL);
        }
        next.step(&sapphire_gameplay::TickActions::NEUTRAL);
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let mut screen = EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        show.step(
            &previous,
            &mut screen,
            &presets,
            Mat4::IDENTITY,
            CueStage::OneCharacter { stand: Vec3::X },
        );
        show.step(&next, &mut screen, &presets, Mat4::IDENTITY, CueStage::OneCharacter { stand: Vec3::Y });
        assert!(show.sounds().started_over);
        assert_eq!(show.tick, Some((next.world().id(), next.run_generation(), next.tick())));
        assert!(show.emitters.iter().all(|emitter| emitter.position == Vec3::Y));
        assert!(show.sounds().requests.is_empty(), "the first step has no old hit sounds");
        assert!(screen.is_empty(), "the old fight's screen effects are gone");
    }

    #[test]
    fn a_prepared_show_and_its_restart_do_not_allocate_in_a_step() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        assert!(sapphire_core::alloc::is_installed());
        let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
        let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
        let presets = ScreenLibrary::default_effects();
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let mut screen = EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        show.prepare(&sim);
        for tick in 0..300 {
            sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                if tick == 150 {
                    show.start_over(&mut screen);
                }
                let stage = CueStage::Actors { director: None, fighters: &[None; 2], player_blade: false };
                show.step(&sim, &mut screen, &presets, Mat4::IDENTITY, stage);
                let _ = show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ());
            });
            assert_eq!(counts.allocations, 0, "presentation step {tick}");
        }
    }

    #[test]
    fn preparing_a_larger_actor_pool_survives_effect_reload() {
        assert!(sapphire_core::alloc::is_installed());
        let small = Simulation::new(sapphire_gameplay::SimConfig { max_actors: 8, ..Default::default() });
        let large = Simulation::new(sapphire_gameplay::SimConfig { max_actors: 128, ..Default::default() });
        let presets = ScreenLibrary::default_effects();
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        show.prepare(&small);
        show.prepare(&large);
        assert!(show.emitters.capacity() >= 128);
        show.take_effects(VfxLibrary::default_effects(), &presets).expect("the same names");
        let ((), counts) = sapphire_core::alloc::count_in_scope(|| show.prepare(&large));
        assert_eq!(counts.allocations, 0);
    }

    /// New effects reach what plays a fight's, as long as the table still finds its names.
    #[test]
    fn new_effects_are_taken_if_the_table_finds_its_names() {
        let presets = ScreenLibrary::default_effects();
        let shipped = VfxLibrary::default_effects;
        let mut show =
            CueShow::with_table(CueTable::default_table(), shipped(), VfxBudget::DESKTOP_LOW, &presets)
                .expect("the shipped table finds them");
        show.take_effects(shipped(), &presets).expect("the same names");
        let without = VfxLibrary::from_ron(
            "(type: \"sapphire::vfx::EffectLibrary\", version: 1, data: (effects: [(name: \"other\", \
             emitters: [(name: \"flies\", spawn: Burst(count: 1), lifetime: (1.0, 1.0), \
             motion: (speed: (1.0, 1.0)), size: [(0.0, 0.1)], color: [(0.0, (1.0, 1.0, 1.0))])])]))",
        )
        .expect("valid");
        assert!(show.take_effects(without, &presets).is_err(), "the table's sparks are gone");
        assert!(show.world.library().find("hit_spark").is_some(), "and it keeps what it has");
    }

    /// A hit on the target's chest, a little off its middle, lands there on the character, whichever
    /// way and wherever the fight had the target stand.
    #[test]
    fn a_hit_lands_on_the_character_as_on_its_target() {
        let scenario =
            sapphire_gameplay::replay::find_scenario(&sapphire_gameplay::replay::SCENARIOS, "duel")
                .expect("a scenario");
        let (sim, cast) = scenario.start(scenario.config).expect("sets up");
        // The opponent stands 4 m along +Z, half a turn round, facing the player.
        let target = sim.actor(cast[1]).expect("there");
        assert!((target.yaw.abs() - std::f32::consts::PI).abs() < 1e-5);
        let event = CueEvent {
            actor: ActorId::from_raw(cast[0].raw()).expect("not zero"),
            cue: Name::new("hit_landed"),
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
        let scenario =
            sapphire_gameplay::replay::find_scenario(&sapphire_gameplay::replay::SCENARIOS, "duel")
                .expect("a scenario");
        let (sim, cast) = scenario.start(scenario.config).expect("sets up");
        let event = CueEvent {
            actor: cast[0],
            cue: Name::new("swing"),
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
            let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
            let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
            let mut director = director;
            for _ in 0..140 {
                sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
                let stage = CueStage::Actors {
                    director: director.as_deref_mut(),
                    fighters: &[],
                    player_blade: false,
                };
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

    /// A new fight, in the frame's second step, say: the sounds the last one's first step asked for go
    /// with it, the audio system hears once that it started over, and the new fight's sounds still play.
    #[test]
    fn a_new_fight_tells_the_audio_system_once() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        let presets = ScreenLibrary::default_effects();
        let mut screen = EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
        let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
        assert!(!show.sounds().started_over);
        let until_a_sound = |sim: &mut Simulation, show: &mut CueShow, screen: &mut EffectStack| {
            for _ in 0..600 {
                sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
                let stage = CueStage::Actors { director: None, fighters: &[], player_blade: false };
                show.step(sim, screen, &presets, Mat4::IDENTITY, stage);
                if !show.sounds().requests.is_empty() {
                    return;
                }
                let _ = show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ());
            }
            panic!("a whole fight's start with no sound");
        };
        until_a_sound(&mut sim, &mut show, &mut screen);

        show.start_over(&mut screen);
        assert!(show.sounds().started_over);
        assert!(show.sounds().requests.is_empty(), "the last fight's, not yet started");

        let _ = show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ());
        assert!(!show.sounds().started_over, "once");
        until_a_sound(&mut sim, &mut show, &mut screen);
        assert!(!show.sounds().started_over);
    }

    /// The shipped table resolves against the libraries a scene starts with.
    #[test]
    fn a_show_starts_on_the_shipped_libraries() {
        let presets = ScreenLibrary::default_effects();
        let show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("every name's there");
        assert_ne!(show.post_variants(&presets), PostVariants::NONE);
        assert!(!show.table().sound_cues().is_empty());
    }

    /// A fight's voice line is captioned (M4 A2) under its key, for [`CAPTION_SECONDS`] of the view's time,
    /// which a pause holds, and not past a start over. On the one character, every line is the player's.
    #[test]
    fn a_said_line_is_captioned_while_its_time_lasts() {
        use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

        let presets = ScreenLibrary::default_effects();
        let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
        let (mut sim, _) = scenario.start(scenario.config).expect("sets up");
        let landed = || Name::new("hit_landed");
        for _ in 0..600 {
            sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
            if sim.cues().any(|cue| cue.cue == landed()) {
                break;
            }
        }
        assert!(sim.cues().any(|cue| cue.cue == landed()), "the fixture reaches a hit");
        let speaking = CueTable::from_ron(
            r#"(type: "sapphire::cues::CueTable", version: 1, data: (rows: [
                (cue: "hit_landed", line: Some("perfect-dodge")),
            ]))"#,
        )
        .expect("valid on its own");
        let mut screen = EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        let mut show =
            CueShow::with_table(speaking, VfxLibrary::default_effects(), VfxBudget::DESKTOP_LOW, &presets)
                .expect("resolves");
        show.prepare(&sim);
        assert_eq!(show.caption(), None);
        let stage = || CueStage::OneCharacter { stand: Vec3::ZERO };
        show.step(&sim, &mut screen, &presets, Mat4::IDENTITY, stage());
        let said = Captioned { key: "subtitle-perfect-dodge", by_player: true };
        assert_eq!(show.caption(), Some(said));
        // Paused, it stays; played, it goes once its time's up.
        show.end_frame(10.0, 0.0, 1.0, &mut ());
        assert_eq!(show.caption(), Some(said));
        show.end_frame(1.0, CAPTION_SECONDS - 0.5, 1.0, &mut ());
        assert_eq!(show.caption(), Some(said));
        show.end_frame(1.0, 0.5, 1.0, &mut ());
        assert_eq!(show.caption(), None);
        show.start_over(&mut screen);
        assert_eq!(show.caption(), None, "a fight started over says nothing yet");
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
            cue: Name::intern(&row.cue),
            tick,
            local_time: SimTime::ZERO,
            move_time: SimDuration::ZERO,
            hit: row.weight.map(hit),
        };
        let mut clock = sapphire_time::ScaledClock::new();
        let mut screen = EffectStack::new(clock.new_local_clock());
        let mut fired = FiredShakes::default();
        let stand_of = |_| Some(Stand { feet: Vec3::ZERO, crown: Vec3::Y });
        let mut step = CueStep {
            vfx: &mut show.world,
            screen: &mut screen,
            presets,
            shakes: &mut fired,
            view_projection: Mat4::IDENTITY,
            stand_of: &stand_of,
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

    /// The committed boss fight (M3 Z5a): its boss's named phase is what the show asks the score for in place
    /// of `running`, from the step its health crosses it, said as the cue of its name, and not before. The
    /// name is its kind's document's alone.
    #[test]
    fn a_bosss_named_phase_is_asked_for_from_the_step_it_crosses() {
        use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};

        let presets = ScreenLibrary::default_effects();
        let mut screen = EffectStack::new(sapphire_time::ScaledClock::new().new_local_clock());
        let mut show = CueShow::new(VfxBudget::DESKTOP_LOW, &presets).expect("resolves");
        let bytes = include_bytes!("../../../tests/replays/arena-boss.sreplay");
        let mut player =
            Player::new(Replay::decode(bytes).expect("reads"), &SCENARIOS).expect("recorded on these");
        let boss = player.cast()[1];
        let enraged = Name::new("enraged");
        let (mut crossed, mut asked) = (None, None);
        while player.step() {
            let sim = player.simulation();
            let stage = CueStage::Actors { director: None, fighters: &[], player_blade: false };
            show.step(sim, &mut screen, &presets, Mat4::IDENTITY, stage);
            if sim.cues().any(|cue| cue.actor == boss && cue.cue == enraged) {
                crossed.get_or_insert(sim.tick());
            }
            let sounds = show.sounds();
            assert_eq!(sounds.music_phase, Some(RUNNING), "a duel always runs");
            if sounds.boss_phase.is_some() {
                assert_eq!(sounds.boss_phase, Some(enraged));
                asked.get_or_insert(sim.tick());
            }
            let _ = show.end_frame(1.0 / 60.0, 1.0 / 60.0, 1.0, &mut ());
        }
        assert!(crossed.is_some(), "the recording takes it past half health");
        assert_eq!(asked, crossed, "asked for from the step it crossed");
    }

    /// The shipped score's phases (M3 Z5a): past the three a fight's encounters choose, each is a phase a
    /// shipped boss names, so none is out of reach, and there's one, so a shipped boss's phase is heard. Which
    /// boss is content's to say: no code names one.
    #[test]
    fn every_phase_the_shipped_score_has_past_a_fights_is_a_bosss() {
        fn kinds_under(folder: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(folder).expect("a folder") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    kinds_under(&path, found);
                } else if path.to_string_lossy().ends_with(".enemy.ron") {
                    found.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let read = |path: &std::path::Path| std::fs::read_to_string(path).expect("reads");
        let music = root.join("content/audio/generated/music");
        let score = sapphire_assets::score::Score::from_document(&read(&music.join("lantern.score.ron")))
            .expect("reads");
        let shipped = CueTable::default_table().music().expect("the table names a score");
        let sidecar = read(&music.join("lantern.score.ron.meta"));
        assert!(sidecar.contains(&shipped.to_string()), "that's the score the table plays");

        let mut kinds = Vec::new();
        kinds_under(&root.join("content/combat"), &mut kinds);
        let mut bosses_phases = Vec::new();
        for kind in &kinds {
            let document = sapphire_moves::EnemyDocument::parse(&read(kind)).expect("a kind reads");
            if document.boss {
                bosses_phases.extend(document.phases.into_iter().filter_map(|phase| phase.name));
            }
        }
        let past: Vec<&str> = score
            .phases
            .iter()
            .map(|phase| phase.name.as_str())
            .filter(|name| ![RUNNING, "waiting", "cleared"].contains(name))
            .collect();
        assert!(!past.is_empty(), "the score plays some boss's phase");
        for name in past {
            assert!(bosses_phases.iter().any(|named| named == name), "no shipped boss names `{name}`");
        }
    }
}
