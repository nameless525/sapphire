//! The flow document (`content/game.flow.ron`, a [`FlowDocument`]): which scenario a run stands, in which
//! zone, the beats that fire as it goes, the music each of the flow's own moments asks for and how long they
//! hold. A game's flow needs no Rust: the manifest names the document (`GameDocuments::flow`) and the game
//! reads it at its start.
//!
//! [`FlowDocument::check`] holds what the document says by itself and names the field that's wrong. What it
//! says about other content, that its scenario, encounters, timelines, dialogues and flags exist, can only be
//! checked by what can read those, at the game's start: the cooker can't link gameplay.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::{AssetId, DocumentError};
use serde::{Deserialize, Serialize};

use crate::flow::MAX_BEATS;

/// The longest any of the flow's holds may be, in seconds: past this a number is a typo, not a design.
pub const MAX_HOLD_SECONDS: f32 = 60.0;

/// A game's flow, as a document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::game"]
#[serde(deny_unknown_fields)]
pub struct FlowDocument {
    /// The scenario a run stands: its cast, its tables and where they start.
    pub scenario: String,
    /// The zone the scenario stands in, by its document's ID.
    pub zone: AssetId,
    /// What happens as a run goes, checked in order after each step. Each fires at most once a run.
    #[serde(default)]
    pub beats: Vec<Beat>,
    /// The music the flow's own moments ask for, by the score's phase names.
    pub music: FlowMusic,
    /// How long the flow's moments hold, in real seconds.
    pub holds: FlowHolds,
}

impl Versioned for FlowDocument {
    /// 1: M3 G1.
    const VERSION: u32 = 1;
}

/// One thing that happens during a run: when `when` holds, unless the flag `unless` is set, `then`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Beat {
    pub when: When,
    /// A flag that, set to anything but 0, keeps the beat from firing: a choice already made, a scene
    /// already seen.
    #[serde(default)]
    pub unless: Option<String>,
    /// A checkpoint the player is brought to as a timeline starts, so they stand where its shots are
    /// framed. Only a timeline's beat has one.
    #[serde(default)]
    pub align: Option<String>,
    pub then: Then,
}

/// What a beat waits for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum When {
    /// The encounter, by its document's ID, has begun.
    Started(AssetId),
    /// The encounter has been cleared.
    Cleared(AssetId),
    /// The timeline, by its ID, has played to its end or been skipped there.
    Ended(AssetId),
    /// The checkpoint, by its name in the zone, has been reached.
    Reached(String),
}

/// What a beat does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Then {
    /// Plays a timeline, by its ID: a cutscene, which the player may skip.
    Timeline(AssetId),
    /// Opens a dialogue, by its ID: the run waits on it.
    Dialogue(AssetId),
    /// The run is won.
    Victory,
}

/// The score's phases the flow's own moments ask for. Anything else, the fight's music decides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowMusic {
    pub title: String,
    pub victory: String,
    pub defeat: String,
}

/// How long each of the flow's moments holds, in real seconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowHolds {
    /// A banner: a checkpoint reached, the run won or lost.
    pub banner: f32,
    /// From a fall to the restart at the save.
    pub defeat: f32,
    /// From the win to the title.
    pub victory: f32,
}

/// A flow document that's wrong in itself: which field, and why.
#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("`{field}`: {message}")]
    Invalid { field: String, message: String },
}

impl FlowError {
    fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid { field: field.into(), message: message.into() }
    }
}

impl FlowDocument {
    /// Reads a document and [checks](Self::check) it.
    pub fn from_ron(text: &str) -> Result<Self, FlowError> {
        let document: Self = sapphire_serialize::from_ron(text)?;
        document.check()?;
        Ok(document)
    }

    /// Holds the document to what it can say by itself: names that name something, holds a person can sit
    /// through, at most [`MAX_BEATS`] beats, exactly one of which wins, a checkpoint to align to only where a
    /// timeline starts, and no beat waiting on a timeline that none plays.
    pub fn check(&self) -> Result<(), FlowError> {
        named("scenario", &self.scenario)?;
        if self.beats.len() > MAX_BEATS {
            return Err(FlowError::invalid(
                "beats",
                format!("{} beats, past the {MAX_BEATS} a flow may have", self.beats.len()),
            ));
        }
        for (index, beat) in self.beats.iter().enumerate() {
            let field = |name: &str| format!("beats[{index}].{name}");
            if let When::Reached(checkpoint) = &beat.when {
                named(&field("when"), checkpoint)?;
            }
            if let When::Ended(timeline) = beat.when
                && !self.beats.iter().any(|other| other.then == Then::Timeline(timeline))
            {
                return Err(FlowError::invalid(
                    field("when"),
                    format!("no beat plays the timeline {timeline}, so this one would never fire"),
                ));
            }
            if let Some(flag) = &beat.unless {
                named(&field("unless"), flag)?;
            }
            if let Some(checkpoint) = &beat.align {
                named(&field("align"), checkpoint)?;
                if !matches!(beat.then, Then::Timeline(_)) {
                    return Err(FlowError::invalid(
                        field("align"),
                        "only a beat that plays a timeline brings the player to a checkpoint",
                    ));
                }
            }
        }
        match self.beats.iter().filter(|beat| beat.then == Then::Victory).count() {
            1 => {}
            0 => return Err(FlowError::invalid("beats", "no beat wins the run (`then: Victory`)")),
            count => return Err(FlowError::invalid("beats", format!("{count} beats win the run; one does"))),
        }
        named("music.title", &self.music.title)?;
        named("music.victory", &self.music.victory)?;
        named("music.defeat", &self.music.defeat)?;
        for (field, seconds) in [
            ("holds.banner", self.holds.banner),
            ("holds.defeat", self.holds.defeat),
            ("holds.victory", self.holds.victory),
        ] {
            if !(seconds > 0.0 && seconds <= MAX_HOLD_SECONDS) {
                return Err(FlowError::invalid(
                    field,
                    format!("{seconds} seconds: a hold is over 0 and at most {MAX_HOLD_SECONDS}"),
                ));
            }
        }
        Ok(())
    }
}

fn named(field: &str, name: &str) -> Result<(), FlowError> {
    if name.trim().is_empty() {
        return Err(FlowError::invalid(field, "names nothing"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FLOW: &str = include_str!("../test_content/test.flow.ron");

    #[test]
    fn the_test_flow_reads_and_holds() {
        let flow = FlowDocument::from_ron(TEST_FLOW).expect("reads");
        assert_eq!(flow.scenario, "duel");
        assert_eq!(flow.beats.len(), 4);
        assert_eq!(flow.beats[0].align.as_deref(), Some("gate"));
        assert_eq!(flow.holds.defeat, 3.0);
    }

    /// Each mistake is refused by the field it's in, so whoever wrote it knows where to look.
    #[test]
    fn what_a_flow_gets_wrong_is_named_by_field() {
        let cases = [
            ("scenario: \"duel\"", "scenario: \" \"", "`scenario`"),
            ("align: Some(\"gate\")", "align: Some(\"\")", "`beats[0].align`"),
            ("unless: Some(\"gate_answered\")", "unless: Some(\"\")", "`beats[0].unless`"),
            ("when: Reached(\"terrace\")", "when: Reached(\"\")", "`beats[2].when`"),
            ("then: Victory", "then: Dialogue(\"00000000-0000-0000-0000-0000000000d1\")", "no beat wins"),
            ("victory: \"cleared\"", "victory: \"\"", "`music.victory`"),
            ("defeat: 3.0", "defeat: 0.0", "`holds.defeat`"),
            ("banner: 2.0", "banner: 600.0", "`holds.banner`"),
            ("defeat: 3.0", "defeat: NaN", "`holds.defeat`"),
        ];
        for (from, to, named) in cases {
            assert!(TEST_FLOW.contains(from), "the test flow has {from}");
            let refused = FlowDocument::from_ron(&TEST_FLOW.replacen(from, to, 1)).expect_err(to);
            assert!(refused.to_string().contains(named), "{to}: {refused}");
        }
    }

    #[test]
    fn a_checkpoint_to_align_to_belongs_to_a_timelines_beat() {
        let moved = TEST_FLOW.replacen("align: Some(\"gate\"),", "", 1).replacen(
            "then: Dialogue(",
            "align: Some(\"gate\"), then: Dialogue(",
            1,
        );
        let refused = FlowDocument::from_ron(&moved).expect_err("a dialogue's beat can't align");
        assert!(refused.to_string().contains("`beats[1].align`"), "{refused}");
    }

    #[test]
    fn a_beat_waiting_on_a_timeline_no_beat_plays_would_never_fire() {
        let orphan = TEST_FLOW.replacen(
            "when: Ended(\"00000000-0000-0000-0000-0000000000a1\")",
            "when: Ended(\"00000000-0000-0000-0000-0000000000a2\")",
            1,
        );
        let refused = FlowDocument::from_ron(&orphan).expect_err("an orphan");
        assert!(refused.to_string().contains("`beats[1].when`"), "{refused}");
    }

    #[test]
    fn two_wins_and_unknown_fields_are_refused() {
        let twice = TEST_FLOW.replacen(
            "then: Dialogue(\"00000000-0000-0000-0000-0000000000d1\")",
            "then: Victory",
            1,
        );
        let refused = FlowDocument::from_ron(&twice).expect_err("two wins");
        assert!(refused.to_string().contains("2 beats win"), "{refused}");
        let typo = TEST_FLOW.replacen("unless:", "unles:", 1);
        let refused = FlowDocument::from_ron(&typo).expect_err("a typo");
        assert!(refused.to_string().contains("unles"), "{refused}");
    }

    /// The cooker can't link this type, so the manifest holds its name as text, and this holds it to the type.
    #[test]
    fn the_manifest_names_this_type() {
        use sapphire_assets::documents::GameDocuments;
        let manifest = GameDocuments { flow: Some("game.flow.ron".to_owned()), ..GameDocuments::default() };
        let named: Vec<_> = manifest.named().collect();
        assert_eq!(named.len(), 1);
        assert_eq!(
            (named[0].field, named[0].type_name),
            ("flow", sapphire_reflect::stable_name::<FlowDocument>())
        );
    }

    #[test]
    fn too_many_beats_are_refused() {
        let mut flow = FlowDocument::from_ron(TEST_FLOW).expect("reads");
        let filler = flow.beats[2].clone();
        flow.beats.splice(0..0, std::iter::repeat_n(filler, MAX_BEATS));
        let refused = flow.check().expect_err("too many");
        assert!(refused.to_string().contains("`beats`"), "{refused}");
    }
}
