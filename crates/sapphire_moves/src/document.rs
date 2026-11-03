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
use sapphire_time::NORMAL_SPEED;
use serde::{Deserialize, Serialize};

use crate::brain::{
    Against, BrainAttack, BrainDef, BrainPace, BrainRise, MAX_BRAIN_ATTACKS, Reach, Recent, Spacing,
    TickRange,
};
use crate::movement::ControllerProfile;
use crate::set::{
    Allow, ArmourWindow, Attack, Button, CancelMode, CancelWindow, CounterWindow, Cue, DodgeVolume,
    FighterDef, FighterHurtbox, HIT_LANDED, Hitbox, HurtboxChange, HurtboxState, IntentDef, Knockback,
    MAX_BUFFER_TICKS, MAX_HITSTOP_TICKS, MAX_HURTBOXES, MAX_INTENTS, MAX_MOVE_REACH, MAX_MOVE_TICKS,
    MAX_SLOW_MOTION_STEPS, MIN_SHAPE_EXTENT, MoveParts, MoveSet, MoveSetError, PerfectDodgeWindow, Reactions,
    Shape, Span, WarpWindow, Weight, WorldSlowMotion,
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
#[serde(deny_unknown_fields)]
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
    /// How this set's actors fight on their own. Without it, something else has to press their intents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brain: Option<BrainDocument>,
    pub moves: Vec<MoveDocument>,
}

impl Versioned for MoveSetDocument {
    /// 1: M2 G2.
    const VERSION: u32 = 1;
}

/// A kind of intent. See [`IntentDef`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// How it moves: any of [`ControllerProfile`]'s numbers, the rest being its defaults, and each
    /// within [`MOVEMENT_LIMITS`](crate::MOVEMENT_LIMITS). Left out, its actors move by the profile they
    /// spawned with. A live edit reaches a running fight on its next step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movement: Option<ControllerProfile>,
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

/// How an enemy fights. See [`BrainDef`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainDocument {
    pub spacing: Spacing,
    pub pace: BrainPace,
    pub patience: TickRange,
    pub rest: TickRange,
    pub recover: TickRange,
    pub attacks: Vec<BrainAttackDocument>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rise: Option<BrainRiseDocument>,
}

/// One attack a brain can choose. See [`BrainAttack`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainAttackDocument {
    pub intent: String,
    /// How much it wants this attack over the others. Left out, 1.
    #[serde(default = "one")]
    pub weight: f32,
    pub reach: Reach,
    #[serde(default)]
    pub against: Against,
    #[serde(default)]
    pub recent: Recent,
}

fn one() -> f32 {
    1.0
}

/// When a brain gets up. See [`BrainRise`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainRiseDocument {
    pub intent: String,
    pub during: String,
    pub from: u32,
    pub to: u32,
}

/// An intent starting a move. See [`Allow`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct MoveDocument {
    pub name: String,
    pub startup: u32,
    pub active: u32,
    pub recovery: u32,
    /// Root motion and hit paths, keyed by hand, from a clip, or from a clip with keys beside it.
    #[serde(default, skip_serializing_if = "MotionDocument::is_empty")]
    pub motion: MotionDocument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hitboxes: Vec<HitboxDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hurtboxes: Vec<HurtboxDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invulnerable: Vec<SpanDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub perfect_dodge: Vec<PerfectDodgeDocument>,
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
#[serde(deny_unknown_fields)]
pub struct SpanDocument {
    pub from: u32,
    pub to: u32,
}

/// A hit volume. See [`Hitbox`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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

/// A perfect-dodge window. See [`PerfectDodgeWindow`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerfectDodgeDocument {
    pub from: u32,
    pub to: u32,
    /// What a live swing has to touch to fire it. Left out, it's a window and nothing more. With it,
    /// the set needs a fighter, and every tick of the window has to be invulnerable: a dodge that can
    /// be hit isn't one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<DodgeVolumeDocument>,
    /// How it slows the world. Left out, it doesn't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_motion: Option<SlowMotionDocument>,
    /// What it lets an intent start. Left out, nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter: Option<CounterDocument>,
}

/// What a live swing has to touch. See [`DodgeVolume`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DodgeVolumeDocument {
    /// Metres from the root, in the root's space.
    #[serde(default)]
    pub offset: [f32; 3],
    /// A unit rotation from the root, as x, y, z, w.
    #[serde(default = "identity")]
    pub rotation: [f32; 4],
    pub shape: Shape,
}

/// How a perfect dodge slows the world. See [`WorldSlowMotion`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlowMotionDocument {
    /// Thousandths of the world's speed, below 1000, and a divisor of 1,000,000: 500, 250, 200, 125, 100…
    pub per_mille: u32,
    /// Real fixed steps, from 1 to [`MAX_SLOW_MOTION_STEPS`].
    pub steps: u16,
}

/// What a perfect dodge lets an intent start. See [`CounterWindow`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CounterDocument {
    /// Ticks of the dodger's clock it stays open, from 1 to [`MAX_MOVE_TICKS`].
    pub ticks: u32,
    pub allow: Vec<AllowDocument>,
}

/// A cancel window. See [`CancelWindow`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelDocument {
    pub from: u32,
    pub to: u32,
    pub allow: Vec<AllowDocument>,
}

/// A warp window. See [`WarpWindow`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct CueDocument {
    pub name: String,
    pub at: u32,
}

/// What a move's tables hold: see the [crate docs](crate).
///
/// Keys are by hand, and may be all a move needs. Values blend linearly between them and hold past
/// the last, and the root starts at zero whether or not a key says so. A move with a [`clip`] bakes
/// from it at cook time, and its keys still mean what they say: [`root`]'s replace the clip's root
/// motion, and [`paths`] sit beside those that follow the clip's bones.
///
/// [`clip`]: Self::clip
/// [`root`]: Self::root
/// [`paths`]: Self::paths
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionDocument {
    /// The clip this move plays, and what its tables take from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipMotionDocument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root: Vec<RootKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<PathDocument>,
}

impl MotionDocument {
    fn is_empty(&self) -> bool {
        self.clip.is_none() && self.root.is_empty() && self.paths.is_empty()
    }
}

/// A move's motion from a clip. See the [crate docs](crate)' "From a clip".
///
/// `source`, `name` and `rig` say which clip by what a person can read, as a sidecar names it, rather
/// than by the ID a cook derives from them. They are text here because this crate knows nothing of
/// assets: the cook reads them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipMotionDocument {
    /// The sidecar ID of a key-pose document or an animation source.
    pub source: String,
    /// Which of an animation source's clips. A key-pose document is one clip, and names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The sidecar ID of the rig it plays on: a retarget's `target`, or one of a key-pose document's
    /// rigs. Unset, a key-pose document's `rig` and an animation source's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rig: Option<String>,
    /// Where root motion comes from, when the move has no root keys.
    #[serde(default, skip_serializing_if = "RootSource::is_clip")]
    pub root: RootSource,
    /// Named paths that follow the clip's joints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<ClipPathDocument>,
}

/// Where a move with a clip takes its root motion from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootSource {
    /// The clip's root track: what its sidecar lifted, or a key-pose document's `root` and
    /// `root_turn`. None at all if it has no such track.
    #[default]
    Clip,
    /// None: the clip plays where the character stands, whatever its track says.
    InPlace,
}

impl RootSource {
    fn is_clip(&self) -> bool {
        *self == Self::Clip
    }
}

/// A named hit path along a joint of the clip's skeleton: a socket, with an offset. See [`BakedPath`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipPathDocument {
    pub name: String,
    pub follows: Follows,
    /// Metres from the joint, along its own axes, unscaled.
    #[serde(default)]
    pub offset: [f32; 3],
    /// A unit rotation from the joint, as x, y, z, w.
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub rotation: [f32; 4],
}

/// What a clip's path follows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Follows {
    /// A humanoid bone, as a key-pose document spells it (`RightHand`), whichever joint the rig's
    /// map says it is, so a document fits any rig with a map.
    Bone(String),
    /// A joint of the skeleton by its name, for a rig with no map, or what no bone names: a weapon's
    /// slot.
    Joint(String),
}

/// The root at one tick. See [`RootSample`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct PathDocument {
    pub name: String,
    pub keys: Vec<PathKey>,
}

/// A path at one tick. See [`PathSample`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

fn is_identity(rotation: &[f32; 4]) -> bool {
    *rotation == identity()
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
            self.add(
                track,
                format!("`{name}` has a shape thinner than {MIN_SHAPE_EXTENT} m or a size that isn't finite"),
            );
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
        } else if distance > MAX_MOVE_REACH {
            self.add(
                "hitboxes",
                format!("`{name}` has a knockback of {distance} m, past the limit of {MAX_MOVE_REACH}"),
            );
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
        if let Some(movement) = &fighter.movement {
            for issue in movement.document_issues() {
                self.add("movement", issue);
            }
        }
        FighterDef {
            health: fighter.health,
            poise: fighter.poise,
            poise_refill: fighter.poise_refill,
            hurtboxes,
            reactions,
            movement: fighter.movement,
        }
    }

    /// Checks a perfect-dodge window and resolves what it names. `invulnerable` is the move's i-frames
    /// as [`covered_runs`] gives them.
    fn perfect_dodge(
        &mut self,
        window: &PerfectDodgeDocument,
        ticks: u32,
        invulnerable: &[Span],
        known: &Known<'_>,
    ) -> PerfectDodgeWindow {
        const TRACK: &str = "perfect_dodge";
        let (from, to) = (window.from, window.to);
        let span = self.span(TRACK, from, to, ticks);
        let volume = window.volume.map(|volume| {
            if known.fighter.is_none() {
                self.add(
                    TRACK,
                    format!("window {from}..{to} has a volume, in a set with no fighter to dodge"),
                );
            }
            if !runs_cover(invulnerable, from, to) {
                self.add(
                    TRACK,
                    format!("window {from}..{to} has a volume, and isn't invulnerable throughout"),
                );
            }
            self.shape(TRACK, "its volume", volume.shape);
            if !volume.offset.iter().chain(&volume.rotation).all(|value| value.is_finite()) {
                self.add(TRACK, format!("window {from}..{to}'s volume has a place that isn't finite"));
            }
            let length_squared: f32 = volume.rotation.iter().map(|value| value * value).sum();
            if (length_squared - 1.0).abs() > UNIT_TOLERANCE {
                self.add(TRACK, format!("window {from}..{to}'s volume doesn't have a unit rotation"));
            }
            let local = PathSample { translation: volume.offset, rotation: volume.rotation };
            DodgeVolume { local, shape: volume.shape }
        });
        if volume.is_none() && (window.slow_motion.is_some() || window.counter.is_some()) {
            self.add(
                TRACK,
                format!("window {from}..{to} says what it does when it fires, with no volume to fire it"),
            );
        }
        let slow_motion = window.slow_motion.map(|slow| {
            let SlowMotionDocument { per_mille, steps } = slow;
            let inverse = NORMAL_SPEED * NORMAL_SPEED;
            if per_mille == 0 || per_mille >= NORMAL_SPEED || !inverse.is_multiple_of(per_mille) {
                self.add(
                    TRACK,
                    format!(
                        "window {from}..{to} slows the world to {per_mille}‰, which has to be below \
                         {NORMAL_SPEED} and divide {inverse}, so the one who dodged runs at exactly 1×"
                    ),
                );
            }
            if !(1..=MAX_SLOW_MOTION_STEPS).contains(&steps) {
                self.add(
                    TRACK,
                    format!("window {from}..{to} slows the world for {steps} steps, not 1 to {MAX_SLOW_MOTION_STEPS}"),
                );
            }
            WorldSlowMotion { per_mille, steps }
        });
        let counter = window.counter.as_ref().map(|counter| {
            if !(1..=MAX_MOVE_TICKS).contains(&counter.ticks) {
                let count = counter.ticks;
                self.add(
                    TRACK,
                    format!("window {from}..{to}'s counter lasts {count} ticks, not 1 to {MAX_MOVE_TICKS}"),
                );
            }
            if counter.allow.is_empty() {
                self.add(TRACK, format!("window {from}..{to}'s counter allows nothing"));
            }
            let allow = self.allows(TRACK, &counter.allow, known.intents, known.moves);
            CounterWindow { ticks: counter.ticks, allow }
        });
        PerfectDodgeWindow { span, volume, slow_motion, counter }
    }

    /// Checks a brain and resolves its names: every number holds what [`BrainDef`] says of it, an attack's
    /// intent starts something from neutral, and a rise presses inside a cancel window that gets up.
    fn brain(&mut self, brain: &BrainDocument, document: &MoveSetDocument) -> BrainDef {
        const TRACK: &str = "brain";
        let intent_at = |name: &str| document.intents.iter().position(|intent| intent.name == name);
        let index = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);

        let Spacing { near, far } = brain.spacing;
        if !(near.is_finite() && far.is_finite() && 0.0 <= near && near < far) {
            self.add(TRACK, format!("spacing {near}..{far} isn't a band of distances"));
        }
        let BrainPace { approach, retreat, circle, circle_share, turn_rate_degrees } = brain.pace;
        for (what, tilt) in
            [("approach", approach), ("retreat", retreat), ("circle", circle), ("circle_share", circle_share)]
        {
            if !(0.0..=1.0).contains(&tilt) {
                self.add(TRACK, format!("{what} is {tilt}, not within 0 to 1"));
            }
        }
        if !(turn_rate_degrees.is_finite() && turn_rate_degrees > 0.0) {
            self.add(TRACK, format!("turn_rate_degrees is {turn_rate_degrees}, not above 0"));
        }
        for (what, range) in [("patience", brain.patience), ("rest", brain.rest), ("recover", brain.recover)]
        {
            if range.min > range.max || range.max > MAX_MOVE_TICKS {
                let (min, max) = (range.min, range.max);
                self.add(TRACK, format!("{what} is {min}..={max} ticks, not a range up to {MAX_MOVE_TICKS}"));
            }
        }

        if brain.attacks.is_empty() {
            self.add(TRACK, "a brain needs an attack".to_owned());
        }
        if brain.attacks.len() > MAX_BRAIN_ATTACKS {
            let count = brain.attacks.len();
            self.add(TRACK, format!("{count} attacks, past the limit of {MAX_BRAIN_ATTACKS}"));
        }
        let mut seen = BTreeSet::new();
        let mut attacks = Vec::with_capacity(brain.attacks.len());
        for attack in &brain.attacks {
            let name = attack.intent.as_str();
            if !seen.insert(name) {
                self.add(TRACK, format!("attack `{name}` appears twice"));
            }
            let at = intent_at(name);
            if at.is_none() {
                self.add(TRACK, format!("attack `{name}` is not an intent this set declares"));
            } else if !document.neutral.iter().any(|allow| allow.intent == name) {
                self.add(
                    TRACK,
                    format!("attack `{name}` starts nothing from neutral, so pressing it does nothing"),
                );
            }
            self.brain_attack(name, attack);
            let built = BrainAttack::new(
                index(at.unwrap_or(0)),
                attack.weight,
                attack.reach,
                attack.against,
                attack.recent,
            );
            attacks.push(built);
        }
        let rise = brain.rise.as_ref().map(|rise| {
            let at = intent_at(&rise.intent);
            if at.is_none() {
                self.add(TRACK, format!("rise presses `{}`, not an intent this set declares", rise.intent));
            }
            let opens = document.moves.iter().find(|mv| mv.name == rise.during).map(|mv| {
                mv.cancels.iter().any(|window| {
                    window.from <= rise.from
                        && rise.to <= window.to
                        && window.allow.iter().any(|allow| allow.intent == rise.intent)
                })
            });
            let (intent, during, from, to) = (&rise.intent, &rise.during, rise.from, rise.to);
            match opens {
                None => self.add(TRACK, format!("rise happens during `{during}`, not a move in this set")),
                Some(false) => self.add(
                    TRACK,
                    format!("rise at {from}..{to} isn't inside a cancel window of `{during}` that takes `{intent}`"),
                ),
                Some(true) if from >= to => self.add(TRACK, format!("rise window {from}..{to} is empty")),
                Some(true) => {}
            }
            BrainRise::new(index(at.unwrap_or(0)), Name::intern(during), from, to)
        });
        BrainDef {
            spacing: brain.spacing,
            pace: brain.pace,
            patience: brain.patience,
            rest: brain.rest,
            recover: brain.recover,
            attacks,
            rise,
        }
    }

    /// Checks the numbers of one of a brain's attacks.
    fn brain_attack(&mut self, name: &str, attack: &BrainAttackDocument) {
        const TRACK: &str = "brain";
        if !(attack.weight.is_finite() && attack.weight >= 0.0) {
            self.add(TRACK, format!("attack `{name}`'s weight is {}, not 0 or above", attack.weight));
        }
        let Reach { near, best, far } = attack.reach;
        if !(near.is_finite() && far.is_finite() && near <= best && best <= far && near < far) {
            self.add(TRACK, format!("attack `{name}`'s reach {near}, {best}, {far} isn't in order"));
        }
        let Against { free, swinging, opening, reacting, untouchable } = attack.against;
        if ![free, swinging, opening, reacting, untouchable].iter().all(|at| at.is_finite() && *at >= 0.0) {
            self.add(TRACK, format!("attack `{name}` has a multiplier below 0 or not finite"));
        }
        let Recent { ticks, floor } = attack.recent;
        if !(0.0..=1.0).contains(&floor) && ticks > 0 {
            self.add(TRACK, format!("attack `{name}`'s recent floor is {floor}, not within 0 to 1"));
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
            } else if !key.offset.iter().chain([&key.yaw]).all(|value| value.abs() <= MAX_MOVE_REACH) {
                self.add(
                    "motion",
                    format!("root key at tick {} goes past the limit of {MAX_MOVE_REACH}", key.at),
                );
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
        if let Some(clip) = &motion.clip {
            self.clip(clip, motion);
            if self.issues.len() != before {
                return None;
            }
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

impl Checker {
    /// Checks a clip's block of a move: everything but what only its cook can read.
    fn clip(&mut self, clip: &ClipMotionDocument, motion: &MotionDocument) {
        if clip.source.is_empty() {
            self.add("motion", "the clip's `source` is empty".to_owned());
        }
        if clip.root == RootSource::InPlace && !motion.root.is_empty() {
            self.add(
                "motion",
                "the clip's `root` says `InPlace` and the move has root keys: say one or the other"
                    .to_owned(),
            );
        }
        let mut seen: BTreeSet<&str> = motion.paths.iter().map(|path| path.name.as_str()).collect();
        for path in &clip.paths {
            self.named("motion", &path.name);
            if !seen.insert(path.name.as_str()) {
                self.add("motion", format!("path `{}` appears twice", path.name));
            }
            let (Follows::Bone(joint) | Follows::Joint(joint)) = &path.follows;
            if joint.is_empty() {
                self.add("motion", format!("path `{}` follows a joint with no name", path.name));
            }
            if !path.offset.iter().chain(&path.rotation).all(|value| value.is_finite()) {
                self.add("motion", format!("path `{}` isn't finite", path.name));
            }
            let length_squared: f32 = path.rotation.iter().map(|value| value * value).sum();
            if (length_squared - 1.0).abs() > UNIT_TOLERANCE {
                self.add("motion", format!("path `{}` doesn't have a unit rotation", path.name));
            }
        }
    }

    /// `keyed`, what a move's keys bake to, with `baked`, what its clip did, under it: the clip's
    /// length must be the move's, since a move plays its clip tick for tick. See the [crate
    /// docs](crate).
    fn with_clip(
        &mut self,
        baked: BakedTables,
        keyed: &BakedTables,
        motion: &MotionDocument,
    ) -> Option<BakedTables> {
        let (clip_ticks, ticks) = (baked.ticks(), keyed.ticks());
        if clip_ticks != ticks {
            self.add(
                "motion",
                format!(
                    "the clip is {clip_ticks} ticks long and the phases add up to {ticks}: a move plays its \
                     clip tick for tick, so they must agree"
                ),
            );
            return None;
        }
        let tables = if motion.root.is_empty() { baked.root_samples() } else { keyed.root_samples() };
        let paths = baked.paths().iter().chain(keyed.paths()).cloned().collect();
        match BakedTables::new(tables.to_vec(), paths) {
            Ok(tables) => Some(tables),
            Err(error) => {
                self.add("motion", error.to_string());
                None
            }
        }
    }
}

/// The ticks `spans` cover, as runs in order that neither overlap nor touch. An empty or reversed span
/// covers nothing. Its cost is the spans', never the ticks between their ends: a document being refused
/// can say anything a `u32` holds there.
pub(crate) fn covered_runs(spans: &[Span]) -> Vec<Span> {
    let mut spans: Vec<Span> = spans.iter().copied().filter(|span| span.from < span.to).collect();
    spans.sort_by_key(|span| span.from);
    let mut runs: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        match runs.last_mut() {
            Some(run) if span.from <= run.to => run.to = run.to.max(span.to),
            _ => runs.push(span),
        }
    }
    runs
}

/// Whether [`covered_runs`] cover every tick of `from..to`, which an empty window has none of.
pub(crate) fn runs_cover(runs: &[Span], from: u32, to: u32) -> bool {
    let after = runs.partition_point(|run| run.from <= from);
    from >= to || after.checked_sub(1).is_some_and(|run| to <= runs[run].to)
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

impl MoveDocument {
    /// Its length: its three phases, end to end.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        self.startup.saturating_add(self.active).saturating_add(self.recovery)
    }
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

    /// Everything wrong with this document that it can say by itself: what only a cook can read of a
    /// clip stands in as a table with no motion on the clip's named paths, of the move's own length.
    #[must_use]
    pub fn check(&self) -> Vec<Issue> {
        let stand_in = |mv: &MoveDocument| {
            let Some(clip) = &mv.motion.clip else { return Ok(None) };
            let samples = mv.ticks() as usize + 1;
            let path = |path: &ClipPathDocument| {
                BakedPath::new(Name::intern(&path.name), vec![PathSample::IDENTITY; samples])
            };
            BakedTables::new(vec![RootSample::ZERO; samples], clip.paths.iter().map(path).collect())
                .map(Some)
                .map_err(|error| error.to_string())
        };
        match self.compile_with(stand_in) {
            Ok(_) => Vec::new(),
            Err(MoveSetError::Invalid(issues)) => issues,
            Err(error) => vec![Issue { in_move: None, track: "moves", message: error.to_string() }],
        }
    }

    /// Checks this document and resolves its names, with each move's tables baked from its keys. A
    /// move that names a clip is an issue here: only a cook can read one, and it comes through
    /// [`Self::compile_with`].
    pub fn compile(&self) -> Result<MoveSet, MoveSetError> {
        self.compile_with(|_| Ok(None))
    }

    /// [`Self::compile`] where there are no clips to bake from, as in a running app (M2 T2): a move that
    /// names a clip reads the tables its clip gave it in `running`, the set this document last became.
    /// So windows, how a move's length divides into phases and the keys laid over a clip all take effect
    /// with no cook, in the bytes a cook of this document makes.
    ///
    /// A move's length is its clip's, so changing it is the same issue a cook gives, and a new move or path
    /// says only a cook can bake it. What this can't see is an edit of a `clip` block itself, or a move's
    /// root keys taken away, since a running set holds what they made and not what they say: a cook is
    /// what brings those, as a new `.smoves`.
    pub fn compile_over(&self, running: &MoveSet) -> Result<MoveSet, MoveSetError> {
        self.compile_with(|mv| {
            let Some(clip) = &mv.motion.clip else { return Ok(None) };
            let Some((_, was)) = running.find(Name::intern(&mv.name)) else {
                return Err("this names a clip and the running moves don't have it: only a cook can bake it"
                    .to_owned());
            };
            let path = |path: &ClipPathDocument| {
                was.tables.path(Name::intern(&path.name)).cloned().ok_or_else(|| {
                    format!("the running moves don't have path `{}`: only a cook can bake it", path.name)
                })
            };
            let paths = clip.paths.iter().map(path).collect::<Result<Vec<_>, _>>()?;
            BakedTables::new(was.tables.root_samples().to_vec(), paths)
                .map(Some)
                .map_err(|error| error.to_string())
        })
    }

    /// Whether any of its moves names a clip.
    #[must_use]
    pub fn names_clips(&self) -> bool {
        self.moves.iter().any(|mv| mv.motion.clip.is_some())
    }

    /// [`Self::compile`], asking `tables` for each move's tables first: a cook that bakes them from
    /// a clip, or a test with a fixture. A move it answers `Ok(None)` for bakes from its keys, and an
    /// `Err` is an issue on that move's `motion`, reported with the rest.
    ///
    /// Tables for a move with a clip are what its clip gives, the clip's own length: this checks that
    /// against the move's phases and lays the move's keys over it. Tables for any other move stand in
    /// for its keys whole. Wherever tables come from, the hit paths named must be in them, and they
    /// must have the same bits on every architecture: see the [crate docs](crate).
    pub fn compile_with(
        &self,
        mut tables: impl FnMut(&MoveDocument) -> Result<Option<BakedTables>, String>,
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
        let brain = self.brain.as_ref().map(|brain| check.brain(brain, self));

        let mut names = BTreeSet::new();
        let mut moves = Vec::with_capacity(self.moves.len());
        for mv in &self.moves {
            check.in_move = Some(mv.name.clone());
            check.named("name", &mv.name);
            if !names.insert(mv.name.as_str()) {
                check.add("name", "two moves have this name".to_owned());
            }
            let ticks = mv.ticks();
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
            Ok(MoveSet::from_parts(intents, neutral, fighter, brain, moves))
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

/// A move's tables, whichever way they come.
fn compile_motion(
    mv: &MoveDocument,
    ticks: u32,
    check: &mut Checker,
    tables: &mut impl FnMut(&MoveDocument) -> Result<Option<BakedTables>, String>,
) -> Option<BakedTables> {
    let motion = &mv.motion;
    if motion.clip.is_none() {
        return match tables(mv) {
            Ok(Some(tables)) => Some(tables),
            Ok(None) => check.motion(motion, ticks),
            Err(message) => {
                check.add("motion", message);
                None
            }
        };
    }
    // A clip's block and the keys beside it are checked before a cook is asked for anything.
    let keyed = check.motion(motion, ticks)?;
    match tables(mv) {
        Ok(Some(baked)) => check.with_clip(baked, &keyed, motion),
        Ok(None) => {
            check.add("motion", "this names a clip, which only a cook can bake".to_owned());
            None
        }
        Err(message) => {
            check.add("motion", message);
            None
        }
    }
}

fn compile_move(
    mv: &MoveDocument,
    ticks: u32,
    known: &Known<'_>,
    check: &mut Checker,
    tables: &mut impl FnMut(&MoveDocument) -> Result<Option<BakedTables>, String>,
) -> Option<crate::set::Move> {
    let tables = compile_motion(mv, ticks, check, tables);
    if tables.as_ref().is_some_and(|tables| !tables.root_within_reach()) {
        check.add("motion", format!("the root goes past the limit of {MAX_MOVE_REACH}"));
    }

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
    let invulnerable_runs = covered_runs(&invulnerable);
    let perfect_dodge = mv
        .perfect_dodge
        .iter()
        .map(|window| check.perfect_dodge(window, ticks, &invulnerable_runs, known))
        .collect();
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
        assert_eq!(dodge.perfect_dodge.len(), 1);
        let perfect = &dodge.perfect_dodge[0];
        assert_eq!(perfect.span, Span { from: 1, to: 7 });
        assert!(perfect.volume.is_some());
        assert_eq!(perfect.slow_motion, Some(WorldSlowMotion { per_mille: 200, steps: 30 }));
        let counter = perfect.counter.as_ref().expect("a counter window");
        assert_eq!(counter.duration(), SimDuration::from_ticks(45));
        assert_eq!(counter.allow[0].target_name(), Name::new("dodge_counter"));
        let (_, dodge_intent) = set.intent(Name::new("dodge")).expect("declared");
        let (_, light_intent) = set.intent(Name::new("light")).expect("declared");
        assert!(dodge_intent.priority > light_intent.priority);
        assert_eq!(light_intent.buffer(), SimDuration::from_ticks(12));
        assert_eq!(MOVE_SET_EXTENSION, "smoves");
    }

    /// A fighter's `movement` is optional, names what it changes, and is checked with the rest: the issue
    /// says the section and the number, and everything wrong is said at once.
    #[test]
    fn a_fighters_movement_reads_and_is_checked_with_the_rest() {
        let with = |section: &str| {
            MoveSetDocument::parse(&SLICE.replacen("fighter: (", &format!("fighter: (\n{section},"), 1))
        };
        let good =
            with("movement: (run_speed: 7.5, coyote_ticks: 2)").expect("parses").compile().expect("compiles");
        let wanted = ControllerProfile { run_speed: 7.5, coyote_ticks: 2, ..ControllerProfile::default() };
        assert_eq!(good.movement(), Some(&wanted));
        assert_eq!(slice().compile().unwrap().movement(), None, "left out, it says nothing");

        let bad = with("movement: (run_speed: 600.0, gravity: -1.0)").expect("parses");
        let Err(MoveSetError::Invalid(issues)) = bad.compile() else { panic!("it should be refused") };
        let said: Vec<String> = issues.iter().map(ToString::to_string).collect();
        assert!(
            said.iter().any(|issue| issue.starts_with("`movement`: `run_speed` is 600"))
                && said.iter().any(|issue| issue.starts_with("`movement`: `gravity` is -1")),
            "{said:?}"
        );
        assert_eq!(bad.check().len(), issues.len(), "and a check says the same");

        let error = with("movement: (run_sped: 7.5)").expect_err("a misspelt number");
        assert!(error.to_string().contains("run_sped"), "{error}");
        let error = with("movment: (run_speed: 7.5)").expect_err("a misspelt section");
        assert!(error.to_string().contains("movment"), "{error}");
    }

    /// A running app's route takes the document's numbers, not what's running, and a set takes its bytes
    /// with it through a cook, where bad numbers read as damage.
    #[test]
    fn movement_goes_through_a_compile_over_and_a_cook() {
        let with = |number: f32| {
            let text =
                SLICE.replacen("fighter: (", &format!("fighter: (\nmovement: (run_speed: {number:?}),"), 1);
            MoveSetDocument::parse(&text).unwrap()
        };
        let running = with(6.5).compile().unwrap();
        let edited = with(7.5).compile_over(&running).unwrap();
        assert_eq!(edited.movement().unwrap().run_speed, 7.5);
        let back = MoveSet::from_cooked(&edited.to_cooked().unwrap()).unwrap();
        assert_eq!(back, edited);
        assert_ne!(*with(7.5).compile().unwrap().to_cooked().unwrap(), *running.to_cooked().unwrap());

        // Well-formed bytes that no document could have made.
        let mut bad = edited;
        bad.fighter_mut().movement =
            Some(ControllerProfile { run_speed: f32::NAN, ..ControllerProfile::default() });
        let cooked = bad.to_cooked().unwrap();
        let Err(MoveSetError::Damaged(said)) = MoveSet::from_cooked(&cooked) else {
            panic!("it should be refused")
        };
        assert!(said.contains("run_speed"), "{said}");
    }

    /// A misspelt track would otherwise read as no track at all: `cancles:` plays a move nothing can
    /// cancel out of and answers `as revision 2`, with nothing anywhere to say what went.
    #[test]
    fn a_field_the_format_has_no_name_for_is_refused() {
        for (field, typo) in [
            ("neutral:", "neutarl:"),
            ("fighter: (", "figther: ("),
            ("cancels:", "cancles:"),
            ("hitboxes:", "hitbox:"),
            ("attack: (", "atack: ("),
            ("invulnerable:", "invulnerables:"),
            ("hurtboxes:", "hurtbox:"),
            ("cues:", "cue:"),
            ("root:", "roots:"),
        ] {
            assert!(SLICE.contains(field), "the test content has no `{field}`");
            let error = MoveSetDocument::parse(&SLICE.replacen(field, typo, 1))
                .expect_err(&format!("`{typo}` should be refused"));
            let name = typo.split(':').next().unwrap();
            assert!(error.to_string().contains(name), "`{typo}` isn't named in: {error}");
        }
        for (place, field) in [
            ("buffer:", "bufferr: 1, buffer:"),
            ("startup:", "startp: 1, startup:"),
            ("from:", "form: 1, from:"),
            ("(intent:", "(intnet: \"x\", intent:"),
        ] {
            assert!(SLICE.contains(place), "the test content has no `{place}`");
            let name = field.split(':').next().unwrap().trim_start_matches('(');
            let error = MoveSetDocument::parse(&SLICE.replacen(place, field, 1))
                .expect_err(&format!("`{name}` should be refused"));
            assert!(error.to_string().contains(name), "`{name}` isn't named in: {error}");
        }
    }

    /// Everything the repository ships still reads, since the fields above are refused.
    #[test]
    fn every_document_shipped_parses() {
        for text in [
            SLICE,
            include_str!("../test_content/party.moves.ron"),
            include_str!("../../../content/combat/player/player.moves.ron"),
            include_str!("../../../content/combat/enemy/warrior.moves.ron"),
            include_str!("../../../content/lookdev/character/show.moves.ron"),
        ] {
            MoveSetDocument::parse(text).expect("a shipped document");
        }
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
                found: 6,
                current: 5
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
            None,
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
        /// A good brain, on the test content's moves, for one thing to be wrong with: it gets up from
        /// `light_1`, whose window 10..24 takes `light`.
        fn brain(document: &mut MoveSetDocument) -> &mut BrainDocument {
            document.brain.get_or_insert_with(good_brain)
        }
        fn attack(document: &mut MoveSetDocument) -> &mut AttackDocument {
            light(document).hitboxes[0].attack.as_mut().expect("in the content")
        }
        fn fighter(document: &mut MoveSetDocument) -> &mut FighterDocument {
            document.fighter.as_mut().expect("in the content")
        }
        fn dodge(document: &mut MoveSetDocument) -> &mut MoveDocument {
            document.moves.iter_mut().find(|mv| mv.name == "dodge").expect("in the content")
        }
        fn volume(document: &mut MoveSetDocument) -> &mut DodgeVolumeDocument {
            dodge(document).perfect_dodge[0].volume.as_mut().expect("in the content")
        }
        fn slow(document: &mut MoveSetDocument) -> &mut SlowMotionDocument {
            dodge(document).perfect_dodge[0].slow_motion.as_mut().expect("in the content")
        }
        fn counter(document: &mut MoveSetDocument) -> &mut CounterDocument {
            dodge(document).perfect_dodge[0].counter.as_mut().expect("in the content")
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
                "a knockback out of the world",
                |d| attack(d).knockback.distance = 1e30,
                &["light_1", "hitboxes", "light_1_hit", "knockback", "limit of 1000"],
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
                "a root key out of the world",
                |d| light(d).motion.root[0].offset[2] = 1e30,
                &["light_1", "motion", "root key", "limit of 1000"],
            ),
            (
                "a root key turning without end",
                |d| light(d).motion.root[0].yaw = -1e30,
                &["light_1", "motion", "root key", "limit of 1000"],
            ),
            (
                "a perfect-dodge window past the move's end",
                |d| dodge(d).perfect_dodge[0].to = 99,
                &["dodge", "perfect_dodge", "1..99", "past the move's end at tick 22"],
            ),
            (
                "a perfect-dodge window that can be hit through",
                |d| dodge(d).invulnerable[0].from = 2,
                &["dodge", "perfect_dodge", "window 1..7", "isn't invulnerable throughout"],
            ),
            (
                "a perfect-dodge window with a gap in its invulnerability",
                |d| {
                    dodge(d).invulnerable =
                        vec![SpanDocument { from: 1, to: 3 }, SpanDocument { from: 4, to: 14 }];
                },
                &["dodge", "perfect_dodge", "isn't invulnerable throughout"],
            ),
            (
                "a perfect-dodge volume in a set with no fighter",
                |d| {
                    d.fighter = None;
                    dodge(d).hurtboxes.clear();
                },
                &["dodge", "perfect_dodge", "no fighter"],
            ),
            (
                "a perfect-dodge volume with no size",
                |d| volume(d).shape = Shape::Sphere { radius: 0.0 },
                &["dodge", "perfect_dodge", "its volume", "thinner than"],
            ),
            (
                "a perfect-dodge volume somewhere that isn't",
                |d| volume(d).offset[1] = f32::NAN,
                &["dodge", "perfect_dodge", "volume", "isn't finite"],
            ),
            (
                "a perfect-dodge volume with a rotation that isn't unit",
                |d| volume(d).rotation = [0.0, 0.0, 0.0, 2.0],
                &["dodge", "perfect_dodge", "volume", "unit rotation"],
            ),
            (
                "slow motion with nothing to fire it",
                |d| {
                    let window = &mut dodge(d).perfect_dodge[0];
                    (window.volume, window.counter) = (None, None);
                },
                &["dodge", "perfect_dodge", "no volume to fire it"],
            ),
            (
                "a counter with nothing to fire it",
                |d| {
                    let window = &mut dodge(d).perfect_dodge[0];
                    (window.volume, window.slow_motion) = (None, None);
                },
                &["dodge", "perfect_dodge", "no volume to fire it"],
            ),
            (
                "slow motion that stops the world",
                |d| slow(d).per_mille = 0,
                &["dodge", "perfect_dodge", "0‰", "below 1000"],
            ),
            (
                "slow motion that doesn't slow it",
                |d| slow(d).per_mille = 1000,
                &["dodge", "perfect_dodge", "1000‰", "below 1000"],
            ),
            (
                "slow motion a clock can't exactly undo",
                |d| slow(d).per_mille = 300,
                &["dodge", "perfect_dodge", "300‰", "exactly 1×"],
            ),
            (
                "slow motion for no time",
                |d| slow(d).steps = 0,
                &["dodge", "perfect_dodge", "0 steps", "not 1 to 600"],
            ),
            (
                "slow motion for too long",
                |d| slow(d).steps = 601,
                &["dodge", "perfect_dodge", "601 steps", "not 1 to 600"],
            ),
            (
                "a counter window of no length",
                |d| counter(d).ticks = 0,
                &["dodge", "perfect_dodge", "counter lasts 0 ticks"],
            ),
            (
                "a counter window past any move's length",
                |d| counter(d).ticks = MAX_MOVE_TICKS + 1,
                &["dodge", "perfect_dodge", "counter lasts 3601 ticks"],
            ),
            (
                "a counter window that allows nothing",
                |d| counter(d).allow.clear(),
                &["dodge", "perfect_dodge", "counter allows nothing"],
            ),
            (
                "a counter to a move that doesn't exist",
                |d| counter(d).allow[0].target = "ghost".to_owned(),
                &["dodge", "perfect_dodge", "`ghost`", "not a move"],
            ),
            (
                "a counter on an undeclared intent",
                |d| counter(d).allow[0].intent = "ghost".to_owned(),
                &["dodge", "perfect_dodge", "`ghost`", "not an intent"],
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
            (
                "a spacing the wrong way round",
                |d| brain(d).spacing = Spacing { near: 3.0, far: 2.0 },
                &["`brain`", "spacing 3..2"],
            ),
            (
                "a spacing that isn't finite",
                |d| brain(d).spacing = Spacing { near: 0.0, far: f32::INFINITY },
                &["`brain`", "spacing"],
            ),
            ("a tilt past the stick", |d| brain(d).pace.approach = 1.5, &["`brain`", "approach is 1.5"]),
            ("a tilt that's no number", |d| brain(d).pace.circle = f32::NAN, &["`brain`", "circle is NaN"]),
            (
                "a turn that goes nowhere",
                |d| brain(d).pace.turn_rate_degrees = 0.0,
                &["`brain`", "turn_rate_degrees"],
            ),
            (
                "a patience the wrong way round",
                |d| brain(d).patience = TickRange { min: 9, max: 3 },
                &["`brain`", "patience is 9..=3"],
            ),
            (
                "a rest too long",
                |d| brain(d).rest = TickRange { min: 0, max: MAX_MOVE_TICKS + 1 },
                &["`brain`", "rest is"],
            ),
            (
                "a recover the wrong way round",
                |d| brain(d).recover = TickRange { min: 2, max: 1 },
                &["`brain`", "recover is"],
            ),
            ("a brain with no attacks", |d| brain(d).attacks.clear(), &["`brain`", "needs an attack"]),
            (
                "a brain with too many attacks",
                |d| {
                    let copy = brain(d).attacks[0].clone();
                    brain(d).attacks = vec![copy; MAX_BRAIN_ATTACKS + 1];
                },
                &["`brain`", "9 attacks", "past the limit"],
            ),
            (
                "an attack twice",
                |d| {
                    let copy = brain(d).attacks[0].clone();
                    brain(d).attacks.push(copy);
                },
                &["`brain`", "`light`", "appears twice"],
            ),
            (
                "an attack that's no intent",
                |d| brain(d).attacks[0].intent = "ghost".to_owned(),
                &["`brain`", "`ghost`", "not an intent"],
            ),
            (
                "an attack that starts nothing",
                |d| {
                    brain(d);
                    d.neutral.retain(|allow| allow.intent != "light");
                },
                &["`brain`", "`light`", "starts nothing from neutral"],
            ),
            (
                "an attack's bad weight",
                |d| brain(d).attacks[0].weight = -1.0,
                &["`brain`", "`light`", "weight is -1"],
            ),
            (
                "a reach out of order",
                |d| brain(d).attacks[0].reach = Reach { near: 2.0, best: 1.0, far: 3.0 },
                &["`brain`", "`light`", "reach 2, 1, 3"],
            ),
            (
                "a reach with no width",
                |d| brain(d).attacks[0].reach = Reach { near: 2.0, best: 2.0, far: 2.0 },
                &["`brain`", "`light`", "reach"],
            ),
            (
                "a multiplier below nothing",
                |d| brain(d).attacks[0].against.opening = -1.0,
                &["`brain`", "`light`", "multiplier"],
            ),
            (
                "a recency floor past 1",
                |d| brain(d).attacks[0].recent = Recent { ticks: 5, floor: 2.0 },
                &["`brain`", "`light`", "floor is 2"],
            ),
            (
                "a rise pressing no intent",
                |d| brain(d).rise.as_mut().expect("in this one").intent = "ghost".to_owned(),
                &["`brain`", "rise presses `ghost`"],
            ),
            (
                "a rise from no move",
                |d| brain(d).rise.as_mut().expect("in this one").during = "ghost".to_owned(),
                &["`brain`", "rise happens during `ghost`"],
            ),
            (
                "a rise past its window",
                |d| brain(d).rise.as_mut().expect("in this one").to = 25,
                &["`brain`", "rise at 12..25", "cancel window of `light_1`"],
            ),
            (
                "a rise with no room",
                |d| brain(d).rise.as_mut().expect("in this one").to = 12,
                &["`brain`", "rise window 12..12 is empty"],
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

    fn good_brain() -> BrainDocument {
        BrainDocument {
            spacing: Spacing { near: 1.0, far: 2.0 },
            pace: BrainPace {
                approach: 0.7,
                retreat: 0.3,
                circle: 0.3,
                circle_share: 0.5,
                turn_rate_degrees: 300.0,
            },
            patience: TickRange { min: 10, max: 20 },
            rest: TickRange { min: 10, max: 20 },
            recover: TickRange { min: 5, max: 10 },
            attacks: vec![BrainAttackDocument {
                intent: "light".to_owned(),
                weight: 1.0,
                reach: Reach { near: 0.0, best: 1.0, far: 2.0 },
                against: Against::default(),
                recent: Recent::default(),
            }],
            rise: Some(BrainRiseDocument {
                intent: "light".to_owned(),
                during: "light_1".to_owned(),
                from: 12,
                to: 20,
            }),
        }
    }

    /// A brain's section compiles and cooks with the rest, and a replay's hash sees every number.
    #[test]
    fn a_brain_compiles_and_cooks_by_its_intents_places() {
        let mut document = slice();
        document.brain = Some(good_brain());
        let set = document.compile().expect("a good brain");
        let brain = set.brain().expect("it's there");
        let (at, intent) = set.intent(Name::new("light")).expect("declared");
        assert_eq!(intent.name(), Name::new("light"));
        assert_eq!(brain.attacks[0].intent() as usize, at);
        assert_eq!(brain.rise.expect("there").intent() as usize, at);
        assert_eq!(brain.rise.expect("there").during(), Name::new("light_1"));
        let cooked = set.to_cooked().expect("cooks");
        assert_eq!(MoveSet::from_cooked(&cooked).expect("reads").brain(), set.brain());

        // A number moving moves the bytes.
        let mut tuned = document.clone();
        tuned.brain.as_mut().expect("there").patience.max += 1;
        let other = tuned.compile().expect("still good").to_cooked().expect("cooks");
        assert_ne!(other.as_slice(), cooked.as_slice());
        assert!(slice().compile().expect("compiles").brain().is_none(), "the test content has none");
    }

    /// A move of ordinary length whose dodge and i-frames say they run to the end of a `u32`, behind
    /// plenty of i-frames that don't cover it. Walking its ticks took minutes to say what's wrong, so this
    /// runs off to one side, against a limit it would only reach by doing that again.
    #[test]
    fn a_refused_dodge_costs_its_windows_not_their_ticks() {
        let mut document = slice();
        let dodge = document.moves.iter_mut().find(|mv| mv.name == "dodge").expect("in the content");
        dodge.perfect_dodge[0].from = 0;
        dodge.perfect_dodge[0].to = u32::MAX;
        dodge.invulnerable = vec![SpanDocument { from: 4_000_000_000, to: 4_000_000_001 }; 64];
        dodge.invulnerable.push(SpanDocument { from: 0, to: u32::MAX });

        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || said.send(document.check()));
        let issues = heard
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("what's wrong takes no time to find, whatever the numbers say");
        let said: Vec<String> = issues.iter().map(ToString::to_string).collect();
        assert!(
            said.iter().any(|issue| issue.contains("`dodge`, track `perfect_dodge`")
                && issue.contains(&format!("0..{}", u32::MAX))
                && issue.contains("past the move's end")),
            "{said:?}"
        );
        assert!(
            said.iter().any(|issue| issue.contains("`dodge`, track `invulnerable`")
                && issue.contains("past the move's end")),
            "{said:?}"
        );
    }

    /// Tables from a cook are held to what keys are, or the set it writes would read back as damaged.
    #[test]
    fn a_root_from_elsewhere_stays_in_the_world() {
        let document = slice();
        let far = |mv: &MoveDocument| {
            let mut root = vec![RootSample::ZERO; mv.ticks() as usize + 1];
            root[1].yaw = MAX_MOVE_REACH * 2.0;
            let paths =
                mv.motion.paths.iter().map(|path| {
                    BakedPath::new(Name::intern(&path.name), vec![PathSample::IDENTITY; root.len()])
                });
            let paths = paths.collect();
            BakedTables::new(root, paths).map(Some).map_err(|error| error.to_string())
        };
        let Err(MoveSetError::Invalid(issues)) = document.compile_with(far) else {
            panic!("it should be refused");
        };
        assert!(issues.iter().all(|issue| issue.track == "motion" && issue.message.contains("limit")));
    }

    /// A dodge's volume against what's invulnerable, each row a window and its i-frames, with or
    /// without a gap: runs that touch or overlap in any order, and empty and reversed ones, which cover
    /// nothing.
    #[test]
    fn a_dodges_volume_needs_iframes_on_every_tick() {
        const THROUGHOUT: &str = "isn't invulnerable throughout";
        type Ticks = (u32, u32);
        #[rustfmt::skip]
        let rows: &[(Ticks, &[Ticks], bool)] = &[
            ((1, 7), &[(1, 7)], true),
            ((1, 7), &[(0, 26)], true),
            ((1, 7), &[(1, 4), (4, 7)], true),
            ((1, 7), &[(4, 7), (1, 4)], true),
            ((1, 7), &[(1, 5), (3, 7)], true),
            ((1, 7), &[(3, 7), (1, 5), (2, 3)], true),
            ((1, 7), &[(1, 7), (9, 9), (12, 10)], true),
            ((1, 7), &[(1, 3), (2, 2), (9, 2), (3, 7)], true),
            ((1, 7), &[(1, 5), (2, 3), (5, 7)], true),
            ((1, 7), &[], false),
            ((1, 7), &[(2, 7)], false),
            ((1, 7), &[(1, 6)], false),
            ((1, 7), &[(1, 3), (4, 7)], false),
            ((1, 7), &[(4, 7), (1, 3)], false),
            ((1, 7), &[(1, 3), (3, 3), (4, 7)], false),
            ((1, 7), &[(1, 3), (7, 3), (4, 7)], false),
            ((1, 7), &[(7, 1)], false),
            ((1, 2), &[(1, 2)], true),
            ((1, 2), &[(0, 1), (2, 3)], false),
        ];
        for &((from, to), iframes, covered) in rows {
            let mut document = slice();
            let dodge = document.moves.iter_mut().find(|mv| mv.name == "dodge").expect("in the content");
            (dodge.perfect_dodge[0].from, dodge.perfect_dodge[0].to) = (from, to);
            dodge.invulnerable = iframes.iter().map(|&(from, to)| SpanDocument { from, to }).collect();
            let issues = document.check();
            let found = issues.iter().any(|issue| issue.to_string().contains(THROUGHOUT));
            assert_eq!(found, !covered, "{from}..{to} under {iframes:?}: {issues:?}");
        }

        // An empty window has no ticks to cover, and says what's wrong with it by itself.
        let mut document = slice();
        let dodge = document.moves.iter_mut().find(|mv| mv.name == "dodge").expect("in the content");
        (dodge.perfect_dodge[0].from, dodge.perfect_dodge[0].to) = (5, 5);
        dodge.invulnerable.clear();
        let said: Vec<String> = document.check().iter().map(ToString::to_string).collect();
        assert!(said.iter().any(|issue| issue.contains("window 5..5 is empty")), "{said:?}");
        assert!(!said.iter().any(|issue| issue.contains(THROUGHOUT)), "{said:?}");
    }

    /// Bytes from anywhere: a brain that points at no intent is damaged, as a move's window would be.
    #[test]
    fn a_cooked_brain_naming_no_intent_is_refused() {
        let mut document = slice();
        document.brain = Some(good_brain());
        let set = document.compile().expect("good");
        let mut brain = set.brain().expect("there").clone();
        brain.attacks[0] =
            BrainAttack::new(99, 1.0, brain.attacks[0].reach, Against::default(), Recent::default());
        let bad = MoveSet::from_parts(
            set.intents().to_vec(),
            set.neutral().to_vec(),
            set.fighter().cloned(),
            Some(brain),
            set.moves().to_vec(),
        );
        let bytes = bad.to_cooked().expect("cooks");
        assert!(matches!(MoveSet::from_cooked(&bytes), Err(MoveSetError::Damaged(_))));
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
        let document =
            MoveSetDocument { intents: vec![], neutral: vec![], fighter: None, brain: None, moves: vec![] };
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
        for window in document.moves.iter_mut().flat_map(|mv| &mut mv.perfect_dodge) {
            (window.volume, window.slow_motion, window.counter) = (None, None, None);
        }
        let text = document.to_text().unwrap();
        assert!(!text.contains("fighter:") && !text.contains("attack:"), "{text}");
        assert!(text.contains("perfect_dodge:") && !text.contains("volume:"), "a window and nothing more");
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
        assert!(matches!(with("volume:", "volme:"), Err(MoveSetError::Document(_))));
        assert!(matches!(with("per_mille:", "per_mile:"), Err(MoveSetError::Document(_))));
        assert!(matches!(with("counter: (ticks:", "counter: (tick:"), Err(MoveSetError::Document(_))));
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
            let set =
                MoveSet::from_parts(good.intents().to_vec(), good.neutral().to_vec(), fighter, None, moves);
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

        fn perfect(moves: &mut [Move]) -> &mut PerfectDodgeWindow {
            let dodge = moves.iter_mut().find(|mv| mv.name() == Name::new("dodge")).unwrap();
            &mut dodge.perfect_dodge[0]
        }
        let span = rebuilt(|moves, _| perfect(moves).span.to = 999);
        assert!(matches!(span, Err(MoveSetError::Damaged(text)) if text.contains("window")));
        let counter = rebuilt(|moves, _| {
            let allow = Allow::new(Name::new("light"), 999, Name::new("x"), CancelMode::Buffered);
            perfect(moves).counter = Some(CounterWindow { ticks: 1, allow: vec![allow] });
        });
        assert!(matches!(counter, Err(MoveSetError::Damaged(text)) if text.contains("counter")));
        for per_mille in [0, 1000, 5000] {
            let slowed = WorldSlowMotion { per_mille, steps: 1 };
            let set = {
                let mut moves = good.moves().to_vec();
                perfect(&mut moves).slow_motion = Some(slowed);
                MoveSet::from_parts(
                    good.intents().to_vec(),
                    good.neutral().to_vec(),
                    good.fighter().cloned(),
                    None,
                    moves,
                )
            };
            let slow = MoveSet::from_cooked(&set.to_cooked().unwrap());
            assert!(matches!(slow, Err(MoveSetError::Damaged(text)) if text.contains("slow")), "{per_mille}");
        }
    }

    /// Everything a perfect dodge adds is optional: a window can be just a window.
    #[test]
    fn a_perfect_dodge_window_is_a_window_and_what_it_adds_is_optional() {
        let compiled = |windows: &str| {
            let text = format!(
                r#"(
                    type: "sapphire::moves::MoveSetDocument",
                    version: 1,
                    data: (
                        intents: [],
                        fighter: (health: 1, hurtboxes: [(name: "body", shape: Sphere(radius: 0.1))]),
                        moves: [(
                            name: "dodge",
                            startup: 0,
                            active: 20,
                            recovery: 0,
                            invulnerable: [(from: 0, to: 20)],
                            perfect_dodge: [{windows}],
                        )],
                    ),
                )"#
            );
            let set = MoveSetDocument::parse(&text).unwrap().compile().unwrap();
            let again = MoveSetDocument::parse(&text).unwrap().to_text().unwrap();
            assert_eq!(MoveSetDocument::parse(&again).unwrap().compile().unwrap(), set, "{again}");
            set.moves()[0].perfect_dodge.clone()
        };
        let plain = compiled("(from: 1, to: 7)");
        assert_eq!(plain[0].span, Span { from: 1, to: 7 });
        assert_eq!((plain[0].volume, plain[0].slow_motion, &plain[0].counter), (None, None, &None));

        let sensed = compiled("(from: 2, to: 3, volume: (shape: Sphere(radius: 0.5)))");
        let volume = sensed[0].volume.unwrap();
        assert_eq!(volume.local, PathSample::IDENTITY, "at the root, unturned, unless it says");
        assert_eq!(volume.shape, Shape::Sphere { radius: 0.5 });
        assert_eq!((sensed[0].slow_motion, &sensed[0].counter), (None, &None));

        let sorted = compiled("(from: 9, to: 10), (from: 2, to: 3)");
        assert_eq!(sorted[0].span.from, 2, "windows sort by when they open");
        let divisors =
            [800, 625, 500, 400, 320, 250, 200, 160, 125, 100, 80, 50, 40, 25, 20, 10, 8, 5, 4, 2, 1];
        for per_mille in divisors {
            let text = format!(
                "(from: 1, to: 7, volume: (shape: Sphere(radius: 0.5)), \
                 slow_motion: (per_mille: {per_mille}, steps: 1))"
            );
            assert_eq!(compiled(&text)[0].slow_motion.unwrap().per_mille, per_mille);
        }
    }

    #[test]
    fn keys_blend_linearly_and_hold_past_the_last() {
        let motion = MotionDocument {
            clip: None,
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
                Ok(Some(table.clone()))
            })
            .expect("every hit rides `blade` or `fist`, which this table has");
        assert_eq!(asked.len(), slice().moves.len());
        assert_eq!(set.moves()[0].tables, table, "shorter than the move: its readings hold the last");

        let no_path = BakedTables::new(vec![RootSample::ZERO; 3], vec![]).unwrap();
        let issues = slice().compile_with(|_| Ok(Some(no_path.clone())));
        let Err(MoveSetError::Invalid(issues)) = issues else { panic!("compiled") };
        assert!(issues.iter().any(|issue| issue.track == "hitboxes" && issue.message.contains("`blade`")));
    }

    /// A set of one move from a clip, 10 ticks long, on which each test edits what it's about.
    fn from_a_clip() -> MoveSetDocument {
        MoveSetDocument::parse(
            r#"(
                type: "sapphire::moves::MoveSetDocument",
                version: 1,
                data: (
                    intents: [(name: "light", button: Light, buffer: 12)],
                    neutral: [(intent: "light", move: "swing")],
                    moves: [(
                        name: "swing",
                        startup: 2,
                        active: 3,
                        recovery: 5,
                        motion: (
                            clip: (
                                source: "15af86bb-67b3-4208-bc54-1618aabf450b",
                                paths: [(name: "blade", follows: Bone("RightHand"), offset: (0.0, 0.0, 0.9))],
                            ),
                            paths: [(name: "shockwave", keys: [(at: 0, offset: (0.0, 0.5, 1.0))])],
                        ),
                        hitboxes: [
                            (name: "swing_hit", from: 2, to: 5, path: "blade", shape: Sphere(radius: 0.3)),
                            (name: "wave_hit", from: 2, to: 3, path: "shockwave", shape: Sphere(radius: 0.5)),
                        ],
                    )],
                ),
            )"#,
        )
        .expect("parses")
    }

    /// What a cook's bake answers: the clip's own root motion, and its named path.
    fn baked(ticks: usize) -> BakedTables {
        let root = (0..=ticks)
            .map(|tick| RootSample { translation: [0.0, 0.0, tick as f32 * 0.1], yaw: 0.0 })
            .collect();
        let sample = PathSample { translation: [0.1, 1.0, 0.2], rotation: identity() };
        BakedTables::new(root, vec![BakedPath::new(Name::new("blade"), vec![sample; ticks + 1])]).unwrap()
    }

    fn issues_of(document: &MoveSetDocument, tables: BakedTables) -> String {
        let Err(MoveSetError::Invalid(issues)) = document.compile_with(|_| Ok(Some(tables.clone()))) else {
            panic!("compiled");
        };
        issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn a_clips_block_round_trips_through_its_text_leaving_defaults_out() {
        let document = from_a_clip();
        let text = document.to_text().unwrap();
        assert_eq!(MoveSetDocument::parse(&text).unwrap(), document);
        // Its hand-keyed path comes after the block, and a key says its rotation in full.
        let (block, _) = text.split_once("shockwave").unwrap();
        for quiet in ["rig:", "root: Clip", "rotation:", "name: None"] {
            assert!(!block.contains(quiet), "{quiet} is the default:\n{block}");
        }
        let clip = document.moves[0].motion.clip.as_ref().unwrap();
        assert_eq!(clip.root, RootSource::Clip);
        assert_eq!(clip.paths[0].rotation, identity());
        assert_eq!(clip.paths[0].follows, Follows::Bone("RightHand".to_owned()));
    }

    /// A document with no cook behind it can check everything but what only a cook can read, and can't
    /// compile.
    #[test]
    fn a_clip_is_checked_without_a_cook_and_compiled_only_with_one() {
        assert_eq!(from_a_clip().check(), []);

        let mut wrong_path = from_a_clip();
        wrong_path.moves[0].hitboxes[0].path = "ghost".to_owned();
        let issues = wrong_path.check();
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].message.contains("`ghost`"), "{}", issues[0]);

        let Err(MoveSetError::Invalid(issues)) = from_a_clip().compile() else { panic!("compiled") };
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].track, "motion");
        assert!(issues[0].to_string().contains("swing"), "{}", issues[0]);
        assert!(issues[0].message.contains("only a cook"), "{}", issues[0]);
    }

    /// Over the set it last became, a document of clips needs no cook for what its clips don't hold, and
    /// makes the bytes a cook would. What only a cook can do says so (M2 T2).
    #[test]
    fn a_running_set_answers_for_its_clips() {
        let cook =
            |document: &MoveSetDocument| document.compile_with(|_| Ok(Some(baked(10)))).expect("cooks");
        let bytes = |set: &MoveSet| set.to_cooked().expect("writes").into_vec();
        let running = cook(&from_a_clip());
        assert_eq!(bytes(&from_a_clip().compile_over(&running).expect("the same")), bytes(&running));

        let mut later = from_a_clip();
        later.moves[0].startup = 3;
        later.moves[0].recovery = 4;
        later.moves[0].hitboxes[0].from = 3;
        later.moves[0].motion.paths[0].keys[0].offset = [0.0, 0.7, 1.0];
        assert_eq!(bytes(&later.compile_over(&running).expect("no cook needed")), bytes(&cook(&later)));

        // Keys beside the clip's root take over from it, as in a cook.
        let mut rooted = from_a_clip();
        rooted.moves[0].motion.root = vec![RootKey { at: 10, offset: [0.0, 0.0, 2.0], yaw: 0.0 }];
        assert_eq!(bytes(&rooted.compile_over(&running).expect("no cook needed")), bytes(&cook(&rooted)));

        let issue = |document: &MoveSetDocument| {
            let Err(MoveSetError::Invalid(issues)) = document.compile_over(&running) else {
                panic!("compiled")
            };
            assert_eq!(issues.len(), 1, "{issues:?}");
            assert_eq!(issues[0].track, "motion");
            issues[0].to_string()
        };
        let mut longer = from_a_clip();
        longer.moves[0].recovery += 1;
        assert!(issue(&longer).contains("the clip is 10 ticks long"), "{}", issue(&longer));

        let mut new_path = from_a_clip();
        let clip = new_path.moves[0].motion.clip.as_mut().expect("a clip");
        clip.paths[0].name = "off_hand".to_owned();
        new_path.moves[0].hitboxes[0].path = "off_hand".to_owned();
        assert!(issue(&new_path).contains("path `off_hand`: only a cook"), "{}", issue(&new_path));

        let mut new_move = from_a_clip();
        new_move.moves[0].name = "sweep".to_owned();
        new_move.neutral[0].target = "sweep".to_owned();
        assert!(issue(&new_move).contains("only a cook"), "{}", issue(&new_move));

        let mut keyed = from_a_clip();
        keyed.moves[0].motion.clip = None;
        keyed.moves[0].hitboxes.remove(0);
        assert!(keyed.compile_over(&running).is_ok(), "keys need no clip");
    }

    /// A move plays its clip tick for tick, so nothing retimes it: a clip of another length is the
    /// document's error, either way, saying both lengths. With a shorter table a hand-keyed move would
    /// hold its last sample, and a clip's mustn't.
    #[test]
    fn a_clip_must_be_as_long_as_its_move() {
        let set = from_a_clip().compile_with(|_| Ok(Some(baked(10)))).expect("the same length");
        assert_eq!(set.moves()[0].ticks(), 10);
        assert_eq!(set.moves()[0].tables.ticks(), 10);
        for ticks in [9, 11] {
            let text = issues_of(&from_a_clip(), baked(ticks));
            assert!(text.contains(&format!("the clip is {ticks} ticks long")), "{text}");
            assert!(text.contains("add up to 10"), "{text}");
            assert!(text.contains("swing") && text.contains("motion"), "{text}");
        }
    }

    /// The clip's root and bone paths with what the move keys beside them; root keys replace its root.
    #[test]
    fn keys_lie_over_a_clip() {
        let set = from_a_clip().compile_with(|_| Ok(Some(baked(10)))).unwrap();
        let tables = &set.moves()[0].tables;
        assert_eq!(tables.root_samples()[10].translation, [0.0, 0.0, 1.0], "the clip's root");
        let blade = tables.path(Name::new("blade")).expect("from the clip");
        assert_eq!(blade.samples()[4].translation, [0.1, 1.0, 0.2]);
        let wave = tables.path(Name::new("shockwave")).expect("keyed beside it");
        assert_eq!(wave.samples()[10].translation, [0.0, 0.5, 1.0]);

        let mut keyed = from_a_clip();
        keyed.moves[0].motion.root = vec![RootKey { at: 5, offset: [2.0, 0.0, 0.0], yaw: 0.5 }];
        let set = keyed.compile_with(|_| Ok(Some(baked(10)))).unwrap();
        let tables = &set.moves()[0].tables;
        assert_eq!(tables.root_samples()[10].translation, [2.0, 0.0, 0.0], "keys replace it");
        assert_eq!(tables.root_samples()[10].yaw, 0.5);
        assert!(tables.path(Name::new("blade")).is_some(), "and its paths stay");
    }

    /// One bad block per check: the message names the move, the track and the thing.
    #[test]
    fn a_bad_clip_block_names_what_it_is() {
        type Edit = fn(&mut ClipMotionDocument);
        let cases: &[(&str, Edit, &[&str])] = &[
            ("no source", |clip| clip.source.clear(), &["`source` is empty"]),
            (
                "a path named twice",
                |clip| {
                    let copy = clip.paths[0].clone();
                    clip.paths.push(copy);
                },
                &["`blade` appears twice"],
            ),
            (
                "a path sharing a keyed one's name",
                |clip| clip.paths[0].name = "shockwave".to_owned(),
                &["`shockwave` appears twice"],
            ),
            ("a path with no name", |clip| clip.paths[0].name.clear(), &["a name is empty"]),
            (
                "a path following nothing",
                |clip| clip.paths[0].follows = Follows::Joint(String::new()),
                &["`blade` follows a joint with no name"],
            ),
            (
                "an offset that isn't finite",
                |clip| clip.paths[0].offset[1] = f32::NAN,
                &["`blade` isn't finite"],
            ),
            (
                "a rotation that isn't unit",
                |clip| clip.paths[0].rotation = [0.0, 0.0, 0.0, 2.0],
                &["`blade` doesn't have a unit rotation"],
            ),
        ];
        for (what, edit, expected) in cases {
            let mut document = from_a_clip();
            edit(document.moves[0].motion.clip.as_mut().unwrap());
            let issues = document.check();
            assert!(!issues.is_empty(), "{what}");
            let text = issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
            for part in ["swing", "motion"].iter().chain(*expected) {
                assert!(text.contains(part), "{what}: no {part:?} in:\n{text}");
            }
        }

        let mut both = from_a_clip();
        both.moves[0].motion.root = vec![RootKey { at: 5, offset: [2.0, 0.0, 0.0], yaw: 0.0 }];
        both.moves[0].motion.clip.as_mut().unwrap().root = RootSource::InPlace;
        assert!(both.check().iter().any(|issue| issue.message.contains("say one or the other")));
    }

    /// A cook's fault with one move's clip is an issue on that move, and doesn't hide the others'.
    #[test]
    fn a_cooks_fault_is_an_issue_on_its_move() {
        let mut document = from_a_clip();
        document.intents[0].buffer = 99;
        let error = document.compile_with(|_| Err("the clip isn't cooked".to_owned()));
        let Err(MoveSetError::Invalid(issues)) = error else { panic!("compiled") };
        let text = issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.contains("move `swing`, track `motion`: the clip isn't cooked"), "{text}");
        assert!(text.contains("`intents`"), "{text}");
    }

    /// A hand-keyed move never asks a cook to read anything, and stays as it cooked before there were
    /// clips: the test content's hash pins that for the bytes, `sapphire_gameplay`'s
    /// `the_test_contents_hash_is_the_same_on_every_architecture`.
    #[test]
    fn hand_keys_stand_as_they_were() {
        let plain = slice().compile().unwrap();
        let asked = slice().compile_with(|mv| {
            assert!(mv.motion.clip.is_none());
            Ok(None)
        });
        assert_eq!(plain, asked.unwrap());
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

        // Thinner than the physics world takes.
        assert!(Shape::Sphere { radius: MIN_SHAPE_EXTENT }.is_valid());
        assert!(!Shape::Sphere { radius: 0.0005 }.is_valid());
        assert!(!Shape::Capsule { radius: 0.0005, length: 1.0 }.is_valid());
        assert!(!Shape::Box { half_extents: [0.1, 0.0005, 0.1] }.is_valid());
    }
}
