//! Voice limits through the real mixer (M6 U3): what the ranking keeps is what's heard, and a virtual voice
//! comes back where it would have been, without a click. The rules themselves are tested against the mock, in
//! `system.rs`.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue, CueNode};
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRequest, CueRng, CueTarget, Emitter, EmitterId, KiraBackend, Listener,
    MAX_VOICES, OfflineOutput, VoiceCaps,
};
use sapphire_math::Vec3;

const RATE: u32 = 48_000;
/// A frame of the game at 60 fps, in audio frames.
const GAME_FRAME: usize = RATE as usize / 60;
/// The mixer's own buffer, in frames: what a voice's start waits for at most.
const BUFFER: usize = 128;
/// [`sapphire_audio::VIRTUAL_FADE`] in frames.
const FADE: usize = RATE as usize / 50;

const STEP: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000d0");
const ROAR: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000d1");
const HUM: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000d2");
const BOSS: EmitterId = EmitterId(1);

const STEP_HZ: f32 = 2_000.0;
const ROAR_HZ: f32 = 220.0;

/// A sine at `hz`, a quarter of full scale, for this long.
fn tone(hz: f32, seconds: f32) -> DecodedSound {
    let samples = (0..(seconds * RATE as f32) as usize)
        .map(|frame| 0.25 * (std::f32::consts::TAU * hz * frame as f32 / RATE as f32).sin())
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

/// A rising sweep, 200 Hz to 600 Hz, for this long: no stretch of it is like another, so where it is shows.
fn sweep(seconds: f32) -> DecodedSound {
    let length = seconds * RATE as f32;
    let samples = (0..length as usize)
        .map(|frame| {
            let time = frame as f32 / RATE as f32;
            let phase = 200.0 * time + 400.0 * time * time / (2.0 * seconds);
            0.25 * (std::f32::consts::TAU * phase).sin()
        })
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

fn left(stereo: &[f32]) -> Vec<f32> {
    stereo.as_chunks::<2>().0.iter().map(|frame| frame[0]).collect()
}

/// How much of `hz` is in `samples`: Goertzel's filter, as an amplitude.
fn energy_at(samples: &[f32], hz: f32) -> f32 {
    let coefficient = 2.0 * (std::f32::consts::TAU * hz / RATE as f32).cos();
    let (mut previous, mut before) = (0.0f32, 0.0f32);
    for &sample in samples {
        let next = sample + coefficient * previous - before;
        before = previous;
        previous = next;
    }
    let power = previous * previous + before * before - coefficient * previous * before;
    2.0 * power.max(0.0).sqrt() / samples.len() as f32
}

fn largest_step(samples: &[f32]) -> f32 {
    samples.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max)
}

fn mixer() -> AudioSystem<KiraBackend<OfflineOutput>> {
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).expect("an offline mixer"));
    system.add_sound(STEP, &tone(STEP_HZ, 0.3));
    system.add_sound(ROAR, &tone(ROAR_HZ, 2.5));
    system.add_sound(HUM, &sweep(0.5));
    let flat = Cue { play: CueNode::one_of(vec![STEP]), bus: Bus::Sfx, ..Cue::default() };
    system.add_cue(STEP, Cue { volume_db: [-30.0, -30.0], priority: 40, ..flat.clone() });
    system.add_cue(ROAR, Cue { play: CueNode::one_of(vec![ROAR]), priority: 200, ..flat.clone() });
    system.add_cue(HUM, Cue { play: CueNode::one_of(vec![HUM]), priority: 10, looped: true, ..flat });
    system
}

/// One game frame: the update, then its audio. The left channel.
fn step(
    system: &mut AudioSystem<KiraBackend<OfflineOutput>>,
    emitters: &[Emitter],
    cues: &[CueRequest],
    rng: &mut CueRng,
) -> Vec<f32> {
    let frame = AudioFrame {
        dt: GAME_FRAME as f32 / RATE as f32,
        listener: Listener::default(),
        world_time_scale: 1.0,
        snapshots: &[],
        emitters,
        cues,
        lines: &[],
        speech: None,
        music: None,
        rooms: sapphire_audio::Rooms::NONE,
    };
    system.update(&frame, rng);
    left(&system.backend_mut().output_mut().render(GAME_FRAME))
}

fn footsteps(count: usize) -> Vec<CueRequest> {
    vec![CueRequest::new(STEP, CueTarget::Flat); count]
}

/// The boss's roar asked for in a frame of 199 footsteps, after them, and in a frame after 64 footsteps hold
/// every voice: either way it's mixed, and footsteps make the room.
#[test]
fn a_storm_of_footsteps_keeps_the_bosss_roar() {
    let roar = CueRequest::new(ROAR, CueTarget::Flat);

    // 200 requests in one frame.
    let mut system = mixer();
    let mut rng = CueRng::new(1);
    let mut storm = footsteps(199);
    storm.push(roar);
    let mut heard = step(&mut system, &[], &storm, &mut rng);
    for _ in 0..5 {
        heard.extend(step(&mut system, &[], &[], &mut rng));
    }
    let stats = system.stats();
    assert_eq!((stats.voices as usize, stats.cues_played, stats.dropped_voice_limit), (MAX_VOICES, 64, 136));
    assert!(energy_at(&heard, ROAR_HZ) > 0.1, "the roar: {}", energy_at(&heard, ROAR_HZ));
    assert!(energy_at(&heard, STEP_HZ) > 0.1, "the footsteps: {}", energy_at(&heard, STEP_HZ));

    // A frame after the footsteps hold every voice: the newest of them, as the ranking breaks a tie, gives way.
    let mut system = mixer();
    step(&mut system, &[], &footsteps(MAX_VOICES), &mut rng);
    let mut heard = step(&mut system, &[], &[roar], &mut rng);
    for _ in 0..5 {
        heard.extend(step(&mut system, &[], &[], &mut rng));
    }
    let stats = system.stats();
    assert_eq!((stats.voices_stolen, stats.cues_played, stats.dropped_voice_limit), (1, 65, 0));
    assert!(energy_at(&heard, ROAR_HZ) > 0.1, "the roar: {}", energy_at(&heard, ROAR_HZ));
}

/// A looping sweep, outranked by a roar with one real voice to share, goes virtual and comes back when the roar
/// ends: within a buffer of the frame there's room, where it would have been had it played on, and with no
/// click going or coming.
#[test]
fn a_virtual_loop_comes_back_in_place_without_a_click() {
    let boss = [Emitter { id: BOSS, position: Vec3::ZERO, hitstop: false }];
    let hum = [CueRequest::new(HUM, CueTarget::On(BOSS))];
    let run = |roar_at: Option<usize>| {
        let mut system = mixer();
        system.set_voice_caps(VoiceCaps { real: 1, virtual_voices: 4 });
        // Short, and at a whisper: a voice to make it go, not one to hear.
        system.add_sound(ROAR, &tone(ROAR_HZ, 0.6));
        system.add_cue(
            ROAR,
            Cue {
                play: CueNode::one_of(vec![ROAR]),
                volume_db: [-50.0, -50.0],
                priority: 200,
                ..Cue::default()
            },
        );
        let mut rng = CueRng::new(1);
        let (mut out, mut went, mut back) = (Vec::new(), None, None);
        for frame in 0..90 {
            let roar = [CueRequest::new(ROAR, CueTarget::Flat)];
            let cues: &[CueRequest] = if frame == 0 {
                &hum
            } else if Some(frame) == roar_at {
                &roar
            } else {
                &[]
            };
            out.extend(step(&mut system, &boss, cues, &mut rng));
            let stats = system.stats();
            if went.is_none() && stats.voices_virtualized > 0 {
                went = Some(frame);
            }
            if back.is_none() && stats.voices_revived > 0 {
                back = Some(frame);
            }
        }
        (out, went, back)
    };
    let (reference, ..) = run(None);
    let (out, went, back) = run(Some(10));
    let (went, back) = (went.expect("it went virtual"), back.expect("it came back"));
    assert_eq!(went, 10);
    // The roar's 0.6 s, then the frame its slot is seen free.
    assert!((46..=48).contains(&back), "back at frame {back}");

    // Silent while it waits, once the fade out is done.
    let waiting = &out[(went + 1) * GAME_FRAME + BUFFER + FADE..back * GAME_FRAME];
    assert!(waiting.iter().all(|sample| sample.abs() < 1e-3), "it played on while virtual");

    // No click either way: no step bigger than the sweep's own.
    let sweep_step = largest_step(&reference);
    for (seam, at) in [("going", went), ("coming back", back)] {
        let around = &out[at * GAME_FRAME..(at + 1) * GAME_FRAME + BUFFER + FADE];
        assert!(
            largest_step(around) <= sweep_step * 1.05,
            "a click {seam}: {} against {sweep_step}",
            largest_step(around)
        );
    }

    // Within a buffer, and in its place: past the fade it is the sweep that never stopped, shifted by no more
    // than the mixer's buffer.
    let from = back * GAME_FRAME + BUFFER + FADE;
    let window = &out[from..from + 4_800];
    let (lag, error) = (-256_isize..=256)
        .map(|lag| {
            let start = (from as isize + lag) as usize;
            let error = window
                .iter()
                .zip(&reference[start..start + window.len()])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            (lag, error)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    #[allow(clippy::print_stdout, reason = "the measurement the plan's note reports")]
    {
        let steps =
            [went, back].map(|at| largest_step(&out[at * GAME_FRAME..(at + 1) * GAME_FRAME + BUFFER + FADE]));
        println!(
            "virtual at frame {went}, back at {back}, {lag} frames from where it would be, {error} off it; \
             largest steps {steps:?} against the sweep's {sweep_step}"
        );
    }
    assert!(lag.unsigned_abs() <= BUFFER, "{lag} frames out");
    assert!(error < 2e-3, "{error} off the sweep that played on, {lag} frames out");
    let before = &out[back * GAME_FRAME - GAME_FRAME..back * GAME_FRAME];
    let after = &out[back * GAME_FRAME + BUFFER + FADE..(back + 1) * GAME_FRAME + BUFFER + FADE];
    assert!(before.iter().all(|sample| sample.abs() < 1e-3) && after.iter().any(|sample| sample.abs() > 0.1));
}
