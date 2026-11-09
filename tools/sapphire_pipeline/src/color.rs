//! sRGB transfer functions (IEC 61966-2-1), 8-bit quantization, and OkLab.
//!
//! glTF stores material factors and `COLOR_0` as linear values, while the cooked vertex
//! color is sRGB-encoded so 8 bits keep precision in the darks. Ramps interpolate their
//! color stops in OkLab (Ottosson 2020) by default, where equal steps look like equal
//! changes of lightness and color.
//!
//! The transfer functions and OkLab are `sapphire_math::color`'s, shared with the runtime
//! (time of day blends its colors in OkLab), so tools and the engine blend colors the same way;
//! 8-bit quantization is the tools' own.

pub(crate) use sapphire_math::color::{
    linear_srgb_to_oklab, linear_to_srgb, oklab_to_linear_srgb, srgb_to_linear,
};

/// The sRGB transfer functions with their powers through `sapphire_math::det`, for generators whose
/// committed bytes are pinned on every OS (figures, and the gunmetal matcap they copy). The shared
/// ones call `std`'s `powf`, which rounds by each platform's C library: a code value apart, now and
/// then, once quantized.
pub(crate) mod exact {
    use sapphire_math::det::pow;

    /// [`super::srgb_to_linear`], the same on every target and OS.
    pub(crate) fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 { c / 12.92 } else { pow((c + 0.055) / 1.055, 2.4) }
    }

    /// [`super::linear_to_srgb`], the same on every target and OS.
    pub(crate) fn linear_to_srgb(c: f32) -> f32 {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * pow(c, 1.0 / 2.4) - 0.055 }
    }
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
