//! Fog (M1 W3): the frame's distance and height fog, and the CPU reference of how the renderer's
//! shaders apply it to a surface's final color over a view ray ([`Fog::ray`]: `fog_ray` in the
//! renderer's `frame.wgsl`), for tests and tools. How the renderer draws it, and why the layers
//! reach a ceiling and mix over the stretch of a ray the camera sees, is `sapphire_render::fog`'s
//! to say. The constants here are the shaders' too: the renderer's tests hold its WGSL to them,
//! and its fog to [`Fog::ray`].

use glam::Vec3;

/// Meters above the camera the distance layers reach: a ray's path through them ends where it
/// has risen that far, so the sky takes them as a band along the horizon and tall peaks rise out
/// of the haze.
pub const CEILING: f32 = 30.0;

/// The sky's distance as the fog sees it: far enough that every layer looking at the horizon has
/// saturated.
pub const SKY_DISTANCE: f32 = 1.0e6;

/// The largest exponent the height layer's density and path take, so its optical depth stays
/// finite (under about 10³² for any density under 10) and the layers' mix with it.
pub const MAX_EXPONENT: f32 = 30.0;

/// The sky's horizon in the layers' mix, as an optical depth: what the light leans toward where
/// the fog is thin, so fog cards in a frame without fog take the horizon's color.
pub const HORIZON_WEIGHT: f32 = 1.0e-3;

/// Below this `|x|`, the height layer's `(1 − e^(−x)) / x` is its series.
pub const SERIES_BELOW: f32 = 1.0e-3;

/// The optical depth, at the densities around the camera, of the stretch of a ray over which the
/// layers' colors mix: past it, under 2% of the light reaches the camera.
pub const MIX_DEPTH: f32 = 4.0;

/// The least density the mixing stretch is measured by, so a frame without fog has one.
pub const MIN_DENSITY: f32 = 1.0e-12;

/// Distance and height fog (look pillar 3: layered atmosphere; M1 W3; the renderer draws it,
/// `sapphire_render::fog`): two distance layers and a height layer, each fading what lies behind it toward its
/// color, plus light scattered toward the camera around the key light. Every world pass applies
/// it at its final color, the sky included, so distant geometry melts into the sky without a
/// seam; fog cards take its light.
///
/// Densities are extinction per meter: a layer's optical depth is its density integrated over
/// the meters of view ray inside it, and what lies behind keeps `exp(−depth)` of its light. The
/// layers mix in proportion to their depths along the ray. The default has none, and a frame
/// without any density draws exactly as it would without fog.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fog {
    /// Haze near the camera.
    pub near: FogLayer,
    /// Aerial perspective: what melts distant geometry into the sky. Its color is a tint on the
    /// sky's horizon ([`SkyGradient::horizon`](crate::SkyGradient::horizon)): white is the
    /// horizon itself, so geometry far enough away takes the sky's own color.
    pub far: FogLayer,
    pub height: HeightFog,
    /// Linear RGB light the fog scatters toward the camera around the key light's direction
    /// (the sun by day, the moon at night), color × strength, in a forward lobe; black for none.
    /// It adds where the fog is, so the sun side of a hazy view glows.
    pub sun_inscatter: Vec3,
    /// How much of the fog characters take (the instances the frame packet lists in its
    /// `characters`, and their outlines): their optical depth is scaled by it,
    /// from 0 (characters keep their colors at any distance) to 1 (as the world). Anime games
    /// keep characters readable in fog, so time of day sets it below 1.
    pub characters: f32,
}

/// One distance layer of [`Fog`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FogLayer {
    /// Linear RGB (the far layer's: a tint on the sky's horizon).
    pub color: Vec3,
    /// Extinction per meter: 0 is clear.
    pub density: f32,
    /// Meters from the camera where the layer begins.
    pub start: f32,
}

/// Fog that thickens toward the ground: its density is `density × exp(−falloff × (y − base))`
/// at height `y` (meters, world space), so it lies in valleys and thins over hilltops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeightFog {
    /// Linear RGB.
    pub color: Vec3,
    /// Extinction per meter at `base`.
    pub density: f32,
    /// How fast it thins with height, per meter: 0 keeps `density` at every height.
    pub falloff: f32,
    /// The height, in meters, where `density` holds.
    pub base: f32,
}

impl Fog {
    /// Whether any layer has a density: a frame whose fog doesn't draws exactly as without it.
    pub fn is_visible(&self) -> bool {
        [self.near.density, self.far.density, self.height.density].iter().any(|density| *density > 0.0)
    }

    /// The fog as the renderer draws it: densities, colors, starts and the falloff that are
    /// negative or not finite are 0, a base that isn't finite is 0, and the characters' share is
    /// held to 0..=1 (0 where it isn't finite). The renderer uploads this, and [`Self::ray`]
    /// evaluates it.
    pub fn sanitized(&self) -> Self {
        let amount = |value: f32| if value.is_finite() { value.max(0.0) } else { 0.0 };
        let color = |color: Vec3| Vec3::from_array(color.to_array().map(amount));
        let layer = |layer: &FogLayer| FogLayer {
            color: color(layer.color),
            density: amount(layer.density),
            start: amount(layer.start),
        };
        let height = &self.height;
        Self {
            near: layer(&self.near),
            far: layer(&self.far),
            height: HeightFog {
                color: color(height.color),
                density: amount(height.density),
                falloff: amount(height.falloff),
                base: if height.base.is_finite() { height.base } else { 0.0 },
            },
            sun_inscatter: color(self.sun_inscatter),
            characters: if self.characters.is_finite() { self.characters.clamp(0.0, 1.0) } else { 0.0 },
        }
    }

    /// The fog over the ray from `eye` along the unit vector `direction` to what lies `distance`
    /// meters along it ([`SKY_DISTANCE`] for the sky), as the shaders apply it to a surface's
    /// final color: `share` is the share of it the surface takes (1 for the world,
    /// [`Self::characters`] for characters), `horizon` the sky's horizon color, and `toward_key`
    /// the unit vector toward the key light. [`FogRay::CLEAR`] where the fog has no density.
    ///
    /// The values are made safe as the renderer makes them for the GPU ([`Self::sanitized`]), so
    /// this is exactly what the shaders compute, to `f32` rounding.
    pub fn ray(
        &self,
        eye: Vec3,
        direction: Vec3,
        distance: f32,
        share: f32,
        horizon: Vec3,
        toward_key: Vec3,
    ) -> FogRay {
        let fog = self.sanitized();
        if !fog.is_visible() {
            return FogRay::CLEAR;
        }
        let (near, far, height) = (fog.near, fog.far, fog.height);
        let rise = direction.y;
        // The height layer's density at the eye, over its density at its base.
        let start = (-height.falloff * (eye.y - height.base)).min(MAX_EXPONENT).exp();
        // The layers' optical depths over the ray's first `length` meters.
        let depths = |length: f32| {
            // The distance layers end where the ray has risen CEILING above the eye.
            let reach = if rise > 0.0 { length.min(CEILING / rise) } else { length };
            let near_depth = near.density * (reach - near.start).max(0.0);
            let far_depth = far.density * (reach - far.start).max(0.0);
            // density × ∫₀ˡ exp(−k (h₀ + rise·t)) dt.
            let x = height.falloff * rise * length;
            let ratio =
                if x.abs() < SERIES_BELOW { 1.0 - 0.5 * x } else { (1.0 - (-x).min(MAX_EXPONENT).exp()) / x };
            Vec3::new(near_depth, far_depth, height.density * start * length * ratio)
        };
        let total = depths(distance).element_sum();
        // The colors mix over the stretch the camera sees.
        let around = near.density + far.density + height.density * start;
        let mixed = depths(distance.min(MIX_DEPTH / around.max(MIN_DENSITY)));
        let light = (near.color * mixed.x
            + far.color * horizon * mixed.y
            + height.color * mixed.z
            + horizon * HORIZON_WEIGHT)
            / (mixed.element_sum() + HORIZON_WEIGHT);
        let c = direction.dot(toward_key).clamp(0.0, 1.0);
        let lobe = (c * c) * (c * c);
        FogRay { transmittance: (-share * total).exp(), light, sun: fog.sun_inscatter * (lobe * lobe) }
    }
}

/// The fog over one view ray ([`Fog::ray`]), as the shaders evaluate it (`fog_ray` in the
/// renderer's `frame.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogRay {
    /// The share of the light behind that reaches the camera: 1 through clear air, 0 through
    /// opaque fog.
    pub transmittance: f32,
    /// The fog's light: its layers' colors mixed by their optical depths along the ray (the far
    /// layer's tint × the sky's horizon), leaning to the horizon where they are thin.
    pub light: Vec3,
    /// The key light scattered toward the camera along the ray: the inscatter color × its lobe.
    pub sun: Vec3,
}

impl FogRay {
    /// No fog: everything behind it as it is.
    pub const CLEAR: Self = Self { transmittance: 1.0, light: Vec3::ZERO, sun: Vec3::ZERO };

    /// `color` seen through the fog: what reaches the camera of it, plus the fog's own light.
    pub fn apply(&self, color: Vec3) -> Vec3 {
        color * self.transmittance + (self.light + self.sun) * (1.0 - self.transmittance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HORIZON: Vec3 = Vec3::new(0.6, 0.75, 0.95);

    fn height_fog(density: f32, falloff: f32, base: f32) -> Fog {
        let height = HeightFog { color: Vec3::ONE, density, falloff, base };
        Fog { height, ..Fog::default() }
    }

    /// The height layer's optical depth along a ray, by the midpoint rule over many steps.
    fn numeric_height_depth(fog: &Fog, eye: Vec3, direction: Vec3, distance: f32) -> f64 {
        const STEPS: u32 = 200_000;
        let h = &fog.height;
        let dt = f64::from(distance) / f64::from(STEPS);
        (0..STEPS)
            .map(|step| {
                let y = f64::from(eye.y) + f64::from(direction.y) * (f64::from(step) + 0.5) * dt;
                f64::from(h.density) * (-f64::from(h.falloff) * (y - f64::from(h.base))).exp() * dt
            })
            .sum()
    }

    fn depth(ray: FogRay) -> f64 {
        -f64::from(ray.transmittance).ln()
    }

    #[test]
    fn the_height_layer_is_its_exact_integral_from_inside_and_outside_it() {
        let fog = height_fog(0.05, 0.3, 2.0);
        let cases = [
            // Inside the layer: up, down, across, and gently climbing out of it.
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.6, 0.8), 40.0),
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -0.05, 1.0).normalize(), 18.0),
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 120.0),
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.02, 1.0).normalize(), 300.0),
            // Above it: looking down into it, and across.
            (Vec3::new(0.0, 30.0, 0.0), Vec3::new(0.0, -0.5, 0.866), 60.0),
            (Vec3::new(0.0, 30.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 500.0),
            // Below its base, looking up through the densest part.
            (Vec3::new(0.0, -5.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 25.0),
        ];
        for (eye, direction, distance) in cases {
            let ray = fog.ray(eye, direction, distance, 1.0, HORIZON, Vec3::Y);
            let exact = numeric_height_depth(&fog, eye, direction, distance);
            let found = depth(ray);
            assert!(
                (found - exact).abs() <= 1e-4 * exact.max(1.0),
                "eye {eye}, {direction} for {distance} m: {found} against {exact}"
            );
        }
    }

    #[test]
    fn horizontal_rays_and_no_falloff_are_homogeneous() {
        let fog = height_fog(0.02, 0.25, 0.0);
        let eye = Vec3::new(0.0, 3.0, 0.0);
        let across = fog.ray(eye, Vec3::X, 50.0, 1.0, HORIZON, Vec3::Y);
        let density = 0.02 * (-0.25f32 * 3.0).exp();
        assert!((depth(across) - f64::from(density * 50.0)).abs() < 1e-5);
        // A ray a hair off horizontal agrees with the horizontal one.
        let tilted = fog.ray(eye, Vec3::new(1.0, 1e-7, 0.0).normalize(), 50.0, 1.0, HORIZON, Vec3::Y);
        assert!((depth(tilted) - depth(across)).abs() < 1e-6);
        let flat = height_fog(0.02, 0.0, 0.0).ray(eye, Vec3::new(0.0, 0.8, 0.6), 50.0, 1.0, HORIZON, Vec3::Y);
        assert!((depth(flat) - 1.0).abs() < 1e-5, "{}", depth(flat));
    }

    #[test]
    fn extreme_rays_stay_finite() {
        // Densities past any authored one, up and down every way.

        let fog = Fog {
            near: FogLayer { color: Vec3::ONE, density: 0.01, start: 0.0 },
            far: FogLayer { color: Vec3::ONE, density: 0.004, start: 30.0 },
            ..height_fog(0.5, 2.0, 0.0)
        };
        for eye in [Vec3::new(0.0, -400.0, 0.0), Vec3::new(0.0, 900.0, 0.0), Vec3::ZERO] {
            for direction in [Vec3::NEG_Y, Vec3::Y, Vec3::X, Vec3::new(0.0, -0.99, 0.14).normalize()] {
                for distance in [0.0, 1.0, 1e3, SKY_DISTANCE] {
                    let ray = fog.ray(eye, direction, distance, 1.0, HORIZON, Vec3::Y);
                    assert!(
                        ray.transmittance.is_finite() && ray.light.is_finite() && ray.sun.is_finite(),
                        "{eye} {direction} {distance}: {ray:?}"
                    );
                    assert!((0.0..=1.0).contains(&ray.transmittance));
                }
            }
        }
    }

    #[test]
    fn distance_layers_begin_at_their_starts_and_end_at_the_ceiling() {
        let fog = Fog {
            near: FogLayer { color: Vec3::X, density: 0.01, start: 5.0 },
            far: FogLayer { color: Vec3::ONE, density: 0.002, start: 20.0 },
            ..Fog::default()
        };
        let ray = |direction: Vec3, distance| fog.ray(Vec3::ZERO, direction, distance, 1.0, HORIZON, Vec3::Y);
        assert_eq!(ray(Vec3::Z, 4.0).transmittance, 1.0, "before both starts");
        assert!((depth(ray(Vec3::Z, 15.0)) - 0.1).abs() < 1e-6, "the near layer alone");
        assert!((depth(ray(Vec3::Z, 100.0)) - (0.95 + 0.16)).abs() < 1e-5);
        let mixed = (Vec3::X * 0.1 + HORIZON * HORIZON_WEIGHT) / (0.1 + HORIZON_WEIGHT);
        assert!((ray(Vec3::Z, 15.0).light - mixed).abs().max_element() < 1e-6);
        // Straight up the path ends CEILING above the eye, however far the surface is.
        const { assert!(CEILING > 20.0, "the test's far layer starts below the ceiling") };
        let up = depth(ray(Vec3::Y, SKY_DISTANCE));
        let expected = 0.01 * (CEILING - 5.0) + 0.002 * (CEILING - 20.0);
        assert!((up - f64::from(expected)).abs() < 1e-5, "{up}");
        assert_eq!(ray(Vec3::Y, 1000.0), ray(Vec3::Y, 50_000.0));
    }

    #[test]
    fn distant_surfaces_take_the_sky_s_fog_in_every_direction() {
        let fog = Fog {
            near: FogLayer { color: Vec3::new(1.0, 0.8, 0.7), density: 0.003, start: 0.0 },
            far: FogLayer { color: Vec3::new(1.0, 0.95, 0.9), density: 0.002, start: 40.0 },
            height: HeightFog { color: Vec3::new(0.7, 0.8, 0.9), density: 0.02, falloff: 0.2, base: 0.0 },
            sun_inscatter: Vec3::new(0.5, 0.3, 0.1),
            characters: 0.5,
        };
        let eye = Vec3::new(0.0, 2.0, 0.0);
        let toward_key = Vec3::new(0.3, 0.1, -0.95).normalize();
        for elevation in [-0.3f32, -0.01, 0.0, 0.004, 0.05, 0.3, 1.2] {
            let direction = Vec3::new(0.0, elevation.sin(), -elevation.cos());
            let sky = fog.ray(eye, direction, SKY_DISTANCE, 1.0, HORIZON, toward_key);
            let surface = fog.ray(eye, direction, 2.0e5, 1.0, HORIZON, toward_key);
            let (sky, surface) = (sky.apply(HORIZON), surface.apply(Vec3::ZERO));
            // A black surface that far away is the sky's color, wherever the sky is fogged.
            let sky_fogged =
                fog.ray(eye, direction, SKY_DISTANCE, 1.0, HORIZON, toward_key).transmittance < 1e-3;
            if sky_fogged {
                assert!((sky - surface).abs().max_element() < 1e-3, "{elevation}: {sky} against {surface}");
            }
        }
        // At and below the horizon the sky is all fog; overhead it keeps most of its color.
        let level = fog.ray(eye, Vec3::NEG_Z, SKY_DISTANCE, 1.0, HORIZON, toward_key);
        assert!(level.transmittance < 1e-6);
        let overhead = fog.ray(eye, Vec3::Y, SKY_DISTANCE, 1.0, HORIZON, toward_key);
        assert!(overhead.transmittance > 0.7, "{}", overhead.transmittance);
        // The far layer alone takes the horizon's own color (but for the horizon's weight in the
        // mix, over the stretch the camera sees).
        let aerial = Fog { far: fog.far, ..Fog::default() };
        let far = aerial.ray(eye, Vec3::NEG_Z, 1.0e5, 1.0, HORIZON, toward_key);
        assert!((far.light - HORIZON * fog.far.color).abs().max_element() < HORIZON_WEIGHT);
    }

    #[test]
    fn colors_mix_over_the_stretch_the_camera_sees() {
        // Layers of very different colors, thick enough that a kilometer saturates.
        let fog = Fog {
            near: FogLayer { color: Vec3::new(0.9, 0.5, 0.3), density: 0.01, start: 3.0 },
            far: FogLayer { color: Vec3::ONE, density: 0.005, start: 20.0 },
            height: HeightFog { color: Vec3::new(0.2, 0.4, 0.9), density: 0.04, falloff: 0.25, base: 0.0 },
            ..Fog::default()
        };
        let eye = Vec3::new(0.0, 2.0, 0.0);
        let ray = |elevation: f32, distance| {
            let direction = Vec3::new(0.0, elevation.sin(), -elevation.cos());
            fog.ray(eye, direction, distance, 1.0, HORIZON, Vec3::Y)
        };
        // Geometry two kilometers out along the horizon is all fog, the color of the sky beside
        // it, though the sky's ray runs on through the distance layers to the ceiling...
        for elevation in [-0.003, -0.001, 0.001, 0.003] {
            let (geometry, sky) = (ray(elevation, 2000.0), ray(elevation, SKY_DISTANCE));
            assert!(geometry.transmittance < 1e-6 && sky.transmittance < 1e-6);
            let difference = (geometry.light - sky.light).abs().max_element();
            assert!(difference < 1e-3, "{elevation}: {} against {}", geometry.light, sky.light);
        }
        // ...and across the horizon the color changes smoothly: no seam where the ground ends.
        let (below, above) = (ray(-0.001, 1000.0), ray(0.001, SKY_DISTANCE));
        assert!(
            (below.light - above.light).abs().max_element() < 0.01,
            "{} against {}",
            below.light,
            above.light
        );
        // A ray shorter than the stretch mixes over its whole length: by its layers' depths.
        let short = ray(0.0, 30.0);
        let (near, far, height) = (0.01 * 27.0, 0.005 * 10.0, 0.04 * (-0.5f32).exp() * 30.0);
        let expected =
            (fog.near.color * near + HORIZON * far + fog.height.color * height + HORIZON * HORIZON_WEIGHT)
                / (near + far + height + HORIZON_WEIGHT);
        assert!((short.light - expected).abs().max_element() < 1e-5, "{} against {expected}", short.light);
    }

    #[test]
    fn characters_take_their_share_and_the_sun_side_glows() {
        let fog = Fog {
            far: FogLayer { color: Vec3::ONE, density: 0.01, start: 0.0 },
            sun_inscatter: Vec3::new(0.6, 0.3, 0.1),
            ..Fog::default()
        };
        let world = fog.ray(Vec3::ZERO, Vec3::NEG_Z, 100.0, 1.0, HORIZON, Vec3::NEG_Z);
        let character = fog.ray(Vec3::ZERO, Vec3::NEG_Z, 100.0, 0.25, HORIZON, Vec3::NEG_Z);
        assert!((depth(character) - 0.25 * depth(world)).abs() < 1e-6);
        assert_eq!(fog.ray(Vec3::ZERO, Vec3::NEG_Z, 100.0, 0.0, HORIZON, Vec3::NEG_Z).transmittance, 1.0);
        assert_eq!(world.sun, fog.sun_inscatter, "toward the key light, the whole lobe");
        let side = fog.ray(Vec3::ZERO, Vec3::NEG_X, 100.0, 1.0, HORIZON, Vec3::NEG_Z);
        assert_eq!(side.sun, Vec3::ZERO, "nothing at right angles");
        // cos⁸: 20° away, 61% of it.
        let angle = 20f32.to_radians();
        let off =
            fog.ray(Vec3::ZERO, Vec3::new(-angle.sin(), 0.0, -angle.cos()), 100.0, 1.0, HORIZON, Vec3::NEG_Z);
        assert!(
            (off.sun - fog.sun_inscatter * angle.cos().powi(8)).abs().max_element() < 1e-5,
            "{}",
            off.sun
        );
    }

    #[test]
    fn fog_without_density_is_clear_and_bad_values_are_safe() {
        let colored = Fog {
            near: FogLayer { color: Vec3::ONE, density: 0.0, start: 0.0 },
            sun_inscatter: Vec3::ONE,
            ..Fog::default()
        };
        assert!(!colored.is_visible());
        assert_eq!(colored.ray(Vec3::ZERO, Vec3::Z, 1e4, 1.0, HORIZON, Vec3::Z), FogRay::CLEAR);
        let bad = Fog {
            near: FogLayer { color: Vec3::NAN, density: f32::INFINITY, start: -3.0 },
            far: FogLayer { color: Vec3::ONE, density: -1.0, start: f32::NAN },
            height: HeightFog { color: Vec3::ONE, density: 0.01, falloff: f32::NAN, base: f32::INFINITY },
            sun_inscatter: Vec3::splat(-1.0),
            characters: f32::NAN,
        };
        let safe = bad.sanitized();
        assert_eq!(safe.near, FogLayer::default());
        assert_eq!(safe.far.density, 0.0);
        let shape = [safe.near.start, safe.far.start, safe.height.falloff, safe.height.base];
        assert_eq!(shape, [0.0; 4]);
        assert_eq!((safe.sun_inscatter, safe.characters), (Vec3::ZERO, 0.0));
        assert_eq!(safe.height.density, 0.01, "the one layer left");
        assert!(safe.is_visible());
        let ray = bad.ray(Vec3::ZERO, Vec3::Z, 100.0, 1.0, HORIZON, Vec3::Z);
        assert!(ray.transmittance.is_finite() && ray.light.is_finite() && ray.sun.is_finite());
    }
}
