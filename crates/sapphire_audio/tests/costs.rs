//! What audio costs, for spec §3.2's budget and the plan's "As built": prints the numbers, and fails
//! on none of them, since a loaded machine would fail it. `#[ignore]`d, and only meaningful optimized:
//! `cargo test -p sapphire_audio --release --test costs -- --ignored --nocapture --test-threads=1`.
//! Update/mix lines give the observed minimum and median pooled across all rounds; decoding gives
//! the observed minimum. These describe this instrumented workload, not stable tail estimates.
//! One immutable tone is synthesized per process; mixer construction and PCM conversion still
//! happen independently for every round, outside the timed update/mix scopes.
//! Silent: it renders into a buffer, and opens no device.

#![allow(clippy::print_stdout, reason = "printing the numbers is this test's whole job")]

use std::path::Path;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Attenuation, AttenuationCurve, Bus, Cue, HitstopResponse};
use sapphire_assets::score::{Phase, Score, Stem};
use sapphire_assets::sound::{DecodedSound, decode_vorbis};
use sapphire_audio::{
    AudioFrame, AudioSystem, BusId, CueRequest, CueRng, CueTarget, Emitter, EmitterId, KiraBackend, Listener,
    MAX_VOICES, MusicRequest, OfflineOutput,
};
use sapphire_math::Vec3;
use sapphire_time::Stopwatch;

const RATE: u32 = 48_000;
/// A device's usual block.
const BLOCK: usize = 512;
const ROUNDS: usize = 12;
const SOUND: AssetId = AssetId::from_text("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c");
const CUE: AssetId = AssetId::from_text("11111111-2222-4333-8444-555555555555");
const ACTOR: EmitterId = EmitterId(1);

/// Long enough to outlast a round.
fn sound() -> DecodedSound {
    let samples = (0..RATE as usize * 30).map(|frame| (frame as f32 * 0.05).sin() * 0.1).collect();
    DecodedSound { sample_rate: RATE, channels: 1, samples }
}

fn system(sound: &DecodedSound) -> AudioSystem<KiraBackend<OfflineOutput>> {
    let Ok(backend) = KiraBackend::<OfflineOutput>::new(RATE);
    let mut system = AudioSystem::new(backend);
    system.add_sound(SOUND, sound);
    system.add_cue(
        CUE,
        Cue {
            sounds: vec![SOUND],
            volume_db: [-12.0, -6.0],
            pitch: [0.95, 1.05],
            attenuation: Some(Attenuation { near: 1.0, far: 40.0, curve: AttenuationCurve::Inverse }),
            bus: Bus::Sfx,
            hitstop: HitstopResponse::Freeze,
            ..Cue::default()
        },
    );
    system
}

/// A frame's input, with the actor and the listener both moving, so every voice takes a command.
fn frame_at<'a>(tick: usize, emitters: &'a mut [Emitter; 1], cues: &'a [CueRequest]) -> AudioFrame<'a> {
    let angle = tick as f32 * 0.5;
    emitters[0].position = Vec3::new(angle.sin() * 6.0, 1.0, angle.cos() * 6.0);
    let listener = Listener { position: Vec3::new(0.0, 1.6, angle.cos()), ..Listener::default() };
    AudioFrame {
        dt: 1.0 / 60.0,
        listener,
        world_time_scale: 0.6 + 0.4 * angle.sin(),
        snapshots: &[],
        emitters,
        cues,
        lines: &[],
        speech: None,
        music: None,
    }
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

/// The fastest and the middle of `times`.
fn min_and_median(times: &mut [Duration]) -> (Duration, Duration) {
    times.sort_unstable();
    (times[0], times[times.len() / 2])
}

#[test]
#[ignore = "prints what audio costs: run it optimized, by hand"]
fn what_audio_costs() {
    let sound = sound();
    let requests = [CueRequest { cue: CUE, target: CueTarget::On(ACTOR) }];
    let start = [Emitter { id: ACTOR, position: Vec3::ZERO, hitstop: false }];

    // A frame's update, by how many voices it moves, with the mix keeping up beside it.
    for voices in [0, 1, 8, MAX_VOICES] {
        let mut times = Vec::new();
        for _ in 0..ROUNDS {
            let mut system = system(&sound);
            let mut random = CueRng::new(7);
            let mut emitters = start;
            for _ in 0..voices {
                system.update(&frame_at(0, &mut emitters, &requests), &mut random);
            }
            let mut block = vec![0.0; BLOCK * 2];
            system.backend_mut().output_mut().render_into(&mut block);
            for tick in 1..60 {
                let timer = Stopwatch::start();
                system.update(&frame_at(tick, &mut emitters, &[]), &mut random);
                times.push(timer.elapsed());
                system.backend_mut().output_mut().render_into(&mut block);
            }
            assert_eq!(system.stats().voices as usize, voices);
        }
        let (fastest, median) = min_and_median(&mut times);
        println!(
            "update, {voices} voices moving: {:.2} us, median {:.2} us",
            micros(fastest),
            micros(median)
        );
    }

    // A frame that also starts a cue: what a hit costs the frame it lands on.
    let mut times = Vec::new();
    for _ in 0..ROUNDS {
        let mut system = system(&sound);
        let mut random = CueRng::new(7);
        let mut emitters = start;
        for tick in 0..16 {
            let timer = Stopwatch::start();
            system.update(&frame_at(tick, &mut emitters, &requests), &mut random);
            times.push(timer.elapsed());
        }
        assert_eq!(system.stats().cues_played, 16, "{:?}", system.stats());
    }
    let (fastest, median) = min_and_median(&mut times);
    println!(
        "update, starting one cue with 0..15 voices live: {:.2} us, median {:.2} us",
        micros(fastest),
        micros(median)
    );

    // The mix, on the callback, against the block's length in real time.
    let length = Duration::from_secs_f64(BLOCK as f64 / f64::from(RATE));
    for voices in [1, 8, MAX_VOICES] {
        let mut times = Vec::new();
        for _ in 0..ROUNDS {
            let mut system = system(&sound);
            let mut random = CueRng::new(7);
            let mut emitters = start;
            for _ in 0..voices {
                system.update(&frame_at(0, &mut emitters, &requests), &mut random);
            }
            let mut block = vec![0.0; BLOCK * 2];
            for tick in 1..64 {
                system.update(&frame_at(tick, &mut emitters, &[]), &mut random);
                let timer = Stopwatch::start();
                system.backend_mut().output_mut().render_into(&mut block);
                times.push(timer.elapsed());
            }
        }
        let (fastest, median) = min_and_median(&mut times);
        println!(
            "mix of {BLOCK} frames, {voices} voices: {:.1} us, median {:.1} us, {:.2}% of real time at the median",
            micros(fastest),
            micros(median),
            median.as_secs_f64() / length.as_secs_f64() * 100.0
        );
    }

    voice_against_kiras_static_sound(&sound);

    // Decoding real sounds, against their length.
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party");
    for name in
        ["kenney_impact_sounds/Audio/impactMetal_heavy_000.ogg", "kenney_rpg_audio/Audio/metalPot1.ogg"]
    {
        let vorbis = std::fs::read(content.join(name)).expect("the sound is in the tree");
        let mut best = Duration::MAX;
        let mut seconds = 0.0;
        for _ in 0..ROUNDS {
            let timer = Stopwatch::start();
            let decoded = decode_vorbis(&vorbis).expect("it decodes");
            best = best.min(timer.elapsed());
            seconds = decoded.frames() as f64 / f64::from(decoded.sample_rate);
        }
        println!(
            "decode {name}: {} bytes, {seconds:.3} s of sound in minimum {:.0} us, 1/{:.0} of its length",
            vorbis.len(),
            micros(best),
            seconds / best.as_secs_f64()
        );
    }

    music_costs();
}

/// U1's kill line (m6-plan): the engine voice's mix at [`MAX_VOICES`] against kira's own static sound, as
/// v0.2.0 played every voice, on the same bus track with the same parameters, the voice's low-pass open and
/// then closed. Both mixes are timed in alternate rounds, so a load that comes and goes falls on both.
fn voice_against_kiras_static_sound(sound: &DecodedSound) {
    use std::sync::Arc;

    use kira::effect::filter::{FilterBuilder, FilterMode};
    use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
    use kira::track::TrackBuilder;
    use kira::{AudioManager, AudioManagerSettings, Decibels, Frame, Mix, Panning, PlaybackRate};
    use sapphire_audio::{AudioBackend, VoiceParams, VoiceSlot};

    let params = |n: usize, low_pass_hz: f32| VoiceParams {
        gain_db: -9.0,
        pan: 0.3,
        pitch: 0.95 + 0.003 * n as f32,
        low_pass_hz,
    };
    let kira_frames: Arc<[Frame]> = sound.samples.iter().map(|&sample| Frame::from_mono(sample)).collect();
    let kiras = || {
        let Ok(mut manager) = AudioManager::<OfflineOutput>::new(AudioManagerSettings {
            internal_buffer_size: 128,
            backend_settings: RATE,
            ..AudioManagerSettings::default()
        });
        let mut builder = TrackBuilder::new().sound_capacity(MAX_VOICES);
        builder.add_effect(FilterBuilder::new().mode(FilterMode::LowPass).cutoff(20_000.0).mix(Mix::DRY));
        let mut track = manager.add_sub_track(builder).expect("a track");
        for n in 0..MAX_VOICES {
            let voice = params(n, f32::INFINITY);
            let data = StaticSoundData {
                sample_rate: RATE,
                frames: Arc::clone(&kira_frames),
                settings: StaticSoundSettings::new()
                    .volume(Decibels(voice.gain_db))
                    .panning(Panning(voice.pan))
                    .playback_rate(PlaybackRate(f64::from(voice.pitch))),
                slice: None,
            };
            track.play(data).expect("room for the voice");
        }
        (manager, track)
    };
    let ours = |low_pass_hz: f32| {
        let Ok(mut backend) = KiraBackend::<OfflineOutput>::new(RATE);
        backend.add_sound(SOUND, sound);
        for n in 0..MAX_VOICES {
            let sfx = BusId::category(Bus::Sfx);
            backend.start_voice(VoiceSlot(n as u32), SOUND, sfx, params(n, low_pass_hz), false).unwrap();
        }
        backend
    };
    let mut block = vec![0.0; BLOCK * 2];
    let (mut kira_times, mut open_times, mut closed_times) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let (mut manager, _track) = kiras();
        let (mut open, mut closed) = (ours(f32::INFINITY), ours(2_000.0));
        for _ in 0..64 {
            let timer = Stopwatch::start();
            manager.backend_mut().render_into(&mut block);
            kira_times.push(timer.elapsed());
            let timer = Stopwatch::start();
            open.output_mut().render_into(&mut block);
            open_times.push(timer.elapsed());
            let timer = Stopwatch::start();
            closed.output_mut().render_into(&mut block);
            closed_times.push(timer.elapsed());
        }
    }
    let ratio = |ours: Duration, kiras: Duration| ours.as_secs_f64() / kiras.as_secs_f64();
    let (kira_fastest, kira) = min_and_median(&mut kira_times);
    let (open_fastest, open) = min_and_median(&mut open_times);
    let (closed_fastest, closed) = min_and_median(&mut closed_times);
    println!(
        "mix of {BLOCK} frames, {MAX_VOICES} voices, minimum and median: kira's static sound {:.1}/{:.1} us; the engine voice {:.1}/{:.1} us open ({:.2}x/{:.2}x), {:.1}/{:.1} us through its low-pass ({:.2}x/{:.2}x); the kill line is 1.5x",
        micros(kira_fastest),
        micros(kira),
        micros(open_fastest),
        micros(open),
        ratio(open_fastest, kira_fastest),
        ratio(open, kira),
        micros(closed_fastest),
        micros(closed),
        ratio(closed_fastest, kira_fastest),
        ratio(closed, kira),
    );
}

const SCORE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-000000000001");

/// The shipped placeholder score's four stems (24 kHz, resampled by the mixer), its phases changing
/// every frame for the update's worst, and eight voices beside it for the mix's.
fn music_costs() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/audio/generated/music");
    let names = ["pad", "pulse", "drums", "bells"];
    let mut decoding = Duration::ZERO;
    let stems: Vec<(AssetId, DecodedSound)> = names
        .iter()
        .enumerate()
        .map(|(n, name)| {
            let vorbis =
                std::fs::read(folder.join(format!("lantern_{name}.ogg"))).expect("the stem is in the tree");
            let timer = Stopwatch::start();
            let decoded = decode_vorbis(&vorbis).expect("it decodes");
            decoding += timer.elapsed();
            (format!("5c0e0000-0000-4000-8000-0000000000a{n}").parse().unwrap(), decoded)
        })
        .collect();
    println!("decode the four stems, 8 s each: {:.1} ms", decoding.as_secs_f64() * 1e3);
    let score = Score {
        tempo_bpm: 120.0,
        beats_per_bar: 4,
        bars: 4,
        stems: stems
            .iter()
            .zip(names)
            .map(|((id, _), name)| Stem { name: name.to_owned(), sound: *id })
            .collect(),
        phases: vec![
            Phase { name: "waiting".to_owned(), play: vec!["pad".to_owned()] },
            Phase { name: "running".to_owned(), play: names.iter().map(|&name| name.to_owned()).collect() },
        ],
        fade_beats: 2.0,
        volume_db: -6.0,
    };
    let sound = sound();
    let music_system = || {
        let mut system = system(&sound);
        for (id, stem) in &stems {
            system.add_sound(*id, stem);
        }
        system.add_score(SCORE, score.clone());
        system
    };
    let requests = [CueRequest { cue: CUE, target: CueTarget::On(ACTOR) }];
    let start = [Emitter { id: ACTOR, position: Vec3::ZERO, hitstop: false }];
    let (mut updates, mut mixes) = (Vec::new(), Vec::new());
    for _ in 0..ROUNDS {
        let mut system = music_system();
        let mut random = CueRng::new(7);
        let mut emitters = start;
        let mut block = vec![0.0; BLOCK * 2];
        for tick in 0..8 {
            let music = Some(MusicRequest { score: SCORE, phase: "waiting", instead: None });
            system.update(&AudioFrame { music, ..frame_at(tick, &mut emitters, &requests) }, &mut random);
            system.backend_mut().output_mut().render_into(&mut block);
        }
        for tick in 8..72 {
            let phase = if tick % 2 == 0 { "running" } else { "waiting" };
            let music = Some(MusicRequest { score: SCORE, phase, instead: None });
            let timer = Stopwatch::start();
            system.update(&AudioFrame { music, ..frame_at(tick, &mut emitters, &[]) }, &mut random);
            updates.push(timer.elapsed());
            let timer = Stopwatch::start();
            system.backend_mut().output_mut().render_into(&mut block);
            mixes.push(timer.elapsed());
        }
        assert_eq!(system.stats().music_phase_changes, 64);
    }
    let (fastest, median) = min_and_median(&mut updates);
    println!(
        "update, 8 voices moving and the phase changing: {:.2} us, median {:.2} us",
        micros(fastest),
        micros(median)
    );
    let length = Duration::from_secs_f64(BLOCK as f64 / f64::from(RATE));
    let (fastest, median) = min_and_median(&mut mixes);
    println!(
        "mix of {BLOCK} frames, four stems and 8 voices: {:.1} us, median {:.1} us, {:.2}% of real time at the median",
        micros(fastest),
        micros(median),
        median.as_secs_f64() / length.as_secs_f64() * 100.0
    );
}
