//! WGSL sources, embedded at build time. Each scene pass is `frame.wgsl` followed by the
//! files it uses, its own last; M1 replaces the concatenation with WESL imports (spec §5.12).
//! The post passes share `post.wgsl`, which stands alone.

macro_rules! pass_source {
    ($($file:literal),+) => {
        concat!(include_str!("../shaders/frame.wgsl") $(, "\n", include_str!(concat!("../shaders/", $file)))+)
    };
}

pub(crate) const SKY: &str = pass_source!("sky.wgsl");
pub(crate) const TOON: &str = pass_source!("lights.wgsl", "toon.wgsl");
pub(crate) const OUTLINE: &str = pass_source!("outline.wgsl");
/// The light-cluster compute pass.
pub(crate) const CLUSTERS: &str = pass_source!("lights.wgsl", "clusters.wgsl");
/// Every post pass: the bloom chain and the fused composite.
pub(crate) const POST: &str = include_str!("../shaders/post.wgsl");

#[cfg(test)]
mod tests {
    //! Headless shader checks: CI catches WGSL errors without a GPU. naga comes through the
    //! wgpu re-export, so it is always the exact version wgpu translates with at runtime.

    use std::collections::BTreeMap;

    use sapphire_assets::mesh::{PositionFormat, ToonMaterialParams};
    use sapphire_rhi::wgpu::{self, naga};

    use super::*;
    use crate::gpu_types::{FrameUniforms, InstanceData, ViewUniforms};
    use crate::lights::GpuLight;
    use crate::pipelines::{DRAW_ATTRIBUTES, SHADING_ATTRIBUTES, SURFACE_ATTRIBUTES, position_attributes};
    use crate::post::PostUniforms;

    const PASSES: [(&str, &str, &[&str]); 5] = [
        ("sky", SKY, &["vs_sky", "fs_sky"]),
        ("toon", TOON, &["vs_toon", "fs_toon"]),
        ("outline", OUTLINE, &["vs_outline", "fs_outline"]),
        ("post", POST, &["vs_fullscreen", "fs_bloom_prefilter", "fs_bloom_down", "fs_bloom_up", "fs_post"]),
        ("clusters", CLUSTERS, &["cs_clusters"]),
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
            let attributes: BTreeMap<u32, naga::TypeInner> = DRAW_ATTRIBUTES
                .iter()
                .chain(position_attributes(format))
                .chain(&SHADING_ATTRIBUTES)
                .chain(&SURFACE_ATTRIBUTES)
                .map(|attribute| (attribute.shader_location, shader_input_type(attribute.format)))
                .collect();
            for (name, source, entry_points) in PASSES {
                if matches!(name, "sky" | "post" | "clusters") {
                    continue; // Fullscreen triangles and compute: no vertex buffers.
                }
                let vertex_entry = entry_points[0];
                let module = validate(name, source);
                let entry_point =
                    module.entry_points.iter().find(|ep| ep.name == vertex_entry).expect("entry point");
                let mut inputs = BTreeMap::new();
                for argument in &entry_point.function.arguments {
                    collect_locations(&module, argument.ty, argument.binding.as_ref(), &mut inputs);
                }
                assert_eq!(
                    inputs.keys().collect::<Vec<_>>(),
                    attributes.keys().collect::<Vec<_>>(),
                    "{name}.wgsl vertex input locations vs pipelines.rs layouts ({format:?})"
                );
                for (location, shader) in &inputs {
                    let attribute = &attributes[location];
                    assert!(
                        accepts(shader, attribute),
                        "{name}.wgsl location {location}: {shader:?} can't take {attribute:?} ({format:?})"
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
        }
        assert_eq!(end(&SHADING_ATTRIBUTES), size_of::<VertexShading>() as u64);
        assert_eq!(end(&SURFACE_ATTRIBUTES), size_of::<VertexSurface>() as u64);
        assert_eq!(end(&DRAW_ATTRIBUTES), size_of::<DrawData>() as u64);
        assert_eq!(size_of::<DrawData>(), 48);
    }

    #[test]
    fn rust_structs_match_wgsl_layouts() {
        let expected = [
            ("toon", TOON, "FrameUniforms", size_of::<FrameUniforms>()),
            ("toon", TOON, "ViewUniforms", size_of::<ViewUniforms>()),
            ("toon", TOON, "InstanceData", size_of::<InstanceData>()),
            ("toon", TOON, "ToonMaterial", size_of::<ToonMaterialParams>()),
            ("post", POST, "PostUniforms", size_of::<PostUniforms>()),
            ("toon", TOON, "LocalLight", size_of::<GpuLight>()),
            ("clusters", CLUSTERS, "FrameUniforms", size_of::<FrameUniforms>()),
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
        let toon = validate("toon", TOON);
        let clusters = validate("clusters", CLUSTERS);
        for module in [&toon, &clusters] {
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
}
