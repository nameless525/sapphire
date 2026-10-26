//! The sky (M1 W2; spec §5.12 "Sky", look pillars 1 and 3): the backdrop of every outdoor frame,
//! drawn in the anime way from one time-of-day key rather than a physical atmosphere.
//!
//! # Layers
//!
//! One fullscreen triangle in the main pass, at the far plane after the opaque surfaces and
//! outlines (depth test `Equal`, so only pixels no geometry covered shade), draws, back to front
//! (`shaders/sky.wgsl`):
//!
//! 1. **The gradient**, `sky_color()` in `frame.wgsl`, unchanged: the anime character shader
//!    reads it for ambient light too.
//! 2. **Stars**, procedural and fixed to the celestial sphere, which time of day turns with the
//!    hour ([`Celestial::sphere`](crate::Celestial)): two grids of cells on a cube around the
//!    sphere, each cell holding at most one star with a hashed position, brightness, color and
//!    twinkle. A star is a normalized Gaussian about a pixel wide, measured in pixels from the
//!    pixel's own footprint, so its summed light stays the same under sub-pixel camera motion
//!    and stars never shimmer; a grid whose cells shrink below a few pixels fades out. Optional
//!    maps tint the stars and set where they are dense, and add the Milky Way's glow.
//! 3. **Glows** around the sun and moon: compact, so a body far out of view adds exactly
//!    nothing, and stretched along the horizon while the body is low, as anime skies spread a
//!    setting sun's warmth along it.
//! 4. **The sun and moon discs**, in HDR so they bloom, cut by the horizon, the moon with an
//!    optional face map (whose alpha can paint a crescent). The moon hides the stars behind it.
//! 5. **Cirrus**, a high layer on a plane over the camera, scrolled by the wind.
//! 6. **Card clouds**, a band of painted cumulus cards around the horizon that drifts around it.
//!
//! Both cloud layers sample RGBA maps in the Genshin scheme: R the lit amount (the painted
//! terminator is its 0.5 line), G a painted rim, B a signed distance to the cloud's edge (0.5 on
//! the painted silhouette, rising inward) and A opacity. Coverage from time of day moves the
//! dissolve threshold along B, so clouds shrink to their cores and vanish instead of fading,
//! and both the silhouette and the terminator stay one pixel sharp at any magnification. Their
//! colors are authored per key: a lit and a shade tone, never a darkened albedo, and on the card
//! clouds a rim toward the key light (the cirrus's wisps are too thin for one: G goes unread).
//!
//! # Data
//!
//! - **Time of day** (`sapphire_world::tod`) resolves the gradient, where the bodies and the
//!   celestial sphere are, and [`SkyLayers`]: the discs' colors and glows, star brightness, and
//!   the clouds' colors and coverage, all blended in OkLab with the rest of the look.
//! - **The sky material** ([`FramePacket::sky_material`], [`SkyMaterial`]): the zone's disc sizes
//!   and softness, the moon's face, star twinkle, the star and Milky Way maps, and the two cloud
//!   maps with their scale, drift and dissolve softness. Without one the renderer draws with
//!   [`SkyMaterial::default`], uploaded at creation: default discs, even stars, no clouds.
//! - **Each frame** the CPU folds both into one uniform block ([`SkyUniforms`], group 2 of the
//!   sky's pipelines), reading the material's block from the material buffer's CPU mirror. Cloud
//!   drift is reduced there in `f64` from the packet's unwrapped time, so it never jumps; the
//!   GPU copy of the sky family's buffer goes unread.
//! - **Post's lens flare** (M1 P2) takes the sun's disc as the frame draws it from there
//!   (`Sky::sun_disc`: its direction, angular radius and radiance), to find it on screen.
//!
//! # The sky cube
//!
//! A cubemap of the sky alone, no geometry, for the probes and reflections that read it: the
//! ambient probe grid's sky term and the reflection cube's prefiltered mips (W5, the
//! [`ambient`](crate::ambient) module), and later reflection-probe and screen-space-reflection
//! misses (C7's reflection add). It holds the gradient, the glows and both cloud layers, and
//! leaves out the discs and the stars: the bodies are the key light, which shading adds
//! analytically, so a disc would add them twice and alias at probe resolution, and stars are far
//! smaller than its texels, so they would only cost (the night sky's light there is the
//! gradient's).
//!
//! - **Format:** `Rg11b10Ufloat` (4 bytes a texel, HDR to 65,000) where the device renders and
//!   filters it, else `Rgba16Float`. One level: readers prefilter (W5's relight and prefilter).
//! - **Size and rate per tier** ([`SkySettings`], `tiers.ron`): 256² on desktop and 128² on
//!   Mobile-High, a face every frame; 64² on Mobile-Low, a face every other frame (a full cube
//!   every 0.4 s at 30 fps). The first recorded frame draws all six faces, so the cube is never
//!   empty.
//! - **Faces** are WebGPU's (+X, −X, +Y, −Y, +Z, −Z), each oriented as a cube-map lookup reads
//!   it; the face pass draws the same shader through a view of the face (`cube_face_camera`).
//! - It is a persistent texture, outside the frame graph like the renderer's buffers: its pass
//!   is a side effect added before the main pass, so a reader added later sees this frame's face.
//!   Readers get it through [`Sky::cube_view`].
//!
//! # Cost
//!
//! The sky's budget is 0.1–0.3 ms of GPU time on every tier (M1 plan, W2). It shades most of the
//! screen, so its cost is arithmetic per pixel: the CPU works out every per-draw constant, edges
//! are linear ramps, the arctangent is a polynomial, and a star cell is hashed once before its
//! star's reach is known. Stars and clouds are pipeline variants (`Layers`), so a frame pays
//! nothing for a layer time of day isn't drawing; within one, every layer is a branch every pixel
//! takes alike, skipped at zero. Pixels also skip what can't reach them, leaving their color
//! exactly as drawing it would: under the horizon the discs, stars and glows (below the glows'
//! fringe) and both cloud layers (per 2 × 2 quad, which their texels' derivatives need), and a
//! cloud layer's shading where its texel covers nothing. Mobile-Low drops the cirrus layer. The
//! cube adds one face of at most 256² per frame, which costs about a render pass. Measured on an
//! Apple M5 in the sandbox's default scene (spec §5.12, "Sky"), cube included: 0.16 ms by day and
//! 0.2 ms at night at 1080p on Desktop-High, 0.09–0.12 ms with Mobile-High's settings and
//! 0.05–0.08 ms with Mobile-Low's. A sky filling the view above the horizon costs about 0.32 ms
//! at 1080p, the arithmetic of its layers (see "Later milestones").
//!
//! # Later milestones
//!
//! - **Weather** (time of day's later weather layer): coverage and cloud colors as partial
//!   overrides, and a rain or storm sky.
//! - **Moon phases** from the sun's direction, and sparkle shapes for the brightest stars.
//! - **Cloud shadows** on the world from the card clouds' coverage (R4, W4).
//! - **A sky filling the view** within 0.3 ms at 1080p. Above the horizon every layer is
//!   arithmetic on every pixel: on an M5 at night, about 0.2 ms of full-screen shading for the
//!   stars and the Milky Way, 0.15 ms for the two cloud layers and 0.07 ms for the fog. The
//!   candidates are the star grids' per-pixel cell lookup and shading the smooth layers (the
//!   Milky Way, the glows, the fog) at a lower rate.
//! - **A graph resource:** the cube, and the reflection cube its readers prefilter (W5), become
//!   frame graph resources once textures can outlive a frame in it.

use std::f32::consts::FRAC_PI_2;
use std::num::NonZeroU64;
use std::ops::Range;

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};
use sapphire_assets::material::{
    Material, MaterialFamily, SWIZZLE_BITS, SkyMaterial, SkyParams, swizzle_code,
};
use sapphire_assets::texture::Swizzle;
use sapphire_rhi::{GpuContext, wgpu};
use serde::{Deserialize, Serialize};

use crate::materials::{MaterialHandle, Materials};
use crate::pipelines::{DEPTH_FORMAT, Layouts, SCENE_COLOR_FORMAT};
use crate::shaders::{self, Program, ShaderSources};
use crate::{CameraData, DirectionalLight, FramePacket, LOG_TARGET, RenderError};

/// How much the sky costs on a quality tier (`tiers.ron`,
/// [`QualityTier`](crate::quality::QualityTier)): its cube's size and update rate, and whether it
/// draws the cirrus layer. The card clouds, stars and discs draw on every tier: they are the
/// look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(deny_unknown_fields)]
pub struct SkySettings {
    /// Texels along each edge of the sky cube's faces: a power of two in 16..=1024.
    pub cube_size: u32,
    /// Frames between face updates, 1..=60: 1 draws a face every frame, so the whole cube every
    /// six.
    pub cube_interval: u32,
    /// The high cirrus layer: one more texture sample per sky pixel.
    pub cirrus: bool,
}

impl Default for SkySettings {
    /// The desktop tiers' sky, which every golden image renders with.
    fn default() -> Self {
        Self { cube_size: 256, cube_interval: 1, cirrus: true }
    }
}

impl SkySettings {
    /// Checks the cube's size and interval, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if !(self.cube_size.is_power_of_two() && (16..=1024).contains(&self.cube_size)) {
            return Err(("sky.cube_size", "must be a power of two in 16..=1024"));
        }
        if !(1..=60).contains(&self.cube_interval) {
            return Err(("sky.cube_interval", "must be within 1..=60"));
        }
        Ok(())
    }
}

/// The sky's counters for the stats overlay (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkyStats {
    /// Texels along each edge of the cube's faces.
    pub cube_size: u32,
    /// Cube faces drawn since the renderer was created.
    pub cube_faces: u64,
    /// The face the last recorded frame drew, if it drew one (0–5: +X, −X, +Y, −Y, +Z, −Z).
    pub last_face: Option<u8>,
    /// What the last prepared frame drew: stars, card clouds and cirrus (each only while time of
    /// day gives it a brightness or coverage, and its map is bound).
    pub stars: bool,
    pub clouds: bool,
    pub cirrus: bool,
    /// Packets whose `sky_material` wasn't an uploaded sky material since the renderer was
    /// created; they draw with the default sky material.
    pub ignored_materials: u32,
}

/// The sky's group 2 (WGSL `SkyUniforms`): the frame's sky layers, the sky material's shapes and
/// the clouds' drift, folded together on the CPU. 320 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct SkyUniforms {
    /// xyz: unit vector toward the sun; w: its disc's angular radius (0: a dark disc, skipped).
    pub sun: [f32; 4],
    /// rgb: the disc's radiance; w: the width of its edge, in radians.
    pub sun_disc: [f32; 4],
    /// rgb: the glow at its center; w: the glow's angular radius (0: none).
    pub sun_glow: [f32; 4],
    pub moon: [f32; 4],
    pub moon_disc: [f32; 4],
    pub moon_glow: [f32; 4],
    /// The moon face's axes on the sky: unit vectors to the right of and up the disc, as the
    /// viewer sees it (up toward the zenith), for its map.
    pub moon_right: [f32; 4],
    pub moon_up: [f32; 4],
    /// The rotation from world directions into the celestial sphere's frame, by columns.
    pub celestial: [[f32; 4]; 3],
    /// x: star brightness; y: twinkle, 0 to 1.
    pub stars: [f32; 4],
    /// rgb: the clouds' lit tone; w: card cloud coverage (0 skips the layer).
    pub cloud_lit: [f32; 4],
    /// rgb: their shade tone; w: cirrus coverage (0 skips the layer).
    pub cloud_shade: [f32; 4],
    /// rgb: their rim.
    pub cloud_rim: [f32; 4],
    /// x: card cloud repeats around the horizon; y: dissolve softness; z: drift around the
    /// horizon, in repeats (0 to 1).
    pub clouds: [f32; 4],
    /// x: cirrus scale; y: dissolve softness; zw: drift, in repeats (0 to 1).
    pub cirrus: [f32; 4],
    /// x: the material's bound texture slots (`SkyParams::modes`); y: the slots whose swizzle is
    /// the identity, which the shader reads as sampled.
    pub modes: [u32; 4],
    /// Each texture slot's swizzle code (`SkyParams::swizzles`).
    pub swizzles: [u32; 4],
    /// The glows' shapes, sun then moon: x, z the inverse of the radius (0 without a glow); y, w
    /// how much a vertical offset counts, squared, less one, which spreads a low body's glow
    /// along the horizon.
    pub glow_shape: [f32; 4],
}

/// The sky pass's binding of [`SkyUniforms`] in group 2. Bindings 0 and 1 are the scene
/// group's instances and anime character materials, which `frame.wgsl` declares for every
/// scene pass; the sky reads neither, so its group 2 holds this binding alone.
const UNIFORMS_BINDING: u32 = 2;

/// A low body's glow spreads along the horizon, as anime skies spread a setting sun's warmth
/// along it: vertical offsets count up to this many times, fully once the body is at the horizon
/// and not at all from 14.5° above it.
const GLOW_HORIZON_STRETCH: f32 = 2.0;

/// The sky: its pipelines, uniforms, default material and cube.
#[derive(Debug)]
pub(crate) struct Sky {
    settings: SkySettings,
    uniforms: wgpu::Buffer,
    /// Group 2 of the sky's pipelines.
    bind_group: wgpu::BindGroup,
    /// Into the main pass's scene color, at its sample count, against its depth: one per
    /// [`Layers`] variant.
    pipelines: [wgpu::RenderPipeline; Layers::VARIANTS],
    /// Into a cube face: one sample, no depth, no discs or stars; without and with clouds.
    cube_pipelines: [wgpu::RenderPipeline; 2],
    /// What the pipelines are built with besides their module, kept to build them again from new
    /// shader sources (hot reload, [`Self::rebuilt_pipelines`]).
    layout: wgpu::PipelineLayout,
    samples: u32,
    cache: Option<wgpu::PipelineCache>,
    /// [`SkyMaterial::default`], for packets without a sky material of their own.
    default_material: MaterialHandle,
    cube: SkyCube,
    /// What the last `prepare` computed for `record`.
    prepared: Option<PreparedSky>,
    stats: SkyStats,
}

#[derive(Clone, Copy, Debug)]
struct PreparedSky {
    uniforms: SkyUniforms,
    material: MaterialHandle,
    layers: Layers,
}

/// Which of the sky's optional layers a frame draws, and so which pipeline variant draws it
/// (`SKY_STARS` and `SKY_CLOUDS` in sky.wgsl): a frame compiles nothing it doesn't draw, and the
/// plainest sky keeps the plainest shader's registers. The discs and glows are in every variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Layers {
    /// Stars and the Milky Way: time of day gives them a brightness.
    stars: bool,
    /// Either cloud layer: one has a coverage and a map.
    clouds: bool,
}

impl Layers {
    const VARIANTS: usize = 4;
    const ALL: [Self; Self::VARIANTS] = [
        Self { stars: false, clouds: false },
        Self { stars: true, clouds: false },
        Self { stars: false, clouds: true },
        Self { stars: true, clouds: true },
    ];

    /// The layers `uniforms` draw.
    fn of(uniforms: &SkyUniforms) -> Self {
        Self {
            stars: uniforms.stars[0] > 0.0,
            clouds: uniforms.cloud_lit[3] > 0.0 || uniforms.cloud_shade[3] > 0.0,
        }
    }

    /// The variant's index in [`Self::ALL`].
    fn index(self) -> usize {
        usize::from(self.stars) | usize::from(self.clouds) << 1
    }
}

/// The sky's pipelines from `module`: every [`Layers`] variant into the main pass's scene color
/// at `samples` against its depth, and the cube's two into `cube_format`.
fn build_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    samples: u32,
    cube_format: wgpu::TextureFormat,
    cache: Option<&wgpu::PipelineCache>,
) -> SkyPipelines {
    let pipeline =
        |label, format, samples, depth: Option<wgpu::DepthStencilState>, discs: bool, layers: Layers| {
            let switch = |on: bool| if on { 1.0 } else { 0.0 };
            let constants = [
                (DISCS_CONSTANT, switch(discs)),
                (STARS_CONSTANT, switch(layers.stars)),
                (CLOUDS_CONSTANT, switch(layers.clouds)),
            ];
            let compilation_options =
                wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_sky"),
                    compilation_options: compilation_options.clone(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth,
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_sky"),
                    compilation_options,
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache,
            })
        };
    // A fullscreen triangle at the far plane, drawn after the opaque surfaces: `Equal`
    // against the cleared depth (0) shades only pixels no geometry covered. Every variant is
    // built now, behind the load, so night falling never compiles a pipeline mid-game.
    let depth = wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Equal),
        stencil: Default::default(),
        bias: Default::default(),
    };
    let main = Layers::ALL
        .map(|layers| pipeline("sky", SCENE_COLOR_FORMAT, samples, Some(depth.clone()), true, layers));
    let cube = [false, true]
        .map(|clouds| pipeline("sky cube", cube_format, 1, None, false, Layers { stars: false, clouds }));
    SkyPipelines { main, cube }
}

/// The sky-only cubemap and when each face is drawn.
#[derive(Debug)]
struct SkyCube {
    #[cfg_attr(not(test), expect(dead_code, reason = "its views keep it alive; tests read it back"))]
    texture: wgpu::Texture,
    /// One 2D view per face, to render into.
    faces: [wgpu::TextureView; 6],
    /// The whole cube, for readers.
    view: wgpu::TextureView,
    schedule: FaceSchedule,
}

/// Which cube faces each recorded frame draws: all six the first time, then one every
/// `interval` frames, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FaceSchedule {
    interval: u32,
    /// Frames recorded since the last face was drawn.
    waited: u32,
    next_face: u32,
    /// Whether every face has been drawn once.
    filled: bool,
}

impl FaceSchedule {
    fn new(interval: u32) -> Self {
        Self { interval: interval.max(1), waited: 0, next_face: 0, filled: false }
    }

    /// The faces the next recorded frame draws.
    fn faces_due(&mut self) -> Range<u32> {
        if !self.filled {
            self.filled = true;
            return 0..6;
        }
        self.waited += 1;
        if self.waited < self.interval {
            return 0..0;
        }
        self.waited = 0;
        let face = self.next_face;
        self.next_face = (face + 1) % 6;
        face..face + 1
    }
}

/// What a frame's sky needs from the renderer to record: borrowed for the frame graph's lifetime.
#[derive(Clone, Copy)]
pub(crate) struct SkyDraw<'a> {
    pipeline: &'a wgpu::RenderPipeline,
    bind_group: &'a wgpu::BindGroup,
    /// The sky material's group 3.
    material: &'a wgpu::BindGroup,
}

impl SkyDraw<'_> {
    /// Draws the sky into the main pass: groups 0 and 1 must be bound. Leaves group 2 and 3
    /// bound to the sky's, so mesh draws after it rebind theirs.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(self.pipeline);
        pass.set_bind_group(2, self.bind_group, &[]);
        pass.set_bind_group(3, self.material, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The cube faces a frame draws, and what drawing them needs.
pub(crate) struct CubePass<'a> {
    faces: Range<u32>,
    views: &'a [wgpu::TextureView; 6],
    pipeline: &'a wgpu::RenderPipeline,
    bind_group: &'a wgpu::BindGroup,
    material: &'a wgpu::BindGroup,
}

impl CubePass<'_> {
    /// The faces it draws, which the ambient pass then projects and prefilters (M1 W5).
    pub(crate) fn faces(&self) -> Range<u32> {
        self.faces.clone()
    }

    /// Records the faces, each its own render pass: `frame` and `view` are groups 0 and 1, the
    /// view group bound at `view_offset(face)` for each face's view.
    pub(crate) fn record(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::BindGroup,
        view: &wgpu::BindGroup,
        view_offset: impl Fn(u32) -> u32,
        labels: bool,
        mut timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        for face in self.faces.clone() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: labels.then_some("sky cube face"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.views[face as usize],
                    depth_slice: None,
                    resolve_target: None,
                    // The triangle covers the face; clearing is the cheapest load on tilers.
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                // Timed as one pass: the first face carries the graph pass's timestamps.
                timestamp_writes: timestamps.take(),
                ..Default::default()
            });
            pass.set_pipeline(self.pipeline);
            pass.set_bind_group(0, frame, &[]);
            pass.set_bind_group(1, view, &[view_offset(face)]);
            pass.set_bind_group(2, self.bind_group, &[]);
            pass.set_bind_group(3, self.material, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// The sky's pipelines from one module: [`Sky::rebuilt_pipelines`]' result.
#[derive(Debug)]
pub(crate) struct SkyPipelines {
    main: [wgpu::RenderPipeline; Layers::VARIANTS],
    cube: [wgpu::RenderPipeline; 2],
}

impl SkyPipelines {
    /// How many there are: every one is built up front.
    pub(crate) const COUNT: u32 = (Layers::VARIANTS + 2) as u32;
}

impl Sky {
    /// The sky's pipelines (for the main pass's `samples`, built from `sources`), its uniforms
    /// and cube, and the default sky material, uploaded into `materials`.
    pub(crate) fn new(
        gpu: &GpuContext,
        layouts: &Layouts,
        materials: &mut Materials,
        settings: SkySettings,
        samples: u32,
        sources: &ShaderSources,
    ) -> Result<Self, RenderError> {
        let device = gpu.device();
        let defaults = Material::from_document(
            &SkyMaterial::default(),
            &[Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()],
        )?
        .to_cooked()
        .map_err(|error| RenderError::InvalidMaterial(error.into()))?;
        let (default_material, _) =
            materials
                .upload(device, gpu.queue(), &layouts.materials, Material::read(&defaults)?, |_| None)?;

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky uniforms"),
            size: size_of::<SkyUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: UNIFORMS_BINDING,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(size_of::<SkyUniforms>() as u64),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: UNIFORMS_BINDING,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[
                Some(&layouts.frame),
                Some(&layouts.view),
                Some(&group_layout),
                Some(&layouts.materials[MaterialFamily::Sky.index()]),
            ],
            immediate_size: 0,
        });
        let module = shaders::module(device, Program::Sky, sources);
        let cache = gpu.pipeline_cache().cloned();
        let cube = SkyCube::new(gpu, settings);
        let SkyPipelines { main: pipelines, cube: cube_pipelines } =
            build_pipelines(device, &layout, &module, samples, cube.format(), cache.as_ref());
        tracing::debug!(
            target: LOG_TARGET,
            cube_size = settings.cube_size,
            cube_format = ?cube.format(),
            cube_interval = settings.cube_interval,
            cirrus = settings.cirrus,
            "sky created"
        );
        Ok(Self {
            settings,
            uniforms,
            bind_group,
            pipelines,
            cube_pipelines,
            layout,
            samples,
            cache,
            default_material,
            cube,
            prepared: None,
            stats: SkyStats { cube_size: settings.cube_size, ..SkyStats::default() },
        })
    }

    /// The sky's pipelines built anew from `sources` (hot reload), on the layout, sample count
    /// and cube format they were built with: [`Self::set_pipelines`] takes them once the
    /// renderer knows every pipeline it rebuilt is valid.
    pub(crate) fn rebuilt_pipelines(&self, device: &wgpu::Device, sources: &ShaderSources) -> SkyPipelines {
        let module = shaders::module(device, Program::Sky, sources);
        build_pipelines(device, &self.layout, &module, self.samples, self.cube.format(), self.cache.as_ref())
    }

    /// Draws with `pipelines` from the next frame, which draws every face of the cube again, as
    /// the first frame does, so the cube shows the new shaders at once.
    pub(crate) fn set_pipelines(&mut self, pipelines: SkyPipelines) {
        self.pipelines = pipelines.main;
        self.cube_pipelines = pipelines.cube;
        self.cube.schedule = FaceSchedule::new(self.settings.cube_interval);
    }

    /// The CPU half of a frame: folds the packet's sky layers and its sky material (or the
    /// default) into the frame's uniforms.
    pub(crate) fn prepare(&mut self, packet: &FramePacket, materials: &Materials) {
        let requested = packet.sky_material;
        let material = requested
            .filter(|&handle| materials.record(handle).is_some_and(|r| r.family == MaterialFamily::Sky))
            .unwrap_or(self.default_material);
        if requested.is_some_and(|handle| handle != material) {
            self.stats.ignored_materials += 1;
        }
        let params: SkyParams = bytemuck::pod_read_unaligned(
            materials.params(material).expect("a sky material's block is in its family's mirror"),
        );
        let uniforms = SkyUniforms::new(packet, &params, self.settings);
        self.stats.stars = uniforms.stars[0] > 0.0;
        self.stats.clouds = uniforms.cloud_lit[3] > 0.0;
        self.stats.cirrus = uniforms.cloud_shade[3] > 0.0;
        self.prepared = Some(PreparedSky { uniforms, material, layers: Layers::of(&uniforms) });
    }

    /// The prepared frame's uniforms, the main pass's draw, and the cube faces this frame
    /// updates (none unless one is due), for [`Renderer::record`](crate::Renderer::record).
    /// Without a prepared frame (a bug the renderer's own `record` catches first), it draws the
    /// default sky with every layer off.
    pub(crate) fn record<'a>(
        &'a mut self,
        materials: &'a [crate::materials::MaterialRecord],
    ) -> (SkyUniforms, &'a wgpu::Buffer, SkyDraw<'a>, Option<CubePass<'a>>) {
        let prepared = self.prepared.take();
        let (uniforms, material, layers) = prepared
            .map_or((SkyUniforms::zeroed(), self.default_material, Layers::default()), |p| {
                (p.uniforms, p.material, p.layers)
            });
        let material = &materials[material.index()].bind_group;
        let faces = self.cube.schedule.faces_due();
        self.stats.last_face = None;
        if !faces.is_empty() {
            self.stats.cube_faces += u64::from(faces.len() as u32);
            self.stats.last_face = Some((faces.end - 1) as u8);
        }
        let draw =
            SkyDraw { pipeline: &self.pipelines[layers.index()], bind_group: &self.bind_group, material };
        let cube = (!faces.is_empty()).then_some(CubePass {
            faces,
            views: &self.cube.faces,
            pipeline: &self.cube_pipelines[usize::from(layers.clouds)],
            bind_group: &self.bind_group,
            material,
        });
        (uniforms, &self.uniforms, draw, cube)
    }

    /// The sun's disc as the prepared frame draws it, for post's lens flare: `None` before the
    /// first `prepare`.
    pub(crate) fn sun_disc(&self) -> Option<crate::post::SunDisc> {
        let SkyUniforms { sun, sun_disc, .. } = self.prepared.as_ref()?.uniforms;
        Some(crate::post::SunDisc {
            toward: Vec3::new(sun[0], sun[1], sun[2]),
            angular_radius: sun[3],
            radiance: Vec3::new(sun_disc[0], sun_disc[1], sun_disc[2]),
        })
    }

    /// The sky cube, for the probes and reflections that read it (see the module docs).
    pub(crate) fn cube_view(&self) -> &wgpu::TextureView {
        &self.cube.view
    }

    /// The cube texture, for tests that read its faces back.
    #[cfg(test)]
    pub(crate) fn cube_texture(&self) -> &wgpu::Texture {
        &self.cube.texture
    }

    pub(crate) fn stats(&self) -> SkyStats {
        self.stats
    }
}

/// The names of `sky.wgsl`'s pipeline-overridable constants that turn the discs, the stars and
/// the clouds on.
pub(crate) const DISCS_CONSTANT: &str = "SKY_DISCS";
pub(crate) const STARS_CONSTANT: &str = "SKY_STARS";
pub(crate) const CLOUDS_CONSTANT: &str = "SKY_CLOUDS";

/// Faces of the sky cube, each with a view of its own after the main view's in the view buffer.
pub(crate) const CUBE_FACES: u32 = 6;

impl SkyCube {
    fn new(gpu: &GpuContext, settings: SkySettings) -> Self {
        let compact = gpu.format_support(wgpu::TextureFormat::Rg11b10Ufloat);
        // Half of `Rgba16Float`'s memory and bandwidth, with range enough for the glows.
        let format = if compact.renderable && compact.filterable {
            wgpu::TextureFormat::Rg11b10Ufloat
        } else {
            SCENE_COLOR_FORMAT
        };
        let size = settings.cube_size;
        let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("sky cube"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let faces = std::array::from_fn(|face| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("sky cube face"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: face as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("sky cube"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        Self { texture, faces, view, schedule: FaceSchedule::new(settings.cube_interval) }
    }

    fn format(&self) -> wgpu::TextureFormat {
        self.faces[0].texture().format()
    }
}

/// The directions across, up and through cube face `face` (WebGPU's order: +X, −X, +Y, −Y, +Z,
/// −Z): a cube-map lookup reads the texel at `(s, t)` (each −1 to 1, `t` down the face) in
/// direction `forward + s·right − t·up`.
fn cube_face_axes(face: usize) -> [Vec3; 3] {
    match face {
        0 => [Vec3::NEG_Z, Vec3::Y, Vec3::X],
        1 => [Vec3::Z, Vec3::Y, Vec3::NEG_X],
        2 => [Vec3::X, Vec3::NEG_Z, Vec3::Y],
        3 => [Vec3::X, Vec3::Z, Vec3::NEG_Y],
        4 => [Vec3::X, Vec3::Y, Vec3::Z],
        _ => [Vec3::NEG_X, Vec3::Y, Vec3::NEG_Z],
    }
}

/// A camera at the origin seeing cube face `face` exactly as a cube-map lookup reads it: a 90°
/// square frustum whose pixel at `(x, y)` (from the top left) is the face's texel there. Its view
/// is mirrored (cube maps are left-handed), which only matters to geometry drawn through it;
/// the sky draws directions.
pub(crate) fn cube_face_camera(face: usize) -> CameraData {
    let [right, up, forward] = cube_face_axes(face);
    // View space looks down −Z with +X right and +Y up (NDC y up is the face's top row).
    let view = Mat4::from_mat3(Mat3::from_cols(right, up, -forward).transpose());
    let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(FRAC_PI_2, 1.0, 0.1);
    CameraData { view, proj, position: Vec3::ZERO }
}

/// How many times the card cloud band repeats around the horizon for a material `scale`: a whole
/// number, at least 1, so the band joins itself where the azimuth wraps.
fn band_repeats(scale: f32) -> f32 {
    scale.round().max(1.0)
}

/// Bit `i` set for each of the sky family's texture slots whose swizzle code (in a block's
/// `swizzles`, two to a word) is the identity's.
fn identity_swizzles(swizzles: &[u32; 4]) -> u32 {
    let identity = swizzle_code(Swizzle::IDENTITY);
    (0..SkyMaterial::TEXTURE_SLOTS.len()).fold(0, |bits, slot| {
        let code = (swizzles[slot / 2] >> (SWIZZLE_BITS * (slot % 2) as u32)) & ((1 << SWIZZLE_BITS) - 1);
        if code == identity { bits | 1 << slot } else { bits }
    })
}

/// `value` within 0..=1, and 0 when it isn't finite: what a branch on it may trust.
fn unit(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 }
}

/// Where `value × seconds` lands within one repeat, computed in `f64` so drift stays smooth for
/// as long as the session runs.
fn drift(speed: f32, seconds: f64) -> f32 {
    let repeats = f64::from(speed) * seconds;
    if repeats.is_finite() { repeats.rem_euclid(1.0) as f32 } else { 0.0 }
}

impl SkyUniforms {
    /// The block for `packet`'s sky drawn with `params` (a sky material's block) on a tier with
    /// `settings`.
    pub(crate) fn new(packet: &FramePacket, params: &SkyParams, settings: SkySettings) -> Self {
        let layers = &packet.sky_layers;
        let toward = |light: &DirectionalLight| (-light.direction).try_normalize().unwrap_or(Vec3::Y);
        let (sun, moon) = (toward(&packet.celestial.sun), toward(&packet.celestial.moon));
        let disc = |body: &crate::SkyDisc, size: f32, softness: f32| {
            let radiance = body.color.max(Vec3::ZERO) * body.intensity.max(0.0);
            // A disc without light, or one that isn't finite, draws nothing: radius 0 skips it.
            let radiance = if radiance.is_finite() { radiance } else { Vec3::ZERO };
            let radius = if radiance == Vec3::ZERO { 0.0 } else { size * 0.5 };
            (radiance.extend(size * softness).to_array(), radius)
        };
        let glow = |body: &crate::SkyDisc| {
            let radius = if body.glow_radius.is_finite() { body.glow_radius.max(0.0) } else { 0.0 };
            let color = body.glow.max(Vec3::ZERO);
            let color = if color.is_finite() && radius > 0.0 { color } else { Vec3::ZERO };
            color.extend(radius).to_array()
        };
        let shape = |body: Vec3, glow: &[f32; 4]| {
            let stretch =
                GLOW_HORIZON_STRETCH + (1.0 - GLOW_HORIZON_STRETCH) * (body.y * 4.0).clamp(0.0, 1.0);
            [if glow[3] > 0.0 { glow[3].recip() } else { 0.0 }, stretch * stretch - 1.0]
        };
        let [sun_size, sun_softness, moon_size, moon_softness] = params.discs;
        let (sun_disc, sun_radius) = disc(&layers.sun, sun_size, sun_softness);
        let (moon_disc, moon_radius) = disc(&layers.moon, moon_size, moon_softness);
        // The moon's face stands upright: up toward the zenith, or along −Z at the zenith itself.
        let moon_up = (Vec3::Y - moon * moon.y).try_normalize().unwrap_or(Vec3::NEG_Z);
        let moon_right = moon.cross(moon_up);
        let celestial = Mat3::from_quat(packet.celestial.sphere.normalize()).transpose();
        let celestial = if celestial.is_finite() { celestial } else { Mat3::IDENTITY };
        let bound = |slot: usize| params.modes[0] & (1 << slot) != 0;
        let seconds = packet.time_seconds;
        let clouds = &layers.clouds;
        let coverage = if bound(SkyMaterial::CLOUDS) { unit(clouds.coverage) } else { 0.0 };
        let cirrus = if bound(SkyMaterial::CIRRUS) && settings.cirrus { unit(clouds.cirrus) } else { 0.0 };
        let color = |rgb: Vec3, w: f32| {
            let rgb = rgb.max(Vec3::ZERO);
            (if rgb.is_finite() { rgb } else { Vec3::ZERO }).extend(w).to_array()
        };
        let stars = if layers.stars.is_finite() { layers.stars.max(0.0) } else { 0.0 };
        let (sun_glow, moon_glow) = (glow(&layers.sun), glow(&layers.moon));
        let ([sun_inverse, sun_stretch], [moon_inverse, moon_stretch]) =
            (shape(sun, &sun_glow), shape(moon, &moon_glow));
        Self {
            sun: sun.extend(sun_radius).to_array(),
            sun_disc,
            sun_glow,
            moon: moon.extend(moon_radius).to_array(),
            moon_disc,
            moon_glow,
            moon_right: moon_right.extend(0.0).to_array(),
            moon_up: moon_up.extend(0.0).to_array(),
            celestial: [
                celestial.x_axis.extend(0.0).to_array(),
                celestial.y_axis.extend(0.0).to_array(),
                celestial.z_axis.extend(0.0).to_array(),
            ],
            stars: [stars, unit(params.stars[0]), 0.0, 0.0],
            cloud_lit: color(clouds.lit, coverage),
            cloud_shade: color(clouds.shade, cirrus),
            cloud_rim: color(clouds.rim, 0.0),
            clouds: [band_repeats(params.clouds[0]), params.clouds[1], drift(params.clouds[2], seconds), 0.0],
            cirrus: [
                params.cirrus[0],
                params.cirrus[1],
                drift(params.cirrus[2], seconds),
                drift(params.cirrus[3], seconds),
            ],
            modes: [params.modes[0], identity_swizzles(&params.swizzles), 0, 0],
            swizzles: params.swizzles,
            glow_shape: [sun_inverse, sun_stretch, moon_inverse, moon_stretch],
        }
    }
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use glam::Quat;

    use super::*;
    use crate::{CloudLight, SkyDisc, SkyLayers};

    fn params(material: &SkyMaterial) -> SkyParams {
        material.gpu_params(&[Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()])
    }

    #[test]
    fn the_default_packet_draws_the_gradient_alone() {
        let uniforms = SkyUniforms::new(
            &FramePacket::default(),
            &params(&SkyMaterial::default()),
            SkySettings::default(),
        );
        assert_eq!(uniforms.sun_disc[..3], [0.0; 3]);
        assert_eq!(uniforms.moon_disc[..3], [0.0; 3]);
        assert_eq!((uniforms.sun[3], uniforms.moon[3]), (0.0, 0.0), "dark discs are skipped");
        assert_eq!((uniforms.sun_glow[3], uniforms.moon_glow[3]), (0.0, 0.0));
        assert_eq!(uniforms.stars[0], 0.0);
        assert_eq!((uniforms.cloud_lit[3], uniforms.cloud_shade[3]), (0.0, 0.0));
    }

    #[test]
    fn clouds_need_their_maps_and_the_tier() {
        let clouds = CloudLight { coverage: 0.7, cirrus: 2.0, ..CloudLight::default() };
        let mut packet = FramePacket {
            sky_layers: SkyLayers { clouds, ..SkyLayers::default() },
            ..FramePacket::default()
        };
        let bare = SkyUniforms::new(&packet, &params(&SkyMaterial::default()), SkySettings::default());
        assert_eq!((bare.cloud_lit[3], bare.cloud_shade[3]), (0.0, 0.0), "no maps, no clouds");
        let mut material = SkyMaterial::default();
        let map = Some(sapphire_assets::AssetId::from_text("00000000-0000-0000-0000-000000000001"));
        material.clouds.map = map;
        material.cirrus.map = map;
        let drawn = SkyUniforms::new(&packet, &params(&material), SkySettings::default());
        assert_eq!((drawn.cloud_lit[3], drawn.cloud_shade[3]), (0.7, 1.0), "coverage is clamped to 1");
        let low = SkySettings { cirrus: false, ..SkySettings::default() };
        assert_eq!(SkyUniforms::new(&packet, &params(&material), low).cloud_shade[3], 0.0);
        packet.sky_layers.clouds.coverage = f32::NAN;
        assert_eq!(SkyUniforms::new(&packet, &params(&material), low).cloud_lit[3], 0.0, "NaN is no cover");
    }

    #[test]
    fn identity_swizzles_are_marked_for_the_shader() {
        use sapphire_assets::texture::Component;
        let ra01 = Swizzle([Component::R, Component::A, Component::Zero, Component::One]);
        let mut swizzles = [Swizzle::IDENTITY; SkyMaterial::TEXTURE_SLOTS.len()];
        swizzles[SkyMaterial::CIRRUS] = ra01;
        let mut material = SkyMaterial::default();
        material.cirrus.map =
            Some(sapphire_assets::AssetId::from_text("00000000-0000-0000-0000-000000000002"));
        let block = material.gpu_params(&swizzles);
        assert_eq!(identity_swizzles(&block.swizzles), 0b01111, "every slot but the cirrus' two-channel map");
    }

    #[test]
    fn each_layer_set_has_its_variant() {
        let indices: Vec<usize> = Layers::ALL.iter().map(|layers| layers.index()).collect();
        assert_eq!(indices, [0, 1, 2, 3]);
        let mut uniforms = SkyUniforms::new(
            &FramePacket::default(),
            &params(&SkyMaterial::default()),
            SkySettings::default(),
        );
        assert_eq!(Layers::of(&uniforms), Layers::default(), "the gradient alone");
        uniforms.cloud_shade[3] = 0.4;
        assert_eq!(Layers::of(&uniforms), Layers { stars: false, clouds: true }, "cirrus alone draws clouds");
        uniforms.stars[0] = 0.1;
        assert_eq!(Layers::of(&uniforms), Layers { stars: true, clouds: true });
    }

    #[test]
    fn the_band_repeats_a_whole_number_of_times() {
        assert_eq!(band_repeats(1.0), 1.0);
        assert_eq!(band_repeats(2.4), 2.0);
        assert_eq!(band_repeats(0.2), 1.0, "at least once around");
    }

    #[test]
    fn drift_wraps_in_f64_and_never_jumps() {
        // An hour and a half of drift at 0.002 repeats a second, past where f32 time would round.
        let speed = 0.002;
        let at = |seconds: f64| drift(speed, seconds);
        assert_eq!(at(0.0), 0.0);
        assert!((at(250.0) - 0.5).abs() < 1e-6);
        let step = 1.0 / 60.0;
        let late = 5.0e6;
        let moved = (at(late + step) - at(late)).rem_euclid(1.0);
        assert!((moved - speed * step as f32).abs() < 1e-6, "a frame's drift late in a session: {moved}");
        assert_eq!(drift(speed, f64::NAN), 0.0);
        assert!((drift(-speed, 250.0) - 0.5).abs() < 1e-6, "negative speeds drift the other way");
    }

    #[test]
    fn discs_and_the_celestial_sphere_come_from_the_packet() {
        let mut packet = FramePacket::default();
        packet.celestial.sun.direction = Vec3::new(0.0, -1.0, -1.0);
        packet.celestial.moon.direction = Vec3::new(1.0, -0.2, 0.0);
        packet.celestial.sphere = Quat::from_rotation_y(0.5);
        packet.sky_layers.sun =
            SkyDisc { color: Vec3::ONE, intensity: 20.0, glow: Vec3::splat(0.5), glow_radius: 0.4 };
        let material = SkyMaterial::default();
        let uniforms = SkyUniforms::new(&packet, &params(&material), SkySettings::default());
        let sun = Vec3::from_slice(&uniforms.sun[..3]);
        assert!(sun.abs_diff_eq(Vec3::new(0.0, 1.0, 1.0).normalize(), 1e-6), "toward the sun");
        assert!((uniforms.sun[3] - material.sun.size.to_radians() * 0.5).abs() < 1e-7, "the disc's radius");
        assert_eq!(uniforms.sun_disc[..3], [20.0; 3]);
        assert_eq!(uniforms.sun_glow, [0.5, 0.5, 0.5, 0.4]);
        // 45° up, the sun's glow is round: its inverse radius, and no stretch.
        assert_eq!(uniforms.glow_shape[..2], [2.5, 0.0]);
        // The moon's face axes are perpendicular to it, up toward the zenith.
        let (moon, right, up) = (
            Vec3::from_slice(&uniforms.moon[..3]),
            Vec3::from_slice(&uniforms.moon_right[..3]),
            Vec3::from_slice(&uniforms.moon_up[..3]),
        );
        assert!(moon.dot(right).abs() < 1e-6 && moon.dot(up).abs() < 1e-6 && up.y > 0.0);
        assert!(right.cross(up).dot(moon) < 0.0, "seen from inside the sphere, right × up faces the viewer");
        // World to celestial undoes the sphere's turn.
        let celestial =
            Mat3::from_cols_array_2d(&uniforms.celestial.map(|column| [column[0], column[1], column[2]]));
        let star = Vec3::new(0.3, 0.5, 0.8).normalize();
        assert!((celestial * (packet.celestial.sphere * star)).abs_diff_eq(star, 1e-6));
    }

    #[test]
    fn cube_faces_match_webgpu_cube_lookups() {
        // The direction through each face's center and its top-left corner, as a lookup reads
        // them (the WebGPU / Vulkan cube-map selection rules).
        let expected = [
            (Vec3::X, Vec3::new(1.0, 1.0, 1.0)),
            (Vec3::NEG_X, Vec3::new(-1.0, 1.0, -1.0)),
            (Vec3::Y, Vec3::new(-1.0, 1.0, -1.0)),
            (Vec3::NEG_Y, Vec3::new(-1.0, -1.0, 1.0)),
            (Vec3::Z, Vec3::new(-1.0, 1.0, 1.0)),
            (Vec3::NEG_Z, Vec3::new(1.0, 1.0, -1.0)),
        ];
        for (face, (center, corner)) in expected.into_iter().enumerate() {
            let camera = cube_face_camera(face);
            let world_from_clip = (camera.proj * camera.view).inverse();
            let through = |x: f32, y: f32| {
                let near = world_from_clip * glam::Vec4::new(x, y, 1.0, 1.0);
                (near.truncate() / near.w).normalize()
            };
            assert!(through(0.0, 0.0).abs_diff_eq(center, 1e-5), "face {face} center");
            assert!(through(-1.0, 1.0).abs_diff_eq(corner.normalize(), 1e-5), "face {face} top left");
        }
    }

    #[test]
    fn faces_update_in_turn_after_a_full_first_frame() {
        let faces = |interval: u32, frames: usize| {
            let mut schedule = FaceSchedule::new(interval);
            (0..frames).map(|_| schedule.faces_due()).collect::<Vec<_>>()
        };
        assert_eq!(faces(1, 9), [0..6, 0..1, 1..2, 2..3, 3..4, 4..5, 5..6, 0..1, 1..2]);
        assert_eq!(faces(2, 6), [0..6, 0..0, 0..1, 0..0, 1..2, 0..0]);
    }

    #[test]
    fn settings_are_checked() {
        assert_eq!(SkySettings::default().validate(), Ok(()));
        let bad_size = SkySettings { cube_size: 100, ..SkySettings::default() };
        assert_eq!(bad_size.validate().unwrap_err().0, "sky.cube_size");
        let bad_interval = SkySettings { cube_interval: 0, ..SkySettings::default() };
        assert_eq!(bad_interval.validate().unwrap_err().0, "sky.cube_interval");
    }
}
