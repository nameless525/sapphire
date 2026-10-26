//! Bind group layouts, vertex layouts and render pipelines for the scene passes and the
//! prepasses: the surface variants of the material families meshes draw with (the anime character
//! shader's static permutations, the environment shader's, fog cards' and foliage's), the anime
//! characters' outline hulls, and foliage's own scene prepass and shadow casters, which displace
//! by the wind and cut alpha-tested cards out.

use std::mem::offset_of;
use std::num::NonZeroU64;

use sapphire_assets::material::{MaterialFamily, MaterialFlags};
use sapphire_assets::mesh::{PositionFormat, VertexFoliage, VertexShading, VertexSurface};
use sapphire_rhi::wgpu;

use crate::LOG_TARGET;
use crate::ambient;
use crate::character_prepass;
use crate::characters::GpuCharacter;
use crate::environment::{self, EnvironmentSettings};
use crate::fog;
use crate::foliage::{self, FoliageSettings, WindUniforms};
use crate::gpu_types::{Dequantize, DrawData, FrameUniforms, InstanceData, ViewUniforms};
use crate::lights::GpuLight;
use crate::shaders::{self, Program, ShaderSources};
use crate::shadows::{self as shadow, ShadowSettings, ShadowUniforms};

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
/// pass (shadows, the character prepass) needs nothing more, so the streams it skips come last,
/// foliage's own stream after them all.
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
    /// The foliage stream (M1 W6): wind weights, phase and gradient, of foliage passes alone.
    pub const FOLIAGE: u32 = 5;
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

/// Slot 5: [`VertexFoliage`], its weights and phase, then its gradient and reserved bytes.
pub(crate) const FOLIAGE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    [attribute(11, wgpu::VertexFormat::Unorm8x4, 0), attribute(12, wgpu::VertexFormat::Unorm8x4, 4)];

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
    /// Foliage's main pass draws (M1 W6): the main pass's streams and the foliage stream.
    Foliage,
    /// Foliage in the scene prepass: every stream, the surface stream for the alpha test's UVs
    /// and the previous positions for velocity.
    FoliagePrepass,
    /// Foliage's shadow casters: positions, the surface stream's UVs and the foliage stream.
    FoliageShadow,
}

impl MeshPass {
    /// The streams the pass binds besides the draw record and positions.
    pub(crate) fn reads_shading(self) -> bool {
        !matches!(self, Self::CharacterPrepass | Self::Shadow | Self::FoliageShadow)
    }

    pub(crate) fn reads_surface(self) -> bool {
        matches!(
            self,
            Self::Main | Self::OutlinePrepass | Self::Foliage | Self::FoliagePrepass | Self::FoliageShadow
        )
    }

    pub(crate) fn reads_previous(self) -> bool {
        matches!(self, Self::ScenePrepass | Self::OutlinePrepass | Self::FoliagePrepass)
    }

    pub(crate) fn reads_foliage(self) -> bool {
        matches!(self, Self::Foliage | Self::FoliagePrepass | Self::FoliageShadow)
    }

    /// Whether the pass shades with materials, binding each draw's group 3: the main pass, and
    /// foliage's every pass, whose alpha test reads the base color map; the other prepasses' and
    /// casters' layouts leave group 3 out.
    pub(crate) fn binds_materials(self) -> bool {
        matches!(self, Self::Main | Self::Foliage | Self::FoliagePrepass | Self::FoliageShadow)
    }

    /// The pass's vertex buffers for a mesh in `format`, in slot order.
    pub(crate) fn buffers(self, format: PositionFormat) -> [Option<wgpu::VertexBufferLayout<'static>>; 6] {
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
            if self.reads_foliage() { vertex(size_of::<VertexFoliage>(), &FOLIAGE_ATTRIBUTES) } else { None },
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
/// | face | the material ([`MaterialFlags::FACE`]) | [`Part::Face`] (M1 C3): the face's light split from its SDF map and the key light in head space, its masks and expression, from the character's entry; the bangs' shadow on it (C5); outlines are the body's |
/// | hair | the material ([`MaterialFlags::HAIR`]) | [`Part::Hair`] (M1 C5): the hair mask's occlusion and the highlight band, painted, angel ring or Kajiya-Kay by a branch on the block |
/// | eye | the material ([`MaterialFlags::EYE`]) | [`Part::Eye`] (M1 C5): always lit, no rim; C6 layers it |
/// | transparent | the material ([`MaterialFlags::TRANSPARENT`]) | [`Coverage::Transparent`]: blended over the frame after the sky, depth-tested without writing, with no outline and in no prepass |
///
/// Eyes through hair ([`MaterialFlags::THROUGH_HAIR`], M1 C5) is no permutation bit: like the
/// outline, it adds the draw to a pass, the eyes-through-hair draws after the outlines, through
/// a pipeline of the face or eye variant's own.
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

    /// The surface pipelines it draws with. A material is one part, so at most one of the face,
    /// hair and eye bits is set.
    pub(crate) fn surface(self) -> SurfaceVariant {
        let coverage = Coverage::of(self.contains(Self::ALPHA_TEST), self.contains(Self::TRANSPARENT));
        let part = if self.contains(Self::FACE) {
            Part::Face
        } else if self.contains(Self::HAIR) {
            Part::Hair
        } else if self.contains(Self::EYE) {
            Part::Eye
        } else {
            Part::Body
        };
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

/// Which part of a character a surface shades, each a shader variant of its own: the body, faces
/// (M1 C3), hair and eyes (M1 C5; C6 layers eyes). In the order of the material's
/// `AnimeCharacterParams::SURFACE_*`, which the character prepass writes as the pixel's part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Body,
    Face,
    Hair,
    Eye,
}

impl Part {
    pub(crate) const ALL: [Self; 4] = [Self::Body, Self::Face, Self::Hair, Self::Eye];

    pub(crate) fn index(self) -> usize {
        self as usize
    }

    /// Its permutation constant, if it has one.
    fn constant(self) -> Option<&'static str> {
        match self {
            Self::Body => None,
            Self::Face => Some(override_constant::FACE),
            Self::Hair => Some(override_constant::HAIR),
            Self::Eye => Some(override_constant::EYE),
        }
    }
}

/// The material families meshes draw with, each through pipelines of its own that bind its
/// parameter buffer in the scene group (group 2, beside the instances) and its textures in
/// group 3: the anime character family, the environment family (M1 W4), fog cards (M1 W3), whose
/// scene group also holds the depth they fade against, and foliage (M1 W6), whose scene group also
/// holds the frame's wind.
pub(crate) const MESH_FAMILIES: [MaterialFamily; 4] = [
    MaterialFamily::AnimeCharacter,
    MaterialFamily::Environment,
    MaterialFamily::FogCard,
    MaterialFamily::Foliage,
];

/// The fog card family's scene group binding of the depth cards fade against (`fog_card.wgsl`).
pub(crate) const FOG_CARD_DEPTH_BINDING: u32 = 2;

/// The foliage family's scene group binding of the frame's wind ([`WindUniforms`], `foliage.wgsl`).
pub(crate) const FOLIAGE_WIND_BINDING: u32 = 2;

/// What a surface's pipeline shades with: its material family's shader, and for anime characters
/// the part that has a variant of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Shading {
    /// The anime character shader (`anime_character.wgsl`).
    Character(Part),
    /// The environment shader (`environment.wgsl`, M1 W4).
    Environment,
    /// The foliage shader (`foliage.wgsl`, M1 W6).
    Foliage,
    /// Fog cards (`fog_card.wgsl`, M1 W3): transparent alone.
    FogCard,
}

impl Shading {
    /// The shadings drawn at every coverage, in drawing order within one: characters, then the
    /// world (the environment, then foliage), whose pixels behind them depth then rejects.
    /// Foliage is never transparent: its variant of that coverage has no draws and no pipeline.
    /// Fog cards are transparent alone.
    pub(crate) const SURFACES: [Self; 6] = [
        Self::Character(Part::Body),
        Self::Character(Part::Face),
        Self::Character(Part::Hair),
        Self::Character(Part::Eye),
        Self::Environment,
        Self::Foliage,
    ];

    /// Its position in [`Self::SURFACES`], or past it for fog cards.
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Character(part) => part.index(),
            Self::Environment => Part::ALL.len(),
            Self::Foliage => Part::ALL.len() + 1,
            Self::FogCard => Self::SURFACES.len(),
        }
    }

    /// The material family it draws.
    pub(crate) fn family(self) -> MaterialFamily {
        match self {
            Self::Character(_) => MaterialFamily::AnimeCharacter,
            Self::Environment => MaterialFamily::Environment,
            Self::Foliage => MaterialFamily::Foliage,
            Self::FogCard => MaterialFamily::FogCard,
        }
    }

    /// The streams its main pass draws read: foliage's add the foliage stream.
    pub(crate) fn main_pass(self) -> MeshPass {
        if self == Self::Foliage { MeshPass::Foliage } else { MeshPass::Main }
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
/// environment and foliage materials vary by their coverage alone, foliage is never transparent
/// (its variant of that coverage has no draws and no pipeline), and fog cards are always
/// transparent. Outline hulls depend on the coverage alone, and only anime characters draw them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SurfaceVariant {
    pub coverage: Coverage,
    pub shading: Shading,
}

impl SurfaceVariant {
    /// Every variant, in drawing order: by coverage, then shading, and fog cards last.
    pub(crate) const ALL: [Self; 19] = {
        let mut all = [Self::FOG_CARD; 19];
        let coverages = [Coverage::Opaque, Coverage::AlphaTest, Coverage::Transparent];
        let mut index = 0;
        while index < coverages.len() * Shading::SURFACES.len() {
            let coverage = coverages[index / Shading::SURFACES.len()];
            all[index] = Self { coverage, shading: Shading::SURFACES[index % Shading::SURFACES.len()] };
            index += 1;
        }
        all
    };
    /// The variants that write depth: drawn first, and into the prepasses and shadow maps.
    pub(crate) const DEPTH_WRITING: [Self; 12] = {
        let mut variants = [Self::OPAQUE; 12];
        let mut index = 0;
        while index < variants.len() {
            variants[index] = Self::ALL[index];
            index += 1;
        }
        variants
    };
    /// The depth-writing variants the shared scene prepass and shadow caster pipelines draw:
    /// every family's but foliage's, whose own pipelines displace by the wind and cut alpha-tested
    /// cards out ([`Self::FOLIAGE`]).
    pub(crate) const SHARED_DEPTH: [Self; 10] = {
        let mut variants = [Self::OPAQUE; 10];
        let (mut from, mut to) = (0, 0);
        while from < Self::DEPTH_WRITING.len() {
            if !matches!(Self::DEPTH_WRITING[from].shading, Shading::Foliage) {
                variants[to] = Self::DEPTH_WRITING[from];
                to += 1;
            }
            from += 1;
        }
        assert!(to == variants.len(), "every depth-writing variant but foliage's");
        variants
    };
    /// Foliage's variants, opaque and alpha-tested (M1 W6).
    pub(crate) const FOLIAGE: [Self; 2] =
        [Self::foliage(Coverage::Opaque), Self::foliage(Coverage::AlphaTest)];
    /// The anime character variants that write depth: what the character prepass draws, and
    /// the variants that can draw an outline hull.
    pub(crate) const CHARACTER_DEPTH_WRITING: [Self; 8] = {
        let mut variants = [Self::OPAQUE; 8];
        let mut index = 0;
        while index < variants.len() {
            let coverage = if index < Part::ALL.len() { Coverage::Opaque } else { Coverage::AlphaTest };
            variants[index] = Self::character(coverage, Part::ALL[index % Part::ALL.len()]);
            index += 1;
        }
        variants
    };
    /// The variants that can draw an outline hull: the anime character's that write depth.
    pub(crate) const OUTLINED: [Self; 8] = Self::CHARACTER_DEPTH_WRITING;
    /// The surfaces' variants blended over the frame after the sky (foliage's never has draws);
    /// fog cards ([`Self::FOG_CARD`]) blend after them, sorted back to front.
    pub(crate) const TRANSPARENT: [Self; 6] = {
        let mut variants = [Self::OPAQUE; 6];
        let mut index = 0;
        while index < variants.len() {
            variants[index] = Self::ALL[Self::DEPTH_WRITING.len() + index];
            index += 1;
        }
        variants
    };
    /// Fog cards (M1 W3).
    pub(crate) const FOG_CARD: Self = Self { coverage: Coverage::Transparent, shading: Shading::FogCard };
    /// The variants whose draws can draw eyes through the character's hair
    /// ([`MaterialFlags::THROUGH_HAIR`]): faces and eyes that write depth.
    pub(crate) const THROUGH_HAIR: [Self; 4] = [
        Self::character(Coverage::Opaque, Part::Face),
        Self::character(Coverage::Opaque, Part::Eye),
        Self::character(Coverage::AlphaTest, Part::Face),
        Self::character(Coverage::AlphaTest, Part::Eye),
    ];
    /// Opaque anime character bodies: what a material with no permutation bits draws with.
    pub(crate) const OPAQUE: Self = Self::character(Coverage::Opaque, Part::Body);

    pub(crate) const fn character(coverage: Coverage, part: Part) -> Self {
        Self { coverage, shading: Shading::Character(part) }
    }

    pub(crate) const fn environment(coverage: Coverage) -> Self {
        Self { coverage, shading: Shading::Environment }
    }

    pub(crate) const fn foliage(coverage: Coverage) -> Self {
        Self { coverage, shading: Shading::Foliage }
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
    /// `anime_character.wgsl`, `environment.wgsl` and `foliage.wgsl`): its coverage's
    /// ([`Coverage::constants`], which outlines take alone), then its part's. Environment
    /// pipelines add their tier's specular model, foliage's their tier's settings. Fog cards have
    /// none (their pipelines add the depth fade).
    pub(crate) fn constants(self) -> Vec<(&'static str, f64)> {
        debug_assert!(
            self != Self::foliage(Coverage::Transparent),
            "foliage is never transparent (`mesh_draw`)"
        );
        let mut constants = match self.shading {
            Shading::FogCard => Vec::new(),
            _ => self.coverage.constants().to_vec(),
        };
        if let Shading::Character(part) = self.shading {
            constants.extend(part.constant().map(|name| (name, 1.0)));
        }
        constants
    }

    /// Its position in [`Self::THROUGH_HAIR`], if it is there.
    pub(crate) fn through_hair_index(self) -> Option<usize> {
        Self::THROUGH_HAIR.iter().position(|&variant| variant == self)
    }
}

/// How a submesh drawn with a material of `family` with `flags`, on a mesh that is `skinned` and
/// `morphed` or not, draws: its surface variant, and whether it draws an outline hull. Anime
/// character materials go by their static permutation ([`Permutation`]); environment materials
/// by their coverage, with no hull (decision M1 W4: the environment's edges are the spec's
/// screen-space edge pass, a later desktop option, and props that should read like characters
/// use the anime character family); foliage by its alpha test, never transparent and with no
/// hull, on static meshes only (uploads refuse it on skinned or morphed ones); fog cards are
/// transparent, with no hull. `None` for a family meshes don't draw with.
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
        MaterialFamily::Foliage if !skinned && !morphed => {
            let coverage = Coverage::of(flags.contains(MaterialFlags::ALPHA_TEST), false);
            Some((SurfaceVariant::foliage(coverage), false))
        }
        MaterialFamily::Foliage | MaterialFamily::Sky => None,
    }
}

/// The names of the shaders' permutation constants.
pub(crate) mod override_constant {
    pub const ALPHA_TEST: &str = "PERMUTATION_ALPHA_TEST";
    pub const TRANSPARENT: &str = "PERMUTATION_TRANSPARENT";
    pub const FACE: &str = "PERMUTATION_FACE";
    pub const HAIR: &str = "PERMUTATION_HAIR";
    pub const EYE: &str = "PERMUTATION_EYE";
}

/// The bind group layouts of `shaders/frame.wgsl`, and group 3 per material family.
#[derive(Debug)]
pub(crate) struct Layouts {
    /// Group 0: frame constants, local lights and their lists, the characters' entries, the
    /// shadows' uniforms, maps and comparison sampler (M1 R4), the character prepass's depth and
    /// IDs with the sampler that gathers them (M1 C5), and the ambient uniforms, probe atlas, brick
    /// table, reflection cube, GTAO and their sampler (M1 W5).
    pub frame: wgpu::BindGroupLayout,
    /// Group 1: one view, bound with a dynamic offset. The light-cluster pass binds it too.
    pub view: wgpu::BindGroupLayout,
    /// Group 2 per [`MESH_FAMILIES`] entry: instances and that family's parameter buffer, and for
    /// fog cards the depth they fade against ([`FOG_CARD_DEPTH_BINDING`]), for foliage the wind
    /// ([`FOLIAGE_WIND_BINDING`]). The prepasses and shadow casters, which read no material, bind
    /// the anime character family's; foliage's own bind foliage's.
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
        let frame = [
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
            depth(8, wgpu::TextureViewDimension::D2),
            wgpu::BindGroupLayoutEntry {
                binding: 9,
                visibility: S::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 10,
                visibility: S::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            },
        ];
        let frame: Vec<wgpu::BindGroupLayoutEntry> =
            frame.into_iter().chain(ambient::frame_layout_entries()).collect();
        Self {
            frame: layout("frame", &frame),
            view: layout(
                "view",
                &[buffer(0, S::VERTEX_FRAGMENT | S::COMPUTE, uniform, true, size_of::<ViewUniforms>())],
            ),
            scene: MESH_FAMILIES.map(|family| {
                let mut entries = vec![
                    buffer(0, S::VERTEX, storage, false, size_of::<InstanceData>()),
                    // The family's own parameter buffer: its pipelines read nothing else.
                    buffer(1, S::VERTEX_FRAGMENT, storage, false, family.params_size()),
                ];
                match family {
                    MaterialFamily::FogCard => {
                        entries.push(depth(FOG_CARD_DEPTH_BINDING, wgpu::TextureViewDimension::D2));
                    }
                    MaterialFamily::Foliage => {
                        let wind = buffer(
                            FOLIAGE_WIND_BINDING,
                            S::VERTEX,
                            uniform,
                            false,
                            size_of::<WindUniforms>(),
                        );
                        entries.push(wind);
                    }
                    _ => {}
                }
                layout(family.name(), &entries)
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
/// `fs_<name>`, with spaces in `name` as underscores, unless `fragment` names another.
struct PassDesc<'a> {
    name: &'static str,
    /// The fragment entry point, when it isn't `fs_<name>`.
    fragment: Option<&'static str>,
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

/// Transparent surfaces, and eyes through hair: blended by coverage, keeping the scene color's
/// alpha, which opaque surfaces set (post's character mask, M1 P2: the world's share of the
/// pixel).
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
    /// The surfaces of every family, per [`SurfaceVariant::ALL`] entry; each is built with the
    /// first draw that needs it ([`Pipelines::prepare_variant`]).
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
    /// The characters' depth and IDs (`prepass.wgsl`, M1 C5), whatever their variant.
    pub character_prepass: wgpu::RenderPipeline,
    /// Eyes and brows through the character's hair (M1 C5), per [`SurfaceVariant::THROUGH_HAIR`]
    /// entry, built with the first draw of a face or eye material that draws through.
    through_hair: [Option<wgpu::RenderPipeline>; SurfaceVariant::THROUGH_HAIR.len()],
    /// Foliage into the scene prepass (`foliage.wgsl`), per [`Coverage::DEPTH_WRITING`] entry,
    /// built with that coverage's surface: displaced by the wind, its velocity with it, and
    /// alpha-tested cards cut out.
    foliage_prepass: [Option<wgpu::RenderPipeline>; Coverage::DEPTH_WRITING.len()],
    /// Foliage's shadow casters likewise, while shadows are on: displaced alike, and cut out.
    foliage_casters: [Option<wgpu::RenderPipeline>; Coverage::DEPTH_WRITING.len()],
}

impl MeshPipelines {
    /// The surface pipeline of `variant`, once it was prepared.
    pub(crate) fn surface(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        self.surfaces[variant.index()].as_ref()
    }

    /// The outline pipeline of `variant`'s coverage, once it was prepared; transparent surfaces,
    /// the environment and foliage have none.
    pub(crate) fn outline(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        if matches!(variant.shading, Shading::Environment | Shading::Foliage) {
            return None;
        }
        self.outline.get(variant.coverage.index())?.as_ref()
    }

    /// Foliage's scene prepass pipeline of `coverage`, once a foliage draw of it was prepared.
    pub(crate) fn foliage_prepass(&self, coverage: Coverage) -> Option<&wgpu::RenderPipeline> {
        self.foliage_prepass.get(coverage.index())?.as_ref()
    }

    /// Foliage's shadow caster pipeline of `coverage`, once a foliage draw of it was prepared
    /// with shadows on.
    pub(crate) fn foliage_caster(&self, coverage: Coverage) -> Option<&wgpu::RenderPipeline> {
        self.foliage_casters.get(coverage.index())?.as_ref()
    }

    /// The outline hulls' scene prepass pipeline, once a draw with an outline was prepared.
    pub(crate) fn outline_prepass(&self) -> Option<&wgpu::RenderPipeline> {
        self.outline_prepass.as_ref()
    }

    /// The eyes-through-hair pipeline of `variant`, once it was prepared.
    pub(crate) fn through_hair(&self, variant: SurfaceVariant) -> Option<&wgpu::RenderPipeline> {
        self.through_hair[variant.through_hair_index()?].as_ref()
    }

    /// Pipelines built of `family`: its surfaces, the anime character family's outlines and eyes
    /// through hair, and foliage's prepasses and casters.
    fn built(&self, family: MaterialFamily) -> u32 {
        let surfaces = SurfaceVariant::ALL
            .iter()
            .filter(|variant| variant.shading.family() == family && self.surface(**variant).is_some())
            .count();
        let own = match family {
            MaterialFamily::AnimeCharacter => {
                self.outline.iter().flatten().count() + self.through_hair.iter().flatten().count()
            }
            MaterialFamily::Foliage => {
                self.foliage_prepass.iter().flatten().count() + self.foliage_casters.iter().flatten().count()
            }
            _ => 0,
        };
        (surfaces + own) as u32
    }
}

/// The mesh pipelines (the sky's are the `sky` module's). Quantized-mesh pipelines are built up
/// front, the prepasses' and opaque anime character surfaces only; the f32 ones when the first
/// f32 mesh, or skinned or morphed mesh, is uploaded ([`Self::prepare_format`]); the other
/// surface variants, the environment's among them, when a draw first needs them
/// ([`Self::prepare_variant`]); and eyes through hair with the first material that draws
/// through ([`Self::prepare_through_hair`]). Apps never pay for compiling what they don't draw.
///
/// They are built from the renderer's current shader sources, whose modules they keep; new
/// sources (hot reload) rebuild the same set from new modules ([`Self::rebuilt`]).
#[derive(Debug)]
pub(crate) struct Pipelines {
    /// Per [`PositionFormat::ALL`] entry.
    mesh: [Option<MeshPipelines>; 2],
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
    foliage: wgpu::ShaderModule,
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
            Program::Foliage => &mut self.foliage,
            Program::Sky
            | Program::Clusters
            | Program::Skinning
            | Program::Post
            | Program::Temporal
            | Program::Shadows
            | Program::Ambient => unreachable!("not a mesh program"),
        }
    }

    /// The module a surface of `shading` draws with, and the program it is built from.
    fn surface(&self, shading: Shading) -> (&wgpu::ShaderModule, Program) {
        match shading {
            Shading::Character(_) => (&self.character, Program::AnimeCharacter),
            Shading::Environment => (&self.environment, Program::Environment),
            Shading::Foliage => (&self.foliage, Program::Foliage),
            Shading::FogCard => (&self.fog_card, Program::FogCard),
        }
    }
}

/// What every scene pipeline shares, kept to build the f32 ones later.
#[derive(Clone, Debug)]
struct PipelineTarget {
    /// Groups 0–2, for passes that read no material textures: the scene prepass.
    layout: wgpu::PipelineLayout,
    /// Groups 1 and 2 alone: the character prepass, which writes the targets group 0 binds.
    character_layout: wgpu::PipelineLayout,
    /// Groups 0–3 per [`MESH_FAMILIES`] entry, with that family's scene group and group 3: the
    /// passes that shade with it (its surfaces, and the anime character family's outlines).
    family_layouts: [wgpu::PipelineLayout; MESH_FAMILIES.len()],
    color_format: wgpu::TextureFormat,
    sample_count: u32,
    /// The environment shader's tier settings: its specular model.
    environment: EnvironmentSettings,
    /// Foliage's tier settings: its wind levels, alpha-to-coverage and translucency.
    foliage: FoliageSettings,
    /// Foliage's shadow casters, while shadows are on: no group 0 (it holds the maps they draw
    /// into, as the shared casters' layout leaves it out), the view, foliage's scene group and
    /// its group 3, for the alpha test.
    foliage_caster_layout: Option<wgpu::PipelineLayout>,
    cache: Option<wgpu::PipelineCache>,
    /// The receivers' shadow constants (`frame.wgsl`'s `SHADOWS` and `SHADOW_TENT`), which every
    /// surface and outline takes with its permutation's; none with shadows off, which keeps the
    /// shaders as they were without them.
    shadows: Option<[(&'static str, f64); 2]>,
    /// Fog cards fade against the scene prepass's depth (`fog::FogCards`).
    card_depth_fade: bool,
    /// The environment shader's ambient constants (M1 W5, `ambient::constants`): only those on,
    /// so without a probe grid or ambient occlusion it is the shader as it was without them.
    ambient: Vec<(&'static str, f64)>,
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
        foliage: FoliageSettings,
        cache: Option<&wgpu::PipelineCache>,
        sources: &ShaderSources,
        shadows: Option<&ShadowSettings>,
        card_depth_fade: bool,
        ambient: Vec<(&'static str, f64)>,
    ) -> Self {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.view), Some(&layouts.scene[0])],
            immediate_size: 0,
        });
        let character_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("character prepass"),
            bind_group_layouts: &[None, Some(&layouts.view), Some(&layouts.scene[0])],
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
        let foliage_group = Shading::Foliage.scene_group();
        let foliage_caster_layout = shadows.is_some().then(|| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("foliage shadow caster"),
                bind_group_layouts: &[
                    None,
                    Some(&layouts.view),
                    Some(&layouts.scene[foliage_group]),
                    Some(&layouts.materials[MaterialFamily::Foliage.index()]),
                ],
                immediate_size: 0,
            })
        });
        let target = PipelineTarget {
            layout,
            character_layout,
            family_layouts,
            color_format,
            sample_count,
            environment,
            foliage,
            foliage_caster_layout,
            cache: cache.cloned(),
            shadows: shadows.map(ShadowSettings::constants),
            card_depth_fade,
            ambient,
        };
        let modules = MeshModules {
            character: shaders::module(device, Program::AnimeCharacter, sources),
            environment: shaders::module(device, Program::Environment, sources),
            fog_card: shaders::module(device, Program::FogCard, sources),
            outline: shaders::module(device, Program::Outline, sources),
            prepass: shaders::module(device, Program::Prepass, sources),
            foliage: shaders::module(device, Program::Foliage, sources),
        };
        let mut pipelines = Self { mesh: [None, None], target, modules };
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
        let mut mesh = [None, None];
        for format in PositionFormat::ALL {
            let Some(old) = self.mesh(format) else { continue };
            let scene_prepass = rebuild(Program::Prepass, &old.scene_prepass, &|| {
                build_scene_prepass(device, target, &modules, format)
            });
            let outline_prepass = old.outline_prepass.as_ref().map(|old| {
                rebuild(Program::Outline, old, &|| build_outline_prepass(device, target, &modules, format))
            });
            let character_prepass = rebuild(Program::Prepass, &old.character_prepass, &|| {
                build_character_prepass(device, target, &modules, format)
            });
            let mut pipelines = MeshPipelines {
                surfaces: Default::default(),
                outline: Default::default(),
                scene_prepass,
                outline_prepass,
                character_prepass,
                through_hair: Default::default(),
                foliage_prepass: Default::default(),
                foliage_casters: Default::default(),
            };
            for (index, variant) in SurfaceVariant::THROUGH_HAIR.into_iter().enumerate() {
                if let Some(old) = &old.through_hair[index] {
                    pipelines.through_hair[index] = Some(rebuild(Program::AnimeCharacter, old, &|| {
                        build_through_hair(device, target, &modules, format, variant)
                    }));
                }
            }
            for variant in SurfaceVariant::ALL {
                if let Some(old) = old.surface(variant) {
                    let (_, program) = modules.surface(variant.shading);
                    pipelines.surfaces[variant.index()] = Some(rebuild(program, old, &|| {
                        build_surface(device, target, &modules, format, variant)
                    }));
                }
            }
            for coverage in Coverage::DEPTH_WRITING {
                let index = coverage.index();
                if let Some(old) = &old.outline[index] {
                    pipelines.outline[index] = Some(rebuild(Program::Outline, old, &|| {
                        build_outline(device, target, &modules, format, coverage)
                    }));
                }
                if let Some(old) = &old.foliage_prepass[index] {
                    pipelines.foliage_prepass[index] = Some(rebuild(Program::Foliage, old, &|| {
                        build_foliage_prepass(device, target, &modules, format, coverage)
                    }));
                }
                if let Some(old) = &old.foliage_casters[index] {
                    pipelines.foliage_casters[index] = Some(rebuild(Program::Foliage, old, &|| {
                        build_foliage_caster(device, target, &modules, format, coverage)
                    }));
                }
            }
            mesh[format_index(format)] = Some(pipelines);
        }
        let pipelines = Self { mesh, target: self.target.clone(), modules };
        (pipelines, built)
    }

    /// Builds the mesh pipelines for `format` unless they exist: the two prepasses, and the
    /// opaque anime character surfaces and their outlines.
    pub(crate) fn prepare_format(&mut self, device: &wgpu::Device, format: PositionFormat) {
        if self.mesh[format_index(format)].is_some() {
            return;
        }
        let scene_prepass = build_scene_prepass(device, &self.target, &self.modules, format);
        let character_prepass = build_character_prepass(device, &self.target, &self.modules, format);
        self.mesh[format_index(format)] = Some(MeshPipelines {
            surfaces: Default::default(),
            outline: Default::default(),
            scene_prepass,
            outline_prepass: None,
            character_prepass,
            through_hair: Default::default(),
            foliage_prepass: Default::default(),
            foliage_casters: Default::default(),
        });
        self.prepare_variant(device, format, SurfaceVariant::OPAQUE);
    }

    /// Builds the eyes-through-hair pipeline of `variant`, a face or eye variant that writes
    /// depth, for `format`, whose mesh pipelines must exist, unless it does already.
    pub(crate) fn prepare_through_hair(
        &mut self,
        device: &wgpu::Device,
        format: PositionFormat,
        variant: SurfaceVariant,
    ) {
        let Some(index) = variant.through_hair_index() else { return };
        let mesh =
            self.mesh[format_index(format)].as_ref().expect("prepare_format built the format's pipelines");
        if mesh.through_hair[index].is_some() {
            return;
        }
        let pipeline = build_through_hair(device, &self.target, &self.modules, format, variant);
        let mesh = self.mesh[format_index(format)].as_mut().expect("checked above");
        mesh.through_hair[index] = Some(pipeline);
    }

    /// Builds the surface pipeline of `variant` for `format`, whose mesh pipelines must exist,
    /// and for anime characters the outline pipeline of its coverage and the outline hulls'
    /// prepass pipeline, for foliage its coverage's scene prepass and (with shadows) shadow
    /// caster, unless they do already.
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
        let foliage = variant.shading == Shading::Foliage;
        let coverage = variant.coverage;
        let foliage_prepass = (foliage && mesh.foliage_prepass(coverage).is_none())
            .then(|| build_foliage_prepass(device, &self.target, &self.modules, format, coverage));
        let foliage_caster = (foliage
            && self.target.foliage_caster_layout.is_some()
            && mesh.foliage_caster(coverage).is_none())
        .then(|| build_foliage_caster(device, &self.target, &self.modules, format, coverage));
        let mesh = self.mesh[format_index(format)].as_mut().expect("checked above");
        if let Some(prepass) = foliage_prepass {
            mesh.foliage_prepass[coverage.index()] = Some(prepass);
        }
        if let Some(caster) = foliage_caster {
            mesh.foliage_casters[coverage.index()] = Some(caster);
        }
        mesh.surfaces[variant.index()] = Some(surface);
        if let Some(outline) = outline {
            mesh.outline[variant.coverage.index()] = Some(outline);
        }
        if outline_prepass.is_some() {
            mesh.outline_prepass = outline_prepass;
        }
    }

    /// Sets the environment and foliage shaders' ambient constants (M1 W5: a probe grid arrived or
    /// left) and builds every environment and foliage surface pipeline built so far again with
    /// them, on this thread. Returns the pipelines built: none if the constants didn't change.
    pub(crate) fn set_ambient_constants(
        &mut self,
        device: &wgpu::Device,
        constants: Vec<(&'static str, f64)>,
    ) -> u32 {
        if self.target.ambient == constants {
            return 0;
        }
        self.target.ambient = constants;
        let mut built = 0;
        for format in PositionFormat::ALL {
            let Some(mesh) = self.mesh[format_index(format)].as_mut() else { continue };
            let lit_by_ambient =
                |v: &SurfaceVariant| matches!(v.shading, Shading::Environment | Shading::Foliage);
            for variant in SurfaceVariant::ALL.into_iter().filter(lit_by_ambient) {
                if mesh.surfaces[variant.index()].is_some() {
                    let pipeline = build_surface(device, &self.target, &self.modules, format, variant);
                    mesh.surfaces[variant.index()] = Some(pipeline);
                    built += 1;
                }
            }
        }
        built
    }

    /// Pipelines of `family` built so far, across formats: anime character surfaces and
    /// outlines, the environment's or fog cards' surfaces, or foliage's surfaces, prepasses and
    /// casters.
    pub(crate) fn built(&self, family: MaterialFamily) -> u32 {
        self.mesh.iter().flatten().map(|mesh| mesh.built(family)).sum()
    }

    /// The mesh pipelines for `format`, once [`Self::prepare_format`] built them.
    pub(crate) fn mesh(&self, format: PositionFormat) -> Option<&MeshPipelines> {
        self.mesh[format_index(format)].as_ref()
    }
}

/// The surface pipeline of `variant` for meshes in `format`: the anime character, environment,
/// foliage or fog card shader, on its family's layout, with the variant's permutation constants
/// (and the tier's specular model for the environment, its foliage settings for foliage, the
/// depth fade for fog cards).
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
    let mut constants = variant.constants();
    if variant.shading != Shading::FogCard {
        constants.extend(target.shadows.iter().flatten());
    }
    let (mut fragment, mut cull) = (None, Some(wgpu::Face::Back));
    let name = match variant.shading {
        Shading::Character(_) => "anime character",
        Shading::Environment => {
            constants.push((environment::SPECULAR_CONSTANT, target.environment.specular.constant()));
            constants.extend(target.ambient.iter().copied());
            "environment"
        }
        Shading::FogCard => {
            constants.extend(fog::card_constants(target.card_depth_fade));
            // Cards are seen from either side.
            cull = None;
            "fog card"
        }
        Shading::Foliage => {
            constants.extend(target.foliage.constants());
            constants.extend(target.ambient.iter().copied());
            constants.push((foliage::override_constant::SAMPLES, f64::from(target.sample_count)));
            // Alpha-to-coverage under MSAA, where the tier asks for it.
            if variant.coverage == Coverage::AlphaTest && target.foliage.covers(target.sample_count) {
                fragment = Some("fs_foliage_coverage");
            }
            // Cards and blades show both sides, lit alike by their volume normals; opaque foliage
            // (wood, solid canopies) is closed, so its back faces are culled.
            cull = foliage::cull(variant.coverage);
            "foliage"
        }
    };
    target.build_with(
        device,
        &target.family_layouts[variant.shading.scene_group()],
        PassDesc {
            name,
            fragment,
            module,
            vertex_buffers: &variant.shading.main_pass().buffers(format),
            cull,
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
            fragment: None,
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
        Some(wgpu::Face::Back),
        &scene_prepass_targets(false),
        (&target.layout, &[]),
    )
}

/// Foliage's scene prepass pipeline of `coverage` for meshes in `format`: the scene prepass's
/// targets, culled as foliage's surfaces are ([`foliage::cull`]), on foliage's layout (its scene group holds the wind
/// and its group 3 the cards' coverage), with the tier's wind levels.
fn build_foliage_prepass(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
    coverage: Coverage,
) -> wgpu::RenderPipeline {
    let mut constants = coverage.constants().to_vec();
    constants.push(target.foliage.wind_constant());
    target.build_prepass(
        device,
        "foliage prepass",
        &modules.foliage,
        &MeshPass::FoliagePrepass.buffers(format),
        foliage::cull(coverage),
        &scene_prepass_targets(false),
        (&target.family_layouts[Shading::Foliage.scene_group()], &constants),
    )
}

/// Foliage's shadow caster pipeline of `coverage` for meshes in `format` (shadows on): displaced
/// by the wind as the main pass draws it, two-sided with the shared casters' raster bias, and for
/// alpha-tested cards a fragment stage that cuts them out.
fn build_foliage_caster(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
    coverage: Coverage,
) -> wgpu::RenderPipeline {
    let layout = target.foliage_caster_layout.as_ref().expect("built while shadows are on");
    let mut constants = coverage.constants().to_vec();
    constants.push(target.foliage.wind_constant());
    let compilation_options =
        wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() };
    let cut_out = coverage == Coverage::AlphaTest;
    tracing::debug!(target: LOG_TARGET, pipeline = "foliage shadow caster", cut_out, "building a mesh pipeline");
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("foliage shadow caster"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &modules.foliage,
            entry_point: Some("vs_foliage_caster"),
            compilation_options: compilation_options.clone(),
            buffers: &MeshPass::FoliageShadow.buffers(format),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: shadow::CASTER_BIAS,
        }),
        multisample: wgpu::MultisampleState::default(),
        // Opaque foliage writes depth alone, as the shared casters do.
        fragment: cut_out.then_some(wgpu::FragmentState {
            module: &modules.foliage,
            entry_point: Some("fs_foliage_caster"),
            compilation_options,
            targets: &[],
        }),
        multiview_mask: None,
        cache: target.cache.as_ref(),
    });
    tracing::debug!(target: LOG_TARGET, pipeline = "foliage shadow caster", "built a mesh pipeline");
    pipeline
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
        Some(wgpu::Face::Front),
        &scene_prepass_targets(true),
        (&target.layout, &[]),
    )
}

/// The character prepass pipeline for meshes in `format` (skinning outputs are f32): depth, and
/// each pixel's character ID (M1 C5), on a layout without group 0, which binds what it writes.
fn build_character_prepass(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
) -> wgpu::RenderPipeline {
    let buffers = MeshPass::CharacterPrepass.buffers(format);
    let ids = [Some(wgpu::ColorTargetState {
        format: character_prepass::IDS_FORMAT,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    target.build_prepass(
        device,
        "character prepass",
        &modules.prepass,
        &buffers,
        Some(wgpu::Face::Back),
        &ids,
        (&target.character_layout, &[]),
    )
}

/// The eyes-through-hair pipeline of `variant` (a face or eye that writes depth) for meshes in
/// `format` (M1 C5): the anime character shader's own vertex stage and its `fs_through_hair`,
/// with the variant's constants, blended by coverage over the hair. It writes the hair's depth
/// from the character prepass, nudged nearer, and tests it with greater-or-equal against the
/// main pass's, so nothing that hides the hair is drawn over, and writes no depth.
fn build_through_hair(
    device: &wgpu::Device,
    target: &PipelineTarget,
    modules: &MeshModules,
    format: PositionFormat,
    variant: SurfaceVariant,
) -> wgpu::RenderPipeline {
    let mut constants = variant.constants();
    constants.extend(target.shadows.iter().flatten());
    target.build_with(
        device,
        &target.family_layouts[variant.shading.scene_group()],
        PassDesc {
            name: "through hair",
            fragment: None,
            module: &modules.character,
            vertex_buffers: &MeshPass::Main.buffers(format),
            cull: Some(wgpu::Face::Back),
            depth_write: false,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            blend: Some(TRANSPARENT_BLEND),
            constants: &constants,
        },
    )
}

impl PipelineTarget {
    /// A prepass pipeline: `cull` faces culled, reverse-Z depth written, no multisampling (the
    /// targets are sampled by later passes), and `color_targets` written by the
    /// `fs_<entry>` entry point when there are any, on `layout` with its pipeline constants.
    /// `name` is the entry points' suffix with spaces as underscores.
    #[expect(clippy::too_many_arguments, reason = "a pipeline's states, each read once")]
    fn build_prepass(
        &self,
        device: &wgpu::Device,
        name: &'static str,
        module: &wgpu::ShaderModule,
        vertex_buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
        cull: Option<wgpu::Face>,
        color_targets: &[Option<wgpu::ColorTargetState>],
        (layout, constants): (&wgpu::PipelineLayout, &[(&str, f64)]),
    ) -> wgpu::RenderPipeline {
        let entry = name.replace(' ', "_");
        let (vs, fs) = (format!("vs_{entry}"), format!("fs_{entry}"));
        let targets = color_targets.len();
        let compilation_options = wgpu::PipelineCompilationOptions { constants, ..Default::default() };
        tracing::debug!(target: LOG_TARGET, pipeline = name, targets, "building a mesh pipeline");
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(name),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(&vs),
                compilation_options: compilation_options.clone(),
                buffers: vertex_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: cull,
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
                compilation_options,
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
        let vs = format!("vs_{entry}");
        let fs = pass.fragment.map_or_else(|| format!("fs_{entry}"), str::to_owned);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_permutations_are_eight_bits_and_pick_one_part() {
        let bits = [
            Permutation::SKINNED,
            Permutation::ALPHA_TEST,
            Permutation::OUTLINE,
            Permutation::MORPH,
            Permutation::FACE,
            Permutation::HAIR,
            Permutation::EYE,
            Permutation::TRANSPARENT,
        ];
        let all = bits.iter().fold(0u8, |all, bit| {
            assert_eq!((bit.0.count_ones(), all & bit.0), (1, 0), "one bit each, distinct");
            all | bit.0
        });
        assert_eq!(all, u8::MAX, "the plan's eight bits, no more");
        // Eyes through hair adds a pass, not a permutation: the variant stays the part's.
        for (flags, part) in [
            (MaterialFlags::empty(), Part::Body),
            (MaterialFlags::FACE, Part::Face),
            (MaterialFlags::HAIR, Part::Hair),
            (MaterialFlags::EYE, Part::Eye),
            (MaterialFlags::FACE | MaterialFlags::THROUGH_HAIR, Part::Face),
            (MaterialFlags::EYE | MaterialFlags::THROUGH_HAIR, Part::Eye),
        ] {
            let variant = Permutation::new(flags, false, false).surface();
            assert_eq!(variant, SurfaceVariant::character(Coverage::Opaque, part), "{flags:?}");
        }
        let eye = Permutation::new(MaterialFlags::EYE | MaterialFlags::ALPHA_TEST, true, false).surface();
        assert_eq!(eye, SurfaceVariant::character(Coverage::AlphaTest, Part::Eye));
        assert!(SurfaceVariant::THROUGH_HAIR.iter().all(|variant| variant.through_hair_index().is_some()));
        assert_eq!(SurfaceVariant::ALL.len(), 3 * Shading::SURFACES.len() + 1, "and fog cards");
    }
}
