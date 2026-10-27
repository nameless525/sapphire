//! The sky: its gradient and the layers drawn over it, and the CPU models of its light that the
//! probe bake shares with the renderer.

use glam::Vec3;
use sapphire_math::sh;

use crate::AmbientLight;

/// What the sky draws over its gradient (W2; look pillars 1 and 3), resolved by time of day like
/// the rest of the frame's look: the sun's and the moon's discs and glows, how bright the stars
/// are, the nebula's colors and the clouds' light and cover. Where the discs and stars are comes
/// from [`Celestial`](crate::Celestial); their sizes, the star, nebula and cloud maps and the
/// clouds' motion from the sky material (the frame packet's `sky_material`).
///
/// The default draws nothing over the gradient (no discs, glows, stars, nebula or clouds): the
/// sky as it was drawn before them (M1 W1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyLayers {
    pub sun: SkyDisc,
    pub moon: SkyDisc,
    /// Star brightness: 0 by day, 1 for a full night sky. It also lights the Milky Way.
    pub stars: f32,
    /// The nebula's colors (M1 X1 part 2e): black by day.
    pub nebula: Nebula,
    pub clouds: CloudLight,
}

/// The night sky's nebula (M1 X1 part 2e): the light of each of the sky material's nebula map's
/// three channels, linear RGB (HDR), added under the stars. A channel's light is its color
/// times its density; all black draws nothing, as by day.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Nebula {
    /// The map's R: its broad clouds.
    pub clouds: Vec3,
    /// G: the bright wisps along them.
    pub wisps: Vec3,
    /// B: sparse accents.
    pub accents: Vec3,
}

/// How the sky draws the sun or the moon: a disc and the glow around it, both in HDR so they
/// bloom. The disc's angular size comes from the sky material.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyDisc {
    /// The disc's color, linear RGB; its radiance is `color × intensity`.
    pub color: Vec3,
    /// 0 hides the disc. The sun's is far above 1, so it blooms into a white-hot core.
    pub intensity: f32,
    /// The glow around the disc: linear RGB added at its center, fading smoothly to nothing at
    /// `glow_radius`. Black for none.
    pub glow: Vec3,
    /// The glow's angular radius, in radians.
    pub glow_radius: f32,
}

/// The clouds' light and cover (W2), for both cloud layers: the card clouds around the horizon
/// and the high cirrus. Clouds are painted in the anime way: a lit and a shaded tone split along
/// the painted terminator, with a rim along their edges toward the key light, never a darkened
/// albedo.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CloudLight {
    /// Linear RGB of the clouds' lit sides, facing the key light (the sun by day, the moon at
    /// night).
    pub lit: Vec3,
    /// Linear RGB of their shaded sides: an authored color, usually a sky-ward hue.
    pub shade: Vec3,
    /// Linear RGB added along cloud edges, strongest toward the key light: the silver lining.
    pub rim: Vec3,
    /// How much of the card clouds shows, 0 (a clear sky) to 1 (the clouds as painted): their
    /// SDF dissolve threshold, so clouds shrink to their cores and vanish rather than fade.
    pub coverage: f32,
    /// The same for the cirrus layer.
    pub cirrus: f32,
}

/// A three-stop sky gradient in linear RGB: the sky pass's colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyGradient {
    pub zenith: Vec3,
    pub horizon: Vec3,
    pub ground: Vec3,
}

impl Default for SkyGradient {
    /// A clear midday sky.
    fn default() -> Self {
        Self {
            zenith: Vec3::new(0.10, 0.30, 0.80),
            horizon: Vec3::new(0.62, 0.78, 0.95),
            ground: Vec3::new(0.36, 0.34, 0.42),
        }
    }
}

/// The sky gradient's radiance in direction `d` (unit): `sky_color()` in the renderer's
/// `frame.wgsl`.
pub fn sky_gradient(sky: &SkyGradient, d: Vec3) -> Vec3 {
    let y = d.y;
    if y >= 0.0 {
        sky.horizon.lerp(sky.zenith, y.clamp(0.0, 1.0).sqrt())
    } else {
        sky.horizon.lerp(sky.ground, (-y * 5.0).clamp(0.0, 1.0))
    }
}

/// The sky light the ambient probe grid relights with, as L2 SH per color channel: the sky
/// gradient's shape, scaled per channel so a surface facing straight up under the open sky
/// receives exactly time of day's authored ambient sky (`ambient.sky × ambient.intensity`), as
/// it does from the ambient hemisphere, in the L1 irradiance the grid holds and the bake lights
/// with (`sh::SkyTransfer`, which leaves out the sky's L2 band). The grid then changes where
/// ambient light comes from and what blocks it, never how strong time of day authored it.
///
/// The renderer projects the sky cube the same way each frame on the GPU (its clouds and glows
/// included); the cooker's bake, which has no sky cube, lights surfaces with this, which is why it
/// lives here rather than beside the renderer's shaders.
pub fn sky_radiance(sky: &SkyGradient, ambient: &AmbientLight) -> [[f32; sh::L2]; 3] {
    // The gradient depends on height alone, so its projection reduces to an integral over
    // y = cos θ (the azimuth integrates out): only Y₀₀, Y₁₋₁ (∝ y), Y₂₀ and Y₂₂ are nonzero, and
    // over a full turn of azimuth both of the latter integrate to π(1 − 3y²) times their constant.
    const STEPS: u32 = 512;
    // The constants of Y₀₀, Y₁₋₁ = c₁y, Y₂₀ = c₂₀(3z² − 1) and Y₂₂ = c₂₂(x² − y²), read off the
    // basis straight up.
    let up = sh::basis(Vec3::Y);
    let (c0, c1, c20, c22) = (up[0], up[1], -up[6], -up[8]);
    let mut channels = [[0.0f32; sh::L2]; 3];
    let dy = 2.0 / STEPS as f32;
    for step in 0..STEPS {
        let y = -1.0 + (step as f32 + 0.5) * dy;
        let radiance = sky_gradient(sky, Vec3::new((1.0 - y * y).sqrt(), y, 0.0));
        let around = std::f32::consts::TAU * dy;
        let zonal2 = std::f32::consts::PI * (1.0 - 3.0 * y * y) * dy;
        for (channel, value) in channels.iter_mut().zip(radiance.to_array()) {
            channel[0] += value * around * c0;
            channel[1] += value * around * c1 * y;
            channel[6] += value * zonal2 * c20;
            channel[8] += value * zonal2 * c22;
        }
    }
    let target = ambient.sky * ambient.intensity;
    for (channel, target) in channels.iter_mut().zip(target.to_array()) {
        let open = sh::evaluate(&sh::irradiance(*channel)[..sh::L1], Vec3::Y);
        let scale = if open > 1e-6 { target.max(0.0) / open } else { 0.0 };
        for value in channel.iter_mut() {
            *value *= scale;
        }
    }
    channels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_radiance_gives_the_authored_ambient_facing_up() {
        let sky = SkyGradient {
            zenith: Vec3::new(0.2, 0.4, 0.9),
            horizon: Vec3::new(0.7, 0.8, 0.9),
            ground: Vec3::new(0.1, 0.08, 0.05),
        };
        let ambient =
            AmbientLight { sky: Vec3::new(0.5, 0.6, 0.8), ground: Vec3::splat(0.2), intensity: 1.5 };
        let channels = sky_radiance(&sky, &ambient);
        for (channel, target) in channels.iter().zip((ambient.sky * ambient.intensity).to_array()) {
            let up = sh::evaluate(&sh::irradiance(*channel)[..sh::L1], Vec3::Y);
            assert!((up - target).abs() < 1e-4, "{up} vs {target}");
            // As an open probe holds it: the transfer of the sky through visibility 1.
            let open = sh::SkyTransfer::new(channel).apply(&sh::project_constant(1.0));
            let held = sh::evaluate(&open, Vec3::Y);
            assert!((held - target).abs() < 1e-4, "{held} vs {target}");
            // Lit less from below, where the gradient's ground is darker, and alike around.
            let down = sh::evaluate(&sh::irradiance(*channel), Vec3::NEG_Y);
            assert!(down < up);
            let east = sh::evaluate(&sh::irradiance(*channel), Vec3::X);
            let north = sh::evaluate(&sh::irradiance(*channel), Vec3::NEG_Z);
            assert!((east - north).abs() < 1e-5);
        }
        // A black sky lends nothing, whatever the ambient.
        let black = SkyGradient { zenith: Vec3::ZERO, horizon: Vec3::ZERO, ground: Vec3::ZERO };
        assert_eq!(sky_radiance(&black, &ambient), [[0.0; sh::L2]; 3]);
    }

    #[test]
    fn the_cpu_gradient_matches_sky_color() {
        let sky = SkyGradient { zenith: Vec3::X, horizon: Vec3::Y, ground: Vec3::Z };
        assert_eq!(sky_gradient(&sky, Vec3::Y), Vec3::X, "the zenith");
        assert_eq!(sky_gradient(&sky, Vec3::X), Vec3::Y, "the horizon");
        assert_eq!(sky_gradient(&sky, Vec3::NEG_Y), Vec3::Z, "the ground");
        let low = sky_gradient(&sky, Vec3::new(0.99, -0.1, 0.0).normalize());
        assert!(low.y > 0.0 && low.z > 0.0, "a quick fade below the horizon");
    }
}
