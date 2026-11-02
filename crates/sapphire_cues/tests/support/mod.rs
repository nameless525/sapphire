//! A real fight, with every real sink a cue fires into but the pad, which nothing here can feel: what
//! the tests read and what `CueDispatcher`'s docs describe.

#![allow(dead_code, reason = "each test binary uses its own part of this")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Cue;
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{AudioFrame, AudioSystem, CueRng, Emitter, Listener, MockBackend, MockCall};
use sapphire_camera::{CameraDirector, CameraFrame, CameraTuning, Subject};
use sapphire_cues::{CueDispatcher, CueStep, CueTable, RumbleSink, actor_emitter};
use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};
use sapphire_look::VfxFrame;
use sapphire_math::{Mat4, Vec2, Vec3};
use sapphire_physics::PhysicsWorld;
use sapphire_render::PostEffects;
use sapphire_time::ScaledClock;
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

pub const FRAME: f32 = 1.0 / 60.0;

/// Writes down what the pad was told.
#[derive(Default)]
pub struct FakePad(pub Vec<(f32, f32, Duration)>);

impl RumbleSink for FakePad {
    fn rumble(&mut self, low: f32, high: f32, duration: Duration) {
        self.0.push((low, high, duration));
    }
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// What the player got in one frame, from each sink itself.
#[derive(Clone, Debug, Default)]
pub struct Reached {
    /// Effects the world started: `VfxWorld`'s own count.
    pub particles: usize,
    /// Screen effect layers pushed.
    pub screen_layers: u64,
    /// What the frame's post would draw.
    pub post: Option<PostEffects>,
    pub shakes: u32,
    /// Voices started on the mixer.
    pub voices: usize,
    /// What the pad was told.
    pub rumbles: usize,
    /// Fighters holding still at this frame's end.
    pub in_hitstop: usize,
    /// How many of this frame's cues carried a hit.
    pub hits: usize,
    /// The view moved from where it would be with no shake.
    pub view_moved: bool,
}

/// A replay, and everything its cues fire into.
pub struct Fight {
    pub player: Player,
    pub cues: CueDispatcher,
    pub vfx: VfxWorld,
    pub screen: EffectStack,
    world: ScaledClock,
    pub presets: ScreenLibrary,
    pub camera: CameraDirector,
    /// The same camera, which no cue fires a shake on.
    still_camera: CameraDirector,
    empty: PhysicsWorld,
    pub audio: AudioSystem<MockBackend>,
    random: CueRng,
    pub pad: FakePad,
    frame: VfxFrame,
    emitters: Vec<Emitter>,
    /// The mixer's calls seen so far: every voice it starts plays straight out, so none stays to fill it.
    calls_seen: usize,
}

fn player(name: &str) -> Player {
    let bytes = fs::read(repo().join("tests/replays").join(format!("{name}.sreplay"))).expect("committed");
    Player::new(Replay::decode(&bytes).expect("reads"), SCENARIOS).expect("sets up")
}

impl Fight {
    /// `tests/replays/<name>.sreplay`, under the shipped table with the cue documents it names.
    pub fn new(name: &str) -> Self {
        let table = CueTable::default_table();
        let (effects, presets) = (VfxLibrary::default_effects(), ScreenLibrary::default_effects());
        let tuning = CameraTuning::default_tuning();
        let cues = CueDispatcher::new(&table, &effects, &presets, &tuning).expect("resolves");
        let mut audio = AudioSystem::new(MockBackend::new());
        load_cue_documents(&mut audio);
        audio.backend_mut().reserve_calls(1 << 14);

        let mut world = ScaledClock::new();
        let screen = EffectStack::new(world.new_local_clock());
        Self {
            player: player(name),
            cues,
            vfx: VfxWorld::new(effects, VfxBudget::DESKTOP_LOW, 7),
            screen,
            world,
            presets,
            camera: CameraDirector::new(tuning.clone(), 7).expect("valid"),
            still_camera: CameraDirector::new(tuning, 7).expect("valid"),
            empty: PhysicsWorld::new(Vec::new(), 0).expect("empty"),
            audio,
            random: CueRng::new(7),
            pad: FakePad(Vec::with_capacity(1 << 12)),
            frame: VfxFrame::default(),
            emitters: Vec::with_capacity(16),
            calls_seen: 0,
        }
    }

    /// The same fight from its start, on the same sinks, which have grown whatever they grow.
    pub fn restart(&mut self, name: &str) {
        self.player = player(name);
    }

    /// One fixed step and what fires in it: the hits it published, or `None` past the replay's end.
    pub fn step(&mut self) -> Option<usize> {
        self.player.step().then(|| self.fire_step())
    }

    /// What fires in the step just taken.
    pub fn fire_step(&mut self) -> usize {
        self.world.tick();
        self.screen.tick(&self.world);
        let sim = self.player.simulation();
        let hits = sim.cues().filter(|cue| cue.hit.is_some()).count();
        let root_of = |actor| sim.actor(actor).map(|view| view.position);
        self.cues.read_step(
            sim.tick(),
            sim.cues(),
            &mut CueStep {
                vfx: &mut self.vfx,
                screen: &mut self.screen,
                presets: &self.presets,
                shakes: &mut self.camera,
                view_projection: Mat4::IDENTITY,
                root_of: &root_of,
            },
        );
        self.cues.advance_effects(sim, &mut self.vfx);
        hits
    }

    /// What ends a frame, after its steps.
    pub fn end_frame(&mut self) -> (PostEffects, bool) {
        let sim = self.player.simulation();
        self.emitters.clear();
        self.emitters.extend(sim.actors().map(|actor| actor_emitter(&actor)));
        self.audio.update(
            &AudioFrame {
                dt: FRAME,
                listener: Listener::default(),
                world_time_scale: 1.0,
                emitters: &self.emitters,
                cues: self.cues.sounds(),
            },
            &mut self.random,
        );
        self.play_voices_out();
        self.cues.end_frame(FRAME, &mut self.pad);
        self.vfx.write_frame(&mut self.frame, 1.0);
        let post = self.screen.resolve(1.0);
        let camera_frame = CameraFrame {
            dt: FRAME,
            behind: 0.0,
            player: Subject { position: Vec3::ZERO, height: 1.7, radius: 0.35 },
            player_yaw: 0.0,
            target: None,
            look: Vec2::ZERO,
            zoom: 0.0,
            aspect: 16.0 / 9.0,
        };
        let shaken = self.camera.update(&camera_frame, &self.empty);
        let still = self.still_camera.update(&camera_frame, &self.empty);
        (post, shaken != still)
    }

    /// A frame of `steps` fixed steps, or as many as the replay has left. `None` past its end.
    pub fn frame(&mut self, steps: u32) -> Option<Reached> {
        let mut reached = Reached::default();
        let pushed = self.screen.stats().pushed;
        let (rumbles, voices) = (self.pad.0.len(), self.voices_started());
        let mut stepped = false;
        for _ in 0..steps {
            let Some(hits) = self.step() else { break };
            reached.hits += hits;
            stepped = true;
        }
        if !stepped {
            return None;
        }
        let shakes = self.camera.take_stats().shakes_fired;
        let (post, view_moved) = self.end_frame();
        reached.particles = self.frame.sprites.len();
        reached.screen_layers = self.screen.stats().pushed - pushed;
        reached.post = Some(post);
        reached.shakes = shakes;
        reached.voices = self.voices_started() - voices;
        reached.rumbles = self.pad.0.len() - rumbles;
        reached.view_moved = view_moved;
        let sim = self.player.simulation();
        reached.in_hitstop = sim.actors().filter(|actor| actor.hitstop_ticks > 0).count();
        Some(reached)
    }

    fn play_voices_out(&mut self) {
        let backend = self.audio.backend_mut();
        for index in self.calls_seen..backend.calls().len() {
            if let MockCall::StartVoice { slot, .. } = backend.calls()[index] {
                backend.finish(slot);
            }
        }
        self.calls_seen = backend.calls().len();
    }

    /// Voices started on the mixer so far.
    pub fn voices_started(&self) -> usize {
        let started = |call: &&MockCall| matches!(call, MockCall::StartVoice { .. });
        self.audio.backend().calls().iter().filter(started).count()
    }
}

/// Adds every cue the shipped table names, from its document in `content/audio`, each sound a short
/// stand-in: the mixer's a mock, and it's what's asked of it that counts.
fn load_cue_documents(audio: &mut AudioSystem<MockBackend>) {
    let folder = repo().join("content/audio");
    let silence = DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.0; 480] };
    for entry in fs::read_dir(folder).expect("reads") {
        let path = entry.expect("reads").path();
        if !path.to_string_lossy().ends_with(".cue.ron") {
            continue;
        }
        let cue = Cue::from_document(&fs::read_to_string(&path).expect("reads")).expect("a cue document");
        let sidecar = fs::read_to_string(format!("{}.meta", path.display())).expect("has a sidecar");
        let id = sidecar.split('"').nth(3).expect("type, then id").parse::<AssetId>().expect("an ID");
        for &sound in &cue.sounds {
            audio.add_sound(sound, &silence);
        }
        audio.add_cue(id, cue);
    }
}
