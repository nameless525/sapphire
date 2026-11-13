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
//! Each round starts a fresh dispatcher, effects world, camera, audio mixer and seeded RNG.
//! Libraries are read-only during each round. The first frame's dispatcher allocation is included and reported;
//! a process warmup fight does not imply that newly constructed objects are warmed. Frame categories
//! use this frame's produced sprites and hits. Every round reports its own distribution, without
//! selecting independent minima. `--raw PATH` exports all per-frame observations as CSV.
//! Timings include clocks and allocation-counter scopes, and exclude simulation, emitter gathering,
//! offline mixing and setup. These are instrumented calling-thread costs, not device callback times.

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
use sapphire_cues::{CueDispatcher, CueStep, CueTable, Stand, actor_emitter};
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
    tick: u64,
    checksum: u64,
    audio_fingerprint: u64,
}

/// Immutable libraries and one round's mutable sinks. Reconstructed between independent fights.
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
    let mut player = Player::new(Replay::decode(&bytes).expect("reads"), &SCENARIOS).expect("sets up");
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
        let stand_of = |actor| Stand::of(sim, actor);
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
                    stand_of: &stand_of,
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
            lines: cues.lines(),
            speech: None,
            music: None,
        };
        let timer = Stopwatch::start();
        let ((), update) = count_in_scope(|| sinks.audio.update(&audio_frame, &mut sinks.random));
        let audio = timer.elapsed();
        sinks.audio.backend_mut().output_mut().render_into(&mut sinks.block);

        let timer = Stopwatch::start();
        let ((), end) = count_in_scope(|| cues.end_frame(FRAME, &mut pad));
        dispatch += timer.elapsed();

        let kind = if hit {
            Kind::Hit
        } else if !sinks.frame.sprites.is_empty() {
            Kind::Live
        } else {
            Kind::Quiet
        };
        // Outside measured scopes: retain a deterministic witness that every round rendered the
        // same audio, rather than trusting only cue counts while mixer/RNG state drifts.
        let audio_fingerprint = sinks.block.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, sample| {
            (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x100_0000_01b3)
        });
        samples.push(Sample {
            kind,
            dispatch,
            audio,
            dispatch_allocations: step.allocations + end.allocations,
            audio_allocations: update.allocations,
            sounds,
            sprites: sinks.frame.sprites.len(),
            tick: sim.tick(),
            checksum: player.trail().end(),
            audio_fingerprint,
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

fn fresh_sinks() -> Sinks {
    Sinks {
        table: CueTable::default_table(),
        effects: VfxLibrary::default_effects(),
        presets: ScreenLibrary::default_effects(),
        tuning: CameraTuning::default_tuning(),
        audio: audio(),
        block: vec![0.0; BLOCK],
        random: CueRng::new(7),
        frame: VfxFrame::default(),
        emitters: Vec::with_capacity(16),
    }
}

fn report(round: usize, samples: &[Sample]) {
    let first = samples.first().expect("the fight has frames");
    println!(
        "round {round}: {} frames, checksum {:016x}; first frame dispatch {:.2} us / {} allocations, audio {:.2} us / {} allocations",
        samples.len(),
        samples.last().expect("frames").checksum,
        micros(first.dispatch),
        first.dispatch_allocations,
        micros(first.audio),
        first.audio_allocations
    );
    for kind in Kind::ALL {
        let of_kind: Vec<_> = samples.iter().filter(|sample| sample.kind == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        let dispatch_allocations: u64 = of_kind.iter().map(|sample| sample.dispatch_allocations).sum();
        let audio_allocations: u64 = of_kind.iter().map(|sample| sample.audio_allocations).sum();
        let sounds: usize = of_kind.iter().map(|sample| sample.sounds).sum();
        let sprites = of_kind.iter().map(|sample| sample.sprites).max().unwrap_or(0);
        println!(
            "  {}: {} frames, at most {sprites} sprites, {sounds} cues; allocations dispatch {dispatch_allocations}, audio {audio_allocations}",
            kind.name(),
            of_kind.len()
        );
        for (name, mut times) in [
            ("dispatch", of_kind.iter().map(|sample| sample.dispatch).collect::<Vec<_>>()),
            ("audio", of_kind.iter().map(|sample| sample.audio).collect::<Vec<_>>()),
        ] {
            let misses = times.iter().filter(|time| time.as_secs_f64() > f64::from(FRAME)).count();
            let p50 = percentile(&mut times, 0.5);
            let p95 = percentile(&mut times, 0.95);
            let p99 = percentile(&mut times, 0.99);
            let max = *times.last().expect("nonempty category");
            println!(
                "    {name}: p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2} us; {misses} frame-budget misses",
                micros(p50),
                micros(p95),
                micros(p99),
                micros(max)
            );
        }
    }
}

fn main() {
    use std::io::Write as _;

    let mut args = std::env::args().skip(1);
    let mut raw = args.next().map(|flag| {
        assert_eq!(flag, "--raw", "usage: cue_bench [--raw PATH]");
        let path = args.next().expect("--raw requires a CSV path");
        assert!(args.next().is_none(), "usage: cue_bench [--raw PATH]");
        let mut file = std::io::BufWriter::new(fs::File::create(path).expect("creates raw CSV"));
        writeln!(file, "round,tick,kind,dispatch_ns,audio_ns,dispatch_allocations,audio_allocations,cues,sprites,checksum,audio_fingerprint").expect("writes CSV");
        file
    });
    let mut samples = Vec::with_capacity(4096);
    // Warm process/code caches only. Every measured round constructs its own mutable state.
    fight(&mut fresh_sinks(), &mut samples);
    let expected: Vec<_> = samples
        .iter()
        .map(|sample| (sample.tick, sample.checksum, sample.sounds, sample.sprites, sample.audio_fingerprint))
        .collect();
    let mut overhead = Vec::with_capacity(2048);
    for _ in 0..2048 {
        let timer = Stopwatch::start();
        std::hint::black_box(count_in_scope(|| std::hint::black_box(())));
        overhead.push(timer.elapsed());
    }
    let p50 = percentile(&mut overhead, 0.5);
    let p99 = percentile(&mut overhead, 0.99);
    println!(
        "empty clock/allocation scope: p50 {:.3}, p99 {:.3} us (not subtracted); allocator instrumentation overhead remains included",
        micros(p50),
        micros(p99)
    );
    for round in 1..=ROUNDS {
        fight(&mut fresh_sinks(), &mut samples);
        assert_eq!(samples.len(), expected.len(), "same replay length every round");
        for (sample, expected) in samples.iter().zip(&expected) {
            assert_eq!(
                (sample.tick, sample.checksum, sample.sounds, sample.sprites, sample.audio_fingerprint),
                *expected,
                "rounds must run identical work"
            );
            if let Some(raw) = &mut raw {
                writeln!(
                    raw,
                    "{round},{},{},{},{},{},{},{},{},{:016x},{:016x}",
                    sample.tick,
                    sample.kind.name(),
                    sample.dispatch.as_nanos(),
                    sample.audio.as_nanos(),
                    sample.dispatch_allocations,
                    sample.audio_allocations,
                    sample.sounds,
                    sample.sprites,
                    sample.checksum,
                    sample.audio_fingerprint
                )
                .expect("writes CSV");
            }
        }
        report(round, &samples);
    }
    if let Some(raw) = &mut raw {
        raw.flush().expect("flushes CSV");
    }
}
