//! Depth of field from a timeline's shots (M5 L8): what a shot's focus keys say at the playhead
//! ([`ShotFocus`]), and the [`Focus`] a frame hands post once its target is found and the camera's blend is
//! counted ([`FocusFollow`]).
//!
//! A focus key names what the sharp plane passes through: a distance from the eye, a place, or a role's head.
//! Between two keys the plane's distance moves from one target's to the other's by the later key's ease, as the
//! shot does, and a cut lands it at once. A role is found by whoever plays the timeline, every frame, so the
//! plane follows a character as it moves; while it can't be found the plane holds where it last was, and the
//! plane's distance eases toward a target it jumped to over [`FOCUS_FOLLOW_HALF_LIFE`] (a role found again, or
//! lost), so nothing pops but a cut. The blur scales with the takeover's share of the view, so focus comes in
//! with the shot's blend and goes out with the blend back to the gameplay camera.

use glam::Vec3;
use sapphire_core::Name;
use sapphire_look::Focus;
use sapphire_timeline::{CameraKey, FocusKey, FocusOn};

use crate::player::{Origin, Shot};

/// How quickly a focus plane that jumped (a role found or lost) eases to where it now is, as a half-life in
/// seconds: well under a shot's length, and long enough that a jump of a few metres reads as a pull.
pub const FOCUS_FOLLOW_HALF_LIFE: f32 = 0.08;

/// What a focus key's sharp plane passes through, in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FocusAim {
    /// This many metres in front of the eye, along its axis.
    Distance(f32),
    /// A place in the world.
    Point(Vec3),
    /// A character's head, by its role's hash.
    Role(Name),
}

impl FocusAim {
    fn of(on: &FocusOn, origin: &Origin) -> Self {
        match on {
            FocusOn::Distance(distance) => Self::Distance(*distance),
            FocusOn::Point(point) => Self::Point(origin.point(*point)),
            FocusOn::Role(role) => Self::Role(Name::new(role)),
        }
    }

    /// How far the plane is along the view from `eye` looking along `forward`, with `head_of` finding a role's
    /// head: `None` for a role it can't find.
    pub fn distance(&self, eye: Vec3, forward: Vec3, head_of: &impl Fn(Name) -> Option<Vec3>) -> Option<f32> {
        match *self {
            Self::Distance(distance) => Some(distance),
            Self::Point(point) => Some((point - eye).dot(forward)),
            Self::Role(role) => head_of(role).map(|head| (head - eye).dot(forward)),
        }
    }
}

/// What the camera track's focus keys say at the playhead: the targets of the keys either side, how far from
/// one to the other, and the ranges and largest blur between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotFocus {
    pub from: FocusAim,
    pub to: FocusAim,
    /// How far from `from`'s distance to `to`'s, eased by the later key: 0 up to a cut.
    pub share: f32,
    pub sharp: f32,
    pub near: f32,
    pub far: f32,
    /// The largest blur, as a fraction of the picture's height. Between a key with focus and one without, the
    /// blur eases to or from 0 around the one target.
    pub blur: f32,
}

impl ShotFocus {
    /// What `from` and `to`, the camera keys either side of the playhead `share` of the way between them, say.
    /// `None` if neither has focus.
    pub(crate) fn between(from: &CameraKey, to: &CameraKey, share: f32, origin: &Origin) -> Option<Self> {
        let lerp = |a: f32, b: f32| a + (b - a) * share;
        let ranges = |key: &FocusKey| (FocusAim::of(&key.on, origin), key.sharp, key.near, key.far, key.blur);
        let (a, b) = match (&from.focus, &to.focus) {
            (None, None) => return None,
            (Some(a), Some(b)) => (ranges(a), ranges(b)),
            // The side without focus is the other's target and ranges with no blur.
            (Some(a), None) => (ranges(a), (ranges(a).0, a.sharp, a.near, a.far, 0.0)),
            (None, Some(b)) => ((ranges(b).0, b.sharp, b.near, b.far, 0.0), ranges(b)),
        };
        Some(Self {
            from: a.0,
            to: b.0,
            share,
            sharp: lerp(a.1, b.1),
            near: lerp(a.2, b.2),
            far: lerp(a.3, b.3),
            blur: lerp(a.4, b.4),
        })
    }
}

/// Turns each frame's [`ShotFocus`] into the [`Focus`] post draws: finds the target, holds it while it's lost, eases over a jump, lands a cut at once, and scales the blur by the
/// camera's blend. Plain values; allocates nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FocusFollow {
    /// Where the plane is, once a target has been found.
    distance: Option<f32>,
    /// The last frame's focus at full blur, which the blend back to the gameplay camera fades out.
    last: Option<Focus>,
}

impl FocusFollow {
    /// The focus for a frame `dt` real seconds after the last: `shot` is the timeline's, or `None` once it's over
    /// (or without one); `weight` the takeover's share of the view (`CameraDirector::weight(Rig::Takeover)`);
    /// `head_of` where a role's head is in the world, if the game can find it.
    pub fn follow(
        &mut self,
        shot: Option<&Shot>,
        weight: f32,
        dt: f32,
        head_of: impl Fn(Name) -> Option<Vec3>,
    ) -> Focus {
        let weight = if weight.is_finite() { weight.clamp(0.0, 1.0) } else { 0.0 };
        let Some(shot) = shot else {
            // Over: what it last showed fades with the blend back, then nothing.
            if weight == 0.0 {
                *self = Self::default();
            }
            return self.last.map_or(Focus::NONE, |focus| Focus { blur: focus.blur * weight, ..focus });
        };
        let Some(keys) = shot.focus else {
            *self = Self::default();
            return Focus::NONE;
        };
        let forward = shot.rotation * Vec3::Z;
        let distance = |aim: FocusAim| aim.distance(shot.eye, forward, &head_of).filter(|d| d.is_finite());
        let target = match (distance(keys.from), distance(keys.to)) {
            (Some(from), Some(to)) => Some(from + (to - from) * keys.share),
            (Some(one), None) | (None, Some(one)) => Some(one),
            (None, None) => self.distance,
        };
        let Some(target) = target else {
            self.last = None;
            return Focus::NONE;
        };
        let distance = match self.distance {
            Some(held) if !shot.cut && dt.is_finite() && dt > 0.0 => {
                let kept = 0.5_f32.powf(dt / FOCUS_FOLLOW_HALF_LIFE);
                target + (held - target) * kept
            }
            Some(held) if !shot.cut => held,
            _ => target,
        };
        self.distance = Some(distance);
        let focus = Focus { distance, sharp: keys.sharp, near: keys.near, far: keys.far, blur: keys.blur };
        self.last = Some(focus);
        Focus { blur: focus.blur * weight, ..focus }
    }
}

#[cfg(test)]
mod tests {
    use glam::Quat;

    use super::*;

    fn shot(focus: Option<ShotFocus>, cut: bool) -> Shot {
        Shot { eye: Vec3::ZERO, rotation: Quat::IDENTITY, fov_y: 0.6, cut, focus }
    }

    fn on(from: FocusAim, to: FocusAim, share: f32) -> ShotFocus {
        ShotFocus { from, to, share, sharp: 0.5, near: 1.0, far: 6.0, blur: 0.01 }
    }

    const HERO: Name = Name::new("hero");

    #[test]
    fn a_role_is_followed_held_while_lost_and_eased_after_a_jump() {
        let mut follow = FocusFollow::default();
        let head = |z: f32| move |role: Name| (role == HERO).then_some(Vec3::new(0.0, 1.5, z));
        let aim = on(FocusAim::Role(HERO), FocusAim::Role(HERO), 0.0);
        let first = follow.follow(Some(&shot(Some(aim), false)), 1.0, 1.0 / 60.0, head(4.0));
        assert_eq!(first.distance, 4.0, "the first frame lands on its target");
        assert_eq!((first.sharp, first.near, first.far, first.blur), (0.5, 1.0, 6.0, 0.01));
        let lost = follow.follow(Some(&shot(Some(aim), false)), 1.0, 1.0 / 60.0, |_| None);
        assert_eq!(lost.distance, 4.0, "a lost role holds where it was");
        let found = follow.follow(Some(&shot(Some(aim), false)), 1.0, 1.0 / 60.0, head(8.0));
        assert!(found.distance > 4.0 && found.distance < 5.0, "a jump eases: {}", found.distance);
        let mut last = found.distance;
        for _ in 0..60 {
            last = follow.follow(Some(&shot(Some(aim), false)), 1.0, 1.0 / 60.0, head(8.0)).distance;
        }
        assert!((last - 8.0).abs() < 1e-3, "and gets there: {last}");
        let cut = follow.follow(Some(&shot(Some(aim), true)), 1.0, 1.0 / 60.0, head(2.0));
        assert_eq!(cut.distance, 2.0, "a cut lands at once");
    }

    #[test]
    fn targets_mix_by_the_share_and_blur_follows_the_blend() {
        let mut follow = FocusFollow::default();
        let mixed = on(FocusAim::Distance(2.0), FocusAim::Point(Vec3::new(1.0, 0.0, 6.0)), 0.25);
        let focus = follow.follow(Some(&shot(Some(mixed), true)), 0.5, 0.0, |_| None);
        assert_eq!(focus.distance, 3.0, "a quarter of the way from 2 m to the point's 6 m");
        assert_eq!(focus.blur, 0.005, "half the takeover, half the blur");
        // Over: the last focus fades with the blend back, then lets go.
        let fading = follow.follow(None, 0.25, 1.0 / 60.0, |_| None);
        assert_eq!((fading.distance, fading.blur), (3.0, 0.0025));
        assert_eq!(follow.follow(None, 0.0, 1.0 / 60.0, |_| None), Focus::NONE);
        assert_eq!(follow.follow(None, 1.0, 1.0 / 60.0, |_| None), Focus::NONE, "nothing left to fade");
        // A shot without focus is sharp everywhere, at once.
        assert_eq!(follow.follow(Some(&shot(None, false)), 1.0, 0.0, |_| None), Focus::NONE);
        // A role never found has nowhere to be.
        let nobody = on(FocusAim::Role(HERO), FocusAim::Role(HERO), 0.0);
        assert_eq!(follow.follow(Some(&shot(Some(nobody), false)), 1.0, 0.0, |_| None), Focus::NONE);
    }

    #[test]
    fn the_plain_crate_holds_the_looks_largest_blur() {
        assert_eq!(sapphire_timeline::MAX_FOCUS_BLUR, sapphire_look::MAX_FOCUS_BLUR);
    }
}
