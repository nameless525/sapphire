//! A character's feel as plain data: what a game tunes, and what's worked out from it once.

use sapphire_math::{Vec2, Vec3, det};
use sapphire_physics::{BodyDesc, BodyId, CollisionGroups, Group, Pose, Shape};
use serde::{Deserialize, Serialize};

use crate::checksum::StateHasher;

/// What a character's capsule hits: the scenery, invisible walls and other characters.
fn capsule_groups() -> CollisionGroups {
    CollisionGroups::new(Group::Character.mask(), Group::World | Group::Character | Group::CharacterBlocker)
}

/// Everything about how a character moves that a game tunes, in metres, seconds and degrees. Every
/// field has a default, so a document names only what it changes.
///
/// The defaults are starting numbers for Zenless Zone Zero's snappy feel (`docs/m2-plan.md` §2),
/// reasoned from the pillar rather than measured from the game, for the maintainer's reviews to move.
/// At a full stick a press moves the character 1.25 cm, 5 cm and 11.25 cm at ticks 1, 2 and 3, and
/// it's at full speed from tick 4. The clips' run stride comes later (M2 N3), so nothing here assumes
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControllerProfile {
    /// The capsule's whole height.
    pub height: f32,
    /// The capsule's radius.
    pub radius: f32,
    /// The gap kept from surfaces, which an actor's feet don't show: on flat ground they're exactly
    /// on it.
    pub skin: f32,

    /// Speed at [`Self::walk_stick`] of tilt, in m/s. Below that, speed scales with the tilt.
    pub walk_speed: f32,
    /// Speed from [`Self::run_stick`] of tilt on, in m/s. Between the two it's a straight line.
    pub run_speed: f32,
    /// How far the stick tilts for a walk, within 0..1.
    pub walk_stick: f32,
    /// How far the stick tilts for a run, above [`Self::walk_stick`] and at most 1.
    pub run_stick: f32,
    /// Speeding up on the ground, in m/s². Pillar 1 lives here.
    pub acceleration: f32,
    /// Slowing down on the ground, in m/s². A reversal takes the larger of the two.
    pub deceleration: f32,
    /// How fast facing turns toward the stick, in degrees a second. Motion doesn't wait for it.
    pub turn_rate_degrees: f32,

    /// Downward, in m/s².
    pub gravity: f32,
    /// A jump's upward speed, in m/s: it reaches `jump_speed² / (2 × gravity)`.
    pub jump_speed: f32,
    /// Steering while airborne, in m/s². A neutral stick keeps what momentum there is.
    pub air_acceleration: f32,
    /// The fastest fall, in m/s.
    pub max_fall_speed: f32,
    /// Ticks of the owner's time after walking off a ledge in which a jump still works.
    pub coyote_ticks: u32,

    /// The steepest ground that can be stood on, in degrees. Steeper slides.
    pub slope_limit_degrees: f32,
    /// The tallest step the feet take without a jump.
    pub step_height: f32,
    /// How far down the feet follow the ground while walking, so a downhill run or a stair going down
    /// stays grounded. A tick's own travel at the slope limit is added to it.
    pub ground_snap: f32,
    /// How far from the capsule's axis a ledge's edge still holds the character: past this it falls
    /// off. Under the radius, because a capsule's round foot would otherwise roll off slowly.
    pub stance_radius: f32,
    /// The fastest slide down ground too steep to stand on, in m/s.
    pub max_slide_speed: f32,
}

impl Default for ControllerProfile {
    fn default() -> Self {
        Self {
            height: 1.6,
            radius: 0.3,
            skin: 0.01,
            walk_speed: 2.0,
            run_speed: 6.0,
            walk_stick: 0.5,
            run_stick: 0.9,
            acceleration: 90.0,
            deceleration: 60.0,
            turn_rate_degrees: 1080.0,
            gravity: 22.0,
            jump_speed: 7.0,
            air_acceleration: 18.0,
            max_fall_speed: 30.0,
            coyote_ticks: 5,
            slope_limit_degrees: 50.0,
            step_height: 0.3,
            ground_snap: 0.3,
            stance_radius: 0.15,
            max_slide_speed: 8.0,
        }
    }
}

/// A profile with a number that can't work, named by its field.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[error("the controller profile's {field} is {value}: it must be {expected}")]
pub struct ProfileError {
    pub field: &'static str,
    pub value: f32,
    pub expected: &'static str,
}

impl ControllerProfile {
    /// Checks every number, and works out what ticks read.
    pub fn resolve(&self) -> Result<ResolvedProfile, ProfileError> {
        let check = |field, value: f32, good: bool, expected| {
            if value.is_finite() && good { Ok(()) } else { Err(ProfileError { field, value, expected }) }
        };
        let positive = |field, value: f32| check(field, value, value > 0.0, "above 0");
        let not_negative = |field, value: f32| check(field, value, value >= 0.0, "at least 0");

        positive("radius", self.radius)?;
        check("height", self.height, self.height > 2.0 * self.radius, "above twice the radius")?;
        check(
            "skin",
            self.skin,
            self.skin > 0.0 && self.skin <= self.radius / 4.0,
            "above 0, up to a quarter of the radius",
        )?;
        positive("walk_speed", self.walk_speed)?;
        check("run_speed", self.run_speed, self.run_speed >= self.walk_speed, "at least the walk speed")?;
        check("walk_stick", self.walk_stick, self.walk_stick > 0.0 && self.walk_stick < 1.0, "within 0..1")?;
        check(
            "run_stick",
            self.run_stick,
            self.run_stick > self.walk_stick && self.run_stick <= 1.0,
            "above the walk stick, at most 1",
        )?;
        positive("acceleration", self.acceleration)?;
        positive("deceleration", self.deceleration)?;
        positive("turn_rate_degrees", self.turn_rate_degrees)?;
        not_negative("gravity", self.gravity)?;
        not_negative("jump_speed", self.jump_speed)?;
        not_negative("air_acceleration", self.air_acceleration)?;
        positive("max_fall_speed", self.max_fall_speed)?;
        check(
            "slope_limit_degrees",
            self.slope_limit_degrees,
            self.slope_limit_degrees > 0.0 && self.slope_limit_degrees <= 85.0,
            "above 0, at most 85",
        )?;
        not_negative("step_height", self.step_height)?;
        not_negative("ground_snap", self.ground_snap)?;
        check(
            "stance_radius",
            self.stance_radius,
            self.stance_radius >= 0.0 && self.stance_radius <= self.radius,
            "within the radius",
        )?;
        positive("max_slide_speed", self.max_slide_speed)?;

        let limit = self.slope_limit_degrees.to_radians();
        Ok(ResolvedProfile {
            authored: *self,
            half_height: self.height / 2.0 - self.radius,
            walkable_cos: det::cos(limit),
            walkable_tan: det::tan(limit),
            turn_rate: self.turn_rate_degrees.to_radians(),
        })
    }

    /// Writes every field as canonical bytes: destructured, so a new field can't be forgotten.
    pub(super) fn write_canonical(&self, hasher: &mut StateHasher) {
        let Self {
            height,
            radius,
            skin,
            walk_speed,
            run_speed,
            walk_stick,
            run_stick,
            acceleration,
            deceleration,
            turn_rate_degrees,
            gravity,
            jump_speed,
            air_acceleration,
            max_fall_speed,
            coyote_ticks,
            slope_limit_degrees,
            step_height,
            ground_snap,
            stance_radius,
            max_slide_speed,
        } = *self;
        hasher.f32s(&[
            height,
            radius,
            skin,
            walk_speed,
            run_speed,
            walk_stick,
            run_stick,
            acceleration,
            deceleration,
            turn_rate_degrees,
            gravity,
            jump_speed,
            air_acceleration,
            max_fall_speed,
        ]);
        hasher.u32(coyote_ticks);
        hasher.f32s(&[slope_limit_degrees, step_height, ground_snap, stance_radius, max_slide_speed]);
    }
}

/// A checked [`ControllerProfile`], with what ticks read worked out once: the only form a controller
/// runs on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedProfile {
    authored: ControllerProfile,
    half_height: f32,
    walkable_cos: f32,
    walkable_tan: f32,
    /// In radians a second.
    turn_rate: f32,
}

impl ResolvedProfile {
    /// How much of a cosine a surface may fall short of the slope limit by and still count as
    /// walkable: about a tenth of a degree. A sweep reads a slope at the limit with rounding in it,
    /// and flickering between standing and sliding there would be the worst answer.
    const WALKABLE_SLACK: f32 = 1e-3;

    /// The numbers as authored.
    #[must_use]
    pub const fn authored(&self) -> &ControllerProfile {
        &self.authored
    }

    /// The capsule, as `sapphire_physics` has it.
    #[must_use]
    pub const fn capsule(&self) -> Shape {
        Shape::Capsule { half_height: self.half_height, radius: self.authored.radius }
    }

    /// The capsule's cylinder's half height.
    #[must_use]
    pub const fn half_height(&self) -> f32 {
        self.half_height
    }

    /// From an actor's feet up to its capsule's center.
    #[must_use]
    pub fn feet_to_center(&self) -> Vec3 {
        Vec3::Y * (self.half_height + self.authored.radius + self.authored.skin)
    }

    /// The body for a character standing at `feet`, facing `yaw`.
    #[must_use]
    pub fn body(&self, id: BodyId, feet: Vec3, yaw: f32) -> BodyDesc {
        BodyDesc {
            id,
            shape: self.capsule(),
            pose: Pose::new(feet + self.feet_to_center(), det::quat_from_rotation_y(yaw)),
            groups: capsule_groups(),
        }
    }

    /// The horizontal velocity a stick asks for, its x along world +X and its y along world +Z.
    #[must_use]
    pub fn target_velocity(&self, stick: Vec2) -> Vec3 {
        let tilt = stick.length();
        if tilt <= 0.0 {
            return Vec3::ZERO;
        }
        let p = &self.authored;
        let speed = if tilt <= p.walk_stick {
            p.walk_speed * tilt / p.walk_stick
        } else {
            let run = ((tilt - p.walk_stick) / (p.run_stick - p.walk_stick)).min(1.0);
            p.walk_speed + (p.run_speed - p.walk_speed) * run
        };
        Vec3::new(stick.x, 0.0, stick.y) * (speed / tilt)
    }

    /// Whether a surface facing `normal` can be stood on.
    pub(super) fn walkable(&self, normal: Vec3) -> bool {
        normal.y >= self.walkable_cos - Self::WALKABLE_SLACK
    }

    pub(super) const fn walkable_tan(&self) -> f32 {
        self.walkable_tan
    }

    pub(super) const fn walkable_cos(&self) -> f32 {
        self.walkable_cos
    }

    pub(super) const fn turn_rate(&self) -> f32 {
        self.turn_rate
    }
}
