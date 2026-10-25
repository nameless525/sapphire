//! WGSL sources, embedded at build time. Each scene pass is `frame.wgsl` and `materials.wgsl`
//! (the material families' blocks, which frame.wgsl's material binding names) followed by the
//! files it uses, its own last; M1 replaces the concatenation with WESL imports (spec §5.12).
//! The post passes share `post.wgsl`, and compute skinning has `skinning.wgsl`; both stand
//! alone.

macro_rules! pass_source {
    ($($file:literal),+) => {
        concat!(
            include_str!("../shaders/frame.wgsl"),
            "\n",
            include_str!("../shaders/materials.wgsl")
            $(, "\n", include_str!(concat!("../shaders/", $file)))+
        )
    };
}

pub(crate) const SKY: &str = pass_source!("sky.wgsl");
pub(crate) const ANIME_CHARACTER: &str = pass_source!("lights.wgsl", "anime_character.wgsl");
pub(crate) const OUTLINE: &str = pass_source!("outline.wgsl");
/// The light-cluster compute pass.
pub(crate) const CLUSTERS: &str = pass_source!("lights.wgsl", "clusters.wgsl");
/// The character and scene prepasses.
pub(crate) const PREPASS: &str = pass_source!("prepass.wgsl");
/// Compute skinning, which binds its own group 0.
pub(crate) const SKINNING: &str = include_str!("../shaders/skinning.wgsl");
/// Every post pass: the bloom chain and the fused composite.
pub(crate) const POST: &str = include_str!("../shaders/post.wgsl");

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
    use crate::gpu_types::{FrameUniforms, InstanceData, ViewUniforms};
    use crate::lights::GpuLight;
    use crate::pipelines::{
        DRAW_ATTRIBUTES, MeshPass, SHADING_ATTRIBUTES, SURFACE_ATTRIBUTES, position_attributes,
        previous_position_attributes,
    };
    use crate::post::PostUniforms;

    const PASSES: [(&str, &str, &[&str]); 7] = [
        ("sky", SKY, &["vs_sky", "fs_sky"]),
        ("anime_character", ANIME_CHARACTER, &["vs_anime_character", "fs_anime_character"]),
        ("outline", OUTLINE, &["vs_outline", "fs_outline"]),
        ("post", POST, &["vs_fullscreen", "fs_bloom_prefilter", "fs_bloom_down", "fs_bloom_up", "fs_post"]),
        ("clusters", CLUSTERS, &["cs_clusters"]),
        ("prepass", PREPASS, &["vs_character_prepass", "vs_scene_prepass", "fs_scene_prepass"]),
        ("skinning", SKINNING, &["cs_skinning"]),
    ];

    /// Every mesh pass's vertex entry point and the pipeline layout it is built with.
    const MESH_ENTRY_POINTS: [(&str, &str, &str, MeshPass); 4] = [
        ("anime_character", ANIME_CHARACTER, "vs_anime_character", MeshPass::Main),
        ("outline", OUTLINE, "vs_outline", MeshPass::Main),
        ("prepass", PREPASS, "vs_character_prepass", MeshPass::CharacterPrepass),
        ("prepass", PREPASS, "vs_scene_prepass", MeshPass::ScenePrepass),
    ];

    fn validate(name: &str, source: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|err| panic!("{name}.wgsl: {}", err.emit_to_string(source)));
        // No optional capabilities: the shaders must run on every supported device.
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
            .validate(&module)
            .unwrap_or_else(|err| panic!("{name}.wgsl: {}", err.emit_to_string(source)));
        module
    }

    #[test]
    fn every_pass_parses_and_validates() {
        for (name, source, entry_points) in PASSES {
            let module = validate(name, source);
            for &entry_point in entry_points {
                assert!(
                    module.entry_points.iter().any(|ep| ep.name == entry_point),
                    "{name}.wgsl has no entry point {entry_point}"
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
            for (name, source, vertex_entry, pass) in MESH_ENTRY_POINTS {
                let attributes: BTreeMap<u32, naga::TypeInner> = pass
                    .buffers(format)
                    .iter()
                    .flatten()
                    .flat_map(|buffer| buffer.attributes)
                    .map(|attribute| (attribute.shader_location, shader_input_type(attribute.format)))
                    .collect();
                let module = validate(name, source);
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
                // The main pass's shaders read every stream they bind; a prepass may bind a
                // stream for one of its attributes (the scene prepass ignores outline normals).
                if pass == MeshPass::Main {
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
        use sapphire_assets::mesh::{VertexShading, VertexSurface};

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
        assert_eq!(end(&DRAW_ATTRIBUTES), size_of::<DrawData>() as u64);
        assert_eq!(size_of::<DrawData>(), 48);
    }

    #[test]
    fn rust_structs_match_wgsl_layouts() {
        let expected = [
            ("anime_character", ANIME_CHARACTER, "FrameUniforms", size_of::<FrameUniforms>()),
            ("anime_character", ANIME_CHARACTER, "ViewUniforms", size_of::<ViewUniforms>()),
            ("anime_character", ANIME_CHARACTER, "InstanceData", size_of::<InstanceData>()),
            ("anime_character", ANIME_CHARACTER, "AnimeCharacterParams", size_of::<AnimeCharacterParams>()),
            ("anime_character", ANIME_CHARACTER, "AnimeCharacterRegion", size_of::<AnimeCharacterRegion>()),
            ("sky", SKY, "EnvironmentParams", size_of::<EnvironmentParams>()),
            ("sky", SKY, "FoliageParams", size_of::<FoliageParams>()),
            ("sky", SKY, "SkyParams", size_of::<SkyParams>()),
            ("sky", SKY, "FogCardParams", size_of::<FogCardParams>()),
            ("post", POST, "PostUniforms", size_of::<PostUniforms>()),
            ("anime_character", ANIME_CHARACTER, "LocalLight", size_of::<GpuLight>()),
            ("clusters", CLUSTERS, "FrameUniforms", size_of::<FrameUniforms>()),
            ("prepass", PREPASS, "InstanceData", size_of::<InstanceData>()),
            ("prepass", PREPASS, "ViewUniforms", size_of::<ViewUniforms>()),
        ];
        for (source_name, source, name, rust_size) in expected {
            let module = validate(source_name, source);
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
        let module = validate("anime_character", ANIME_CHARACTER);
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
        let character = validate("anime_character", ANIME_CHARACTER);
        let clusters = validate("clusters", CLUSTERS);
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
        let skinning = validate("skinning", SKINNING);
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
        let character = validate("anime_character", ANIME_CHARACTER);
        let slots = [
            ("CHARACTER_BASE_COLOR", M::BASE_COLOR),
            ("CHARACTER_CONTROL", M::CONTROL),
            ("CHARACTER_NORMAL", M::NORMAL),
            ("CHARACTER_RAMP", M::RAMP),
            ("CHARACTER_FACE_SHADOW", M::FACE_SHADOW),
            ("CHARACTER_SURFACE_MASK", M::SURFACE_MASK),
            ("CHARACTER_MATCAP", M::MATCAP),
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
        assert_eq!(bindings, expected);
    }
}
