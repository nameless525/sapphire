//! WGSL sources, embedded at build time. Each scene pass is `frame.wgsl` followed by the
//! pass's own file; M1 replaces the concatenation with WESL imports (spec §5.12). The post
//! passes share `post.wgsl`, which stands alone.

macro_rules! pass_source {
    ($file:literal) => {
        concat!(include_str!("../shaders/frame.wgsl"), "\n", include_str!(concat!("../shaders/", $file)))
    };
}

pub(crate) const SKY: &str = pass_source!("sky.wgsl");
pub(crate) const TOON: &str = pass_source!("toon.wgsl");
pub(crate) const OUTLINE: &str = pass_source!("outline.wgsl");
/// Every post pass: the bloom chain and the fused composite.
pub(crate) const POST: &str = include_str!("../shaders/post.wgsl");

#[cfg(test)]
mod tests {
    //! Headless shader checks: CI catches WGSL errors without a GPU. naga comes through the
    //! wgpu re-export, so it is always the exact version wgpu translates with at runtime.

    use std::collections::BTreeMap;

    use sapphire_assets::mesh::ToonMaterialParams;
    use sapphire_rhi::wgpu::{self, naga};

    use super::*;
    use crate::gpu_types::{FrameUniforms, InstanceData, ViewUniforms};
    use crate::pipelines::{DRAW_ATTRIBUTES, MESH_VERTEX_ATTRIBUTES};
    use crate::post::PostUniforms;

    const PASSES: [(&str, &str, &[&str]); 4] = [
        ("sky", SKY, &["vs_sky", "fs_sky"]),
        ("toon", TOON, &["vs_toon", "fs_toon"]),
        ("outline", OUTLINE, &["vs_outline", "fs_outline"]),
        ("post", POST, &["vs_fullscreen", "fs_bloom_prefilter", "fs_bloom_down", "fs_bloom_up", "fs_post"]),
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
        match format {
            wgpu::VertexFormat::Float32x2 => TypeInner::Vector { size: VectorSize::Bi, scalar: Scalar::F32 },
            wgpu::VertexFormat::Float32x3 => TypeInner::Vector { size: VectorSize::Tri, scalar: Scalar::F32 },
            wgpu::VertexFormat::Unorm8x4 => TypeInner::Vector { size: VectorSize::Quad, scalar: Scalar::F32 },
            wgpu::VertexFormat::Uint32 => TypeInner::Scalar(Scalar::U32),
            other => panic!("add {other:?} to shader_input_type"),
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
        let expected: BTreeMap<u32, naga::TypeInner> = MESH_VERTEX_ATTRIBUTES
            .iter()
            .chain(&DRAW_ATTRIBUTES)
            .map(|attribute| (attribute.shader_location, shader_input_type(attribute.format)))
            .collect();
        for (name, source, entry_points) in PASSES {
            if matches!(name, "sky" | "post") {
                continue; // Fullscreen triangles: no vertex buffers.
            }
            let vertex_entry = entry_points[0];
            let module = validate(name, source);
            let entry_point =
                module.entry_points.iter().find(|ep| ep.name == vertex_entry).expect("entry point");
            let mut inputs = BTreeMap::new();
            for argument in &entry_point.function.arguments {
                collect_locations(&module, argument.ty, argument.binding.as_ref(), &mut inputs);
            }
            assert_eq!(inputs, expected, "{name}.wgsl vertex inputs vs pipelines.rs layouts");
        }
    }

    #[test]
    fn rust_structs_match_wgsl_layouts() {
        let expected = [
            ("toon", TOON, "FrameUniforms", size_of::<FrameUniforms>()),
            ("toon", TOON, "ViewUniforms", size_of::<ViewUniforms>()),
            ("toon", TOON, "InstanceData", size_of::<InstanceData>()),
            ("toon", TOON, "ToonMaterial", size_of::<ToonMaterialParams>()),
            ("post", POST, "PostUniforms", size_of::<PostUniforms>()),
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
}
