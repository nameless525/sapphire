//! Cues (M2 E3, spec §5.22's step 8, `docs/m2-plan.md` pillar 3): what the simulation's cues fire, so a
//! hit lands everywhere in one tick. Hitstop, effects, a screen effect, a shake, sound and rumble all
//! start from one cue in one frame.
//!
//! **Presentation, not simulation.** It reads what a step published and feeds nothing back, so nothing
//! here is in the checksum, and the simulation's crates don't link it (`[simulation]` in
//! `xtask/layers.toml`). It sits above them at L5, since it needs `sapphire_gameplay`, the camera and
//! the world alike.
//!
//! # Owns
//!
//! - **The table** ([`CueTable`], a `CueTable` document, schema 1): a cue's name, optionally a hit's
//!   weight, and what fires: `sapphire_vfx` effects, `sapphire_audio` cues, a camera shake, a screen
//!   effect preset, a [`Rumble`] and a voice line by its key (M3 A1). [`CueTable::default_table`] is
//!   `default_cues.ron`, compiled in like the presets it names, so a capture needs no pack mounted to
//!   fire one, and an app's load checks every name at once. The table also names the fight's score and
//!   its voice lines' tables, for the app to load.
//! - **The dispatcher** ([`CueDispatcher`]): resolves those names once, at load and after a reload, then
//!   fires rows with no allocation. [`CueStep`] is what a step fires into.
//! - **Cue marks** (M4 A5, [`Stand`], [`CueMarks`]): a row's effect over a fighter's head
//!   ([`EffectPlace::Crown`]), and the cues no document names: [`DODGE_WINDOW`], fired on a foe as its strike
//!   comes within a perfect dodge's reach, read from the moves, and [`NAMED_PHASE`], on an enemy crossing a
//!   named phase. Every warning a fight gives is a mark of its own shape there, so none relies on colour
//!   alone.
//! - **Rumble's mixing** ([`RumbleMixer`]): SDL holds one rumble per pad, so overlapping hits meet here.
//!   SDL's call is `sapphire_platform`'s, and the app's [`RumbleSink`] makes it.
//!
//! # Doesn't own
//!
//! - Hitstop, which G3 applies in the same step that publishes the cue: it's simulated, and in the
//!   checksum.
//! - What a row fires: effects' documents are `sapphire_vfx`'s, presets are `sapphire_world`'s, shakes
//!   are `sapphire_camera`'s, and sounds, scores and voice lines are documents in `audio.spack`.
//! - Which phase a fight's music is in: the app says, from the simulation's encounters.
//! - Live tuning (T2).
//! - A phone's haptics: a touch device has no pad, and an app there hands the dispatcher `()`.
//!
//! # A frame
//!
//! ```
//! use sapphire_audio::{AudioFrame, AudioSystem, CueRng, Listener, MockBackend};
//! use sapphire_camera::{CameraDirector, CameraTuning};
//! use sapphire_cues::{CueDispatcher, CueStep, CueTable, Stand};
//! use sapphire_gameplay::replay::{SCENARIOS, find_scenario};
//! use sapphire_gameplay::TickActions;
//! use sapphire_look::VfxFrame;
//! use sapphire_math::Mat4;
//! use sapphire_time::ScaledClock;
//! use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
//! use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};
//!
//! // At load: every name the table holds is resolved here, or this fails naming it.
//! let (effects, presets) = (VfxLibrary::default_effects(), ScreenLibrary::default_effects());
//! let tuning = CameraTuning::default_tuning();
//! let table = CueTable::default_table();
//! let mut cues = CueDispatcher::new(&table, &effects, &presets, &tuning).unwrap();
//! # let scenario = find_scenario(&SCENARIOS, "duel").unwrap();
//! # let (mut sim, _) = scenario.start(scenario.config).unwrap();
//! # let mut world = ScaledClock::new();
//! # let mut screen = EffectStack::new(world.new_local_clock());
//! # let mut vfx = VfxWorld::new(effects, VfxBudget::DESKTOP_LOW, 7);
//! # let mut camera = CameraDirector::new(tuning, 7).unwrap();
//! # let mut audio = AudioSystem::new(MockBackend::new());
//! # let mut random = CueRng::new(7);
//! # let mut frame = VfxFrame::default();
//! // Each of a frame's fixed steps, since the next step clears these cues:
//! sim.step(&TickActions::NEUTRAL);
//! let stand_of = |actor| Stand::of(&sim, actor);
//! cues.read_step(sim.tick(), sim.cues(), &mut CueStep {
//!     vfx: &mut vfx,
//!     screen: &mut screen,
//!     presets: &presets,
//!     shakes: &mut camera,
//!     view_projection: Mat4::IDENTITY, // The view's, which puts a hit on screen.
//!     stand_of: &stand_of,
//! });
//! cues.advance_effects(&sim, &mut vfx);
//!
//! // Once a frame, after its steps: what the steps asked for that can't happen in one.
//! audio.update(
//!     &AudioFrame {
//!         dt: 1.0 / 60.0,
//!         listener: Listener::default(),
//!         world_time_scale: 1.0,
//!         emitters: &[],
//!         cues: cues.sounds(),
//!         lines: cues.lines(),
//!         music: None, // A score's phase, if the table names a score.
//!     },
//!     &mut random,
//! );
//! cues.end_frame(1.0 / 60.0, &mut ()); // `()`: no rumble, as on a phone.
//! vfx.write_frame(&mut frame, 1.0); // `alpha`: where the frame draws between its last two steps.
//! ```
//!
//! # When things fire
//!
//! A frame can run several steps, so [`CueDispatcher::read_step`] runs after each, and everything that
//! can take a cue in a step does: an effect has to spawn before the step's `VfxWorld::advance` to show
//! on this tick's frame, and a screen effect or a shake fired with its hit ages with it. Sound and
//! rumble wait for the frame: a warmed `sapphire_audio` cue allocates (11 times, kira's per-sound
//! state), and SDL's call may write to the device. So all of it reaches the player in the frame of the
//! tick, whichever of its steps the tick was, and `tests/pillar.rs` holds that through a real fight.
//!
//! That frame draws where a pose does, `alpha` of the way from the step before to the tick's, so what
//! starts there reads from where it begins, and each frame after reads `alpha` along its own clock's last
//! step (`VfxWorld::write_frame`, `EffectStack::resolve`, `CameraFrame::behind`), so nothing leads the
//! blow (review WA-3). A shake's on the real clock from there, and rises from nothing, so the view moves
//! on the frame after; sound and rumble have no frames of their own to be read at, and start with the
//! frame of the tick.
//!
//! # Choices a table can change
//!
//! - **Effects a hit starts run on the world's clock,** not an actor's. A spawn on a frozen owner emits
//!   nothing until it thaws, so a blow landing on someone still frozen would show its spark late, and
//!   the fighting-game grammar this follows lets a spark burn through the freeze anyway.
//! - **Impact frames are authored in frames per row,** rather than stretched to `HitCue::hitstop`: a
//!   two-tone frame as long as a long freeze reads as a stuck image.

mod dispatch;
mod marks;
mod rumble;
mod table;

pub use dispatch::{
    CueDispatcher, CueStats, CueStep, MAX_FRAME_LINES, MAX_FRAME_SOUNDS, SaidLine, ShakeSink, actor_emitter,
    emitter_of, owner_of,
};
pub use marks::{CROWN_CLEARANCE, CueMarks, DODGE_WINDOW, NAMED_PHASE, Stand, named_phases};
pub use rumble::{MAX_RUMBLES, Rumble, RumbleMixer, RumbleSink};
pub use table::{
    CueRow, CueTable, CueTableError, EffectFire, EffectOwner, EffectPlace, MAX_ROW_EFFECTS, MAX_ROW_SOUNDS,
    ScreenCenter, ScreenFire, ShakeFire,
};
