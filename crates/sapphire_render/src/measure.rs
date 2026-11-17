//! Measurement switches and counters for the anime character pixel (M6 B6, m6-plan §3's "Measurement
//! switches"): what prices the character work before it is built, and never a look anyone ships.
//!
//! - **Switches** ([`CharacterSwitches`]) each take one term of the character pixel away, or make it
//!   cheaper, so a bench can price it against the shipping pixel: the rim, the local lights' loop and a
//!   cap on it, the received character shadows past the first slot, the hull's shadow, the hull's light,
//!   the second ramp row at full night, the material reads' second level, the hair styles not drawn,
//!   `face_margin`'s maths and the control map. Each is a pipeline-overridable constant (`MEASURE_*` in
//!   `anime_character.wgsl`) whose default is the shipping pipeline's, so a switch at its default
//!   compiles the shipping program: a uniform branch around the same code moved six goldens by up to 13
//!   code values (M5 L4), and an override that is off when a pipeline is built leaves nothing to take.
//!   A switch that is off isn't passed at all ([`CharacterSwitches::constants`] is empty at the
//!   default), so every pipeline is built from exactly the constants it was built from before. They
//!   change constants only: no pipeline, key or variant is added or left out.
//! - **The renderer has no `dev` feature,** so the switches are a hidden settings field
//!   ([`RenderSettings::character_switches`](crate::RenderSettings::character_switches)), fixed for a
//!   renderer's life, which only a dev sandbox's flags set. Tiers never do.
//! - **They act on the anime character family's main-pass pipelines:** surfaces, outline hulls and eyes
//!   through hair, and the debug views' copies of them. The prepasses and the shadow casters don't shade
//!   and keep theirs.
//! - **Counters** ([`PixelCounter`], `measure.wgsl`): the main view's character pixels and hull pixels,
//!   and the edge upscaler's 32-pixel groups and the wholly flat ones among them, counted on the GPU and
//!   read back a few frames late ([`MeasuredPixels`]). Built only where
//!   [`RenderSettings::measure_pixels`](crate::RenderSettings::measure_pixels) asks, so no tier's frame
//!   has their passes. The character triangles by pass are [`PassCounts`](crate::PassCounts)' in every
//!   build.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use bytemuck::{Pod, Zeroable};
use sapphire_rhi::wgpu;

use crate::graph::{FrameGraph, TextureId};
use crate::pipeline_builds::{PipelineBuilds, PipelineFamily};
use crate::shader_builds::Step;
use crate::shaders::{self, Program, ShaderSources};

/// How the character pixel reads a material texture ([`CharacterSwitches::material_read`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaterialRead {
    /// The two whole levels either side of the texel's level, each read exactly and blended (the shipping
    /// read, `material_sample` in `materials.wgsl`), exact across a streamed texture's lowered levels.
    #[default]
    TwoLevels,
    /// One read at the fractional level, the hardware's trilinear blend: half the reads, and a code value
    /// or two off on a lowered texture (m6-plan P4).
    Fractional,
    /// One read at the nearest whole level: half the reads, exact across lowered levels, and a level that
    /// steps on a moving figure (m6-plan P4).
    Whole,
}

/// The measurement switches for the anime character pixel (M6 B6). The default is the shipping pixel: every
/// switch off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharacterSwitches {
    /// No depth-offset rim: its mask is never taken.
    pub no_rim: bool,
    /// No local lights on characters: their loop, and the derivatives it reads, are skipped.
    pub no_local_lights: bool,
    /// The most local lights a character pixel walks (m6-plan P6's sweep); `None` caps nothing.
    pub light_cap: Option<u32>,
    /// Another character's shadow from the first received slot alone, the one whose caster lies nearest
    /// along the light (m6-plan P1).
    pub one_received_slot: bool,
    /// Outline hulls skip their cast and received shadows (m6-plan P2).
    pub no_hull_shadow: bool,
    /// Outline hulls are their line's color, unlit and unshadowed (m6-plan P2's next probe).
    pub unlit_hulls: bool,
    /// A frame at full night reads the ramp's deep row alone (m6-plan P3).
    pub one_ramp_row: bool,
    /// How material textures are read (m6-plan P4).
    pub material_read: MaterialRead,
    /// Hair takes its material's highlight style's coordinate alone, on a branch (m6-plan P3).
    pub one_hair_style: bool,
    /// A face's light in head space is a constant, as if the CPU gave it: `face_margin`'s maths priced
    /// away, both of the face map's taps kept (m6-plan P3). The face is lit as by a fixed light.
    pub face_light_constant: bool,
    /// The control map is never read, as if unbound: each pixel takes its vertex's region and the
    /// neutral highlight mask, shadow bias and threshold (m6-plan K4's probe). It acts on every character,
    /// so it stands for K4 only where every figure's control map is constant per region, as the
    /// benchmark's generated figures' are.
    pub control_unbound: bool,
}

/// The constants' names (`anime_character.wgsl`).
pub(crate) mod override_constant {
    pub const NO_RIM: &str = "MEASURE_NO_RIM";
    pub const NO_LOCAL_LIGHTS: &str = "MEASURE_NO_LOCAL_LIGHTS";
    pub const LIGHT_CAP: &str = "MEASURE_LIGHT_CAP";
    pub const ONE_RECEIVED_SLOT: &str = "MEASURE_ONE_RECEIVED_SLOT";
    pub const NO_HULL_SHADOW: &str = "MEASURE_NO_HULL_SHADOW";
    pub const UNLIT_HULLS: &str = "MEASURE_UNLIT_HULLS";
    pub const ONE_RAMP_ROW: &str = "MEASURE_ONE_RAMP_ROW";
    pub const MATERIAL_READ: &str = "MEASURE_MATERIAL_READ";
    pub const ONE_HAIR_STYLE: &str = "MEASURE_ONE_HAIR_STYLE";
    pub const FACE_LIGHT_CONSTANT: &str = "MEASURE_FACE_LIGHT_CONSTANT";
    pub const CONTROL_UNBOUND: &str = "MEASURE_CONTROL_UNBOUND";

    /// Every one, in the order [`super::CharacterSwitches::constants`] gives them.
    #[cfg(test)]
    pub const ALL: [&str; 11] = [
        NO_RIM,
        NO_LOCAL_LIGHTS,
        LIGHT_CAP,
        ONE_RECEIVED_SLOT,
        NO_HULL_SHADOW,
        UNLIT_HULLS,
        ONE_RAMP_ROW,
        MATERIAL_READ,
        ONE_HAIR_STYLE,
        FACE_LIGHT_CONSTANT,
        CONTROL_UNBOUND,
    ];
}

impl CharacterSwitches {
    /// The shipping pixel: every switch off.
    pub const SHIPPING: Self = Self {
        no_rim: false,
        no_local_lights: false,
        light_cap: None,
        one_received_slot: false,
        no_hull_shadow: false,
        unlit_hulls: false,
        one_ramp_row: false,
        material_read: MaterialRead::TwoLevels,
        one_hair_style: false,
        face_light_constant: false,
        control_unbound: false,
    };

    /// Whether every switch is off.
    pub fn is_shipping(&self) -> bool {
        *self == Self::SHIPPING
    }

    /// The constants of the switches that are on, for the anime character family's main-pass pipelines;
    /// none at the default, so those pipelines are built exactly as without the switches.
    pub(crate) fn constants(&self) -> Vec<(&'static str, f64)> {
        use override_constant as name;
        let flags = [
            (name::NO_RIM, self.no_rim),
            (name::NO_LOCAL_LIGHTS, self.no_local_lights),
            (name::ONE_RECEIVED_SLOT, self.one_received_slot),
            (name::NO_HULL_SHADOW, self.no_hull_shadow),
            (name::UNLIT_HULLS, self.unlit_hulls),
            (name::ONE_RAMP_ROW, self.one_ramp_row),
            (name::ONE_HAIR_STYLE, self.one_hair_style),
            (name::FACE_LIGHT_CONSTANT, self.face_light_constant),
            (name::CONTROL_UNBOUND, self.control_unbound),
        ];
        let mut constants: Vec<_> =
            flags.into_iter().filter(|&(_, on)| on).map(|(name, _)| (name, 1.0)).collect();
        if let Some(cap) = self.light_cap {
            constants.push((name::LIGHT_CAP, f64::from(cap)));
        }
        match self.material_read {
            MaterialRead::TwoLevels => {}
            MaterialRead::Fractional => constants.push((name::MATERIAL_READ, 1.0)),
            MaterialRead::Whole => constants.push((name::MATERIAL_READ, 2.0)),
        }
        constants
    }
}

/// What the character pixel's counters counted in one frame (M6 B6;
/// [`RenderSettings::measure_pixels`](crate::RenderSettings::measure_pixels)), read back a few frames late.
/// All zero, at frame 0, before the first reading and wherever the counters aren't built.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MeasuredPixels {
    /// The frame counted, numbered from 1 by the counters themselves; 0 before any reading.
    pub frame: u64,
    /// The main view's pixels a listed character's surface or outline hull is nearest at: scene colour's
    /// character mask.
    pub character_pixels: u32,
    /// Of those, the ones no listed character's surface covers in the character prepass's IDs: hull lines
    /// outside every silhouette (one over another character behind it goes uncounted).
    pub hull_pixels: u32,
    /// The edge upscaler's output in 8 × 4 groups (a 32-wide SIMD group's stand-in), on a frame that
    /// upscales; 0 on one that doesn't.
    pub upscale_groups: u32,
    /// Of those, the ones whose every pixel has its four nearest input texels equal: wholly flat.
    pub flat_upscale_groups: u32,
}

/// `MeasureUniforms` in `measure.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct MeasureUniforms {
    /// The upscaler's `UpscaleUniforms::easu`.
    pub easu: [f32; 4],
    /// xy: the upscaler's output size.
    pub output: [u32; 4],
}

/// The counters' two pipelines, from one module.
#[derive(Debug)]
pub(crate) struct MeasurePipelines {
    pixels: wgpu::ComputePipeline,
    upscale: wgpu::ComputePipeline,
}

impl MeasurePipelines {
    /// Pipelines per set, for hot reload's count.
    pub(crate) const COUNT: u32 = 2;
}

/// The counters `measure.wgsl` adds into.
const COUNTERS: usize = 4;
const COUNTER_BYTES: u64 = (COUNTERS * size_of::<u32>()) as u64;
/// Readbacks in flight at most; a frame finding none free goes uncounted.
const READBACKS: usize = 3;

/// A readback's states.
const FREE: u8 = 0;
/// A frame's pass copied the counters into it; mapped once that frame is known to have been submitted.
const COPIED: u8 = 1;
const MAPPING: u8 = 2;
const MAPPED: u8 = 3;

#[derive(Debug)]
struct Readback {
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    frame: u64,
}

/// The counters' passes' names.
const PIXELS_PASS: &str = "measure pixels";
const UPSCALE_PASS: &str = "measure upscale";

/// The character pixel's counters (M6 B6): two compute passes over the frame, built with the renderer where
/// [`RenderSettings::measure_pixels`](crate::RenderSettings::measure_pixels) asks, and nowhere else. Their
/// readbacks follow the frame timer's: copied in the frame, mapped once [`Self::settle`] knows the frame ran,
/// read at a later frame's [`Self::begin`].
#[derive(Debug)]
pub(crate) struct PixelCounter {
    pipelines: MeasurePipelines,
    /// Kept to build the pipelines again from new shader sources: the pixels' and the upscale's.
    layouts: [wgpu::PipelineLayout; 2],
    builds: PipelineBuilds,
    shared: wgpu::BindGroup,
    pixels_layout: wgpu::BindGroupLayout,
    upscale_layout: wgpu::BindGroupLayout,
    counts: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    readbacks: Vec<Readback>,
    /// The readback this frame copies into, if one was free.
    current: Option<usize>,
    frames: u64,
    latest: MeasuredPixels,
}

impl PixelCounter {
    pub(crate) fn new(device: &wgpu::Device, builds: &PipelineBuilds, sources: &ShaderSources) -> Self {
        let counts = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("measured pixels"),
            size: COUNTER_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("measure uniforms"),
            size: size_of::<MeasureUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let buffer = |binding, ty, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(size),
            },
            count: None,
        };
        let shared_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("measure"),
            entries: &[
                buffer(0, wgpu::BufferBindingType::Storage { read_only: false }, COUNTER_BYTES),
                buffer(1, wgpu::BufferBindingType::Uniform, size_of::<MeasureUniforms>() as u64),
            ],
        });
        let shared = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("measure"),
            layout: &shared_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: counts.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: uniforms.as_entire_binding() },
            ],
        });
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let pixels_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(PIXELS_PASS),
            entries: &[texture(0, float), texture(1, wgpu::TextureSampleType::Uint)],
        });
        let upscale_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(UPSCALE_PASS),
            entries: &[texture(2, float)],
        });
        let layouts = [&pixels_layout, &upscale_layout].map(|layout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("measure"),
                bind_group_layouts: &[Some(&shared_layout), Some(layout)],
                immediate_size: 0,
            })
        });
        let readbacks = (0..READBACKS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("measured pixels readback"),
                    size: COUNTER_BYTES,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
                frame: 0,
            })
            .collect();
        let pipelines = build(device, &layouts, builds, sources);
        Self {
            pipelines,
            layouts,
            builds: builds.clone(),
            shared,
            pixels_layout,
            upscale_layout,
            counts,
            uniforms,
            readbacks,
            current: None,
            frames: 0,
            latest: MeasuredPixels::default(),
        }
    }

    /// The pipelines, to build from new sources instead (off the frame thread: `shader_builds`), for
    /// [`Self::set_pipelines`] (shader hot reload).
    pub(crate) fn rebuild(&self, device: &wgpu::Device) -> Step<MeasurePipelines> {
        let (device, layouts, builds) = (device.clone(), self.layouts.clone(), self.builds.clone());
        Box::new(move |sources, _| build(&device, &layouts, &builds, sources))
    }

    pub(crate) fn set_pipelines(&mut self, pipelines: MeasurePipelines) {
        self.pipelines = pipelines;
    }

    /// At the start of a prepared frame: whether the frame before ran (`ran`), so a readback it copied into is
    /// mapped, now that it was submitted.
    pub(crate) fn settle(&mut self, ran: bool) {
        for readback in &self.readbacks {
            if readback.state.load(Ordering::Acquire) != COPIED {
                continue;
            }
            if !ran {
                readback.state.store(FREE, Ordering::Release);
                continue;
            }
            readback.state.store(MAPPING, Ordering::Release);
            let state = Arc::clone(&readback.state);
            readback.buffer.map_async(wgpu::MapMode::Read, .., move |result| {
                // A failed map (device lost, buffer dropped) frees the readback with nothing read.
                state.store(if result.is_ok() { MAPPED } else { FREE }, Ordering::Release);
            });
        }
    }

    /// Reads every mapped readback, keeping the newest frame's, and takes a free one for the frame about to be
    /// recorded, whose uniforms it writes: `easu` and `output` are the upscaler's, on a frame that upscales.
    pub(crate) fn begin(&mut self, queue: &wgpu::Queue, easu: [f32; 4], output: (u32, u32)) {
        for readback in &mut self.readbacks {
            if readback.state.load(Ordering::Acquire) != MAPPED {
                continue;
            }
            if readback.frame > self.latest.frame {
                // The buffer mapped successfully, so the range is available.
                if let Ok(bytes) = readback.buffer.get_mapped_range(..) {
                    let counts: [u32; COUNTERS] =
                        bytemuck::pod_read_unaligned(&bytes[..COUNTER_BYTES as usize]);
                    let [character_pixels, hull_pixels, upscale_groups, flat_upscale_groups] = counts;
                    self.latest = MeasuredPixels {
                        frame: readback.frame,
                        character_pixels,
                        hull_pixels,
                        upscale_groups,
                        flat_upscale_groups,
                    };
                }
            }
            readback.buffer.unmap();
            readback.state.store(FREE, Ordering::Release);
        }
        self.frames += 1;
        self.current =
            self.readbacks.iter().position(|readback| readback.state.load(Ordering::Acquire) == FREE);
        if let Some(index) = self.current {
            self.readbacks[index].frame = self.frames;
            let uniforms = MeasureUniforms { easu, output: [output.0, output.1, 0, 0] };
            queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        }
    }

    /// The newest frame read back.
    pub(crate) fn latest(&self) -> MeasuredPixels {
        self.latest
    }

    /// Adds the counters' passes to `graph`, if a readback was free for the frame: the pixels' over
    /// `scene_color` and `character_ids`, then, on a frame that upscales, the upscale's over the edge
    /// upscaler's input `upscale_input`, at `output` pixels.
    pub(crate) fn add_passes<'f>(
        &'f self,
        graph: &mut FrameGraph<'f>,
        device: &'f wgpu::Device,
        (scene_color, character_ids): (TextureId, TextureId),
        upscale_input: Option<TextureId>,
        output: (u32, u32),
        labels: bool,
    ) {
        let Some(index) = self.current else { return };
        let readback = &self.readbacks[index];
        let size = graph.size(scene_color);
        // The upscale's pass copies the counters out where it runs, else the pixels'.
        let copy = move |encoder: &mut wgpu::CommandEncoder| {
            encoder.copy_buffer_to_buffer(&self.counts, 0, &readback.buffer, 0, COUNTER_BYTES);
            readback.state.store(COPIED, Ordering::Release);
        };
        let pixels_copy = upscale_input.is_none().then_some(copy);
        graph.add_pass(PIXELS_PASS).read(scene_color).read(character_ids).side_effect().record(move |ctx| {
            let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: labels.then_some(PIXELS_PASS),
                layout: &self.pixels_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(ctx.view(scene_color)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(ctx.view(character_ids)),
                    },
                ],
            });
            ctx.encoder.clear_buffer(&self.counts, 0, None);
            {
                let mut pass = ctx.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: labels.then_some(PIXELS_PASS),
                    timestamp_writes: ctx.compute_timestamp_writes(),
                });
                pass.set_pipeline(&self.pipelines.pixels);
                pass.set_bind_group(0, &self.shared, &[]);
                pass.set_bind_group(1, &textures, &[]);
                pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
            }
            if let Some(copy) = pixels_copy {
                copy(ctx.encoder);
            }
        });
        let Some(input) = upscale_input else { return };
        graph.add_pass(UPSCALE_PASS).read(input).side_effect().record(move |ctx| {
            let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: labels.then_some(UPSCALE_PASS),
                layout: &self.upscale_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(ctx.view(input)),
                }],
            });
            {
                let mut pass = ctx.encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: labels.then_some(UPSCALE_PASS),
                    timestamp_writes: ctx.compute_timestamp_writes(),
                });
                pass.set_pipeline(&self.pipelines.upscale);
                pass.set_bind_group(0, &self.shared, &[]);
                pass.set_bind_group(1, &textures, &[]);
                pass.dispatch_workgroups(output.0.div_ceil(8), output.1.div_ceil(4), 1);
            }
            copy(ctx.encoder);
        });
    }
}

/// The counters' pipelines on `layouts` (the pixels' and the upscale's), from the measure program in `sources`.
fn build(
    device: &wgpu::Device,
    layouts: &[wgpu::PipelineLayout; 2],
    builds: &PipelineBuilds,
    sources: &ShaderSources,
) -> MeasurePipelines {
    let module = shaders::module(device, Program::Measure, sources);
    let pipeline = |label, layout, entry_point| {
        builds.compute(
            device,
            PipelineFamily::Debug,
            &wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: builds.cache(),
            },
        )
    };
    MeasurePipelines {
        pixels: pipeline(PIXELS_PASS, &layouts[0], "cs_measure_pixels"),
        upscale: pipeline(UPSCALE_PASS, &layouts[1], "cs_measure_upscale"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::TierTable;

    /// Every switch on, the light cap at zero lights.
    fn every_switch() -> CharacterSwitches {
        CharacterSwitches {
            no_rim: true,
            no_local_lights: true,
            light_cap: Some(0),
            one_received_slot: true,
            no_hull_shadow: true,
            unlit_hulls: true,
            one_ramp_row: true,
            material_read: MaterialRead::Whole,
            one_hair_style: true,
            face_light_constant: true,
            control_unbound: true,
        }
    }

    #[test]
    fn the_default_is_the_shipping_pixel_and_passes_no_constant() {
        assert_eq!(CharacterSwitches::default(), CharacterSwitches::SHIPPING);
        assert!(CharacterSwitches::default().is_shipping());
        assert_eq!(CharacterSwitches::SHIPPING.constants(), []);
    }

    /// No tier turns a switch on: they are a dev sandbox's alone.
    #[test]
    fn every_tier_ships_with_every_switch_off() {
        let table = TierTable::shipped();
        for tier in table.tiers() {
            let settings = tier.render_settings();
            assert!(settings.character_switches.is_shipping(), "{:?}", tier.id);
        }
    }

    /// Each switch passes its own constant and nothing else, each name once.
    #[test]
    fn each_switch_passes_its_one_constant() {
        let all = every_switch().constants();
        let names: Vec<_> = all.iter().map(|&(name, _)| name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        let mut expected = override_constant::ALL.to_vec();
        expected.sort_unstable();
        assert_eq!(sorted, expected);
        assert!(all.contains(&(override_constant::LIGHT_CAP, 0.0)));
        assert!(all.contains(&(override_constant::MATERIAL_READ, 2.0)));
        let fractional =
            CharacterSwitches { material_read: MaterialRead::Fractional, ..CharacterSwitches::SHIPPING };
        assert_eq!(fractional.constants(), [(override_constant::MATERIAL_READ, 1.0)]);
        let capped = CharacterSwitches { light_cap: Some(8), ..CharacterSwitches::SHIPPING };
        assert_eq!(capped.constants(), [(override_constant::LIGHT_CAP, 8.0)]);
    }

    /// Every constant is one the programs that take them declare, as a bool or a u32 whose default is the
    /// shipping value: the anime character program (surfaces and eyes through hair) and the outline program.
    #[test]
    fn the_character_programs_declare_every_switch_at_its_shipping_default() {
        use sapphire_rhi::wgpu::naga;

        for program in [Program::AnimeCharacter, Program::Outline] {
            let module = shaders::validate(program, &ShaderSources::compiled_in())
                .expect("compiled-in shaders validate");
            for name in override_constant::ALL {
                let found = module.overrides.iter().find(|(_, o)| o.name.as_deref() == Some(name));
                let (_, declared) = found.unwrap_or_else(|| panic!("{program:?} doesn't declare {name}"));
                let init = declared.init.expect("a default");
                let literal = match module.global_expressions[init] {
                    naga::Expression::Literal(literal) => literal,
                    ref other => panic!("{name}'s default is {other:?}"),
                };
                let shipping = match name {
                    override_constant::LIGHT_CAP => naga::Literal::U32(u32::MAX),
                    override_constant::MATERIAL_READ => naga::Literal::U32(0),
                    _ => naga::Literal::Bool(false),
                };
                assert_eq!(literal, shipping, "{program:?}'s {name}");
            }
        }
    }
}
