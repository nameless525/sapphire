//! The real mixer, rendered with no device: what the backend is asked for really happens to the
//! samples. `AudioSystem`'s own decisions are tested against the mock, next to it.

use std::path::Path;

use cpal::Sample;
use sapphire_assets::AssetId;
use sapphire_assets::cue::{Attenuation, AttenuationCurve, Bus, Cue, HitstopResponse};
use sapphire_assets::sound::{DecodedSound, Sound};
use sapphire_audio::{
    AudioBackend, AudioFrame, AudioSystem, BusParams, CueRequest, CueRng, CueTarget, DeviceState,
    KiraBackend, Listener, OfflineOutput, SnapshotTuning, VoiceParams, VoiceSlot,
};
use sapphire_math::Vec3;

const RATE: u32 = 48_000;
/// A frame of the game at 60 fps, in audio frames.
const GAME_FRAME: usize = RATE as usize / 60;
const SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");
const CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
const NEUTRAL: VoiceParams = VoiceParams { gain_db: 0.0, pan: 0.0, pitch: 1.0 };

/// Sine tones at these frequencies, summed at a quarter of full scale each, for this long.
fn tones(hz: &[f32], seconds: f32) -> DecodedSound {
    let frames = (seconds * RATE as f32) as usize;
    let samples = (0..frames)
        .map(|frame| {
            let time = frame as f32 / RATE as f32;
            hz.iter().map(|hz| 0.25 * (std::f32::consts::TAU * hz * time).sin()).sum()
        })
        .collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

/// The left channel of interleaved stereo.
fn left(stereo: &[f32]) -> Vec<f32> {
    stereo.as_chunks::<2>().0.iter().map(|frame| frame[0]).collect()
}

fn right(stereo: &[f32]) -> Vec<f32> {
    stereo.as_chunks::<2>().0.iter().map(|frame| frame[1]).collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

fn db(ratio: f32) -> f32 {
    20.0 * ratio.max(1e-9).log10()
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

/// The frequency of a steady tone, by its rising zero crossings.
fn frequency_of(samples: &[f32]) -> f32 {
    let rising = samples.windows(2).filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0).count();
    rising as f32 * RATE as f32 / samples.len() as f32
}

fn mixer(sound: &DecodedSound) -> KiraBackend<OfflineOutput> {
    let mut backend = KiraBackend::<OfflineOutput>::new(RATE).expect("an offline mixer");
    backend.add_sound(SOUND, sound);
    backend
}

fn start(backend: &mut KiraBackend<OfflineOutput>, params: VoiceParams) {
    backend.start_voice(VoiceSlot(0), SOUND, Bus::Sfx, params, false).expect("a voice");
}

/// `seconds` of output, as interleaved stereo.
fn render(backend: &mut KiraBackend<OfflineOutput>, seconds: f32) -> Vec<f32> {
    backend.output_mut().render((seconds * RATE as f32) as usize)
}

#[test]
fn a_low_pass_really_takes_out_the_high_frequencies() {
    const LOW: f32 = 300.0;
    const HIGH: f32 = 8_000.0;
    let sound = tones(&[LOW, HIGH], 1.0);
    let heard = |low_pass: BusParams| {
        let mut backend = mixer(&sound);
        backend.set_bus(Bus::Sfx, low_pass);
        start(&mut backend, NEUTRAL);
        // Past the filter's move.
        let _settle = render(&mut backend, 0.1);
        let out = left(&render(&mut backend, 0.5));
        (energy_at(&out, LOW), energy_at(&out, HIGH))
    };
    let (open_low, open_high) = heard(BusParams::OPEN);
    assert!((db(open_low / 0.25)).abs() < 1.0, "the open mix is the sound as authored: {open_low}");
    assert!((db(open_high / 0.25)).abs() < 1.0, "the open mix is the sound as authored: {open_high}");

    let (low, high) = heard(BusParams { low_pass_hz: 900.0 });
    let high_drop = db(open_high / high);
    let low_drop = db(open_low / low);
    assert!(high_drop > 30.0, "8 kHz only fell {high_drop} dB under a 900 Hz low-pass");
    assert!(low_drop < 3.0, "300 Hz fell {low_drop} dB under a 900 Hz low-pass");
}

#[test]
fn only_the_world_buses_have_a_low_pass() {
    let sound = tones(&[8_000.0], 0.6);
    let mut backend = mixer(&sound);
    backend.set_bus(Bus::Ui, BusParams { low_pass_hz: 300.0 });
    backend.start_voice(VoiceSlot(0), SOUND, Bus::Ui, NEUTRAL, false).unwrap();
    let _settle = render(&mut backend, 0.1);
    let out = left(&render(&mut backend, 0.3));
    assert!(db(energy_at(&out, 8_000.0) / 0.25).abs() < 1.0);
}

#[test]
fn a_pitch_change_really_moves_a_tone() {
    let sound = tones(&[1_000.0], 2.0);
    for pitch in [1.0, 1.5, 0.75, 0.5] {
        let mut backend = mixer(&sound);
        start(&mut backend, VoiceParams { pitch, ..NEUTRAL });
        let _settle = render(&mut backend, 0.05);
        let out = left(&render(&mut backend, 0.5));
        let expected = 1_000.0 * pitch;
        let measured = frequency_of(&out);
        assert!((measured - expected).abs() < 5.0, "pitch {pitch}: {measured} Hz");
        assert!(energy_at(&out, expected) > 0.2, "pitch {pitch}: the tone is at {expected} Hz");
        if pitch != 1.0 {
            assert!(energy_at(&out, 1_000.0) < 0.01, "pitch {pitch}: and no longer at 1 kHz");
        }
    }
}

#[test]
fn a_change_of_pitch_on_a_live_voice_arrives() {
    let sound = tones(&[1_000.0], 2.0);
    let mut backend = mixer(&sound);
    start(&mut backend, NEUTRAL);
    let _before = render(&mut backend, 0.1);
    backend.set_voice(VoiceSlot(0), VoiceParams { pitch: 0.75, ..NEUTRAL });
    let _tween = render(&mut backend, 0.1);
    let out = left(&render(&mut backend, 0.3));
    assert!((frequency_of(&out) - 750.0).abs() < 5.0);
}

#[test]
fn gain_and_pan_really_reach_the_channels() {
    let sound = tones(&[1_000.0], 1.0);
    let heard = |params: VoiceParams| {
        let mut backend = mixer(&sound);
        start(&mut backend, params);
        let out = render(&mut backend, 0.3);
        (rms(&left(&out)), rms(&right(&out)))
    };
    let (centre_left, centre_right) = heard(NEUTRAL);
    assert!((centre_left - centre_right).abs() < 1e-4);
    // -30 dB is -30 dB.
    let (quiet_left, _) = heard(VoiceParams { gain_db: -30.0, ..NEUTRAL });
    assert!((db(quiet_left / centre_left) + 30.0).abs() < 0.5, "{}", db(quiet_left / centre_left));
    // -60 dB is silence.
    let (silent_left, _) = heard(VoiceParams { gain_db: -60.0, ..NEUTRAL });
    assert!(silent_left < 1e-6, "{silent_left}");
    // Hard right is all in the right channel.
    let (panned_left, panned_right) = heard(VoiceParams { pan: 1.0, ..NEUTRAL });
    assert!(panned_right > centre_right, "{panned_right} against {centre_right}");
    assert!(panned_left < panned_right * 0.01, "{panned_left} against {panned_right}");
}

/// A tone whose samples miss zero: 1 kHz at 48 kHz lands on it twice a cycle, and would read as
/// silence in [`sounding`].
const QUIET_FREE_TONE: f32 = 997.0;

/// Non-silent frames in interleaved stereo.
fn sounding(stereo: &[f32]) -> usize {
    stereo.as_chunks::<2>().0.iter().filter(|frame| frame[0].abs() + frame[1].abs() > 1e-4).count()
}

#[test]
fn a_frozen_voice_stops_and_goes_on_from_where_it_was() {
    const LENGTH: f32 = 0.4;
    let sound = tones(&[QUIET_FREE_TONE], LENGTH);
    let mut backend = mixer(&sound);
    start(&mut backend, NEUTRAL);
    let mut heard = sounding(&render(&mut backend, 0.1));
    backend.freeze_voice(VoiceSlot(0), true);
    heard += sounding(&render(&mut backend, 0.02));
    // Well after the fade: silence for as long as it lasts, and the sound keeps its place.
    assert_eq!(sounding(&render(&mut backend, 1.0)), 0);
    assert!(!backend.voice_finished(VoiceSlot(0)));
    backend.freeze_voice(VoiceSlot(0), false);
    heard += sounding(&render(&mut backend, 1.0));
    assert!(backend.voice_finished(VoiceSlot(0)));
    // All of it was heard, give or take the fades.
    let expected = (LENGTH * RATE as f32) as i64;
    assert!((heard as i64 - expected).abs() < 400, "{heard} of {expected} frames");
}

/// Ended, a voice is silent within a moment, and gets there without a click.
#[test]
fn a_stopped_voice_ends_at_once_and_smoothly() {
    let sound = tones(&[QUIET_FREE_TONE], 1.0);
    let mut backend = mixer(&sound);
    start(&mut backend, NEUTRAL);
    let before = left(&render(&mut backend, 0.1));
    backend.stop_voice(VoiceSlot(0));
    let after = left(&render(&mut backend, 0.05));
    let largest_step =
        |samples: &[f32]| samples.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max);
    assert!(
        largest_step(&after) <= largest_step(&before) * 1.5,
        "a click: {} against the tone's {}",
        largest_step(&after),
        largest_step(&before)
    );
    let settled = (0.015 * RATE as f32) as usize;
    assert!(after[settled..].iter().all(|sample| sample.abs() < 1e-4), "still sounding after 15 ms");
    assert!(backend.voice_finished(VoiceSlot(0)));
}

/// A frozen voice ends too, and thawing what's left of it brings nothing back.
#[test]
fn a_frozen_voice_ends_when_stopped() {
    let sound = tones(&[QUIET_FREE_TONE], 1.0);
    let mut backend = mixer(&sound);
    backend.start_voice(VoiceSlot(0), SOUND, Bus::Sfx, NEUTRAL, true).expect("a voice");
    assert_eq!(sounding(&render(&mut backend, 0.05)), 0);
    backend.stop_voice(VoiceSlot(0));
    assert_eq!(sounding(&render(&mut backend, 0.05)), 0);
    assert!(backend.voice_finished(VoiceSlot(0)));
}

#[test]
fn a_voice_started_frozen_waits() {
    let sound = tones(&[1_000.0], 0.1);
    let mut backend = mixer(&sound);
    backend.start_voice(VoiceSlot(0), SOUND, Bus::Sfx, NEUTRAL, true).unwrap();
    assert_eq!(sounding(&render(&mut backend, 0.3)), 0);
    backend.freeze_voice(VoiceSlot(0), false);
    assert!(sounding(&render(&mut backend, 0.3)) > 4_000);
    assert!(backend.voice_finished(VoiceSlot(0)));
}

/// A device that doesn't take `f32` hears the same mix, in its own samples, and suspended its own silence.
#[test]
fn a_device_of_integers_hears_the_same_mix() {
    let sound = tones(&[QUIET_FREE_TONE], 0.5);
    let (mut floats, mut integers) = (mixer(&sound), mixer(&sound));
    start(&mut floats, NEUTRAL);
    start(&mut integers, NEUTRAL);
    let heard = render(&mut floats, 0.25);
    let mut samples = vec![0_u16; heard.len()];
    integers.output_mut().render_samples(&mut samples);
    assert!(sounding(&heard) > 0);
    for (float, integer) in heard.iter().zip(&samples) {
        assert_eq!(*integer, u16::from_sample(*float), "{float}");
    }
    let status = integers.status();
    assert_eq!(status.frames, heard.len() as u64 / 2);
    assert_eq!(status.callbacks, floats.status().callbacks);

    integers.suspend();
    integers.output_mut().render_samples(&mut samples);
    assert!(samples.iter().all(|&sample| sample == u16::EQUILIBRIUM), "an unsigned sample's middle");
    assert_eq!(integers.status().frames, status.frames, "the mixer doesn't move");
}

#[test]
fn suspending_keeps_every_voice_where_it_was() {
    const LENGTH: f32 = 0.4;
    let sound = tones(&[QUIET_FREE_TONE], LENGTH);
    let mut backend = mixer(&sound);
    start(&mut backend, NEUTRAL);
    let mut heard = sounding(&render(&mut backend, 0.2));
    backend.suspend();
    assert_eq!(backend.status().state, DeviceState::Suspended);
    assert_eq!(sounding(&render(&mut backend, 2.0)), 0);
    assert!(!backend.voice_finished(VoiceSlot(0)));
    backend.resume();
    assert_eq!(backend.status().state, DeviceState::Running);
    heard += sounding(&render(&mut backend, 0.5));
    assert!(backend.voice_finished(VoiceSlot(0)));
    let expected = (LENGTH * RATE as f32) as i64;
    assert!((heard as i64 - expected).abs() < 400, "{heard} of {expected} frames");
}

#[test]
fn a_cooked_sound_plays_at_its_own_rate_in_a_mix_of_another() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/third_party/kenney_impact_sounds/Audio/impactMetal_heavy_000.ogg");
    let source = Sound::from_vorbis(std::fs::read(path).expect("the sample")).unwrap();
    let cooked = source.to_cooked().unwrap();
    let sound = Sound::read(&cooked).unwrap().decode().unwrap();
    let seconds = sound.frames() as f32 / sound.sample_rate as f32;
    let mut backend = mixer(&sound);
    start(&mut backend, NEUTRAL);
    let out = render(&mut backend, seconds + 0.5);
    let frames_expected = seconds * RATE as f32;
    let last_sounding =
        out.as_chunks::<2>().0.iter().rposition(|frame| frame[0] != 0.0 || frame[1] != 0.0).unwrap();
    assert!(
        (last_sounding as f32 - frames_expected).abs() < 2_048.0,
        "{last_sounding} frames, {frames_expected} expected"
    );
    assert!(rms(&left(&out)) > 0.001, "it is heard");
    assert!(backend.voice_finished(VoiceSlot(0)));
}

#[test]
fn stereo_sounds_stay_stereo() {
    let samples = (0..RATE as usize / 2).flat_map(|_| [0.5, -0.25]).collect();
    let sound = DecodedSound { sample_rate: RATE, channels: 2, samples };
    let mut backend = mixer(&sound);
    start(&mut backend, NEUTRAL);
    let out = render(&mut backend, 0.25);
    let last = out.as_chunks::<2>().0[GAME_FRAME];
    assert!((last[0] - 0.5).abs() < 0.01 && (last[1] + 0.25).abs() < 0.01, "{last:?}");
}

/// The whole path, frame by frame, as a game drives it.
mod through_the_system {
    use super::*;

    fn system(sound: &DecodedSound, cue: Cue) -> AudioSystem<KiraBackend<OfflineOutput>> {
        let mut system = AudioSystem::new(mixer(sound));
        system.add_cue(CUE, cue);
        system
    }

    fn cue() -> Cue {
        Cue {
            sounds: vec![SOUND],
            volume_db: [0.0, 0.0],
            pitch: [1.0, 1.0],
            attenuation: None,
            bus: Bus::Sfx,
            hitstop: HitstopResponse::Ignore,
        }
    }

    /// Runs `frames` game frames at this time scale, the first starting a voice at `target`.
    fn run(
        system: &mut AudioSystem<KiraBackend<OfflineOutput>>,
        target: Option<CueTarget>,
        time_scale: f32,
        frames: usize,
    ) -> Vec<f32> {
        let mut rng = CueRng::new(1);
        let mut out = Vec::new();
        for frame in 0..frames {
            let request = target.filter(|_| frame == 0).map(|target| CueRequest { cue: CUE, target });
            let cues: &[CueRequest] = request.as_slice();
            system.update(
                &AudioFrame {
                    dt: GAME_FRAME as f32 / RATE as f32,
                    listener: Listener::default(),
                    world_time_scale: time_scale,
                    emitters: &[],
                    cues,
                },
                &mut rng,
            );
            out.extend(system.backend_mut().output_mut().render(GAME_FRAME));
        }
        out
    }

    #[test]
    fn slow_motion_really_bends_and_darkens_the_world() {
        let tuning = SnapshotTuning::default();
        let (low, high) = (500.0, 8_000.0);
        let sound = tones(&[low, high], 3.0);
        let mut normal = system(&sound, cue());
        let at_speed = left(&run(&mut normal, Some(CueTarget::Flat), 1.0, 30));
        let tail = |out: &[f32]| out[out.len() - GAME_FRAME * 12..].to_vec();
        let at_speed = tail(&at_speed);
        assert!(energy_at(&at_speed, high) > 0.2);

        let mut slowed = system(&sound, cue());
        let dodge = tail(&left(&run(&mut slowed, Some(CueTarget::Flat), tuning.floor_scale, 30)));
        // Both tones go down by the floor's pitch, and the upper one nearly away.
        let (low_there, high_there) = (low * tuning.floor_pitch, high * tuning.floor_pitch);
        assert!(energy_at(&dodge, low_there) > 0.15, "{}", energy_at(&dodge, low_there));
        assert!(energy_at(&dodge, low) < 0.02, "{}", energy_at(&dodge, low));
        let drop = db(energy_at(&at_speed, high) / energy_at(&dodge, high_there));
        assert!(drop > 30.0, "only {drop} dB down");

        // And when the world speeds up again, it all comes back.
        let back = tail(&left(&run(&mut slowed, None, 1.0, 120)));
        assert!(energy_at(&back, low) > 0.2, "{}", energy_at(&back, low));
    }

    #[test]
    fn distance_and_side_reach_the_output() {
        let placed = Cue {
            attenuation: Some(Attenuation { near: 1.0, far: 21.0, curve: AttenuationCurve::Linear }),
            ..cue()
        };
        let sound = tones(&[1_000.0], 1.0);
        let heard = |position: Vec3| {
            let mut system = system(&sound, placed.clone());
            let out = run(&mut system, Some(CueTarget::At(position)), 1.0, 20);
            (rms(&left(&out)), rms(&right(&out)))
        };
        let (near_left, near_right) = heard(Vec3::new(0.0, 0.0, -1.0));
        let (far_left, _) = heard(Vec3::new(0.0, 0.0, -11.0));
        assert!((db(far_left / near_left) + 30.0).abs() < 1.0, "{}", db(far_left / near_left));
        assert!((near_left - near_right).abs() < 1e-4);
        let (side_left, side_right) = heard(Vec3::new(5.0, 0.0, 0.0));
        assert!(side_right > side_left * 10.0, "{side_left} against {side_right}");
    }

    /// A document can't say this any more, but code can make one. One sample that isn't a number stays in
    /// a bus's filter for good, even with the filter out of the mix, so it has to stay out of the mixer.
    #[test]
    fn a_cue_with_no_near_radius_leaves_the_mix_alive() {
        let placed = Cue {
            attenuation: Some(Attenuation { near: 0.0, far: 30.0, curve: AttenuationCurve::Inverse }),
            ..cue()
        };
        let sound = tones(&[1_000.0], 0.2);
        let mut system = system(&sound, placed);
        let out = run(&mut system, Some(CueTarget::At(Vec3::new(0.0, 0.0, -5.0))), 1.0, 30);
        assert!(out.iter().all(|sample| sample.is_finite()), "it reached the device as NaN");

        // And what plays next on the same bus is heard.
        system.add_cue(CUE, cue());
        let out = run(&mut system, Some(CueTarget::Flat), 1.0, 6);
        assert!(out.iter().all(|sample| sample.is_finite()), "the bus never recovered");
        assert!(rms(&left(&out)) > 0.05, "{}", rms(&left(&out)));
    }

    /// Through the system, as a fight starting over asks it: the old voice is gone from the samples
    /// and the slots, and what plays next plays.
    #[test]
    fn stopped_voices_leave_the_mix_and_free_their_slots() {
        let sound = tones(&[1_000.0], 1.0);
        let mut system = system(&sound, cue());
        let out = run(&mut system, Some(CueTarget::Flat), 1.0, 6);
        assert!(rms(&left(&out)) > 0.05, "{}", rms(&left(&out)));
        assert_eq!(system.stats().voices, 1);

        system.stop_voices();
        let _gone = run(&mut system, None, 1.0, 2);
        let out = run(&mut system, None, 1.0, 6);
        assert_eq!(sounding(&out), 0);
        assert_eq!(system.stats().voices, 0);

        let out = run(&mut system, Some(CueTarget::Flat), 1.0, 6);
        assert!(rms(&left(&out)) > 0.05, "{}", rms(&left(&out)));
        assert_eq!(system.stats().cues_played, 2);
    }

    #[test]
    fn played_out_voices_free_their_slots_as_the_mixer_runs() {
        let sound = tones(&[1_000.0], 0.05);
        let mut system = system(&sound, cue());
        run(&mut system, Some(CueTarget::Flat), 1.0, 10);
        assert_eq!(system.stats().voices, 0);
        assert_eq!(system.stats().cues_played, 1);
    }
}
