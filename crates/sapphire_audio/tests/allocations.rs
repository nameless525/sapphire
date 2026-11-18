//! What a cue costs the allocator. Cues come from gameplay every frame, so this is measured rather
//! than assumed, with the counting allocator every app installs (spec R6; `sapphire_core::alloc`).
//! One test, since the counts are of this thread and a harness thread of its own keeps them honest.
//! Music's phase changes (M3 A1) are held to none, once a score plays, in the same test.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Attenuation, AttenuationCurve, Bus, Cue, HitstopResponse, Limit, LimitRule};
use sapphire_assets::mix::{Mix, VoiceGroup};
use sapphire_assets::mouth::MouthCurve;
use sapphire_assets::score::{Phase, Score, Stem};
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioBackend, AudioFrame, AudioSystem, BusId, CueRequest, CueRng, CueTarget, Emitter, EmitterId,
    KiraBackend, Listener, MAX_VOICES, MockBackend, MockCall, MusicRequest, OfflineOutput, PAUSE_SNAPSHOT,
    Room, RoomShape, Rooms, VoiceCaps, VoiceParams, VoiceSlot, VoiceStart,
};
use sapphire_core::Name;
use sapphire_core::alloc::{EngineAllocator, PER_THREAD_COUNTS, count_in_scope, engine_allocator};
use sapphire_math::Vec3;

#[global_allocator]
static GLOBAL: EngineAllocator = engine_allocator();

const RATE: u32 = 48_000;
const GAME_FRAME: usize = RATE as usize / 60;
const SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");
const CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
const ACTOR: EmitterId = EmitterId(1);

/// What a voice may take on the calling thread as it starts (M6 U1): its three command buffers, the
/// state its handle reads and the box kira carries it in. kira's own static sound took 11. A rise means a
/// cost crept in; see the numbers this test prints with `--nocapture`.
const PLAY_ALLOCATIONS: u64 = 5;

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
        ..Cue::default()
    }
}

fn frame<'a>(emitters: &'a [Emitter], cues: &'a [CueRequest]) -> AudioFrame<'a> {
    AudioFrame {
        dt: GAME_FRAME as f32 / RATE as f32,
        listener: Listener::default(),
        world_time_scale: 1.0,
        snapshots: &[],
        emitters,
        cues,
        lines: &[],
        speech: None,
        music: None,
        rooms: Rooms::NONE,
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
    assert!(play.allocations <= PLAY_ALLOCATIONS, "a warmed play allocated {} times", play.allocations);
    assert_eq!(moving.allocations, 0, "moving a voice allocated");
    assert_eq!(mixing.allocations, 0, "the mix allocated, which the audio callback must never do");
    assert_eq!(converting.allocations, 0, "converting the mix allocated, on the audio callback");

    every_voice_filtering_mixes_with_no_allocation();
    music_changes_phase_with_no_allocation();
    the_mix_moves_with_no_allocation();
    voice_limits_place_with_no_allocation();
    rooms_cross_with_no_allocation(2);
    rooms_cross_with_no_allocation(1);
}

/// The room's reverb (M6 U5) on `slots` reverbs, through the real mixer and the shipped mix: 24 voices on an
/// emitter walking with the listener from a street into a shelter and out again, each sending, so the room
/// changes twice a round and every voice's send moves. Once warmed, its frames and its mixing allocate nothing.
fn rooms_cross_with_no_allocation(slots: u8) {
    let mix = Mix::from_document(include_str!("../../../content/audio/game.mix.ron")).unwrap();
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    system.set_mix(&mix).unwrap();
    system.set_reverb_slots(slots);
    system.add_sound(SOUND, &DecodedSound { samples: vec![0.25; RATE as usize * 4], ..sound() });
    system.add_cue(CUE, Cue { looped: true, ..cue() });
    let rooms = [
        Room {
            position: Vec3::ZERO,
            shape: RoomShape::Box { half_extents: Vec3::splat(50.0) },
            priority: 0,
            reverb: Name::new("street"),
            outdoor: true,
            fade: 0.5,
        },
        Room {
            position: Vec3::new(0.0, 0.0, -10.0),
            shape: RoomShape::Sphere { radius: 4.0 },
            priority: 1,
            reverb: Name::new("shelter"),
            outdoor: false,
            fade: 0.3,
        },
    ];
    let mut rng = CueRng::new(1);
    let mut buffer = vec![0.0; GAME_FRAME * 2];
    let mut round = |system: &mut AudioSystem<KiraBackend<OfflineOutput>>, rng: &mut CueRng| {
        for n in 0..90 {
            let at = Vec3::new(0.0, 0.0, if (30..60).contains(&n) { -10.0 } else { 0.0 });
            let walker = [Emitter { id: ACTOR, position: at, hitstop: false }];
            let listener = Listener { position: at, ..Listener::default() };
            let rooms = Rooms { rooms: &rooms, origin: Vec3::ZERO };
            system.update(&AudioFrame { listener, rooms, ..frame(&walker, &[]) }, rng);
            system.backend_mut().output_mut().render_into(&mut buffer);
        }
    };
    let walker = [Emitter { id: ACTOR, position: Vec3::ZERO, hitstop: false }];
    let requests = [CueRequest { cue: CUE, target: CueTarget::On(ACTOR) }];
    for _ in 0..24 {
        system.update(&frame(&walker, &requests), &mut rng);
    }
    round(&mut system, &mut rng);
    let changes = system.stats().room_changes;
    let ((), crossing) = count_in_scope(|| round(&mut system, &mut rng));
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!(
            "90 frames into a shelter and out on {slots} reverb(s), 24 voices: {} allocations",
            crossing.allocations
        );
    }
    assert_eq!(system.stats().voices, 24, "they play");
    assert_eq!(system.stats().room_changes - changes, 2, "in and out");
    assert!(buffer.iter().any(|sample| *sample != 0.0), "it's heard");
    assert_eq!(crossing.allocations, 0, "a room's change, a send or the reverbs allocated");
}

/// A frame of the shipped mix: the world's clock at `scale`, paused or not.
fn mix_frame(
    system: &mut AudioSystem<KiraBackend<OfflineOutput>>,
    scale: f32,
    paused: bool,
    rng: &mut CueRng,
    buffer: &mut [f32],
) {
    let snapshots: &[Name] = if paused { &[PAUSE_SNAPSHOT] } else { &[] };
    system.update(&AudioFrame { world_time_scale: scale, snapshots, ..frame(&[], &[]) }, rng);
    system.backend_mut().output_mut().render_into(buffer);
}

/// The mix (M6 U2) with everything it does moving at once, through the real mixer: the shipped mix's slow
/// motion in and out, its pause on and off, and a voice line ducking the music and the effects by its mouth
/// curve. Once warmed, its frames and its mixing allocate nothing; starting the line is a cue's own cost.
fn the_mix_moves_with_no_allocation() {
    let mix = Mix::from_document(include_str!("../../../content/audio/game.mix.ron")).unwrap();
    let line: AssetId = "5c0e0000-0000-4000-8000-0000000000b0".parse().unwrap();
    let samples =
        (0..RATE as usize * 4).map(|n| 0.25 * (n as f32 * 0.03).sin() * (n as f32 * 1e-4).sin()).collect();
    let voiced = DecodedSound { sample_rate: RATE, channels: 1, samples };
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    system.set_mix(&mix).unwrap();
    system.add_sound(line, &voiced);
    system.add_mouth(line, MouthCurve::analyze(&voiced));
    system.add_cue(line, Cue { sounds: vec![line], bus: Bus::Voice, attenuation: None, ..cue() });
    let requests = [CueRequest { cue: line, target: CueTarget::Flat }];
    let mut rng = CueRng::new(1);
    let mut buffer = vec![0.0; GAME_FRAME * 2];
    let mut round = |system: &mut AudioSystem<KiraBackend<OfflineOutput>>, rng: &mut CueRng| {
        for n in 0..90 {
            let scale = if (20..50).contains(&n) { 0.2 } else { 1.0 };
            mix_frame(system, scale, (55..75).contains(&n), rng, &mut buffer);
        }
    };
    system.update(&frame(&[], &requests), &mut rng);
    round(&mut system, &mut rng);
    system.update(&frame(&[], &requests), &mut rng);
    let ((), moving) = count_in_scope(|| round(&mut system, &mut rng));
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!("90 frames of the mix moving: {} allocations", moving.allocations);
    }
    assert!(system.stats().voices > 0, "the line still speaks");
    assert_eq!(moving.allocations, 0, "a snapshot, a pause or a duck allocated");
}

/// The engine voice (M6 U1) with all it can do at once: every slot playing, each through its own closed
/// low-pass, a pitch and a pan, a stereo sound and a mono one, and parameters moving between blocks.
fn every_voice_filtering_mixes_with_no_allocation() {
    let mut backend = KiraBackend::<OfflineOutput>::new(RATE).unwrap();
    let stereo: AssetId = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5d".parse().unwrap();
    backend.add_sound(SOUND, &DecodedSound { samples: vec![0.25; RATE as usize], ..sound() });
    backend.add_sound(
        stereo,
        &DecodedSound { sample_rate: RATE, channels: 2, samples: vec![0.25; 2 * RATE as usize] },
    );
    let params = |n: u32| VoiceParams {
        gain_db: -6.0,
        pan: 0.5,
        pitch: 0.9 + 0.01 * n as f32,
        low_pass_hz: 800.0,
        send_db: -12.0,
    };
    for n in 0..MAX_VOICES as u32 {
        let sound = if n % 2 == 0 { SOUND } else { stereo };
        backend
            .start_voice(VoiceSlot(n), sound, BusId::category(Bus::Sfx), VoiceStart::once(params(n), false))
            .unwrap();
    }
    let mut buffer = vec![0.0; GAME_FRAME * 2];
    backend.output_mut().render_into(&mut buffer);
    let ((), mixing) = count_in_scope(|| {
        for round in 0..8 {
            for n in 0..MAX_VOICES as u32 {
                backend.set_voice(
                    VoiceSlot(n),
                    VoiceParams { low_pass_hz: 400.0 + 100.0 * round as f32, ..params(n) },
                );
            }
            backend.output_mut().render_into(&mut buffer);
        }
    });
    assert!(buffer.iter().any(|sample| *sample != 0.0), "they play");
    assert_eq!(mixing.allocations, 0, "every voice filtering, moved and mixed, allocated");
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

/// Voice limits (M6 U3) under a storm: 200 requests a frame against a group's limit, the tier's caps and the
/// room for live voices, loops going virtual and coming back as slots free. Through the mock the system's own
/// placing allocates nothing; through the real mixer, the only allocations are each start's own.
fn voice_limits_place_with_no_allocation() {
    let step: AssetId = "5c0e0000-0000-4000-8000-0000000000c0".parse().unwrap();
    let roar: AssetId = "5c0e0000-0000-4000-8000-0000000000c1".parse().unwrap();
    let hum: AssetId = "5c0e0000-0000-4000-8000-0000000000c2".parse().unwrap();
    let long = DecodedSound { sample_rate: RATE, channels: 1, samples: vec![0.1; 3 * RATE as usize] };
    let mut mix = Mix::plain();
    let steps = Limit { max: 6, rule: LimitRule::StopOldest };
    mix.groups.push(VoiceGroup { name: "steps".to_owned(), limit: steps });
    let cues = [
        (step, Cue { priority: 40, group: Some("steps".to_owned()), ..cue() }),
        (roar, Cue { sounds: vec![roar], priority: 200, ..cue() }),
        (hum, Cue { sounds: vec![hum], priority: 10, looped: true, ..cue() }),
    ];
    let emitters: Vec<Emitter> = (0..24)
        .map(|n| Emitter { id: EmitterId(100 + n), position: Vec3::new(n as f32, 0.0, 0.0), hitstop: false })
        .collect();
    let hums: Vec<CueRequest> =
        emitters.iter().map(|emitter| CueRequest { cue: hum, target: CueTarget::On(emitter.id) }).collect();
    let mut storm: Vec<CueRequest> = (0..199)
        .map(|n| CueRequest { cue: step, target: CueTarget::At(Vec3::new(0.0, 0.0, -((n % 20) as f32))) })
        .collect();
    storm.push(CueRequest { cue: roar, target: CueTarget::Flat });
    let mut rng = CueRng::new(1);

    let mut system = AudioSystem::new(MockBackend::new());
    ready(&mut system, &mix, &cues, &long);
    system.backend_mut().reserve_calls(1 << 16);
    system.update(&frame(&emitters, &hums), &mut rng);
    system.update(&frame(&emitters, &storm), &mut rng);
    let mut placing = 0;
    for round in 0..20 {
        let ((), counts) = count_in_scope(|| {
            let cues: &[CueRequest] = if round % 2 == 0 { &storm } else { &[] };
            system.update(&frame(&emitters, cues), &mut rng);
        });
        placing += counts.allocations;
        // Every third frame, all that sounds plays out, so what waits comes back. Outside the count: the mock
        // writes it down in a set.
        if round % 3 == 2 {
            for slot in 0..2 * MAX_VOICES as u32 {
                system.backend_mut().finish(VoiceSlot(slot));
            }
        }
    }
    let stats = system.stats();
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!(
            "20 frames of a 200-request storm: {placing} allocations; {} stolen, {} virtualized, {} revived",
            stats.voices_stolen, stats.voices_virtualized, stats.voices_revived
        );
    }
    assert!(stats.voices_stolen > 0 && stats.voices_virtualized > 0 && stats.voices_revived > 0, "{stats:?}");
    assert_eq!(placing, 0, "placing voices against their limits allocated");

    // Through kira: warmed, then the same storm and churn, every allocation a start's.
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    ready(&mut system, &mix, &cues, &long);
    let mut buffer = vec![0.0; GAME_FRAME * 2];
    let mut churn = |system: &mut AudioSystem<KiraBackend<OfflineOutput>>, rng: &mut CueRng| {
        system.update(&frame(&emitters, &hums), rng);
        for round in 0..20 {
            let cues: &[CueRequest] = if round % 5 == 0 { &storm } else { &[] };
            system.update(&frame(&emitters, cues), rng);
            system.backend_mut().output_mut().render_into(&mut buffer);
        }
    };
    churn(&mut system, &mut rng);
    let before = system.stats();
    let ((), churning) = count_in_scope(|| churn(&mut system, &mut rng));
    let after = system.stats();
    let starts = (after.cues_played - before.cues_played) + (after.voices_revived - before.voices_revived);
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        println!("the storm through kira: {} allocations for {starts} starts", churning.allocations);
    }
    assert!(starts > 0);
    assert!(
        churning.allocations <= starts * PLAY_ALLOCATIONS,
        "{} allocations for {starts} starts",
        churning.allocations
    );
}

/// Loads [`voice_limits_place_with_no_allocation`]'s mix, caps, sounds and cues.
fn ready<B: AudioBackend>(
    system: &mut AudioSystem<B>,
    mix: &Mix,
    cues: &[(AssetId, Cue)],
    long: &DecodedSound,
) {
    system.set_mix(mix).unwrap();
    system.set_voice_caps(VoiceCaps { real: 8, virtual_voices: 32 });
    system.add_sound(SOUND, &sound());
    for (id, cue) in cues {
        if cue.sounds[0] != SOUND {
            system.add_sound(cue.sounds[0], long);
        }
        system.add_cue(*id, cue.clone());
    }
}
