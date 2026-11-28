//! Version 1 of `.animgraph.ron` (M2 N3): a locomotion state machine, a combat table, additive layers,
//! reactions and a blink, as the shipped graphs said until M6 A5c moved them to version 2. Nothing writes
//! it, cooks it or binds it any more. It's here so an old document isn't lost: [`v2::AnimGraph::from_document`]
//! reads one through [`AnimGraph::from_document`], and so through its own check, and
//! [`convert`](super::v2::convert) turns it into version 2, saying in the document what v1's runtime did by
//! name, by asset ID or by a condition's variant.
//!
//! [`v2::AnimGraph::from_document`]: super::v2::AnimGraph::from_document

use std::collections::BTreeSet;

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use super::v2::compile::named;
use super::{
    BlendSample, Blink, EntrySample, GraphClip, GraphError, GraphIssue, GraphMask, MAX_ADDITIVE_LAYERS,
    MAX_BLEND_SAMPLES, MAX_DECAY_TICKS, MAX_GRAPH_CLIPS, MAX_GRAPH_MOVES, MAX_MASK_BONES, MAX_MASKS,
    MAX_REACTIONS,
};

/// The most locomotion states.
const MAX_GRAPH_STATES: usize = 16;
/// The most locomotion transitions.
const MAX_GRAPH_TRANSITIONS: usize = 64;

/// A version 1 graph document. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct AnimGraph {
    /// Every clip it plays.
    pub clips: Vec<GraphClip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<GraphMask>,
    pub locomotion: LocomotionLayer,
    #[serde(default)]
    pub combat: CombatLayer,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additive: Vec<AdditiveLayer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionLayer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blink: Option<Blink>,
}

impl Versioned for AnimGraph {
    /// 1: M2 N3.
    const VERSION: u32 = 1;
}

/// The base layer: a state machine over what a character's feet are doing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocomotionLayer {
    /// The state it starts in.
    pub start: String,
    /// How quickly the blend space's position follows the character's velocity: a critically damped
    /// spring's half-life, in ticks. 0 follows it exactly.
    #[serde(default = "default_smoothing")]
    pub smoothing: f32,
    pub states: Vec<LocomotionState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<LocomotionTransition>,
}

fn default_smoothing() -> f32 {
    3.0
}

/// A state of the locomotion layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocomotionState {
    pub name: String,
    pub plays: StateMotion,
}

/// What a state plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StateMotion {
    /// One clip from its start: holding its last pose, or looping.
    Clip {
        clip: String,
        #[serde(default)]
        looping: bool,
    },
    /// Clips blended by the character's velocity.
    Blend(BlendSpace),
    /// Finite clips blended by fixed takeoff speed. Playback uses elapsed ticks, not gait phase.
    TakeoffBlend(EntryBlend),
    /// Finite touchdown clips. The first band uses takeoff speed to carry the airborne pose
    /// into contact; the optional second band uses forward touchdown speed.
    LandingBlend(EntryBlend),
    /// Takeoff held through ascent, with smooth actual-speed preparation during descent.
    TakeoffAnticipating(EntryBlend),
}

/// A standing clip and one or two progressively faster one-shot clips.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryBlend {
    pub standing: String,
    pub moving: Vec<EntrySample>,
}

/// Clips placed by the velocity they play at. Gradient-band weights, so the clips that weigh in stay
/// few, every sample is exact at its place, and weights change smoothly between.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlendSpace {
    pub samples: Vec<BlendSample>,
    /// The least and most the synced clips' playback may scale by to match the feet to the speed.
    #[serde(default = "default_rate")]
    pub rate: [f32; 2],
}

fn default_rate() -> [f32; 2] {
    [0.5, 2.0]
}

/// What moves the locomotion layer from one state to another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocomotionTransition {
    pub from: String,
    pub to: String,
    pub when: Condition,
    /// Ticks the old pose's offset and velocity take to die away.
    pub decay: u32,
}

/// When a [`LocomotionTransition`] fires.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    /// The character has left the ground.
    Airborne,
    /// The character is on the ground.
    Grounded,
    /// The character has just landed.
    Landed,
    /// The state's clip has played to its end. A blend or a loop never finishes.
    Finished,
    /// Speed, smoothed, is at least this many m/s.
    Moving(f32),
    /// Speed, smoothed, is below this many m/s.
    Stopped(f32),
    /// Touchdown with at least this much forward speed, in m/s in the character's axes.
    LandedMoving(f32),
    /// The last pose of the state's clip has been drawn. The next blend starts at this phase.
    FinishedAtPhase(f32),
    /// Smoothed forward speed, in m/s in the character's axes, is below this value.
    ForwardBelow(f32),
    /// Airborne while travelling at least this fast, with the outgoing stride carried into ascent.
    AirborneMoving(f32),
    /// Whether a supported standing jump is preparing before the impulse.
    JumpPreparation(bool),
}

/// The layer the simulation's moves drive.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CombatLayer {
    /// Ticks a move's start takes to decay the pose before it, cancels included.
    pub decay: u32,
    /// Ticks locomotion takes to decay the pose a move ended on.
    pub exit: u32,
    pub moves: Vec<MoveClip>,
}

/// What plays for a move.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveClip {
    /// The move's name in its move set.
    #[serde(rename = "move")]
    pub name: String,
    pub clip: String,
    /// The joints it takes, for an upper-body override over locomotion. Every joint without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    /// This move's own decay into it, for one that wants to read quicker or softer than the layer's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay: Option<u32>,
}

/// A clip added over everything below it, always: the difference between it and its own first tick,
/// looping on the owner's clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdditiveLayer {
    pub name: String,
    pub clip: String,
    /// Every joint without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    /// 0 to 1.
    pub weight: f32,
}

/// A clip added once when the game asks: the difference from its own first tick, so it starts with
/// nothing to fade in, and fading out over its last ticks, so it ends with nothing left.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReactionLayer {
    pub name: String,
    pub clip: String,
    /// Every joint without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    /// 0 to 1.
    pub weight: f32,
    /// Ticks at its end over which its weight falls to nothing.
    #[serde(default)]
    pub fade_out: u32,
    /// Ticks the pose takes to decay if it starts again while it's still playing.
    #[serde(default = "default_restart")]
    pub restart: u32,
}

fn default_restart() -> u32 {
    4
}

/// Everything [`AnimGraph::check`] has found so far.
#[derive(Default)]
struct Check {
    issues: Vec<GraphIssue>,
}

impl Check {
    fn issue(&mut self, section: &'static str, name: Option<&str>, message: String) {
        let path = name.map_or_else(|| section.to_owned(), |name| named(section, name));
        self.issues.push(GraphIssue { path, message });
    }

    /// `names` have no repeats and none is empty.
    fn names<'a>(
        &mut self,
        section: &'static str,
        names: impl Iterator<Item = &'a str>,
    ) -> BTreeSet<&'a str> {
        let mut seen = BTreeSet::new();
        for name in names {
            if name.is_empty() {
                self.issue(section, None, "an empty name".to_owned());
            } else if !seen.insert(name) {
                self.issue(section, Some(name), "this name is used twice".to_owned());
            }
        }
        seen
    }

    /// `count` is within `limit`.
    fn limit(&mut self, section: &'static str, count: usize, limit: usize) {
        if count > limit {
            self.issue(section, None, format!("{count} of them, past the limit of {limit}"));
        }
    }

    /// `value` is a weight, 0 to 1.
    fn weight(&mut self, section: &'static str, name: &str, value: f32) {
        if !(0.0..=1.0).contains(&value) {
            self.issue(section, Some(name), format!("a weight of {value} isn't 0 to 1"));
        }
    }

    /// `ticks` is a decay this engine runs.
    fn decay(&mut self, section: &'static str, name: &str, ticks: u32) {
        if ticks > MAX_DECAY_TICKS {
            self.issue(section, Some(name), format!("a decay of {ticks} ticks, past {MAX_DECAY_TICKS}"));
        }
    }
}

impl AnimGraph {
    /// Reads and checks a graph document, naming everything wrong with it.
    pub fn from_document(text: &str) -> Result<Self, GraphError> {
        let graph = sapphire_serialize::from_ron::<Self>(text).map_err(|error| match error {
            DocumentError::Parse(spanned) => {
                let line = spanned.span.start.line;
                let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
                GraphError::Malformed {
                    line,
                    column: spanned.span.start.col,
                    message: spanned.code.to_string(),
                    text: quoted.to_owned(),
                }
            }
            other => GraphError::Document(Box::new(other)),
        })?;
        let issues = graph.check();
        if issues.is_empty() { Ok(graph) } else { Err(GraphError::Invalid { issues }) }
    }

    /// Everything wrong with this graph that doesn't depend on a character, in document order.
    #[must_use]
    pub fn check(&self) -> Vec<GraphIssue> {
        let mut check = Check::default();
        check.limit("clips", self.clips.len(), MAX_GRAPH_CLIPS);
        let clips = check.names("clips", self.clips.iter().map(|clip| clip.name.as_str()));
        check.limit("masks", self.masks.len(), MAX_MASKS);
        let masks = check.names("masks", self.masks.iter().map(|mask| mask.name.as_str()));
        for mask in &self.masks {
            check.limit("masks", mask.bones.len(), MAX_MASK_BONES);
            for bone in &mask.bones {
                check.weight("masks", &mask.name, bone.weight);
            }
        }
        self.check_locomotion(&mut check, &clips);
        self.check_combat(&mut check, &clips, &masks);
        self.check_additive(&mut check, &clips, &masks);
        if let Some(blink) = &self.blink {
            check_blink(&mut check, blink);
        }
        check.issues
    }

    fn check_locomotion(&self, check: &mut Check, clips: &BTreeSet<&str>) {
        let layer = &self.locomotion;
        check.limit("locomotion.states", layer.states.len(), MAX_GRAPH_STATES);
        let states = check.names("locomotion.states", layer.states.iter().map(|state| state.name.as_str()));
        if !states.contains(layer.start.as_str()) {
            check.issue("locomotion.start", Some(&layer.start), "no such state".to_owned());
        }
        if !(layer.smoothing.is_finite() && layer.smoothing >= 0.0) {
            check.issue("locomotion.smoothing", None, format!("{} isn't a half-life", layer.smoothing));
        }
        for state in &layer.states {
            match &state.plays {
                StateMotion::Clip { clip, .. } => {
                    if !clips.contains(clip.as_str()) {
                        let message = format!("plays `{clip}`, which `clips` doesn't name");
                        check.issue("locomotion.states", Some(&state.name), message);
                    }
                }
                StateMotion::Blend(space) => check_space(check, &state.name, space, clips),
                StateMotion::TakeoffBlend(blend)
                | StateMotion::LandingBlend(blend)
                | StateMotion::TakeoffAnticipating(blend) => {
                    let max = if matches!(state.plays, StateMotion::LandingBlend(_)) { 3 } else { 1 };
                    let section = "locomotion.states";
                    if !(1..=max).contains(&blend.moving.len()) {
                        check.issue(section, Some(&state.name), format!("expects one to {max} moving clips"));
                    }
                    for clip in [&blend.standing].into_iter().chain(blend.moving.iter().map(|s| &s.clip)) {
                        if !clips.contains(clip.as_str()) {
                            check.issue(
                                section,
                                Some(&state.name),
                                format!("plays `{clip}`, which `clips` doesn't name"),
                            );
                        }
                    }
                    for (index, sample) in blend.moving.iter().enumerate() {
                        if (index == 2) != sample.short_lead.is_some()
                            || sample.short_lead.is_some_and(|[low, high]| {
                                !(low.is_finite() && high.is_finite() && low >= 0.0 && high > low)
                            })
                        {
                            check.issue(
                                section,
                                Some(&state.name),
                                "third landing sample needs an increasing short_lead range".into(),
                            );
                        }
                        let [low, high] = sample.fade;
                        if !(low.is_finite() && high.is_finite() && low >= 0.0 && high > low) {
                            check.issue(
                                section,
                                Some(&state.name),
                                format!("fade {:?} isn't an increasing speed range", sample.fade),
                            );
                        }
                    }
                }
            }
        }
        check.limit("locomotion.transitions", layer.transitions.len(), MAX_GRAPH_TRANSITIONS);
        for transition in &layer.transitions {
            let section = "locomotion.transitions";
            for end in [&transition.from, &transition.to] {
                if !states.contains(end.as_str()) {
                    check.issue(section, Some(end), "no such state".to_owned());
                }
            }
            check.decay(section, &transition.from, transition.decay);
            if let Condition::Moving(speed)
            | Condition::Stopped(speed)
            | Condition::LandedMoving(speed)
            | Condition::ForwardBelow(speed)
            | Condition::AirborneMoving(speed) = transition.when
                && !(speed.is_finite() && speed >= 0.0)
            {
                check.issue(section, Some(&transition.from), format!("{speed} isn't a speed"));
            }
            if let Condition::FinishedAtPhase(phase) = transition.when
                && !(0.0..1.0).contains(&phase)
            {
                check.issue(section, Some(&transition.from), format!("{phase} isn't a gait phase"));
            }
        }
    }

    fn check_combat(&self, check: &mut Check, clips: &BTreeSet<&str>, masks: &BTreeSet<&str>) {
        let layer = &self.combat;
        check.decay("combat.decay", "the layer", layer.decay);
        check.decay("combat.exit", "the layer", layer.exit);
        check.limit("combat.moves", layer.moves.len(), MAX_GRAPH_MOVES);
        check.names("combat.moves", layer.moves.iter().map(|entry| entry.name.as_str()));
        for entry in &layer.moves {
            let section = "combat.moves";
            if !clips.contains(entry.clip.as_str()) {
                check.issue(
                    section,
                    Some(&entry.name),
                    format!("plays `{}`, which `clips` doesn't name", entry.clip),
                );
            }
            check_mask_name(check, section, &entry.name, entry.mask.as_deref(), masks);
            if let Some(decay) = entry.decay {
                check.decay(section, &entry.name, decay);
            }
        }
    }

    fn check_additive(&self, check: &mut Check, clips: &BTreeSet<&str>, masks: &BTreeSet<&str>) {
        check.limit("additive", self.additive.len(), MAX_ADDITIVE_LAYERS);
        check.names("additive", self.additive.iter().map(|layer| layer.name.as_str()));
        for layer in &self.additive {
            if !clips.contains(layer.clip.as_str()) {
                let message = format!("plays `{}`, which `clips` doesn't name", layer.clip);
                check.issue("additive", Some(&layer.name), message);
            }
            check_mask_name(check, "additive", &layer.name, layer.mask.as_deref(), masks);
            check.weight("additive", &layer.name, layer.weight);
        }
        check.limit("reactions", self.reactions.len(), MAX_REACTIONS);
        check.names("reactions", self.reactions.iter().map(|layer| layer.name.as_str()));
        for layer in &self.reactions {
            if !clips.contains(layer.clip.as_str()) {
                let message = format!("plays `{}`, which `clips` doesn't name", layer.clip);
                check.issue("reactions", Some(&layer.name), message);
            }
            check_mask_name(check, "reactions", &layer.name, layer.mask.as_deref(), masks);
            check.weight("reactions", &layer.name, layer.weight);
            check.decay("reactions", &layer.name, layer.restart);
            check.decay("reactions", &layer.name, layer.fade_out);
        }
    }
}

fn check_space(check: &mut Check, name: &str, space: &BlendSpace, clips: &BTreeSet<&str>) {
    let section = "locomotion.states";
    check.limit(section, space.samples.len(), MAX_BLEND_SAMPLES);
    if space.samples.is_empty() {
        check.issue(section, Some(name), "a blend of no samples".to_owned());
    }
    let [low, high] = space.rate;
    if !(low.is_finite() && high.is_finite() && 0.0 < low && low <= high) {
        check.issue(section, Some(name), format!("a rate of ({low}, {high}) isn't a range, least to most"));
    }
    for sample in &space.samples {
        if !clips.contains(sample.clip.as_str()) {
            check.issue(section, Some(name), format!("plays `{}`, which `clips` doesn't name", sample.clip));
        }
        if !sample.at.iter().all(|value| value.is_finite()) {
            check.issue(section, Some(name), format!("`{}` sits at {:?}", sample.clip, sample.at));
        }
        if !(0.0..=1.0).contains(&sample.phase) {
            let message = format!("`{}` has a phase of {}, which isn't 0 to 1", sample.clip, sample.phase);
            check.issue(section, Some(name), message);
        }
    }
    for (index, sample) in space.samples.iter().enumerate() {
        if space.samples[..index].iter().any(|other| other.at == sample.at) {
            let message = format!("two samples sit at {:?}, so neither can be exact", sample.at);
            check.issue(section, Some(name), message);
        }
    }
}

fn check_mask_name(
    check: &mut Check,
    section: &'static str,
    name: &str,
    mask: Option<&str>,
    masks: &BTreeSet<&str>,
) {
    if let Some(mask) = mask
        && !masks.contains(mask)
    {
        check.issue(section, Some(name), format!("uses mask `{mask}`, which `masks` doesn't name"));
    }
}

fn check_blink(check: &mut Check, blink: &Blink) {
    let section = "blink";
    if blink.expression.is_empty() {
        check.issue(section, None, "no expression".to_owned());
    }
    let [least, most] = blink.every;
    if least == 0 || least > most {
        check.issue(section, None, format!("every ({least}, {most}) isn't a range, least to most"));
    }
    let length = u64::from(blink.close) + u64::from(blink.hold) + u64::from(blink.open);
    if length > u64::from(least) {
        check.issue(section, None, format!("a blink lasts {length} ticks, longer than the {least} between"));
    }
    if blink.close == 0 && blink.open == 0 {
        check.issue(section, None, "it neither closes nor opens over any time".to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "9a29e8c0-f131-4ea4-98b9-04879740413d";
    const TARGET: &str = "6f8a81b6-29c4-485f-b161-847b001d561f";

    /// Everything a document can say, once.
    fn document() -> String {
        format!(
            r#"(
    type: "sapphire::assets::AnimGraph",
    version: 1,
    data: (
        clips: [
            (name: "idle", source: "{SOURCE}", clip: "Idle_A", target: "{TARGET}"),
            (name: "run", source: "{SOURCE}", clip: "Running_A"),
            (name: "jab", source: "{TARGET}"),
        ],
        masks: [(name: "torso", bones: [(bone: Spine, weight: 1.0), (bone: Head, weight: 0.5)])],
        locomotion: (
            start: "ground",
            smoothing: 4.0,
            states: [
                (name: "ground", plays: Blend((
                    samples: [
                        (clip: "idle", at: (0.0, 0.0), sync: false),
                        (clip: "run", at: (0.0, 4.7), phase: 0.25),
                    ],
                    rate: (0.75, 1.5),
                ))),
                (name: "air", plays: Clip(clip: "run", looping: true)),
            ],
            transitions: [
                (from: "ground", to: "air", when: Airborne, decay: 8),
                (from: "air", to: "ground", when: Landed, decay: 4),
                (from: "air", to: "ground", when: Moving(1.5), decay: 6),
            ],
        ),
        combat: (decay: 5, exit: 12, moves: [
            (move: "jab", clip: "jab"),
            (move: "wave", clip: "jab", mask: "torso", decay: 3),
        ]),
        additive: [(name: "breathing", clip: "idle", mask: "torso", weight: 0.5)],
        reactions: [(name: "flinch", clip: "jab", weight: 0.7, fade_out: 8)],
        blink: Some((expression: "blink", every: (150, 360), close: 3, hold: 1, open: 7)),
    ),
)"#
        )
    }

    fn graph() -> AnimGraph {
        AnimGraph::from_document(&document()).expect("a good document")
    }

    /// The issues in `document()` with `from` replaced by `to`.
    fn issues_with(from: &str, to: &str) -> Vec<String> {
        let text = document().replacen(from, to, 1);
        assert_ne!(text, document(), "`{from}` isn't in the document");
        match AnimGraph::from_document(&text) {
            Err(GraphError::Invalid { issues }) => issues.iter().map(ToString::to_string).collect(),
            other => panic!("expected issues, got {other:?}"),
        }
    }

    #[test]
    fn a_document_reads_every_section() {
        let graph = graph();
        assert_eq!(graph.clips.len(), 3);
        assert_eq!(graph.clips[1].target, None);
        assert_eq!(graph.clips[2].clip, None);
        assert_eq!(graph.masks[0].bones[1].weight, 0.5);
        assert_eq!(graph.locomotion.smoothing, 4.0);
        let StateMotion::Blend(space) = &graph.locomotion.states[0].plays else { panic!("a blend") };
        assert!(!space.samples[0].sync && space.samples[1].sync);
        assert_eq!(space.rate, [0.75, 1.5]);
        assert_eq!(graph.locomotion.transitions[2].when, Condition::Moving(1.5));
        assert_eq!(graph.combat.moves[1].decay, Some(3));
        assert_eq!(graph.reactions[0].restart, 4, "the default");
        assert_eq!(graph.blink.as_ref().map(|blink| blink.every), Some([150, 360]));
    }

    #[test]
    fn landing_speed_and_resume_phase_read_and_are_checked() {
        let text = document()
            .replace("when: Airborne", "when: ForwardBelow(4.5)")
            .replace("when: Landed", "when: LandedMoving(4.5)")
            .replace("when: Moving(1.5)", "when: FinishedAtPhase(0.3)");
        let graph = AnimGraph::from_document(&text).expect("landing conditions parse");
        assert_eq!(graph.locomotion.transitions[0].when, Condition::ForwardBelow(4.5));
        assert_eq!(graph.locomotion.transitions[1].when, Condition::LandedMoving(4.5));
        assert_eq!(graph.locomotion.transitions[2].when, Condition::FinishedAtPhase(0.3));
        assert!(issues_with("when: Landed", "when: LandedMoving(-1.0)").join(" ").contains("isn't a speed"));
        assert!(
            issues_with("when: Moving(1.5)", "when: FinishedAtPhase(1.0)")
                .join(" ")
                .contains("isn't a gait phase")
        );
    }

    #[test]
    fn the_moving_airborne_condition_reads_and_checks_its_speed() {
        let text = document().replace("when: Airborne", "when: AirborneMoving(1.0)");
        let graph = AnimGraph::from_document(&text).expect("moving airborne parses");
        assert_eq!(graph.locomotion.transitions[0].when, Condition::AirborneMoving(1.0));
        assert!(
            issues_with("when: Airborne", "when: AirborneMoving(-1.0)").join(" ").contains("isn't a speed")
        );
    }

    #[test]
    fn a_stale_version_is_refused_by_the_envelope() {
        let bad = document().replace("version: 1", "version: 999");
        assert!(matches!(AnimGraph::from_document(&bad), Err(GraphError::Document(_))));
    }

    /// Each row breaks one thing, and the message has to name what and where.
    #[test]
    fn issues_name_the_section_and_the_thing() {
        let rows: &[(&str, &str, &[&str])] = &[
            (r#"name: "run""#, r#"name: "idle""#, &["clips", "idle", "twice"]),
            (r#"start: "ground""#, r#"start: "grounded""#, &["locomotion.start", "grounded"]),
            (r#"smoothing: 4.0"#, r#"smoothing: -1.0"#, &["locomotion.smoothing"]),
            (r#"(clip: "run", at"#, r#"(clip: "sprint", at"#, &["locomotion.states", "ground", "sprint"]),
            (r#"at: (0.0, 4.7)"#, r#"at: (0.0, 0.0)"#, &["ground", "two samples"]),
            (r#"phase: 0.25"#, r#"phase: 1.5"#, &["run", "phase"]),
            (r#"rate: (0.75, 1.5)"#, r#"rate: (1.5, 0.75)"#, &["ground", "rate"]),
            (r#"Clip(clip: "run""#, r#"Clip(clip: "sprint""#, &["air", "sprint"]),
            (r#"to: "air""#, r#"to: "sky""#, &["locomotion.transitions", "sky"]),
            (r#"decay: 8"#, r#"decay: 500"#, &["locomotion.transitions", "500"]),
            (r#"Moving(1.5)"#, r#"Moving(-1.0)"#, &["locomotion.transitions", "speed"]),
            (r#"exit: 12"#, r#"exit: 500"#, &["combat.exit", "500"]),
            (
                r#"(move: "jab", clip: "jab")"#,
                r#"(move: "jab", clip: "hook")"#,
                &["combat.moves", "jab", "hook"],
            ),
            (r#"mask: "torso", decay: 3"#, r#"mask: "legs", decay: 3"#, &["combat.moves", "wave", "legs"]),
            (r#"clip: "idle", mask"#, r#"clip: "nap", mask"#, &["additive", "breathing", "nap"]),
            (
                r#"mask: "torso", weight: 0.5"#,
                r#"mask: "torso", weight: 1.5"#,
                &["additive", "breathing", "1.5"],
            ),
            (
                r#"(name: "flinch", clip: "jab""#,
                r#"(name: "flinch", clip: "ow""#,
                &["reactions", "flinch", "ow"],
            ),
            (r#"every: (150, 360)"#, r#"every: (360, 150)"#, &["blink", "every"]),
            (r#"every: (150, 360)"#, r#"every: (5, 360)"#, &["blink", "longer"]),
        ];
        for (from, to, expected) in rows {
            let issues = issues_with(from, to);
            assert!(
                issues.iter().any(|issue| expected.iter().all(|word| issue.contains(word))),
                "`{to}` gave {issues:?}, wanting {expected:?}",
            );
        }
    }

    #[test]
    fn everything_wrong_is_named_at_once() {
        let text =
            document().replace(r#"start: "ground""#, r#"start: "x""#).replace(r#"exit: 12"#, r#"exit: 999"#);
        let Err(GraphError::Invalid { issues }) = AnimGraph::from_document(&text) else { panic!("it read") };
        assert_eq!(issues.len(), 2, "{issues:?}");
    }

    #[test]
    fn a_malformed_document_quotes_its_line() {
        let text = document().replace("smoothing: 4.0", "smoothing: \"fast\"");
        let Err(GraphError::Malformed { text: quoted, .. }) = AnimGraph::from_document(&text) else {
            panic!("it read")
        };
        assert!(quoted.contains("smoothing"), "{quoted}");
    }
}
