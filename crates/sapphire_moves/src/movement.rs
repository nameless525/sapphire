//! How a character moves, as numbers a document holds (M2 T2): a fighter's `movement` section, what it
//! may hold, and what a simulation is given when a document says nothing.
//!
//! This holds the plain numbers and their checks, so a cook and a running app refuse the same document.
//! Running on them, and what's worked out from them once, is `sapphire_gameplay`'s `controller` module.

use serde::{Deserialize, Serialize};

/// Everything about how a character moves that a game tunes, in metres, seconds and degrees. Every
/// field has a default, so a document names only what it changes.
///
/// The defaults are starting numbers for Zenless Zone Zero's snappy feel (`docs/m2-plan.md` §2),
/// reasoned from the pillar rather than measured from the game, for the maintainer's reviews to move.
/// At a full stick a press moves the character 1.25 cm, 5 cm and 11.25 cm at ticks 1, 2 and 3, and
/// it's at full speed from tick 4. The clips' run stride comes later (M2 N3), so nothing here assumes
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

/// The highest a jump in a document may reach, in metres. The arena's walls stand 6 m tall, "well over
/// any jump from what can be climbed", and from the ground this reaches nothing taller than 3 m.
pub const MAX_JUMP_REACH: f32 = 2.5;

/// What one number of a document's `movement` may be: `min` to `max`, both good.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementLimit {
    pub field: &'static str,
    pub min: f32,
    pub max: f32,
    pub unit: &'static str,
}

const fn limit(field: &'static str, min: f32, max: f32, unit: &'static str) -> MovementLimit {
    MovementLimit { field, min, max, unit }
}

/// Every number of a document's `movement`, in the order [`ControllerProfile`] has them, with what a real
/// character can use. Tighter than [`ControllerProfile::check`], which is what the arithmetic needs: this
/// is for a person typing, so `run_speed: 600` is a typo that's refused rather than a character at the far
/// end of the map, and neither a speed nor a rate can leave a character that never gets anywhere.
pub const MOVEMENT_LIMITS: [MovementLimit; 20] = [
    limit("height", 0.5, 3.0, "m"),
    limit("radius", 0.1, 1.0, "m"),
    limit("skin", 0.001, 0.05, "m"),
    limit("walk_speed", 0.1, 15.0, "m/s"),
    limit("run_speed", 0.1, 30.0, "m/s"),
    limit("walk_stick", 0.05, 0.95, "of the stick's tilt"),
    limit("run_stick", 0.1, 1.0, "of the stick's tilt"),
    limit("acceleration", 1.0, 2000.0, "m/s²"),
    limit("deceleration", 1.0, 2000.0, "m/s²"),
    limit("turn_rate_degrees", 30.0, 7200.0, "degrees a second"),
    limit("gravity", 2.0, 200.0, "m/s²"),
    limit("jump_speed", 0.0, 30.0, "m/s"),
    limit("air_acceleration", 0.0, 2000.0, "m/s²"),
    limit("max_fall_speed", 1.0, 100.0, "m/s"),
    limit("coyote_ticks", 0.0, 60.0, "ticks"),
    limit("slope_limit_degrees", 10.0, 85.0, "degrees"),
    limit("step_height", 0.0, 1.0, "m"),
    limit("ground_snap", 0.0, 1.0, "m"),
    limit("stance_radius", 0.0, 1.0, "m"),
    limit("max_slide_speed", 0.5, 50.0, "m/s"),
];

impl ControllerProfile {
    /// Whether a controller can run on these numbers: what its arithmetic needs of them. A document may
    /// hold a good deal less ([`Self::document_issues`]).
    pub fn check(&self) -> Result<(), ProfileError> {
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
        Ok(())
    }

    /// Everything wrong with these numbers as a document's, one line each, each naming its field. What
    /// passes this also passes [`Self::check`].
    #[must_use]
    pub fn document_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        for (limit, value) in MOVEMENT_LIMITS.iter().zip(self.numbers()) {
            if !(limit.min..=limit.max).contains(&value) {
                let MovementLimit { field, min, max, unit } = *limit;
                issues.push(format!("`{field}` is {value}: it must be {min} to {max} {unit}"));
            }
        }
        let Self { height, radius, skin, run_speed, walk_speed, run_stick, walk_stick, .. } = *self;
        if height <= 2.0 * radius {
            issues.push(format!("`height` is {height}: it must be above twice the radius, {radius}"));
        }
        if skin > radius / 4.0 {
            issues.push(format!("`skin` is {skin}: it must be at most a quarter of the radius, {radius}"));
        }
        if run_speed < walk_speed {
            issues.push(format!("`run_speed` is {run_speed}: it must be at least `walk_speed`, {walk_speed}"));
        }
        if run_stick <= walk_stick {
            issues.push(format!("`run_stick` is {run_stick}: it must be above `walk_stick`, {walk_stick}"));
        }
        if self.stance_radius > radius {
            let stance = self.stance_radius;
            issues.push(format!("`stance_radius` is {stance}: it must be within the radius, {radius}"));
        }
        let (speed, gravity) = (self.jump_speed, self.gravity);
        let reach = speed * speed / (2.0 * gravity);
        if reach > MAX_JUMP_REACH {
            issues.push(format!(
                "`jump_speed` is {speed}: at a `gravity` of {gravity} that reaches {reach} m, past the limit of \
                 {MAX_JUMP_REACH}"
            ));
        }
        issues
    }

    /// Every number, in [`MOVEMENT_LIMITS`]' order: destructured, so a new field can't be forgotten.
    fn numbers(&self) -> [f32; 20] {
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
        [
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
            // Rounding only matters far past the 60 a limit asks about.
            coyote_ticks as f32,
            slope_limit_degrees,
            step_height,
            ground_snap,
            stance_radius,
            max_slide_speed,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Today's numbers with one of them changed, by its name in the limits.
    fn with(field: &str, value: f32) -> ControllerProfile {
        let mut profile = ControllerProfile::default();
        let slots: [&mut f32; 19] = [
            &mut profile.height,
            &mut profile.radius,
            &mut profile.skin,
            &mut profile.walk_speed,
            &mut profile.run_speed,
            &mut profile.walk_stick,
            &mut profile.run_stick,
            &mut profile.acceleration,
            &mut profile.deceleration,
            &mut profile.turn_rate_degrees,
            &mut profile.gravity,
            &mut profile.jump_speed,
            &mut profile.air_acceleration,
            &mut profile.max_fall_speed,
            &mut profile.slope_limit_degrees,
            &mut profile.step_height,
            &mut profile.ground_snap,
            &mut profile.stance_radius,
            &mut profile.max_slide_speed,
        ];
        let names = MOVEMENT_LIMITS.iter().map(|limit| limit.field).filter(|name| *name != "coyote_ticks");
        match names.zip(slots).find(|(name, _)| *name == field) {
            Some((_, slot)) => *slot = value,
            None => {
                assert_eq!(field, "coyote_ticks");
                profile.coyote_ticks = value as u32;
            }
        }
        profile
    }

    #[test]
    fn todays_numbers_are_a_good_document() {
        assert_eq!(ControllerProfile::default().document_issues(), Vec::<String>::new());
        assert_eq!(ControllerProfile::default().check(), Ok(()));
    }

    /// The table and the struct name the same numbers in the same order: reading a field through
    /// `numbers` finds the value that was written to that field.
    #[test]
    fn the_limits_read_the_field_they_name() {
        for (index, limit) in MOVEMENT_LIMITS.iter().enumerate() {
            let value = (limit.min + limit.max) / 2.0;
            let value = if limit.field == "coyote_ticks" { value.trunc() } else { value };
            assert_eq!(with(limit.field, value).numbers()[index], value, "{}", limit.field);
        }
    }

    /// Past either end of its limits, each number is refused by its own name.
    #[test]
    fn every_number_is_held_to_its_limits() {
        for limit in MOVEMENT_LIMITS {
            let bad: &[f32] = if limit.field == "coyote_ticks" {
                &[limit.max + 1.0, 4e9]
            } else {
                &[limit.max * 1.5, limit.max + 1.0, limit.min - 1.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 1e30]
            };
            let named = format!("`{}`", limit.field);
            for &value in bad {
                let issues = with(limit.field, value).document_issues();
                assert!(issues.iter().any(|issue| issue.starts_with(&named)), "{named} at {value}: {issues:?}");
            }
        }
    }

    #[test]
    fn what_a_number_must_be_is_said_with_its_unit() {
        let issues = with("run_speed", 600.0).document_issues();
        assert_eq!(issues, ["`run_speed` is 600: it must be 0.1 to 30 m/s"]);
    }

    #[test]
    fn numbers_are_held_to_each_other() {
        let good = ControllerProfile::default();
        for (profile, field) in [
            (ControllerProfile { height: 0.6, radius: 0.3, ..good }, "`height`"),
            (ControllerProfile { skin: 0.05, radius: 0.1, height: 1.0, stance_radius: 0.05, ..good }, "`skin`"),
            (ControllerProfile { run_speed: 1.0, walk_speed: 2.0, ..good }, "`run_speed`"),
            (ControllerProfile { walk_stick: 0.9, run_stick: 0.5, ..good }, "`run_stick`"),
            (ControllerProfile { stance_radius: 0.5, radius: 0.3, ..good }, "`stance_radius`"),
        ] {
            let issues = profile.document_issues();
            assert!(issues.len() == 1 && issues[0].starts_with(field), "{field}: {issues:?}");
        }
    }

    /// `2.2` for `22` is a jump that leaves the arena, so the limit reads the pair.
    #[test]
    fn a_jump_can_only_reach_so_high() {
        let good = ControllerProfile::default();
        let issues = ControllerProfile { gravity: 2.2, ..good }.document_issues();
        assert!(issues.len() == 1 && issues[0].contains("`jump_speed`") && issues[0].contains("2.5"), "{issues:?}");
        let highest = ControllerProfile { jump_speed: 10.0, gravity: 20.0, ..good };
        assert_eq!(highest.document_issues(), Vec::<String>::new(), "exactly the limit");
        let none = ControllerProfile { jump_speed: 0.0, ..good };
        assert_eq!(none.document_issues(), Vec::<String>::new(), "a character needn't jump");
    }

    /// The stricter level holds the other, at each end of each number and all at once.
    #[test]
    fn what_a_document_can_hold_a_controller_can_run() {
        let good = ControllerProfile::default();
        let mut stood = 0;
        for limit in MOVEMENT_LIMITS {
            for value in [limit.min, limit.max] {
                let profile = with(limit.field, value);
                if profile.document_issues().is_empty() {
                    assert_eq!(profile.check(), Ok(()), "{} at {value}", limit.field);
                    stood += 1;
                }
            }
        }
        assert!(stood >= 20, "too few ends stood on their own: {stood}");
        for corner in [
            ControllerProfile { height: 3.0, radius: 1.0, skin: 0.05, stance_radius: 1.0, ..good },
            ControllerProfile { height: 0.5, radius: 0.1, skin: 0.001, stance_radius: 0.0, ..good },
            ControllerProfile { walk_speed: 0.1, run_speed: 0.1, walk_stick: 0.05, run_stick: 0.1, ..good },
            ControllerProfile { walk_speed: 15.0, run_speed: 30.0, walk_stick: 0.95, run_stick: 1.0, ..good },
        ] {
            assert_eq!(corner.document_issues(), Vec::<String>::new());
            assert_eq!(corner.check(), Ok(()));
        }
    }
}
