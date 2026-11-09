//! A score's stems by phase (M3 A1): which play, and the bar a change of phase lands on.
//!
//! Every stem of a score loops from the same first frame, the silent ones at
//! [`SILENCE_DB`], so they stay in time with one another; a phase is the set that's up. A change of
//! phase fades the stems that differ on the next bar line of the music's clock, as the mixer counts
//! it, so the frame that asks only has to ask a little ahead: [`LEAD_SECONDS`].

use std::collections::HashMap;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::SILENCE_DB;
use sapphire_assets::score::{MAX_STEMS, Score};

use crate::LOG_TARGET;
use crate::backend::{AudioBackend, StemStart};
use crate::voice::fnv1a;

/// How far ahead of a bar line a change must be asked to land on it. The clock a frame reads is up to
/// one device callback old, and the command lands at the next: on desktop and phone devices each is
/// 5 to 20 ms. A change asked closer to the line takes the bar after.
pub const LEAD_SECONDS: f64 = 0.05;

/// The music asked for this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicRequest<'a> {
    /// The score, added with [`AudioSystem::add_score`](crate::AudioSystem::add_score).
    pub score: AssetId,
    /// One of its phases, by name.
    pub phase: &'a str,
}

/// A score, with each phase's stems as a mask.
struct LoadedScore {
    score: Score,
    /// Bit `n` set: stem `n` plays in that phase.
    masks: Vec<u8>,
}

impl LoadedScore {
    fn beat_seconds(&self) -> f64 {
        60.0 / f64::from(self.score.tempo_bpm)
    }

    fn fade(&self) -> Duration {
        Duration::from_secs_f64(f64::from(self.score.fade_beats) * self.beat_seconds())
    }

    fn gain_of(&self, mask: u8, stem: usize) -> f32 {
        if mask & (1 << stem) != 0 { self.score.volume_db } else { SILENCE_DB }
    }
}

/// The score playing.
struct Playing {
    score: AssetId,
    /// Readied, and told to play on a later frame ([`AudioBackend::load_music`]).
    started: bool,
    /// The stems up, or going up at the bar.
    mask: u8,
    /// The name of the phase last asked for, hashed, so a frame asking the same compares one number.
    asked: u64,
}

/// What [`Music::update`] did, for the stats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MusicCounts {
    pub phase_changes: u64,
    pub unknown_phases: u64,
}

/// The scores, and the one playing.
#[derive(Default)]
pub(crate) struct Music {
    scores: HashMap<AssetId, LoadedScore>,
    playing: Option<Playing>,
    /// A score that couldn't start, not tried again.
    failed: Option<AssetId>,
    pub counts: MusicCounts,
}

impl Music {
    pub fn add_score(&mut self, id: AssetId, score: Score) {
        let masks = score
            .phases
            .iter()
            .map(|phase| {
                // `Score::check` held every name to a stem.
                phase.play.iter().filter_map(|name| score.stem(name)).fold(0, |mask, stem| mask | 1 << stem)
            })
            .collect();
        self.scores.insert(id, LoadedScore { score, masks });
    }

    /// How far the music goes down under a voice line: the playing score's, or nothing.
    pub fn duck_db(&self) -> f32 {
        self.playing
            .as_ref()
            .and_then(|playing| self.scores.get(&playing.score))
            .map_or(0.0, |loaded| loaded.score.duck_db)
    }

    /// One frame: starts, changes or ends the music as `request` asks. `plays` says whether the output
    /// would be heard, without which nothing starts.
    pub fn update(
        &mut self,
        request: Option<MusicRequest<'_>>,
        plays: bool,
        backend: &mut impl AudioBackend,
    ) {
        let Some(request) = request else {
            if let Some(playing) = self.playing.take() {
                let fade = self.scores.get(&playing.score).map_or(Duration::ZERO, LoadedScore::fade);
                backend.stop_music(fade);
            }
            return;
        };
        match &mut self.playing {
            Some(playing) if playing.score == request.score => {
                if !playing.started {
                    playing.started = true;
                    backend.play_music();
                }
                self.change_phase(request.phase, backend);
            }
            _ if plays && self.failed != Some(request.score) => self.start(request, backend),
            _ => {}
        }
    }

    /// Readies `request`'s score at its phase, ending what played before over its own fade.
    fn start(&mut self, request: MusicRequest<'_>, backend: &mut impl AudioBackend) {
        let Some(loaded) = self.scores.get(&request.score) else {
            self.failed = Some(request.score);
            tracing::warn!(target: LOG_TARGET, score = %request.score, "no music: the score isn't loaded");
            return;
        };
        let mask = match loaded.score.phase(request.phase) {
            Some(phase) => loaded.masks[phase],
            None => {
                self.counts.unknown_phases += 1;
                0
            }
        };
        // On the stack: past the score's own count, the rest are never read.
        let stems: [StemStart; MAX_STEMS] = std::array::from_fn(|index| {
            let stem = loaded.score.stems.get(index).unwrap_or(&loaded.score.stems[0]);
            StemStart { sound: stem.sound, gain_db: loaded.gain_of(mask, index) }
        });
        let fade = self
            .playing
            .as_ref()
            .and_then(|playing| self.scores.get(&playing.score))
            .map_or(Duration::ZERO, LoadedScore::fade);
        match backend.load_music(loaded.score.tempo_bpm, &stems[..loaded.score.stems.len()], fade) {
            Ok(()) => {
                self.playing = Some(Playing {
                    score: request.score,
                    started: false,
                    mask,
                    asked: fnv1a(request.phase.as_bytes()),
                });
            }
            Err(error) => {
                self.playing = None;
                self.failed = Some(request.score);
                tracing::warn!(target: LOG_TARGET, score = %request.score, %error, "no music");
            }
        }
    }

    /// Fades the stems `phase` changes on the next bar line it can make.
    fn change_phase(&mut self, phase: &str, backend: &mut impl AudioBackend) {
        let Some(playing) = &mut self.playing else { return };
        let asked = fnv1a(phase.as_bytes());
        if asked == playing.asked {
            return;
        }
        playing.asked = asked;
        let loaded = &self.scores[&playing.score];
        let Some(index) = loaded.score.phase(phase) else {
            // The stems stay as they were.
            self.counts.unknown_phases += 1;
            return;
        };
        self.counts.phase_changes += 1;
        let mask = loaded.masks[index];
        if mask == playing.mask {
            return;
        }
        let bar = next_bar(backend.music_beats(), loaded);
        let fade = loaded.fade();
        for stem in 0..loaded.score.stems.len() {
            if (mask ^ playing.mask) & (1 << stem) != 0 {
                backend.fade_stem(stem, loaded.gain_of(mask, stem), bar, fade);
            }
        }
        playing.mask = mask;
    }
}

/// The first bar line at least [`LEAD_SECONDS`] after `beats`, in beats: the first, while the clock
/// hasn't run.
fn next_bar(beats: Option<f64>, loaded: &LoadedScore) -> u64 {
    let Some(beats) = beats else { return 0 };
    let per_bar = f64::from(loaded.score.beats_per_bar);
    let earliest = beats + LEAD_SECONDS / loaded.beat_seconds();
    ((earliest / per_bar).ceil() * per_bar) as u64
}

#[cfg(test)]
mod tests {
    use sapphire_assets::cue::{Bus, Cue, HitstopResponse};
    use sapphire_assets::score::{Phase, Stem};
    use sapphire_assets::sound::DecodedSound;
    use sapphire_assets::voice::{VoiceLine, VoiceLines};

    use super::*;
    use crate::backend::DeviceState;
    use crate::mock::{MockBackend, MockCall};
    use crate::rng::CueRng;
    use crate::system::{
        AudioFrame, AudioSystem, CueTarget, DUCK_ATTACK, DUCK_RELEASE, LineRequest, Listener,
    };
    use crate::voice::LineKey;

    const SCORE: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-000000000001");
    const STEMS: [AssetId; 3] = [
        AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a0"),
        AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a1"),
        AssetId::from_text("5c0e0000-0000-4000-8000-0000000000a2"),
    ];
    const LINE_EN: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000b0");
    const LINE_JA: AssetId = AssetId::from_text("5c0e0000-0000-4000-8000-0000000000b1");
    const VOLUME_DB: f32 = -6.0;
    /// A beat at 120 beats a minute, and the score's fade.
    const HALF_A_SECOND: Duration = Duration::from_millis(500);

    fn score() -> Score {
        let phase = |name: &str, play: &[&str]| Phase {
            name: name.to_owned(),
            play: play.iter().map(|&stem| stem.to_owned()).collect(),
        };
        let stems = ["pad", "drums", "lead"].iter().zip(STEMS);
        Score {
            tempo_bpm: 120.0,
            beats_per_bar: 4,
            bars: 4,
            stems: stems.map(|(name, sound)| Stem { name: (*name).to_owned(), sound }).collect(),
            phases: vec![
                phase("waiting", &["pad"]),
                phase("running", &["pad", "drums", "lead"]),
                phase("cleared", &["pad", "lead"]),
            ],
            fade_beats: 1.0,
            volume_db: VOLUME_DB,
            duck_db: -10.0,
        }
    }

    /// The score's stems, and a line in each of two languages, each a cue of its own sound.
    fn system() -> AudioSystem<MockBackend> {
        let mut system = AudioSystem::new(MockBackend::new());
        let sound = DecodedSound { sample_rate: 48_000, channels: 1, samples: vec![0.25; 480] };
        for id in STEMS.into_iter().chain([LINE_EN, LINE_JA]) {
            system.add_sound(id, &sound);
            let cue = Cue {
                sounds: vec![id],
                volume_db: [0.0, 0.0],
                pitch: [1.0, 1.0],
                attenuation: None,
                bus: Bus::Voice,
                hitstop: HitstopResponse::Ignore,
            };
            system.add_cue(id, cue);
        }
        system.add_score(SCORE, score());
        system.backend_mut().take_calls();
        system
    }

    fn update(
        system: &mut AudioSystem<MockBackend>,
        phase: Option<&str>,
        lines: &[LineRequest],
    ) -> Vec<MockCall> {
        let frame = AudioFrame {
            dt: 1.0 / 60.0,
            listener: Listener::default(),
            world_time_scale: 1.0,
            emitters: &[],
            cues: &[],
            lines,
            music: phase.map(|phase| MusicRequest { score: SCORE, phase }),
        };
        system.update(&frame, &mut CueRng::new(1));
        system.backend_mut().take_calls()
    }

    #[test]
    fn a_phase_change_fades_the_stems_it_changes_on_the_next_bar_line() {
        let mut system = system();
        let loaded = [MockCall::LoadMusic { tempo_bpm: 120.0, fade: Duration::ZERO }];
        assert_eq!(update(&mut system, Some("waiting"), &[]), loaded);
        let gains: Vec<f32> = system.backend().stems().iter().map(|stem| stem.gain_db).collect();
        assert_eq!(gains, [VOLUME_DB, SILENCE_DB, SILENCE_DB], "only the pad plays while it waits");
        assert_eq!(update(&mut system, Some("waiting"), &[]), [MockCall::PlayMusic], "a frame later");

        // Mid-bar: the next bar line is beat 8.
        system.backend_mut().set_music_beats(Some(5.3));
        assert_eq!(
            update(&mut system, Some("running"), &[]),
            [
                MockCall::FadeStem { stem: 1, gain_db: VOLUME_DB, beat: 8, fade: HALF_A_SECOND },
                MockCall::FadeStem { stem: 2, gain_db: VOLUME_DB, beat: 8, fade: HALF_A_SECOND },
            ]
        );
        // 15 ms short of a bar line can't be sure of it: the one after.
        system.backend_mut().set_music_beats(Some(7.97));
        assert_eq!(
            update(&mut system, Some("cleared"), &[]),
            [MockCall::FadeStem { stem: 1, gain_db: SILENCE_DB, beat: 12, fade: HALF_A_SECOND }]
        );
        assert_eq!(update(&mut system, Some("cleared"), &[]), [], "the same phase asks nothing");
        assert_eq!(system.stats().music_phase_changes, 2);

        // A phase the score lacks leaves the stems as they were, and counts once.
        assert_eq!(update(&mut system, Some("boss-2"), &[]), []);
        assert_eq!(update(&mut system, Some("boss-2"), &[]), []);
        assert_eq!(system.stats().music_unknown_phases, 1);
    }

    #[test]
    fn music_starts_only_where_its_heard_and_ends_with_its_fade() {
        let mut system = system();
        system.backend_mut().set_state(DeviceState::NoDevice);
        assert_eq!(update(&mut system, Some("running"), &[]), []);
        system.backend_mut().set_state(DeviceState::Running);
        assert!(matches!(update(&mut system, Some("running"), &[])[..], [MockCall::LoadMusic { .. }]));
        assert_eq!(update(&mut system, None, &[]), [MockCall::StopMusic { fade: HALF_A_SECOND }]);
        assert_eq!(update(&mut system, None, &[]), []);
    }

    #[test]
    fn a_score_that_cant_start_isnt_tried_every_frame() {
        let mut system = system();
        system.backend_mut().remove_sound(STEMS[2]);
        system.backend_mut().take_calls();
        assert_eq!(update(&mut system, Some("running"), &[]), []);
        assert_eq!(update(&mut system, Some("running"), &[]), []);
    }

    fn voice_lines(locale: &str, cue: AssetId) -> VoiceLines {
        VoiceLines {
            locale: locale.to_owned(),
            lines: vec![VoiceLine { key: "perfect-dodge".to_owned(), cue }],
        }
    }

    #[test]
    fn a_line_speaks_the_voice_language_and_the_music_goes_down_under_it() {
        let mut system = system();
        system.add_voice_lines(&voice_lines("en", LINE_EN)).unwrap();
        system.add_voice_lines(&voice_lines("ja", LINE_JA)).unwrap();
        assert!(system.set_voice_language("ja"));
        update(&mut system, Some("running"), &[]);

        let dodge = [LineRequest { line: LineKey::of("perfect-dodge"), target: CueTarget::Flat }];
        let calls = update(&mut system, Some("running"), &dodge);
        let started = calls.iter().find_map(|call| match *call {
            MockCall::StartVoice { slot, sound, bus, .. } => Some((slot, sound, bus)),
            _ => None,
        });
        let (slot, sound, bus) = started.expect("the line starts");
        assert_eq!((sound, bus), (LINE_JA, Bus::Voice));
        assert!(calls.contains(&MockCall::SetBusGain { bus: Bus::Music, gain_db: -10.0, fade: DUCK_ATTACK }));
        assert_eq!(update(&mut system, Some("running"), &[]), [], "still speaking, still down");
        system.backend_mut().finish(slot);
        assert_eq!(
            update(&mut system, Some("running"), &[]),
            [MockCall::SetBusGain { bus: Bus::Music, gain_db: 0.0, fade: DUCK_RELEASE }]
        );

        let missing = [LineRequest { line: LineKey::of("not-recorded"), target: CueTarget::Flat }];
        update(&mut system, Some("running"), &missing);
        assert_eq!((system.stats().lines_asked, system.stats().lines_missing), (1, 1));
    }

    #[test]
    fn the_music_follows_the_players_volume_and_ducks_under_it() {
        let mut system = system();
        system.add_voice_lines(&voice_lines("en", LINE_EN)).unwrap();
        assert!(system.set_voice_language("en"));
        update(&mut system, Some("running"), &[]);
        assert_eq!(update(&mut system, Some("running"), &[]), [MockCall::PlayMusic]);

        system.set_volume(0.5);
        let half = 20.0 * 0.5_f32.log10();
        assert_eq!(
            update(&mut system, Some("running"), &[]),
            [MockCall::SetBusGain { bus: Bus::Music, gain_db: half, fade: DUCK_ATTACK }],
            "the music turns down with every voice"
        );
        let dodge = [LineRequest { line: LineKey::of("perfect-dodge"), target: CueTarget::Flat }];
        let calls = update(&mut system, Some("running"), &dodge);
        let slot = calls.iter().find_map(|call| match *call {
            MockCall::StartVoice { slot, .. } => Some(slot),
            _ => None,
        });
        let ducked = MockCall::SetBusGain { bus: Bus::Music, gain_db: half - 10.0, fade: DUCK_ATTACK };
        assert!(calls.contains(&ducked), "{calls:?}");

        // Off is silent, under a line or not, and never past silence.
        system.set_volume(0.0);
        let calls = update(&mut system, Some("running"), &[]);
        let silent = MockCall::SetBusGain { bus: Bus::Music, gain_db: SILENCE_DB, fade: DUCK_ATTACK };
        assert!(calls.contains(&silent), "{calls:?}");
        system.set_volume(1.0);
        system.backend_mut().finish(slot.expect("the line starts"));
        assert_eq!(
            update(&mut system, Some("running"), &[]),
            [MockCall::SetBusGain { bus: Bus::Music, gain_db: 0.0, fade: DUCK_RELEASE }]
        );
    }
}
