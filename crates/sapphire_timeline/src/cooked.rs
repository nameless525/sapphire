//! The cooked timeline (`.stimeline`): an rkyv archive, which the runtime copies out once at load.
//!
//! Two halves, because they live by different rules. [`TimelineSimulation`] is the simulation's, loaded
//! between steps and read inside them, and a replay pins a digest of it
//! ([`TimelineSimulation::digest`]). [`TimelinePresentation`] is everything else, which only the sequencer
//! reads and which can change with no replay moving.
//!
//! No upgrades, like every cooked format: a changed layout takes a version bump and recooks from its
//! document.
//!
//! [`Timeline::read`] takes bytes that came from a pack, which needn't have come from our cooker: bytecheck
//! rules out a bad archive, and [`Timeline::validate`] everything the runtime would otherwise have to
//! trust, by the same checks a document gets, so a field a document can't hold a pack can't either.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::AlignedVec;
use xxhash_rust::xxh3::Xxh3;

use crate::document::{
    CameraTrack, MAX_LENGTH, MAX_PER_MILLE, PlaceKey, PostTrack, SubtitleKey, TimelineClock, TimelineError,
    check_camera, check_count, check_places, check_position, check_post, check_subtitles, check_times, tick_of,
};

/// Conventional file extension for cooked timelines.
pub const TIMELINE_EXTENSION: &str = "stimeline";

/// The most beats a timeline's simulation half may hold. A beat is a few nanoseconds on the step it falls
/// on, and a ramp takes one a step, so this is over a minute of nothing but ramps.
pub const MAX_BEATS: usize = 4096;

/// A cooked timeline. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::timeline"]
pub struct Timeline {
    pub simulation: TimelineSimulation,
    pub presentation: TimelinePresentation,
}

impl Versioned for Timeline {
    /// 1: M3 Q1.
    const VERSION: u32 = 1;
}

/// All the simulation reads of a timeline. Everything here is in a replay's digest.
#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TimelineSimulation {
    /// How many steps it holds input for: 0 for [`TimelineClock::Real`], which takes none.
    pub ticks: u32,
    /// By ascending tick, each before `ticks`, or at 0 if there are no ticks: then all of them fire on the
    /// first step after it.
    pub beats: Vec<Beat>,
}

/// Something that happens to the simulation at a step of a timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Beat {
    /// The step, counting from 0: the first step after the timeline starts.
    pub tick: u32,
    pub what: BeatKind,
}

/// What a [`Beat`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum BeatKind {
    /// The world clock runs at this speed from the next step on, under `ScaleSource::Cinematic`.
    Scale { per_mille: u32 },
    /// Gameplay is told this, by the name's hash.
    Event { event: u64, value: i32 },
}

/// What only the sequencer reads of a timeline.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TimelinePresentation {
    /// In seconds.
    pub length: f32,
    pub clock: TimelineClock,
    pub camera: Option<CameraTrack>,
    pub characters: Vec<CookedCharacter>,
    pub cues: Vec<CookedCue>,
    pub post: Vec<PostTrack>,
    pub subtitles: Vec<SubtitleKey>,
}

/// A character's part, by the hashes of its names.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedCharacter {
    pub role: u64,
    pub plays: Vec<CookedPlay>,
    pub places: Vec<PlaceKey>,
}

/// A row of a graph, played from its start.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedPlay {
    pub row: u64,
    pub from: f32,
    pub to: f32,
}

/// A row of the cue table, by its name's hash.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct CookedCue {
    pub at: f32,
    pub cue: u64,
    pub place: [f32; 3],
}

impl Timeline {
    pub fn to_cooked(&self) -> Result<AlignedVec, TimelineError> {
        Ok(sapphire_serialize::to_cooked(self)?)
    }

    /// Reads cooked bytes, checking the envelope, the archive and every value the runtime reads
    /// ([`Self::validate`]). What it allocates, the bytes paid for.
    pub fn read(bytes: &[u8]) -> Result<Self, TimelineError> {
        let archived = sapphire_serialize::access::<Self>(bytes)?;
        let timeline = rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .expect("an archive bytecheck accepted deserializes: it holds only plain values and strings");
        timeline.validate()?;
        Ok(timeline)
    }

    /// Everything the runtime relies on without looking, for bytes that may be anyone's.
    pub fn validate(&self) -> Result<(), TimelineError> {
        let (simulation, presentation) = (&self.simulation, &self.presentation);
        let length = presentation.length;
        if !(length > 0.0 && length <= MAX_LENGTH) {
            return Err(TimelineError::invalid("length", format!("{length} is outside 0 to {MAX_LENGTH}")));
        }
        let ticks = match presentation.clock {
            TimelineClock::Real => 0,
            TimelineClock::Steps => tick_of(length),
        };
        if simulation.ticks != ticks || (presentation.clock == TimelineClock::Steps && ticks == 0) {
            let problem = format!("{} steps, for a timeline of {length} seconds", simulation.ticks);
            return Err(TimelineError::invalid("ticks", problem));
        }
        simulation.validate()?;
        if let Some(camera) = &presentation.camera {
            check_camera(camera, length)?;
        }
        check_count("characters", presentation.characters.len())?;
        for (index, character) in presentation.characters.iter().enumerate() {
            let plays = format!("characters[{index}].plays");
            check_count(&plays, character.plays.len())?;
            let mut last = None;
            for (at, play) in character.plays.iter().enumerate() {
                let bad = !(play.from >= 0.0 && play.to > play.from && play.to <= length)
                    || last.is_some_and(|last| play.from < last);
                if bad {
                    let problem = format!("{} to {} isn't in order within {length}", play.from, play.to);
                    return Err(TimelineError::invalid(format!("{plays}[{at}]"), problem));
                }
                last = Some(play.to);
            }
            check_places(&format!("characters[{index}].places"), &character.places, length)?;
        }
        check_count("cues", presentation.cues.len())?;
        check_times("cues", presentation.cues.iter().map(|cue| cue.at), length, true)?;
        for (index, cue) in presentation.cues.iter().enumerate() {
            check_position(|| format!("cues[{index}].place"), cue.place)?;
        }
        check_post(&presentation.post, length)?;
        check_subtitles(&presentation.subtitles, length)
    }
}

impl TimelineSimulation {
    /// What the simulation relies on without looking.
    pub fn validate(&self) -> Result<(), TimelineError> {
        if self.beats.len() > MAX_BEATS {
            let problem = format!("{} beats, past the limit of {MAX_BEATS}", self.beats.len());
            return Err(TimelineError::invalid("beats", problem));
        }
        let mut last = 0;
        for (index, beat) in self.beats.iter().enumerate() {
            if beat.tick < last || beat.tick >= self.ticks.max(1) {
                let problem = format!("step {} isn't in order within {} steps", beat.tick, self.ticks);
                return Err(TimelineError::invalid(format!("beats[{index}].tick"), problem));
            }
            last = beat.tick;
            if let BeatKind::Scale { per_mille } = beat.what {
                if self.ticks == 0 {
                    let problem = "a timeline of no steps has no step for a time scale to run under";
                    return Err(TimelineError::invalid(format!("beats[{index}]"), problem));
                }
                if per_mille > MAX_PER_MILLE {
                    let problem = format!("{per_mille} is past the limit of {MAX_PER_MILLE}");
                    return Err(TimelineError::invalid(format!("beats[{index}]"), problem));
                }
            }
        }
        Ok(())
    }

    /// The hash of everything a run depends on: what a replay pins, in `tables_hash`, so a change here
    /// reads as changed tables rather than as a broken simulation. It hashes values, not an archive's
    /// layout, and they're all whole numbers.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hasher = Xxh3::new();
        hasher.update(&self.ticks.to_le_bytes());
        hasher.update(&(self.beats.len() as u64).to_le_bytes());
        for beat in &self.beats {
            hasher.update(&beat.tick.to_le_bytes());
            match beat.what {
                BeatKind::Scale { per_mille } => {
                    hasher.update(&[0]);
                    hasher.update(&per_mille.to_le_bytes());
                }
                BeatKind::Event { event, value } => {
                    hasher.update(&[1]);
                    hasher.update(&event.to_le_bytes());
                    hasher.update(&value.to_le_bytes());
                }
            }
        }
        hasher.digest()
    }
}

#[cfg(test)]
mod tests {
    use sapphire_serialize::CookedError;

    use super::*;
    use crate::document::TimelineDocument;

    const CINEMATIC: &str = include_str!("../test_content/cinematic.timeline.ron");
    const CUTSCENE: &str = include_str!("../test_content/cutscene.timeline.ron");

    fn cook(text: &str) -> Timeline {
        TimelineDocument::from_ron(text).expect("the document reads").cook().expect("it cooks")
    }

    /// The test document with `from` swapped for `to`, which must be there.
    fn with(text: &str, from: &str, to: &str) -> Result<TimelineDocument, TimelineError> {
        assert!(text.contains(from), "{from:?} isn't in the document");
        TimelineDocument::from_ron(&text.replacen(from, to, 1))
    }

    fn field_of(error: TimelineError) -> String {
        match error {
            TimelineError::Invalid { field, .. } => field,
            other => panic!("expected a named field, got {other}"),
        }
    }

    fn is_event(beat: &Beat) -> bool {
        matches!(beat.what, BeatKind::Event { .. })
    }

    #[test]
    fn a_cook_is_repeatable_and_reads_back() {
        for text in [CINEMATIC, CUTSCENE] {
            let (one, two) = (cook(text).to_cooked().unwrap(), cook(text).to_cooked().unwrap());
            assert_eq!(*one, *two);
            assert_eq!(Timeline::read(&one).expect("what a cook writes reads"), cook(text));
        }
    }

    #[test]
    fn a_cutscenes_simulation_half_is_no_steps_long_with_its_events_on_the_first() {
        let timeline = cook(CUTSCENE);
        assert_eq!(timeline.simulation.ticks, 0);
        assert!(!timeline.simulation.beats.is_empty());
        assert!(timeline.simulation.beats.iter().all(|beat| beat.tick == 0 && is_event(beat)));
    }

    #[test]
    fn beats_fall_on_whole_steps_with_a_steps_time_scale_first() {
        let timeline = cook(CINEMATIC);
        assert_eq!(timeline.simulation.ticks, 150);
        let ticks: Vec<u32> = timeline.simulation.beats.iter().map(|beat| beat.tick).collect();
        assert!(ticks.is_sorted(), "{ticks:?}");
        let at_45: Vec<&Beat> = timeline.simulation.beats.iter().filter(|beat| beat.tick == 45).collect();
        assert!(matches!(at_45[0].what, BeatKind::Scale { per_mille: 300 }), "{at_45:?}");
        assert!(is_event(at_45[1]), "{at_45:?}");
    }

    #[test]
    fn a_ramp_is_a_beat_a_step_in_whole_numbers() {
        let timeline = cook(CINEMATIC);
        let ramp: Vec<(u32, u32)> = timeline
            .simulation
            .beats
            .iter()
            .filter_map(|beat| match beat.what {
                BeatKind::Scale { per_mille } if beat.tick > 45 => Some((beat.tick, per_mille)),
                _ => None,
            })
            .collect();
        // From 300 back to normal over 6 steps, on the steps after the key.
        assert_eq!(ramp, [(91, 416), (92, 533), (93, 650), (94, 766), (95, 883), (96, 1000)]);
    }

    #[test]
    fn the_digest_is_pinned_and_moves_with_every_field() {
        let simulation = cook(CINEMATIC).simulation;
        // One value on every architecture: all it hashes is whole numbers, from decimal text.
        assert_eq!(simulation.digest(), PINNED_DIGEST, "a change here moves every timeline's replays");

        let moved = |change: fn(&mut TimelineSimulation)| {
            let mut changed = simulation.clone();
            change(&mut changed);
            assert_ne!(changed.digest(), simulation.digest());
        };
        moved(|sim| sim.ticks += 1);
        moved(|sim| sim.beats[0].tick += 1);
        moved(|sim| {
            sim.beats.pop();
        });
        moved(|sim| {
            let scale = sim.beats.iter_mut().find(|beat| !is_event(beat)).expect("a scale");
            scale.what = BeatKind::Scale { per_mille: 301 };
        });
        moved(|sim| {
            let beat = sim.beats.iter_mut().find(|beat| is_event(beat)).expect("an event");
            let BeatKind::Event { value, .. } = &mut beat.what else { unreachable!() };
            *value += 1;
        });
        moved(|sim| {
            let beat = sim.beats.iter_mut().find(|beat| is_event(beat)).expect("an event");
            let BeatKind::Event { event, .. } = &mut beat.what else { unreachable!() };
            *event ^= 1;
        });
    }

    #[test]
    fn presentation_stays_out_of_the_digest() {
        let digest = cook(CINEMATIC).simulation.digest();
        let moved = with(CINEMATIC, "fov_degrees: 35.0", "fov_degrees: 36.0").unwrap();
        assert_eq!(moved.cook().unwrap().simulation.digest(), digest);
    }

    /// Each row: what to change in a test document, and the field the refusal must name.
    #[test]
    fn checks_name_the_field() {
        let cases: &[(&str, &str, &str, &str)] = &[
            (CINEMATIC, "length: 2.5", "length: 0.0", "length"),
            (CINEMATIC, "length: 2.5", "length: 0.001", "length"),
            (CINEMATIC, "length: 2.5", "length: 90000.0", "length"),
            (CINEMATIC, "blend_in: 0.15", "blend_in: -1.0", "camera.blend_in"),
            (CINEMATIC, "blend_out: 0.2", "blend_out: 1000.0", "camera.blend_out"),
            (CINEMATIC, "at: 2.0, eye", "at: 0.5, eye", "camera.keys[2].at"),
            (CINEMATIC, "at: 2.0, eye", "at: 9.0, eye", "camera.keys[2].at"),
            (CINEMATIC, "fov_degrees: 35.0", "fov_degrees: 0.5", "camera.keys[0].fov_degrees"),
            (CINEMATIC, "eye: (0.0, 1.6, -4.0)", "eye: (9e9, 1.6, -4.0)", "camera.keys[0].eye"),
            (CINEMATIC, "eye: (0.0, 1.6, -4.0)", "eye: (0.0, 1.4, 0.0)", "camera.keys[0].look_at"),
            (CINEMATIC, "per_mille: 300", "per_mille: 20000", "time_scale[0].per_mille"),
            (CINEMATIC, "at: 1.5, per_mille", "at: 0.5, per_mille", "time_scale[1].at"),
            (CINEMATIC, "at: 1.5, per_mille", "at: 2.45, per_mille", "time_scale[1].at"),
            (CINEMATIC, "ramp: 0.1", "ramp: -0.1", "time_scale[1].ramp"),
            (CINEMATIC, "at: 2.0, event", "at: 2.5, event", "events[2].at"),
            (CINEMATIC, "at: 2.0, event", "at: 0.5, event", "events[2].at"),
            (CINEMATIC, "event: \"guard_up\"", "event: \"\"", "events[0].event"),
            (CUTSCENE, "key: \"intro-first-line\"", "key: \"Hello, world!\"", "subtitles[0].key"),
            (CUTSCENE, "speaker: Some(\"hero-name\")", "speaker: Some(\"\")", "subtitles[0].speaker"),
            (CUTSCENE, "from: 3.0, to: 5.0", "from: 2.0, to: 5.0", "subtitles[1].from"),
            (CUTSCENE, "from: 3.0, to: 5.0", "from: 3.0, to: 3.0", "subtitles[1].to"),
            (CUTSCENE, "role: \"villain\"", "role: \"hero\"", "characters[1].role"),
            (CUTSCENE, "row: \"bow\", from: 0.5, to: 2.0", "row: \"bow\", from: 2.0, to: 1.0", "characters[0].plays[0].to"),
            (CUTSCENE, "row: \"wave\", from: 2.5", "row: \"wave\", from: 1.5", "characters[0].plays[1].from"),
            (CUTSCENE, "yaw_degrees: 180.0", "yaw_degrees: inf", "characters[0].places[0].yaw_degrees"),
            (CUTSCENE, "(at: 5.0, value: 0.0)", "(at: 5.0, value: 1.0)", "post[0].keys[2].value"),
            (CUTSCENE, "parameter: Vignette", "parameter: Exposure", "post[1].parameter"),
            (CUTSCENE, "at: 1.0, cue", "at: 9.0, cue", "cues[0].at"),
            (CUTSCENE, "cue: \"bell\"", "cue: \"\"", "cues[0].cue"),
        ];
        for &(text, from, to, field) in cases {
            let error = with(text, from, to).expect_err(to);
            assert_eq!(field_of(error), field, "{from} → {to}");
        }
    }

    #[test]
    fn a_held_timeline_can_have_no_time_scale() {
        let text = CUTSCENE.replacen("time_scale: []", "time_scale: [(at: 0.0, per_mille: 500)]", 1);
        assert_eq!(field_of(TimelineDocument::from_ron(&text).unwrap_err()), "time_scale");
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let error = with(CINEMATIC, "length: 2.5", "length: 2.5, lenght: 2.5").unwrap_err();
        assert!(matches!(error, TimelineError::Document(_)), "{error}");
    }

    #[test]
    fn hostile_bytes_are_refused_never_panicking() {
        let bytes = cook(CINEMATIC).to_cooked().unwrap();
        for length in 0..bytes.len() {
            let mut truncated = AlignedVec::<16>::new();
            truncated.extend_from_slice(&bytes[..length]);
            assert!(Timeline::read(&truncated).is_err(), "cut to {length} bytes");
        }
        // Every byte, turned over: either it's refused, or what it says passed the checks.
        for at in 0..bytes.len() {
            let mut flipped = AlignedVec::<16>::new();
            flipped.extend_from_slice(&bytes);
            flipped[at] ^= 0xff;
            if let Ok(timeline) = Timeline::read(&flipped) {
                timeline.validate().expect("read holds it to the rules");
            }
        }
    }

    #[test]
    fn another_kinds_bytes_are_refused() {
        #[derive(TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
        #[type_path = "sapphire::test"]
        struct Other {
            value: u32,
        }
        impl Versioned for Other {
            const VERSION: u32 = 1;
        }
        let other = sapphire_serialize::to_cooked(&Other { value: 7 }).unwrap();
        assert!(matches!(Timeline::read(&other), Err(TimelineError::Cooked(CookedError::Header(_)))));
    }

    /// Each row: a thing a document can't say, which bytes might, and the field it's refused by.
    #[test]
    fn values_a_document_cant_hold_a_pack_cant_either() {
        let good = cook(CINEMATIC);
        let cases: &[(&str, fn(&mut Timeline))] = &[
            ("length", |t| t.presentation.length = f32::NAN),
            ("length", |t| t.presentation.length = f32::INFINITY),
            ("ticks", |t| t.simulation.ticks = 1 << 30),
            ("beats[1].tick", |t| t.simulation.beats[1].tick = 0),
            ("beats[0].tick", |t| t.simulation.beats[0].tick = 1 << 20),
            ("beats[0]", |t| t.simulation.beats[0].what = BeatKind::Scale { per_mille: u32::MAX }),
            ("camera.keys[0].fov_degrees", |t| {
                t.presentation.camera.as_mut().unwrap().keys[0].fov_degrees = f32::NAN;
            }),
            ("camera.keys[1].at", |t| t.presentation.camera.as_mut().unwrap().keys[1].at = 0.0),
            ("camera.keys", |t| t.presentation.camera.as_mut().unwrap().keys.clear()),
        ];
        for &(field, tamper) in cases {
            let mut tampered = good.clone();
            tamper(&mut tampered);
            let bytes = tampered.to_cooked().unwrap();
            assert_eq!(field_of(Timeline::read(&bytes).unwrap_err()), field);
        }
    }

    /// A timeline of no steps with a scale in it, which no document makes.
    #[test]
    fn a_time_scale_in_no_steps_is_refused() {
        let mut timeline = cook(CUTSCENE);
        timeline.simulation.beats[0].what = BeatKind::Scale { per_mille: 500 };
        let bytes = timeline.to_cooked().unwrap();
        assert_eq!(field_of(Timeline::read(&bytes).unwrap_err()), "beats[0]");
    }

    /// What the tests of a skip lean on: if these documents lost a track, those tests would hold nothing.
    #[test]
    fn the_test_documents_hold_every_track() {
        let cutscene = TimelineDocument::from_ron(CUTSCENE).unwrap();
        assert!(cutscene.camera.is_some());
        assert!(!cutscene.characters.is_empty() && !cutscene.cues.is_empty());
        assert!(!cutscene.post.is_empty() && !cutscene.subtitles.is_empty() && !cutscene.events.is_empty());
        let cinematic = TimelineDocument::from_ron(CINEMATIC).unwrap();
        assert!(!cinematic.time_scale.is_empty() && !cinematic.events.is_empty());
    }

    const PINNED_DIGEST: u64 = 2869729819227249548;
}
