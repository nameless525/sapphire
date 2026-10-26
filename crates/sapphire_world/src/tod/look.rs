//! The resolved look ([`TodLook`]), as the frame packet's plain data, and how looks blend: colors
//! in OkLab, directions along great circles, scalars linearly, and grades as a pair of LUTs.

use sapphire_math::Vec3;
use sapphire_math::color::{luminance, mix_oklab};
use sapphire_math::det::slerp_direction;
use sapphire_render::{
    AmbientLight, BloomParams, Celestial, CharacterGi, CharacterLight, CharacterLighting, ColorGrade,
    DirectionalLight, Fog, FogLayer, FramePacket, HeightFog, PostParams, SkyGradient, Vignette,
};

use super::key::{
    AmbientKey, BloomKey, CelestialKey, CharacterKey, FogKey, FogLayerKey, GradeKey, HeightFogKey, SkyKey,
    TodKey, VignetteKey,
};

/// The look at one moment, every group resolved: what time of day hands the frame packet.
///
/// [`Self::default`] is the renderer's defaults, which a complete timeline overwrites whole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TodLook {
    /// The sun itself; see [`Self::key_light`] for the light that shades the scene.
    pub sun: DirectionalLight,
    pub moon: DirectionalLight,
    pub sky: SkyGradient,
    pub ambient: AmbientLight,
    pub fog: Fog,
    pub shadow_tint: Vec3,
    pub character_light: CharacterLight,
    pub day_night: f32,
    pub exposure: f32,
    pub grades: GradeMix,
    pub bloom: BloomParams,
    pub vignette: Vignette,
}

impl Default for TodLook {
    fn default() -> Self {
        let post = PostParams::default();
        let celestial = Celestial::default();
        Self {
            sun: celestial.sun,
            moon: celestial.moon,
            sky: SkyGradient::default(),
            ambient: AmbientLight::default(),
            fog: Fog::default(),
            shadow_tint: Vec3::ZERO,
            character_light: CharacterLight::default(),
            day_night: 0.0,
            exposure: post.exposure,
            grades: GradeMix::single(post.grade_a),
            bloom: post.bloom,
            vignette: post.vignette,
        }
    }
}

impl TodLook {
    /// The world's key light: the light that shades the scene and casts its shadows.
    ///
    /// A body gives light only from above the horizon: its intensity fades out over its last
    /// degree, so neither ever lights the scene from below, whatever the keys say between them.
    /// Higher up, a body's light is exactly as keyed.
    ///
    /// The key light is the sun while the moon gives no light, and the moon while the sun gives
    /// none. While both shine (twilight), their light adds up and the direction turns from the
    /// sun's toward the moon's in proportion to the moon's share of the luminance, so the
    /// terminator glides across characters rather than jumping when the sun sets.
    pub fn key_light(&self) -> DirectionalLight {
        let (sun, moon) = (above_horizon(&self.sun), above_horizon(&self.moon));
        let strength = |light: &DirectionalLight| light.intensity.max(0.0) * luminance(light.color).max(0.0);
        let (sun_strength, moon_strength) = (strength(&sun), strength(&moon));
        // Written so a NaN strength counts as no light.
        match (sun_strength > 0.0, moon_strength > 0.0) {
            (_, false) => return sun,
            (false, true) => return moon,
            (true, true) => {}
        }
        let intensity = sun.intensity + moon.intensity;
        let radiance = sun.color * sun.intensity + moon.color * moon.intensity;
        DirectionalLight {
            direction: slerp_direction(
                sun.direction,
                moon.direction,
                moon_strength / (sun_strength + moon_strength),
            ),
            color: radiance / intensity,
            intensity,
        }
    }

    /// Post-processing for this look: exposure, bloom, the two grades and their blend, and the
    /// vignette, with the default tone curve and full filter strength (the player's setting is
    /// the app's to apply).
    pub fn post(&self) -> PostParams {
        let (grade_a, grade_b, grade_blend) = self.grades.lut_pair();
        PostParams {
            exposure: self.exposure,
            bloom: self.bloom,
            grade_a,
            grade_b,
            grade_blend,
            vignette: self.vignette,
            ..PostParams::default()
        }
    }

    /// Writes the look into `packet`'s scene constants: the key light, the sun and moon, sky,
    /// ambient, fog, shadow tint, character light, day/night factor and post-processing.
    pub fn write(&self, packet: &mut FramePacket) {
        packet.sun = self.key_light();
        packet.celestial = Celestial { sun: self.sun, moon: self.moon };
        packet.sky = self.sky;
        packet.ambient = self.ambient;
        packet.fog = self.fog;
        packet.shadow_tint = self.shadow_tint;
        packet.character_light = self.character_light;
        packet.day_night = self.day_night;
        packet.post = self.post();
    }

    /// Blends the groups `from` and `to` both set into the look with `weight`: each group's
    /// value is `segment` of the way from `from`'s to `to`'s, then that value is blended
    /// `weight` of the way over what the look had. A weight of 1 replaces the groups outright,
    /// and a segment of 0 takes `from`'s values exactly.
    pub(crate) fn blend_keys(&mut self, from: &TodKey, to: &TodKey, segment: f32, weight: f32) {
        fn group<K: Resolve>(
            slot: &mut K::Value,
            from: Option<&K>,
            to: Option<&K>,
            segment: f32,
            weight: f32,
        ) {
            if let (Some(from), Some(to)) = (from, to) {
                let value =
                    if segment > 0.0 { mix(&from.resolve(), &to.resolve(), segment) } else { from.resolve() };
                *slot = mix(slot, &value, weight);
            }
        }
        group(&mut self.sun, from.sun.as_ref(), to.sun.as_ref(), segment, weight);
        group(&mut self.moon, from.moon.as_ref(), to.moon.as_ref(), segment, weight);
        group(&mut self.sky, from.sky.as_ref(), to.sky.as_ref(), segment, weight);
        group(&mut self.ambient, from.ambient.as_ref(), to.ambient.as_ref(), segment, weight);
        group(&mut self.fog, from.fog.as_ref(), to.fog.as_ref(), segment, weight);
        group(&mut self.shadow_tint, from.shadow_tint.as_ref(), to.shadow_tint.as_ref(), segment, weight);
        group(&mut self.character_light, from.character.as_ref(), to.character.as_ref(), segment, weight);
        group(&mut self.day_night, from.day_night.as_ref(), to.day_night.as_ref(), segment, weight);
        group(&mut self.exposure, from.exposure.as_ref(), to.exposure.as_ref(), segment, weight);
        group(&mut self.grades, from.grade.as_ref(), to.grade.as_ref(), segment, weight);
        group(&mut self.bloom, from.bloom.as_ref(), to.bloom.as_ref(), segment, weight);
        group(&mut self.vignette, from.vignette.as_ref(), to.vignette.as_ref(), segment, weight);
    }
}

/// The height above the horizon, as the sine of the elevation (about 1°), over which a body's
/// light fades out as it sets.
const HORIZON_FADE: f32 = 0.0175;

/// `light` as it reaches the scene: unchanged while its body is more than [`HORIZON_FADE`]
/// above the horizon, fading to nothing as it reaches it, and dark below.
fn above_horizon(light: &DirectionalLight) -> DirectionalLight {
    // The body sits at `-direction`; a zero direction (never valid in a key) counts as set.
    let height = -light.direction.y / light.direction.length();
    let fade = sapphire_math::Ease::Smooth.apply(height / HORIZON_FADE);
    if fade >= 1.0 { *light } else { DirectionalLight { intensity: light.intensity * fade, ..*light } }
}

/// Up to two color grades and the blend between them: what post's two LUTs can show (spec
/// §5.12, "Tonemapping"). A grade is baked into a LUT, a few milliseconds of CPU, so grades are
/// never interpolated field by field; neighboring keys' grades blend as two LUTs instead, and a
/// key's grade is baked once when time reaches its segment.
///
/// Blending two mixes can bring up to four grades into play. Equal grades merge, and the two
/// that weigh most are kept, renormalized; whatever else weighed is counted in
/// [`Self::dropped`]. A zone that changes the grade while time crosses between keys of
/// different grades is the case that drops weight (a third LUT, or a GPU bake of the mix, would
/// lift it; see the module's later milestones).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradeMix {
    pub a: ColorGrade,
    pub b: ColorGrade,
    /// 0 shows `a` alone, 1 `b` alone.
    pub blend: f32,
    /// The share of the picture whose grade was left out of the pair, 0 to 1.
    pub dropped: f32,
}

impl GradeMix {
    /// `grade` alone.
    pub fn single(grade: ColorGrade) -> Self {
        Self { a: grade, b: grade, blend: 0.0, dropped: 0.0 }
    }

    /// The grades for post's two LUTs and their blend, with LUT B unused (equal to A, blend 0)
    /// whenever one grade shows alone, so post bakes and samples one LUT.
    pub fn lut_pair(&self) -> (ColorGrade, ColorGrade, f32) {
        if self.blend >= 1.0 {
            (self.b, self.b, 0.0)
        } else if self.blend > 0.0 {
            (self.a, self.b, self.blend)
        } else {
            (self.a, self.a, 0.0)
        }
    }
}

/// Blends `a` toward `b` by `t`, exactly `a` at `t ≤ 0` (or NaN) and exactly `b` at `t ≥ 1`, so
/// time on a key, and a volume at full weight, reproduce their values bit for bit.
pub(crate) fn mix<T: Mix>(a: &T, b: &T, t: f32) -> T {
    if t >= 1.0 {
        *b
    } else if t > 0.0 {
        a.mix(b, t)
    } else {
        *a
    }
}

/// A value that blends: [`mix`] calls it only with `0 < t < 1`.
pub(crate) trait Mix: Copy {
    fn mix(&self, other: &Self, t: f32) -> Self;
}

/// An authored group, and the value it resolves to.
trait Resolve {
    type Value: Mix;

    fn resolve(&self) -> Self::Value;
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Mix for f32 {
    fn mix(&self, other: &Self, t: f32) -> Self {
        lerp(*self, *other, t)
    }
}

/// A bare `Vec3` group is a color (the shadow tint).
impl Mix for Vec3 {
    fn mix(&self, other: &Self, t: f32) -> Self {
        mix_oklab(*self, *other, t)
    }
}

impl Mix for DirectionalLight {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            direction: slerp_direction(self.direction, other.direction, t),
            color: mix_oklab(self.color, other.color, t),
            intensity: lerp(self.intensity, other.intensity, t),
        }
    }
}

impl Mix for SkyGradient {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            zenith: mix_oklab(self.zenith, other.zenith, t),
            horizon: mix_oklab(self.horizon, other.horizon, t),
            ground: mix_oklab(self.ground, other.ground, t),
        }
    }
}

impl Mix for AmbientLight {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            sky: mix_oklab(self.sky, other.sky, t),
            ground: mix_oklab(self.ground, other.ground, t),
            intensity: lerp(self.intensity, other.intensity, t),
        }
    }
}

impl Mix for FogLayer {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            color: mix_oklab(self.color, other.color, t),
            density: lerp(self.density, other.density, t),
            start: lerp(self.start, other.start, t),
        }
    }
}

impl Mix for Fog {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            near: self.near.mix(&other.near, t),
            far: self.far.mix(&other.far, t),
            height: HeightFog {
                color: mix_oklab(self.height.color, other.height.color, t),
                density: lerp(self.height.density, other.height.density, t),
                falloff: lerp(self.height.falloff, other.height.falloff, t),
                base: lerp(self.height.base, other.height.base, t),
            },
            sun_inscatter: mix_oklab(self.sun_inscatter, other.sun_inscatter, t),
        }
    }
}

impl Mix for CharacterLight {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            color: mix_oklab(self.color, other.color, t),
            intensity: lerp(self.intensity, other.intensity, t),
        }
    }
}

/// A character's lighting (M1 C2), as a shot's light rig blends over the character's own: the
/// key's direction along the great circle, colors in OkLab, scalars linearly, and the value
/// ceiling through its reciprocal, so no ceiling (infinite) blends smoothly into one (a ceiling
/// under 10⁻⁶ blends as 10⁻⁶).
impl Mix for CharacterLighting {
    fn mix(&self, other: &Self, t: f32) -> Self {
        let (a, b) = (&self.gi, &other.gi);
        let reciprocal = |value: f32| 1.0 / value.max(1e-6);
        Self {
            key: self.key.mix(&other.key, t),
            rim: lerp(self.rim, other.rim, t),
            local_light_ceiling: lerp(self.local_light_ceiling, other.local_light_ceiling, t),
            gi: CharacterGi {
                head: a.head.zip(b.head).map(|(a, b)| a.lerp(b, t)).or(b.head).or(a.head),
                intensity: lerp(a.intensity, b.intensity, t),
                camera_bias: lerp(a.camera_bias, b.camera_bias, t),
                max_saturation: lerp(a.max_saturation, b.max_saturation, t),
                max_value: lerp(reciprocal(a.max_value), reciprocal(b.max_value), t).recip(),
                skin_tint: mix_oklab(a.skin_tint, b.skin_tint, t),
            },
        }
    }
}

impl Mix for BloomParams {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            threshold: lerp(self.threshold, other.threshold, t),
            knee: lerp(self.knee, other.knee, t),
            intensity: lerp(self.intensity, other.intensity, t),
            scatter: lerp(self.scatter, other.scatter, t),
        }
    }
}

impl Mix for Vignette {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self { intensity: lerp(self.intensity, other.intensity, t), start: lerp(self.start, other.start, t) }
    }
}

impl Mix for GradeMix {
    fn mix(&self, other: &Self, t: f32) -> Self {
        let weighted = [
            (self.a, (1.0 - t) * (1.0 - self.blend)),
            (self.b, (1.0 - t) * self.blend),
            (other.a, t * (1.0 - other.blend)),
            (other.b, t * other.blend),
        ];
        // Equal grades merge into their first appearance, which keeps A and B in the order the
        // grades came in, so post's LUT slots stay put while a blend runs.
        let mut grades = [(ColorGrade::NEUTRAL, 0.0); 4];
        let mut count = 0;
        for (grade, weight) in weighted.into_iter().filter(|&(_, weight)| weight > 0.0) {
            match grades[..count].iter_mut().find(|(seen, _)| *seen == grade) {
                Some((_, total)) => *total += weight,
                None => {
                    grades[count] = (grade, weight);
                    count += 1;
                }
            }
        }
        let carried = (1.0 - t) * self.dropped + t * other.dropped;
        let grades = &grades[..count];
        match grades {
            [] => *self,
            [(grade, _)] => Self { dropped: carried, ..Self::single(*grade) },
            _ => {
                // The two heaviest, in order of appearance; earlier ones win ties.
                let heaviest = |skip: Option<usize>| {
                    (0..grades.len())
                        .filter(|&index| Some(index) != skip)
                        .reduce(|best, index| if grades[index].1 > grades[best].1 { index } else { best })
                        .expect("at least two grades")
                };
                let first = heaviest(None);
                let second = heaviest(Some(first));
                let (a, b) = (first.min(second), first.max(second));
                let (weight_a, weight_b) = (grades[a].1, grades[b].1);
                let total: f32 = grades.iter().map(|(_, weight)| weight).sum();
                let kept = weight_a + weight_b;
                Self {
                    a: grades[a].0,
                    b: grades[b].0,
                    blend: weight_b / kept,
                    // What this blend drops, of the share not already counted.
                    dropped: carried + (1.0 - carried) * (total - kept) / total,
                }
            }
        }
    }
}

impl Mix for TodLook {
    fn mix(&self, other: &Self, t: f32) -> Self {
        Self {
            sun: self.sun.mix(&other.sun, t),
            moon: self.moon.mix(&other.moon, t),
            sky: self.sky.mix(&other.sky, t),
            ambient: self.ambient.mix(&other.ambient, t),
            fog: self.fog.mix(&other.fog, t),
            shadow_tint: self.shadow_tint.mix(&other.shadow_tint, t),
            character_light: self.character_light.mix(&other.character_light, t),
            day_night: self.day_night.mix(&other.day_night, t),
            exposure: self.exposure.mix(&other.exposure, t),
            grades: self.grades.mix(&other.grades, t),
            bloom: self.bloom.mix(&other.bloom, t),
            vignette: self.vignette.mix(&other.vignette, t),
        }
    }
}

impl Resolve for CelestialKey {
    type Value = DirectionalLight;

    fn resolve(&self) -> DirectionalLight {
        DirectionalLight {
            direction: self.direction.into(),
            color: self.color.into(),
            intensity: self.intensity,
        }
    }
}

impl Resolve for SkyKey {
    type Value = SkyGradient;

    fn resolve(&self) -> SkyGradient {
        SkyGradient { zenith: self.zenith.into(), horizon: self.horizon.into(), ground: self.ground.into() }
    }
}

impl Resolve for AmbientKey {
    type Value = AmbientLight;

    fn resolve(&self) -> AmbientLight {
        AmbientLight { sky: self.sky.into(), ground: self.ground.into(), intensity: self.intensity }
    }
}

impl Resolve for FogKey {
    type Value = Fog;

    fn resolve(&self) -> Fog {
        let layer =
            |key: &FogLayerKey| FogLayer { color: key.color.into(), density: key.density, start: key.start };
        let HeightFogKey { color, density, falloff, base } = self.height;
        Fog {
            near: layer(&self.near),
            far: layer(&self.far),
            height: HeightFog { color: color.into(), density, falloff, base },
            sun_inscatter: self.sun_inscatter.into(),
        }
    }
}

/// The shadow tint.
impl Resolve for [f32; 3] {
    type Value = Vec3;

    fn resolve(&self) -> Vec3 {
        Vec3::from(*self)
    }
}

impl Resolve for CharacterKey {
    type Value = CharacterLight;

    fn resolve(&self) -> CharacterLight {
        CharacterLight { color: self.color.into(), intensity: self.intensity }
    }
}

/// The day/night factor and exposure.
impl Resolve for f32 {
    type Value = f32;

    fn resolve(&self) -> f32 {
        *self
    }
}

impl Resolve for GradeKey {
    type Value = GradeMix;

    fn resolve(&self) -> GradeMix {
        GradeMix::single((*self).into())
    }
}

impl Resolve for BloomKey {
    type Value = BloomParams;

    fn resolve(&self) -> BloomParams {
        (*self).into()
    }
}

impl Resolve for VignetteKey {
    type Value = Vignette;

    fn resolve(&self) -> Vignette {
        (*self).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grade(saturation: f32) -> ColorGrade {
        ColorGrade { saturation, ..ColorGrade::NEUTRAL }
    }

    #[test]
    fn mixing_is_exact_at_both_ends() {
        let (a, b) = (
            DirectionalLight {
                direction: Vec3::new(0.55, -0.7, -0.45),
                color: Vec3::new(1.0, 0.97, 0.92),
                intensity: 1.0,
            },
            DirectionalLight { direction: Vec3::X, color: Vec3::new(1.0, 0.5, 0.3), intensity: 0.5 },
        );
        assert_eq!(mix(&a, &b, 0.0), a);
        assert_eq!(mix(&a, &b, f32::NAN), a);
        assert_eq!(mix(&a, &b, 1.0), b);
        let half = mix(&a, &b, 0.5);
        assert!(half.direction.is_normalized());
        assert_eq!(half.intensity, 0.75);
    }

    #[test]
    fn bodies_below_the_horizon_give_no_light() {
        let toward = |elevation_degrees: f32| {
            let (sin, cos) = elevation_degrees.to_radians().sin_cos();
            -Vec3::new(cos, sin, 0.0)
        };
        let sun = DirectionalLight { direction: toward(-3.0), color: Vec3::ONE, intensity: 0.8 };
        let moon = DirectionalLight { direction: toward(40.0), color: Vec3::ONE, intensity: 0.0 };
        let look = TodLook { sun, moon, ..TodLook::default() };
        assert_eq!(look.key_light().intensity, 0.0, "set: no light from below");
        let low = TodLook { sun: DirectionalLight { direction: toward(0.5), ..sun }, ..look };
        let faded = low.key_light().intensity;
        assert!(faded > 0.0 && faded < 0.8, "fading over the last degree: {faded}");
        let up = TodLook { sun: DirectionalLight { direction: toward(1.5), ..sun }, ..look };
        assert_eq!(up.key_light(), up.sun, "higher up, exactly as keyed");
        let night = TodLook { moon: DirectionalLight { intensity: 0.3, ..moon }, ..look };
        assert_eq!(night.key_light(), night.moon, "the moon alone, while the sun is down");
    }

    #[test]
    fn character_lighting_blends_exactly_at_both_ends_and_lifts_no_ceiling_smoothly() {
        let a = CharacterLighting::unstyled(DirectionalLight::default());
        let b = CharacterLighting {
            key: DirectionalLight { direction: Vec3::X, color: Vec3::new(1.0, 0.5, 0.3), intensity: 0.5 },
            gi: CharacterGi { max_value: 0.4, camera_bias: 0.6, ..CharacterGi::default() },
            ..a
        };
        assert_eq!(mix(&a, &b, 0.0), a);
        assert_eq!(mix(&a, &b, f32::NAN), a);
        assert_eq!(mix(&a, &b, 1.0), b);
        let half = mix(&a, &b, 0.5);
        assert!(half.key.direction.is_normalized());
        assert_eq!((half.key.intensity, half.gi.camera_bias), (0.75, 0.3));
        // No ceiling (infinite) blends through its reciprocal: halfway is twice the ceiling.
        assert!((half.gi.max_value - 0.8).abs() < 1e-5, "{}", half.gi.max_value);
        assert!(mix(&a, &b, 0.01).gi.max_value > 30.0, "barely in, barely clamped");
    }

    #[test]
    fn neighboring_grades_blend_as_a_lut_pair() {
        let (day, dusk) = (GradeMix::single(grade(1.15)), GradeMix::single(grade(0.9)));
        let mixed = mix(&day, &dusk, 0.25);
        assert_eq!((mixed.a, mixed.b, mixed.blend, mixed.dropped), (grade(1.15), grade(0.9), 0.25, 0.0));
        // Equal grades merge, so a single grade never spends a second LUT.
        let same = mix(&day, &day, 0.5);
        assert_eq!(same, day);
        assert_eq!(same.lut_pair(), (grade(1.15), grade(1.15), 0.0));
        // A full blend shows B alone through LUT A.
        let full = GradeMix { blend: 1.0, ..mixed };
        assert_eq!(full.lut_pair(), (grade(0.9), grade(0.9), 0.0));
    }

    #[test]
    fn a_third_grade_drops_the_lightest() {
        // Time halfway between two keys, under a zone at 30% with a grade of its own.
        let time = mix(&GradeMix::single(grade(1.0)), &GradeMix::single(grade(2.0)), 0.5);
        let zone = GradeMix::single(grade(3.0));
        let mixed = mix(&time, &zone, 0.3);
        // Weights 0.35, 0.35 and 0.3: the zone's grade is dropped.
        assert_eq!((mixed.a, mixed.b), (grade(1.0), grade(2.0)));
        assert!((mixed.blend - 0.5).abs() < 1e-6);
        assert!((mixed.dropped - 0.3).abs() < 1e-6);
        // At 40% the zone outweighs both, and keeps the earlier of the tied pair.
        let mixed = mix(&time, &zone, 0.4);
        assert_eq!((mixed.a, mixed.b), (grade(1.0), grade(3.0)));
        assert!((mixed.blend - 0.4 / 0.7).abs() < 1e-6);
        assert!((mixed.dropped - 0.3).abs() < 1e-6);
    }

    #[test]
    fn the_key_light_follows_whichever_body_shines() {
        let sun = DirectionalLight {
            direction: Vec3::new(1.0, -0.2, 0.0),
            color: Vec3::new(1.0, 0.5, 0.3),
            intensity: 0.5,
        };
        let moon = DirectionalLight {
            direction: Vec3::new(-1.0, -1.0, 0.0),
            color: Vec3::new(0.6, 0.7, 1.0),
            intensity: 0.0,
        };
        let look = TodLook { sun, moon, ..TodLook::default() };
        assert_eq!(look.key_light(), sun, "the moon is dark: exactly the sun");
        let night = TodLook {
            sun: DirectionalLight { intensity: 0.0, ..sun },
            moon: DirectionalLight { intensity: 0.3, ..moon },
            ..look
        };
        assert_eq!(night.key_light(), night.moon, "the sun is dark: exactly the moon");

        let twilight = TodLook { moon: DirectionalLight { intensity: 0.5, ..moon }, ..look };
        let key = twilight.key_light();
        assert_eq!(key.intensity, 1.0);
        let radiance = key.color * key.intensity;
        assert!(radiance.abs_diff_eq(sun.color * 0.5 + moon.color * 0.5, 1e-6), "their light adds up");
        let (to_sun, to_moon) = (sun.direction.normalize(), moon.direction.normalize());
        // The moon's luminance share sets how far the direction has turned.
        let share = luminance(moon.color) / (luminance(moon.color) + luminance(sun.color));
        let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos();
        let turned = angle(key.direction, to_sun) / angle(to_moon, to_sun);
        assert!((turned - share).abs() < 1e-4, "{turned} vs {share}");
    }
}
