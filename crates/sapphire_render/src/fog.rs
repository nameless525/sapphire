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
pub use sapphire_look::fog::{
    CEILING, FogRay, HORIZON_WEIGHT, MAX_EXPONENT, MIN_DENSITY, MIX_DEPTH, SERIES_BELOW, SKY_DISTANCE,
};
use sapphire_rhi::wgpu;

use crate::Fog;
use crate::draw::DrawRef;
use crate::mesh_pool::MeshRecord;
use crate::pipelines::{DEPTH_FORMAT, FOG_CARD_DEPTH_BINDING, SurfaceVariant};
use crate::scene::{DrawList, InstanceSlots};

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
    /// `fog`, made safe for the shaders ([`Fog::sanitized`]: non-finite or negative densities,
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
