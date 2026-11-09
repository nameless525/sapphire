//! Music and voice lines (M3 A1) through the real mixer, rendered with no device: where a phase change
//! lands in the samples, that the stems keep time with the clock that schedules it, and that the music
//! goes down under a line. The decisions are tested against the mock, in `src/music.rs`.

#![allow(clippy::print_stdout, reason = "the latencies are what the plan's note reports")]

use sapphire_assets::AssetId;
use sapphire_assets::cue::{Bus, Cue, HitstopResponse};
use sapphire_assets::score::{Phase, Score, Stem};
use sapphire_assets::sound::DecodedSound;
use sapphire_assets::voice::{VoiceLine, VoiceLines};
use sapphire_audio::{
    AudioFrame, AudioSystem, CueRng, CueTarget, KiraBackend, LineKey, LineRequest, Listener, MusicRequest,
    OfflineOutput,
};

const RATE: u32 = 48_000;
/// A frame of the game at 60 fps, in audio frames.
const GAME_FRAME: usize = RATE as usize / 60;
/// A beat at 120 beats a minute, and a bar of four.
const BEAT: usize = RATE as usize / 2;
const BAR: usize = 4 * BEAT;
/// The mixer's internal buffer: what it sets parameters and starts sounds by.
const BUFFER: usize = 128;

const SCORE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-000000000001");
const CLICK: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a0");
const LAYER: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a1");
const LINE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000b0");

/// A bar-long score of two stems: a click on its first frame, always up, which shows where the stems
/// are; and a steady negative level, up only while `running`, which shows where the change landed.
fn system(fade_beats: f32) -> AudioSystem<KiraBackend<OfflineOutput>> {
    let mut system = AudioSystem::new(KiraBackend::<OfflineOutput>::new(RATE).unwrap());
    let mut click = vec![0.0; BAR];
    click[0] = 0.5;
    system.add_sound(CLICK, &DecodedSound { sample_rate: RATE, channels: 1, samples: click });
    system.add_sound(LAYER, &DecodedSound { sample_rate: RATE, channels: 1, samples: vec![-0.25; BAR] });
    system.add_score(
        SCORE,
        Score {
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            bars: 1,
            stems: vec![
                Stem { name: "click".to_owned(), sound: CLICK },
                Stem { name: "layer".to_owned(), sound: LAYER },
            ],
            phases: vec![
                Phase { name: "waiting".to_owned(), play: vec!["click".to_owned()] },
                Phase { name: "running".to_owned(), play: vec!["click".to_owned(), "layer".to_owned()] },
            ],
            fade_beats,
            volume_db: 0.0,
            duck_db: -10.0,
        },
    );
    system
}

/// Plays `phase` for game frames until `until` audio frames have been rendered, keeping the left
/// channel in `out`.
fn play_until(
    system: &mut AudioSystem<KiraBackend<OfflineOutput>>,
    phase: &str,
    lines: &[LineRequest],
    until: usize,
    out: &mut Vec<f32>,
) {
    let mut block = vec![0.0; GAME_FRAME * 2];
    let mut lines = lines;
    while out.len() < until {
        let frame = AudioFrame {
            dt: GAME_FRAME as f32 / RATE as f32,
            listener: Listener::default(),
            world_time_scale: 1.0,
            emitters: &[],
            cues: &[],
            lines: std::mem::take(&mut lines),
            music: Some(MusicRequest { score: SCORE, phase }),
        };
        system.update(&frame, &mut CueRng::new(1));
        system.backend_mut().output_mut().render_into(&mut block);
        out.extend(block.as_chunks::<2>().0.iter().map(|frame| frame[0]));
    }
}

/// Where the music's first frame is in `out`: its first click. The stems wait a game frame for their
/// clock ([`sapphire_audio::AudioBackend::load_music`]), so it's never the output's first.
fn first_beat(out: &[f32]) -> usize {
    out.iter().position(|&sample| sample > 0.25).expect("the music started")
}

#[test]
fn a_phase_change_sounds_on_the_bar_line_and_the_stems_keep_time() {
    let mut system = system(0.0);
    let mut out = Vec::new();
    // Asked a quarter of the way into the first bar's second beat: the next bar line is beat 4.
    play_until(&mut system, "waiting", &[], GAME_FRAME + BEAT + BEAT / 4, &mut out);
    let origin = first_beat(&out);
    play_until(&mut system, "running", &[], origin + 11 * BAR + BEAT, &mut out);
    // And back, a beat into the twelfth loop: it ends on the thirteenth's line.
    let asked_back = out.len();
    play_until(&mut system, "waiting", &[], origin + 13 * BAR, &mut out);

    // The layer's level, the clicks left out: 0 down, 1 up.
    let is_click = |at: usize| at >= origin && (at - origin).is_multiple_of(BAR);
    let level = |at: usize| if is_click(at) { f32::NAN } else { -out[at] / 0.25 };
    let first =
        |from: usize, test: &dyn Fn(f32) -> bool| (from..out.len()).find(|&at| test(level(at))).unwrap();
    // A change with no fade still takes the one internal buffer it lands in, moving in decibels.
    let (in_starts, in_ends) = (first(0, &|level| level > 1e-3), first(0, &|level| level > 0.999));
    let (out_starts, out_ends) =
        (first(asked_back, &|level| level < 0.999), first(asked_back, &|level| level < 1e-3));
    let line_in = (origin + BAR) as i64;
    let line_out = (origin + 12 * BAR) as i64;
    let ms = |frames: i64| frames as f64 * 1e3 / f64::from(RATE);
    let spans = [
        (in_starts as i64 - line_in, in_ends as i64 - line_in),
        (out_starts as i64 - line_out, out_ends as i64 - line_out),
    ];
    println!(
        "the music's first frame: {origin} frames in, a game frame being {GAME_FRAME}; in: from {} to {} frames \
         after the bar line ({:.2} to {:.2} ms); out, eleven loops on: from {} to {} ({:.2} to {:.2} ms)",
        spans[0].0,
        spans[0].1,
        ms(spans[0].0),
        ms(spans[0].1),
        spans[1].0,
        spans[1].1,
        ms(spans[1].0),
        ms(spans[1].1)
    );
    assert!(origin <= GAME_FRAME + BUFFER, "the music took {origin} frames to start");
    // The mixer moves a stem in the buffer its beat falls in: it begins no more than a buffer early and
    // is done no more than a buffer late.
    for (starts, ends) in spans {
        assert!(
            starts >= -(BUFFER as i64) && ends <= BUFFER as i64,
            "the change took frames {starts} to {ends}"
        );
    }
    // Every loop's click is where the clock's bar starts, to the frame.
    for bar in 0..13 {
        let at = origin + bar * BAR;
        assert!(out[at] > 0.2, "bar {bar}'s click isn't at frame {at}: {}", out[at]);
    }
}

#[test]
fn a_fade_starts_on_the_bar_line_and_takes_its_beats() {
    let mut system = system(1.0);
    let mut out = Vec::new();
    play_until(&mut system, "waiting", &[], GAME_FRAME + BEAT, &mut out);
    let origin = first_beat(&out);
    play_until(&mut system, "running", &[], origin + 2 * BAR, &mut out);
    let level = |at: usize| -out[origin + at + 1];
    assert!(level(BAR - BUFFER) < 0.001, "it waited for the bar: {}", level(BAR - BUFFER));
    // A fade linear in decibels, from silence: still quiet halfway through the beat, all the way up
    // after it.
    assert!(level(BAR + BEAT / 2) < 0.05, "{}", level(BAR + BEAT / 2));
    assert!((level(BAR + BEAT + BUFFER) - 0.25).abs() < 0.01, "{}", level(BAR + BEAT + BUFFER));
}

#[test]
fn the_music_goes_down_under_a_line_and_comes_back() {
    let mut system = system(0.0);
    // A line half a second long, of silence, so only the music is heard.
    system.add_sound(
        LINE,
        &DecodedSound { sample_rate: RATE, channels: 1, samples: vec![0.0; RATE as usize / 2] },
    );
    let cue = Cue {
        sounds: vec![LINE],
        volume_db: [0.0, 0.0],
        pitch: [1.0, 1.0],
        attenuation: None,
        bus: Bus::Voice,
        hitstop: HitstopResponse::Ignore,
    };
    system.add_cue(LINE, cue);
    let lines =
        VoiceLines { locale: "ja".to_owned(), lines: vec![VoiceLine { key: "won".to_owned(), cue: LINE }] };
    system.add_voice_lines(&lines).unwrap();
    assert!(system.set_voice_language("ja"));

    let mut out = Vec::new();
    play_until(&mut system, "running", &[], GAME_FRAME + BAR, &mut out);
    let before = -out[out.len() - 1];
    let said = [LineRequest { line: LineKey::of("won"), target: CueTarget::Flat }];
    let asked = out.len();
    play_until(&mut system, "running", &said, asked + RATE as usize / 4, &mut out);
    let under = -out[out.len() - 1];
    // The line ends half a second in; the music is back 400 ms after the frame that sees it end.
    play_until(&mut system, "running", &[], asked + RATE as usize * 3 / 2, &mut out);
    let after = -out[out.len() - 1];
    let ducked = 20.0 * (under / before).log10();
    println!("under a line: {ducked:.2} dB");
    assert!((ducked + 10.0).abs() < 0.5, "the music went {ducked:.2} dB down, not 10");
    assert!((after - before).abs() < 0.001, "it came back to {after}, from {before}");
    assert_eq!(system.stats().lines_asked, 1);
}
