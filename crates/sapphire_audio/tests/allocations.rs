//! What a cue costs the allocator. Cues come from gameplay every frame, so this is measured rather
//! than assumed, with the counting allocator every app installs (spec R6; `sapphire_core::alloc`).
//! One test, since the counts are of this thread and a harness thread of its own keeps them honest.
//! Music's phase changes (M3 A1) are held to none, once a score plays, in the same test.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Attenuation, AttenuationCurve, Bus, Cue, HitstopResponse};
use sapphire_assets::score::{Phase, Score, Stem};
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRequest, CueRng, CueTarget, Emitter, EmitterId, KiraBackend, Listener,
    MockBackend, MockCall, MusicRequest, OfflineOutput,
};
use sapphire_core::alloc::{EngineAllocator, PER_THREAD_COUNTS, count_in_scope, engine_allocator};
use sapphire_math::Vec3;

#[global_allocator]
static GLOBAL: EngineAllocator = engine_allocator();

const RATE: u32 = 48_000;
const GAME_FRAME: usize = RATE as usize / 60;
const SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");
const CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
const ACTOR: EmitterId = EmitterId(1);

/// What kira may take for one sound on the calling thread: its handle's shared state, a command
/// buffer per parameter and the box it travels in. A rise means kira's playing changed, or a cost
/// of ours crept in; see the numbers this test prints with `--nocapture`.
const KIRA_PLAY_ALLOCATIONS: u64 = 32;

fn sound() -> DecodedSound {
    DecodedSound { sample_rate: RATE, channels: 1, samples: vec![0.25; RATE as usize / 30] }
}

fn cue() -> Cue {
    Cue {
        sounds: vec![SOUND],
        volume_db: [-3.0, 0.0],
        pitch: [0.9, 1.1],
        attenuation: Some(Attenuation { near: 1.0, far: 20.0, curve: AttenuationCurve::Linear }),
        bus: Bus::Sfx,
        hitstop: HitstopResponse::Ignore,
    }
}

fn frame<'a>(emitters: &'a [Emitter], cues: &'a [CueRequest]) -> AudioFrame<'a> {
    AudioFrame {
        dt: GAME_FRAME as f32 / RATE as f32,
        listener: Listener::default(),
        world_time_scale: 1.0,
        emitters,
        cues,
        lines: &[],
        music: None,
    }
}

#[test]
fn what_a_cue_allocates_on_the_calling_thread() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let emitters = [Emitter { id: ACTOR, position: Vec3::new(1.0, 0.0, 0.0), hitstop: false }];
    let requests = [CueRequest { cue: CUE, target: CueTarget::On(ACTOR) }];
    let mut rng = CueRng::new(1);

    // The system's own work, with a backend that does none: nothing at all, voices or no voices.
    let mut system = AudioSystem::new(MockBackend::new());
    system.add_sound(SOUND, &sound());
    system.add_cue(CUE, cue());
    system.backend_mut().reserve_calls(4096);
    system.update(&frame(&emitters, &requests), &mut rng);
    let ((), system_alone) = count_in_scope(|| {
        for _ in 0..16 {
            system.update(&frame(&emitters, &requests), &mut rng);
            system.update(&frame(&emitters, &[]), &mut rng);
        }
    });
    assert_eq!(system_alone.allocations, 0, "AudioSystem::update allocated on its own account");

    // A positive oracle for the same movement measured on kira below: a non-spatial cue would
    // make movement a no-op and accidentally omit the parameter-command path from the gate.
    let moved = [Emitter { position: Vec3::new(-1.0, 0.0, 0.0), ..emitters[0] }];
    system.backend_mut().take_calls();
    system.update(&frame(&moved, &[]), &mut rng);
    let calls = system.backend_mut().take_calls();
    assert_eq!(calls.len(), system.stats().voices as usize);
    assert!(calls.iter().all(|call| matches!(call, MockCall::SetVoice { params, .. } if params.pan == -1.0)));

    // On kira: warmed by a few plays, so its free lists and ours are at their working size.
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    system.add_sound(SOUND, &sound());
    system.add_cue(CUE, cue());
    let mut buffer = vec![0.0; GAME_FRAME * 2];
    let mut mix = |system: &mut AudioSystem<KiraBackend<OfflineOutput>>| {
        system.backend_mut().output_mut().render_into(&mut buffer);
    };
    for _ in 0..8 {
        system.update(&frame(&emitters, &requests), &mut rng);
        for _ in 0..4 {
            mix(&mut system);
            system.update(&frame(&emitters, &[]), &mut rng);
        }
    }

    // A warmed play.
    let ((), play) = count_in_scope(|| system.update(&frame(&emitters, &requests), &mut rng));
    // A frame with a voice live and moving, and no cue.
    let ((), moving) = count_in_scope(|| system.update(&frame(&moved, &[]), &mut rng));
    // What the audio callback does, with that voice playing.
    let ((), mixing) = count_in_scope(|| mix(&mut system));
    // And for a device that takes integers.
    let mut integers = vec![0_i16; GAME_FRAME * 2];
    let ((), converting) = count_in_scope(|| system.backend_mut().output_mut().render_samples(&mut integers));
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!("warmed play: {} allocations, {} bytes", play.allocations, play.bytes);
        println!("a frame with a voice moving: {} allocations", moving.allocations);
        println!("a block of the mix: {} allocations", mixing.allocations);
    }
    assert!(play.allocations <= KIRA_PLAY_ALLOCATIONS, "a warmed play allocated {} times", play.allocations);
    assert_eq!(moving.allocations, 0, "moving a voice allocated");
    assert_eq!(mixing.allocations, 0, "the mix allocated, which the audio callback must never do");
    assert_eq!(converting.allocations, 0, "converting the mix allocated, on the audio callback");

    music_changes_phase_with_no_allocation();
}

const SCORE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-000000000001");

/// Four stems of a bar at 120 beats a minute, each in its own phase, so every change fades two.
fn music_changes_phase_with_no_allocation() {
    let stems: Vec<AssetId> =
        (0..4).map(|n| format!("5c0e0000-0000-4000-8000-0000000000a{n}").parse().unwrap()).collect();
    let score = Score {
        tempo_bpm: 120.0,
        beats_per_bar: 4,
        bars: 1,
        stems: stems.iter().enumerate().map(|(n, &sound)| Stem { name: format!("s{n}"), sound }).collect(),
        phases: (0..4).map(|n| Phase { name: format!("p{n}"), play: vec![format!("s{n}")] }).collect(),
        fade_beats: 1.0,
        volume_db: 0.0,
        duck_db: -10.0,
    };
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    let stem = DecodedSound { sample_rate: RATE, channels: 1, samples: vec![0.1; 2 * RATE as usize] };
    for &id in &stems {
        system.add_sound(id, &stem);
    }
    system.add_score(SCORE, score);
    let mut rng = CueRng::new(1);
    let mut buffer = vec![0.0; GAME_FRAME * 2];
    let phases = ["p0", "p1", "p2", "p3"];
    let mut frame_at = |system: &mut AudioSystem<KiraBackend<OfflineOutput>>, phase: &str| {
        let music = Some(MusicRequest { score: SCORE, phase, instead: None });
        system.update(&AudioFrame { music, ..frame(&[], &[]) }, &mut rng);
        system.backend_mut().output_mut().render_into(&mut buffer);
    };
    // Started, and round every phase twice: warm.
    for phase in phases.iter().cycle().take(8) {
        for _ in 0..30 {
            frame_at(&mut system, phase);
        }
    }
    let ((), changing) = count_in_scope(|| {
        for phase in phases.iter().cycle().take(8) {
            for _ in 0..30 {
                frame_at(&mut system, phase);
            }
        }
    });
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!("eight phase changes and 240 frames of music: {} allocations", changing.allocations);
    }
    assert_eq!(system.stats().music_phase_changes, 15);
    assert_eq!(changing.allocations, 0, "a phase change or the music's mix allocated");
}
