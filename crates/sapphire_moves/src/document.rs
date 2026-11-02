//! [`MoveSetDocument`]: moves as a person writes them, in RON. See the [crate docs](crate) for the
//! format. Everything is in ticks and names; [`MoveSetDocument::compile`] checks it and resolves
//! the names into a [`MoveSet`].

use std::collections::BTreeSet;
use std::fmt;

use bevy_reflect::TypePath;
use sapphire_colliders::{Group, GroupMask};
use sapphire_core::Name;
use sapphire_math::{Quat, det};
use sapphire_reflect::Versioned;
use serde::{Deserialize, Serialize};

use crate::set::{
    Allow, ArmourWindow, Attack, Button, CancelMode, CancelWindow, Cue, FighterDef, FighterHurtbox,
    HIT_LANDED, Hitbox, HurtboxChange, HurtboxState, IntentDef, Knockback, MAX_BUFFER_TICKS,
    MAX_HITSTOP_TICKS, MAX_HURTBOXES, MAX_INTENTS, MAX_MOVE_TICKS, MoveParts, MoveSet, MoveSetError,
    Reactions, Shape, Span, WarpWindow, Weight,
};
use crate::{BakedPath, BakedTables, PathSample, RootSample};

/// One thing wrong with a document: what a cook prints, and what an editor can put beside the track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// The move it's in, if it's in one.
    pub in_move: Option<String>,
    /// The track, or the section of the document: `cancels`, `intents`.
    pub track: &'static str,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.in_move {
            Some(name) => write!(f, "move `{name}`, track `{}`: {}", self.track, self.message),
            None => write!(f, "`{}`: {}", self.track, self.message),
        }
    }
}

/// A document of moves. See the [crate docs](crate) for the format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::moves"]
pub struct MoveSetDocument {
    /// The kinds of intent this set's windows speak of.
    pub intents: Vec<IntentDocument>,
    /// What starts a move from no move at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub neutral: Vec<AllowDocument>,
    /// What makes this set's actors something a hit can land on. Without it they play their moves and
    /// nothing touches them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fighter: Option<FighterDocument>,
    pub moves: Vec<MoveDocument>,
}

impl Versioned for MoveSetDocument {
    /// 1: M2 G2.
    const VERSION: u32 = 1;
}

/// A kind of intent. See [`IntentDef`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentDocument {
    pub name: String,
    /// The button that presses it. A brain presses intents without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<Button>,
    /// Ticks a press stays usable, from 1 to [`MAX_BUFFER_TICKS`].
    pub buffer: u32,
    /// Higher wins, when two could fire at the same instant.
    #[serde(default)]
    pub priority: u8,
}

/// A fighter: its numbers, its hurtboxes and its reactions. See [`FighterDef`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FighterDocument {
    pub health: u32,
    /// The poise meter. Left out or zero, nothing breaks it.
    #[serde(default)]
    pub poise: u32,
    /// Ticks after the last hit that took poise before the meter is full again. Left out or zero,
    /// only a break fills it.
    #[serde(default)]
    pub poise_refill: u32,
    pub hurtboxes: Vec<FighterHurtboxDocument>,
    #[serde(default)]
    pub reactions: ReactionsDocument,
}

/// One of a fighter's own hurtboxes. See [`FighterHurtbox`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FighterHurtboxDocument {
    pub name: String,
    /// Metres from the root, in the root's space.
    #[serde(default)]
    pub offset: [f32; 3],
    /// A unit rotation from the root, as x, y, z, w.
    #[serde(default = "identity")]
    pub rotation: [f32; 4],
    pub shape: Shape,
}

/// Which moves a fighter's reactions play. See [`Reactions`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ReactionsDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub medium: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heavy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poise_break: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defeat: Option<String>,
}

/// An intent starting a move. See [`Allow`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowDocument {
    pub intent: String,
    /// The move it starts.
    #[serde(rename = "move")]
    pub target: String,
    /// How the press has to arrive: buffered unless it says otherwise.
    #[serde(default, skip_serializing_if = "is_buffered")]
    pub when: CancelMode,
}

fn is_buffered(mode: &CancelMode) -> bool {
    *mode == CancelMode::Buffered
}

/// One move. Its length is its three phases, end to end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveDocument {
    pub name: String,
    pub startup: u32,
    pub active: u32,
    pub recovery: u32,
    /// Root motion and hit paths, keyed by hand. A move whose tables come from a clip leaves this
    /// out, and the cook hands [`MoveSetDocument::compile_with`] what it baked.
    #[serde(default, skip_serializing_if = "MotionDocument::is_empty")]
    pub motion: MotionDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hitboxes: Vec<HitboxDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hurtboxes: Vec<HurtboxDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invulnerable: Vec<SpanDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub perfect_dodge: Vec<SpanDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parry: Vec<SpanDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub armour: Vec<ArmourDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancels: Vec<CancelDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warps: Vec<WarpDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<CueDocument>,
}

/// A window with nothing else to say: open on ticks `from` to `to − 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanDocument {
    pub from: u32,
    pub to: u32,
}

/// A hit volume. See [`Hitbox`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HitboxDocument {
    pub name: String,
    pub from: u32,
    pub to: u32,
    /// A path in this move's tables.
    pub path: String,
    pub shape: Shape,
    /// What it does when it lands. Left out, a hit is an event and nothing more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attack: Option<AttackDocument>,
    /// Ticks at which its hit group re-arms, ascending, inside its window: a multi-hit spin's beats.
    /// Hitboxes sharing a name share a hit group, so they list the same ticks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rearm: Vec<u32>,
    /// The collision groups that stop it. Left out, nothing does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<Group>,
}

/// What a hit volume does. See [`Attack`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AttackDocument {
    pub damage: u32,
    pub poise: u32,
    pub weight: Weight,
    /// Ticks both owners freeze for, up to [`MAX_HITSTOP_TICKS`].
    pub hitstop: u16,
    /// Ticks every actor freezes for if this defeats its target, up to [`MAX_HITSTOP_TICKS`].
    #[serde(skip_serializing_if = "is_zero")]
    pub finish_freeze: u16,
    pub knockback: Knockback,
    /// The cue it publishes. Left out, [`HIT_LANDED`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cue: Option<String>,
}

fn is_zero(value: &u16) -> bool {
    *value == 0
}

/// A hurtbox change. See [`HurtboxChange`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HurtboxDocument {
    pub name: String,
    pub from: u32,
    pub to: u32,
    pub state: HurtboxState,
}

/// A window of super armour. See [`ArmourWindow`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmourDocument {
    pub from: u32,
    pub to: u32,
    pub threshold: u32,
}

/// A cancel window. See [`CancelWindow`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelDocument {
    pub from: u32,
    pub to: u32,
    pub allow: Vec<AllowDocument>,
}

/// A warp window. See [`WarpWindow`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WarpDocument {
    pub from: u32,
    pub to: u32,
    #[serde(default)]
    pub max_distance: f32,
    #[serde(default)]
    pub max_turn: f32,
    #[serde(default)]
    pub keep: f32,
}

/// A cue. See [`Cue`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CueDocument {
    pub name: String,
    pub at: u32,
}

/// Motion keyed by hand: what stands in for a baked clip, and all a move may need. Values blend
/// linearly between keys and hold past the last. The root starts at zero whether or not a key says
/// so.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionDocument {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root: Vec<RootKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathDocument>,
}

impl MotionDocument {
    fn is_empty(&self) -> bool {
        self.root.is_empty() && self.paths.is_empty()
    }
}

/// The root at one tick. See [`RootSample`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RootKey {
    pub at: u32,
    /// Metres from the root at the move's first tick, in that root's space: +Z is forward.
    #[serde(default)]
    pub offset: [f32; 3],
    /// Radians about +Y since the move's first tick.
    #[serde(default)]
    pub yaw: f32,
}

/// A named hit path, keyed. See [`BakedPath`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathDocument {
    pub name: String,
    pub keys: Vec<PathKey>,
}

/// A path at one tick. See [`PathSample`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathKey {
    pub at: u32,
    /// Metres from the root at this tick, in that root's space.
    #[serde(default)]
    pub offset: [f32; 3],
    /// A unit rotation from the root at this tick, as x, y, z, w.
    #[serde(default = "identity")]
    pub rotation: [f32; 4],
}

const fn identity() -> [f32; 4] {
    PathSample::IDENTITY.rotation
}

/// How far a keyed rotation may be from unit length, as `BakedTables` holds a sample to.
const UNIT_TOLERANCE: f32 = 2e-3;

/// Gathers a document's issues.
struct Checker {
    issues: Vec<Issue>,
    in_move: Option<String>,
}

impl Checker {
    fn add(&mut self, track: &'static str, message: String) {
        self.issues.push(Issue { in_move: self.in_move.clone(), track, message });
    }

    fn span(&mut self, track: &'static str, from: u32, to: u32, ticks: u32) -> Span {
        if from >= to {
            self.add(track, format!("window {from}..{to} is empty"));
        } else if to > ticks {
            self.add(track, format!("window {from}..{to} runs past the move's end at tick {ticks}"));
        }
        Span { from, to }
    }

    fn named(&mut self, track: &'static str, text: &str) {
        if text.is_empty() {
            self.add(track, "a name is empty".to_owned());
        }
    }

    fn shape(&mut self, track: &'static str, name: &str, shape: Shape) {
        if !shape.is_valid() {
            self.add(track, format!("`{name}` has a shape with no size or a size that isn't finite"));
        }
    }

    /// Resolves an allowance, which must speak of a declared intent and an existing move.
    fn allow(
        &mut self,
        track: &'static str,
        allow: &AllowDocument,
        intents: &BTreeSet<&str>,
        moves: &[MoveDocument],
    ) -> Option<Allow> {
        if !intents.contains(allow.intent.as_str()) {
            self.add(track, format!("`{}` is not an intent this set declares", allow.intent));
        }
        let Some(target) = moves.iter().position(|mv| mv.name == allow.target) else {
            self.add(track, format!("`{}` is not a move in this set", allow.target));
            return None;
        };
        let target = u32::try_from(target).ok()?;
        Some(Allow::new(Name::intern(&allow.intent), target, Name::intern(&allow.target), allow.when))
    }

    /// Checks an allowance list, in which each intent may appear once.
    fn allows(
        &mut self,
        track: &'static str,
        allows: &[AllowDocument],
        intents: &BTreeSet<&str>,
        moves: &[MoveDocument],
    ) -> Vec<Allow> {
        let mut seen = BTreeSet::new();
        for allow in allows {
            if !seen.insert((allow.intent.as_str(), allow.when)) {
                self.add(track, format!("intent `{}` appears twice in one list", allow.intent));
            }
        }
        allows.iter().filter_map(|allow| self.allow(track, allow, intents, moves)).collect()
    }

    /// Checks what a hit volume does, and builds it.
    fn attack(&mut self, name: &str, attack: &AttackDocument) -> Attack {
        for (what, ticks) in [("hitstop", attack.hitstop), ("finish_freeze", attack.finish_freeze)] {
            if ticks > MAX_HITSTOP_TICKS {
                self.add(
                    "hitboxes",
                    format!("`{name}` has a {what} of {ticks} ticks, past the limit of {MAX_HITSTOP_TICKS}"),
                );
            }
        }
        let Knockback { distance, ticks } = attack.knockback;
        if !distance.is_finite() || distance < 0.0 {
            self.add("hitboxes", format!("`{name}` has a knockback that's negative or not finite"));
        } else if distance > 0.0 && !(1..=MAX_MOVE_TICKS).contains(&ticks) {
            self.add(
                "hitboxes",
                format!("`{name}` has a knockback over {ticks} ticks, not 1 to {MAX_MOVE_TICKS}"),
            );
        }
        let cue = match &attack.cue {
            Some(cue) => {
                self.named("hitboxes", cue);
                Name::intern(cue)
            }
            None => HIT_LANDED,
        };
        Attack::new(
            attack.damage,
            attack.poise,
            attack.weight,
            attack.hitstop,
            attack.finish_freeze,
            attack.knockback,
            cue,
        )
    }

    /// Checks a hit volume's re-arm ticks, which land inside its window, in order.
    fn rearm(&mut self, name: &str, hitbox: &HitboxDocument) {
        if !hitbox.rearm.windows(2).all(|pair| pair[0] < pair[1]) {
            self.add("hitboxes", format!("`{name}`'s re-arm ticks are out of order"));
        }
        for &tick in &hitbox.rearm {
            if tick <= hitbox.from || tick >= hitbox.to {
                let (from, to) = (hitbox.from, hitbox.to);
                self.add(
                    "hitboxes",
                    format!("`{name}` re-arms at tick {tick}, outside its window {from}..{to}"),
                );
            }
        }
    }

    /// Checks a fighter and resolves its reactions' names, which must be moves of this set.
    fn fighter(&mut self, fighter: &FighterDocument, moves: &[MoveDocument]) -> FighterDef {
        if fighter.health == 0 {
            self.add("fighter", "a fighter needs some health".to_owned());
        }
        if fighter.hurtboxes.is_empty() {
            self.add("fighter", "a fighter needs a hurtbox".to_owned());
        }
        if fighter.hurtboxes.len() > MAX_HURTBOXES {
            let count = fighter.hurtboxes.len();
            self.add("fighter", format!("{count} hurtboxes, past the limit of {MAX_HURTBOXES}"));
        }
        let mut seen = BTreeSet::new();
        let mut hurtboxes = Vec::with_capacity(fighter.hurtboxes.len());
        for hurtbox in &fighter.hurtboxes {
            self.named("fighter", &hurtbox.name);
            if !seen.insert(hurtbox.name.as_str()) {
                self.add("fighter", format!("hurtbox `{}` appears twice", hurtbox.name));
            }
            self.shape("fighter", &hurtbox.name, hurtbox.shape);
            if !hurtbox.offset.iter().chain(&hurtbox.rotation).all(|value| value.is_finite()) {
                self.add("fighter", format!("hurtbox `{}` has a place that isn't finite", hurtbox.name));
            }
            let length_squared: f32 = hurtbox.rotation.iter().map(|value| value * value).sum();
            if (length_squared - 1.0).abs() > UNIT_TOLERANCE {
                self.add("fighter", format!("hurtbox `{}` doesn't have a unit rotation", hurtbox.name));
            }
            let local = PathSample { translation: hurtbox.offset, rotation: hurtbox.rotation };
            hurtboxes.push(FighterHurtbox::new(Name::intern(&hurtbox.name), local, hurtbox.shape));
        }
        let reactions = &fighter.reactions;
        let mut reaction = |which: &str, name: &Option<String>| {
            let name = name.as_deref()?;
            if !moves.iter().any(|mv| mv.name == name) {
                self.add("fighter", format!("the {which} reaction `{name}` is not a move in this set"));
            }
            Some(Name::intern(name))
        };
        let reactions = Reactions::new(
            reaction("light", &reactions.light),
            reaction("medium", &reactions.medium),
            reaction("heavy", &reactions.heavy),
            reaction("poise break", &reactions.poise_break),
            reaction("defeat", &reactions.defeat),
        );
        FighterDef {
            health: fighter.health,
            poise: fighter.poise,
            poise_refill: fighter.poise_refill,
            hurtboxes,
            reactions,
        }
    }

    /// Checks hand-keyed motion and bakes it, or `None` if a key is wrong.
    fn motion(&mut self, motion: &MotionDocument, ticks: u32) -> Option<BakedTables> {
        let before = self.issues.len();
        let mut previous = None;
        for key in &motion.root {
            if key.at > ticks {
                self.add(
                    "motion",
                    format!("a root key at tick {} is past the move's end at {ticks}", key.at),
                );
            }
            if previous.is_some_and(|previous| key.at <= previous) {
                self.add("motion", format!("root key at tick {} is out of order", key.at));
            }
            previous = Some(key.at);
            if !key.offset.iter().all(|value| value.is_finite()) || !key.yaw.is_finite() {
                self.add("motion", format!("root key at tick {} isn't finite", key.at));
            }
            if key.at == 0 && (key.offset != [0.0; 3] || key.yaw != 0.0) {
                self.add(
                    "motion",
                    "a root key at tick 0 must be zero: root motion counts from it".to_owned(),
                );
            }
        }
        let mut seen = BTreeSet::new();
        for path in &motion.paths {
            if !seen.insert(path.name.as_str()) {
                self.add("motion", format!("path `{}` appears twice", path.name));
            }
            self.named("motion", &path.name);
            if path.keys.is_empty() {
                self.add("motion", format!("path `{}` has no keys", path.name));
            }
            let mut previous = None;
            for key in &path.keys {
                if key.at > ticks {
                    let (name, at) = (&path.name, key.at);
                    self.add(
                        "motion",
                        format!("path `{name}` has a key at tick {at}, past the end at {ticks}"),
                    );
                }
                if previous.is_some_and(|previous| key.at <= previous) {
                    self.add(
                        "motion",
                        format!("path `{}`'s key at tick {} is out of order", path.name, key.at),
                    );
                }
                previous = Some(key.at);
                if !key.offset.iter().chain(&key.rotation).all(|value| value.is_finite()) {
                    self.add("motion", format!("path `{}`'s key at tick {} isn't finite", path.name, key.at));
                }
                let length_squared: f32 = key.rotation.iter().map(|value| value * value).sum();
                if (length_squared - 1.0).abs() > UNIT_TOLERANCE {
                    self.add(
                        "motion",
                        format!("path `{}`'s key at tick {} isn't a unit rotation", path.name, key.at),
                    );
                }
            }
        }
        if self.issues.len() != before {
            return None;
        }
        let root = (0..=ticks).map(|tick| keyed_root(&motion.root, tick)).collect();
        let paths = motion
            .paths
            .iter()
            .map(|path| {
                let samples = (0..=ticks).map(|tick| keyed_path(&path.keys, tick)).collect();
                BakedPath::new(Name::intern(&path.name), samples)
            })
            .collect();
        match BakedTables::new(root, paths) {
            Ok(tables) => Some(tables),
            Err(error) => {
                self.add("motion", error.to_string());
                None
            }
        }
    }
}

/// The keys around `tick`, and how far between them. `None` if there are none at all.
fn around<K: Copy>(keys: &[K], at: impl Fn(&K) -> u32, tick: u32) -> Option<(K, K, f32)> {
    let after = keys.iter().position(|key| at(key) > tick);
    match after {
        None => keys.last().map(|&key| (key, key, 0.0)),
        Some(0) => keys.first().map(|&key| (key, key, 0.0)),
        Some(index) => {
            let (a, b) = (keys[index - 1], keys[index]);
            let t = (tick - at(&a)) as f32 / (at(&b) - at(&a)) as f32;
            Some((a, b, t))
        }
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

fn keyed_root(keys: &[RootKey], tick: u32) -> RootSample {
    if tick == 0 {
        return RootSample::ZERO;
    }
    // The root's implicit key at tick 0.
    let at_zero = RootKey { at: 0, offset: [0.0; 3], yaw: 0.0 };
    let before = keys.iter().rev().find(|key| key.at <= tick).copied().unwrap_or(at_zero);
    let Some(after) = keys.iter().find(|key| key.at > tick).copied() else {
        return RootSample { translation: before.offset, yaw: before.yaw };
    };
    let t = (tick - before.at) as f32 / (after.at - before.at) as f32;
    RootSample {
        translation: lerp3(before.offset, after.offset, t),
        yaw: before.yaw + (after.yaw - before.yaw) * t,
    }
}

fn keyed_path(keys: &[PathKey], tick: u32) -> PathSample {
    let Some((a, b, t)) = around(keys, |key| key.at, tick) else {
        return PathSample::IDENTITY;
    };
    if t == 0.0 {
        return PathSample { translation: a.offset, rotation: normalized(a.rotation) };
    }
    let rotation = det::quat_nlerp(Quat::from_array(a.rotation), Quat::from_array(b.rotation), t);
    PathSample { translation: lerp3(a.offset, b.offset, t), rotation: rotation.to_array() }
}

/// A keyed rotation at exactly unit length, as a blend's is, in the same bits on every architecture.
fn normalized(rotation: [f32; 4]) -> [f32; 4] {
    det::quat_normalize(Quat::from_array(rotation)).to_array()
}

impl MoveSetDocument {
    /// Reads a document, with RON's errors pointing at the line.
    pub fn parse(text: &str) -> Result<Self, MoveSetError> {
        sapphire_serialize::from_ron(text).map_err(|error| MoveSetError::Document(Box::new(error)))
    }

    /// Writes a document, as hand-editing and the timeline editor both see it: empty tracks are
    /// left out.
    pub fn to_text(&self) -> Result<String, MoveSetError> {
        sapphire_serialize::to_ron(self).map_err(|error| MoveSetError::Document(Box::new(error)))
    }

    /// Everything wrong with this document, with each move's tables baked from its keys.
    #[must_use]
    pub fn check(&self) -> Vec<Issue> {
        match self.compile() {
            Ok(_) => Vec::new(),
            Err(MoveSetError::Invalid(issues)) => issues,
            Err(error) => vec![Issue { in_move: None, track: "moves", message: error.to_string() }],
        }
    }

    /// Checks this document and resolves its names, with each move's tables baked from its keys.
    pub fn compile(&self) -> Result<MoveSet, MoveSetError> {
        self.compile_with(|_| None)
    }

    /// [`Self::compile`], asking `tables` for each move's tables first: a cook that bakes them from
    /// a clip, or a test with a fixture. A move it answers `None` for bakes from its keys. Wherever
    /// tables come from, the hit paths named must be in them, and they must have the same bits on
    /// every architecture: see the [crate docs](crate).
    pub fn compile_with(
        &self,
        mut tables: impl FnMut(&MoveDocument) -> Option<BakedTables>,
    ) -> Result<MoveSet, MoveSetError> {
        let mut check = Checker { issues: Vec::new(), in_move: None };

        if self.moves.is_empty() {
            check.add("moves", "a set needs at least one move".to_owned());
        }
        if self.intents.len() > MAX_INTENTS {
            let count = self.intents.len();
            check.add("intents", format!("{count} intents, past the limit of {MAX_INTENTS}"));
        }
        let mut intent_names = BTreeSet::new();
        let mut intents = Vec::with_capacity(self.intents.len());
        for intent in &self.intents {
            check.named("intents", &intent.name);
            if !intent_names.insert(intent.name.as_str()) {
                check.add("intents", format!("`{}` appears twice", intent.name));
            }
            if !(1..=MAX_BUFFER_TICKS).contains(&intent.buffer) {
                let (name, buffer) = (&intent.name, intent.buffer);
                check.add("intents", format!("`{name}` buffers {buffer} ticks, not 1 to {MAX_BUFFER_TICKS}"));
            }
            intents.push(IntentDef::new(intent.name.clone(), intent.button, intent.buffer, intent.priority));
        }
        let neutral = check.allows("neutral", &self.neutral, &intent_names, &self.moves);
        let fighter = self.fighter.as_ref().map(|fighter| check.fighter(fighter, &self.moves));

        let mut names = BTreeSet::new();
        let mut moves = Vec::with_capacity(self.moves.len());
        for mv in &self.moves {
            check.in_move = Some(mv.name.clone());
            check.named("name", &mv.name);
            if !names.insert(mv.name.as_str()) {
                check.add("name", "two moves have this name".to_owned());
            }
            let ticks = mv.startup.saturating_add(mv.active).saturating_add(mv.recovery);
            if ticks == 0 {
                check.add("phases", "startup, active and recovery add up to no ticks".to_owned());
            } else if ticks > MAX_MOVE_TICKS {
                check.add("phases", format!("{ticks} ticks, past the limit of {MAX_MOVE_TICKS}"));
            }
            // Past the limit nothing about this move can be checked, and its tables could be huge.
            if ticks > MAX_MOVE_TICKS {
                continue;
            }
            let known = Known { intents: &intent_names, moves: &self.moves, fighter: self.fighter.as_ref() };
            if let Some(built) = compile_move(mv, ticks, &known, &mut check, &mut tables) {
                moves.push(built);
            }
        }

        if check.issues.is_empty() {
            Ok(MoveSet::from_parts(intents, neutral, fighter, moves))
        } else {
            Err(MoveSetError::Invalid(check.issues))
        }
    }
}

/// What a move's tracks can refer to.
struct Known<'a> {
    intents: &'a BTreeSet<&'a str>,
    moves: &'a [MoveDocument],
    fighter: Option<&'a FighterDocument>,
}

fn compile_move(
    mv: &MoveDocument,
    ticks: u32,
    known: &Known<'_>,
    check: &mut Checker,
    tables: &mut impl FnMut(&MoveDocument) -> Option<BakedTables>,
) -> Option<crate::set::Move> {
    let tables = match tables(mv) {
        Some(tables) => Some(tables),
        None => check.motion(&mv.motion, ticks),
    };

    let mut hitboxes = Vec::with_capacity(mv.hitboxes.len());
    for hitbox in &mv.hitboxes {
        check.named("hitboxes", &hitbox.name);
        let span = check.span("hitboxes", hitbox.from, hitbox.to, ticks);
        check.shape("hitboxes", &hitbox.name, hitbox.shape);
        let path = Name::intern(&hitbox.path);
        if let Some(tables) = &tables
            && tables.path(path).is_none()
        {
            let (name, path) = (&hitbox.name, &hitbox.path);
            check.add("hitboxes", format!("`{name}` rides path `{path}`, which isn't in the move's table"));
        }
        let attack = hitbox.attack.as_ref().map(|attack| check.attack(&hitbox.name, attack));
        check.rearm(&hitbox.name, hitbox);
        let same_group = mv.hitboxes.iter().find(|other| other.name == hitbox.name);
        if same_group.is_some_and(|first| first.rearm != hitbox.rearm) {
            check.add("hitboxes", format!("`{}` share a hit group and re-arm differently", hitbox.name));
        }
        let blocked_by =
            hitbox.blocked_by.iter().fold(GroupMask::NONE, |mask, group| mask.union(group.mask()));
        hitboxes.push(Hitbox::new(
            Name::intern(&hitbox.name),
            path,
            span,
            hitbox.shape,
            attack,
            hitbox.rearm.clone(),
            blocked_by,
        ));
    }

    let mut hurtboxes = Vec::with_capacity(mv.hurtboxes.len());
    for change in &mv.hurtboxes {
        check.named("hurtboxes", &change.name);
        let span = check.span("hurtboxes", change.from, change.to, ticks);
        if let HurtboxState::Shape(shape) = change.state {
            check.shape("hurtboxes", &change.name, shape);
        }
        if let Some(fighter) = known.fighter
            && !fighter.hurtboxes.iter().any(|hurtbox| hurtbox.name == change.name)
        {
            check.add("hurtboxes", format!("`{}` is not one of this fighter's hurtboxes", change.name));
        }
        hurtboxes.push(HurtboxChange::new(Name::intern(&change.name), span, change.state));
    }

    let spans = |check: &mut Checker, track, list: &[SpanDocument]| -> Vec<Span> {
        list.iter().map(|span| check.span(track, span.from, span.to, ticks)).collect()
    };
    let invulnerable = spans(check, "invulnerable", &mv.invulnerable);
    let perfect_dodge = spans(check, "perfect_dodge", &mv.perfect_dodge);
    let parry = spans(check, "parry", &mv.parry);

    let mut armour = Vec::with_capacity(mv.armour.len());
    for window in &mv.armour {
        let span = check.span("armour", window.from, window.to, ticks);
        if window.threshold == 0 {
            let (from, to) = (window.from, window.to);
            check.add("armour", format!("window {from}..{to} has a threshold of 0, which stops nothing"));
        }
        armour.push(ArmourWindow { span, threshold: window.threshold });
    }

    let mut cancels = Vec::with_capacity(mv.cancels.len());
    for window in &mv.cancels {
        let span = check.span("cancels", window.from, window.to, ticks);
        if window.allow.is_empty() {
            check.add("cancels", format!("window {}..{} allows nothing", window.from, window.to));
        }
        let allow = check.allows("cancels", &window.allow, known.intents, known.moves);
        cancels.push(CancelWindow { span, allow });
    }

    let mut warps = Vec::with_capacity(mv.warps.len());
    for warp in &mv.warps {
        let span = check.span("warps", warp.from, warp.to, ticks);
        let limits = [warp.max_distance, warp.max_turn, warp.keep];
        if !limits.iter().all(|limit| limit.is_finite() && *limit >= 0.0) {
            let (from, to) = (warp.from, warp.to);
            check.add("warps", format!("window {from}..{to} has a limit that's negative or not finite"));
        }
        if warp.max_distance == 0.0 && warp.max_turn == 0.0 {
            let (from, to) = (warp.from, warp.to);
            check.add("warps", format!("window {from}..{to} can't move or turn anything"));
        }
        warps.push(WarpWindow {
            span,
            max_distance: warp.max_distance,
            max_turn: warp.max_turn,
            keep: warp.keep,
        });
    }
    let mut sorted: Vec<Span> = warps.iter().map(|warp| warp.span).collect();
    sorted.sort_by_key(|span| span.from);
    for pair in sorted.windows(2) {
        if pair[0].to > pair[1].from {
            let (a, b) = (pair[0], pair[1]);
            check.add("warps", format!("windows {}..{} and {}..{} overlap", a.from, a.to, b.from, b.to));
        }
    }

    let mut cues = Vec::with_capacity(mv.cues.len());
    for cue in &mv.cues {
        check.named("cues", &cue.name);
        if cue.at >= ticks {
            check.add(
                "cues",
                format!(
                    "`{}` at tick {} is past the move's last tick, {}",
                    cue.name,
                    cue.at,
                    ticks.saturating_sub(1)
                ),
            );
        }
        cues.push(Cue::new(Name::intern(&cue.name), cue.at));
    }

    Some(
        MoveParts {
            label: mv.name.clone(),
            startup: mv.startup,
            active: mv.active,
            recovery: mv.recovery,
            tables: tables?,
            hitboxes,
            hurtboxes,
            invulnerable,
            perfect_dodge,
            parry,
            armour,
            cancels,
            warps,
            cues,
        }
        .build(),
    )
}

#[cfg(test)]
mod tests {
    use sapphire_time::SimDuration;

    use super::*;
    use crate::set::{MOVE_SET_EXTENSION, Move, Phase};

    const SLICE: &str = include_str!("../test_content/slice.moves.ron");

    fn slice() -> MoveSetDocument {
        MoveSetDocument::parse(SLICE).expect("the test content parses")
    }

    #[test]
    fn the_test_content_compiles_and_reads_as_plain_data() {
        let set = slice().compile().expect("the test content is good");
        let (_, light) = set.find(Name::new("light_1")).expect("a first hit");
        assert_eq!((light.startup, light.active, light.recovery, light.ticks()), (5, 3, 18, 26));
        assert_eq!(light.phase_at(SimDuration::from_ticks(4)), Phase::Startup);
        assert_eq!(light.phase_at(SimDuration::from_ticks(5)), Phase::Active);
        assert_eq!(light.phase_at(SimDuration::from_ticks(8)), Phase::Recovery);
        let hit = light.hitboxes_on(5).next().expect("the swing");
        assert_eq!(hit.name(), Name::new("light_1_hit"));
        assert_eq!(hit.path(), Name::new("blade"));
        assert!(light.hitboxes_on(4).next().is_none());
        assert!(light.hitboxes_on(8).next().is_none());
        assert!(light.tables.path(hit.path()).is_some());
        assert_eq!(light.tables.ticks(), 26);

        let (_, dodge) = set.find(Name::new("dodge")).expect("a dodge");
        assert_eq!(dodge.invulnerable, [Span { from: 1, to: 14 }]);
        assert_eq!(dodge.perfect_dodge, [Span { from: 1, to: 7 }]);
        let (_, dodge_intent) = set.intent(Name::new("dodge")).expect("declared");
        let (_, light_intent) = set.intent(Name::new("light")).expect("declared");
        assert!(dodge_intent.priority > light_intent.priority);
        assert_eq!(light_intent.buffer(), SimDuration::from_ticks(12));
        assert_eq!(MOVE_SET_EXTENSION, "smoves");
    }

    #[test]
    fn a_document_round_trips_through_its_text() {
        let document = slice();
        let text = document.to_text().unwrap();
        assert_eq!(MoveSetDocument::parse(&text).unwrap(), document);
        assert!(!text.contains("hurtboxes: []"), "empty tracks stay out of the way:\n{text}");
    }

    #[test]
    fn a_cook_is_byte_stable_and_reads_back_the_same() {
        let set = slice().compile().unwrap();
        let cooked = set.to_cooked().unwrap();
        let back = MoveSet::from_cooked(&cooked).unwrap();
        assert_eq!(back, set);
        assert_eq!(*back.to_cooked().unwrap(), *cooked);
        assert_eq!(*slice().compile().unwrap().to_cooked().unwrap(), *cooked);
    }

    #[test]
    fn stale_and_damaged_bytes_are_refused() {
        let cooked = slice().compile().unwrap().to_cooked().unwrap();
        let mut stale = cooked.clone();
        stale[16] += 1;
        assert!(matches!(
            MoveSet::from_cooked(&stale),
            Err(MoveSetError::Cooked(sapphire_serialize::cooked::CookedError::Stale {
                found: 3,
                current: 2
            }))
        ));
        let cut = &cooked[..cooked.len() - 8];
        assert!(MoveSet::from_cooked(cut).is_err());

        // A well-formed archive that points nowhere.
        let mut set = slice().compile().unwrap();
        let target = 99;
        set = MoveSet::from_parts(
            set.intents().to_vec(),
            vec![Allow::new(Name::new("light"), target, Name::new("x"), CancelMode::Buffered)],
            set.fighter().cloned(),
            set.moves().to_vec(),
        );
        let bad = set.to_cooked().unwrap();
        assert!(matches!(MoveSet::from_cooked(&bad), Err(MoveSetError::Damaged(_))));
    }

    /// One bad document per check: the message must name the move, the track and the thing.
    #[test]
    fn bad_documents_name_the_move_and_track() {
        type Edit = fn(&mut MoveSetDocument);
        fn light(document: &mut MoveSetDocument) -> &mut MoveDocument {
            document.moves.iter_mut().find(|mv| mv.name == "light_1").expect("in the content")
        }
        fn attack(document: &mut MoveSetDocument) -> &mut AttackDocument {
            light(document).hitboxes[0].attack.as_mut().expect("in the content")
        }
        fn fighter(document: &mut MoveSetDocument) -> &mut FighterDocument {
            document.fighter.as_mut().expect("in the content")
        }
        let cases: &[(&str, Edit, &[&str])] = &[
            (
                "more intents than a set's holds have bits",
                |d| {
                    let extra = d.intents[0].clone();
                    let name = |index| format!("extra_{index}");
                    d.intents.extend((0..16).map(|i| IntentDocument { name: name(i), ..extra.clone() }));
                },
                &["intents", "21 intents", "limit of 16"],
            ),
            (
                "a window past the move's end",
                |d| light(d).cancels[0].to = 40,
                &["light_1", "cancels", "2..40", "past the move's end at tick 26"],
            ),
            (
                "a cancel to a move that doesn't exist",
                |d| light(d).cancels[0].allow[0].target = "ghost".to_owned(),
                &["light_1", "cancels", "`ghost`", "not a move"],
            ),
            (
                "a cancel on an undeclared intent",
                |d| light(d).cancels[0].allow[0].intent = "ghost".to_owned(),
                &["light_1", "cancels", "`ghost`", "not an intent"],
            ),
            (
                "a hitbox naming a path that isn't in the table",
                |d| light(d).hitboxes[0].path = "ghost".to_owned(),
                &["light_1", "hitboxes", "light_1_hit", "`ghost`", "isn't in the move's table"],
            ),
            (
                "an empty window",
                |d| light(d).hitboxes[0].to = light(d).hitboxes[0].from,
                &["light_1", "hitboxes", "is empty"],
            ),
            (
                "a shape with no size",
                |d| light(d).hitboxes[0].shape = Shape::Sphere { radius: 0.0 },
                &["light_1", "hitboxes", "shape"],
            ),
            (
                "a hurtbox shape with no size",
                |d| {
                    light(d).hurtboxes.push(HurtboxDocument {
                        name: "body".to_owned(),
                        from: 1,
                        to: 2,
                        state: HurtboxState::Shape(Shape::Box { half_extents: [0.1, -1.0, 0.1] }),
                    });
                },
                &["light_1", "hurtboxes", "body"],
            ),
            (
                "a window past the end in a plain track",
                |d| light(d).parry.push(SpanDocument { from: 0, to: 27 }),
                &["light_1", "parry", "0..27"],
            ),
            (
                "a cue past the last tick",
                |d| light(d).cues[0].at = 26,
                &["light_1", "cues", "tick 26", "last tick, 25"],
            ),
            (
                "warps that overlap",
                |d| {
                    let warp = WarpDocument { from: 1, to: 5, max_distance: 1.0, max_turn: 0.0, keep: 0.0 };
                    light(d).warps = vec![warp, WarpDocument { from: 4, ..warp }];
                },
                &["light_1", "warps", "overlap"],
            ),
            (
                "a warp with negative limits",
                |d| {
                    let warp = WarpDocument { from: 1, to: 5, max_distance: -1.0, max_turn: 0.0, keep: 0.0 };
                    light(d).warps = vec![warp];
                },
                &["light_1", "warps", "negative"],
            ),
            (
                "a warp that can't do anything",
                |d| {
                    let warp = WarpDocument { from: 1, to: 5, max_distance: 0.0, max_turn: 0.0, keep: 1.0 };
                    light(d).warps = vec![warp];
                },
                &["light_1", "warps", "can't move or turn"],
            ),
            (
                "phases that add up to nothing",
                |d| {
                    let mv = light(d);
                    (mv.startup, mv.active, mv.recovery) = (0, 0, 0);
                },
                &["light_1", "phases", "no ticks"],
            ),
            (
                "a move too long",
                |d| light(d).recovery = MAX_MOVE_TICKS,
                &["light_1", "phases", "past the limit"],
            ),
            (
                "a move named twice",
                |d| light(d).name = "light_2".to_owned(),
                &["light_2", "two moves have this name"],
            ),
            (
                "a neutral start to nothing",
                |d| d.neutral[0].target = "ghost".to_owned(),
                &["`neutral`", "`ghost`"],
            ),
            (
                "an intent buffered too long",
                |d| d.intents[0].buffer = 16,
                &["`intents`", "`light`", "16 ticks"],
            ),
            ("an intent buffered for nothing", |d| d.intents[0].buffer = 0, &["`intents`", "`light`"]),
            (
                "an intent declared twice",
                |d| {
                    let copy = d.intents[0].clone();
                    d.intents.push(copy);
                },
                &["`intents`", "appears twice"],
            ),
            (
                "the same intent twice in a window",
                |d| {
                    let copy = light(d).cancels[0].allow[0].clone();
                    light(d).cancels[0].allow.push(copy);
                },
                &["light_1", "cancels", "appears twice"],
            ),
            (
                "a window allowing nothing",
                |d| light(d).cancels[0].allow.clear(),
                &["light_1", "cancels", "allows nothing"],
            ),
            (
                "a hitstop past its limit",
                |d| attack(d).hitstop = 121,
                &["light_1", "hitboxes", "light_1_hit", "hitstop of 121 ticks", "limit of 120"],
            ),
            (
                "a finish freeze past its limit",
                |d| attack(d).finish_freeze = 121,
                &["light_1", "hitboxes", "light_1_hit", "finish_freeze of 121 ticks"],
            ),
            (
                "a knockback going backwards",
                |d| attack(d).knockback.distance = -1.0,
                &["light_1", "hitboxes", "light_1_hit", "knockback", "negative"],
            ),
            (
                "a knockback that isn't finite",
                |d| attack(d).knockback.distance = f32::NAN,
                &["light_1", "hitboxes", "light_1_hit", "knockback", "not finite"],
            ),
            (
                "a knockback in no time",
                |d| attack(d).knockback = Knockback { distance: 1.0, ticks: 0 },
                &["light_1", "hitboxes", "light_1_hit", "knockback over 0 ticks"],
            ),
            (
                "a knockback past a move's longest",
                |d| attack(d).knockback = Knockback { distance: 1.0, ticks: 3601 },
                &["light_1", "hitboxes", "light_1_hit", "knockback over 3601 ticks"],
            ),
            (
                "a cue with no name",
                |d| attack(d).cue = Some(String::new()),
                &["light_1", "hitboxes", "empty"],
            ),
            (
                "re-arm ticks out of order",
                |d| {
                    let hitbox = &mut light(d).hitboxes[0];
                    (hitbox.from, hitbox.to, hitbox.rearm) = (5, 8, vec![7, 6]);
                },
                &["light_1", "hitboxes", "light_1_hit", "out of order"],
            ),
            (
                "a re-arm tick repeated",
                |d| light(d).hitboxes[0].rearm = vec![6, 6],
                &["light_1", "hitboxes", "out of order"],
            ),
            (
                "a re-arm on its window's first tick, which can't follow anything",
                |d| light(d).hitboxes[0].rearm = vec![5],
                &["light_1", "hitboxes", "re-arms at tick 5", "outside its window 5..8"],
            ),
            (
                "a re-arm on its window's end",
                |d| light(d).hitboxes[0].rearm = vec![8],
                &["light_1", "hitboxes", "re-arms at tick 8"],
            ),
            (
                "two windows of one group re-arming differently",
                |d| {
                    let copy = HitboxDocument { rearm: vec![6], ..light(d).hitboxes[0].clone() };
                    light(d).hitboxes.push(copy);
                },
                &["light_1", "hitboxes", "`light_1_hit`", "re-arm differently"],
            ),
            (
                "an armour window that stops nothing",
                |d| light(d).armour.push(ArmourDocument { from: 1, to: 4, threshold: 0 }),
                &["light_1", "armour", "1..4", "threshold of 0"],
            ),
            (
                "an armour window past the move's end",
                |d| light(d).armour.push(ArmourDocument { from: 1, to: 40, threshold: 5 }),
                &["light_1", "armour", "1..40", "past the move's end"],
            ),
            (
                "a hurtbox window naming none of the fighter's",
                |d| {
                    light(d).hurtboxes.push(HurtboxDocument {
                        name: "ghost".to_owned(),
                        from: 1,
                        to: 2,
                        state: HurtboxState::Off,
                    });
                },
                &["light_1", "hurtboxes", "`ghost`", "not one of this fighter's"],
            ),
            ("a fighter with no health", |d| fighter(d).health = 0, &["`fighter`", "needs some health"]),
            (
                "a fighter with no hurtbox",
                |d| fighter(d).hurtboxes.clear(),
                &["`fighter`", "needs a hurtbox"],
            ),
            (
                "a fighter with too many hurtboxes",
                |d| {
                    let hurtbox = fighter(d).hurtboxes[0].clone();
                    let named = |i| FighterHurtboxDocument { name: format!("extra_{i}"), ..hurtbox.clone() };
                    fighter(d).hurtboxes.extend((0..16).map(named));
                },
                &["`fighter`", "17 hurtboxes", "limit of 16"],
            ),
            (
                "a hurtbox listed twice",
                |d| {
                    let hurtbox = fighter(d).hurtboxes[0].clone();
                    fighter(d).hurtboxes.push(hurtbox);
                },
                &["`fighter`", "`body`", "appears twice"],
            ),
            (
                "a hurtbox with no size",
                |d| fighter(d).hurtboxes[0].shape = Shape::Sphere { radius: 0.0 },
                &["`fighter`", "`body`", "shape"],
            ),
            (
                "a hurtbox placed nowhere",
                |d| fighter(d).hurtboxes[0].offset[1] = f32::INFINITY,
                &["`fighter`", "`body`", "isn't finite"],
            ),
            (
                "a hurtbox with a rotation that isn't unit",
                |d| fighter(d).hurtboxes[0].rotation = [0.0, 0.0, 0.0, 2.0],
                &["`fighter`", "`body`", "unit rotation"],
            ),
            (
                "a reaction that isn't a move",
                |d| fighter(d).reactions.defeat = Some("ghost".to_owned()),
                &["`fighter`", "defeat reaction", "`ghost`", "not a move"],
            ),
            (
                "another reaction that isn't a move",
                |d| fighter(d).reactions.poise_break = Some("ghost".to_owned()),
                &["`fighter`", "poise break reaction", "`ghost`"],
            ),
            (
                "a root key out of order",
                |d| light(d).motion.root.reverse(),
                &["light_1", "motion", "out of order"],
            ),
            (
                "a root key past the end",
                |d| light(d).motion.root.last_mut().unwrap().at = 40,
                &["light_1", "motion", "past the move's end"],
            ),
            (
                "a root that doesn't start at zero",
                |d| light(d).motion.root.insert(0, RootKey { at: 0, offset: [1.0, 0.0, 0.0], yaw: 0.0 }),
                &["light_1", "motion", "tick 0 must be zero"],
            ),
            (
                "a path rotation that isn't unit",
                |d| light(d).motion.paths[0].keys[0].rotation = [0.0, 0.0, 0.0, 2.0],
                &["light_1", "motion", "blade", "unit"],
            ),
            (
                "a path with no keys",
                |d| light(d).motion.paths[0].keys.clear(),
                &["light_1", "motion", "blade", "no keys"],
            ),
        ];
        for (what, edit, expected) in cases {
            let mut document = slice();
            edit(&mut document);
            let Err(MoveSetError::Invalid(issues)) = document.compile() else {
                panic!("{what}: compiled");
            };
            let text = issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
            for part in *expected {
                assert!(text.contains(part), "{what}: no {part:?} in:\n{text}");
            }
            assert_eq!(document.check(), issues, "{what}");
        }
    }

    #[test]
    fn every_issue_is_reported_at_once() {
        let mut document = slice();
        document.moves[0].cancels[0].to = 400;
        document.moves[1].hitboxes[0].path = "ghost".to_owned();
        document.intents[0].buffer = 99;
        assert_eq!(document.check().len(), 3, "{:#?}", document.check());
    }

    #[test]
    fn an_empty_set_is_refused() {
        let document = MoveSetDocument { intents: vec![], neutral: vec![], fighter: None, moves: vec![] };
        assert!(document.check().iter().any(|issue| issue.message.contains("at least one move")));
    }

    #[test]
    fn what_a_fight_reads_compiles_from_its_document() {
        let set = slice().compile().unwrap();
        let (_, light) = set.find(Name::new("light_1")).unwrap();
        let attack = light.hitboxes[0].attack.expect("authored");
        assert_eq!((attack.damage, attack.poise, attack.weight, attack.hitstop), (6, 8, Weight::Light, 3));
        assert_eq!(attack.knockback, Knockback { distance: 0.25, ticks: 8 });
        assert_eq!(attack.cue(), HIT_LANDED, "with none authored");
        assert_eq!(light.hitboxes[0].rearm, Vec::<u32>::new());
        assert_eq!(light.hitboxes[0].blocked_by(), GroupMask::NONE);

        let (_, heavy) = set.find(Name::new("heavy")).unwrap();
        let attack = heavy.hitboxes[0].attack.expect("authored");
        assert_eq!(attack.weight, Weight::Heavy);
        assert_eq!(attack.cue(), Name::new("hit_heavy"), "authored");
        assert_eq!(attack.finish_freeze, 24);
        assert_eq!(heavy.armour, [ArmourWindow { span: Span { from: 2, to: 20 }, threshold: 15 }]);

        let (_, spin) = set.find(Name::new("spin")).unwrap();
        assert_eq!(spin.hitboxes[0].rearm, [11, 15]);
        assert_eq!(spin.hitboxes[0].blocked_by(), Group::World.mask());

        let fighter = set.fighter().expect("authored");
        assert_eq!((fighter.health, fighter.poise, fighter.poise_refill), (300, 60, 240));
        assert_eq!(fighter.refill(), SimDuration::from_ticks(240));
        assert_eq!(fighter.hurtboxes.len(), 1);
        let body = fighter.hurtboxes[0];
        assert_eq!(body.name(), Name::new("body"));
        assert_eq!(body.local.translation, [0.0, 0.9, 0.0]);
        assert_eq!(body.local.rotation, PathSample::IDENTITY.rotation, "by default");
        assert_eq!(body.shape, Shape::Capsule { radius: 0.35, length: 1.0 });
        let reactions = fighter.reactions;
        assert_eq!(reactions.flinch(Weight::Light), Some(Name::new("flinch")));
        assert_eq!(reactions.flinch(Weight::Medium), Some(Name::new("stagger")));
        assert_eq!(reactions.flinch(Weight::Heavy), Some(Name::new("knockdown")));
        assert_eq!(reactions.poise_break(), Some(Name::new("dazed")));
        assert_eq!(reactions.defeat(), Some(Name::new("defeat")));
        assert!(Weight::Light < Weight::Medium && Weight::Medium < Weight::Heavy);
    }

    #[test]
    fn a_set_with_no_fighter_or_attacks_still_compiles_as_it_did() {
        let mut document = slice();
        document.fighter = None;
        for mv in &mut document.moves {
            for hitbox in &mut mv.hitboxes {
                hitbox.attack = None;
            }
        }
        let text = document.to_text().unwrap();
        assert!(!text.contains("fighter:") && !text.contains("attack:"), "{text}");
        let set = MoveSetDocument::parse(&text).unwrap().compile().unwrap();
        assert!(set.fighter().is_none());
        assert_eq!(set.moves()[0].hitboxes[0].attack, None);
    }

    #[test]
    fn a_misspelt_field_in_what_a_hit_does_is_an_error() {
        let with = |from: &str, to: &str| {
            assert!(SLICE.contains(from), "{from}");
            MoveSetDocument::parse(&SLICE.replacen(from, to, 1))
        };
        assert!(matches!(with("hitstop:", "hitstp:"), Err(MoveSetError::Document(_))));
        assert!(matches!(with("poise_refill:", "poise_refil:"), Err(MoveSetError::Document(_))));
        assert!(matches!(with("threshold:", "treshold:"), Err(MoveSetError::Document(_))));
        assert!(matches!(with("distance:", "distanse:"), Err(MoveSetError::Document(_))));
        assert!(matches!(
            with("heavy: \"knockdown\"", "hevy: \"knockdown\""),
            Err(MoveSetError::Document(_))
        ));
    }

    /// Bytes from anywhere are checked as well as documents are, for what the simulation relies on.
    #[test]
    fn a_fight_read_from_damaged_bytes_is_refused() {
        let good = slice().compile().unwrap();
        let rebuilt = |edit: fn(&mut Vec<Move>, &mut Option<FighterDef>)| {
            let mut moves = good.moves().to_vec();
            let mut fighter = good.fighter().cloned();
            edit(&mut moves, &mut fighter);
            let set = MoveSet::from_parts(good.intents().to_vec(), good.neutral().to_vec(), fighter, moves);
            MoveSet::from_cooked(&set.to_cooked().unwrap())
        };
        assert!(rebuilt(|_, _| {}).is_ok());
        let too_many = rebuilt(|_, fighter| {
            let fighter = fighter.as_mut().unwrap();
            let body = fighter.hurtboxes[0];
            fighter.hurtboxes = vec![body; MAX_HURTBOXES + 1];
        });
        assert!(matches!(too_many, Err(MoveSetError::Damaged(text)) if text.contains("hurtboxes")));
        let rearm = rebuilt(|moves, _| moves[0].hitboxes[0].rearm = vec![7, 6]);
        assert!(matches!(rearm, Err(MoveSetError::Damaged(text)) if text.contains("re-arms")));
        let armour = rebuilt(|moves, _| {
            moves[0].armour = vec![ArmourWindow { span: Span { from: 1, to: 999 }, threshold: 1 }];
        });
        assert!(matches!(armour, Err(MoveSetError::Damaged(text)) if text.contains("window")));
    }

    #[test]
    fn keys_blend_linearly_and_hold_past_the_last() {
        let motion = MotionDocument {
            root: vec![
                RootKey { at: 2, offset: [0.0, 0.0, 1.0], yaw: 0.5 },
                RootKey { at: 4, offset: [0.0, 0.0, 2.0], yaw: 0.5 },
            ],
            paths: vec![PathDocument {
                name: "blade".to_owned(),
                keys: vec![
                    PathKey { at: 1, offset: [1.0, 0.0, 0.0], rotation: identity() },
                    PathKey { at: 3, offset: [3.0, 0.0, 0.0], rotation: identity() },
                ],
            }],
        };
        let mut check = Checker { issues: Vec::new(), in_move: None };
        let tables = check.motion(&motion, 5).expect("good keys");
        assert_eq!(tables.ticks(), 5);
        let z: Vec<f32> = tables.root_samples().iter().map(|sample| sample.translation[2]).collect();
        assert_eq!(z, [0.0, 0.5, 1.0, 1.5, 2.0, 2.0], "an implicit zero at tick 0, held past tick 4");
        assert_eq!(tables.root_samples()[1].yaw, 0.25);
        let blade = tables.path(Name::new("blade")).expect("baked");
        let x: Vec<f32> = blade.samples().iter().map(|sample| sample.translation[0]).collect();
        assert_eq!(x, [1.0, 1.0, 2.0, 3.0, 3.0, 3.0], "held before its first key and after its last");
    }

    #[test]
    fn tables_from_elsewhere_stand_in_for_keys_and_are_still_checked() {
        let path = |name| BakedPath::new(Name::new(name), vec![PathSample::IDENTITY; 3]);
        let table = BakedTables::new(vec![RootSample::ZERO; 3], vec![path("blade"), path("fist")]).unwrap();
        let mut asked = Vec::new();
        let set = slice()
            .compile_with(|mv| {
                asked.push(mv.name.clone());
                Some(table.clone())
            })
            .expect("every hit rides `blade` or `fist`, which this table has");
        assert_eq!(asked.len(), slice().moves.len());
        assert_eq!(set.moves()[0].tables, table, "shorter than the move: its readings hold the last");

        let no_path = BakedTables::new(vec![RootSample::ZERO; 3], vec![]).unwrap();
        let issues = slice().compile_with(|_| Some(no_path.clone()));
        let Err(MoveSetError::Invalid(issues)) = issues else { panic!("compiled") };
        assert!(issues.iter().any(|issue| issue.track == "hitboxes" && issue.message.contains("`blade`")));
    }

    #[test]
    fn button_bits_match_their_order() {
        assert_eq!(Button::Light.bit(), 1);
        assert_eq!(Button::Dodge.bit(), 4);
        assert_eq!(Button::Jump.bit(), 128);
    }

    #[test]
    fn shapes_must_enclose_something() {
        assert!(Shape::Sphere { radius: 0.1 }.is_valid());
        assert!(!Shape::Sphere { radius: f32::NAN }.is_valid());
        assert!(Shape::Capsule { radius: 0.1, length: 0.0 }.is_valid());
        assert!(!Shape::Capsule { radius: 0.0, length: 1.0 }.is_valid());
        assert!(Shape::Box { half_extents: [0.1; 3] }.is_valid());
        assert!(!Shape::Box { half_extents: [0.1, 0.0, 0.1] }.is_valid());
    }
}
