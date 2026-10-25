//! Color math shared by the runtime and the tools: the sRGB transfer functions, luminance, and
//! OkLab (Björn Ottosson, 2020).
//!
//! OkLab is the space colors are interpolated in, wherever a blend should look even: the
//! pipeline's ramps blend their color stops in it, and time of day blends its keys' sky, light
//! and fog colors in it (`sapphire_world::tod`). Equal steps in OkLab look like equal changes of
//! lightness and color, where blending linear light washes a warm-to-cool gradient out and
//! blending sRGB-encoded values darkens its middle.
//!
//! The cube roots go through `libm`, as [`crate::det`]'s functions do, so a blend computes the
//! same bits on every target and OS.

use glam::Vec3;

/// Rec. 709 / sRGB luminance weights for linear RGB.
pub const LUMINANCE: Vec3 = Vec3::new(0.2126, 0.7152, 0.0722);

/// Decodes one sRGB-encoded channel in `[0, 1]` to linear (IEC 61966-2-1).
#[must_use]
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Encodes one linear channel to sRGB, clamping to `[0, 1]` first.
#[must_use]
pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// The luminance of a linear RGB color.
#[must_use]
pub fn luminance(rgb: Vec3) -> f32 {
    rgb.dot(LUMINANCE)
}

/// Linear sRGB to OkLab `[L, a, b]`. Defined for any finite input; HDR values above 1 have a
/// lightness above 1.
#[must_use]
pub fn linear_srgb_to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let [l, m, s] = [l, m, s].map(libm::cbrtf);
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OkLab `[L, a, b]` to linear sRGB. Colors outside the sRGB gamut come back with channels
/// outside `[0, 1]`; encoding clamps them.
#[must_use]
pub fn oklab_to_linear_srgb([lightness, a, b]: [f32; 3]) -> [f32; 3] {
    let l = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s = lightness - 0.089_484_18 * a - 1.291_485_5 * b;
    let [l, m, s] = [l, m, s].map(|v| v * v * v);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// Blends two linear RGB colors `t` of the way from `a` to `b` in OkLab.
///
/// Exact at the ends: `t ≤ 0` returns `a` and `t ≥ 1` returns `b`, bit for bit, so a blend that
/// lands on a key reproduces the key. A blend can step slightly outside the gamut of its ends
/// (OkLab's straight lines curve in RGB); channels below zero come back as zero, so the result
/// is always a valid light color.
#[must_use]
pub fn mix_oklab(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let [a, b] = [a, b].map(|color| Vec3::from(linear_srgb_to_oklab(color.to_array())));
    Vec3::from(oklab_to_linear_srgb(a.lerp(b, t).to_array())).max(Vec3::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < tolerance)
    }

    #[test]
    fn oklab_matches_its_reference_values_and_round_trips() {
        // Ottosson's published values for white and the sRGB primaries.
        assert!(close(linear_srgb_to_oklab([1.0, 1.0, 1.0]), [1.0, 0.0, 0.0], 1e-4));
        assert!(close(linear_srgb_to_oklab([1.0, 0.0, 0.0]), [0.627_955, 0.224_863, 0.125_846], 1e-4));
        assert!(close(linear_srgb_to_oklab([0.0, 1.0, 0.0]), [0.866_440, -0.233_888, 0.179_498], 1e-4));
        assert!(close(linear_srgb_to_oklab([0.0, 0.0, 1.0]), [0.452_014, -0.032_457, -0.311_528], 1e-4));
        for color in
            [[0.0, 0.0, 0.0], [0.2, 0.5, 0.9], [0.94, 0.6, 0.41], [0.01, 0.02, 0.003], [3.0, 2.0, 0.5]]
        {
            let back = oklab_to_linear_srgb(linear_srgb_to_oklab(color));
            assert!(close(back, color, 1e-5 * color[0].max(1.0)), "{color:?} -> {back:?}");
        }
    }

    #[test]
    fn srgb_transfer_known_points() {
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
        // sRGB mid-grey is linear 0.214.
        assert!((linear_to_srgb(0.214_041_14) - 0.5).abs() < 1e-6);
        assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
        assert_eq!(linear_to_srgb(2.0), linear_to_srgb(1.0), "clamped");
        assert!((luminance(Vec3::ONE) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn oklab_blends_are_exact_at_their_ends_and_even_between() {
        let (warm, cool) = (Vec3::new(1.0, 0.45, 0.22), Vec3::new(0.16, 0.16, 0.45));
        assert_eq!(mix_oklab(warm, cool, 0.0), warm);
        assert_eq!(mix_oklab(warm, cool, -1.0), warm);
        assert_eq!(mix_oklab(warm, cool, 1.0), cool);
        // Lightness moves linearly in OkLab, so the midpoint's lightness is the mean.
        let lightness = |color: Vec3| linear_srgb_to_oklab(color.to_array())[0];
        let middle = mix_oklab(warm, cool, 0.5);
        assert!((lightness(middle) - 0.5 * (lightness(warm) + lightness(cool))).abs() < 1e-5);
        // Blending linear light instead would be brighter in the middle: OkLab keeps the
        // midpoint from washing out.
        assert!(luminance(middle) < luminance(warm.lerp(cool, 0.5)));
        // Same hue, different brightness: the blend stays on the hue.
        let grey = mix_oklab(Vec3::splat(0.1), Vec3::splat(0.9), 0.3);
        assert!((grey.x - grey.y).abs() < 1e-5 && (grey.y - grey.z).abs() < 1e-5, "{grey}");
    }

    /// Golden bit patterns. `libm`'s cube root is pure software, so these hold on every target;
    /// a failure means a `libm` upgrade changed results.
    #[test]
    fn oklab_blends_are_deterministic() {
        let mixed = mix_oklab(Vec3::new(0.1, 0.3, 0.8), Vec3::new(1.0, 0.5, 0.2), 0.37);
        assert_eq!(mixed.to_array().map(f32::to_bits), [0x3ea8_32d7, 0x3ec7_8e50, 0x3f11_ed96]);
    }
}
