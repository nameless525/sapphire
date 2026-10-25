//! Bind group layouts, vertex layouts and render pipelines for the scene passes and the
//! prepasses, and the anime character shader's static permutations.

use std::mem::offset_of;
use std::num::NonZeroU64;

use sapphire_assets::material::{AnimeCharacterParams, MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::{PositionFormat, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::gpu_types::{Dequantize, DrawData, FrameUniforms, InstanceData, ViewUniforms};
use crate::lights::GpuLight;
use crate::shaders;

/// Reverse-Z depth: cleared to 0, tested with `Greater`. 32-bit float keeps precision all
/// the way out with an infinite far plane.
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// The main pass's color: linear HDR light, unclamped, which post tone maps into the target.
/// Alpha is kept for the character bloom mask (M1 C7).
pub(crate) const SCENE_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The scene prepass's world normals, octahedral: 4 bytes a pixel with f16's 11 significant
/// bits, finer than 8-bit unorm and renderable everywhere.
pub(crate) const NORMALS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// The scene prepass's velocity, in texture coordinates. f16 resolves motion to a small
/// fraction of a pixel at 4K up to about half the screen per frame.
pub(crate) const VELOCITY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// Vertex buffer slots. Every mesh pass binds the draw record and positions; a position-only
/// pass (shadows, the character prepass) needs nothing more, so the streams it skips come last.
pub(crate) mod slot {
    /// The draw record, stepped per instance.
    pub const DRAW: u32 = 0;
    /// The mesh's position stream, or a skinned or morphed instance's skinning output.
    pub const POSITIONS: u32 = 1;
    /// Normal and outline normal (likewise).
    pub const SHADING: u32 = 2;
    /// UV and vertex color: always the mesh's.
    pub const SURFACE: u32 = 3;
    /// Positions when the instance was last drawn, for velocity (the scene prepass): the other
    /// half of a skinned instance's outputs, or slot 1's buffer again.
    pub const PREVIOUS_POSITIONS: u32 = 4;
}

/// Slot 0: the draw record (WGSL `DrawRecord`).
pub(crate) const DRAW_ATTRIBUTES: [wgpu::VertexAttribute; 5] = {
    let dequantize = offset_of!(DrawData, dequantize);
    [
        attribute(5, wgpu::VertexFormat::Uint32, offset_of!(DrawData, instance)),
        attribute(6, wgpu::VertexFormat::Uint32, offset_of!(DrawData, material)),
        attribute(7, wgpu::VertexFormat::Float32x3, dequantize + offset_of!(Dequantize, position_offset)),
        attribute(8, wgpu::VertexFormat::Float32x3, dequantize + offset_of!(Dequantize, position_scale)),
        attribute(9, wgpu::VertexFormat::Float32x4, dequantize + offset_of!(Dequantize, uv_transform)),
    ]
};

/// Slot 1 for quantized positions ([`PositionFormat::Unorm16`]).
pub(crate) const POSITION_UNORM16_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(0, wgpu::VertexFormat::Unorm16x4, 0)];

/// Slot 1 for f32 positions ([`PositionFormat::Float32`]). The shader reads a `vec4`, whose w
/// the vertex fetch fills with 1.
pub(crate) const POSITION_FLOAT32_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(0, wgpu::VertexFormat::Float32x3, 0)];

/// Slot 2: [`VertexShading`].
pub(crate) const SHADING_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    [attribute(1, wgpu::VertexFormat::Snorm16x2, 0), attribute(2, wgpu::VertexFormat::Snorm16x2, 4)];

/// Slot 3: [`VertexSurface`].
pub(crate) const SURFACE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    [attribute(3, wgpu::VertexFormat::Unorm16x2, 0), attribute(4, wgpu::VertexFormat::Unorm8x4, 4)];

/// The slot 1 attributes for meshes in `format`.
pub(crate) fn position_attributes(format: PositionFormat) -> &'static [wgpu::VertexAttribute] {
    match format {
        PositionFormat::Unorm16 => &POSITION_UNORM16_ATTRIBUTES,
        PositionFormat::Float32 => &POSITION_FLOAT32_ATTRIBUTES,
    }
}

/// Slot 4 for quantized and f32 positions: slot 1's format at location 10.
pub(crate) const PREVIOUS_UNORM16_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(10, wgpu::VertexFormat::Unorm16x4, 0)];
pub(crate) const PREVIOUS_FLOAT32_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    [attribute(10, wgpu::VertexFormat::Float32x3, 0)];

/// The slot 4 attributes for meshes in `format`.
pub(crate) fn previous_position_attributes(format: PositionFormat) -> &'static [wgpu::VertexAttribute] {
    match format {
        PositionFormat::Unorm16 => &PREVIOUS_UNORM16_ATTRIBUTES,
        PositionFormat::Float32 => &PREVIOUS_FLOAT32_ATTRIBUTES,
    }
}

const fn attribute(location: u32, format: wgpu::VertexFormat, offset: usize) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute { format, offset: offset as u64, shader_location: location }
}

/// The mesh passes, by the vertex streams they read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeshPass {
    /// The main pass's surface and outline draws: every stream but the previous positions.
    Main,
    /// Positions only.
    CharacterPrepass,
    /// Positions, shading and previous positions.
    ScenePrepass,
}

impl MeshPass {
    /// The streams the pass binds besides the draw record and positions.
    pub(crate) fn reads_shading(self) -> bool {
        self != Self::CharacterPrepass
    }

    pub(crate) fn reads_surface(self) -> bool {
        self == Self::Main
    }

    pub(crate) fn reads_previous(self) -> bool {
        self == Self::ScenePrepass
    }

    /// Whether the pass shades with materials, binding each draw's group 3: only the main pass;
    /// the prepasses' layout leaves group 3 out.
    pub(crate) fn binds_materials(self) -> bool {
        self == Self::Main
    }

    /// The pass's vertex buffers for a mesh in `format`, in slot order.
    pub(crate) fn buffers(self, format: PositionFormat) -> [Option<wgpu::VertexBufferLayout<'static>>; 5] {
        let vertex = |stride: usize, attributes| {
            Some(wgpu::VertexBufferLayout {
                array_stride: stride as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes,
            })
        };
        [
            Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<DrawData>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &DRAW_ATTRIBUTES,
            }),
            vertex(format.stride(), position_attributes(format)),
            if self.reads_shading() { vertex(size_of::<VertexShading>(), &SHADING_ATTRIBUTES) } else { None },
            if self.reads_surface() { vertex(size_of::<VertexSurface>(), &SURFACE_ATTRIBUTES) } else { None },
            if self.reads_previous() {
                vertex(format.stride(), previous_position_attributes(format))
            } else {
                None
            },
        ]
    }
}

/// Group 3's bindings: texture slot `i` of a family at binding `i` (families have at most
/// [`material_binding::SAMPLER`] slots), then the samplers.
pub(crate) mod material_binding {
    /// Trilinear, repeating: every UV-mapped slot.
    pub const SAMPLER: u32 = 8;
    /// Linear and clamped, without mips: ramps, read at a row's texel centers so rows never
    /// blend.
    pub const RAMP_SAMPLER: u32 = 9;
    /// Nearest, repeating, level 0 only: material IDs, which must never blend
    /// (`sapphire_assets::character_maps`).
    pub const ID_SAMPLER: u32 = 10;
}

/// The anime character shader's static permutation bits (spec §5.12, M1 plan C1): at most
/// eight, in the plan's order. Everything else a material varies (band or ramp diffuse, a
/// deep band, which maps are bound, matcap channels, stockings) is a branch on its block that
/// every pixel of a draw takes alike.
///
/// | Bit | From | What it selects today |
/// |---|---|---|
/// | skinned, morph | the mesh | the f32-position pipelines, which read the compute skinning pass's outputs (M1 R2); no shader variant, since skinning and morphs no longer run in the vertex stage |
/// | alpha test | the material ([`MaterialFlags::ALPHA_TEST`]) | [`SurfaceVariant::AlphaTest`]: the surface and outline shaders discard below the cutoff |
/// | outline | the material ([`MaterialFlags::OUTLINE`]) | whether the draw is in the outline pass |
/// | face, hair, eye | the material | nothing yet: faces (C3), hair (C5) and eyes (C6) add their variants |
/// | transparent | the material ([`MaterialFlags::TRANSPARENT`]) | [`SurfaceVariant::Transparent`]: blended over the frame after the sky, depth-tested without writing, with no outline and in no prepass |
///
/// A variant's pipelines are built for a position format when a mesh in that format is uploaded
/// with a material that needs them, or a material edit moves to the variant (for every format
/// drawn so far), so apps never compile variants they don't use. Building happens there, on
/// the calling thread: an edit into a new variant can hitch once, until pipelines compile on
/// workers at pack load (spec §5.12, "Warm-up"). Transparent draws keep the draw list's order
/// rather than going back to front: sorting comes with draw sorting (spec §5.12, "Sorting").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Permutation(u8);

impl Permutation {
    pub const SKINNED: Self = Self(1 << 0);
    pub const ALPHA_TEST: Self = Self(1 << 1);
    pub const OUTLINE: Self = Self(1 << 2);
    pub const MORPH: Self = Self(1 << 3);
    pub const FACE: Self = Self(1 << 4);
    pub const HAIR: Self = Self(1 << 5);
    pub const EYE: Self = Self(1 << 6);
    pub const TRANSPARENT: Self = Self(1 << 7);

    /// The permutation of a material with `flags` on a mesh that is `skinned` and `morphed` or
    /// not.
    pub(crate) fn new(flags: MaterialFlags, skinned: bool, morphed: bool) -> Self {
        let bits = [
            (Self::SKINNED, skinned),
            (Self::ALPHA_TEST, flags.contains(MaterialFlags::ALPHA_TEST)),
            (Self::OUTLINE, flags.contains(MaterialFlags::OUTLINE)),
            (Self::MORPH, morphed),
            (Self::FACE, flags.contains(MaterialFlags::FACE)),
            (Self::HAIR, flags.contains(MaterialFlags::HAIR)),
            (Self::EYE, flags.contains(MaterialFlags::EYE)),
            (Self::TRANSPARENT, flags.contains(MaterialFlags::TRANSPARENT)),
        ];
        Self(bits.iter().filter(|(_, set)| *set).fold(0, |all, (bit, _)| all | bit.0))
    }

    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The surface pipelines it draws with.
    pub(crate) fn surface(self) -> SurfaceVariant {
        if self.contains(Self::TRANSPARENT) {
            SurfaceVariant::Transparent
        } else if self.contains(Self::ALPHA_TEST) {
            SurfaceVariant::AlphaTest
        } else {
            SurfaceVariant::Opaque
        }
    }

    /// Whether it draws an outline hull: transparent surfaces never do.
    pub(crate) fn outlined(self) -> bool {
        self.contains(Self::OUTLINE) && !self.contains(Self::TRANSPARENT)
    }
}

/// The anime character pipelines one [`PositionFormat`] can draw with: the permutation bits that
/// change the shader or its render state today (see [`Permutation`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SurfaceVariant {
    Opaque,
    AlphaTest,
    Transparent,
}

impl SurfaceVariant {
    /// Every variant, in drawing order.
    pub(crate) const ALL: [Self; 3] = [Self::Opaque, Self::AlphaTest, Self::Transparent];
    /// The variants that write depth and can draw an outline.
    pub(crate) const DEPTH_WRITING: [Self; 2] = [Self::Opaque, Self::AlphaTest];

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// The variant a material with `flags` draws with, on any mesh.
    pub(crate) fn of_material(flags: MaterialFlags) -> Self {
        Permutation::new(flags, false, false).surface()
    }

    /// The shaders' pipeline-overridable constants for the variant (`PERMUTATION_*` in
    /// `anime_character.wgsl` and `outline.wgsl`).
    pub(crate) fn constants(self) -> &'static [(&'static str, f64)] {
        match self {
            Self::Opaque => &[],
            Self::AlphaTest => &[(override_constant::ALPHA_TEST, 1.0)],
            Self::Transparent => &[(override_constant::TRANSPARENT, 1.0)],
        }
    }
}

/// The names of the shaders' permutation constants.
pub(crate) mod override_constant {
    pub const ALPHA_TEST: &str = "PERMUTATION_ALPHA_TEST";
    pub const TRANSPARENT: &str = "PERMUTATION_TRANSPARENT";
}

/// The bind group layouts of `shaders/frame.wgsl`, and group 3 per material family.
#[derive(Debug)]
pub(crate) struct Layouts {
    /// Group 0: frame constants, local lights and their lists.
    pub frame: wgpu::BindGroupLayout,
    /// Group 1: one view, bound with a dynamic offset. The light-cluster pass binds it too.
    pub view: wgpu::BindGroupLayout,
    /// Group 2: instances and a material family's parameter buffer.
    pub scene: wgpu::BindGroupLayout,
    /// Group 3 per [`MaterialFamily::ALL`] entry: the family's texture slots and samplers.
    pub materials: [wgpu::BindGroupLayout; MaterialFamily::ALL.len()],
}

impl Layouts {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        use wgpu::ShaderStages as S;
        let buffer =
            |binding, visibility, ty, has_dynamic_offset, min_size: usize| wgpu::BindGroupLayoutEntry {
                binding,
                visibility,
                ty: wgpu::BindingType::Buffer {
                    ty,
                    has_dynamic_offset,
                    min_binding_size: NonZeroU64::new(min_size as u64),
                },
                count: None,
            };
        let uniform = wgpu::BufferBindingType::Uniform;
        let storage = wgpu::BufferBindingType::Storage { read_only: true };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        Self {
            frame: layout(
                "frame",
                &[
                    buffer(0, S::VERTEX_FRAGMENT, uniform, false, size_of::<FrameUniforms>()),
                    buffer(1, S::FRAGMENT, storage, false, size_of::<GpuLight>()),
                    buffer(2, S::FRAGMENT, storage, false, size_of::<u32>()),
                ],
            ),
            view: layout(
                "view",
                &[buffer(0, S::VERTEX_FRAGMENT | S::COMPUTE, uniform, true, size_of::<ViewUniforms>())],
            ),
            scene: layout(
                "scene",
                &[
                    buffer(0, S::VERTEX, storage, false, size_of::<InstanceData>()),
                    // Each family's pipelines bind their own family's buffer here; only the
                    // anime character family draws so far.
                    buffer(1, S::VERTEX_FRAGMENT, storage, false, size_of::<AnimeCharacterParams>()),
                ],
            ),
            materials: MaterialFamily::ALL.map(|family| material_layout(device, family)),
        }
    }
}

/// Group 3 for `family`: one filterable 2D texture per slot, fragment-visible, then the three
/// samplers ([`material_binding`]).
fn material_layout(device: &wgpu::Device, family: MaterialFamily) -> wgpu::BindGroupLayout {
    let fragment = wgpu::ShaderStages::FRAGMENT;
    let slots = family.texture_slots().len() as u32;
    debug_assert!(slots <= material_binding::SAMPLER, "texture slots end before the samplers");
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..slots)
        .map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        })
        .collect();
    let samplers = [
        (material_binding::SAMPLER, wgpu::SamplerBindingType::Filtering),
        (material_binding::RAMP_SAMPLER, wgpu::SamplerBindingType::Filtering),
        (material_binding::ID_SAMPLER, wgpu::SamplerBindingType::NonFiltering),
    ];
    for (binding, kind) in samplers {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Sampler(kind),
            count: None,
        });
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(family.name()),
        entries: &entries,
    })
}

/// Fixed-function state that differs between passes. Entry points are `vs_<name>` and
/// `fs_<name>`, with spaces in `name` as underscores.
struct PassDesc<'a> {
    name: &'static str,
    module: &'a wgpu::ShaderModule,
    vertex_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    cull: Option<wgpu::Face>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    /// Blending into the color target; `None` replaces it.
    blend: Option<wgpu::BlendState>,
    /// Values of the shaders' pipeline-overridable constants.
    constants: &'a [(&'a str, f64)],
}

/// Transparent anime character surfaces: blended by coverage, keeping the scene color's alpha,
/// which opaque surfaces set (the character bloom mask, C7).
const TRANSPARENT_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// The mesh pipelines for one [`PositionFormat`]: the same shaders, with that format's
/// position attribute.
#[derive(Debug)]
pub(crate) struct MeshPipelines {
    /// The anime character surfaces, per [`SurfaceVariant::ALL`] entry; each is built with the
    /// first draw that needs it ([`Pipelines::prepare_variant`]).
    character: [Option<wgpu::RenderPipeline>; SurfaceVariant::ALL.len()],
    /// Their outline hulls, per [`SurfaceVariant::DEPTH_WRITING`] entry.
    outline: [Option<wgpu::RenderPipeline>; SurfaceVariant::DEPTH_WRITING.len()],
    /// Depth, world normals and velocity of every opaque instance (`prepass.wgsl`).
    pub scene_prepass: wgpu::RenderPipeline,
}

impl MeshPipelines {
    /// The surface pipeline of `variant`, once it was prepared.
    pub(crate) fn character(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        self.character[variant.index()].as_ref()
    }

    /// The outline pipeline of `variant`, once it was prepared; transparent surfaces have none.
    pub(crate) fn outline(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        self.outline.get(variant.index())?.as_ref()
    }

    /// Anime character pipelines built: surfaces and outlines.
    fn built(&self) -> u32 {
        let count = |pipelines: &[Option<wgpu::RenderPipeline>]| pipelines.iter().flatten().count();
        (count(&self.character) + count(&self.outline)) as u32
    }
}

/// The scene pipelines. Quantized-mesh pipelines are built up front, opaque surfaces only; the
/// f32 ones when the first f32 mesh, or skinned or morphed mesh, is uploaded
/// ([`Self::prepare_format`]); the other surface variants when a draw first needs them
/// ([`Self::prepare_variant`]); and the character prepass's with the first skinned or morphed
/// mesh ([`Self::prepare_characters`]). Apps never pay for compiling what they don't draw.
#[derive(Debug)]
pub(crate) struct Pipelines {
    /// Per [`PositionFormat::ALL`] entry.
    mesh: [Option<MeshPipelines>; 2],
    /// Depth of skinned and morphed instances, whose skinning outputs are f32.
    character_prepass: Option<wgpu::RenderPipeline>,
    pub sky: wgpu::RenderPipeline,
    target: PipelineTarget,
    character_module: wgpu::ShaderModule,
    outline_module: wgpu::ShaderModule,
    prepass_module: wgpu::ShaderModule,
}

/// What every scene pipeline shares, kept to build the f32 ones later.
#[derive(Debug)]
struct PipelineTarget {
    /// Groups 0–2, for passes that read no material textures: the sky and the prepasses.
    layout: wgpu::PipelineLayout,
    /// Groups 0–2 and the anime character family's group 3: the passes that shade with it
    /// (its shader and its outlines).
    character_layout: wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
    cache: Option<wgpu::PipelineCache>,
}

impl Pipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        cache: Option<&wgpu::PipelineCache>,
    ) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.view), Some(&layouts.scene)],
            immediate_size: 0,
        });
        let character_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("anime character"),
            bind_group_layouts: &[
                Some(&layouts.frame),
                Some(&layouts.view),
                Some(&layouts.scene),
                Some(&layouts.materials[MaterialFamily::AnimeCharacter.index()]),
            ],
            immediate_size: 0,
        });
        let target =
            PipelineTarget { layout, character_layout, color_format, sample_count, cache: cache.cloned() };
        let module = |name, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        // A fullscreen triangle at the far plane, drawn after the opaque surfaces: `Equal`
        // against the cleared depth (0) shades only pixels no geometry covered.
        let sky = target.build(
            device,
            PassDesc {
                name: "sky",
                module: &module("sky", shaders::SKY),
                vertex_buffers: &[],
                cull: None,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::Equal,
                blend: None,
                constants: &[],
            },
        );
        let mut pipelines = Self {
            mesh: [None, None],
            character_prepass: None,
            sky,
            target,
            character_module: module("anime character", shaders::ANIME_CHARACTER),
            outline_module: module("outline", shaders::OUTLINE),
            prepass_module: module("prepass", shaders::PREPASS),
        };
        pipelines.prepare_format(device, PositionFormat::Unorm16);
        pipelines
    }

    /// Builds the mesh pipelines for `format` unless they exist: the scene prepass, and the
    /// opaque surfaces and their outlines.
    pub(crate) fn prepare_format(&mut self, device: &wgpu::Device, format: PositionFormat) {
        if self.mesh[format_index(format)].is_some() {
            return;
        }
        let normals_and_velocity = [NORMALS_FORMAT, VELOCITY_FORMAT].map(|format| {
            Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })
        });
        let scene_prepass = self.target.build_prepass(
            device,
            "scene prepass",
            &self.prepass_module,
            &MeshPass::ScenePrepass.buffers(format),
            &normals_and_velocity,
        );
        self.mesh[format_index(format)] =
            Some(MeshPipelines { character: [None, None, None], outline: [None, None], scene_prepass });
        self.prepare_variant(device, format, SurfaceVariant::Opaque);
    }

    /// Builds the surface and outline pipelines of `variant` for `format`, whose mesh
    /// pipelines must exist, unless they do already.
    pub(crate) fn prepare_variant(
        &mut self,
        device: &wgpu::Device,
        format: PositionFormat,
        variant: SurfaceVariant,
    ) {
        let mesh =
            self.mesh[format_index(format)].as_ref().expect("prepare_format built the format's pipelines");
        if mesh.character(variant).is_some() {
            return;
        }
        let buffers = MeshPass::Main.buffers(format);
        let layout = &self.target.character_layout;
        let transparent = variant == SurfaceVariant::Transparent;
        let character = self.target.build_with(
            device,
            layout,
            PassDesc {
                name: "anime character",
                module: &self.character_module,
                vertex_buffers: &buffers,
                cull: Some(wgpu::Face::Back),
                // Transparent surfaces are depth-tested against the opaque frame but don't hide
                // what lies behind them.
                depth_write: !transparent,
                depth_compare: wgpu::CompareFunction::Greater,
                blend: transparent.then_some(TRANSPARENT_BLEND),
                constants: variant.constants(),
            },
        );
        // Inverted hull: keep only the back faces of the extruded shell.
        let outline = (!transparent).then(|| {
            self.target.build_with(
                device,
                layout,
                PassDesc {
                    name: "outline",
                    module: &self.outline_module,
                    vertex_buffers: &buffers,
                    cull: Some(wgpu::Face::Front),
                    depth_write: true,
                    depth_compare: wgpu::CompareFunction::Greater,
                    blend: None,
                    constants: variant.constants(),
                },
            )
        });
        let mesh = self.mesh[format_index(format)].as_mut().expect("checked above");
        mesh.character[variant.index()] = Some(character);
        if let Some(outline) = outline {
            mesh.outline[variant.index()] = Some(outline);
        }
    }

    /// Anime character pipelines built so far: surfaces and outlines, across formats.
    pub(crate) fn character_pipelines(&self) -> u32 {
        self.mesh.iter().flatten().map(MeshPipelines::built).sum()
    }

    /// Builds the character prepass's pipeline unless it exists: for the first skinned or
    /// morphed mesh.
    pub(crate) fn prepare_characters(&mut self, device: &wgpu::Device) {
        if self.character_prepass.is_none() {
            let buffers = MeshPass::CharacterPrepass.buffers(PositionFormat::Float32);
            self.character_prepass = Some(self.target.build_prepass(
                device,
                "character prepass",
                &self.prepass_module,
                &buffers,
                &[],
            ));
        }
    }

    /// The mesh pipelines for `format`, once [`Self::prepare_format`] built them.
    pub(crate) fn mesh(&self, format: PositionFormat) -> Option<&MeshPipelines> {
        self.mesh[format_index(format)].as_ref()
    }

    /// The character prepass's pipeline, once [`Self::prepare_characters`] built it.
    pub(crate) fn character_prepass(&self) -> Option<&wgpu::RenderPipeline> {
        self.character_prepass.as_ref()
    }
}

impl PipelineTarget {
    /// A prepass pipeline: back faces culled, reverse-Z depth written, no multisampling (the
    /// targets are sampled by later passes), and `color_targets` written by the
    /// `fs_<entry>` entry point when there are any. `name` is the entry points' suffix with
    /// spaces as underscores.
    fn build_prepass(
        &self,
        device: &wgpu::Device,
        name: &'static str,
        module: &wgpu::ShaderModule,
        vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
        color_targets: &[Option<wgpu::ColorTargetState>],
    ) -> wgpu::RenderPipeline {
        let entry = name.replace(' ', "_");
        let (vs, fs) = (format!("vs_{entry}"), format!("fs_{entry}"));
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(name),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(&vs),
                compilation_options: Default::default(),
                buffers: vertex_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: (!color_targets.is_empty()).then(|| wgpu::FragmentState {
                module,
                entry_point: Some(&fs),
                compilation_options: Default::default(),
                targets: color_targets,
            }),
            multiview_mask: None,
            cache: self.cache.as_ref(),
        })
    }

    /// A pipeline on the groups 0–2 layout.
    fn build(&self, device: &wgpu::Device, pass: PassDesc<'_>) -> wgpu::RenderPipeline {
        self.build_with(device, &self.layout, pass)
    }

    fn build_with(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        pass: PassDesc<'_>,
    ) -> wgpu::RenderPipeline {
        let color_targets = [Some(wgpu::ColorTargetState {
            format: self.color_format,
            blend: pass.blend,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let entry = pass.name.replace(' ', "_");
        let (vs, fs) = (format!("vs_{entry}"), format!("fs_{entry}"));
        let compilation_options =
            wgpu::PipelineCompilationOptions { constants: pass.constants, ..Default::default() };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(pass.name),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: pass.module,
                entry_point: Some(&vs),
                compilation_options: compilation_options.clone(),
                buffers: pass.vertex_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: pass.cull,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(pass.depth_write),
                depth_compare: Some(pass.depth_compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: self.sample_count, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: pass.module,
                entry_point: Some(&fs),
                compilation_options,
                targets: &color_targets,
            }),
            multiview_mask: None,
            cache: self.cache.as_ref(),
        })
    }
}

/// The index of `format` in [`PositionFormat::ALL`].
pub(crate) fn format_index(format: PositionFormat) -> usize {
    PositionFormat::ALL.iter().position(|&f| f == format).expect("ALL lists every format")
}
