//! The proximity fade (M1 C7; spec §5.12 "Proximity fade"): a character fades out as the camera
//! comes close to its body, so a camera pushed against it by a wall or a lock-on never shows the
//! inside of its mesh, as the reference games' third-person cameras do.
//!
//! [`ProximityFade`] is the resolver: from the camera's distance to a character's body capsule
//! ([`CharacterBody`]) to the opacity the frame packet carries
//! ([`CharacterInstance::opacity`]), which the renderer turns into a screen-space dither that
//! every pass drawing the character shares. Resolving allocates nothing.
//!
//! [`CharacterInstance::opacity`]: sapphire_render::CharacterInstance::opacity

use bevy_reflect::Reflect;
use sapphire_math::Vec3;
use sapphire_render::CharacterBody;
use serde::{Deserialize, Serialize};

/// How a character fades as the camera nears its body: whole while the camera is at least
/// `start` meters from the body's surface, gone at `end` or nearer, easing between. A character
/// without a body (radius 0) has no surface to measure from and never fades.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::world"]
#[serde(default, deny_unknown_fields)]
pub struct ProximityFade {
    /// Meters from the body's surface where the fade begins.
    pub start: f32,
    /// Meters from the body's surface where the character is gone; less than `start`.
    pub end: f32,
}

impl Default for ProximityFade {
    /// From half a meter out to a hand's breadth: a gameplay camera (two to four meters back)
    /// never fades a character; one pushed into it by a wall does.
    fn default() -> Self {
        Self { start: 0.5, end: 0.1 }
    }
}

impl ProximityFade {
    /// Whether it can resolve: finite, with `end` at least 0 and below `start`.
    pub fn is_valid(&self) -> bool {
        self.start.is_finite() && self.end.is_finite() && self.end >= 0.0 && self.end < self.start
    }

    /// The opacity of a character with `body` seen from a camera at `camera`: 1 at or beyond
    /// `start` meters from the body's surface (or with an invalid fade, or no body), 0 at `end`
    /// or nearer (inside the body too), smoothstepped between.
    pub fn opacity(&self, camera: Vec3, body: &CharacterBody) -> f32 {
        if !self.is_valid() || !body.casts() || !body.is_valid() {
            return 1.0;
        }
        let distance = segment_distance(camera, body.base, body.top) - body.radius;
        let t = ((distance - self.end) / (self.start - self.end)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

/// How far `point` is from the segment from `a` to `b`.
fn segment_distance(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let along = b - a;
    let length_squared = along.length_squared();
    let t =
        if length_squared > 0.0 { ((point - a).dot(along) / length_squared).clamp(0.0, 1.0) } else { 0.0 };
    point.distance(a + along * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> CharacterBody {
        CharacterBody::capsule(Vec3::new(0.0, 0.25, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.25)
    }

    #[test]
    fn whole_beyond_the_start_and_gone_at_the_end() {
        let fade = ProximityFade::default();
        let at = |x: f32| fade.opacity(Vec3::new(x, 1.0, 0.0), &body());
        // The surface is 0.25 m from the axis.
        assert_eq!(at(0.25 + 0.5), 1.0, "at the start");
        assert_eq!(at(3.0), 1.0, "a gameplay camera");
        assert_eq!(at(0.25 + 0.09), 0.0, "by the end");
        assert_eq!(at(0.0), 0.0, "inside the body");
        let middle = at(0.25 + 0.3);
        assert!((middle - 0.5).abs() < 1e-5, "halfway, smoothstepped: {middle}");
        assert!(at(0.4) < at(0.45) && at(0.45) < at(0.6), "rising with distance");
    }

    #[test]
    fn distance_is_to_the_capsule_not_its_axis_line() {
        let fade = ProximityFade::default();
        // Above the head: measured from the top cap.
        assert_eq!(fade.opacity(Vec3::new(0.0, 1.5 + 0.25 + 0.5, 0.0), &body()), 1.0);
        assert_eq!(fade.opacity(Vec3::new(0.0, 1.5 + 0.25 + 0.09, 0.0), &body()), 0.0);
        // Beside the segment's end, as far from the axis as above it.
        let side = fade.opacity(Vec3::new(0.25 + 0.3, 1.5, 0.0), &body());
        let above = fade.opacity(Vec3::new(0.0, 1.5 + 0.25 + 0.3, 0.0), &body());
        assert!((side - above).abs() < 1e-5);
    }

    #[test]
    fn no_body_or_an_invalid_fade_never_fades() {
        let near = Vec3::new(0.0, 1.0, 0.0);
        assert_eq!(ProximityFade::default().opacity(near, &CharacterBody::default()), 1.0, "no body");
        let broken = CharacterBody { radius: f32::NAN, ..body() };
        assert_eq!(ProximityFade::default().opacity(near, &broken), 1.0);
        for fade in [
            ProximityFade { start: 0.1, end: 0.5 },
            ProximityFade { start: 0.5, end: 0.5 },
            ProximityFade { start: 0.5, end: -0.1 },
            ProximityFade { start: f32::INFINITY, end: 0.1 },
        ] {
            assert!(!fade.is_valid(), "{fade:?}");
            assert_eq!(fade.opacity(near, &body()), 1.0, "{fade:?}");
        }
        assert!(ProximityFade::default().is_valid());
    }

    #[test]
    fn documents_round_trip_and_default_missing_fields() {
        let fade: ProximityFade = sapphire_serialize::ron::from_str("(start: 0.8)").expect("a fade");
        assert_eq!(fade, ProximityFade { start: 0.8, ..ProximityFade::default() });
    }
}
