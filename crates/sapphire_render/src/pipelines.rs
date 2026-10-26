//! Bind group layouts, vertex layouts and render pipelines for the scene passes and the
//! prepasses: the surface variants of the material families meshes draw with (the anime character
//! shader's static permutations, the environment shader's and fog cards'), and their outline
//! hulls.

use std::mem::offset_of;
use std::num::NonZeroU64;

use sapphire_assets::material::{MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::{PositionFormat, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::LOG_TARGET;
use crate::characters::GpuCharacter;
use crate::environment::{self, EnvironmentSettings};
use crate::fog;
use crate::gpu_types::{Dequantize, DrawData, FrameUniforms, InstanceData, ViewUniforms};
use crate::lights::GpuLight;
use crate::shaders::{self, Program, ShaderSources};
use crate::shadows::{ShadowSettings, ShadowUniforms};

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

/// The scene prepass's outline mask (M1 C4): 1 where an outline hull is the nearest surface.
pub(crate) const OUTLINES_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// The scene prepass's color targets: normals, velocity and the outline mask, which only the
/// hulls write (`outlines`); surfaces leave it as cleared, drawn before the hulls.
fn scene_prepass_targets(outlines: bool) -> [Option<wgpu::ColorTargetState>; 3] {
    let mask = if outlines { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::empty() };
    [
        (NORMALS_FORMAT, wgpu::ColorWrites::ALL),
        (VELOCITY_FORMAT, wgpu::ColorWrites::ALL),
        (OUTLINES_FORMAT, mask),
    ]
    .map(|(format, write_mask)| Some(wgpu::ColorTargetState { format, blend: None, write_mask }))
}

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
    /// The outline hulls in the scene prepass (M1 C4): every stream, the surface stream for the
    /// width's vertex color and the previous positions for velocity.
    OutlinePrepass,
    /// Positions only: the shadow casters (M1 R4).
    Shadow,
}

impl MeshPass {
    /// The streams the pass binds besides the draw record and positions.
    pub(crate) fn reads_shading(self) -> bool {
        !matches!(self, Self::CharacterPrepass | Self::Shadow)
    }

    pub(crate) fn reads_surface(self) -> bool {
        matches!(self, Self::Main | Self::OutlinePrepass)
    }

    pub(crate) fn reads_previous(self) -> bool {
        matches!(self, Self::ScenePrepass | Self::OutlinePrepass)
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
/// | alpha test | the material ([`MaterialFlags::ALPHA_TEST`]) | [`Coverage::AlphaTest`]: the surface and outline shaders discard below the cutoff |
/// | outline | the material ([`MaterialFlags::OUTLINE`]) | whether the draw is in the outline pass |
/// | face | the material ([`MaterialFlags::FACE`]) | [`Part::Face`] (M1 C3): the face's light split from its SDF map and the key light in head space, its masks and expression, from the character's entry; outlines are the body's |
/// | hair, eye | the material | nothing yet: hair (C5) and eyes (C6) add their parts |
/// | transparent | the material ([`MaterialFlags::TRANSPARENT`]) | [`Coverage::Transparent`]: blended over the frame after the sky, depth-tested without writing, with no outline and in no prepass |
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
        let coverage = Coverage::of(self.contains(Self::ALPHA_TEST), self.contains(Self::TRANSPARENT));
        let part = if self.contains(Self::FACE) { Part::Face } else { Part::Body };
        SurfaceVariant::character(coverage, part)
    }

    /// Whether it draws an outline hull: transparent surfaces never do.
    pub(crate) fn outlined(self) -> bool {
        self.contains(Self::OUTLINE) && !self.contains(Self::TRANSPARENT)
    }
}

/// How a surface covers what lies behind it: its render state, and the alpha test.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Coverage {
    Opaque,
    AlphaTest,
    Transparent,
}

impl Coverage {
    /// The coverages that write depth and can draw an outline.
    pub(crate) const DEPTH_WRITING: [Self; 2] = [Self::Opaque, Self::AlphaTest];

    /// A material's coverage: transparent wins over alpha-tested, which wins over opaque.
    pub(crate) const fn of(alpha_test: bool, transparent: bool) -> Self {
        if transparent {
            Self::Transparent
        } else if alpha_test {
            Self::AlphaTest
        } else {
            Self::Opaque
        }
    }

    /// The coverage of a material with `flags`.
    pub(crate) fn of_flags(flags: MaterialFlags) -> Self {
        Self::of(flags.contains(MaterialFlags::ALPHA_TEST), flags.contains(MaterialFlags::TRANSPARENT))
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// The pipeline-overridable constants the coverage sets, in `anime_character.wgsl`,
    /// `environment.wgsl` and `outline.wgsl` (which has the alpha test's alone, since it draws no
    /// transparent hulls).
    pub(crate) fn constants(self) -> &'static [(&'static str, f64)] {
        match self {
            Self::Opaque => &[],
            Self::AlphaTest => &[(override_constant::ALPHA_TEST, 1.0)],
            Self::Transparent => &[(override_constant::TRANSPARENT, 1.0)],
        }
    }
}

/// Which part of a character a surface shades, among those with shader variants of their own:
/// the body, and faces (M1 C3). Hair (C5) and eyes (C6) join with theirs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Body,
    Face,
}

impl Part {
    pub(crate) const ALL: [Self; 2] = [Self::Body, Self::Face];

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

/// The material families meshes draw with, each through pipelines of its own that bind its
/// parameter buffer in the scene group (group 2, beside the instances) and its textures in
/// group 3: the anime character family, the environment family (M1 W4) and fog cards (M1 W3),
/// whose scene group also holds the depth they fade against. Foliage (W6) joins with its shader.
pub(crate) const MESH_FAMILIES: [MaterialFamily; 3] =
    [MaterialFamily::AnimeCharacter, MaterialFamily::Environment, MaterialFamily::FogCard];

/// The fog card family's scene group binding of the depth cards fade against (`fog_card.wgsl`).
pub(crate) const FOG_CARD_DEPTH_BINDING: u32 = 2;

/// What a surface's pipeline shades with: its material family's shader, and for anime characters
/// the part that has a variant of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Shading {
    /// The anime character shader (`anime_character.wgsl`).
    Character(Part),
    /// The environment shader (`environment.wgsl`, M1 W4).
    Environment,
    /// Fog cards (`fog_card.wgsl`, M1 W3): transparent alone.
    FogCard,
}

impl Shading {
    /// The shadings drawn at every coverage, in drawing order within one: characters, then the
    /// world, whose pixels behind them depth then rejects. Fog cards are transparent alone.
    pub(crate) const SURFACES: [Self; 3] =
        [Self::Character(Part::Body), Self::Character(Part::Face), Self::Environment];

    /// Its position in [`Self::SURFACES`], or past it for fog cards.
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Character(part) => part.index(),
            Self::Environment => Part::ALL.len(),
            Self::FogCard => Self::SURFACES.len(),
        }
    }

    /// The material family it draws.
    pub(crate) fn family(self) -> MaterialFamily {
        match self {
            Self::Character(_) => MaterialFamily::AnimeCharacter,
            Self::Environment => MaterialFamily::Environment,
            Self::FogCard => MaterialFamily::FogCard,
        }
    }

    /// Its family's position in [`MESH_FAMILIES`]: the scene group its pipelines bind.
    pub(crate) fn scene_group(self) -> usize {
        let family = self.family();
        MESH_FAMILIES
            .iter()
            .position(|&mesh_family| mesh_family == family)
            .expect("meshes draw every shading")
    }
}

/// The surface pipelines one [`PositionFormat`] can draw with: a coverage, and a shading (the
/// family's shader and, for anime characters, the part). For anime characters these are the
/// permutation bits that change the shader or its render state today (see [`Permutation`]);
/// environment materials vary by their coverage alone, and fog cards are always transparent.
/// Outline hulls depend on the coverage alone, and only anime characters draw them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SurfaceVariant {
    pub coverage: Coverage,
    pub shading: Shading,
}

impl SurfaceVariant {
    /// Every variant, in drawing order: by coverage, then shading, and fog cards last.
    pub(crate) const ALL: [Self; 10] = [
        Self::character(Coverage::Opaque, Part::Body),
        Self::character(Coverage::Opaque, Part::Face),
        Self::environment(Coverage::Opaque),
        Self::character(Coverage::AlphaTest, Part::Body),
        Self::character(Coverage::AlphaTest, Part::Face),
        Self::environment(Coverage::AlphaTest),
        Self::character(Coverage::Transparent, Part::Body),
        Self::character(Coverage::Transparent, Part::Face),
        Self::environment(Coverage::Transparent),
        Self::FOG_CARD,
    ];
    /// The variants that write depth: drawn first, and into the prepasses.
    pub(crate) const DEPTH_WRITING: [Self; 6] =
        [Self::ALL[0], Self::ALL[1], Self::ALL[2], Self::ALL[3], Self::ALL[4], Self::ALL[5]];
    /// The variants that can draw an outline hull: the anime character's that write depth.
    pub(crate) const OUTLINED: [Self; 4] = [Self::ALL[0], Self::ALL[1], Self::ALL[3], Self::ALL[4]];
    /// The surfaces' variants blended over the frame after the sky; fog cards ([`Self::FOG_CARD`])
    /// blend after them, sorted back to front.
    pub(crate) const TRANSPARENT: [Self; 3] = [Self::ALL[6], Self::ALL[7], Self::ALL[8]];
    /// Fog cards (M1 W3).
    pub(crate) const FOG_CARD: Self = Self { coverage: Coverage::Transparent, shading: Shading::FogCard };
    /// Opaque anime character bodies: what a material with no permutation bits draws with.
    pub(crate) const OPAQUE: Self = Self::character(Coverage::Opaque, Part::Body);

    pub(crate) const fn character(coverage: Coverage, part: Part) -> Self {
        Self { coverage, shading: Shading::Character(part) }
    }

    pub(crate) const fn environment(coverage: Coverage) -> Self {
        Self { coverage, shading: Shading::Environment }
    }

    /// Its position in [`Self::ALL`].
    pub(crate) fn index(self) -> usize {
        match self.shading {
            Shading::FogCard => Self::ALL.len() - 1,
            shading => self.coverage.index() * Shading::SURFACES.len() + shading.index(),
        }
    }

    /// The variant a material of `family` with `flags` draws with, on any mesh; `None` for a
    /// family meshes don't draw with.
    pub(crate) fn of_material(family: MaterialFamily, flags: MaterialFlags) -> Option<Self> {
        mesh_draw(family, flags, false, false).map(|(variant, _)| variant)
    }

    /// The surface shader's static permutation constants for the variant (`PERMUTATION_*` in
    /// `anime_character.wgsl` and `environment.wgsl`). Outlines take their coverage's
    /// ([`Coverage::constants`]); environment pipelines add their tier's specular model. Fog cards
    /// have none (their pipelines add the depth fade).
    pub(crate) fn constants(self) -> &'static [(&'static str, f64)] {
        use override_constant::{ALPHA_TEST, FACE, TRANSPARENT};
        match (self.coverage, self.shading) {
            (_, Shading::FogCard) => &[],
            (coverage, Shading::Character(Part::Body) | Shading::Environment) => coverage.constants(),
            (Coverage::Opaque, Shading::Character(Part::Face)) => &[(FACE, 1.0)],
            (Coverage::AlphaTest, Shading::Character(Part::Face)) => &[(ALPHA_TEST, 1.0), (FACE, 1.0)],
            (Coverage::Transparent, Shading::Character(Part::Face)) => &[(TRANSPARENT, 1.0), (FACE, 1.0)],
        }
    }
}

/// How a submesh drawn with a material of `family` with `flags`, on a mesh that is `skinned` and
/// `morphed` or not, draws: its surface variant, and whether it draws an outline hull. Anime
/// character materials go by their static permutation ([`Permutation`]); environment materials
/// by their coverage, with no hull (decision M1 W4: the environment's edges are the spec's
/// screen-space edge pass, a later desktop option, and props that should read like characters
/// use the anime character family); fog cards are transparent, with no hull. `None` for a family
/// meshes don't draw with.
pub(crate) fn mesh_draw(
    family: MaterialFamily,
    flags: MaterialFlags,
    skinned: bool,
    morphed: bool,
) -> Option<(SurfaceVariant, bool)> {
    match family {
        MaterialFamily::AnimeCharacter => {
            let permutation = Permutation::new(flags, skinned, morphed);
            Some((permutation.surface(), permutation.outlined()))
        }
        MaterialFamily::Environment => Some((SurfaceVariant::environment(Coverage::of_flags(flags)), false)),
        MaterialFamily::FogCard => Some((SurfaceVariant::FOG_CARD, false)),
        MaterialFamily::Foliage | MaterialFamily::Sky => None,
    }
}

/// The names of the shaders' permutation constants.
pub(crate) mod override_constant {
    pub const ALPHA_TEST: &str = "PERMUTATION_ALPHA_TEST";
    pub const TRANSPARENT: &str = "PERMUTATION_TRANSPARENT";
    pub const FACE: &str = "PERMUTATION_FACE";
}

/// The bind group layouts of `shaders/frame.wgsl`, and group 3 per material family.
#[derive(Debug)]
pub(crate) struct Layouts {
    /// Group 0: frame constants, local lights and their lists, the characters' entries, and the
    /// shadows' uniforms, maps and comparison sampler (M1 R4).
    pub frame: wgpu::BindGroupLayout,
    /// Group 1: one view, bound with a dynamic offset. The light-cluster pass binds it too.
    pub view: wgpu::BindGroupLayout,
    /// Group 2 per [`MESH_FAMILIES`] entry: instances and that family's parameter buffer, and for
    /// fog cards the depth they fade against ([`FOG_CARD_DEPTH_BINDING`]). The prepasses, which
    /// read no material, bind the anime character family's.
    pub scene: [wgpu::BindGroupLayout; MESH_FAMILIES.len()],
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
        let depth = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: S::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        Self {
            frame: layout(
                "frame",
                &[
                    buffer(0, S::VERTEX_FRAGMENT, uniform, false, size_of::<FrameUniforms>()),
                    buffer(1, S::FRAGMENT, storage, false, size_of::<GpuLight>()),
                    buffer(2, S::FRAGMENT, storage, false, size_of::<u32>()),
                    buffer(3, S::FRAGMENT, storage, false, size_of::<GpuCharacter>()),
                    buffer(4, S::FRAGMENT, uniform, false, size_of::<ShadowUniforms>()),
                    depth(5, wgpu::TextureViewDimension::D2Array),
                    depth(6, wgpu::TextureViewDimension::D2),
                    wgpu::BindGroupLayoutEntry {
                        binding: 7,
                        visibility: S::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                ],
            ),
            view: layout(
                "view",
                &[buffer(0, S::VERTEX_FRAGMENT | S::COMPUTE, uniform, true, size_of::<ViewUniforms>())],
            ),
            scene: MESH_FAMILIES.map(|family| {
                let entries = [
                    buffer(0, S::VERTEX, storage, false, size_of::<InstanceData>()),
                    // The family's own parameter buffer: its pipelines read nothing else.
                    buffer(1, S::VERTEX_FRAGMENT, storage, false, family.params_size()),
                    depth(FOG_CARD_DEPTH_BINDING, wgpu::TextureViewDimension::D2),
                ];
                let bindings = if family == MaterialFamily::FogCard { entries.len() } else { 2 };
                layout(family.name(), &entries[..bindings])
            }),
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

/// Transparent surfaces: blended by coverage, keeping the scene color's alpha, which opaque
/// surfaces set (post's character mask, M1 P2: the world's share of the pixel).
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
    /// The surfaces, anime character and environment, per [`SurfaceVariant::ALL`] entry; each is
    /// built with the first draw that needs it ([`Pipelines::prepare_variant`]).
    surfaces: [Option<wgpu::RenderPipeline>; SurfaceVariant::ALL.len()],
    /// The anime characters' outline hulls, per [`Coverage::DEPTH_WRITING`] entry: every part's
    /// outline is the same.
    outline: [Option<wgpu::RenderPipeline>; Coverage::DEPTH_WRITING.len()],
    /// Depth, world normals and velocity of every opaque instance (`prepass.wgsl`), whatever its
    /// family.
    pub scene_prepass: wgpu::RenderPipeline,
    /// The outline hulls into the scene prepass's targets (`outline.wgsl`), built with the
    /// format's first outline: one for every coverage, since the prepass doesn't cut
    /// alpha-tested surfaces out.
    outline_prepass: Option<wgpu::RenderPipeline>,
}

impl MeshPipelines {
    /// The surface pipeline of `variant`, once it was prepared.
    pub(crate) fn surface(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        self.surfaces[variant.index()].as_ref()
    }

    /// The outline pipeline of `variant`'s coverage, once it was prepared; transparent surfaces
    /// and the environment have none.
    pub(crate) fn outline(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        if variant.shading == Shading::Environment {
            return None;
        }
        self.outline.get(variant.coverage.index())?.as_ref()
    }

    /// The outline hulls' scene prepass pipeline, once a draw with an outline was prepared.
    pub(crate) fn outline_prepass(&self) -> Option<&wgpu::RenderPipeline> {
        self.outline_prepass.as_ref()
    }

    /// Pipelines built of `family`: its surfaces, and the anime character family's outlines.
    fn built(&self, family: MaterialFamily) -> u32 {
        let surfaces = SurfaceVariant::ALL
            .iter()
            .filter(|variant| variant.shading.family() == family && self.surface(**variant).is_some())
            .count();
        let outlines = match family {
            MaterialFamily::AnimeCharacter => self.outline.iter().flatten().count(),
            _ => 0,
        };
        (surfaces + outlines) as u32
    }
}

/// The mesh pipelines (the sky's are the `sky` module's). Quantized-mesh pipelines are built up
/// front, opaque anime character surfaces only; the f32 ones when the first f32 mesh, or skinned
/// or morphed mesh, is uploaded ([`Self::prepare_format`]); the other surface variants, the
/// environment's among them, when a draw first needs them ([`Self::prepare_variant`]); and the
/// character prepass's with the first skinned or morphed mesh ([`Self::prepare_characters`]).
/// Apps never pay for compiling what they don't draw.
///
/// They are built from the renderer's current shader sources, whose modules they keep; new
/// sources (hot reload) rebuild the same set from new modules ([`Self::rebuilt`]).
#[derive(Debug)]
pub(crate) struct Pipelines {
    /// Per [`PositionFormat::ALL`] entry.
    mesh: [Option<MeshPipelines>; 2],
    /// Depth of skinned and morphed instances, whose skinning outputs are f32.
    character_prepass: Option<wgpu::RenderPipeline>,
    target: PipelineTarget,
    modules: MeshModules,
}

/// The modules the mesh pipelines are built from.
#[derive(Clone, Debug)]
struct MeshModules {
    character: wgpu::ShaderModule,
    environment: wgpu::ShaderModule,
    fog_card: wgpu::ShaderModule,
    outline: wgpu::ShaderModule,
    prepass: wgpu::ShaderModule,
}

impl MeshModules {
    /// The module of `program`, one of the mesh programs.
    fn get_mut(&mut self, program: Program) -> &mut wgpu::ShaderModule {
        match program {
            Program::AnimeCharacter => &mut self.character,
            Program::Environment => &mut self.environment,
            Program::FogCard => &mut self.fog_card,
            Program::Outline => &mut self.outline,
            Program::Prepass => &mut self.prepass,
            Program::Sky
            | Program::Clusters
            | Program::Skinning
            | Program::Post
            | Program::Temporal
            | Program::Shadows => unreachable!("not a mesh program"),
        }
    }

    /// The module a surface of `shading` draws with, and the program it is built from.
    fn surface(&self, shading: Shading) -> (&wgpu::ShaderModule, Program) {
        match shading {
            Shading::Character(_) => (&self.character, Program::AnimeCharacter),
            Shading::Environment => (&self.environment, Program::Environment),
            Shading::FogCard => (&self.fog_card, Program::FogCard),
        }
    }
}

/// What every scene pipeline shares, kept to build the f32 ones later.
#[derive(Clone, Debug)]
struct PipelineTarget {
    /// Groups 0–2, for passes that read no material textures: the prepasses.
    layout: wgpu::PipelineLayout,
    /// Groups 0–3 per [`MESH_FAMILIES`] entry, with that family's scene group and group 3: the
    /// passes that shade with it (its surfaces, and the anime character family's outlines).
    family_layouts: [wgpu::PipelineLayout; MESH_FAMILIES.len()],
    color_format: wgpu::TextureFormat,
    sample_count: u32,
    /// The environment shader's tier settings: its specular model.
    environment: EnvironmentSettings,
    cache: Option<wgpu::PipelineCache>,
    /// The receivers' shadow constants (`frame.wgsl`'s `SHADOWS` and `SHADOW_TENT`), which every
    /// surface and outline takes with its permutation's; none with shadows off, which keeps the
    /// shaders as they were without them.
    shadows: Option<[(&'static str, f64); 2]>,
    /// Fog cards fade against the scene prepass's depth (`fog::FogCards`).
    card_depth_fade: bool,
}

impl Pipelines {
    #[expect(
        clippy::too_many_arguments,
        reason = "the device's targets and the tier's shader settings, each read once"
    )]
    pub(crate) fn new(
        device: &wgpu::Device,
        layouts: &Layouts,
        color_format: wgpu::TextureFormat,
        sample_count: u32,
        environment: EnvironmentSettings,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
        shadows: Option<&ShadowSettings>,
        card_depth_fade: bool,
    ) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.view), Some(&layouts.scene[0])],
            immediate_size: 0,
        });
        let family_layouts = std::array::from_fn(|group| {
            let family = MESH_FAMILIES[group];
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(family.name()),
                bind_group_layouts: &[
                    Some(&layouts.frame),
                    Some(&layouts.view),
                    Some(&layouts.scene[group]),
                    Some(&layouts.materials[family.index()]),
                ],
                immediate_size: 0,
            })
        });
        let target = PipelineTarget {
            layout,
            family_layouts,
            color_format,
            sample_count,
            environment,
            cache: cache.cloned(),
            shadows: shadows.map(ShadowSettings::constants),
            card_depth_fade,
        };
        let modules = MeshModules {
            character: shaders::module(device, Program::AnimeCharacter, sources),
            environment: shaders::module(device, Program::Environment, sources),
            fog_card: shaders::module(device, Program::FogCard, sources),
            outline: shaders::module(device, Program::Outline, sources),
            prepass: shaders::module(device, Program::Prepass, sources),
        };
        let mut pipelines = Self { mesh: [None, None], character_prepass: None, target, modules };
        pipelines.prepare_format(device, PositionFormat::Unorm16);
        pipelines
    }

    /// The same pipelines as these, with the programs in `changed` (mesh programs among them)
    /// built anew from `sources`: new modules, and every pipeline built from them so far rebuilt
    /// from those. The others are shared. Returns them with the number of pipelines built.
    pub(crate) fn rebuilt(
        &self,
        device: &wgpu::Device,
        sources: &ShaderSources,
        changed: &[Program],
    ) -> (Self, u32) {
        let mut modules = self.modules.clone();
        for &program in changed.iter().filter(|program| program.is_mesh()) {
            *modules.get_mut(program) = shaders::module(device, program, sources);
        }
        let (target, mut built) = (&self.target, 0);
        let mut rebuild =
            |program: Program, old: &wgpu::RenderPipeline, build: &dyn Fn() -> wgpu::RenderPipeline| {
                if changed.contains(&program) {
                    built += 1;
                    build()
                } else {
                    old.clone()
                }
            };
        let character_prepass = self
            .character_prepass
            .as_ref()
            .map(|old| rebuild(Program::Prepass, old, &|| build_character_prepass(device, target, &modules)));
        let mut mesh = [None, None];
        for format in PositionFormat::ALL {
            let Some(old) = self.mesh(format) else { continue };
            let scene_prepass = rebuild(Program::Prepass, &old.scene_prepass, &|| {
                build_scene_prepass(device, target, &modules, format)
            });
            let outline_prepass = old.outline_prepass.as_ref().map(|old| {
                rebuild(Program::Outline, old, &|| build_outline_prepass(device, target, &modules, format))
            });
            let mut pipelines = MeshPipelines {
                surfaces: Default::default(),
                outline: Default::default(),
                scene_prepass,
                outline_prepass,
            };
            for variant in SurfaceVariant::ALL {
                if let Some(old) = old.surface(variant) {
                    let (_, program) = modules.surface(variant.shading);
                    pipelines.surfaces[variant.index()] = Some(rebuild(program, old, &|| {
                        build_surface(device, target, &modules, format, variant)
                    }));
                }
            }
            for coverage in Coverage::DEPTH_WRITING {
                if let Some(old) = &old.outline[coverage.index()] {
                    pipelines.outline[coverage.index()] = Some(rebuild(Program::Outline, old, &|| {
                        build_outline(device, target, &modules, format, coverage)
                    }));
                }
            }
            mesh[format_index(format)] = Some(pipelines);
        }
        let pipelines = Self { mesh, character_prepass, target: self.target.clone(), modules };
        (pipelines, built)
    }

    /// Builds the mesh pipelines for `format` unless they exist: the scene prepass, and the
    /// opaque anime character surfaces and their outlines.
    pub(crate) fn prepare_format(&mut self, device: &wgpu::Device, format: PositionFormat) {
        if self.mesh[format_index(format)].is_some() {
            return;
        }
        let scene_prepass = build_scene_prepass(device, &self.target, &self.modules, format);
        self.mesh[format_index(format)] = Some(MeshPipelines {
            surfaces: Default::default(),
            outline: Default::default(),
            scene_prepass,
            outline_prepass: None,
        });
        self.prepare_variant(device, format, SurfaceVariant::OPAQUE);
    }

    /// Builds the surface pipeline of `variant` for `format`, whose mesh pipelines must exist,
    /// and for anime characters the outline pipeline of its coverage and the outline hulls'
    /// prepass pipeline, unless they do already.
    pub(crate) fn prepare_variant(
        &mut self,
        device: &wgpu::Device,
        format: PositionFormat,
        variant: SurfaceVariant,
    ) {
        let mesh =
            self.mesh[format_index(format)].as_ref().expect("prepare_format built the format's pipelines");
        if mesh.surface(variant).is_some() {
            return;
        }
        let outlined = SurfaceVariant::OUTLINED.contains(&variant);
        let needs_outline = outlined && mesh.outline(variant).is_none();
        let needs_outline_prepass = outlined && mesh.outline_prepass.is_none();
        let surface = build_surface(device, &self.target, &self.modules, format, variant);
        // Transparent surfaces and the environment have no outline; every part shares its
        // coverage's.
        let outline = needs_outline
            .then(|| build_outline(device, &self.target, &self.modules, format, variant.coverage));
        let outline_prepass =
            needs_outline_prepass.then(|| build_outline_prepass(device, &self.target, &self.modules, format));
        let mesh = self.mesh[format_index(format)].as_mut().expect("checked above");
        mesh.surfaces[variant.index()] = Some(surface);
        if let Some(outline) = outline {
            mesh.outline[variant.coverage.index()] = Some(outline);
        }
        if outline_prepass.is_some() {
            mesh.outline_prepass = outline_prepass;
        }
    }

    /// Pipelines of `family` built so far, across formats: anime character surfaces and
    /// outlines, or the environment's or fog cards' surfaces.
    pub(crate) fn built(&self, family: MaterialFamily) -> u32 {
        self.mesh.iter().flatten().map(|mesh| mesh.built(family)).sum()
    }

    /// Builds the character prepass's pipeline unless it exists: for the first skinned or
    /// morphed mesh.
    pub(crate) fn prepare_characters(&mut self, device: &wgpu::Device) {
        if self.character_prepass.is_none() {
            self.character_prepass = Some(build_character_prepass(device, &self.target, &self.modules));
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

/// The surface pipeline of `variant` for meshes in `format`: the anime character shader, the
/// environment shader or fog cards', on its family's layout, with the variant's permutation
/// constants (and the tier's specular model for the environment, the depth fade for fog cards).
fn build_surface(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
    variant: SurfaceVariant,
) -> wgpu::RenderPipeline {
    let transparent = variant.coverage == Coverage::Transparent;
    let (module, _) = modules.surface(variant.shading);
    // The permutation's constants, then the shadows' (M1 R4), which fog cards don't receive.
    let mut constants = variant.constants().to_vec();
    if variant.shading != Shading::FogCard {
        constants.extend(target.shadows.iter().flatten());
    }
    let name = match variant.shading {
        Shading::Character(_) => "anime character",
        Shading::Environment => {
            constants.push((environment::SPECULAR_CONSTANT, target.environment.specular.constant()));
            "environment"
        }
        Shading::FogCard => {
            constants.extend(fog::card_constants(target.card_depth_fade));
            "fog card"
        }
    };
    target.build_with(
        device,
        &target.family_layouts[variant.shading.scene_group()],
        PassDesc {
            name,
            module,
            vertex_buffers: &MeshPass::Main.buffers(format),
            // Cards are seen from either side.
            cull: (variant.shading != Shading::FogCard).then_some(wgpu::Face::Back),
            // Transparent surfaces are depth-tested against the opaque frame but don't hide
            // what lies behind them.
            depth_write: !transparent,
            depth_compare: wgpu::CompareFunction::Greater,
            blend: transparent.then_some(TRANSPARENT_BLEND),
            constants: &constants,
        },
    )
}

/// The outline hull pipeline of `coverage`, one that writes depth, for meshes in `format`.
fn build_outline(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
    coverage: Coverage,
) -> wgpu::RenderPipeline {
    // The coverage's constants, then the shadows' (M1 R4): lit lines receive them.
    let mut constants = coverage.constants().to_vec();
    constants.extend(target.shadows.iter().flatten());
    // Inverted hull: keep only the back faces of the extruded shell.
    target.build_with(
        device,
        &target.family_layouts[Shading::Character(Part::Body).scene_group()],
        PassDesc {
            name: "outline",
            module: &modules.outline,
            vertex_buffers: &MeshPass::Main.buffers(format),
            cull: Some(wgpu::Face::Front),
            depth_write: true,
            depth_compare: wgpu::CompareFunction::Greater,
            blend: None,
            constants: &constants,
        },
    )
}

/// The scene prepass pipeline for meshes in `format`.
fn build_scene_prepass(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
) -> wgpu::RenderPipeline {
    target.build_prepass(
        device,
        "scene prepass",
        &modules.prepass,
        &MeshPass::ScenePrepass.buffers(format),
        wgpu::Face::Back,
        &scene_prepass_targets(false),
    )
}

/// The outline hulls' scene prepass pipeline for meshes in `format`: the scene prepass's
/// targets, the outline mask included, with the hulls' front faces culled as in the main pass.
fn build_outline_prepass(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
) -> wgpu::RenderPipeline {
    target.build_prepass(
        device,
        "outline prepass",
        &modules.outline,
        &MeshPass::OutlinePrepass.buffers(format),
        wgpu::Face::Front,
        &scene_prepass_targets(true),
    )
}

/// The character prepass pipeline: skinning outputs are f32.
fn build_character_prepass(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
) -> wgpu::RenderPipeline {
    let buffers = MeshPass::CharacterPrepass.buffers(PositionFormat::Float32);
    target.build_prepass(device, "character prepass", &modules.prepass, &buffers, wgpu::Face::Back, &[])
}

impl PipelineTarget {
    /// A prepass pipeline: `cull` faces culled, reverse-Z depth written, no multisampling (the
    /// targets are sampled by later passes), and `color_targets` written by the
    /// `fs_<entry>` entry point when there are any. `name` is the entry points' suffix with
    /// spaces as underscores.
    fn build_prepass(
        &self,
        device: &wgpu::Device,
        name: &'static str,
        module: &wgpu::ShaderModule,
        vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
        cull: wgpu::Face,
        color_targets: &[Option<wgpu::ColorTargetState>],
    ) -> wgpu::RenderPipeline {
        let entry = name.replace(' ', "_");
        let (vs, fs) = (format!("vs_{entry}"), format!("fs_{entry}"));
        let targets = color_targets.len();
        tracing::debug!(target: LOG_TARGET, pipeline = name, targets, "building a mesh pipeline");
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                cull_mode: Some(cull),
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
        });
        tracing::debug!(target: LOG_TARGET, pipeline = name, "built a mesh pipeline");
        pipeline
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
        let (pipeline, samples) = (pass.name, self.sample_count);
        tracing::debug!(target: LOG_TARGET, pipeline, samples, constants = ?pass.constants, "building a mesh pipeline");
        let built = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        });
        tracing::debug!(target: LOG_TARGET, pipeline, "built a mesh pipeline");
        built
    }
}

/// The index of `format` in [`PositionFormat::ALL`].
pub(crate) fn format_index(format: PositionFormat) -> usize {
    PositionFormat::ALL.iter().position(|&f| f == format).expect("ALL lists every format")
}
