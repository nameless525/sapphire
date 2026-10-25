//! sRGB transfer functions (IEC 61966-2-1), 8-bit quantization, and OkLab.
//!
//! glTF stores material factors and `COLOR_0` as linear values, while the cooked vertex
//! color is sRGB-encoded so 8 bits keep precision in the darks. Ramps interpolate their
//! color stops in OkLab (Ottosson 2020) by default, where equal steps look like equal
//! changes of lightness and color.

/// Decodes one sRGB-encoded channel in `[0, 1]` to linear.
pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Encodes one linear channel to sRGB, clamping to `[0, 1]` first.
pub(crate) fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Quantizes a `[0, 1]` value to an 8-bit unorm, rounding to nearest.
pub(crate) fn unorm8(c: f32) -> u8 {
    // The clamp keeps the cast in range; NaN saturates to 0.
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Linear RGBA (opaque) from 8-bit sRGB components, the way artists pick palette colors.
pub(crate) fn srgb8(r: u8, g: u8, b: u8) -> [f32; 4] {
    let channel = |v: u8| srgb_to_linear(f32::from(v) / 255.0);
    [channel(r), channel(g), channel(b), 1.0]
}

/// Linear sRGB to OkLab `[L, a, b]`.
pub(crate) fn linear_srgb_to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = 0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let [l, m, s] = [l, m, s].map(f32::cbrt);
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OkLab `[L, a, b]` to linear sRGB. Colors outside the sRGB gamut come back with channels
/// outside `[0, 1]`; encoding clamps them.
pub(crate) fn oklab_to_linear_srgb([lightness, a, b]: [f32; 3]) -> [f32; 3] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_8bit_srgb_value_round_trips_through_linear() {
        for v in 0..=255u8 {
            let linear = srgb_to_linear(f32::from(v) / 255.0);
            assert_eq!(unorm8(linear_to_srgb(linear)), v, "value {v}");
        }
    }

    #[test]
    fn oklab_matches_its_reference_values_and_round_trips() {
        let close = |a: [f32; 3], b: [f32; 3], tolerance: f32| {
            a.iter().zip(&b).all(|(a, b)| (a - b).abs() < tolerance)
        };
        // Ottosson's published values for white and the sRGB primaries.
        assert!(close(linear_srgb_to_oklab([1.0, 1.0, 1.0]), [1.0, 0.0, 0.0], 1e-4));
        assert!(close(linear_srgb_to_oklab([1.0, 0.0, 0.0]), [0.627_955, 0.224_863, 0.125_846], 1e-4));
        assert!(close(linear_srgb_to_oklab([0.0, 1.0, 0.0]), [0.866_440, -0.233_888, 0.179_498], 1e-4));
        assert!(close(linear_srgb_to_oklab([0.0, 0.0, 1.0]), [0.452_014, -0.032_457, -0.311_528], 1e-4));
        for color in [[0.0, 0.0, 0.0], [0.2, 0.5, 0.9], [0.94, 0.6, 0.41], [0.01, 0.02, 0.003]] {
            let back = oklab_to_linear_srgb(linear_srgb_to_oklab(color));
            assert!(close(back, color, 1e-5), "{color:?} -> {back:?}");
        }
    }

    #[test]
    fn known_points() {
        assert_eq!(unorm8(linear_to_srgb(0.0)), 0);
        assert_eq!(unorm8(linear_to_srgb(1.0)), 255);
        // sRGB mid-grey is linear 0.214.
        assert!((linear_to_srgb(0.214_041_14) - 0.5).abs() < 1e-6);
        assert!((srgb_to_linear(0.5) - 0.214_041_14).abs() < 1e-6);
        assert_eq!(unorm8(linear_to_srgb(2.0)), 255);
        assert_eq!(unorm8(f32::NAN), 0);
    }
}
