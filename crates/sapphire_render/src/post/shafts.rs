//! Light shafts from a low sun (M5 L13, spec §5.12 "Light shafts"): at golden hour, light streaks from the sun past
//! roofs, eaves and trees. A screen-space march over the sky the frame shows, in two passes and a variant of
//! bloom's last up pass (`post_shafts.wgsl`):
//!
//! - **The mask** ([`MASK_PASS`], an eighth of post's input): each texel's share of open sky, from the scene
//!   prepass's depth at sixteen points across its footprint. Only the sky stays at the far plane, so nothing that
//!   draws depth casts: not a lantern, not a lit wall, not a character's glow. The prepass's depth is there under
//!   temporal AA (the desktop tiers), and elsewhere only while GTAO runs (Mobile-High): shafts never make the
//!   prepass run, and a frame without that depth draws none.
//! - **The march** ([`MARCH_PASS`], a quarter): each pixel's share of open sky along its line to the sun, weighted
//!   from 1 at the pixel to half at the sun and normalised, so a line all open sky takes exactly 1; times a fade to
//!   nothing at the shafts' length from the sun. One tap a mask texel along the longest line ([`taps`]), each
//!   pixel's taps offset by a fixed per-pixel pattern: grain at an occluder's edge, never bands, and the same
//!   every frame, so nothing crawls on a still camera.
//! - **The composite** is bloom's top up level drawn by `fs_bloom_up_shafts` in place of `fs_bloom_up`: the
//!   march's share times the shafts' light over bloom's intensity, added once, after bloom's own blur and
//!   threshold, so the fused pass's `bloom × intensity` adds exactly their light into the exposed frame before the
//!   grade. That level is half-float on a shaft frame (bloom's 11-bit floats would band a soft ramp) and bloom's
//!   own format on every other: the fused pass and its pipelines are untouched.
//! - **Where the sun is** ([`ShaftFrame::new`]): the sky's sun disc's direction, projected through the main view.
//!   Shafts fade out as the sun leaves the frame, over the shafts' length beyond its edges, and as it sets, over
//!   the degree above the horizon the sky's own light fades over; behind the camera they're gone, continuously,
//!   as the projected place runs off to infinity first.
//! - **Built with the renderer** on a tier that has them (Desktop-High, Desktop-Low, Mobile-High): three
//!   pipelines and one small buffer, so the frame a key's strength first rises from zero builds nothing. None on
//!   another tier.

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
use sapphire_look::post::{LightShafts, MAX_SHAFT_LENGTH};
use sapphire_rhi::wgpu;

use crate::CameraData;

/// The mask's pass: the share of open sky, at an eighth of post's input.
pub const MASK_PASS: &str = "shaft mask";
/// The march's pass: each pixel's share of the shafts' light, at a quarter.
pub const MARCH_PASS: &str = "light shafts";
/// Both, in the order a frame draws them: what the bench and the overlay sum for the shafts' GPU time (bloom's
/// top up level, which composites them, counts as bloom's).
pub const PASSES: [&str; 2] = [MASK_PASS, MARCH_PASS];

/// The mask's size, as a divisor of post's input.
pub(crate) const MASK_DIVISOR: u32 = 8;
/// The march's.
pub(crate) const MARCH_DIVISOR: u32 = 4;
/// The mask's format: a share from 0 to 1.
pub(crate) const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The march's: a smooth share, which 8 bits would band.
pub(crate) const MARCH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
/// Bloom's top up level on a shaft frame: half floats, so the shafts' soft ramp keeps its steps below a display
/// step (bloom's 11-bit floats hold 6 bits of mantissa).
pub(crate) const UP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The fewest taps a march takes, for the shortest shafts.
pub const MIN_TAPS: u32 = 8;
/// The most, at the longest shafts and the tallest mask: 1 080 rows / 8 is 135.
pub const MAX_TAPS: u32 = 160;

/// The sine of the sun's elevation over which shafts fade in above the horizon: one degree, as the sky's own sun
/// fades (`sapphire_tod`'s horizon fade).
pub const HORIZON_FADE: f32 = 0.017_452;

/// What a bloom level holds at most: half float's largest, which is also 11-bit float's (`post.wgsl`'s
/// `BLOOM_MAX`).
const BLOOM_MAX: f32 = 65_024.0;

/// The march's taps for shafts `length` frame heights long over a mask `mask_rows` tall: one a mask texel along
/// the longest line a lit pixel marches, within [`MIN_TAPS`] and [`MAX_TAPS`].
pub fn taps(length: f32, mask_rows: u32) -> u32 {
    let reach = length.clamp(0.0, MAX_SHAFT_LENGTH) * mask_rows as f32;
    (reach.ceil() as u32).clamp(MIN_TAPS, MAX_TAPS)
}

/// The shafts a frame draws, before the sizes it records at: where the sun is, how far they reach, and their
/// light, faded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShaftFrame {
    /// The sun's place in texture coordinates (0 at the top-left, 1 at the bottom-right); it may be outside.
    pub uv: Vec2,
    /// How far they reach, in frame heights.
    pub length: f32,
    /// The frame's width over its height.
    pub aspect: f32,
    /// Their light at the sun, exposed and display-relative as bloom's is: strength × tint × the fades.
    pub light: Vec3,
}

impl ShaftFrame {
    /// The shafts of `shafts` for `camera` (unjittered) with the sun `toward` (from the sky's disc), of a frame
    /// `aspect` wide: `None` where they draw nothing, the sun behind the camera, set, or too far outside the
    /// frame, or the key asks for none.
    pub(crate) fn new(shafts: &LightShafts, camera: &CameraData, toward: Vec3, aspect: f32) -> Option<Self> {
        if !shafts.drawn() || !toward.is_finite() || toward == Vec3::ZERO {
            return None;
        }
        let toward = toward.normalize();
        let length = shafts.length.min(MAX_SHAFT_LENGTH);
        // A direction is a point at infinity: w = 0.
        let clip = (camera.proj * camera.view) * toward.extend(0.0);
        if !(clip.w > 1e-6 && clip.is_finite()) {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let uv = Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        let fade = margin_fade(uv, aspect, length) * horizon_fade(toward.y);
        let light = shafts.tint.max(Vec3::ZERO) * shafts.strength.max(0.0) * fade;
        (fade > 0.0 && uv.is_finite() && light.is_finite()).then_some(Self { uv, length, aspect, light })
    }

    /// The constants of a frame whose post input is `input` texels, with bloom's `intensity` (above 0: a frame
    /// draws shafts only with bloom). The light over the intensity is held within a bloom level's range, so the
    /// fused pass's `bloom × intensity` gives it back.
    pub(crate) fn uniforms(&self, input: (u32, u32), intensity: f32) -> ShaftUniforms {
        let mask = mask_size(input);
        let taps = taps(self.length, mask.1);
        let light = (self.light / intensity.max(1e-6)).clamp(Vec3::ZERO, Vec3::splat(BLOOM_MAX));
        ShaftUniforms {
            sun: [self.uv.x, self.uv.y, self.length, self.aspect],
            light: [light.x, light.y, light.z, taps as f32],
            mask: [1.0 / mask.0 as f32, 1.0 / mask.1 as f32, 0.0, 0.0],
        }
    }
}

/// The mask's size for post's input of `input` texels.
pub(crate) fn mask_size(input: (u32, u32)) -> (u32, u32) {
    (input.0.div_ceil(MASK_DIVISOR).max(1), input.1.div_ceil(MASK_DIVISOR).max(1))
}

/// The march's.
pub(crate) fn march_size(input: (u32, u32)) -> (u32, u32) {
    (input.0.div_ceil(MARCH_DIVISOR).max(1), input.1.div_ceil(MARCH_DIVISOR).max(1))
}

/// 1 with the sun in frame, falling smoothly to 0 as it leaves, over `length` frame heights past the edge.
fn margin_fade(uv: Vec2, aspect: f32, length: f32) -> f32 {
    let outside = Vec2::new((uv.x.clamp(0.0, 1.0) - uv.x) * aspect, uv.y.clamp(0.0, 1.0) - uv.y).length();
    smoothstep(1.0 - outside / length.max(1e-6))
}

/// 1 with the sun a degree or more above the horizon, falling smoothly to 0 at it.
fn horizon_fade(sine: f32) -> f32 {
    smoothstep(sine / HORIZON_FADE)
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The shafts' constants (WGSL `ShaftUniforms` in `post_shafts.wgsl`, group 0 binding 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct ShaftUniforms {
    /// xy: the sun's place in texture coordinates; z: the shafts' length in frame heights; w: the aspect.
    pub sun: [f32; 4],
    /// rgb: their light over bloom's intensity, faded; w: the march's taps.
    pub light: [f32; 4],
    /// xy: one mask texel in texture coordinates.
    pub mask: [f32; 4],
}

/// One of the shafts' three pipelines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShaftPass {
    Mask,
    March,
    /// Bloom's top up level with the shafts added.
    Up,
}

impl ShaftPass {
    pub(super) const ALL: [Self; 3] = [Self::Mask, Self::March, Self::Up];

    /// Its pipeline's name and its fragment shader's entry point.
    pub(super) const fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Mask => (MASK_PASS, "fs_shaft_mask"),
            Self::March => (MARCH_PASS, "fs_shaft_march"),
            Self::Up => ("bloom up with shafts", "fs_bloom_up_shafts"),
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Mat4;

    use super::*;

    const EVENING: LightShafts = LightShafts { strength: 0.6, tint: Vec3::new(1.0, 0.7, 0.4), length: 0.6 };

    /// A camera at the origin looking down −Z, 16:9.
    fn camera() -> CameraData {
        CameraData {
            view: Mat4::IDENTITY,
            proj: glam::camera::rh::proj::directx::perspective_infinite_reverse(
                60f32.to_radians(),
                16.0 / 9.0,
                0.1,
            ),
            position: Vec3::ZERO,
        }
    }

    /// The sun `yaw` degrees right of the view's axis and `pitch` above the horizon.
    fn sun(yaw: f32, pitch: f32) -> Vec3 {
        let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
        Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos())
    }

    fn frame(yaw: f32, pitch: f32) -> Option<ShaftFrame> {
        ShaftFrame::new(&EVENING, &camera(), sun(yaw, pitch), 16.0 / 9.0)
    }

    #[test]
    fn the_sun_lands_where_the_view_sees_it_and_shines_in_frame() {
        let ahead = frame(0.0, 10.0).expect("the sun ahead");
        assert!((ahead.uv.x - 0.5).abs() < 1e-5 && ahead.uv.y < 0.5, "{:?}", ahead.uv);
        // In frame and well above the horizon: the whole light.
        assert!((ahead.light - EVENING.tint * EVENING.strength).abs().max_element() < 1e-5);
        // Right of the axis lands right of the middle.
        assert!(frame(20.0, 10.0).expect("in frame").uv.x > 0.5);
    }

    #[test]
    fn shafts_fade_as_the_sun_leaves_the_frame_and_are_gone_behind_the_camera() {
        // The frame's half-width is about 46 degrees: past it the light falls, continuously, to none.
        let mut last = f32::MAX;
        for step in 0..=240 {
            let yaw = 40.0 + step as f32 * 0.25;
            let light = frame(yaw, 10.0).map_or(0.0, |frame| frame.light.x);
            assert!(light <= last + 1e-6, "rises at {yaw}");
            assert!(last == f32::MAX || last - light < 0.03, "jumps at {yaw}: {last} to {light}");
            last = light;
        }
        assert_eq!(last, 0.0, "gone well outside the frame");
        // Beside and behind the camera: nothing, and no frame (no pass records).
        assert!(frame(90.0, 10.0).is_none());
        assert!(frame(180.0, 10.0).is_none());
        assert!(frame(120.0, 10.0).is_none());
    }

    #[test]
    fn shafts_fade_as_the_sun_sets_and_are_gone_under_the_horizon() {
        let full = frame(0.0, 5.0).expect("up").light.x;
        let low = frame(0.0, 0.5).expect("just up").light.x;
        assert!(low > 0.0 && low < full, "{low} {full}");
        assert!(frame(0.0, 0.0).is_none());
        assert!(frame(0.0, -3.0).is_none());
        // A degree up, the whole light.
        assert!((frame(0.0, 1.01).expect("up").light.x - full).abs() < 1e-5);
    }

    #[test]
    fn a_key_asking_for_none_draws_none() {
        let none = [
            LightShafts::NONE,
            LightShafts { strength: 0.0, ..EVENING },
            LightShafts { length: 0.0, ..EVENING },
            LightShafts { tint: Vec3::ZERO, ..EVENING },
        ];
        for shafts in none {
            assert!(ShaftFrame::new(&shafts, &camera(), sun(0.0, 10.0), 1.0).is_none(), "{shafts:?}");
        }
        assert!(ShaftFrame::new(&EVENING, &camera(), Vec3::ZERO, 1.0).is_none());
        assert!(ShaftFrame::new(&EVENING, &camera(), Vec3::NAN, 1.0).is_none());
    }

    #[test]
    fn taps_follow_the_length_and_the_mask() {
        // 1 080 rows: a mask of 135; shafts 0.6 tall cross 81 of its texels.
        assert_eq!(taps(0.6, mask_size((1920, 1080)).1), 81);
        assert_eq!(taps(1.0, 135), 135);
        assert_eq!(taps(0.01, 135), MIN_TAPS);
        assert_eq!(taps(1.0, 400), MAX_TAPS);
        assert_eq!(taps(2.0, 135), 135, "held to the longest length");
        assert_eq!(mask_size((1921, 1081)), (241, 136));
        assert_eq!(march_size((1920, 1080)), (480, 270));
    }

    #[test]
    fn the_shader_holds_the_same_tap_range_and_constants_size() {
        let source = include_str!("../../shaders/post_shafts.wgsl");
        assert!(source.contains(&format!("const SHAFT_MIN_TAPS: u32 = {MIN_TAPS}u;")));
        assert!(source.contains(&format!("const SHAFT_MAX_TAPS: u32 = {MAX_TAPS}u;")));
        assert_eq!(size_of::<ShaftUniforms>(), 48);
    }

    #[test]
    fn the_light_over_bloom_is_finite_and_gives_the_light_back() {
        let ahead = frame(0.0, 10.0).expect("ahead");
        for intensity in [0.05, 0.3, 1.0, 16.0] {
            let uniforms = ahead.uniforms((1920, 1080), intensity);
            let back = Vec3::from_slice(&uniforms.light[..3]) * intensity;
            assert!((back - ahead.light).abs().max_element() < 1e-5, "{intensity}");
        }
        // Held within a bloom level's range at an intensity that rounds to nothing.
        let held = ahead.uniforms((1920, 1080), 0.0);
        assert!(held.light[..3].iter().all(|&light| light.is_finite() && light <= BLOOM_MAX));
        let uniforms = ahead.uniforms((1920, 1080), 1.0);
        assert_eq!(uniforms.light[3], 81.0);
        assert_eq!(uniforms.mask[..2], [1.0 / 240.0, 1.0 / 135.0]);
        assert_eq!(uniforms.sun[2..], [0.6, 16.0 / 9.0]);
    }
}
