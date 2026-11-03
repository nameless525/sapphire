//! What's worked out once from a character's numbers ([`ControllerProfile`], which a document can hold).

use sapphire_math::{Vec2, Vec3, det};
use sapphire_moves::{ControllerProfile, MAX_JUMP_REACH, MAX_THROW_REACH, ProfileError};
use sapphire_physics::{BodyDesc, BodyId, CollisionGroups, Group, GroupMask, Pose, Shape};

use crate::checksum::StateHasher;

/// What a character's capsule hits: the scenery, invisible walls and other characters.
fn capsule_groups() -> CollisionGroups {
    CollisionGroups::new(Group::Character.mask(), Group::World | Group::Character | Group::CharacterBlocker)
}

/// What a defeated character's capsule is: in no group, so nothing meets it, and meeting only scenery.
pub(super) fn fallen_groups() -> CollisionGroups {
    CollisionGroups::new(GroupMask::NONE, Group::World | Group::CharacterBlocker)
}

/// Writes every number as canonical bytes: destructured, so a new field can't be forgotten.
pub(super) fn write_canonical(profile: &ControllerProfile, hasher: &mut StateHasher) {
    let ControllerProfile {
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
    } = *profile;
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
    /// Checks every number, and works out what ticks read.
    pub fn new(profile: &ControllerProfile) -> Result<Self, ProfileError> {
        profile.check()?;
        let limit = profile.slope_limit_degrees.to_radians();
        Ok(Self {
            authored: *profile,
            half_height: profile.height / 2.0 - profile.radius,
            walkable_cos: det::cos(limit),
            walkable_tan: det::tan(limit),
            turn_rate: profile.turn_rate_degrees.to_radians(),
        })
    }

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

    /// What's left of a velocity that other numbers gave someone, as these take over: a live edit landing
    /// on them, or a party's next member taking over their jump. All of it, to the bit, unless gravity this
    /// light would carry it further than these numbers could have thrown anyone themselves
    /// ([`MAX_JUMP_REACH`], and across with it [`MAX_THROW_REACH`]). Then what's rising and what's going
    /// across are cut to those, with nothing moved. A jump begun at a gravity of 22 and finishing under
    /// a typed 2 would rise 7 m more, which is over the arena's walls. A fall is its own limit's to hold.
    #[must_use]
    pub fn carried(&self, velocity: Vec3) -> Vec3 {
        let twice_gravity = 2.0 * self.authored.gravity;
        if twice_gravity <= 0.0 {
            return velocity;
        }
        let up = velocity.y.min((twice_gravity * MAX_JUMP_REACH).sqrt());
        let rising = up.max(0.0);
        let room = twice_gravity * MAX_THROW_REACH - rising * rising;
        let across = velocity.x * velocity.x + velocity.z * velocity.z;
        let keep = if across > room { (room / across).sqrt() } else { 1.0 };
        Vec3::new(velocity.x * keep, up, velocity.z * keep)
    }

    /// Whether a body made from this stands where one made from `other` would, with the same shape.
    pub(super) fn same_body(&self, other: &Self) -> bool {
        let (a, b) = (&self.authored, &other.authored);
        (a.height, a.radius, a.skin) == (b.height, b.radius, b.skin)
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
