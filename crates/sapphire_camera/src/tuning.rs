//! The camera's tuning document: every rig's numbers, the collision's and the shake presets, as RON
//! through `sapphire_serialize::document`, so T2 can hot-reload it.
//!
//! [`CameraTuning::from_ron`] reads and checks a document; a refusal names the field
//! (`combat.framing.max_distance`). [`CameraDirector::set_tuning`](crate::CameraDirector::set_tuning)
//! swaps one in with the camera running: only goals change, so nothing jumps.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use crate::shake::{NamedShake, ShakeId};

/// The shipped tuning (see [`CameraTuning::default_tuning`]).
const DEFAULT_TUNING: &str = include_str!("../default_camera.ron");

/// All of the camera's numbers (a `CameraTuning` document, schema 1). Angles are radians, distances
/// metres and times seconds. A pitch above zero looks down.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TypePath)]
#[type_path = "sapphire::camera"]
#[serde(deny_unknown_fields)]
pub struct CameraTuning {
    pub explore: ExploreTuning,
    pub combat: CombatTuning,
    pub lock_on: LockOnTuning,
    pub look: LookTuning,
    pub collision: CollisionTuning,
    /// M2 G5's framing pass, so a document from before it still reads: it gets the shipped numbers.
    #[serde(default)]
    pub clearing: ClearingTuning,
    pub takeover: TakeoverTuning,
    pub shakes: ShakeTuning,
}

impl Versioned for CameraTuning {
    /// 1: M2 G5.
    const VERSION: u32 = 1;
}

/// Following one character around.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExploreTuning {
    /// Where on the player the camera looks, as a share of their height. Also where the collision's
    /// arm starts, in every rig: the point it keeps in sight.
    pub pivot_height: f32,
    pub distance: f32,
    /// The pitch recentring returns to.
    pub pitch: f32,
    /// The full vertical field of view.
    pub fov_y: f32,
    pub pivot_half_life: f32,
    /// Slower than across, so jumps and stairs don't bob the view.
    pub pivot_height_half_life: f32,
    pub distance_half_life: f32,
}

/// What combat framing and lock-on share: keeping two fighters in a safe frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramingTuning {
    /// How fast this rig takes over and lets go.
    pub blend_half_life: f32,
    /// How far from the player toward the target the view centres, as a share of the gap.
    pub pivot_bias: f32,
    /// The most that centre leads the player across the ground, however far off the target is.
    pub max_lead: f32,
    /// The same, up or down.
    pub max_rise: f32,
    /// The distance it prefers, which zoom scales.
    pub distance: f32,
    /// How much further than `distance` it would rather stand than turn the view: a target close enough to
    /// fit from there never turns it, so a fighter circling close doesn't spin the shot.
    pub turn_slack: f32,
    /// Across the ground, in metres: at the first of these or closer the fighters have no line between
    /// them to aim along, and at the second the rig follows that line in full.
    pub line_fade: [f32; 2],
    /// As far as it pulls back to fit both fighters.
    pub max_distance: f32,
    /// The closest a framed fighter may come to the camera, measured along the view.
    pub min_depth: f32,
    /// The frame the fighters are solved into, as a share of the screen's half width and height. The
    /// springs' lag lands them a little outside it, so leave room.
    pub safe_frame: [f32; 2],
    pub fov_y: f32,
    pub pivot_half_life: f32,
    pub pivot_height_half_life: f32,
    /// Pulling back is quick, so a fighter doesn't leave the frame.
    pub pull_out_half_life: f32,
    /// Coming back in is slow, so the shot doesn't breathe with every step.
    pub push_in_half_life: f32,
    /// How fast the yaw and pitch move to the angles this rig asks for.
    pub turn_half_life: f32,
}

/// Combat framing: player input stays free, and the view turns only as far as it must to keep the
/// target in the safe frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombatTuning {
    pub framing: FramingTuning,
    pub pitch_min: f32,
    pub pitch_max: f32,
}

/// Lock-on: the view holds the line from player to target, with the player a little to one side.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockOnTuning {
    pub framing: FramingTuning,
    /// How far the view sits off the line from player to target, so the player doesn't hide the
    /// target. It stays on the side it started on.
    pub side_angle: f32,
    pub pitch: f32,
    /// How much of the target's height above or below the player the pitch follows, 0 to 1.
    pub pitch_follow: f32,
    /// Look input can lean the view this far off the line, and no more.
    pub lean_yaw: f32,
    pub lean_pitch: f32,
    /// How fast a lean returns once input lets go.
    pub lean_half_life: f32,
}

/// Look input and the smoothing every rig shares.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookTuning {
    /// Input and every rig stay within these.
    pub pitch_min: f32,
    pub pitch_max: f32,
    pub angle_half_life: f32,
    pub fov_half_life: f32,
    /// How fast the framed target's position follows a new one, so a target switch doesn't jump.
    pub target_half_life: f32,
    /// The fastest any rig may turn the view, radians a second. Input isn't limited.
    pub max_turn_rate: f32,
    /// How softly a rig's turning starts and stops. Look input keeps the quicker `angle_half_life`.
    pub turn_half_life: f32,
    /// How far zoom can go each way, as the natural log of the factor on every rig's distance.
    pub zoom_min: f32,
    pub zoom_max: f32,
    /// How fast [`CameraDirector::recenter`](crate::CameraDirector::recenter) turns the view.
    pub recenter_half_life: f32,
    /// Seconds without look input before the view starts drifting back on its own.
    pub idle_delay: f32,
    /// How fast it drifts back.
    pub idle_half_life: f32,
    /// The player's speed, metres a second, at which the yaw's drift behind them is at full strength.
    /// A player standing still doesn't turn the view; the pitch returns either way.
    pub idle_full_speed: f32,
}

/// The sphere the eye is, and how the arm from player to eye gives way.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollisionTuning {
    /// The sphere's radius. Well over the near plane's corners, so geometry doesn't cut the view.
    pub radius: f32,
    /// How far ahead the arm looks for what its current motion will run into, so it comes in before
    /// it arrives.
    pub lead_time: f32,
    pub pull_in_half_life: f32,
    /// Slower than in, so it doesn't pump.
    pub pull_out_half_life: f32,
    /// Seconds the line stays clear before the arm starts to ease back out.
    pub pull_out_delay: f32,
    /// The fastest the arm comes in or goes out, metres a second.
    pub max_pull_speed: f32,
    /// An occluder at most this deep along the line, with the eye's own spot clear beyond it, is passed
    /// behind rather than pulled in front of: going in front means crossing it.
    pub thin_occluder: f32,
    /// How long such an occluder may hide the player before the arm comes in front of it anyway: a
    /// long thin wall isn't a pillar.
    pub thin_patience: f32,
    /// How far the eye may sit from where the arm wants it without counting as stuck.
    pub stuck_gap: f32,
    /// How long stuck before the eye jumps to its place. After half of this it stops treating what's in
    /// the way as thin.
    pub stuck_time: f32,
}

/// Turning the view round what's in its way, when a framing rig is on (the `clearing` module's docs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClearingTuning {
    /// How often it looks at the view, in seconds.
    pub check_every: f32,
    /// How long a view stays bad before it looks for a better one.
    pub patience: f32,
    /// How long after choosing before it may choose again.
    pub settle: f32,
    /// How long after look input before it may choose.
    pub after_look: f32,
    /// How softly the view turns to what it chose.
    pub half_life: f32,
    /// The most it turns at, in radians a second, however bad the view it's leaving.
    pub top_speed: f32,
    /// The most that turn moves the eye at, in metres a second, so a long arm turns slower.
    pub top_eye_speed: f32,
    /// The furthest it'll turn at one choice, in radians. 0 turns all of it off.
    pub reach: f32,
    /// How much wrong a view may have and be left alone, as `Scene::fault` counts it.
    pub tolerance: f32,
    /// How much better another view must be to turn to it.
    pub better_by: f32,
}

impl Default for ClearingTuning {
    fn default() -> Self {
        Self {
            check_every: 0.1,
            patience: 0.2,
            settle: 0.8,
            after_look: 1.5,
            half_life: 0.4,
            top_speed: 2.0,
            top_eye_speed: 12.0,
            reach: 2.4,
            tolerance: 0.4,
            better_by: 0.6,
        }
    }
}

/// A sequencer's takeover, which M3's cinematics drive.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakeoverTuning {
    pub blend_half_life: f32,
}

/// Shake presets and what limits them all together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShakeTuning {
    /// The most all shakes together may turn the view on each axis.
    pub rotation_limit: f32,
    /// The most all shakes together may move the eye on each axis.
    pub translation_limit: f32,
    pub presets: Vec<NamedShake>,
}

/// A document that can't be read, or a value out of range. A value's refusal names its field.
#[derive(Debug, thiserror::Error)]
pub enum CameraTuningError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("`{field}` {problem}")]
    Invalid { field: String, problem: String },
}

/// Walks a document's values, stopping at the first that's out of range.
struct Check;

impl Check {
    fn range(field: &str, value: f32, min: f32, max: f32) -> Result<(), CameraTuningError> {
        // Written so a NaN fails.
        if value >= min && value <= max {
            return Ok(());
        }
        Err(CameraTuningError::Invalid {
            field: field.to_owned(),
            problem: format!("must be within {min}..={max}, not {value}"),
        })
    }

    fn order(smaller: &str, a: f32, larger: &str, b: f32) -> Result<(), CameraTuningError> {
        if a <= b {
            return Ok(());
        }
        Err(CameraTuningError::Invalid {
            field: larger.to_owned(),
            problem: format!("is {b}, under `{smaller}` at {a}"),
        })
    }
}

const LENS: (f32, f32) = (0.2, 2.6);
const NEAR_LEVEL: (f32, f32) = (-1.4, 1.4);
/// Half-lives. Above zero: zero means landing at once, which is a jump.
const HALF_LIFE: (f32, f32) = (0.001, 10.0);
const LONG_WAY: f32 = 200.0;

fn half_life(field: &str, value: f32) -> Result<(), CameraTuningError> {
    Check::range(field, value, HALF_LIFE.0, HALF_LIFE.1)
}

impl ExploreTuning {
    fn validate(&self) -> Result<(), CameraTuningError> {
        Check::range("explore.pivot_height", self.pivot_height, 0.0, 1.5)?;
        Check::range("explore.distance", self.distance, 0.1, LONG_WAY)?;
        Check::range("explore.pitch", self.pitch, NEAR_LEVEL.0, NEAR_LEVEL.1)?;
        Check::range("explore.fov_y", self.fov_y, LENS.0, LENS.1)?;
        half_life("explore.pivot_half_life", self.pivot_half_life)?;
        half_life("explore.pivot_height_half_life", self.pivot_height_half_life)?;
        half_life("explore.distance_half_life", self.distance_half_life)
    }
}

impl FramingTuning {
    fn validate(&self, rig: &str) -> Result<(), CameraTuningError> {
        let name = |field: &str| format!("{rig}.framing.{field}");
        half_life(&name("blend_half_life"), self.blend_half_life)?;
        Check::range(&name("pivot_bias"), self.pivot_bias, 0.0, 1.0)?;
        Check::range(&name("max_lead"), self.max_lead, 0.0, LONG_WAY)?;
        Check::range(&name("max_rise"), self.max_rise, 0.0, LONG_WAY)?;
        Check::range(&name("distance"), self.distance, 0.1, LONG_WAY)?;
        Check::range(&name("max_distance"), self.max_distance, 0.1, LONG_WAY)?;
        Check::order(&name("distance"), self.distance, &name("max_distance"), self.max_distance)?;
        Check::range(&name("turn_slack"), self.turn_slack, 0.0, LONG_WAY)?;
        Check::range(&name("line_fade[0]"), self.line_fade[0], 0.0, LONG_WAY)?;
        Check::range(&name("line_fade[1]"), self.line_fade[1], 0.001, LONG_WAY)?;
        if self.line_fade[0] >= self.line_fade[1] {
            return Err(CameraTuningError::Invalid {
                field: name("line_fade[1]"),
                problem: format!(
                    "is {}, not above `line_fade[0]` at {}",
                    self.line_fade[1], self.line_fade[0]
                ),
            });
        }
        Check::range(&name("min_depth"), self.min_depth, 0.0, LONG_WAY)?;
        Check::range(&name("safe_frame[0]"), self.safe_frame[0], 0.05, 1.0)?;
        Check::range(&name("safe_frame[1]"), self.safe_frame[1], 0.05, 1.0)?;
        Check::range(&name("fov_y"), self.fov_y, LENS.0, LENS.1)?;
        half_life(&name("pivot_half_life"), self.pivot_half_life)?;
        half_life(&name("pivot_height_half_life"), self.pivot_height_half_life)?;
        half_life(&name("pull_out_half_life"), self.pull_out_half_life)?;
        half_life(&name("push_in_half_life"), self.push_in_half_life)?;
        half_life(&name("turn_half_life"), self.turn_half_life)
    }
}

impl CameraTuning {
    /// Sapphire's tuning (`default_camera.ron` beside this crate's manifest), compiled in.
    #[must_use]
    pub fn default_tuning() -> Self {
        Self::from_ron(DEFAULT_TUNING).expect("the shipped tuning is checked by this module's tests")
    }

    /// Reads a tuning document, upgrading older schema versions and checking it.
    pub fn from_ron(text: &str) -> Result<Self, CameraTuningError> {
        let tuning: Self = sapphire_serialize::from_ron(text)?;
        tuning.validate()?;
        Ok(tuning)
    }

    pub fn to_ron(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// The preset called `name`. An ID holds for this tuning: a cue table resolves its names at load,
    /// and again after a reload, which may reorder them.
    #[must_use]
    pub fn shake_id(&self, name: &str) -> Option<ShakeId> {
        let index = self.shakes.presets.iter().position(|preset| preset.name == name)?;
        Some(ShakeId(index as u16))
    }

    /// Checks every value, naming the first field out of range.
    pub fn validate(&self) -> Result<(), CameraTuningError> {
        self.explore.validate()?;

        self.combat.framing.validate("combat")?;
        Check::range("combat.pitch_min", self.combat.pitch_min, NEAR_LEVEL.0, NEAR_LEVEL.1)?;
        Check::range("combat.pitch_max", self.combat.pitch_max, NEAR_LEVEL.0, NEAR_LEVEL.1)?;
        Check::order("combat.pitch_min", self.combat.pitch_min, "combat.pitch_max", self.combat.pitch_max)?;

        let lock = &self.lock_on;
        lock.framing.validate("lock_on")?;
        Check::range("lock_on.side_angle", lock.side_angle, 0.0, 1.5)?;
        Check::range("lock_on.pitch", lock.pitch, NEAR_LEVEL.0, NEAR_LEVEL.1)?;
        Check::range("lock_on.pitch_follow", lock.pitch_follow, 0.0, 1.0)?;
        Check::range("lock_on.lean_yaw", lock.lean_yaw, 0.0, 1.5)?;
        Check::range("lock_on.lean_pitch", lock.lean_pitch, 0.0, 1.5)?;
        half_life("lock_on.lean_half_life", lock.lean_half_life)?;

        let look = &self.look;
        Check::range("look.pitch_min", look.pitch_min, NEAR_LEVEL.0, NEAR_LEVEL.1)?;
        Check::range("look.pitch_max", look.pitch_max, NEAR_LEVEL.0, NEAR_LEVEL.1)?;
        Check::order("look.pitch_min", look.pitch_min, "look.pitch_max", look.pitch_max)?;
        half_life("look.angle_half_life", look.angle_half_life)?;
        half_life("look.fov_half_life", look.fov_half_life)?;
        half_life("look.target_half_life", look.target_half_life)?;
        Check::range("look.max_turn_rate", look.max_turn_rate, 0.01, 60.0)?;
        half_life("look.turn_half_life", look.turn_half_life)?;
        Check::range("look.zoom_min", look.zoom_min, -3.0, 0.0)?;
        Check::range("look.zoom_max", look.zoom_max, 0.0, 3.0)?;
        half_life("look.recenter_half_life", look.recenter_half_life)?;
        Check::range("look.idle_delay", look.idle_delay, 0.0, 600.0)?;
        half_life("look.idle_half_life", look.idle_half_life)?;
        Check::range("look.idle_full_speed", look.idle_full_speed, 0.01, LONG_WAY)?;

        let collision = &self.collision;
        Check::range("collision.radius", collision.radius, 0.02, 2.0)?;
        Check::range("collision.lead_time", collision.lead_time, 0.0, 2.0)?;
        half_life("collision.pull_in_half_life", collision.pull_in_half_life)?;
        half_life("collision.pull_out_half_life", collision.pull_out_half_life)?;
        Check::range("collision.pull_out_delay", collision.pull_out_delay, 0.0, 30.0)?;
        Check::range("collision.max_pull_speed", collision.max_pull_speed, 0.1, LONG_WAY)?;
        Check::range("collision.thin_occluder", collision.thin_occluder, 0.0, LONG_WAY)?;
        Check::range("collision.thin_patience", collision.thin_patience, 0.0, 60.0)?;
        Check::range("collision.stuck_gap", collision.stuck_gap, 0.01, LONG_WAY)?;
        Check::range("collision.stuck_time", collision.stuck_time, 0.0, 30.0)?;

        let clearing = &self.clearing;
        Check::range("clearing.check_every", clearing.check_every, 0.01, 10.0)?;
        Check::range("clearing.patience", clearing.patience, 0.0, 30.0)?;
        Check::range("clearing.settle", clearing.settle, 0.0, 30.0)?;
        Check::range("clearing.after_look", clearing.after_look, 0.0, 30.0)?;
        half_life("clearing.half_life", clearing.half_life)?;
        Check::range("clearing.top_speed", clearing.top_speed, 0.01, self.look.max_turn_rate)?;
        Check::range("clearing.top_eye_speed", clearing.top_eye_speed, 0.01, LONG_WAY)?;
        Check::range("clearing.reach", clearing.reach, 0.0, 3.1)?;
        Check::range("clearing.tolerance", clearing.tolerance, 0.0, 10.0)?;
        Check::range("clearing.better_by", clearing.better_by, 0.0, 10.0)?;

        half_life("takeover.blend_half_life", self.takeover.blend_half_life)?;

        let shakes = &self.shakes;
        Check::range("shakes.rotation_limit", shakes.rotation_limit, 0.0, 0.5)?;
        Check::range("shakes.translation_limit", shakes.translation_limit, 0.0, 1.0)?;
        if shakes.presets.len() > usize::from(u16::MAX) {
            return Err(CameraTuningError::Invalid {
                field: "shakes.presets".to_owned(),
                problem: "holds more presets than a shake ID can name".to_owned(),
            });
        }
        for (index, preset) in shakes.presets.iter().enumerate() {
            let invalid = |field: String, problem: &str| CameraTuningError::Invalid {
                field,
                problem: problem.to_owned(),
            };
            if preset.name.is_empty() {
                return Err(invalid(format!("shakes.presets[{index}].name"), "can't be empty"));
            }
            if shakes.presets[..index].iter().any(|earlier| earlier.name == preset.name) {
                return Err(invalid(format!("shakes.presets[{index}].name"), "appears twice"));
            }
            preset.shake.validate(&format!("shakes.presets[{index}]"))?;
        }
        Ok(())
    }
}

impl crate::shake::Shake {
    fn validate(&self, name: &str) -> Result<(), CameraTuningError> {
        let field = |field: &str| format!("{name}.{field}");
        Check::range(&field("duration"), self.duration, 0.01, 30.0)?;
        Check::range(&field("attack"), self.attack, 0.001, 30.0)?;
        Check::order(&field("attack"), self.attack, &field("duration"), self.duration)?;
        if self.attack == self.duration {
            return Err(CameraTuningError::Invalid {
                field: field("attack"),
                problem: "must leave time to decay before duration".to_owned(),
            });
        }
        Check::range(&field("falloff"), self.falloff, 0.1, 8.0)?;
        Check::range(&field("frequency"), self.frequency, 0.1, 120.0)?;
        for (axis, value) in self.rotation.into_iter().enumerate() {
            Check::range(&field(&format!("rotation[{axis}]")), value, 0.0, 0.5)?;
        }
        for (axis, value) in self.translation.into_iter().enumerate() {
            Check::range(&field(&format!("translation[{axis}]")), value, 0.0, 1.0)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_tuning_reads_checks_and_round_trips() {
        let tuning = CameraTuning::default_tuning();
        tuning.validate().expect("the shipped tuning is in range");
        let text = tuning.to_ron().expect("it writes");
        assert_eq!(CameraTuning::from_ron(&text).expect("what it wrote reads"), tuning);
        for name in ["hit_light", "hit_heavy", "finish"] {
            assert!(tuning.shake_id(name).is_some(), "{name}");
        }
        assert!(tuning.shake_id("nothing").is_none());
    }

    /// A broken value is refused by name, for T2's message.
    #[test]
    fn each_refusal_names_its_field() {
        type Break = fn(&mut CameraTuning);
        let cases: &[(&str, Break)] = &[
            ("explore.pivot_height", |t| t.explore.pivot_height = -1.0),
            ("explore.distance", |t| t.explore.distance = f32::NAN),
            ("explore.fov_y", |t| t.explore.fov_y = 3.2),
            ("explore.pivot_half_life", |t| t.explore.pivot_half_life = 0.0),
            ("combat.framing.pivot_bias", |t| t.combat.framing.pivot_bias = 1.5),
            ("combat.framing.max_distance", |t| t.combat.framing.max_distance = 0.5),
            ("combat.framing.safe_frame[1]", |t| t.combat.framing.safe_frame[1] = 1.5),
            ("combat.framing.pull_out_half_life", |t| t.combat.framing.pull_out_half_life = -0.1),
            ("combat.framing.line_fade[1]", |t| {
                t.combat.framing.line_fade[1] = t.combat.framing.line_fade[0]
            }),
            ("combat.pitch_max", |t| t.combat.pitch_max = t.combat.pitch_min - 0.1),
            ("lock_on.framing.turn_half_life", |t| t.lock_on.framing.turn_half_life = f32::INFINITY),
            ("lock_on.side_angle", |t| t.lock_on.side_angle = 2.0),
            ("lock_on.pitch_follow", |t| t.lock_on.pitch_follow = 1.5),
            ("lock_on.lean_half_life", |t| t.lock_on.lean_half_life = 0.0),
            ("look.pitch_max", |t| t.look.pitch_max = t.look.pitch_min - 0.1),
            ("look.max_turn_rate", |t| t.look.max_turn_rate = 0.0),
            ("look.zoom_min", |t| t.look.zoom_min = 1.0),
            ("look.zoom_max", |t| t.look.zoom_max = -1.0),
            ("collision.radius", |t| t.collision.radius = 0.0),
            ("collision.pull_in_half_life", |t| t.collision.pull_in_half_life = 0.0),
            ("collision.thin_occluder", |t| t.collision.thin_occluder = -1.0),
            ("clearing.check_every", |t| t.clearing.check_every = 0.0),
            ("clearing.patience", |t| t.clearing.patience = -1.0),
            ("clearing.settle", |t| t.clearing.settle = f32::NAN),
            ("clearing.after_look", |t| t.clearing.after_look = 100.0),
            ("clearing.half_life", |t| t.clearing.half_life = 0.0),
            ("clearing.top_speed", |t| t.clearing.top_speed = 100.0),
            ("clearing.top_eye_speed", |t| t.clearing.top_eye_speed = 0.0),
            ("clearing.reach", |t| t.clearing.reach = 4.0),
            ("clearing.tolerance", |t| t.clearing.tolerance = -0.5),
            ("clearing.better_by", |t| t.clearing.better_by = 100.0),
            ("takeover.blend_half_life", |t| t.takeover.blend_half_life = 0.0),
            ("shakes.rotation_limit", |t| t.shakes.rotation_limit = 1.0),
            ("shakes.presets[1].name", |t| t.shakes.presets[1].name = t.shakes.presets[0].name.clone()),
            ("shakes.presets[0].attack", |t| t.shakes.presets[0].shake.attack = 100.0),
            ("shakes.presets[0].attack", |t| {
                t.shakes.presets[0].shake.attack = t.shakes.presets[0].shake.duration;
            }),
        ];
        for &(field, break_it) in cases {
            let mut tuning = CameraTuning::default_tuning();
            break_it(&mut tuning);
            let error = tuning.validate().expect_err(field);
            assert!(error.to_string().contains(&format!("`{field}`")), "{field}: {error}");
        }
    }

    /// `ClearingTuning::default` is what a document from before it gets, so it says what's shipped.
    #[test]
    fn a_document_with_no_clearing_gets_the_shipped_numbers() {
        assert_eq!(CameraTuning::default_tuning().clearing, ClearingTuning::default());
    }

    #[test]
    fn a_document_out_of_range_or_misspelt_is_refused() {
        let text = CameraTuning::default_tuning().to_ron().expect("it writes");
        let out_of_range = text.replacen("radius: 0.2", "radius: 9.0", 1);
        assert_ne!(out_of_range, text, "the shipped radius moved: update this test");
        let error = CameraTuning::from_ron(&out_of_range).expect_err("a radius of 9 m");
        assert!(error.to_string().contains("collision.radius"), "{error}");

        let misspelt = text.replacen("radius:", "radious:", 1);
        let error = CameraTuning::from_ron(&misspelt).expect_err("an unknown field");
        assert!(matches!(error, CameraTuningError::Document(_)), "{error}");
    }
}
