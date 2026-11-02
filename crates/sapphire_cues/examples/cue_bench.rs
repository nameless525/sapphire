//! What firing a fight's cues costs a frame: M2 E3's measurement.
//!
//! ```text
//! cargo run --release -p sapphire_cues --example cue_bench
//! ```
//!
//! Plays `tests/replays/fight.sreplay` a step to a frame under the shipped table, with every sink real
//! but the pad: effects at Desktop-High's budget, a screen effect stack, a camera director's shakes, and
//! kira on an offline output holding the real sounds, decoded from the real `.ogg` files, so the cue
//! documents are read as they ship. The simulation's own steps aren't timed.
//!
//! It sorts frames by what's happening: none of the effects alive and nothing landing (`quiet`), effects
//! playing out (`effects live`), and a hit landing (`a hit`). For each it reports the dispatcher's time
//! (`read_step`, `advance_effects`, `write_frame` and `end_frame`) and the audio system's (`update`), as
//! the lowest median and the lowest 99th percentile over the rounds, since other work on the machine only
//! ever adds. It then counts allocations per frame on the calling thread, once warmed.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::alloc::System;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Cue;
use sapphire_assets::sound::decode_vorbis;
use sapphire_audio::{AudioFrame, AudioSystem, CueRng, Emitter, KiraBackend, Listener, OfflineOutput};
use sapphire_camera::{CameraDirector, CameraTuning};
use sapphire_core::alloc::{CountingAllocator, count_in_scope};
use sapphire_cues::{CueDispatcher, CueStep, CueTable, actor_emitter};
use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};
use sapphire_look::VfxFrame;
use sapphire_math::Mat4;
use sapphire_time::{ScaledClock, Stopwatch};
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget, VfxWorld};
use sapphire_world::screen_effects::{EffectLibrary as ScreenLibrary, EffectStack};

#[global_allocator]
static ALLOCATOR: CountingAllocator<System> = CountingAllocator::new(System);

const ROUNDS: usize = 7;
const RATE: u32 = 48_000;
const FRAME: f32 = 1.0 / 60.0;
/// A frame's samples, in stereo.
const BLOCK: usize = 800 * 2;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.ogg` under `folder`, by the ID its sidecar gives it.
fn sounds_in(folder: &Path, sounds: &mut HashMap<AssetId, PathBuf>) {
    for entry in fs::read_dir(folder).expect("reads") {
        let path = entry.expect("reads").path();
        if path.is_dir() {
            sounds_in(&path, sounds);
        } else if path.extension().is_some_and(|extension| extension == "ogg") {
            let sidecar = fs::read_to_string(format!("{}.meta", path.display())).expect("has a sidecar");
            let id = sidecar.split('"').nth(3).expect("type, then id").parse().expect("an ID");
            sounds.insert(id, path);
        }
    }
}

/// An audio system holding every cue in `content/audio` and the real sounds they name.
fn audio() -> AudioSystem<KiraBackend<OfflineOutput>> {
    let Ok(backend) = KiraBackend::<OfflineOutput>::new(RATE);
    let mut audio = AudioSystem::new(backend);
    let mut sounds = HashMap::new();
    sounds_in(&repo().join("content/third_party"), &mut sounds);
    for entry in fs::read_dir(repo().join("content/audio")).expect("reads") {
        let path = entry.expect("reads").path();
        if !path.to_string_lossy().ends_with(".cue.ron") {
            continue;
        }
        let cue = Cue::from_document(&fs::read_to_string(&path).expect("reads")).expect("a cue document");
        let sidecar = fs::read_to_string(format!("{}.meta", path.display())).expect("has a sidecar");
        let id = sidecar.split('"').nth(3).expect("type, then id").parse::<AssetId>().expect("an ID");
        for sound in &cue.sounds {
            let bytes = fs::read(&sounds[sound]).expect("reads");
            audio.add_sound(*sound, &decode_vorbis(&bytes).expect("decodes"));
        }
        audio.add_cue(id, cue);
    }
    audio
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Quiet,
    Live,
    Hit,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Quiet, Kind::Live, Kind::Hit];

    fn name(self) -> &'static str {
        match self {
            Kind::Quiet => "quiet",
            Kind::Live => "effects live",
            Kind::Hit => "a hit",
        }
    }
}

/// One frame's measurement.
#[derive(Clone, Copy)]
struct Sample {
    kind: Kind,
    dispatch: Duration,
    audio: Duration,
    dispatch_allocations: u64,
    audio_allocations: u64,
    /// Cues this frame asked the audio system to start.
    sounds: usize,
    sprites: usize,
}

/// Everything a fight fires into, kept across its rounds so they all count as warmed.
struct Sinks {
    table: CueTable,
    effects: VfxLibrary,
    presets: ScreenLibrary,
    tuning: CameraTuning,
    audio: AudioSystem<KiraBackend<OfflineOutput>>,
    block: Vec<f32>,
    random: CueRng,
    frame: VfxFrame,
    emitters: Vec<Emitter>,
}

/// One whole fight.
fn fight(sinks: &mut Sinks, samples: &mut Vec<Sample>) {
    let bytes = fs::read(repo().join("tests/replays/fight.sreplay")).expect("committed");
    let mut player = Player::new(Replay::decode(&bytes).expect("reads"), SCENARIOS).expect("sets up");
    let mut cues =
        CueDispatcher::new(&sinks.table, &sinks.effects, &sinks.presets, &sinks.tuning).expect("resolves");
    let mut vfx = VfxWorld::new(sinks.effects.clone(), VfxBudget::DESKTOP_HIGH, 7);
    let mut world = ScaledClock::new();
    let mut screen = EffectStack::new(world.new_local_clock());
    let mut camera = CameraDirector::new(sinks.tuning.clone(), 7).expect("valid");
    let mut pad = ();
    samples.clear();
    while player.step() {
        world.tick();
        screen.tick(&world);
        let sim = player.simulation();
        let hit = sim.cues().any(|cue| cue.hit.is_some());
        let root_of = |actor| sim.actor(actor).map(|view| view.position);
        let live = !sinks.frame.sprites.is_empty();
        let kind = if hit {
            Kind::Hit
        } else if live {
            Kind::Live
        } else {
            Kind::Quiet
        };
        let timer = Stopwatch::start();
        let ((), step) = count_in_scope(|| {
            cues.read_step(
                sim.tick(),
                sim.cues(),
                &mut CueStep {
                    vfx: &mut vfx,
                    screen: &mut screen,
                    presets: &sinks.presets,
                    shakes: &mut camera,
                    view_projection: Mat4::IDENTITY,
                    root_of: &root_of,
                },
            );
            cues.advance_effects(sim, &mut vfx);
            vfx.write_frame(&mut sinks.frame, 1.0);
        });
        let mut dispatch = timer.elapsed();

        sinks.emitters.clear();
        sinks.emitters.extend(sim.actors().map(|actor| actor_emitter(&actor)));
        let sounds = cues.sounds().len();
        let audio_frame = AudioFrame {
            dt: FRAME,
            listener: Listener::default(),
            world_time_scale: 1.0,
            emitters: &sinks.emitters,
            cues: cues.sounds(),
        };
        let timer = Stopwatch::start();
        let ((), update) = count_in_scope(|| sinks.audio.update(&audio_frame, &mut sinks.random));
        let audio = timer.elapsed();
        sinks.audio.backend_mut().output_mut().render_into(&mut sinks.block);

        let timer = Stopwatch::start();
        let ((), end) = count_in_scope(|| cues.end_frame(FRAME, &mut pad));
        dispatch += timer.elapsed();

        samples.push(Sample {
            kind,
            dispatch,
            audio,
            dispatch_allocations: step.allocations + end.allocations,
            audio_allocations: update.allocations,
            sounds,
            sprites: sinks.frame.sprites.len(),
        });
    }
}

fn percentile(times: &mut [Duration], at: f64) -> Duration {
    times.sort_unstable();
    times[((times.len() - 1) as f64 * at) as usize]
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn main() {
    let mut sinks = Sinks {
        table: CueTable::default_table(),
        effects: VfxLibrary::default_effects(),
        presets: ScreenLibrary::default_effects(),
        tuning: CameraTuning::default_tuning(),
        audio: audio(),
        block: vec![0.0; BLOCK],
        random: CueRng::new(7),
        frame: VfxFrame::default(),
        emitters: Vec::with_capacity(16),
    };
    let mut samples = Vec::with_capacity(4096);
    // A first fight to warm what's grown once.
    fight(&mut sinks, &mut samples);
    let mut best: HashMap<&str, [Duration; 4]> = HashMap::new();
    for _ in 0..ROUNDS {
        fight(&mut sinks, &mut samples);
        for kind in Kind::ALL {
            let of_kind = || samples.iter().filter(|sample| sample.kind == kind);
            let mut dispatch: Vec<_> = of_kind().map(|sample| sample.dispatch).collect();
            let mut audio: Vec<_> = of_kind().map(|sample| sample.audio).collect();
            if dispatch.is_empty() {
                continue;
            }
            let now = [
                percentile(&mut dispatch, 0.5),
                percentile(&mut dispatch, 0.99),
                percentile(&mut audio, 0.5),
                percentile(&mut audio, 0.99),
            ];
            let entry = best.entry(kind.name()).or_insert(now);
            for (kept, new) in entry.iter_mut().zip(now) {
                *kept = (*kept).min(new);
            }
        }
    }
    println!("over {ROUNDS} rounds of `fight`, {} frames each:", samples.len());
    for kind in Kind::ALL {
        let of_kind: Vec<_> = samples.iter().filter(|sample| sample.kind == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        let times = best[kind.name()];
        let dispatch_allocations: u64 = of_kind.iter().map(|sample| sample.dispatch_allocations).sum();
        let audio_allocations: u64 = of_kind.iter().map(|sample| sample.audio_allocations).sum();
        let sounds: usize = of_kind.iter().map(|sample| sample.sounds).sum();
        let sprites = of_kind.iter().map(|sample| sample.sprites).max().unwrap_or(0);
        println!(
            "{:>12} ({:>4} frames, at most {sprites:>3} sprites): dispatch {:>6.2} us, p99 {:>6.2} us; audio \
             {:>6.2} us, p99 {:>6.2} us; allocations: dispatch {dispatch_allocations}, audio \
             {audio_allocations} for {sounds} cues",
            kind.name(),
            of_kind.len(),
            micros(times[0]),
            micros(times[1]),
            micros(times[2]),
            micros(times[3]),
        );
    }
}
