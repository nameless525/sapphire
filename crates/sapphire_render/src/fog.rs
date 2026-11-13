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
//! - **The CPU reference:** [`Fog::ray`] evaluates the same function, for tests and tools. It
//!   lives beside the fog's data in `sapphire_look`, with its constants, so tools have it
//!   without linking the renderer.
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
//! Cards draw after every other transparent surface, sorted back to front, without writing depth;
//! they cast no shadow and have no outline.
//!
//! - **Cards the camera turns sort by depth along the view** (review REN-001): off to the side, a
//!   nearer one can be the farther from the eye. A card turned whole lies in the plane through its
//!   instance's origin, at that origin's depth, wherever its mesh's middle lies. Upright ones lie in
//!   planes parallel to each other, whatever the pitch, each holding its origin's vertical line, so
//!   they compare exactly where those lines meet the eye's height.
//! - **World-placed cards sort by their meshes' middles' distance from the eye**, which is a guess:
//!   two that aren't parallel have no one order across the screen, and no one key can hold it. It's
//!   the guess for sheets that face the eye, as the look-dev valley's rings of mist do, and turning
//!   the camera can't change it, where depth along the view would reorder sheets that haven't moved.
//!   Between a card of each kind, a depth meets a distance, a guess too.
//!
//! - **Decision: the depth they fade against is the scene prepass's**, where the frame has it:
//!   the desktop tiers, whose temporal AA runs the prepass anyway, so it costs nothing more. The
//!   main pass reads it as a texture while it writes its own depth (under temporal AA they are
//!   different textures). Phones draw 4× MSAA in tile memory, and reading the main pass's depth
//!   there would cost a store and a resolve (and on Mali its early-Z), so there cards fade near
//!   the lens alone and their maps' soft edges carry the rest: artists keep mobile cards clear of
//!   geometry or paint their edges where they meet it. Mobile-High does run the scene prepass,
//!   for GTAO (M1 W5), but the thermal ladder can shed GTAO and the prepass with it, and the
//!   depth fade follows temporal AA (`FogCards::new`), so cards there don't read it.
//!
//! # Later milestones
//!
//! - **Volumetric fog** on desktop (froxels lit by the shadow cascades and local lights, the
//!   quality tiers' `volumetric_fog`), with light shafts, over this analytic fog.
//! - **Fog density volumes** per zone (beyond time of day's zone overrides of the whole fog),
//!   and cards lit by local lights and probes.
//! - **One sort for every transparent surface**, with draw sorting (the crate docs); cards sort
//!   among themselves today.
//! - **Soft cards on Mobile-High,** from the scene prepass GTAO already runs there, with the
//!   lens fade alone while the ladder has shed it.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sapphire_assets::material::{CardFacing, FogCardParams};
use sapphire_assets::mesh::PositionFormat;
pub use sapphire_look::fog::{
    CEILING, FogRay, HORIZON_WEIGHT, MAX_EXPONENT, MIN_DENSITY, MIX_DEPTH, SERIES_BELOW, SKY_DISTANCE,
};
use sapphire_rhi::wgpu;

use crate::draw::DrawRef;
use crate::materials::Materials;
use crate::mesh_pool::MeshRecord;
use crate::pipelines::{DEPTH_FORMAT, FOG_CARD_DEPTH_BINDING, SurfaceVariant};
use crate::scene::{DrawList, InstanceSlots};
use crate::{CameraData, Fog};

#[cfg(test)]
mod gpu_tests;

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
    /// `fog`, sanitized before scalar range admission ([`Fog::sanitized`]: non-finite or negative densities,
    /// colors, starts and falloff are 0, a non-finite base 0, and the characters' share within
    /// 0..=1), which [`Fog::ray`] evaluates on the CPU.
    pub(crate) fn new(fog: &Fog) -> Self {
        let fog = fog.sanitized();
        let layer = |rgb: Vec3, density: f32| [rgb.x, rgb.y, rgb.z, density];
        let inscatter = fog.sun_inscatter;
        Self {
            near: layer(fog.near.color, fog.near.density),
            far: layer(fog.far.color, fog.far.density),
            height: layer(fog.height.color, fog.height.density),
            shape: [fog.near.start, fog.far.start, fog.height.falloff, fog.height.base],
            light: [inscatter.x, inscatter.y, inscatter.z, fog.characters],
        }
    }

    /// Layers with a density: the stats' count.
    pub(crate) fn layers(&self) -> u32 {
        [self.near[3], self.far[3], self.height[3]].iter().filter(|density| **density > 0.0).count() as u32
    }
}

/// A finite squared length in the world shaders gives a distance below 2^64 meters.
const MAX_RAY_DISTANCE: f64 = (1_u128 << 64) as f64;
/// Headroom for rounded f32 products and sums in the scalar fog shader.
const FOG_ARITHMETIC_LIMIT: f64 = f32::MAX as f64 / 8.0;
/// The maximum scene color in the renderer's Rgba16Float HDR target.
const MAX_FOG_SCENE_LIGHT: f64 = 65_504.0;
/// Room for the f32 rounded products, weighted sum and normalization of a fog color.
const FOG_LIGHT_ROUNDING: f64 = 1.0 + 32.0 * f32::EPSILON as f64;

/// The scalar fog shader's inputs for one frame and a bound on its card lighting.
#[derive(Clone, Copy)]
pub(crate) struct FogAdmission {
    pub(crate) uniforms: GpuFog,
    pub(crate) profile_issue: Option<&'static str>,
    light_bound: Option<[f64; 3]>,
}

impl FogAdmission {
    pub(crate) fn new(fog: &Fog, horizon: Vec3, eye_y: f32, cube_format: wgpu::TextureFormat) -> Self {
        let uniforms = GpuFog::new(fog);
        let target_limit = if cube_format == wgpu::TextureFormat::Rg11b10Ufloat {
            // With six or five fraction bits and the top finite exponent, the largest
            // values are (2 - 1/64) * 2^15 and (2 - 1/32) * 2^15, respectively.
            [65_024.0, 65_024.0, 64_512.0]
        } else {
            debug_assert_eq!(cube_format, wgpu::TextureFormat::Rgba16Float);
            [MAX_FOG_SCENE_LIGHT; 3]
        };
        match fog_light_bound(uniforms, horizon, eye_y, target_limit) {
            Ok(light_bound) => Self { uniforms, profile_issue: None, light_bound: Some(light_bound) },
            Err(reason) => {
                // Preserve the card sun lobe and the horizon for a clear atmosphere. The
                // card check below still guards their sum and the final tinted light.
                let clear = GpuFog { light: uniforms.light, ..GpuFog::default() };
                Self {
                    uniforms: clear,
                    profile_issue: Some(reason),
                    light_bound: fog_light_bound(clear, horizon, eye_y, target_limit).ok(),
                }
            }
        }
    }

    fn allows_card(&self, params: &FogCardParams) -> bool {
        let Some(light) = self.light_bound else { return false };
        let scatter = f64::from(params.fade[3]);
        (0.0..=1.0).contains(&scatter)
            && (0..3).all(|i| {
                let sun = f64::from(self.uniforms.light[i]);
                let tint = f64::from(params.color[i]).abs();
                let lit = light[i] + scatter * sun;
                // The shader adds light and scatter before it applies tint. Check both
                // the intermediate and the final color, per channel, so a tiny tint
                // can still safely use the full finite range of a clear-frame sun.
                lit <= f64::from(f32::MAX)
                    && sun * tint <= f64::from(f32::MAX)
                    && lit * tint * FOG_LIGHT_ROUNDING <= MAX_FOG_SCENE_LIGHT
            })
    }
}

fn fog_light_bound(
    fog: GpuFog,
    horizon: Vec3,
    eye_y: f32,
    target_limit: [f64; 3],
) -> Result<[f64; 3], &'static str> {
    if !horizon.is_finite() || !eye_y.is_finite() {
        return Err("fog needs a finite view height and horizon");
    }
    let falloff = f64::from(fog.shape[2]);
    let base = f64::from(fog.shape[3]);
    let mut height_start = 1.0_f64;
    // The main view and the sky cube's origin both consume the same profile.
    for eye in [0.0, f64::from(eye_y)] {
        let delta = eye - base;
        let exponent = -falloff * delta;
        let exponent_products = falloff * (eye.abs() + base.abs());
        if delta.abs() > f64::from(f32::MAX) || exponent_products > FOG_ARITHMETIC_LIMIT {
            return Err("height exponent exceeds the scalar fog range");
        }
        // Allow for cancellation if the compiler multiplies eye and base separately.
        let upper_exponent = exponent + 4.0 * f64::from(f32::EPSILON) * exponent_products;
        if upper_exponent > 0.0 {
            height_start = height_start.max(2.0 * upper_exponent.min(f64::from(MAX_EXPONENT)).exp());
        }
    }
    if 2.0 * falloff * MAX_RAY_DISTANCE > FOG_ARITHMETIC_LIMIT {
        return Err("height falloff exceeds the scalar ray range");
    }
    // Bound the integral's numerator, reciprocal and their product, so multiplying
    // either factor into another height term also fits. exp(30) < 2^44, the reciprocal
    // is at most 1000 outside the small-x series, and the ratio peaks at x = -30.
    let integral_factor = if falloff == 0.0 { 1.0 } else { (1_u64 << 44) as f64 };
    let near = f64::from(fog.near[3]);
    let far = f64::from(fog.far[3]);
    let height = f64::from(fog.height[3]);
    let height_factor = height.max(1.0) * height_start * integral_factor;
    let optical_depth = (near + far + height_factor) * MAX_RAY_DISTANCE;
    if optical_depth > FOG_ARITHMETIC_LIMIT {
        return Err("optical depth exceeds the scalar ray range");
    }
    let horizon = horizon.to_array().map(|x| f64::from(x).abs());
    // Both color-mix paths use a segment no longer than the stretch. Max with 1
    // also bounds subsets of the products after inlining or reassociation.
    let mix_length = 2.0 * f64::from(MIX_DEPTH) / f64::from(MIN_DENSITY);
    let mut light = horizon;
    let volume = fog.layers() != 0;
    for i in 0..3 {
        let near_color = f64::from(fog.near[i]);
        let far_tint = f64::from(fog.far[i]);
        let height_color = f64::from(fog.height[i]);
        if volume {
            // The weighted average cannot exceed its brightest input. The shader mixes
            // over at most MIX_DEPTH / density at the eye, so its total mixed weight
            // stays near MIX_DEPTH * ratio. The scene light bound also keeps this
            // unnormalized numerator well inside f32.
            let mut bound = horizon[i];
            if near > 0.0 {
                bound = bound.max(near_color);
            }
            if far > 0.0 {
                bound = bound.max(far_tint * horizon[i]);
            }
            if height > 0.0 {
                bound = bound.max(height_color);
            }
            light[i] = bound * FOG_LIGHT_ROUNDING;
            if (light[i] + f64::from(fog.light[i])) * FOG_LIGHT_ROUNDING > target_limit[i] {
                return Err("volume light and sun scatter exceed the HDR target range");
            }
        } else {
            light[i] *= FOG_LIGHT_ROUNDING;
            if light[i] > f64::from(f32::MAX) {
                return Err("clear fog horizon exceeds the scalar card light range");
            }
        }
        if mix_length
            * (near_color.max(1.0) * near.max(1.0)
                + far_tint.max(1.0) * horizon[i].max(1.0) * far.max(1.0)
                + height_color.max(1.0) * height_factor)
            > FOG_ARITHMETIC_LIMIT
        {
            return Err("fog color products exceed the scalar mix range");
        }
    }
    Ok(light)
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
    /// The scene groups with the prepass's depth, the two used last (cards read it only under
    /// temporal AA, where the prepass's depth is the history pair's, alternating frame by frame),
    /// made again only when the depth texture or the buffers they bind change, which steady frames
    /// never do. Incompatible entries go at the first miss; an empty card list releases both.
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

    /// A frame without a view has no readers that can reuse retained depth bindings.
    pub(crate) fn no_view(&mut self) {
        self.draws.clear();
        self.depth_groups = [None, None];
    }

    /// Orders the frame's card draws back to front from `camera`, each by how far it lies as its
    /// facing has it (the fog cards docs above have why); ties keep the draw list's order.
    pub(crate) fn sort(
        &mut self,
        camera: &CameraData,
        list: &DrawList,
        slots: &InstanceSlots,
        meshes: &[MeshRecord],
        materials: &Materials,
        fog: &FogAdmission,
    ) -> u32 {
        let variant = SurfaceVariant::FOG_CARD.index();
        let depth = |slot: u32, facing: u32| {
            let transform = slots.transform(slot).unwrap_or(Mat4::IDENTITY);
            let origin = transform.w_axis.truncate();
            if facing == CardFacing::Camera.code() {
                camera.view_depth(origin)
            } else if facing == CardFacing::Upright.code() {
                camera.view_depth(origin.with_y(camera.position.y))
            } else {
                let center = slots
                    .mesh(slot)
                    .and_then(|mesh| meshes.get(mesh.index()))
                    .map_or(Vec3::ZERO, |record| record.bounds.center);
                transform.transform_point3(center).distance(camera.position)
            }
        };
        self.order.clear();
        let mut unsupported = 0;
        let mut push = |slot, material, draw| {
            if let Some(params) = materials.fog_card_params(material).filter(|p| fog.allows_card(p)) {
                self.order.push((depth(slot, params.modes[1]), self.order.len() as u32, draw));
            } else {
                unsupported += 1;
            }
        };
        for format in PositionFormat::ALL {
            let commands = &list.commands[crate::pipelines::format_index(format)][variant];
            for (index, command) in commands.iter().enumerate() {
                let draw = DrawRef::Static { format, index: index as u32 };
                push(command.slot, command.material, draw);
            }
        }
        for (index, command) in list.deformed[variant].iter().enumerate() {
            let draw = DrawRef::Deformed { index: index as u32 };
            push(command.slot, command.material, draw);
        }
        // Ties keep the draw list's order, so the order holds frame to frame; unstable, so sorting
        // never allocates.
        self.order.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        self.draws.clear();
        self.draws.extend(self.order.iter().map(|(_, _, draw)| *draw));
        if self.draws.is_empty() {
            self.depth_groups = [None, None];
        }
        unsupported
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
        for group in self.groups.iter_mut() {
            if group.as_ref().is_some_and(|group| {
                group.depth.texture().size() != depth.texture().size()
                    || group.instances != *instances
                    || group.materials != *materials
            }) {
                *group = None;
            }
        }
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
    /// The frame's fog exceeded the supported GPU arithmetic range. Its atmosphere draws clear.
    pub unsupported_profile: bool,
    /// Cards omitted because their fog light could exceed the scalar or HDR scene range.
    pub unsupported_cards: u32,
    /// Fog card pipelines built, across position formats.
    pub card_pipelines: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FogLayer, HeightFog};

    #[test]
    fn ordinary_fog_upload_is_exact_and_extreme_profiles_are_explicitly_refused() {
        let ordinary = Fog {
            near: FogLayer { color: Vec3::splat(0.4), density: 0.03, start: 1.0 },
            height: HeightFog { color: Vec3::splat(0.2), density: 0.01, falloff: 0.02, base: 5.0 },
            sun_inscatter: Vec3::splat(0.1),
            characters: 0.5,
            ..Fog::default()
        };
        let horizon = Vec3::splat(0.3);
        let supported = FogAdmission::new(&ordinary, horizon, 2.0, wgpu::TextureFormat::Rgba16Float);
        assert_eq!(supported.profile_issue, None);
        assert_eq!(supported.uniforms, GpuFog::new(&ordinary));
        let too_dense = Fog {
            near: FogLayer { density: f32::MAX, ..ordinary.near },
            characters: f32::from_bits(1),
            ..ordinary
        };
        let delta =
            Fog { height: HeightFog { base: -f32::MAX, falloff: 0.0, ..ordinary.height }, ..ordinary };
        let too_bright = Fog {
            near: FogLayer { color: Vec3::splat(0.75 * f32::MAX), density: 0.125, start: 0.0 },
            sun_inscatter: Vec3::splat(0.75 * f32::MAX),
            ..Fog::default()
        };
        for (fog, eye, reason) in [
            (too_dense, 2.0, "optical depth exceeds the scalar ray range"),
            (delta, f32::MAX, "height exponent exceeds the scalar fog range"),
            (too_bright, 0.0, "volume light and sun scatter exceed the HDR target range"),
        ] {
            let refused = FogAdmission::new(&fog, horizon, eye, wgpu::TextureFormat::Rgba16Float);
            assert_eq!(refused.profile_issue, Some(reason));
            assert_eq!(refused.uniforms.layers(), 0);
            assert_eq!(refused.uniforms.light, GpuFog::new(&fog).light);
        }
    }

    #[test]
    fn clear_cards_admit_tiny_tint_of_hdr_sun_and_refuse_unsafe_composition() {
        let fog = Fog { sun_inscatter: Vec3::splat(f32::MAX), ..Fog::default() };
        let admission = FogAdmission::new(&fog, Vec3::ZERO, 0.0, wgpu::TextureFormat::Rg11b10Ufloat);
        assert_eq!(admission.profile_issue, None);
        let mut params = sapphire_assets::material::FogCardMaterial {
            color: [f32::from_bits(256), f32::from_bits(256), 0.0, 1.0],
            sun_scatter: 1.0,
            ..sapphire_assets::material::FogCardMaterial::default()
        }
        .gpu_params(&[sapphire_assets::texture::Swizzle::IDENTITY]);
        assert!(admission.allows_card(&params));
        params.color[0] = 1.0;
        assert!(!admission.allows_card(&params));
    }

    #[test]
    fn fog_admission_accounts_for_packed_cube_lanes_and_far_mix_products() {
        let admit = |color, format| {
            let fog = Fog { near: FogLayer { color, density: 0.001, start: 0.0 }, ..Fog::default() };
            FogAdmission::new(&fog, Vec3::ZERO, 0.0, format)
        };
        let packed = wgpu::TextureFormat::Rg11b10Ufloat;
        let half = wgpu::TextureFormat::Rgba16Float;
        assert!(admit(Vec3::splat(65_500.0), half).profile_issue.is_none());
        assert!(admit(Vec3::splat(65_500.0), packed).profile_issue.is_some());
        assert!(admit(Vec3::new(64_750.0, 64_750.0, 64_000.0), packed).profile_issue.is_none());
        assert!(admit(Vec3::new(64_000.0, 64_000.0, 64_750.0), packed).profile_issue.is_some());

        let fog = Fog {
            far: FogLayer { color: Vec3::splat(f32::MAX), density: 0.001, start: 0.0 },
            ..Fog::default()
        };
        let admitted = FogAdmission::new(&fog, Vec3::splat(1e-36), 0.0, half);
        assert_eq!(admitted.profile_issue, Some("fog color products exceed the scalar mix range"));
    }

    #[test]
    fn bad_values_reach_the_shaders_safe() {
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
        assert_eq!(gpu.layers(), 1, "the height layer's");
    }
}
