//! The master (M6 U10) through the real mixer, with no device: the limiter's ceiling on a mix far over it, the
//! dynamic range on a program with a loud and a quiet half, the output profiles' gains and the latency it reports.
//! `sapphire_cues`' `tests/loudness.rs` meters the shipped mix on committed fights.

use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue, CueNode, HitstopResponse};
use sapphire_assets::mix::{Mix, OutputProfile, Profile};
use sapphire_assets::sound::DecodedSound;
use sapphire_audio::loudness::measure;
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRequest, CueRng, CueTarget, DynamicRange, KiraBackend, LIMITER_CEILING_DB,
    Listener, Master, OfflineOutput, Rooms,
};

const RATE: u32 = 48_000;
const FRAME: usize = RATE as usize / 60;
const LOUD: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a0");
const QUIET: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a1");

/// A 1 kHz tone at `peak_db`, `seconds` long.
fn tone(peak_db: f32, seconds: f32) -> DecodedSound {
    let amplitude = 10_f32.powf(peak_db / 20.0);
    let step = std::f32::consts::TAU * 1_000.0 / RATE as f32;
    let samples =
        (0..(seconds * RATE as f32) as usize).map(|n| amplitude * (step * n as f32).sin()).collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

fn flat(sound: AssetId) -> Cue {
    Cue {
        play: CueNode::one_of(vec![sound]),
        volume_db: [0.0, 0.0],
        pitch: [1.0, 1.0],
        attenuation: None,
        bus: Bus::Sfx,
        hitstop: HitstopResponse::Ignore,
        ..Cue::default()
    }
}

/// A mix with nothing between the voices and the master, whose `Home` and `Portable` play at `home_db` and
/// `portable_db`.
fn mix(home_db: f32, portable_db: f32) -> Mix {
    let profile = |profile, volume_db| Profile { profile, volume_db, effects: Vec::new() };
    Mix {
        profiles: vec![profile(OutputProfile::Home, home_db), profile(OutputProfile::Portable, portable_db)],
        ..Mix::plain()
    }
}

/// `cues` started at the frames given, played for `seconds`, through `range` and `profile`.
fn render(
    mix: &Mix,
    sounds: &[(AssetId, DecodedSound)],
    cues: &[(usize, AssetId)],
    seconds: f32,
    profile: OutputProfile,
    range: DynamicRange,
) -> (Vec<f32>, AudioSystem<KiraBackend<OfflineOutput>>) {
    let Ok(backend) = KiraBackend::<OfflineOutput>::new(RATE);
    let mut system = AudioSystem::new(backend);
    system.set_mix(mix).unwrap();
    system.set_output_profile(profile);
    system.set_dynamic_range(range);
    for (id, sound) in sounds {
        system.add_sound(*id, sound);
        system.add_cue(*id, flat(*id));
    }
    let mut rng = CueRng::new(1);
    let mut samples = Vec::new();
    let mut block = vec![0.0; FRAME * 2];
    for frame in 0..(seconds * 60.0) as usize {
        let requests: Vec<CueRequest> = cues
            .iter()
            .filter(|(at, _)| *at == frame)
            .map(|&(_, cue)| CueRequest::new(cue, CueTarget::Flat))
            .collect();
        let frame = AudioFrame {
            dt: 1.0 / 60.0,
            listener: Listener::default(),
            world_time_scale: 1.0,
            snapshots: &[],
            emitters: &[],
            cues: &requests,
            lines: &[],
            speech: None,
            music: None,
            rooms: Rooms::NONE,
        };
        system.update(&frame, &mut rng);
        system.backend_mut().output_mut().render_into(&mut block);
        samples.extend_from_slice(&block);
    }
    (samples, system)
}

/// Eight tones at full scale at once, 18 dB over it, leave at most −1 dBTP, read by the meter's true peak; and the
/// limiter limits, rather than silences.
#[test]
fn a_mix_far_over_full_scale_leaves_under_the_ceiling() {
    let sounds = [(LOUD, tone(0.0, 3.0))];
    let cues: Vec<_> = (0..8).map(|_| (0, LOUD)).collect();
    let (samples, _) = render(&mix(0.0, 0.0), &sounds, &cues, 2.0, OutputProfile::Home, DynamicRange::Wide);
    let read = measure(&samples, RATE);
    assert!(read.true_peak_db <= f64::from(LIMITER_CEILING_DB), "{read:?}");
    assert!(read.true_peak_db > f64::from(LIMITER_CEILING_DB) - 0.5, "{read:?}");
}

/// EBU Tech 3342's two-level program, 20 s 10 dB under the profile's loudness after 20 s at it: Wide reads its
/// 10 LU, and Night narrows the same render by at least 6. Standard sits between them.
#[test]
fn night_narrows_a_programs_range_by_six_lu() {
    let target = OutputProfile::Home.loudness_lufs();
    let sounds = [(LOUD, tone(target, 20.0)), (QUIET, tone(target - 10.0, 20.0))];
    let cues = [(0, LOUD), (1_200, QUIET)];
    let range = |range| {
        let (samples, _) = render(&mix(0.0, 0.0), &sounds, &cues, 40.0, OutputProfile::Home, range);
        measure(&samples, RATE).range_lu
    };
    let (wide, standard, night) =
        (range(DynamicRange::Wide), range(DynamicRange::Standard), range(DynamicRange::Night));
    assert!((wide - 10.0).abs() < 1.0, "Wide plays the program as it is: {wide}");
    assert!(wide - night >= 6.0, "Night narrows 10 LU to {night}");
    assert!(night < standard && standard < wide, "{wide} {standard} {night}");
}

/// A profile's gain is the master's, applied to everything: the same render through `Portable` is louder by the
/// difference of the two profiles' gains, and the dynamic range follows the profile's loudness, so a program at
/// each profile's loudness stays there under Night.
#[test]
fn a_profile_sets_the_masters_gain_and_the_ranges_loudness() {
    let sounds = [(LOUD, tone(-24.0, 6.0))];
    let cues = [(0, LOUD)];
    let read = |profile, range, mix: &Mix| {
        let (samples, _) = render(mix, &sounds, &cues, 5.0, profile, range);
        measure(&samples[samples.len() / 2..], RATE).integrated_lufs
    };
    let gains = mix(0.0, 6.0);
    let home = read(OutputProfile::Home, DynamicRange::Wide, &gains);
    let portable = read(OutputProfile::Portable, DynamicRange::Wide, &gains);
    assert!((portable - home - 6.0).abs() < 0.05, "{home} and {portable}");
    // −24 at home, and −18 through Portable's 6 dB: each its profile's loudness, which Night leaves where it is.
    assert!((read(OutputProfile::Home, DynamicRange::Night, &gains) + 24.0).abs() < 0.3);
    assert!((read(OutputProfile::Portable, DynamicRange::Night, &gains) + 18.0).abs() < 0.3);
}

/// The mixer reports the master's delay once it has run: the look-ahead and the interpolator's half-length, which
/// the first sound out shows.
#[test]
fn the_mix_reports_how_late_it_leaves() {
    let sounds = [(LOUD, DecodedSound { sample_rate: RATE, channels: 1, samples: vec![0.5; RATE as usize] })];
    let (samples, system) =
        render(&mix(0.0, 0.0), &sounds, &[(0, LOUD)], 0.5, OutputProfile::Home, DynamicRange::Wide);
    let latency = system.mix_latency();
    assert_eq!(latency, Master::latency_at(RATE));
    assert!(latency > Duration::from_millis(2) && latency < Duration::from_millis(3), "{latency:?}");
    let first = samples.iter().position(|sample| *sample != 0.0).unwrap() / 2;
    let late = (latency.as_secs_f64() * f64::from(RATE)).round() as usize;
    assert!(first >= late, "the first sound out is at frame {first}, {late} late at least");
}
