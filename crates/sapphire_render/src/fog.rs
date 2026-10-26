//! Fog (M1 W3, spec §5.12 "Fog"; look pillar 3, gradients and layered atmosphere): analytic
//! distance and height fog over every world pass and the sky, and the fog card family.
//!
//! # The fog
//!
//! Time of day resolves the frame's [`Fog`] (its keys set it per hour, zones and cutscenes
//! override it), and every pass that draws the world applies it at its single final-color point
//! through one shared function, `fogged` in `shaders/frame.wgsl`: the environment shader, the
//! anime character shader and its outlines (a line fogs like its surface), eyes and brows drawn
//! through hair (M1 C5, as the surface under them), fog cards, and the sky. It is a few
//! exponentials a pixel, on every tier.
//!
//! - **Three layers.** Two homogeneous distance layers, near haze and far aerial perspective,
//!   each from its own start distance (Honkai Impact 3rd's split), and an exponential height
//!   layer, `density × exp(−falloff × (y − base))`, integrated analytically along the view ray:
//!   `density × e^(−k·h₀) × d × (1 − e^(−x)) / x` with `x = k · Δy`, its series where `x` is
//!   near 0 (horizontal rays, no falloff), and exponents held under [`MAX_EXPONENT`] so a camera
//!   far below the base or a ray plunging down stays finite. A camera inside the layer and one
//!   above it take the same formula.
//! - **Colored by the sky.** The layers mix by their optical depths along the ray, and the far
//!   layer's color is the sky's horizon ([`SkyGradient::horizon`](crate::SkyGradient)) times its
//!   authored tint: geometry far enough away takes the sky's own color, which is how WuWa's and
//!   Genshin's distance melts into the painted sky. Near haze and height fog keep their authored
//!   colors (valley mist, morning haze).
//! - **Decision: the layers mix over the stretch of the ray the camera sees**, its first
//!   [`MIX_DEPTH`] optical depths at the densities around the camera, not over the whole ray. A
//!   long ray's far end lies behind fog the camera can't see through, and there the layers'
//!   proportions differ (a rising ray leaves the height layer, a sky ray runs on to infinity
//!   inside the distance layers): weighing it in would color distant geometry and the sky just
//!   above it differently, a seam along the horizon wherever the layers' colors differ. Rays
//!   shorter than that stretch, all of them in light fog, mix over their whole length.
//! - **Sun inscatter:** a forward lobe, cos⁸ of the angle to the key light (the sun by day, the
//!   moon at night; half its peak 24° away), in [`Fog::sun_inscatter`]'s color, added wherever
//!   the fog is, so the sun side of a hazy view glows.
//! - **Decision: the sky takes the same fog, as at infinite distance** ([`SKY_DISTANCE`]), so
//!   wherever geometry meets the sky the two agree: the fog over a surface tends to the sky's as
//!   its distance grows, in every direction. For that the distance layers fill the air up to
//!   [`CEILING`] meters above the camera, a ray's path through them ending where it has risen
//!   that far: the sky takes them as a band along the horizon (fully fogged at and below it,
//!   fading with elevation) and the zenith keeps its color, while tall peaks rise out of the haze
//!   rather than vanishing. Height fog thins by its own falloff. `sky_color()`, which ambient
//!   light reads, is unchanged: the fog is over the sky pass's color alone. The sky cube's faces
//!   take it too (from the world's origin), so reflections see the hazy horizon.
//! - **Decision: characters take a share of the fog** ([`Fog::characters`], keyed by time of
//!   day, so an art director sets it per hour and a cutscene per shot): their optical depth is
//!   scaled by it. Anime games keep characters readable in fog, and a character standing where
//!   the world melts away still reads as a figure; at 0 characters keep their colors at any
//!   distance. "Characters" are the instances the frame lists, as for post's character mask, and
//!   their outlines with them; anime character surfaces that aren't listed (props) fog as the
//!   world.
//! - **Exactly nothing without fog.** A frame whose fog has no density ([`Fog::is_visible`])
//!   skips it on a uniform branch, and every pass draws exactly what it drew before fog: the pin
//!   of the golden scenes from before it (`--fog off` in the sandbox).
//! - **The CPU reference:** [`Fog::ray`] evaluates the same function, for tests and tools.
//!
//! # Fog cards
//!
//! Translucent sheets artists place for valley mist, light in rooms and street haze (WuWa's fake
//! volumetric fog), [`FogCardMaterial`](sapphire_assets::material::FogCardMaterial)s on meshes
//! (`shaders/fog_card.wgsl`): world-placed, or turned to face the camera (whole, or upright
//! about the world's up axis) around their instance's origin. A card's density is its map's R
//! (its shape, soft at the edges) times G (a tiling octave drifting by its scroll); it fades
//! where it meets geometry (a soft particle) and near the lens, and is lit by the fog itself:
//! its tint × the light the frame's fog has at its distance (the sky's horizon where the frame
//! has none), brightening toward the key light by its sun scatter, then fogged like any surface.
//! Cards draw after every other transparent surface, sorted back to front by their instances'
//! bounds, without writing depth; they cast no shadow and have no outline.
//!
//! - **Decision: the depth they fade against is the scene prepass's**, where the frame has it:
//!   the desktop tiers, whose temporal AA runs the prepass anyway, so it costs nothing more. The
//!   main pass reads it as a texture while it writes its own depth (under temporal AA they are
//!   different textures). Phones draw 4× MSAA in tile memory with no prepass, and reading the
//!   main pass's depth there would cost a store and a resolve (and on Mali its early-Z), so
//!   there cards fade near the lens alone and their maps' soft edges carry the rest: artists
//!   keep mobile cards clear of geometry or paint their edges where they meet it.
//!
//! # Later milestones
//!
//! - **Volumetric fog** on desktop (froxels lit by the shadow cascades and local lights, the
//!   quality tiers' `volumetric_fog`), with light shafts, over this analytic fog.
//! - **Fog density volumes** per zone (beyond time of day's zone overrides of the whole fog),
//!   and cards lit by local lights and probes.
//! - **One sort for every transparent surface**, with draw sorting (the crate docs); cards sort
//!   among themselves today.
//! - **Soft cards on Mobile-High** once a depth prepass runs there for another reader.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sapphire_assets::mesh::PositionFormat;
use sapphire_rhi::wgpu;

use crate::Fog;
use crate::draw::DrawRef;
use crate::mesh_pool::MeshRecord;
use crate::pipelines::{DEPTH_FORMAT, FOG_CARD_DEPTH_BINDING, SurfaceVariant};
use crate::scene::{DrawList, InstanceSlots};

/// Meters above the camera the distance layers reach (see the [module docs](self)).
pub const CEILING: f32 = 30.0;

/// The sky's distance as the fog sees it: far enough that every layer looking at the horizon has
/// saturated.
pub const SKY_DISTANCE: f32 = 1.0e6;

/// The largest exponent the height layer's density and path take, so its optical depth stays
/// finite (under about 10³² for any density under 10) and the layers' mix with it.
pub const MAX_EXPONENT: f32 = 30.0;

/// The sky's horizon in the layers' mix, as an optical depth: what the light leans toward where
/// the fog is thin, so fog cards in a frame without fog take the horizon's color.
pub(crate) const HORIZON_WEIGHT: f32 = 1.0e-3;

/// Below this `|x|`, the height layer's `(1 − e^(−x)) / x` is its series.
pub(crate) const SERIES_BELOW: f32 = 1.0e-3;

/// The optical depth, at the densities around the camera, of the stretch of a ray over which the
/// layers' colors mix (see the [module docs](self)): past it, under 2% of the light reaches the
/// camera.
pub const MIX_DEPTH: f32 = 4.0;

/// The least density the mixing stretch is measured by, so a frame without fog has one.
pub(crate) const MIN_DENSITY: f32 = 1.0e-12;

/// The frame's fog as the shaders read it (WGSL `FogUniforms`, the last field of
/// `FrameUniforms`). 80 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuFog {
    /// rgb: the near layer's color; a: its extinction per meter.
    pub near: [f32; 4],
    /// rgb: the far layer's tint on the sky's horizon; a: its extinction per meter.
    pub far: [f32; 4],
    /// rgb: the height layer's color; a: its extinction per meter at its base.
    pub height: [f32; 4],
    /// x: where the near layer begins, meters from the camera; y: where the far one does; z: the
    /// height layer's falloff per meter; w: its base height.
    pub shape: [f32; 4],
    /// rgb: the light scattered toward the camera around the key light; a: characters' share.
    pub light: [f32; 4],
}

impl GpuFog {
    /// `fog`, made safe for the shaders: non-finite or negative densities, colors, starts and
    /// falloff are 0, a non-finite base 0, and the characters' share within 0..=1 (0 when it
    /// isn't finite).
    pub(crate) fn new(fog: &Fog) -> Self {
        let amount = |value: f32| if value.is_finite() { value.max(0.0) } else { 0.0 };
        let color = |color: Vec3| color.to_array().map(amount);
        let layer = |rgb: Vec3, density: f32| {
            let [r, g, b] = color(rgb);
            [r, g, b, amount(density)]
        };
        let base = if fog.height.base.is_finite() { fog.height.base } else { 0.0 };
        let [r, g, b] = color(fog.sun_inscatter);
        let characters = if fog.characters.is_finite() { fog.characters.clamp(0.0, 1.0) } else { 0.0 };
        Self {
            near: layer(fog.near.color, fog.near.density),
            far: layer(fog.far.color, fog.far.density),
            height: layer(fog.height.color, fog.height.density),
            shape: [amount(fog.near.start), amount(fog.far.start), amount(fog.height.falloff), base],
            light: [r, g, b, characters],
        }
    }

    /// Whether the shaders draw it: some layer has a density.
    pub(crate) fn is_visible(&self) -> bool {
        self.near[3] + self.far[3] + self.height[3] > 0.0
    }

    /// Layers with a density: the stats' count.
    pub(crate) fn layers(&self) -> u32 {
        [self.near[3], self.far[3], self.height[3]].iter().filter(|density| **density > 0.0).count() as u32
    }
}

/// The fog over one view ray ([`Fog::ray`]), as the shaders evaluate it (`fog_ray` in
/// `frame.wgsl`).
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

impl Fog {
    /// The fog over the ray from `eye` along the unit vector `direction` to what lies `distance`
    /// meters along it ([`SKY_DISTANCE`] for the sky), as the shaders apply it to a surface's
    /// final color: `share` is the share of it the surface takes (1 for the world,
    /// [`Self::characters`] for characters), `horizon` the sky's horizon color, and `toward_key`
    /// the unit vector toward the key light. [`FogRay::CLEAR`] where the fog has no density.
    ///
    /// The values are made safe as the renderer makes them for the GPU (negative and non-finite
    /// ones are 0), so this is exactly what the shaders compute, to `f32` rounding.
    pub fn ray(
        &self,
        eye: Vec3,
        direction: Vec3,
        distance: f32,
        share: f32,
        horizon: Vec3,
        toward_key: Vec3,
    ) -> FogRay {
        let fog = GpuFog::new(self);
        if !fog.is_visible() {
            return FogRay::CLEAR;
        }
        let [start_near, start_far, falloff, base] = fog.shape;
        let rise = direction.y;
        // The height layer's density at the eye, over its density at its base.
        let start = (-falloff * (eye.y - base)).min(MAX_EXPONENT).exp();
        // The layers' optical depths over the ray's first `length` meters.
        let depths = |length: f32| {
            // The distance layers end where the ray has risen CEILING above the eye.
            let reach = if rise > 0.0 { length.min(CEILING / rise) } else { length };
            let near = fog.near[3] * (reach - start_near).max(0.0);
            let far = fog.far[3] * (reach - start_far).max(0.0);
            // density × ∫₀ˡ exp(−k (h₀ + rise·t)) dt.
            let x = falloff * rise * length;
            let ratio =
                if x.abs() < SERIES_BELOW { 1.0 - 0.5 * x } else { (1.0 - (-x).min(MAX_EXPONENT).exp()) / x };
            Vec3::new(near, far, fog.height[3] * start * length * ratio)
        };
        let total = depths(distance).element_sum();
        // The colors mix over the stretch the camera sees.
        let around = fog.near[3] + fog.far[3] + fog.height[3] * start;
        let mixed = depths(distance.min(MIX_DEPTH / around.max(MIN_DENSITY)));
        let color = |rgba: [f32; 4]| Vec3::new(rgba[0], rgba[1], rgba[2]);
        let light = (color(fog.near) * mixed.x
            + color(fog.far) * horizon * mixed.y
            + color(fog.height) * mixed.z
            + horizon * HORIZON_WEIGHT)
            / (mixed.element_sum() + HORIZON_WEIGHT);
        let c = direction.dot(toward_key).clamp(0.0, 1.0);
        let lobe = (c * c) * (c * c);
        FogRay { transmittance: (-share * total).exp(), light, sun: color(fog.light) * (lobe * lobe) }
    }
}

/// The name of the fog card shader's depth fade constant (`fog_card.wgsl`).
pub(crate) const DEPTH_FADE_CONSTANT: &str = "FOG_CARD_DEPTH_FADE";

/// The fog card pipelines' constants: their depth fade, on where cards read the scene prepass's
/// depth.
pub(crate) fn card_constants(depth_fade: bool) -> [(&'static str, f64); 1] {
    [(DEPTH_FADE_CONSTANT, if depth_fade { 1.0 } else { 0.0 })]
}

/// The fog card family's share of a frame: what cards fade against, and their order.
#[derive(Debug)]
pub(crate) struct FogCards {
    /// Cards fade against the scene prepass's depth: the frame has it (see the module docs).
    depth_fade: bool,
    /// A 1×1 depth texture the family's scene group binds where cards read no depth.
    stand_in: wgpu::TextureView,
    /// The scene groups with the prepass's depth, the two used last (under temporal AA with MSAA
    /// the prepass's depth is the history pair's, alternating frame by frame), made again only
    /// when the depth texture or the buffers they bind change, which steady frames never do.
    depth_groups: [Option<DepthGroup>; 2],
    /// Which of `depth_groups` the last frame bound.
    last_group: usize,
    /// This frame's card draws with their distances and their places in the draw list, back to
    /// front; kept across frames, so a warm frame allocates nothing.
    order: Vec<(f32, u32, DrawRef)>,
    draws: Vec<DrawRef>,
}

/// The fog card family's scene group with the prepass's depth, and what it binds.
#[derive(Debug)]
struct DepthGroup {
    depth: wgpu::TextureView,
    instances: wgpu::Buffer,
    materials: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl FogCards {
    /// Cards that fade against the scene prepass's depth if `depth_fade`: where the renderer runs
    /// the prepass anyway (temporal AA).
    pub(crate) fn new(device: &wgpu::Device, depth_fade: bool) -> Self {
        let stand_in = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fog card depth stand-in"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self {
            depth_fade,
            stand_in: stand_in.create_view(&Default::default()),
            depth_groups: [None, None],
            last_group: 0,
            order: Vec::new(),
            draws: Vec::new(),
        }
    }

    /// Whether cards fade against the scene prepass's depth.
    pub(crate) fn depth_fade(&self) -> bool {
        self.depth_fade
    }

    /// The depth the family's scene group binds where cards read none.
    pub(crate) fn stand_in(&self) -> &wgpu::TextureView {
        &self.stand_in
    }

    /// Orders the frame's card draws back to front from `eye`, by the distance to the center of
    /// each draw's mesh bounds where its instance places them; ties keep the draw list's order.
    pub(crate) fn sort(&mut self, eye: Vec3, list: &DrawList, slots: &InstanceSlots, meshes: &[MeshRecord]) {
        let variant = SurfaceVariant::FOG_CARD.index();
        let distance = |slot: u32| {
            let center = slots
                .mesh(slot)
                .and_then(|mesh| meshes.get(mesh.index()))
                .map_or(Vec3::ZERO, |record| record.bounds.center);
            let transform = slots.transform(slot).unwrap_or(Mat4::IDENTITY);
            transform.transform_point3(center).distance(eye)
        };
        self.order.clear();
        for format in PositionFormat::ALL {
            let commands = &list.commands[crate::pipelines::format_index(format)][variant];
            for (index, command) in commands.iter().enumerate() {
                let draw = DrawRef::Static { format, index: index as u32 };
                self.order.push((distance(command.slot), self.order.len() as u32, draw));
            }
        }
        for (index, command) in list.deformed[variant].iter().enumerate() {
            let draw = DrawRef::Deformed { index: index as u32 };
            self.order.push((distance(command.slot), self.order.len() as u32, draw));
        }
        // Ties keep the draw list's order, so the order holds frame to frame; unstable, so sorting
        // never allocates.
        self.order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        self.draws.clear();
        self.draws.extend(self.order.iter().map(|(_, _, draw)| *draw));
    }

    /// The frame's card draws, back to front, and the cache of the scene group with the prepass's
    /// depth, for the main pass to draw them.
    pub(crate) fn split_for_record(&mut self) -> (&[DrawRef], CardGroupCache<'_>) {
        (&self.draws, CardGroupCache { groups: &mut self.depth_groups, last: &mut self.last_group })
    }
}

/// The fog card family's scene group with the scene prepass's depth ([`FogCards`]), made in the
/// main pass, where the depth texture's view is known.
#[derive(Debug)]
pub(crate) struct CardGroupCache<'a> {
    groups: &'a mut [Option<DepthGroup>; 2],
    last: &'a mut usize,
}

impl<'a> CardGroupCache<'a> {
    /// The group binding `instances`, the family's `materials` and `depth` (the scene prepass's),
    /// in `layout`: one of the two made last, unless none binds them, which replaces the older.
    pub(crate) fn get(
        self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        instances: &wgpu::Buffer,
        materials: &wgpu::Buffer,
        depth: &wgpu::TextureView,
    ) -> &'a wgpu::BindGroup {
        let holds = |group: &Option<DepthGroup>| {
            group.as_ref().is_some_and(|group| {
                group.depth == *depth && group.instances == *instances && group.materials == *materials
            })
        };
        let hit = [*self.last, 1 - *self.last].into_iter().find(|&index| holds(&self.groups[index]));
        let index = hit.unwrap_or(1 - *self.last);
        if hit.is_none() {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fog card depth"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: instances.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: materials.as_entire_binding() },
                    wgpu::BindGroupEntry {
                        binding: FOG_CARD_DEPTH_BINDING,
                        resource: wgpu::BindingResource::TextureView(depth),
                    },
                ],
            });
            self.groups[index] = Some(DepthGroup {
                depth: depth.clone(),
                instances: instances.clone(),
                materials: materials.clone(),
                bind_group,
            });
        }
        *self.last = index;
        let groups: &'a [Option<DepthGroup>; 2] = self.groups;
        &groups[index].as_ref().expect("made above").bind_group
    }
}

/// What the fog drew in the last frame (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FogStats {
    /// Fog layers with a density in the last prepared frame: near, far and height, 0 to 3. With
    /// none, no pass evaluates the fog.
    pub layers: u32,
    /// Fog card draws in the last prepared frame (of `RenderStats::transparent_draws`).
    pub cards: u32,
    /// Fog card pipelines built, across position formats.
    pub card_pipelines: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FogLayer, HeightFog};

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
        let gpu = GpuFog::new(&bad);
        assert_eq!(gpu.near, [0.0; 4]);
        assert_eq!(gpu.far[3], 0.0);
        assert_eq!(gpu.shape, [0.0; 4]);
        assert_eq!(gpu.light, [0.0; 4]);
        assert_eq!(gpu.layers(), 1);
        assert!(gpu.is_visible());
    }
}
