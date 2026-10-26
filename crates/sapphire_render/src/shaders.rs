//! WGSL sources: the renderer's shader files, embedded at build time ([`ShaderFile`]), and the
//! programs it builds from them (`Program`). Each scene pass's program is `frame.wgsl` and
//! `materials.wgsl` (the material families' blocks, which the passes' material bindings name)
//! followed by the files it uses, its own last, joined by newlines; M1 replaces the
//! concatenation with WESL imports (spec §5.12). The post passes share `post.wgsl`, and compute
//! skinning has `skinning.wgsl`, temporal AA `temporal.wgsl`; all three stand alone. The shadow
//! passes' `shadows.wgsl` follows `frame.wgsl` and `materials.wgsl` like a scene pass. The
//! ambient passes (M1 W5) join `ambient.wgsl`, which the environment shader shares, with their
//! own `probes.wgsl` and `occlusion.wgsl` in one program.
//!
//! # Replacing sources (hot reload, M1 A8)
//!
//! [`ShaderSources`] holds every file's text: compiled in, or edited. Development builds read
//! edited files from disk (the desktop hot reload) and live link will push them (A9); the
//! renderer only ever gets text, never a path (rule R5), and shipping builds use the compiled-in
//! files alone. [`Renderer::set_shader_sources`](crate::Renderer::set_shader_sources) rebuilds
//! the programs whose files changed and every pipeline built from them, after checking each
//! program with naga ([`validate`]) the way wgpu compiles it, with no optional capabilities,
//! so an error names the file and line it is in. Anything that fails keeps the pipelines the
//! renderer has.
//!
//! A new shader file joins [`ShaderFile`] (its name and its compiled-in text) and the programs
//! that use it (`Program::files`); a new program also needs its pipelines' owner to build them
//! from [`ShaderSources`] and rebuild them in `Renderer::set_shader_sources`, as the mesh
//! pipelines, the sky, post, temporal AA, the light clusters, skinning, the shadows and the
//! ambient passes do.

use std::borrow::Cow;
use std::fmt;

use sapphire_rhi::wgpu::{self, naga};

/// One of the renderer's WGSL files (`crates/sapphire_render/shaders/`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderFile {
    /// Declarations every scene pass shares: bindings, uniforms, vertex decoding, the sky.
    Frame,
    /// The material families' parameter blocks.
    Materials,
    /// Local lights and their lists.
    Lights,
    Sky,
    AnimeCharacter,
    /// The environment shader (M1 W4).
    Environment,
    /// Fog cards (M1 W3).
    FogCard,
    Outline,
    /// The light-cluster compute pass.
    Clusters,
    /// The character and scene prepasses.
    Prepass,
    /// Compute skinning, which binds its own group 0.
    Skinning,
    /// Every post pass: the bloom chain and the fused composite.
    Post,
    /// Temporal AA: the responsive mask and the resolve, which binds its own groups.
    Temporal,
    /// The shadow passes: casters and the face pass (the receivers are `frame.wgsl`'s).
    Shadows,
    /// The foliage shader (M1 W6): its surfaces, scene prepass and shadow casters.
    Foliage,
    /// Ambient light and occlusion as surfaces read them: the probe grid's lookup, capsule AO,
    /// GTAO's upsample and the reflection cube (M1 W5).
    Ambient,
    /// The ambient passes: the sky's projection, the reflection cube's prefilter, the grid's
    /// relight and the characters' ambient.
    Probes,
    /// GTAO and its denoiser.
    Occlusion,
}

impl ShaderFile {
    pub const ALL: [Self; 18] = [
        Self::Frame,
        Self::Materials,
        Self::Lights,
        Self::Sky,
        Self::AnimeCharacter,
        Self::Environment,
        Self::FogCard,
        Self::Outline,
        Self::Clusters,
        Self::Prepass,
        Self::Skinning,
        Self::Post,
        Self::Temporal,
        Self::Shadows,
        Self::Foliage,
        Self::Ambient,
        Self::Probes,
        Self::Occlusion,
    ];

    /// Its name in the renderer's shader directory, which errors name it by too.
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Frame => "frame.wgsl",
            Self::Materials => "materials.wgsl",
            Self::Lights => "lights.wgsl",
            Self::Sky => "sky.wgsl",
            Self::AnimeCharacter => "anime_character.wgsl",
            Self::Environment => "environment.wgsl",
            Self::FogCard => "fog_card.wgsl",
            Self::Outline => "outline.wgsl",
            Self::Clusters => "clusters.wgsl",
            Self::Prepass => "prepass.wgsl",
            Self::Skinning => "skinning.wgsl",
            Self::Post => "post.wgsl",
            Self::Temporal => "temporal.wgsl",
            Self::Shadows => "shadows.wgsl",
            Self::Foliage => "foliage.wgsl",
            Self::Ambient => "ambient.wgsl",
            Self::Probes => "probes.wgsl",
            Self::Occlusion => "occlusion.wgsl",
        }
    }

    /// The file called `name`, if it is one of the renderer's.
    pub fn from_file_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|file| file.file_name() == name)
    }

    /// Its text as compiled into the renderer.
    pub const fn compiled_in(self) -> &'static str {
        match self {
            Self::Frame => include_str!("../shaders/frame.wgsl"),
            Self::Materials => include_str!("../shaders/materials.wgsl"),
            Self::Lights => include_str!("../shaders/lights.wgsl"),
            Self::Sky => include_str!("../shaders/sky.wgsl"),
            Self::AnimeCharacter => include_str!("../shaders/anime_character.wgsl"),
            Self::Environment => include_str!("../shaders/environment.wgsl"),
            Self::FogCard => include_str!("../shaders/fog_card.wgsl"),
            Self::Outline => include_str!("../shaders/outline.wgsl"),
            Self::Clusters => include_str!("../shaders/clusters.wgsl"),
            Self::Prepass => include_str!("../shaders/prepass.wgsl"),
            Self::Skinning => include_str!("../shaders/skinning.wgsl"),
            Self::Post => include_str!("../shaders/post.wgsl"),
            Self::Temporal => include_str!("../shaders/temporal.wgsl"),
            Self::Shadows => include_str!("../shaders/shadows.wgsl"),
            Self::Foliage => include_str!("../shaders/foliage.wgsl"),
            Self::Ambient => include_str!("../shaders/ambient.wgsl"),
            Self::Probes => include_str!("../shaders/probes.wgsl"),
            Self::Occlusion => include_str!("../shaders/occlusion.wgsl"),
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

impl fmt::Display for ShaderFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.file_name())
    }
}

/// A shader module the renderer builds: its files, concatenated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Program {
    Sky,
    AnimeCharacter,
    Environment,
    FogCard,
    Outline,
    Clusters,
    Prepass,
    Skinning,
    Post,
    Temporal,
    Shadows,
    Foliage,
    /// The ambient passes and GTAO (M1 W5).
    Ambient,
}

impl Program {
    pub(crate) const ALL: [Self; 13] = [
        Self::Sky,
        Self::AnimeCharacter,
        Self::Environment,
        Self::FogCard,
        Self::Outline,
        Self::Clusters,
        Self::Prepass,
        Self::Skinning,
        Self::Post,
        Self::Temporal,
        Self::Shadows,
        Self::Foliage,
        Self::Ambient,
    ];

    /// Its files, in order.
    pub(crate) const fn files(self) -> &'static [ShaderFile] {
        use ShaderFile as F;
        match self {
            Self::Sky => &[F::Frame, F::Materials, F::Sky],
            Self::AnimeCharacter => &[F::Frame, F::Materials, F::Lights, F::AnimeCharacter],
            Self::Environment => &[F::Frame, F::Materials, F::Lights, F::Ambient, F::Environment],
            Self::FogCard => &[F::Frame, F::Materials, F::FogCard],
            Self::Outline => &[F::Frame, F::Materials, F::Lights, F::AnimeCharacter, F::Outline],
            Self::Clusters => &[F::Frame, F::Materials, F::Lights, F::Clusters],
            Self::Prepass => &[F::Frame, F::Materials, F::Prepass],
            Self::Skinning => &[F::Skinning],
            Self::Post => &[F::Post],
            Self::Temporal => &[F::Temporal],
            Self::Shadows => &[F::Frame, F::Materials, F::Shadows],
            Self::Foliage => &[F::Frame, F::Materials, F::Lights, F::Ambient, F::Foliage],
            Self::Ambient => &[F::Frame, F::Materials, F::Ambient, F::Probes, F::Occlusion],
        }
    }

    /// Its module's label.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Sky => "sky",
            Self::AnimeCharacter => "anime character",
            Self::Environment => "environment",
            Self::FogCard => "fog card",
            Self::Outline => "outline",
            Self::Clusters => "light clusters",
            Self::Prepass => "prepass",
            Self::Skinning => "skinning",
            Self::Post => "post",
            Self::Temporal => "temporal",
            Self::Shadows => "shadows",
            Self::Foliage => "foliage",
            Self::Ambient => "ambient",
        }
    }

    /// Whether the mesh pipelines (`pipelines::Pipelines`) are built from it.
    pub(crate) const fn is_mesh(self) -> bool {
        matches!(
            self,
            Self::AnimeCharacter
                | Self::Environment
                | Self::FogCard
                | Self::Outline
                | Self::Prepass
                | Self::Foliage
        )
    }
}

/// The text of every [`ShaderFile`], which the renderer builds its programs from: the
/// compiled-in files ([`Self::compiled_in`], the default), or edited ones ([`Self::set`]; see
/// the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderSources {
    files: [Cow<'static, str>; ShaderFile::ALL.len()],
}

impl Default for ShaderSources {
    fn default() -> Self {
        Self::compiled_in()
    }
}

impl ShaderSources {
    /// The files compiled into the renderer.
    pub fn compiled_in() -> Self {
        Self { files: ShaderFile::ALL.map(|file| Cow::Borrowed(file.compiled_in())) }
    }

    /// The text of `file`.
    pub fn get(&self, file: ShaderFile) -> &str {
        &self.files[file.index()]
    }

    /// Replaces the text of `file`.
    pub fn set(&mut self, file: ShaderFile, text: impl Into<Cow<'static, str>>) {
        self.files[file.index()] = text.into();
    }

    /// Whether every file is the compiled-in one.
    pub fn is_compiled_in(&self) -> bool {
        ShaderFile::ALL.into_iter().all(|file| self.get(file) == file.compiled_in())
    }

    /// The source of `program`: its files, joined by newlines.
    pub(crate) fn program(&self, program: Program) -> String {
        let files = program.files();
        let mut source = String::with_capacity(files.iter().map(|&file| self.get(file).len() + 1).sum());
        for (index, &file) in files.iter().enumerate() {
            if index > 0 {
                source.push('\n');
            }
            source.push_str(self.get(file));
        }
        source
    }

    /// Whether `program`'s files differ between these sources and `other`.
    pub(crate) fn differs(&self, other: &Self, program: Program) -> bool {
        program.files().iter().any(|&file| self.get(file) != other.get(file))
    }

    /// The file and line (1-based) of line `line` of `program`'s source, which joins its files.
    fn locate(&self, program: Program, line: u32) -> (ShaderFile, u32) {
        let files = program.files();
        let mut first = 1;
        for &file in files {
            // Each file starts on a line of its own: the newline that joins it to the next adds
            // one line beyond those the file's own newlines end.
            let lines = self.get(file).matches('\n').count() as u32 + 1;
            if line < first + lines {
                return (file, line - first + 1);
            }
            first += lines;
        }
        let last = *files.last().expect("every program has files");
        (last, line.saturating_sub(first) + 1)
    }
}

/// Why shader sources weren't applied. The renderer keeps the pipelines it has.
#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    /// A program's source doesn't parse or validate, at `file`, `line` and `column` (1-based).
    #[error("{file}:{line}:{column}: {message}")]
    Invalid {
        file: ShaderFile,
        line: u32,
        column: u32,
        message: String,
        /// naga's report, with the source lines it points at (lines numbered in the program,
        /// which joins its files).
        report: String,
    },
    /// wgpu refused a module or pipeline built from programs that validate (`programs`, the
    /// changed ones): entry points, bindings or vertex inputs that don't match what the renderer
    /// builds and binds.
    #[error("pipelines of the {programs} shaders: {message}")]
    Pipeline { programs: String, message: String },
}

/// Checks `program`'s source in `sources` as wgpu will compile it, with naga and no optional
/// capabilities (every shader must run on every supported device), and returns the module.
pub(crate) fn validate(program: Program, sources: &ShaderSources) -> Result<naga::Module, ShaderError> {
    let source = sources.program(program);
    let invalid = |location: Option<naga::SourceLocation>, message: String, report: String| {
        let (line, column) = location.map_or((1, 1), |at| (at.line_number, at.line_position));
        let (file, line) = sources.locate(program, line);
        ShaderError::Invalid { file, line, column, message, report }
    };
    let module = naga::front::wgsl::parse_str(&source).map_err(|error| {
        let report = error.emit_to_string_with_path(&source, program.name());
        invalid(error.location(&source), error.message().to_owned(), report)
    })?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|error| {
            let report = error.emit_to_string_with_path(&source, program.name());
            invalid(error.location(&source), error.as_inner().to_string(), report)
        })?;
    Ok(module)
}

/// `program`'s shader module, from `sources`.
pub(crate) fn module(device: &wgpu::Device, program: Program, sources: &ShaderSources) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(program.name()),
        source: wgpu::ShaderSource::Wgsl(sources.program(program).into()),
    })
}

#[cfg(test)]
mod tests {
    //! Headless shader checks: CI catches WGSL errors without a GPU. naga comes through the
    //! wgpu re-export, so it is always the exact version wgpu translates with at runtime.

    use std::collections::BTreeMap;

    use sapphire_assets::material::{
        AnimeCharacterMaterial, AnimeCharacterParams, AnimeCharacterRegion, EnvironmentParams, FogCardParams,
        FoliageParams, SkyParams,
    };
    use sapphire_assets::mesh::PositionFormat;
    use sapphire_rhi::wgpu::{self, naga};

    use super::*;
    use crate::ambient::{
        AmbientUniforms, GpuAmbientCapsule, GtaoUniforms, PrefilterUniforms, RelightUniforms,
    };
    use crate::characters::{GpuCharacter, GpuCharacterAmbient, GpuCharacterShadow, GpuFace, GpuRig};
    use crate::fog::GpuFog;
    use crate::foliage::{WindState, WindUniforms};
    use crate::gpu_types::{FrameUniforms, InstanceData, ViewUniforms};
    use crate::lights::GpuLight;
    use crate::pipelines::{
        DRAW_ATTRIBUTES, FOLIAGE_ATTRIBUTES, MeshPass, SHADING_ATTRIBUTES, SURFACE_ATTRIBUTES,
        position_attributes, previous_position_attributes,
    };
    use crate::post::PostUniforms;
    use crate::shadows::{GpuCapsule, GpuCascade, GpuCharacterMap, ShadowUniforms};
    use crate::temporal::TemporalUniforms;

    const PASSES: [(Program, &[&str]); 13] = [
        (Program::Sky, &["vs_sky", "fs_sky"]),
        (
            Program::AnimeCharacter,
            &["vs_anime_character", "fs_anime_character", "vs_through_hair", "fs_through_hair"],
        ),
        (Program::Environment, &["vs_environment", "fs_environment"]),
        (Program::FogCard, &["vs_fog_card", "fs_fog_card"]),
        (Program::Outline, &["vs_outline", "fs_outline", "vs_outline_prepass", "fs_outline_prepass"]),
        (Program::Clusters, &["cs_clusters"]),
        (
            Program::Prepass,
            &["vs_character_prepass", "fs_character_prepass", "vs_scene_prepass", "fs_scene_prepass"],
        ),
        (Program::Skinning, &["cs_skinning"]),
        (
            Program::Post,
            &[
                "vs_fullscreen",
                "fs_bloom_prefilter",
                "fs_bloom_down",
                "fs_bloom_up",
                "fs_post",
                "vs_composite",
                "fs_composite",
            ],
        ),
        (Program::Temporal, &["vs_fullscreen", "fs_responsive_mask", "cs_temporal_resolve"]),
        (Program::Shadows, &["vs_shadow_caster", "cs_face_shadow"]),
        (
            Program::Foliage,
            &[
                "vs_foliage",
                "fs_foliage",
                "fs_foliage_coverage",
                "vs_foliage_prepass",
                "fs_foliage_prepass",
                "vs_foliage_caster",
                "fs_foliage_caster",
            ],
        ),
        (
            Program::Ambient,
            &[
                "cs_sky_projection",
                "cs_prefilter",
                "cs_relight",
                "cs_character_ambient",
                "cs_gtao_depths",
                "cs_gtao",
                "cs_gtao_denoise",
            ],
        ),
    ];

    /// Every mesh pass's vertex entry point and the pipeline layout it is built with.
    const MESH_ENTRY_POINTS: [(Program, &str, MeshPass); 12] = [
        (Program::AnimeCharacter, "vs_anime_character", MeshPass::Main),
        (Program::AnimeCharacter, "vs_through_hair", MeshPass::Main),
        (Program::Environment, "vs_environment", MeshPass::Main),
        (Program::FogCard, "vs_fog_card", MeshPass::Main),
        (Program::Outline, "vs_outline", MeshPass::Main),
        (Program::Outline, "vs_outline_prepass", MeshPass::OutlinePrepass),
        (Program::Prepass, "vs_character_prepass", MeshPass::CharacterPrepass),
        (Program::Prepass, "vs_scene_prepass", MeshPass::ScenePrepass),
        (Program::Shadows, "vs_shadow_caster", MeshPass::Shadow),
        (Program::Foliage, "vs_foliage", MeshPass::Foliage),
        (Program::Foliage, "vs_foliage_prepass", MeshPass::FoliagePrepass),
        (Program::Foliage, "vs_foliage_caster", MeshPass::FoliageShadow),
    ];

    /// The compiled-in `program`, checked as the renderer checks sources.
    fn validate(program: Program) -> naga::Module {
        super::validate(program, &ShaderSources::compiled_in()).unwrap_or_else(|error| match error {
            ShaderError::Invalid { ref report, .. } => panic!("{error}\n{report}"),
            ShaderError::Pipeline { .. } => unreachable!("validation builds no pipelines"),
        })
    }

    #[test]
    fn programs_join_their_files_as_they_always_have() {
        let sources = ShaderSources::compiled_in();
        let character = concat!(
            include_str!("../shaders/frame.wgsl"),
            "\n",
            include_str!("../shaders/materials.wgsl"),
            "\n",
            include_str!("../shaders/lights.wgsl"),
            "\n",
            include_str!("../shaders/anime_character.wgsl")
        );
        assert_eq!(sources.program(Program::AnimeCharacter), character);
        assert_eq!(sources.program(Program::Post), include_str!("../shaders/post.wgsl"));
        // Every file is in a program, and found by its name.
        for file in ShaderFile::ALL {
            assert!(Program::ALL.iter().any(|program| program.files().contains(&file)), "{file}");
            assert_eq!(ShaderFile::from_file_name(file.file_name()), Some(file));
        }
        assert_eq!(ShaderFile::from_file_name("missing.wgsl"), None);
    }

    #[test]
    fn errors_name_the_file_and_line_they_are_in() {
        let compiled_in = ShaderSources::compiled_in();
        let at = |file: ShaderFile, after: &str, inserted: &str| {
            let mut sources = compiled_in.clone();
            let text = file.compiled_in();
            let end = text.find(after).expect("present") + after.len();
            sources.set(file, format!("{}\n{inserted}{}", &text[..end], &text[end..]));
            let line = text[..end].matches('\n').count() as u32 + 2;
            (sources, line)
        };
        // A parse error in the last file of a program, and one in the first file of another.
        let anchor = "fn fs_anime_character(in: CharacterVaryings) -> @location(0) vec4<f32> {";
        let (sources, line) = at(ShaderFile::AnimeCharacter, anchor, "    let broken = ;");
        assert!(sources.differs(&compiled_in, Program::AnimeCharacter));
        assert!(!sources.differs(&compiled_in, Program::Sky));
        match validate_sources(Program::AnimeCharacter, &sources) {
            ShaderError::Invalid { file, line: found, column, .. } => {
                assert_eq!((file, found, column), (ShaderFile::AnimeCharacter, line, 18));
            }
            other => panic!("{other}"),
        }
        let (sources, line) = at(ShaderFile::Frame, "struct FrameUniforms {", "    broken,");
        match validate_sources(Program::Sky, &sources) {
            ShaderError::Invalid { file, line: found, .. } => {
                assert_eq!((file, found), (ShaderFile::Frame, line))
            }
            other => panic!("{other}"),
        }
    }

    fn validate_sources(program: Program, sources: &ShaderSources) -> ShaderError {
        super::validate(program, sources).expect_err("a broken source")
    }

    #[test]
    fn every_pass_parses_and_validates() {
        assert_eq!(PASSES.map(|(program, _)| program), Program::ALL);
        for (program, entry_points) in PASSES {
            let name = program.name();
            let module = validate(program);
            for &entry_point in entry_points {
                assert!(
                    module.entry_points.iter().any(|ep| ep.name == entry_point),
                    "the {name} program has no entry point {entry_point}"
                );
            }
        }
    }

    /// The shader type a vertex attribute of `format` arrives as.
    fn shader_input_type(format: wgpu::VertexFormat) -> naga::TypeInner {
        use naga::{Scalar, TypeInner, VectorSize};
        let float = |size| TypeInner::Vector { size, scalar: Scalar::F32 };
        match format {
            wgpu::VertexFormat::Float32x2 | wgpu::VertexFormat::Snorm16x2 | wgpu::VertexFormat::Unorm16x2 => {
                float(VectorSize::Bi)
            }
            wgpu::VertexFormat::Float32x3 => float(VectorSize::Tri),
            wgpu::VertexFormat::Float32x4 | wgpu::VertexFormat::Unorm8x4 | wgpu::VertexFormat::Unorm16x4 => {
                float(VectorSize::Quad)
            }
            wgpu::VertexFormat::Uint32 => TypeInner::Scalar(Scalar::U32),
            other => panic!("add {other:?} to shader_input_type"),
        }
    }

    /// Whether a shader input of type `shader` can take an attribute of type `attribute`: the
    /// same scalar, and at least as many components, since the vertex fetch fills missing ones
    /// with (0, 0, 0, 1).
    fn accepts(shader: &naga::TypeInner, attribute: &naga::TypeInner) -> bool {
        use naga::TypeInner;
        match (shader, attribute) {
            (TypeInner::Scalar(a), TypeInner::Scalar(b)) => a == b,
            (TypeInner::Vector { size: a, scalar: sa }, TypeInner::Vector { size: b, scalar: sb }) => {
                sa == sb && (*a as u8) >= (*b as u8)
            }
            _ => false,
        }
    }

    /// Collects `@location` inputs, looking through struct arguments.
    fn collect_locations(
        module: &naga::Module,
        ty: naga::Handle<naga::Type>,
        binding: Option<&naga::Binding>,
        out: &mut BTreeMap<u32, naga::TypeInner>,
    ) {
        match (binding, &module.types[ty].inner) {
            (Some(naga::Binding::Location { location, .. }), inner) => {
                out.insert(*location, inner.clone());
            }
            (None, naga::TypeInner::Struct { members, .. }) => {
                for member in members {
                    collect_locations(module, member.ty, member.binding.as_ref(), out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn vertex_inputs_match_rust_vertex_layouts() {
        for format in PositionFormat::ALL {
            for (program, vertex_entry, pass) in MESH_ENTRY_POINTS {
                let attributes: BTreeMap<u32, naga::TypeInner> = pass
                    .buffers(format)
                    .iter()
                    .flatten()
                    .flat_map(|buffer| buffer.attributes)
                    .map(|attribute| (attribute.shader_location, shader_input_type(attribute.format)))
                    .collect();
                let module = validate(program);
                let entry_point =
                    module.entry_points.iter().find(|ep| ep.name == vertex_entry).expect("entry point");
                let mut inputs = BTreeMap::new();
                for argument in &entry_point.function.arguments {
                    collect_locations(&module, argument.ty, argument.binding.as_ref(), &mut inputs);
                }
                for (location, shader) in &inputs {
                    let attribute = attributes.get(location).unwrap_or_else(|| {
                        panic!("{vertex_entry} location {location} has no vertex attribute ({format:?})")
                    });
                    assert!(
                        accepts(shader, attribute),
                        "{vertex_entry} location {location}: {shader:?} can't take {attribute:?} ({format:?})"
                    );
                }
                // The main pass's shaders read every stream they bind, as the hull's prepass and
                // foliage's do; a prepass or caster may bind a stream for one of its attributes
                // (the scene prepass ignores outline normals, foliage's casters vertex colors).
                let exact =
                    [MeshPass::Main, MeshPass::OutlinePrepass, MeshPass::Foliage, MeshPass::FoliagePrepass];
                if exact.contains(&pass) {
                    assert_eq!(
                        inputs.keys().collect::<Vec<_>>(),
                        attributes.keys().collect::<Vec<_>>(),
                        "{vertex_entry} vertex input locations vs pipelines.rs layouts ({format:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn vertex_layouts_match_the_packed_streams() {
        use sapphire_assets::mesh::{VertexFoliage, VertexShading, VertexSurface};

        use crate::gpu_types::DrawData;
        let end = |attributes: &[wgpu::VertexAttribute]| {
            attributes.iter().map(|a| a.offset + a.format.size()).max().unwrap_or(0)
        };
        for format in PositionFormat::ALL {
            assert_eq!(end(position_attributes(format)), format.stride() as u64, "{format:?}");
            assert_eq!(end(previous_position_attributes(format)), format.stride() as u64, "{format:?}");
        }
        assert_eq!(end(&SHADING_ATTRIBUTES), size_of::<VertexShading>() as u64);
        assert_eq!(end(&SURFACE_ATTRIBUTES), size_of::<VertexSurface>() as u64);
        assert_eq!(end(&FOLIAGE_ATTRIBUTES), size_of::<VertexFoliage>() as u64);
        assert_eq!(end(&DRAW_ATTRIBUTES), size_of::<DrawData>() as u64);
        assert_eq!(size_of::<DrawData>(), 48);
    }

    #[test]
    fn rust_structs_match_wgsl_layouts() {
        let expected = [
            (Program::AnimeCharacter, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::AnimeCharacter, "FogUniforms", size_of::<GpuFog>()),
            (Program::AnimeCharacter, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::AnimeCharacter, "InstanceData", size_of::<InstanceData>()),
            (Program::AnimeCharacter, "AnimeCharacterParams", size_of::<AnimeCharacterParams>()),
            (Program::AnimeCharacter, "AnimeCharacterRegion", size_of::<AnimeCharacterRegion>()),
            (Program::Sky, "EnvironmentParams", size_of::<EnvironmentParams>()),
            (Program::Sky, "FoliageParams", size_of::<FoliageParams>()),
            (Program::Sky, "SkyParams", size_of::<SkyParams>()),
            (Program::Sky, "FogCardParams", size_of::<FogCardParams>()),
            (Program::Sky, "SkyUniforms", size_of::<crate::sky::SkyUniforms>()),
            (Program::Sky, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::Sky, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::Post, "PostUniforms", size_of::<PostUniforms>()),
            (Program::AnimeCharacter, "LocalLight", size_of::<GpuLight>()),
            (Program::AnimeCharacter, "CharacterData", size_of::<GpuCharacter>()),
            (Program::AnimeCharacter, "CharacterRig", size_of::<GpuRig>()),
            (Program::AnimeCharacter, "CharacterFace", size_of::<GpuFace>()),
            (Program::AnimeCharacter, "CharacterShadow", size_of::<GpuCharacterShadow>()),
            (Program::AnimeCharacter, "ShadowUniforms", size_of::<ShadowUniforms>()),
            (Program::AnimeCharacter, "ShadowCascade", size_of::<GpuCascade>()),
            (Program::AnimeCharacter, "CharacterShadowMap", size_of::<GpuCharacterMap>()),
            (Program::AnimeCharacter, "ShadowCapsule", size_of::<GpuCapsule>()),
            (Program::Shadows, "CharacterData", size_of::<GpuCharacter>()),
            (Program::Shadows, "ShadowUniforms", size_of::<ShadowUniforms>()),
            (Program::Shadows, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::Clusters, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::Environment, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::Environment, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::Environment, "InstanceData", size_of::<InstanceData>()),
            (Program::Environment, "EnvironmentParams", size_of::<EnvironmentParams>()),
            (Program::Environment, "LocalLight", size_of::<GpuLight>()),
            (Program::FogCard, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::FogCard, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::FogCard, "InstanceData", size_of::<InstanceData>()),
            (Program::FogCard, "FogCardParams", size_of::<FogCardParams>()),
            (Program::Prepass, "InstanceData", size_of::<InstanceData>()),
            (Program::Prepass, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::Temporal, "TemporalUniforms", size_of::<TemporalUniforms>()),
            (Program::Foliage, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::Foliage, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::Foliage, "InstanceData", size_of::<InstanceData>()),
            (Program::Foliage, "FoliageParams", size_of::<FoliageParams>()),
            (Program::Foliage, "LocalLight", size_of::<GpuLight>()),
            (Program::Foliage, "WindState", size_of::<WindState>()),
            (Program::Foliage, "WindUniforms", size_of::<WindUniforms>()),
            (Program::AnimeCharacter, "CharacterAmbient", size_of::<GpuCharacterAmbient>()),
            (Program::Environment, "AmbientUniforms", size_of::<AmbientUniforms>()),
            (Program::Environment, "AmbientCapsule", size_of::<GpuAmbientCapsule>()),
            (Program::Ambient, "AmbientUniforms", size_of::<AmbientUniforms>()),
            (Program::Ambient, "CharacterData", size_of::<GpuCharacter>()),
            (Program::Ambient, "FrameUniforms", size_of::<FrameUniforms>()),
            (Program::Ambient, "ViewUniforms", size_of::<ViewUniforms>()),
            (Program::Ambient, "RelightUniforms", size_of::<RelightUniforms>()),
            (Program::Ambient, "PrefilterUniforms", size_of::<PrefilterUniforms>()),
            (Program::Ambient, "GtaoUniforms", size_of::<GtaoUniforms>()),
            (Program::Ambient, "SkyLight", crate::ambient::SKY_LIGHT_BYTES as usize),
        ];
        for (program, name, rust_size) in expected {
            let module = validate(program);
            let mut layouter = naga::proc::Layouter::default();
            layouter.update(module.to_ctx()).expect("layout");
            let (handle, _) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no WGSL struct {name}"));
            assert_eq!(layouter[handle].size as usize, rust_size, "{name}: WGSL vs Rust size");
        }
    }

    /// `(field, offset)` for each listed field of a Rust struct.
    macro_rules! offsets {
        ($ty:ty { $($field:ident),+ $(,)? }) => {
            vec![$((stringify!($field), std::mem::offset_of!($ty, $field))),+]
        };
    }

    /// Material blocks are copied into their buffers byte for byte, so beyond their sizes, every
    /// field must sit where the WGSL struct reads it.
    #[test]
    fn material_blocks_match_wgsl_field_by_field() {
        let blocks = [
            (
                "AnimeCharacterParams",
                offsets!(AnimeCharacterParams {
                    base_color,
                    split,
                    edges,
                    terms,
                    surface,
                    surface_color,
                    modes,
                    swizzles,
                    regions,
                    outline_curve,
                    outline_shape,
                    outline_color,
                    rim,
                    hair_shadow,
                    through_hair,
                    rim_regions,
                }),
            ),
            (
                "AnimeCharacterRegion",
                offsets!(AnimeCharacterRegion { shade, deep_shade, subsurface, specular, rim, outline }),
            ),
            (
                "EnvironmentParams",
                offsets!(EnvironmentParams {
                    base_color,
                    surface,
                    shaping,
                    terminator,
                    shadow_tint,
                    distance,
                    emission,
                    modes,
                    swizzles,
                }),
            ),
            (
                "FoliageParams",
                offsets!(FoliageParams {
                    base_color,
                    shading,
                    shadow_tint,
                    translucency,
                    gradient_root,
                    gradient_tip,
                    wind,
                    distance,
                    modes,
                    swizzles,
                }),
            ),
            ("SkyParams", offsets!(SkyParams { discs, stars, clouds, cirrus, modes, swizzles })),
            ("FogCardParams", offsets!(FogCardParams { color, fade, scroll, modes, swizzles })),
        ];
        let module = validate(Program::AnimeCharacter);
        for (name, rust) in blocks {
            let (_, ty) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no WGSL struct {name}"));
            let naga::TypeInner::Struct { members, .. } = &ty.inner else { panic!("{name} is a struct") };
            let wgsl: Vec<(&str, usize)> = members
                .iter()
                .map(|member| (member.name.as_deref().unwrap_or_default(), member.offset as usize))
                .collect();
            assert_eq!(wgsl, rust, "{name}: WGSL vs Rust fields");
        }
    }

    /// The per-frame structs are uploaded byte for byte too: every field where the WGSL struct
    /// reads it.
    #[test]
    fn frame_structs_match_wgsl_field_by_field() {
        let structs = [
            (
                "FrameUniforms",
                offsets!(FrameUniforms {
                    sun_direction,
                    sun_radiance,
                    sky_zenith,
                    sky_horizon,
                    sky_ground,
                    time,
                    lights,
                    clusters,
                    day_night,
                    outline_distance,
                    ambient_sky,
                    ambient_ground,
                    shadow_tint,
                    weather,
                    fog,
                }),
            ),
            ("FogUniforms", offsets!(GpuFog { near, far, height, shape, light })),
            (
                "InstanceData",
                offsets!(InstanceData {
                    world_from_object,
                    normal_from_object,
                    previous_world_from_object,
                    character,
                }),
            ),
            ("CharacterData", offsets!(GpuCharacter { rig, face, shadow, ambient })),
            ("CharacterAmbient", offsets!(GpuCharacterAmbient { sky, ground, axis, probe })),
            ("CharacterRig", offsets!(GpuRig { key_direction, key_radiance, head, gi, skin_tint })),
            ("CharacterFace", offsets!(GpuFace { forward, up, expression })),
            ("CharacterShadow", offsets!(GpuCharacterShadow { probe, visibility })),
            (
                "ShadowUniforms",
                offsets!(ShadowUniforms {
                    light_x,
                    light_y,
                    light_z,
                    counts,
                    camera,
                    atlas,
                    cascades,
                    characters,
                    capsules
                }),
            ),
            ("ShadowCascade", offsets!(GpuCascade { area, depth })),
            ("CharacterShadowMap", offsets!(GpuCharacterMap { area, depth, tile })),
            ("ShadowCapsule", offsets!(GpuCapsule { base, top })),
        ];
        let character = validate(Program::AnimeCharacter);
        let ambient = validate(Program::Ambient);
        let ambient_structs = [
            (
                "AmbientUniforms",
                offsets!(AmbientUniforms {
                    grid_origin,
                    grid_cells,
                    grid_bricks,
                    atlas,
                    atlas_texel,
                    occlusion_size,
                    counts,
                    capsules
                }),
            ),
            ("AmbientCapsule", offsets!(GpuAmbientCapsule { base, top })),
            ("RelightUniforms", offsets!(RelightUniforms { keys, weights, counts, offsets, atlas, slice })),
            ("PrefilterUniforms", offsets!(PrefilterUniforms { mip, faces })),
            ("GtaoUniforms", offsets!(GtaoUniforms { size, params })),
        ];
        let modules = structs
            .into_iter()
            .map(|s| (&character, s))
            .chain(ambient_structs.into_iter().map(|s| (&ambient, s)));
        for (module, (name, rust)) in modules {
            let (_, ty) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no WGSL struct {name}"));
            let naga::TypeInner::Struct { members, .. } = &ty.inner else { panic!("{name} is a struct") };
            let wgsl: Vec<(&str, usize)> = members
                .iter()
                .map(|member| (member.name.as_deref().unwrap_or_default(), member.offset as usize))
                .collect();
            assert_eq!(wgsl, rust, "{name}: WGSL vs Rust fields");
        }
    }

    /// The value of the WGSL `const` called `name`, a `u32` literal.
    fn wgsl_u32(module: &naga::Module, name: &str) -> u32 {
        let (_, constant) = module
            .constants
            .iter()
            .find(|(_, constant)| constant.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no WGSL const {name}"));
        match module.global_expressions[constant.init] {
            naga::Expression::Literal(naga::Literal::U32(value)) => value,
            ref other => panic!("{name} is {other:?}, not a u32 literal"),
        }
    }

    #[test]
    fn wgsl_light_constants_match_rust() {
        use crate::lights::ListMode;
        use crate::lights::clusters::{CLUSTER_COUNT, SLICES, TILES_X, TILES_Y, WORKGROUP_SIZE};
        let character = validate(Program::AnimeCharacter);
        let clusters = validate(Program::Clusters);
        for module in [&character, &clusters] {
            assert_eq!(wgsl_u32(module, "CLUSTER_TILES_X"), TILES_X);
            assert_eq!(wgsl_u32(module, "CLUSTER_TILES_Y"), TILES_Y);
            assert_eq!(wgsl_u32(module, "CLUSTER_SLICES"), SLICES);
            assert_eq!(wgsl_u32(module, "CLUSTER_COUNT"), CLUSTER_COUNT);
            assert_eq!(wgsl_u32(module, "LIGHT_LISTS_NONE"), ListMode::None as u32);
            assert_eq!(wgsl_u32(module, "LIGHT_LISTS_CLUSTERS"), ListMode::Clusters as u32);
            assert_eq!(wgsl_u32(module, "LIGHT_LISTS_OBJECTS"), ListMode::Objects as u32);
        }
        assert_eq!(wgsl_u32(&clusters, "CLUSTER_WORKGROUP"), WORKGROUP_SIZE);
    }

    #[test]
    fn wgsl_skinning_constants_match_rust() {
        use crate::skinning::{JOB_ROWS, WORKGROUP_SIZE, flags};
        let skinning = validate(Program::Skinning);
        assert_eq!(wgsl_u32(&skinning, "SKINNING_WORKGROUP"), WORKGROUP_SIZE);
        assert_eq!(wgsl_u32(&skinning, "JOB_ROWS"), JOB_ROWS);
        assert_eq!(wgsl_u32(&skinning, "FLAG_FLOAT_POSITIONS"), flags::FLOAT_POSITIONS);
        assert_eq!(wgsl_u32(&skinning, "FLAG_SKIN"), flags::SKIN);
        assert_eq!(wgsl_u32(&skinning, "FLAG_WIDE_INFLUENCES"), flags::WIDE_INFLUENCES);
        assert_eq!(wgsl_u32(&skinning, "FLAG_MORPHS"), flags::MORPHS);
    }

    #[test]
    fn wgsl_material_constants_match_rust() {
        use crate::pipelines::material_binding;
        type M = AnimeCharacterMaterial;
        type P = AnimeCharacterParams;
        let character = validate(Program::AnimeCharacter);
        let slots = [
            ("CHARACTER_BASE_COLOR", M::BASE_COLOR),
            ("CHARACTER_CONTROL", M::CONTROL),
            ("CHARACTER_NORMAL", M::NORMAL),
            ("CHARACTER_RAMP", M::RAMP),
            ("CHARACTER_FACE_SHADOW", M::FACE_SHADOW),
            ("CHARACTER_SURFACE_MASK", M::SURFACE_MASK),
            ("CHARACTER_MATCAP", M::MATCAP),
            ("CHARACTER_EXPRESSION", M::EXPRESSION),
        ];
        assert_eq!(slots.len(), M::TEXTURE_SLOTS.len());
        for (name, slot) in slots {
            assert_eq!(wgsl_u32(&character, name) as usize, slot, "{name}");
        }
        let values = [
            ("CHARACTER_FLAG_RAMP", P::FLAG_RAMP),
            ("CHARACTER_FLAG_DEEP_SHADE", P::FLAG_DEEP_SHADE),
            ("CHARACTER_FLAG_ALPHA_TEST", P::FLAG_ALPHA_TEST),
            ("CHARACTER_FLAG_TRANSPARENT", P::FLAG_TRANSPARENT),
            ("CHARACTER_SURFACE_BODY", P::SURFACE_BODY),
            ("CHARACTER_SURFACE_FACE", P::SURFACE_FACE),
            ("CHARACTER_SURFACE_HAIR", P::SURFACE_HAIR),
            ("CHARACTER_SURFACE_EYE", P::SURFACE_EYE),
        ];
        for (name, value) in values {
            assert_eq!(wgsl_u32(&character, name), value, "{name}");
        }
        // Group 3: the texture slots at their own indices, then the two samplers.
        let bindings: BTreeMap<u32, String> = character
            .global_variables
            .iter()
            .filter_map(|(_, global)| {
                let binding = global.binding.as_ref().filter(|binding| binding.group == 3)?;
                Some((binding.binding, global.name.clone().unwrap_or_default()))
            })
            .collect();
        let mut expected: BTreeMap<u32, String> = M::TEXTURE_SLOTS
            .iter()
            .zip(0..)
            .map(|(slot, binding)| (binding, format!("{}_map", slot.name)))
            .collect();
        expected.insert(material_binding::SAMPLER, "material_sampler".into());
        expected.insert(material_binding::RAMP_SAMPLER, "ramp_sampler".into());
        expected.insert(material_binding::ID_SAMPLER, "material_id_sampler".into());
        assert_eq!(bindings, expected);
    }

    /// The environment shader's numbers, as `sapphire_assets` and the renderer state them: texture
    /// slots, flags and group 3 bindings, its material binding, and the specular models' values.
    #[test]
    fn wgsl_environment_matches_rust() {
        use sapphire_assets::material::EnvironmentMaterial;

        use crate::EnvironmentSpecular;
        use crate::pipelines::material_binding;
        type M = EnvironmentMaterial;
        let environment = validate(Program::Environment);
        let slots = [
            ("ENVIRONMENT_BASE_COLOR", M::BASE_COLOR),
            ("ENVIRONMENT_NORMAL", M::NORMAL),
            ("ENVIRONMENT_MASK", M::MASK),
            ("ENVIRONMENT_TERMINATOR_BAND", M::TERMINATOR_BAND),
            ("ENVIRONMENT_EMISSION", M::EMISSION),
        ];
        assert_eq!(slots.len(), M::TEXTURE_SLOTS.len());
        for (name, slot) in slots {
            assert_eq!(wgsl_u32(&environment, name) as usize, slot, "{name}");
        }
        assert_eq!(wgsl_u32(&environment, "ENVIRONMENT_FLAG_ALPHA_TEST"), EnvironmentParams::FLAG_ALPHA_TEST);
        assert_eq!(
            wgsl_u32(&environment, "ENVIRONMENT_FLAG_TRANSPARENT"),
            EnvironmentParams::FLAG_TRANSPARENT
        );
        let models = [
            ("ENVIRONMENT_SPECULAR_OFF", EnvironmentSpecular::Off),
            ("ENVIRONMENT_SPECULAR_APPROXIMATE", EnvironmentSpecular::Approximate),
            ("ENVIRONMENT_SPECULAR_FULL", EnvironmentSpecular::Full),
        ];
        for (name, model) in models {
            assert_eq!(f64::from(wgsl_u32(&environment, name)), model.constant(), "{name}");
        }
        let overrides: Vec<&str> =
            environment.overrides.iter().filter_map(|(_, o)| o.name.as_deref()).collect();
        // `frame.wgsl`'s shadow constants (M1 R4), which its receivers read, then its own: the
        // permutation, the tier's specular model and the ambient terms (M1 W5).
        let shadows = [crate::shadows::override_constant::SHADOWS, crate::shadows::override_constant::TENT];
        use crate::ambient::override_constant as ambient;
        assert_eq!(
            overrides,
            [
                shadows[0],
                shadows[1],
                "PERMUTATION_ALPHA_TEST",
                "PERMUTATION_TRANSPARENT",
                crate::environment::SPECULAR_CONSTANT,
                ambient::PROBES,
                ambient::PROBES_PER_VERTEX,
                ambient::CAPSULE_AO,
                ambient::GTAO,
            ]
        );
        // Group 3: the texture slots at their own indices, then the samplers it reads; group 2
        // binding 1 is the environment family's buffer.
        let bindings: BTreeMap<u32, String> = environment
            .global_variables
            .iter()
            .filter_map(|(_, global)| {
                let binding = global.binding.as_ref().filter(|binding| binding.group == 3)?;
                Some((binding.binding, global.name.clone().unwrap_or_default()))
            })
            .collect();
        let mut expected: BTreeMap<u32, String> = M::TEXTURE_SLOTS
            .iter()
            .zip(0..)
            .map(|(slot, binding)| (binding, format!("{}_map", slot.name)))
            .collect();
        expected.insert(material_binding::SAMPLER, "material_sampler".into());
        expected.insert(material_binding::RAMP_SAMPLER, "ramp_sampler".into());
        assert_eq!(bindings, expected);
        // frame.wgsl declares the anime character family's at the same binding; the environment's
        // entry points read only their own.
        let (_, buffer) = environment
            .global_variables
            .iter()
            .find(|(_, global)| global.name.as_deref() == Some("environment_materials"))
            .expect("declared");
        let binding = buffer.binding.as_ref().expect("bound");
        assert_eq!((binding.group, binding.binding), (2, 1));
        let naga::TypeInner::Array { base, .. } = environment.types[buffer.ty].inner else {
            panic!("an array of blocks")
        };
        assert_eq!(environment.types[base].name.as_deref(), Some("EnvironmentParams"));
    }

    /// The foliage shader's numbers, as `sapphire_assets` and the `foliage` module state them:
    /// texture slots and group 3 bindings, its scene group's buffers, the wind's uniforms field
    /// by field and the constants its CPU mirror shares, and its overrides.
    #[test]
    fn wgsl_foliage_matches_rust() {
        use sapphire_assets::material::FoliageMaterial;

        use crate::foliage::{self, override_constant};
        use crate::pipelines::material_binding;
        type M = FoliageMaterial;
        let module = validate(Program::Foliage);
        assert_eq!(wgsl_u32(&module, "FOLIAGE_BASE_COLOR") as usize, M::BASE_COLOR);
        assert_eq!(wgsl_u32(&module, "FOLIAGE_MASK") as usize, M::MASK);
        assert_eq!(M::TEXTURE_SLOTS.len(), 2);
        let bindings = |group: u32| -> BTreeMap<u32, String> {
            module
                .global_variables
                .iter()
                .filter_map(|(_, global)| {
                    let binding = global.binding.as_ref().filter(|binding| binding.group == group)?;
                    Some((binding.binding, global.name.clone().unwrap_or_default()))
                })
                .collect()
        };
        let mut expected: BTreeMap<u32, String> = M::TEXTURE_SLOTS
            .iter()
            .zip(0..)
            .map(|(slot, binding)| (binding, format!("{}_map", slot.name)))
            .collect();
        expected.insert(material_binding::SAMPLER, "material_sampler".into());
        assert_eq!(bindings(3), expected);
        // Group 2: frame.wgsl's instances and anime character blocks (which foliage's entry points
        // never read), foliage's blocks at the same binding, and the wind.
        let scene = bindings(2);
        assert_eq!(scene.get(&2).map(String::as_str), Some("wind"));
        let (_, buffer) = module
            .global_variables
            .iter()
            .find(|(_, global)| global.name.as_deref() == Some("foliage_materials"))
            .expect("declared");
        let binding = buffer.binding.as_ref().expect("bound");
        assert_eq!((binding.group, binding.binding), (2, 1));
        let fields = |name: &str| -> Vec<(String, usize)> {
            let (_, ty) = module
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no WGSL struct {name}"));
            let naga::TypeInner::Struct { members, .. } = &ty.inner else { panic!("{name} is a struct") };
            members
                .iter()
                .map(|member| (member.name.clone().unwrap_or_default(), member.offset as usize))
                .collect()
        };
        let owned = |fields: Vec<(&str, usize)>| -> Vec<(String, usize)> {
            fields.into_iter().map(|(name, offset)| (name.to_owned(), offset)).collect()
        };
        assert_eq!(fields("WindState"), owned(offsets!(WindState { direction, time })));
        assert_eq!(fields("WindUniforms"), owned(offsets!(WindUniforms { current, previous })));
        // The wind's numbers, which `foliage::wind_offset` mirrors.
        assert_eq!(f64::from(wgsl_f32(&module, "WIND_PERIOD")), foliage::WIND_PERIOD);
        let numbers = [
            ("WIND_SWAY_HZ", foliage::SWAY_HZ),
            ("WIND_BEND_HZ", foliage::BEND_HZ),
            ("WIND_BOB_HZ", foliage::BOB_HZ),
            ("WIND_GUST_HZ", foliage::GUST_HZ),
            ("WIND_GUST_WAVELENGTH", foliage::GUST_WAVELENGTH),
            ("WIND_LEAN", foliage::LEAN),
            ("WIND_SWING", foliage::SWING),
            ("TAU", std::f32::consts::TAU),
        ];
        for (name, value) in numbers {
            assert_eq!(wgsl_f32(&module, name), value, "{name}");
        }
        assert_eq!(wgsl_vec3(&module, "WIND_FLUTTER_SPREAD"), foliage::FLUTTER_SPREAD.to_array());
        let overrides: Vec<&str> = module.overrides.iter().filter_map(|(_, o)| o.name.as_deref()).collect();
        let shadows = [crate::shadows::override_constant::SHADOWS, crate::shadows::override_constant::TENT];
        assert_eq!(
            overrides,
            [
                shadows[0],
                shadows[1],
                "PERMUTATION_ALPHA_TEST",
                override_constant::SAMPLES,
                override_constant::WIND_LEVELS,
                override_constant::TRANSLUCENCY,
                // The ambient terms, by the names the environment shader's take (M1 W5).
                crate::ambient::override_constant::PROBES,
                crate::ambient::override_constant::PROBES_PER_VERTEX,
                crate::ambient::override_constant::CAPSULE_AO,
                crate::ambient::override_constant::GTAO,
            ]
        );
    }

    /// The sky's uniform block is written by the CPU every frame, so every field must sit where
    /// the WGSL struct reads it; its texture slots and group 3 bindings are the sky family's.
    #[test]
    fn wgsl_sky_matches_rust() {
        use sapphire_assets::material::SkyMaterial;

        use crate::pipelines::material_binding;
        use crate::sky::{CLOUDS_CONSTANT, DISCS_CONSTANT, STARS_CONSTANT, SkyUniforms};
        let sky = validate(Program::Sky);
        let rust = offsets!(SkyUniforms {
            sun,
            sun_disc,
            sun_glow,
            moon,
            moon_disc,
            moon_glow,
            moon_right,
            moon_up,
            celestial,
            stars,
            cloud_lit,
            cloud_shade,
            cloud_rim,
            clouds,
            cirrus,
            modes,
            swizzles,
            glow_shape,
        });
        let (_, ty) = sky
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some("SkyUniforms"))
            .expect("no WGSL struct SkyUniforms");
        let naga::TypeInner::Struct { members, .. } = &ty.inner else { panic!("SkyUniforms is a struct") };
        let wgsl: Vec<(&str, usize)> = members
            .iter()
            .map(|member| (member.name.as_deref().unwrap_or_default(), member.offset as usize))
            .collect();
        assert_eq!(wgsl, rust, "SkyUniforms: WGSL vs Rust fields");
        let slots = [
            ("SKY_SLOT_MOON", SkyMaterial::MOON),
            ("SKY_SLOT_STARS", SkyMaterial::STARS),
            ("SKY_SLOT_MILKY_WAY", SkyMaterial::MILKY_WAY),
            ("SKY_SLOT_CLOUDS", SkyMaterial::CLOUDS),
            ("SKY_SLOT_CIRRUS", SkyMaterial::CIRRUS),
        ];
        assert_eq!(slots.len(), SkyMaterial::TEXTURE_SLOTS.len());
        for (name, slot) in slots {
            assert_eq!(wgsl_u32(&sky, name) as usize, slot, "{name}");
        }
        let bindings: BTreeMap<u32, String> = sky
            .global_variables
            .iter()
            .filter_map(|(_, global)| {
                let binding = global.binding.as_ref().filter(|binding| binding.group == 3)?;
                Some((binding.binding, global.name.clone().unwrap_or_default()))
            })
            .collect();
        let mut expected: BTreeMap<u32, String> = SkyMaterial::TEXTURE_SLOTS
            .iter()
            .zip(0..)
            .map(|(slot, binding)| (binding, format!("{}_map", slot.name)))
            .collect();
        expected.insert(material_binding::SAMPLER, "sky_sampler".into());
        assert_eq!(bindings, expected);
        let names = [DISCS_CONSTANT, STARS_CONSTANT, CLOUDS_CONSTANT];
        for name in names {
            assert!(sky.overrides.iter().any(|(_, o)| o.name.as_deref() == Some(name)), "{name}");
        }
        // Every variant specializes: discs on and off, and each set of layers.
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&sky)
        .expect("validated above");
        for bits in 0..8u32 {
            let constants: naga::back::PipelineConstants = names
                .iter()
                .zip(0..)
                .map(|(name, bit)| (name.to_string(), f64::from(bits >> bit & 1)))
                .collect();
            for entry_point in &sky.entry_points {
                let stage = Some((entry_point.stage, entry_point.name.as_str()));
                naga::back::pipeline_constants::process_overrides(&sky, &info, stage, &constants)
                    .unwrap_or_else(|error| panic!("sky.wgsl {bits:03b} {}: {error}", entry_point.name));
            }
        }
    }

    /// Post's constants are written by the CPU every frame, so every field must sit where the
    /// WGSL struct reads it, the compositing layers' and the effects' structs included; their
    /// enumerations are the WGSL constants', and every variant of the five switches specializes,
    /// with every tap count the radial blur takes.
    #[test]
    fn wgsl_post_matches_rust() {
        use crate::post::layers::{BlendKind, FLARE_ELEMENTS, FlareKind, GpuFlareElement, GpuGradient};
        use crate::post::{
            CHARACTERS_CONSTANT, COLOR_EFFECTS_CONSTANT, GpuEffects, LAYERS_CONSTANT, LENS_EFFECTS_CONSTANT,
            MAX_GRADIENT_LAYERS, RADIAL_TAPS_CONSTANT, SHARPEN_CONSTANT,
        };
        let post = validate(Program::Post);
        let fields = |name: &str| -> Vec<(String, usize)> {
            let (_, ty) = post
                .types
                .iter()
                .find(|(_, ty)| ty.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no WGSL struct {name}"));
            let naga::TypeInner::Struct { members, .. } = &ty.inner else { panic!("{name} is a struct") };
            members
                .iter()
                .map(|member| (member.name.clone().unwrap_or_default(), member.offset as usize))
                .collect()
        };
        let owned = |fields: Vec<(&str, usize)>| -> Vec<(String, usize)> {
            fields.into_iter().map(|(name, offset)| (name.to_owned(), offset)).collect()
        };
        let uniforms = offsets!(PostUniforms {
            composite,
            bloom,
            lut,
            vignette,
            vignette_color,
            character_bloom,
            diffusion,
            diffusion_threshold,
            frame,
            flare_sun,
            flare,
            counts,
            gradients,
            flare_elements,
            effects,
        });
        assert_eq!(fields("PostUniforms"), owned(uniforms), "PostUniforms: WGSL vs Rust fields");
        let effects = offsets!(GpuEffects {
            lens,
            chromatic,
            inversion,
            duotone,
            duotone_dark,
            duotone_light,
            lines,
            lines_shape,
            lines_color,
            flags,
        });
        assert_eq!(fields("PostEffects"), owned(effects), "PostEffects: WGSL vs Rust fields");
        assert_eq!(
            fields("GradientLayer"),
            owned(offsets!(GpuGradient { color, geometry, mode, scale, offset }))
        );
        assert_eq!(fields("FlareElement"), owned(offsets!(GpuFlareElement { shape, color, reach })));
        assert_eq!(wgsl_u32(&post, "MAX_GRADIENT_LAYERS") as usize, MAX_GRADIENT_LAYERS);
        assert_eq!(wgsl_u32(&post, "FLARE_ELEMENTS") as usize, FLARE_ELEMENTS);
        let kinds = [
            ("FLARE_DISC", FlareKind::Disc),
            ("FLARE_RING", FlareKind::Ring),
            ("FLARE_HEXAGON", FlareKind::Hexagon),
            ("FLARE_STREAK", FlareKind::Streak),
        ];
        for (name, kind) in kinds {
            assert_eq!(wgsl_u32(&post, name), kind as u32, "{name}");
        }
        let blends = [
            ("BLEND_AFFINE", BlendKind::Affine),
            ("BLEND_OVERLAY", BlendKind::Overlay),
            ("BLEND_SOFT_LIGHT", BlendKind::SoftLight),
        ];
        for (name, kind) in blends {
            assert_eq!(wgsl_u32(&post, name), kind as u32, "{name}");
        }
        assert_eq!((wgsl_u32(&post, "GRADIENT_LINEAR"), wgsl_u32(&post, "GRADIENT_RADIAL")), (0, 1));
        let names = [
            LAYERS_CONSTANT,
            SHARPEN_CONSTANT,
            CHARACTERS_CONSTANT,
            LENS_EFFECTS_CONSTANT,
            COLOR_EFFECTS_CONSTANT,
        ];
        for name in names.into_iter().chain([RADIAL_TAPS_CONSTANT]) {
            assert!(post.overrides.iter().any(|(_, o)| o.name.as_deref() == Some(name)), "{name}");
        }
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&post)
        .expect("validated above");
        for (bits, taps) in (0..32u32).flat_map(|bits| [4, 8, 12, 16].map(|taps| (bits, taps))) {
            let constants: naga::back::PipelineConstants = names
                .iter()
                .zip(0..)
                .map(|(name, bit)| (name.to_string(), f64::from(bits >> bit & 1)))
                .chain([(RADIAL_TAPS_CONSTANT.to_owned(), f64::from(taps))])
                .collect();
            for entry_point in &post.entry_points {
                let stage = Some((entry_point.stage, entry_point.name.as_str()));
                naga::back::pipeline_constants::process_overrides(&post, &info, stage, &constants)
                    .unwrap_or_else(|error| {
                        panic!("post.wgsl {bits:05b} {taps} {}: {error}", entry_point.name)
                    });
            }
        }
    }

    /// The value of the WGSL `const` called `name`, an `f32` (const-evaluated by naga).
    fn wgsl_f32(module: &naga::Module, name: &str) -> f32 {
        let (_, constant) = module
            .constants
            .iter()
            .find(|(_, constant)| constant.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no WGSL const {name}"));
        match module.global_expressions[constant.init] {
            naga::Expression::Literal(naga::Literal::F32(value)) => value,
            ref other => panic!("{name} is {other:?}, not an f32 literal"),
        }
    }

    /// The anime character contract's numbers, as the shaders and `sapphire_assets` both state
    /// them: the vertex flags, the control map's encodings and the ramp rows.
    #[test]
    fn wgsl_character_contract_matches_rust() {
        use sapphire_assets::character_maps::{MATERIAL_IDS, face_sdf, hair_mask, ilm, ramp};
        use sapphire_assets::material::AnimeCharacterParams;

        use crate::character_prepass;
        use sapphire_assets::mesh::VertexFlags;
        let character = validate(Program::AnimeCharacter);
        let outline = validate(Program::Outline);
        for module in [&character, &outline] {
            assert_eq!(wgsl_u32(module, "VERTEX_MATERIAL_ID_MASK"), u32::from(VertexFlags::MATERIAL_ID_MASK));
        }
        assert_eq!(wgsl_f32(&character, "SHADOW_BIAS_NEUTRAL"), ilm::NEUTRAL[ilm::SHADOW_BIAS]);
        assert_eq!(wgsl_f32(&character, "SHADOW_BIAS_LIT"), ilm::SHADOW_BIAS_LIT);
        assert_eq!(wgsl_f32(&character, "SHADOW_BIAS_SHADOWED"), ilm::SHADOW_BIAS_SHADOWED);
        assert_eq!(wgsl_f32(&character, "RAMP_ROWS"), ramp::ROWS as f32);
        assert_eq!(wgsl_f32(&character, "MATERIAL_ID_LEVELS"), f32::from(MATERIAL_IDS));
        assert_eq!(wgsl_f32(&character, "FACE_MIRROR_BLEND"), face_sdf::MIRROR_BLEND);
        assert_eq!(wgsl_f32(&character, "PI"), std::f32::consts::PI);
        // The hair mask (M1 C5).
        assert_eq!(wgsl_f32(&character, "HAIR_MASK_NO_SHIFT"), hair_mask::NO_SHIFT);
        assert_eq!(
            (
                wgsl_u32(&character, "CHARACTER_HIGHLIGHT_ANGEL_RING"),
                wgsl_u32(&character, "CHARACTER_HIGHLIGHT_KAJIYA_KAY")
            ),
            (AnimeCharacterParams::HIGHLIGHT_ANGEL_RING, AnimeCharacterParams::HIGHLIGHT_KAJIYA_KAY)
        );
        assert_eq!(
            wgsl_u32(&character, "CHARACTER_HIGHLIGHT_PAINTED_BAND"),
            AnimeCharacterParams::HIGHLIGHT_PAINTED_BAND
        );
        // The character prepass's IDs (M1 C5), which the prepass writes and the surface reads.
        let prepass = validate(Program::Prepass);
        for module in [&character, &prepass] {
            assert_eq!(wgsl_u32(module, "CHARACTER_ID_ENTRY_BITS"), character_prepass::ID_ENTRY_BITS);
            assert_eq!(wgsl_u32(module, "CHARACTER_ID_ENTRY_MASK"), character_prepass::ID_ENTRY_MASK);
        }
        // Its parts are the material's surfaces, in `pipelines::Part`'s order.
        use crate::pipelines::Part;
        let surfaces = [
            AnimeCharacterParams::SURFACE_BODY,
            AnimeCharacterParams::SURFACE_FACE,
            AnimeCharacterParams::SURFACE_HAIR,
            AnimeCharacterParams::SURFACE_EYE,
        ];
        assert_eq!(Part::ALL.map(|part| part.index() as u32), surfaces);
        assert_eq!(character_prepass::encode_id(5, AnimeCharacterParams::SURFACE_HAIR), 5 | 2 << 14);
        assert_eq!(character_prepass::encode_id(16384 + 3, 1), 3 | 1 << 14, "entries wrap at 14 bits");
    }

    /// The value of the WGSL `const` called `name`, a `vec3<f32>` of literals.
    fn wgsl_vec3(module: &naga::Module, name: &str) -> [f32; 3] {
        let (_, constant) = module
            .constants
            .iter()
            .find(|(_, constant)| constant.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no WGSL const {name}"));
        let naga::Expression::Compose { ref components, .. } = module.global_expressions[constant.init]
        else {
            panic!("{name} is not a vector");
        };
        let value = |&handle: &naga::Handle<naga::Expression>| match module.global_expressions[handle] {
            naga::Expression::Literal(naga::Literal::F32(value)) => value,
            ref other => panic!("{name}: {other:?} is not an f32 literal"),
        };
        let values: Vec<f32> = components.iter().map(value).collect();
        values.try_into().unwrap_or_else(|values| panic!("{name}: {values:?}"))
    }

    /// Outlines v2's numbers (M1 C4): the outline frame that frame.wgsl and skinning.wgsl build
    /// as `sapphire_assets::mesh` does, and the outline curve's fade.
    #[test]
    fn wgsl_outline_constants_match_rust() {
        use sapphire_assets::material::OutlineCurve;
        use sapphire_assets::mesh::{OUTLINE_FRAME_AXIS, OUTLINE_FRAME_FALLBACK, OUTLINE_FRAME_MIN_CROSS};
        for program in [Program::Outline, Program::AnimeCharacter, Program::Skinning] {
            let module = validate(program);
            assert_eq!(wgsl_vec3(&module, "OUTLINE_FRAME_AXIS"), OUTLINE_FRAME_AXIS, "{program:?}");
            assert_eq!(wgsl_vec3(&module, "OUTLINE_FRAME_FALLBACK"), OUTLINE_FRAME_FALLBACK, "{program:?}");
            assert_eq!(wgsl_f32(&module, "OUTLINE_FRAME_MIN_CROSS"), OUTLINE_FRAME_MIN_CROSS, "{program:?}");
        }
        let outline = validate(Program::Outline);
        assert_eq!(wgsl_f32(&outline, "OUTLINE_CUTOFF_FADE"), OutlineCurve::FADE);
        let tan = wgsl_f32(&outline, "OUTLINE_REFERENCE_TAN_HALF_FOV");
        assert!((tan - 22.5f32.to_radians().tan()).abs() < 1e-7, "{tan}");
    }

    /// Every static permutation of every mesh pass specializes and validates: what each variant's
    /// pipeline compiles (the GPU tests build them on the device too).
    #[test]
    fn every_permutation_specializes_and_validates() {
        use crate::foliage::FoliageSettings;
        use crate::pipelines::{Coverage, Shading, SurfaceVariant, override_constant};
        use crate::shadows::{ShadowFilter, ShadowSettings};
        use crate::{EnvironmentSpecular, foliage};
        type Constants = Vec<(&'static str, f64)>;
        // Every surface variant without shadows, and with them under either filter (M1 R4).
        let shadows = [None, Some(ShadowFilter::Tent3), Some(ShadowFilter::Tent5)].map(|filter| {
            filter.map(|filter| ShadowSettings { filter, ..ShadowSettings::default() }.constants())
        });
        let surfaces = |shading: fn(Shading) -> bool| -> Vec<(String, Constants)> {
            SurfaceVariant::ALL
                .iter()
                .filter(move |variant| shading(variant.shading))
                // Foliage is never transparent: that variant never builds.
                .filter(|variant| {
                    !(variant.shading == Shading::Foliage && variant.coverage == Coverage::Transparent)
                })
                .flat_map(|variant| {
                    shadows.iter().map(move |shadows| {
                        let constants =
                            variant.constants().iter().chain(shadows.iter().flatten()).copied().collect();
                        (format!("{variant:?} {shadows:?}"), constants)
                    })
                })
                .collect()
        };
        // The environment's variants under every tier's specular model.
        let specular =
            [EnvironmentSpecular::Off, EnvironmentSpecular::Approximate, EnvironmentSpecular::Full];
        let environment: Vec<(String, Constants)> = surfaces(|shading| shading == Shading::Environment)
            .into_iter()
            .flat_map(|(name, constants)| {
                specular.map(|model| {
                    let mut constants = constants.clone();
                    constants.push((crate::environment::SPECULAR_CONSTANT, model.constant()));
                    (format!("{name} {model:?}"), constants)
                })
            })
            .collect();
        // Lit lines receive shadows too: each coverage without them and under either filter.
        let outlines: Vec<(String, Constants)> = Coverage::DEPTH_WRITING
            .iter()
            .flat_map(|coverage| {
                shadows.iter().map(move |shadows| {
                    let constants =
                        coverage.constants().iter().chain(shadows.iter().flatten()).copied().collect();
                    (format!("{coverage:?} {shadows:?}"), constants)
                })
            })
            .collect();
        // Foliage's surfaces under every tier's settings and sample count, and its prepasses and
        // casters (whose constants are their coverage's and the wind levels) alike.
        let tiers = [
            FoliageSettings::default(),
            FoliageSettings { wind_levels: 2, translucency: false, ..FoliageSettings::default() },
            FoliageSettings { wind_levels: 0, alpha_to_coverage: false, translucency: false },
        ];
        let mut foliage = Vec::new();
        for (name, constants) in surfaces(|shading| shading == Shading::Foliage) {
            for settings in &tiers {
                for samples in [1u32, 4] {
                    let mut constants = constants.clone();
                    constants.extend(settings.constants());
                    constants.push((foliage::override_constant::SAMPLES, f64::from(samples)));
                    foliage.push((format!("{name} {settings:?} {samples}x"), constants));
                }
            }
        }
        let character = |shading: Shading| matches!(shading, Shading::Character(_));
        let passes = [
            (Program::AnimeCharacter, surfaces(character)),
            (Program::Environment, environment),
            (Program::Outline, outlines),
            (Program::Foliage, foliage),
        ];
        for (program, variants) in passes {
            let name = program.name();
            let module = validate(program);
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(&module)
            .expect("validated above");
            // Every permutation constant the module declares, by name.
            let overrides: Vec<&str> =
                module.overrides.iter().filter_map(|(_, o)| o.name.as_deref()).collect();
            for (variant, variant_constants) in variants {
                let constants: naga::back::PipelineConstants =
                    variant_constants.iter().map(|&(key, value)| (key.to_owned(), value)).collect();
                for key in constants.keys() {
                    assert!(overrides.contains(&key.as_str()), "the {name} program has no override {key}");
                }
                for entry_point in &module.entry_points {
                    let stage = Some((entry_point.stage, entry_point.name.as_str()));
                    naga::back::pipeline_constants::process_overrides(&module, &info, stage, &constants)
                        .unwrap_or_else(|error| {
                            panic!("the {name} program, {variant:?} {}: {error}", entry_point.name)
                        });
                }
            }
            assert!(overrides.contains(&override_constant::ALPHA_TEST), "{name}");
        }
        let character = validate(Program::AnimeCharacter);
        let constants = [
            override_constant::TRANSPARENT,
            override_constant::FACE,
            override_constant::HAIR,
            override_constant::EYE,
        ];
        for constant in constants {
            assert!(
                character.overrides.iter().any(|(_, o)| o.name.as_deref() == Some(constant)),
                "{constant}"
            );
        }
        // Fog cards with and without their depth fade.
        let cards = validate(Program::FogCard);
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&cards)
        .expect("validated above");
        for depth_fade in [false, true] {
            let constants: naga::back::PipelineConstants = crate::fog::card_constants(depth_fade)
                .iter()
                .map(|&(key, value)| (key.to_owned(), value))
                .collect();
            for entry_point in &cards.entry_points {
                let stage = Some((entry_point.stage, entry_point.name.as_str()));
                naga::back::pipeline_constants::process_overrides(&cards, &info, stage, &constants)
                    .unwrap_or_else(|error| {
                        panic!("fog_card.wgsl {} ({depth_fade}): {error}", entry_point.name)
                    });
            }
        }
    }

    /// The fog's numbers as `frame.wgsl` and the `fog` module both state them, and the fog card
    /// shader's facing codes and slot as the asset crate's.
    #[test]
    fn wgsl_fog_constants_match_rust() {
        use sapphire_assets::material::{CardFacing, FogCardMaterial};
        let cards = validate(Program::FogCard);
        assert_eq!(wgsl_f32(&cards, "FOG_CEILING"), crate::fog::CEILING);
        assert_eq!(wgsl_f32(&cards, "FOG_SKY_DISTANCE"), crate::fog::SKY_DISTANCE);
        assert_eq!(wgsl_f32(&cards, "FOG_MAX_EXPONENT"), crate::fog::MAX_EXPONENT);
        assert_eq!(wgsl_f32(&cards, "FOG_MIX_DEPTH"), crate::fog::MIX_DEPTH);
        assert_eq!(wgsl_f32(&cards, "FOG_MIN_DENSITY"), crate::fog::MIN_DENSITY);
        assert_eq!(wgsl_f32(&cards, "FOG_HORIZON_WEIGHT"), crate::fog::HORIZON_WEIGHT);
        assert_eq!(wgsl_f32(&cards, "FOG_SERIES_BELOW"), crate::fog::SERIES_BELOW);
        assert_eq!(wgsl_u32(&cards, "CARD_FACING_WORLD"), CardFacing::World.code());
        assert_eq!(wgsl_u32(&cards, "CARD_FACING_UPRIGHT"), CardFacing::Upright.code());
        assert_eq!(wgsl_u32(&cards, "FOG_CARD_MAP") as usize, FogCardMaterial::MAP);
        assert_eq!(f64::from(wgsl_f32(&cards, "FOG_CARD_TIME_PERIOD")), crate::gpu_types::SHADER_TIME_PERIOD);
        assert!(
            cards.overrides.iter().any(|(_, o)| o.name.as_deref() == Some(crate::fog::DEPTH_FADE_CONSTANT))
        );
    }

    /// The shadow passes' numbers as the shaders and the `shadows` module both state them, and
    /// the shadow program's entry points specialized as their pipelines are.
    #[test]
    fn wgsl_shadow_constants_match_rust() {
        use crate::shadows::{
            FACE_WORKGROUP, MAX_CAPSULES, MAX_CASCADES, MAX_CHARACTER_MAPS, override_constant,
        };
        let character = validate(Program::AnimeCharacter);
        let shadows = validate(Program::Shadows);
        for module in [&character, &shadows] {
            assert_eq!(wgsl_u32(module, "MAX_SHADOW_CASCADES") as usize, MAX_CASCADES);
            assert_eq!(wgsl_u32(module, "MAX_CHARACTER_SHADOW_MAPS") as usize, MAX_CHARACTER_MAPS);
            assert_eq!(wgsl_u32(module, "MAX_SHADOW_CAPSULES") as usize, MAX_CAPSULES);
            for constant in [override_constant::SHADOWS, override_constant::TENT] {
                assert!(
                    module.overrides.iter().any(|(_, o)| o.name.as_deref() == Some(constant)),
                    "{constant}"
                );
            }
        }
        assert_eq!(wgsl_u32(&shadows, "FACE_SHADOW_WORKGROUP"), FACE_WORKGROUP);
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&shadows)
        .expect("validated above");
        let on: naga::back::PipelineConstants =
            [(override_constant::SHADOWS.to_owned(), 1.0)].into_iter().collect();
        for entry_point in &shadows.entry_points {
            let stage = Some((entry_point.stage, entry_point.name.as_str()));
            naga::back::pipeline_constants::process_overrides(&shadows, &info, stage, &on)
                .unwrap_or_else(|error| panic!("shadows.wgsl {}: {error}", entry_point.name));
        }
    }

    /// The ambient passes' numbers as the shaders and the `ambient` module both state them, and
    /// the environment shader with every ambient term on: what a grid and the tiers' occlusion
    /// build.
    #[test]
    fn wgsl_ambient_matches_rust() {
        use crate::ambient::{
            ATLAS_SUB_VOLUMES, GTAO_DEPTH_MIPS, GTAO_WORKGROUP, MAX_CAPSULES, PREFILTER_WORKGROUP,
            PROBE_EDGE_BIT, WORKGROUP, override_constant,
        };
        use sapphire_assets::probes::ProbeData;
        use sapphire_assets::probes::{BRICK_CELLS, BRICK_SIDE, EMPTY_BRICK};
        let ambient = validate(Program::Ambient);
        let environment = validate(Program::Environment);
        for module in [&ambient, &environment] {
            assert_eq!(wgsl_u32(module, "MAX_AMBIENT_CAPSULES") as usize, MAX_CAPSULES);
            assert_eq!(wgsl_u32(module, "ATLAS_SUB_VOLUMES"), ATLAS_SUB_VOLUMES);
            assert_eq!(wgsl_u32(module, "BRICK_CELLS"), BRICK_CELLS);
            assert_eq!(wgsl_u32(module, "BRICK_SIDE"), BRICK_SIDE);
            assert_eq!(wgsl_u32(module, "EMPTY_BRICK"), EMPTY_BRICK);
        }
        assert_eq!(wgsl_u32(&ambient, "AMBIENT_WORKGROUP"), WORKGROUP);
        assert_eq!(wgsl_u32(&ambient, "PREFILTER_WORKGROUP"), PREFILTER_WORKGROUP);
        assert_eq!(wgsl_u32(&ambient, "GTAO_WORKGROUP"), GTAO_WORKGROUP);
        assert_eq!(wgsl_u32(&ambient, "GTAO_DEPTH_MIPS"), GTAO_DEPTH_MIPS);
        // The probe data's bits: the cooked valid bit, and the edge bit the upload sets in the
        // bit cooked grids leave clear.
        assert_eq!(wgsl_u32(&ambient, "PROBE_EDGE_BIT"), PROBE_EDGE_BIT);
        assert_eq!(ProbeData::UNUSED_BITS, PROBE_EDGE_BIT);
        let valid = ProbeData { free: [0; 6], valid: true }.to_bits();
        assert_eq!(wgsl_u32(&ambient, "PROBE_VALID_BIT"), valid);
        assert_eq!(
            1 << (GTAO_DEPTH_MIPS - 1),
            GTAO_WORKGROUP,
            "one workgroup reduces to a texel of the last mip"
        );
        // Every ambient term on, the grid read per pixel and per vertex, in both shaders that
        // read them: the environment's and foliage's (M1 W6).
        let foliage = validate(Program::Foliage);
        for (module, file) in [(&environment, "environment.wgsl"), (&foliage, "foliage.wgsl")] {
            let info = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::empty(),
            )
            .validate(module)
            .expect("validated above");
            for per_vertex in [false, true] {
                let on: naga::back::PipelineConstants =
                    [override_constant::PROBES, override_constant::CAPSULE_AO, override_constant::GTAO]
                        .into_iter()
                        .chain(per_vertex.then_some(override_constant::PROBES_PER_VERTEX))
                        .map(|name| (name.to_owned(), 1.0))
                        .collect();
                for entry_point in &module.entry_points {
                    let stage = Some((entry_point.stage, entry_point.name.as_str()));
                    naga::back::pipeline_constants::process_overrides(module, &info, stage, &on)
                        .unwrap_or_else(|error| panic!("{file} {}: {error}", entry_point.name));
                }
            }
        }
    }
}
