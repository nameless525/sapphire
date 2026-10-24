//! sRGB transfer functions (IEC 61966-2-1) and 8-bit quantization.
//!
//! glTF stores material factors and `COLOR_0` as linear values, while the cooked vertex
//! color is sRGB-encoded so 8 bits keep precision in the darks.

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
