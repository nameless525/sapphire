//! Colour vision as a dichromat sees it (M4 A5): the three dichromacies by Machado, Oliveira and
//! Fernandes's model, and a perceptual distance to say whether two colours stay apart under them.
//!
//! The renderer's colour-vision debug view (`r.color_vision`) applies [`Dichromacy::matrix`] to the
//! finished frame, and tests apply [`Dichromacy::simulate`] to a cue's colours, so the two can't drift.
//!
//! # The model
//!
//! G. M. Machado, M. M. Oliveira and L. A. F. Fernandes, "A Physiologically-based Model for Simulation
//! of Color Vision Deficiency", IEEE Transactions on Visualization and Computer Graphics 15(6),
//! pp. 1291–1298, 2009. Its matrices for protanomaly, deuteranomaly and tritanomaly at severity 1.0,
//! which the authors give as dichromacy, are copied from Table 1 of the paper's page
//! (`inf.ufrgs.br/~oliveira/pubs_files/CVD_Simulation/CVD_Simulation.html`, read 2026-11-10) and
//! checked against it digit for digit. The page doesn't name the RGB space they take. They come from
//! linear sums of cone responses to the primaries' spectra, so they're linear in light, and they're
//! applied here to linear RGB, after decoding sRGB and before encoding it again.
//!
//! Two checks hold the copy (the tests below): each row sums to 1, so white and every grey stay what
//! they were, and the protan and deutan matrices are singular, since their model's L or M cone takes
//! the other's sensitivity and two cone types are left. Tritanomaly's model shifts the S cone rather
//! than merging it, so its matrix isn't.
//!
//! # Apart
//!
//! [`oklab`] is Björn Ottosson's Oklab (2020), whose Euclidean distance is close to how different two
//! colours look. Tests call two colours apart under a dichromacy when their simulations are at least
//! [`APART`] from each other, and where they aren't, the cue must differ by shape.

use glam::Vec3;

/// A dichromacy: one of the three cone types missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Hash)]
pub enum Dichromacy {
    /// No L cones: reds dark and confused with greens.
    Protanopia,
    /// No M cones: reds and greens confused.
    Deuteranopia,
    /// No S cones: blues confused with greens, yellows with pinks.
    Tritanopia,
}

/// The Oklab distance at which two colours are apart: a judgement for a cue read at a glance in a
/// fight, well past the smallest difference anyone sees, rather than a measurement.
pub const APART: f32 = 0.1;

impl Dichromacy {
    pub const ALL: [Self; 3] = [Self::Protanopia, Self::Deuteranopia, Self::Tritanopia];

    /// How the debug view's cvar and the sandbox spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Protanopia => "protanopia",
            Self::Deuteranopia => "deuteranopia",
            Self::Tritanopia => "tritanopia",
        }
    }

    /// Machado et al.'s matrix at severity 1.0, by rows, on linear RGB (see the module docs).
    #[must_use]
    pub const fn matrix(self) -> [[f32; 3]; 3] {
        match self {
            Self::Protanopia => [
                [0.152_286, 1.052_583, -0.204_868],
                [0.114_503, 0.786_281, 0.099_216],
                [-0.003_882, -0.048_116, 1.051_998],
            ],
            Self::Deuteranopia => [
                [0.367_322, 0.860_646, -0.227_968],
                [0.280_085, 0.672_501, 0.047_413],
                [-0.011_820, 0.042_940, 0.968_881],
            ],
            Self::Tritanopia => [
                [1.255_528, -0.076_749, -0.178_779],
                [-0.078_411, 0.930_809, 0.147_602],
                [0.004_733, 0.691_367, 0.303_900],
            ],
        }
    }

    /// `linear`, a linear RGB colour from 0 to 1, as this dichromacy sees it, clamped to the display's
    /// range as the debug view's target clamps it.
    #[must_use]
    pub fn simulate(self, linear: Vec3) -> Vec3 {
        let [r, g, b] = self.matrix().map(Vec3::from);
        Vec3::new(r.dot(linear), g.dot(linear), b.dot(linear)).clamp(Vec3::ZERO, Vec3::ONE)
    }
}

/// Oklab's lightness and its two opponent axes, of a linear RGB colour (Ottosson 2020).
#[must_use]
pub fn oklab(linear: Vec3) -> Vec3 {
    let l = 0.412_221_47 * linear.x + 0.536_332_55 * linear.y + 0.051_445_995 * linear.z;
    let m = 0.211_903_5 * linear.x + 0.680_699_5 * linear.y + 0.107_396_96 * linear.z;
    let s = 0.088_302_46 * linear.x + 0.281_718_85 * linear.y + 0.629_978_7 * linear.z;
    let (l, m, s) = (l.max(0.0).cbrt(), m.max(0.0).cbrt(), s.max(0.0).cbrt());
    Vec3::new(
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    )
}

/// How far apart two linear RGB colours look: the distance between them in Oklab.
#[must_use]
pub fn distance(a: Vec3, b: Vec3) -> f32 {
    oklab(a).distance(oklab(b))
}

/// A display-encoded (sRGB) code from 0 to 255 in linear light.
#[must_use]
pub fn srgb_to_linear(code: u8) -> f32 {
    let encoded = f32::from(code) / 255.0;
    if encoded <= 0.040_45 { encoded / 12.92 } else { ((encoded + 0.055) / 1.055).powf(2.4) }
}

/// A display-encoded colour's red, green and blue in linear light.
#[must_use]
pub fn srgb_bytes_to_linear([r, g, b, _]: [u8; 4]) -> Vec3 {
    Vec3::new(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn determinant([a, b, c]: [[f32; 3]; 3]) -> f32 {
        Vec3::from(a).dot(Vec3::from(b).cross(Vec3::from(c)))
    }

    #[test]
    fn every_dichromacy_keeps_white_and_the_greys() {
        for dichromacy in Dichromacy::ALL {
            for row in dichromacy.matrix() {
                assert!((row.iter().sum::<f32>() - 1.0).abs() < 2e-6, "{dichromacy:?}: {row:?}");
            }
            for grey in [0.0, 0.18, 0.5, 1.0] {
                let seen = dichromacy.simulate(Vec3::splat(grey));
                assert!(seen.abs_diff_eq(Vec3::splat(grey), 1e-5), "{dichromacy:?}: {seen}");
            }
        }
    }

    #[test]
    fn a_red_green_dichromat_has_two_cone_types_left() {
        // Singular to the copy's six digits: the model's L and M rows are one.
        assert!(determinant(Dichromacy::Protanopia.matrix()).abs() < 1e-5);
        assert!(determinant(Dichromacy::Deuteranopia.matrix()).abs() < 1e-5);
        // Tritanomaly shifts the S cone instead, which leaves three.
        assert!(determinant(Dichromacy::Tritanopia.matrix()) > 0.2);
    }

    #[test]
    fn the_simulation_confuses_what_each_dichromacy_is_known_to() {
        let red = Vec3::new(0.8, 0.05, 0.05);
        let green = Vec3::new(0.1, 0.3, 0.05);
        assert!(distance(red, green) > 2.0 * APART);
        // Protans and deutans lose red against green far more than tritans do.
        let lost = |dichromacy: Dichromacy| distance(dichromacy.simulate(red), dichromacy.simulate(green));
        assert!(lost(Dichromacy::Protanopia) < lost(Dichromacy::Tritanopia));
        assert!(lost(Dichromacy::Deuteranopia) < lost(Dichromacy::Tritanopia));
        // And tritans lose blue against green.
        let (blue, teal) = (Vec3::new(0.05, 0.1, 0.8), Vec3::new(0.02, 0.3, 0.3));
        let tritan = Dichromacy::Tritanopia;
        assert!(distance(tritan.simulate(blue), tritan.simulate(teal)) < distance(blue, teal) * 0.5);
    }

    #[test]
    fn oklab_is_ottossons() {
        // White is L 1 with no hue; his page's reference values for the primaries.
        assert!(oklab(Vec3::ONE).abs_diff_eq(Vec3::new(1.0, 0.0, 0.0), 1e-4));
        assert!(oklab(Vec3::X).abs_diff_eq(Vec3::new(0.627_955, 0.224_863, 0.125_846), 1e-4));
        assert!(oklab(Vec3::Y).abs_diff_eq(Vec3::new(0.866_440, -0.233_888, 0.179_498), 1e-4));
        assert!(oklab(Vec3::Z).abs_diff_eq(Vec3::new(0.452_014, -0.032_457, -0.311_528), 1e-4));
    }

    #[test]
    fn srgb_codes_decode_to_linear_light() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert_eq!(srgb_to_linear(255), 1.0);
        assert!((srgb_to_linear(188) - 0.5).abs() < 0.003);
    }
}
