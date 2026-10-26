//! Where the shadow maps look (CPU only): the quantized key light, the light space every map of
//! a frame shares, the cascades' splits and their stable fits, and the per-character frusta.
//!
//! - **The key light, quantized.** Shadows are cast along [`quantize`]d directions: the
//!   horizontal offset a shadow moves per meter of height (`toward.xz / toward.y`) is rounded to
//!   [`SLOPE_STEP`]. Between steps every map's light space is the same, so texel snapping holds
//!   and shadows stay perfectly still while time of day turns the sun slowly; at a step every
//!   shadow moves by at most [`SLOPE_STEP`] meters per meter of its caster's height. Rounding the
//!   slope rather than the angle keeps that step the same at noon and at sunset, where an angle
//!   step would move long shadows by meters.
//! - **Light space** ([`LightSpace`]): z toward the key light, x and y across it, from the
//!   direction alone and smooth in it (never degenerate above the horizon).
//! - **Cascades** split the view's depth range by the practical scheme (a blend of logarithmic
//!   and uniform splits, [`split_distances`]). Each covers the sphere around its slice of the
//!   view frustum ([`slice_sphere`]): its size depends only on the slice and the field of view,
//!   never on where the camera looks, so turning the camera moves no texel. Its center is snapped
//!   to whole texels in light space ([`CascadeFit::new`]), so moving the camera moves no texel
//!   either: the map's grid stays fixed to the world.
//! - **Character frusta** ([`CascadeFit::character`]) cover a character's sphere, padded so its
//!   silhouette keeps clear of the tile's edges, and snapped the same way.

use glam::{Mat4, Vec2, Vec3};

use crate::CameraData;
use crate::bounds::Sphere;

/// The quantization step of the key light's shadow slope (see the module docs): a caster's
/// shadow moves by at most 5 mm per meter of its height at a step.
pub(crate) const SLOPE_STEP: f32 = 0.005;
/// The lowest the key light casts from, as the sine of its elevation (3°): a light at or below
/// the horizon casts as if just above it, so shadows stay finite.
const MIN_ELEVATION_SIN: f32 = 0.052_335_96;
/// How far toward the light a cascade's near plane reaches past its sphere, in meters: casters
/// between the sun and the cascade (a tall building's top) still cast into it.
const CASTER_REACH: f32 = 256.0;
/// How far past a character's sphere, away from the light, its frustum reaches: the longest
/// shadow it casts onto the ground, in meters (a 2 m character's shadow at 3° of elevation).
pub(crate) const CHARACTER_REACH: f32 = 40.0;
/// Texels kept clear around a character's silhouette in its tile, so the filter never reads a
/// neighbouring tile.
pub(crate) const TILE_PADDING: f32 = 4.0;
/// Weight of the logarithmic split in the practical split scheme: the rest is uniform.
const SPLIT_LOG_WEIGHT: f32 = 0.8;

/// The key light's direction as shadows are cast along it: `toward` (a vector toward the light,
/// any length) with its elevation held at 3° or more and its shadow slope rounded to
/// [`SLOPE_STEP`] (see the module docs). Unit length; straight up for a zero or non-finite
/// vector.
pub(crate) fn quantize(toward: Vec3) -> Vec3 {
    let toward = toward.try_normalize().unwrap_or(Vec3::Y);
    let horizontal = Vec2::new(toward.x, toward.z);
    let length = horizontal.length();
    let (horizontal, height) = if toward.y >= MIN_ELEVATION_SIN {
        (horizontal, toward.y)
    } else {
        // Along the horizon at its azimuth, raised to the lowest elevation that casts.
        let azimuth = if length > 1e-6 { horizontal / length } else { Vec2::X };
        (azimuth * (1.0 - MIN_ELEVATION_SIN * MIN_ELEVATION_SIN).sqrt(), MIN_ELEVATION_SIN)
    };
    let slope = (horizontal / height / SLOPE_STEP).round() * SLOPE_STEP;
    Vec3::new(slope.x, 1.0, slope.y).normalize()
}

/// The light space every shadow map of a frame shares: z toward the key light, x and y across
/// it. A right-handed view space whose camera looks along the light, as the shadow passes
/// render through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightSpace {
    pub x: Vec3,
    pub y: Vec3,
    /// Unit vector toward the light.
    pub z: Vec3,
}

impl LightSpace {
    /// The light space of a light toward `toward` (unit, above the horizon, as [`quantize`]
    /// returns). x is horizontal, `Z × toward` normalized, which never degenerates while the
    /// light is above the horizon and turns smoothly with it.
    pub(crate) fn new(toward: Vec3) -> Self {
        let x = Vec3::new(-toward.y, toward.x, 0.0).normalize();
        Self { x, y: toward.cross(x), z: toward }
    }

    /// `point` in light space.
    pub(crate) fn point(&self, point: Vec3) -> Vec3 {
        Vec3::new(point.dot(self.x), point.dot(self.y), point.dot(self.z))
    }

    /// World to light space.
    pub(crate) fn view(&self) -> Mat4 {
        Mat4::from_cols(
            Vec3::new(self.x.x, self.y.x, self.z.x).extend(0.0),
            Vec3::new(self.x.y, self.y.y, self.z.y).extend(0.0),
            Vec3::new(self.x.z, self.y.z, self.z.z).extend(0.0),
            glam::Vec4::W,
        )
    }
}

/// Where one shadow map looks: a square in light space centered on `center` (x and y snapped
/// to whole texels), `2 × radius` meters across, between depths `near` (toward the light,
/// depth 1 under reverse-Z) and `far` (depth 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CascadeFit {
    pub light: LightSpace,
    pub center: Vec3,
    pub radius: f32,
    pub near: f32,
    pub far: f32,
    /// Texels across.
    pub resolution: u32,
}

impl CascadeFit {
    /// The fit of a cascade of `resolution` texels around `sphere` (world space), seen along
    /// `light`: the sphere's radius rounded up to 1/16 m, its center snapped to whole texels.
    pub(crate) fn new(light: LightSpace, sphere: Sphere, resolution: u32) -> Self {
        let radius = (sphere.radius * 16.0).ceil() / 16.0;
        let center = light.point(sphere.center);
        Self::snapped(
            light,
            center,
            radius,
            resolution,
            (center.z + radius + CASTER_REACH, center.z - radius),
        )
    }

    /// The frustum of a character's map of `resolution` texels around `sphere` (world space):
    /// padded by [`TILE_PADDING`] texels each side, reaching [`CHARACTER_REACH`] past the
    /// sphere away from the light.
    pub(crate) fn character(light: LightSpace, sphere: Sphere, resolution: u32) -> Self {
        let texels = resolution as f32;
        let radius = sphere.radius * texels / (texels - 2.0 * TILE_PADDING);
        let center = light.point(sphere.center);
        let depths = (center.z + radius, center.z - radius - CHARACTER_REACH);
        Self::snapped(light, center, radius, resolution, depths)
    }

    fn snapped(
        light: LightSpace,
        center: Vec3,
        radius: f32,
        resolution: u32,
        (near, far): (f32, f32),
    ) -> Self {
        let texel = 2.0 * radius / resolution as f32;
        let snapped =
            Vec3::new((center.x / texel).floor() * texel, (center.y / texel).floor() * texel, center.z);
        Self { light, center: snapped, radius, near, far, resolution }
    }

    /// One texel, in meters.
    pub(crate) fn texel(&self) -> f32 {
        2.0 * self.radius / self.resolution as f32
    }

    /// Reverse-Z depth of light-space z: `z × scale + offset`, 1 at `near`, 0 at `far`.
    pub(crate) fn depth_transform(&self) -> (f32, f32) {
        let scale = 1.0 / (self.near - self.far);
        (scale, -self.far * scale)
    }

    /// The orthographic projection from light space into its clip space (reverse-Z).
    pub(crate) fn projection(&self) -> Mat4 {
        let (scale, offset) = self.depth_transform();
        let inverse = 1.0 / self.radius;
        Mat4::from_cols(
            glam::Vec4::new(inverse, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, inverse, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, scale, 0.0),
            glam::Vec4::new(-self.center.x * inverse, -self.center.y * inverse, offset, 1.0),
        )
    }

    /// The camera the shadow pass renders the map through: light space, then the projection.
    pub(crate) fn camera(&self) -> CameraData {
        let view = self.light.view();
        let position = self.light.x * self.center.x + self.light.y * self.center.y + self.light.z * self.near;
        CameraData { view, proj: self.projection(), position }
    }

    /// Whether a caster's world-space `sphere` can cast into the map: it overlaps the square and
    /// lies between the planes.
    pub(crate) fn holds(&self, sphere: &Sphere) -> bool {
        let at = self.light.point(sphere.center);
        let reach = self.radius + sphere.radius;
        (at.x - self.center.x).abs() <= reach
            && (at.y - self.center.y).abs() <= reach
            && at.z + sphere.radius >= self.far
            && at.z - sphere.radius <= self.near
    }
}

/// The view distances at which `cascades` cascades end, from `near` out to `distance`, by the
/// practical split scheme (Zhang et al. 2006): each a blend of the logarithmic and the uniform
/// split, mostly logarithmic, so near cascades stay small.
pub(crate) fn split_distances(near: f32, distance: f32, cascades: usize) -> [f32; super::MAX_CASCADES] {
    let mut splits = [distance; super::MAX_CASCADES];
    let near = near.clamp(1e-3, distance * 0.5);
    for (index, split) in splits.iter_mut().enumerate().take(cascades) {
        let t = (index + 1) as f32 / cascades as f32;
        let logarithmic = near * (distance / near).powf(t);
        let uniform = near + (distance - near) * t;
        *split = SPLIT_LOG_WEIGHT * logarithmic + (1.0 - SPLIT_LOG_WEIGHT) * uniform;
    }
    splits
}

/// What a view's perspective projection says about its frustum: its near plane and the tangents
/// of its half-angles. `CameraData::proj` is a symmetric, reverse-Z, infinite perspective
/// (`perspective_infinite_reverse`), as `CameraData` requires.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frustum {
    pub near: f32,
    /// tan(half the horizontal field of view), tan(half the vertical one).
    pub tangents: Vec2,
}

impl Frustum {
    pub(crate) fn of(camera: &CameraData) -> Self {
        let proj = camera.proj;
        let near = proj.w_axis.z;
        let tangents = Vec2::new(1.0 / proj.x_axis.x, 1.0 / proj.y_axis.y);
        let valid = near.is_finite() && near > 0.0 && tangents.is_finite() && tangents.min_element() > 0.0;
        if valid { Self { near, tangents } } else { Self { near: 0.1, tangents: Vec2::ONE } }
    }
}

/// The smallest sphere around the slice of `camera`'s frustum between view distances `from` and
/// `to`: on the view axis, so its size is the same however the camera turns.
pub(crate) fn slice_sphere(camera: &CameraData, frustum: &Frustum, from: f32, to: f32) -> Sphere {
    // Squared distance of a slice corner from the axis per meter of view depth.
    let spread = frustum.tangents.length_squared();
    // Equidistant from the near and far corners, unless that falls past the far plane, where the
    // far corners alone bound it.
    let along = ((from + to) * 0.5 * (1.0 + spread)).min(to);
    let radius = ((to - along).powi(2) + to * to * spread).sqrt();
    let forward = -camera.view.row(2).truncate().normalize_or(Vec3::NEG_Z);
    Sphere { center: camera.position + forward * along, radius }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(eye: Vec3, target: Vec3) -> CameraData {
        CameraData::look_at(eye, target, Vec3::Y, 0.75, 16.0 / 9.0, 0.05)
    }

    #[test]
    fn quantized_directions_round_the_shadow_slope() {
        let toward = Vec3::new(-0.55, 0.7, 0.45).normalize();
        let quantized = quantize(toward);
        assert!((quantized.length() - 1.0).abs() < 1e-6);
        let slope = |v: Vec3| Vec2::new(v.x, v.z) / v.y;
        let step = slope(quantized) / SLOPE_STEP;
        assert!((step - step.round()).abs().max_element() < 1e-3, "on the grid: {step}");
        assert!((slope(quantized) - slope(toward)).abs().max_element() <= SLOPE_STEP * 0.5 + 1e-6);
        // A slow turn keeps the direction until the slope crosses a step.
        let nudged = Vec3::new(-0.5502, 0.7, 0.45).normalize();
        assert_eq!(quantize(nudged), quantized, "a sub-step turn moves nothing");
        // A low sun's step moves its shadows no more than a high one's.
        let low = Vec3::new(1.0, 0.1, 0.0).normalize();
        let shadow_of_a_meter = |v: Vec3| slope(quantize(v)).x;
        assert!((shadow_of_a_meter(low) - slope(low).x).abs() <= SLOPE_STEP * 0.5 + 1e-4);
    }

    #[test]
    fn lights_at_the_horizon_cast_from_just_above_it() {
        for toward in [Vec3::X, Vec3::new(1.0, -0.5, 1.0), Vec3::NEG_Y, Vec3::ZERO, Vec3::splat(f32::NAN)] {
            let quantized = quantize(toward);
            assert!(
                quantized.is_finite() && quantized.y >= MIN_ELEVATION_SIN * 0.99,
                "{toward} → {quantized}"
            );
        }
        assert_eq!(quantize(Vec3::Y), Vec3::Y);
        assert_eq!(quantize(Vec3::ZERO), Vec3::Y, "no direction: straight up");
        let along_x = quantize(Vec3::new(1.0, -0.2, 0.0));
        assert!(along_x.x > 0.99 && along_x.z == 0.0, "keeps its azimuth: {along_x}");
    }

    #[test]
    fn light_space_is_orthonormal_and_smooth_at_the_zenith() {
        for toward in [Vec3::Y, quantize(Vec3::new(0.3, 1.0, -0.2)), quantize(Vec3::new(-1.0, 0.06, 0.1))] {
            let light = LightSpace::new(toward);
            for (a, b) in [(light.x, light.y), (light.y, light.z), (light.z, light.x)] {
                assert!(a.dot(b).abs() < 1e-6 && (a.length() - 1.0).abs() < 1e-6);
            }
            assert!((light.x.cross(light.y) - light.z).length() < 1e-6, "right-handed");
            let p = Vec3::new(1.0, 2.0, 3.0);
            assert!((light.view().transform_point3(p) - light.point(p)).length() < 1e-5);
        }
        let zenith = LightSpace::new(Vec3::Y);
        let near = LightSpace::new(Vec3::new(1e-4, 1.0, 0.0).normalize());
        assert!((zenith.x - near.x).length() < 1e-3, "no flip near the zenith");
    }

    #[test]
    fn cascades_ignore_the_cameras_turn_and_snap_to_texels() {
        let frustum = Frustum::of(&camera(Vec3::ZERO, Vec3::NEG_Z));
        assert!((frustum.near - 0.05).abs() < 1e-6);
        assert!((frustum.tangents.y - (0.375f32).tan()).abs() < 1e-5);
        let light = LightSpace::new(quantize(Vec3::new(-0.55, 0.7, 0.45)));
        let fit = |eye: Vec3, target: Vec3| {
            let camera = camera(eye, target);
            CascadeFit::new(light, slice_sphere(&camera, &frustum, 0.05, 6.0), 2048)
        };
        let ahead = fit(Vec3::new(0.0, 1.5, 3.0), Vec3::new(0.0, 1.0, 0.0));
        let turned = fit(Vec3::new(0.0, 1.5, 3.0), Vec3::new(4.0, 0.0, -2.0));
        assert_eq!(ahead.radius, turned.radius, "the size holds however the camera turns");
        // A sub-texel move of the camera keeps the grid; the snapped center moves by whole texels.
        let texel = ahead.texel();
        let moved = fit(Vec3::new(0.0003, 1.5, 3.0), Vec3::new(0.0003, 1.0, 0.0));
        let shift = (moved.center - ahead.center) / texel;
        assert!((shift.x - shift.x.round()).abs() < 1e-3 && (shift.y - shift.y.round()).abs() < 1e-3);
        assert!(shift.x.abs() <= 1.0 && shift.y.abs() <= 1.0, "{shift}");
        // The square holds the slice.
        let camera = camera(Vec3::new(0.0, 1.5, 3.0), Vec3::new(0.0, 1.0, 0.0));
        let sphere = slice_sphere(&camera, &frustum, 0.05, 6.0);
        assert!(ahead.holds(&Sphere { center: sphere.center, radius: 0.0 }));
    }

    #[test]
    fn slice_spheres_hold_their_corners() {
        let camera = camera(Vec3::new(1.0, 2.0, 3.0), Vec3::new(-2.0, 0.0, -5.0));
        let frustum = Frustum::of(&camera);
        let world_from_view = camera.view.inverse();
        for (from, to) in [(0.05, 6.0), (6.0, 14.0), (30.0, 120.0)] {
            let sphere = slice_sphere(&camera, &frustum, from, to);
            for depth in [from, to] {
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                    let view =
                        Vec3::new(sx * frustum.tangents.x * depth, sy * frustum.tangents.y * depth, -depth);
                    let corner = world_from_view.transform_point3(view);
                    assert!(corner.distance(sphere.center) <= sphere.radius * 1.0001, "{from}–{to}");
                }
            }
        }
    }

    #[test]
    fn splits_grow_from_near_to_the_distance() {
        let splits = split_distances(0.05, 120.0, 4);
        assert_eq!(splits[3], 120.0);
        assert!(splits.windows(2).all(|pair| pair[0] < pair[1]), "{splits:?}");
        assert!(splits[0] < 10.0, "the first cascade stays near: {splits:?}");
        let two = split_distances(0.05, 40.0, 2);
        assert_eq!(two[1], 40.0);
        assert!(two[0] > 1.0 && two[0] < 10.0, "{two:?}");
    }

    #[test]
    fn projections_map_the_square_and_depths_to_clip_space() {
        let light = LightSpace::new(quantize(Vec3::new(0.2, 1.0, 0.1)));
        let fit = CascadeFit::new(light, Sphere { center: Vec3::new(3.0, 0.0, -2.0), radius: 5.0 }, 1024);
        let clip_from_world = fit.projection() * light.view();
        let world = |x: f32, y: f32, z: f32| light.x * x + light.y * y + light.z * z;
        let corner = clip_from_world.project_point3(world(
            fit.center.x + fit.radius,
            fit.center.y - fit.radius,
            fit.near,
        ));
        assert!((corner - Vec3::new(1.0, -1.0, 1.0)).length() < 1e-4, "{corner}");
        let far = clip_from_world.project_point3(world(fit.center.x, fit.center.y, fit.far));
        assert!(far.abs().max_element() < 1e-4, "the far plane is depth 0: {far}");
        let camera = fit.camera();
        assert!((camera.proj * camera.view - clip_from_world).abs_diff_eq(Mat4::ZERO, 1e-5));
        // Casters between the light and the cascade cast into it; those behind it don't.
        let at = |z: f32| Sphere { center: world(fit.center.x, fit.center.y, z), radius: 0.5 };
        assert!(fit.holds(&at(fit.near - 1.0)) && fit.holds(&at(fit.center.z)));
        assert!(!fit.holds(&at(fit.far - 2.0)), "behind the far plane");
        assert!(
            !fit.holds(&Sphere { center: world(fit.center.x + fit.radius + 1.0, 0.0, 0.0), radius: 0.5 })
        );
    }

    #[test]
    fn character_frusta_keep_their_silhouette_clear_of_the_edges() {
        let light = LightSpace::new(Vec3::Y);
        let sphere = Sphere { center: Vec3::new(0.0, 1.0, 0.0), radius: 1.0 };
        let fit = CascadeFit::character(light, sphere, 1024);
        let padding = fit.radius - sphere.radius;
        assert!(padding >= TILE_PADDING * fit.texel() * 0.99, "{padding} m");
        assert!(fit.far < light.point(sphere.center).z - sphere.radius - CHARACTER_REACH + 1e-3);
    }
}
