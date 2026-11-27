//! The shipped mix (M6 U2) through the real mixer, rendered with no device: what its snapshots and ducking
//! rules do to the samples.

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue, CueNode, HitstopResponse};
use sapphire_assets::mix::{Mix, OutputProfile};
use sapphire_assets::mouth::MouthCurve;
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRequest, CueRng, CueTarget, KiraBackend, Listener, OfflineOutput,
    PAUSE_SNAPSHOT,
};
use sapphire_core::Name;

const RATE: u32 = 48_000;
const GAME_FRAME: usize = RATE as usize / 60;
const FRAME_SECONDS: f32 = 1.0 / 60.0;
const SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");
const CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
const MUSIC_SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5d");
const MUSIC_CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555556");

/// A perfect dodge's slow motion at v0.2.0 (ccd51a670, before the mix), measured by [`slow_motion`] through its
/// `ClockSnapshot`: each window's level in dB, ...
const V0_2_0_LEVELS: [f32; 34] = [
    -15.050, -15.056, -15.048, -14.293, -15.873, -23.495, -26.365, -26.644, -26.672, -26.675, -26.675,
    -26.676, -26.677, -26.677, -26.678, -26.678, -26.678, -26.678, -26.493, -21.916, -18.082, -16.448,
    -15.788, -15.478, -15.336, -15.251, -15.201, -15.175, -15.156, -15.142, -15.132, -15.128, -15.119,
    -15.121,
];
/// ... and its frequency in hertz.
const V0_2_0_HZ: [f32; 34] = [
    2000.00, 2000.00, 2000.00, 1983.74, 1675.52, 1523.46, 1502.53, 1500.54, 1500.55, 1500.55, 1500.55,
    1500.55, 1500.55, 1500.55, 1500.55, 1500.55, 1500.55, 1500.55, 1502.68, 1563.13, 1652.97, 1727.88,
    1788.01, 1835.51, 1872.68, 1901.66, 1924.30, 1941.70, 1955.20, 1965.62, 1973.48, 1979.64, 1984.05,
    1987.23,
];

fn shipped() -> Mix {
    Mix::from_document(include_str!("../../../content/audio/game.mix.ron")).unwrap()
}

/// `seconds` of a sine at `hz`, a quarter of full scale, shaped by `envelope` over the seconds.
fn tone(hz: f32, seconds: f32, envelope: impl Fn(f32) -> f32) -> DecodedSound {
    let frames = (seconds * RATE as f32) as usize;
    let samples = (0..frames)
        .map(|frame| {
            let time = frame as f32 / RATE as f32;
            0.25 * envelope(time) * (std::f32::consts::TAU * hz * time).sin()
        })
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

fn cue_on(bus: Bus, sound: AssetId, volume_db: f32) -> Cue {
    Cue {
        play: CueNode::one_of(vec![sound]),
        volume_db: [volume_db, volume_db],
        pitch: [1.0, 1.0],
        attenuation: None,
        bus,
        hitstop: HitstopResponse::Ignore,
        ..Cue::default()
    }
}

fn frame<'a>(world_time_scale: f32, snapshots: &'a [Name], cues: &'a [CueRequest]) -> AudioFrame<'a> {
    AudioFrame {
        dt: FRAME_SECONDS,
        listener: Listener::default(),
        world_time_scale,
        snapshots,
        emitters: &[],
        cues,
        lines: &[],
        speech: None,
        music: None,
        rooms: sapphire_audio::Rooms::NONE,
    }
}

/// One game frame: the update, then its sixtieth of a second of the mix, the left channel.
fn step(
    system: &mut AudioSystem<KiraBackend<OfflineOutput>>,
    frame: &AudioFrame<'_>,
    rng: &mut CueRng,
) -> Vec<f32> {
    system.update(frame, rng);
    let out = system.backend_mut().output_mut().render(GAME_FRAME);
    out.chunks(2).map(|frame| frame[0]).collect()
}

/// The frequency of a steady tone, from its first and last rising zero crossings, each placed between samples.
fn frequency_of(samples: &[f32]) -> f32 {
    let crossings: Vec<f32> = samples
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] < 0.0 && pair[1] >= 0.0)
        .map(|(at, pair)| at as f32 + pair[0] / (pair[0] - pair[1]))
        .collect();
    let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
    (crossings.len() - 1) as f32 * RATE as f32 / (last - first)
}

fn rms_db(samples: &[f32]) -> f32 {
    let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt();
    20.0 * rms.max(1e-9).log10()
}

/// How much of `hz` is in `samples`, in dB of amplitude: Goertzel's filter.
fn level_at(samples: &[f32], hz: f32) -> f32 {
    let coefficient = 2.0 * (std::f32::consts::TAU * hz / RATE as f32).cos();
    let (mut previous, mut before) = (0.0f32, 0.0f32);
    for &sample in samples {
        let next = sample + coefficient * previous - before;
        before = previous;
        previous = next;
    }
    let power = previous * previous + before * before - coefficient * previous * before;
    20.0 * (2.0 * power.max(0.0).sqrt() / samples.len() as f32).max(1e-9).log10()
}

/// A 2 kHz tone on the effects bus through a perfect dodge, `mix` given or not: 0.25 s at normal speed, 1 s
/// at 0.2×, 1 s back at normal speed. Each window of four game frames: its level and its frequency, as asked, with
/// the master's look-ahead (M6 U10) taken off the front and its `Home` gain off the level, which v0.2.0 hadn't.
fn slow_motion(mix: Option<&Mix>) -> Vec<(f32, f32)> {
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    if let Some(mix) = mix {
        system.set_mix(mix).unwrap();
    }
    system.add_sound(SOUND, &tone(2_000.0, 4.0, |_| 1.0));
    system.add_cue(CUE, cue_on(Bus::Sfx, SOUND, 0.0));
    let mut rng = CueRng::new(1);
    let mut left = Vec::new();
    // One frame more, to make up what the look-ahead holds back.
    for index in 0..136 {
        let scale = if (15..75).contains(&index) { 0.2 } else { 1.0 };
        let play = [CueRequest::new(CUE, CueTarget::Flat)];
        let cues: &[CueRequest] = if index == 0 { &play } else { &[] };
        left.extend(step(&mut system, &frame(scale, &[], cues), &mut rng));
    }
    let late = (system.mix_latency().as_secs_f64() * f64::from(RATE)).round() as usize;
    left.drain(..late);
    left.truncate(135 * GAME_FRAME);
    let master_db = mix.map_or(0.0, |mix| mix.profiles[mix.profile(OutputProfile::Home)].volume_db);
    left.chunks(4 * GAME_FRAME).map(|window| (rms_db(window) - master_db, frequency_of(window))).collect()
}

/// The shipped mix's `slow_motion` is v0.2.0's perfect dodge, sample for sample as far as a listener goes:
/// within 0.1 dB and 1 Hz in every window, into the dip, through it and out.
#[test]
fn slow_motion_is_v0_2_0s() {
    let windows = slow_motion(Some(&shipped()));
    assert_eq!(windows.len(), V0_2_0_LEVELS.len());
    for (index, &(level, hz)) in windows.iter().enumerate() {
        let (was_level, was_hz) = (V0_2_0_LEVELS[index], V0_2_0_HZ[index]);
        assert!((level - was_level).abs() <= 0.1, "window {index}: {level} dB, v0.2.0 {was_level}");
        assert!((hz - was_hz).abs() <= 1.0, "window {index}: {hz} Hz, v0.2.0 {was_hz}");
    }
    // With no mix, nothing follows the clock: the plain mix has no snapshot.
    let plain = slow_motion(None);
    assert!(plain.iter().all(|&(_, hz)| (hz - 2_000.0).abs() < 1.0), "{plain:?}");
}

/// A voice line ducks the music by the shipped rule while its mouth is open, and lets it back up within 150 ms
/// of a pause in it. The line is silent here (its cue is at -60 dB), so the mix is the music alone; its mouth
/// curve is the cook's analysis of its sound.
#[test]
fn a_line_ducks_the_music_and_lets_it_up_in_a_pause() {
    const SPOKEN: f32 = 0.6;
    const GAP: f32 = 0.5;
    const LINE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000b1");
    // 600 Hz fits a game frame ten times over, so each frame's level is steady.
    let music = tone(600.0, 6.0, |_| 1.0);
    let line =
        tone(
            300.0,
            2.0 * SPOKEN + GAP,
            |time| if (SPOKEN..SPOKEN + GAP).contains(&time) { 0.0 } else { 1.0 },
        );
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    system.set_mix(&shipped()).unwrap();
    system.add_sound(MUSIC_SOUND, &music);
    system.add_cue(MUSIC_CUE, cue_on(Bus::Music, MUSIC_SOUND, 0.0));
    system.add_sound(LINE, &line);
    system.add_mouth(LINE, MouthCurve::analyze(&line));
    system.add_cue(LINE, cue_on(Bus::Voice, LINE, -60.0));
    let rule = &shipped().ducking[0];
    let duck_db = rule.duck.iter().find(|target| target.bus == "music").unwrap().gain_db;

    let mut rng = CueRng::new(1);
    let (music_at, line_at) = (0, 30);
    let mut levels = Vec::new();
    for index in 0..150 {
        let cue = if index == music_at { MUSIC_CUE } else { LINE };
        let play = [CueRequest::new(cue, CueTarget::Flat)];
        let cues: &[CueRequest] = if index == music_at || index == line_at { &play } else { &[] };
        levels.push(rms_db(&step(&mut system, &frame(1.0, &[], cues), &mut rng)));
    }
    let base = levels[line_at - 5];
    let gap_at = line_at + (SPOKEN / FRAME_SECONDS).round() as usize;
    let back_at = gap_at + (GAP / FRAME_SECONDS).round() as usize;
    for (index, level) in levels.iter().enumerate().take(gap_at).skip(line_at + 15) {
        let down = level - base;
        assert!(
            (down - duck_db).abs() < 0.5,
            "frame {index}: {down} dB under the line, the rule says {duck_db}"
        );
    }
    let lifted =
        (gap_at..back_at).find(|&index| levels[index] >= base - 1.0).expect("the music comes back up");
    let lift = (lifted - gap_at) as f32 * FRAME_SECONDS;
    assert!(lift <= 0.15, "the music came back {lift} s into the pause: {:?}", &levels[gap_at..back_at]);
    assert!((levels[back_at - 2] - base).abs() < 0.3, "all the way back in the pause");
    assert!((levels[back_at + 20] - base - duck_db).abs() < 0.5, "and down again when it speaks");
}

/// The shipped `pause` holds the effects where they are, tails and all, while the music goes on quieter, and
/// lets them go on from there: a decaying tone paused partway comes back as loud as it went, louder than the
/// same tone left to ring.
#[test]
fn a_pause_holds_the_worlds_tails_and_leaves_the_music() {
    // 1.2 kHz fits a game frame twenty times over, and dies away over a quarter of a second.
    let tail = tone(1_200.0, 3.0, |time| (-time / 0.25).exp());
    let music = tone(600.0, 6.0, |_| 1.0);
    let run = |paused: std::ops::Range<usize>| {
        let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
        system.set_mix(&shipped()).unwrap();
        system.add_sound(SOUND, &tail);
        system.add_cue(CUE, cue_on(Bus::Sfx, SOUND, 0.0));
        system.add_sound(MUSIC_SOUND, &music);
        system.add_cue(MUSIC_CUE, cue_on(Bus::Music, MUSIC_SOUND, 0.0));
        let mut rng = CueRng::new(1);
        let play = [CueRequest::new(MUSIC_CUE, CueTarget::Flat)];
        for index in 0..20 {
            let cues: &[CueRequest] = if index == 0 { &play } else { &[] };
            step(&mut system, &frame(1.0, &[], cues), &mut rng);
        }
        let hit = [CueRequest::new(CUE, CueTarget::Flat)];
        let pause = [PAUSE_SNAPSHOT];
        (0..90)
            .map(|index| {
                let snapshots: &[Name] = if paused.contains(&index) { &pause } else { &[] };
                let cues: &[CueRequest] = if index == 0 { &hit } else { &[] };
                let out = step(&mut system, &frame(1.0, snapshots, cues), &mut rng);
                (level_at(&out, 1_200.0), level_at(&out, 600.0))
            })
            .collect::<Vec<_>>()
    };
    let rung = run(0..0);
    let held = run(6..36);
    let music = rung[3].1;
    // Held: the effects silent, the music down by the snapshot's 6 dB and darkened a little.
    for (index, (effects, _)) in held.iter().enumerate().take(36).skip(12) {
        assert!(*effects < -70.0, "frame {index}: the effects at {effects} dB in the pause");
    }
    for (index, (_, played)) in held.iter().enumerate().take(36).skip(20) {
        let down = played - music;
        assert!((-7.0..-5.0).contains(&down), "frame {index}: the music {down} dB in the pause");
    }
    // Let go: the tail goes on from where it was held, so well above the one that rang on, and the music
    // comes back.
    for index in 50..60 {
        let above = held[index].0 - rung[index].0;
        assert!(above > 5.0, "frame {index}: only {above} dB above the tail that rang on");
    }
    assert!((held[70].1 - music).abs() < 0.5, "the music back: {} against {music}", held[70].1);
}
