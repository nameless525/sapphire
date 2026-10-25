//! Capability probing (spec §5.11): what the device can do, filled from the adapter, never
//! from the OS. The renderer selects passes by these plus the tier table.

use wgpu::{
    AdapterInfo, Backend, DownlevelFlags, Features, Limits, TextureFormat, TextureFormatFeatureFlags,
    TextureFormatFeatures, TextureUsages,
};

use crate::LOG_TARGET;

/// Downlevel capabilities every Sapphire renderer path relies on: storage buffers in vertex
/// and fragment shaders. Vulkan, Metal and DX12 always have them, including the iOS
/// Simulator; an adapter without them is rejected instead of failing mid-frame.
///
/// `BASE_VERTEX` is deliberately absent: the CPU-driven renderer path draws with
/// `base_vertex = 0` and `first_instance = 0` so it runs on the iOS Simulator, which lacks
/// both. It is reported as [`GpuCaps::base_vertex_first_instance`] instead.
pub(crate) const REQUIRED_DOWNLEVEL: DownlevelFlags =
    DownlevelFlags::VERTEX_STORAGE.union(DownlevelFlags::FRAGMENT_STORAGE);

/// The bindless group is only useful together, so it is enabled all-or-nothing.
fn binding_array_features() -> Features {
    Features::TEXTURE_BINDING_ARRAY
        | Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
        | Features::PARTIALLY_BOUND_BINDING_ARRAY
}

/// Optional features enabled whenever the adapter has them.
fn optional_features() -> Features {
    // Lets the device use everything the adapter reports per format (2× / 8× MSAA, more
    // renderable and filterable formats) instead of only the WebGPU-guaranteed subset.
    Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
        // HDR scene color without an alpha channel (M1 post-processing).
        | Features::RG11B10UFLOAT_RENDERABLE
        | Features::TEXTURE_COMPRESSION_BC
        | Features::TEXTURE_COMPRESSION_ASTC
        // R16 / RG16 unorm: the desktop cook's 16-bit SDF face maps (M1 A2).
        | Features::TEXTURE_FORMAT_16BIT_NORM
        | Features::TIMESTAMP_QUERY
        | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        | Features::TIMESTAMP_QUERY_INSIDE_PASSES
        | Features::INDIRECT_FIRST_INSTANCE
        | Features::MULTI_DRAW_INDIRECT_COUNT
        | Features::SHADER_F16
        | Features::PIPELINE_CACHE
}

/// What the device can do. Every flag describes the *created device*, so a `true` here means
/// the feature is enabled and usable, not merely present on the adapter.
#[derive(Clone, Debug)]
pub struct GpuCaps {
    pub adapter: AdapterInfo,
    /// Every feature the device was created with.
    pub features: Features,
    /// The adapter's downlevel capabilities.
    pub downlevel: DownlevelFlags,
    /// BC1–7 textures: the `desktop` cook (spec §2).
    pub texture_compression_bc: bool,
    /// ASTC LDR textures: the `mobile` cook.
    pub texture_compression_astc: bool,
    pub timestamp_query: bool,
    /// Timestamps inside render and compute passes. Tile-based GPUs, Apple's included, usually
    /// lack this and can only time whole passes.
    pub timestamp_query_inside_passes: bool,
    /// Non-zero `base_vertex` and `first_instance` in draws (wgpu's `BASE_VERTEX` downlevel
    /// flag covers both). Every real GPU has it; the iOS Simulator does not, because its
    /// Metal device only reports the Apple2 family. Only the future GPU-driven path needs it.
    pub base_vertex_first_instance: bool,
    /// Indirect draws whose `first_instance` selects the draw record: the GPU-driven
    /// multi-draw-indirect path (later, desktop first). Implies
    /// [`Self::base_vertex_first_instance`].
    pub multi_draw_indirect: bool,
    /// `multi_draw_*_indirect_count`; also means multi-draw indirect is native, not emulated.
    pub multi_draw_indirect_count: bool,
    /// Non-uniformly indexed, partially bound texture arrays (bindless materials).
    pub binding_arrays: bool,
    pub shader_f16: bool,
    /// [`wgpu::PipelineCache`] is supported (Vulkan only in wgpu 30).
    pub pipeline_cache: bool,
    /// Limits the device was created with; see [`required_limits`] for how they are chosen.
    pub limits: Limits,
    /// Everything the adapter could offer. Higher tiers may request more in later milestones.
    pub adapter_limits: Limits,
}

impl GpuCaps {
    /// The backend the device runs on.
    pub fn backend(&self) -> Backend {
        self.adapter.backend
    }

    pub(crate) fn probe(adapter: &wgpu::Adapter) -> Self {
        Self::from_parts(
            adapter.get_info(),
            adapter.features(),
            adapter.get_downlevel_capabilities().flags,
            adapter.limits(),
        )
    }

    fn from_parts(
        adapter: AdapterInfo,
        adapter_features: Features,
        downlevel: DownlevelFlags,
        adapter_limits: Limits,
    ) -> Self {
        let mut features = adapter_features & optional_features();
        if adapter_features.contains(binding_array_features()) {
            features |= binding_array_features();
        }
        let limits = required_limits(&adapter_limits, features);
        Self {
            texture_compression_bc: features.contains(Features::TEXTURE_COMPRESSION_BC),
            texture_compression_astc: features.contains(Features::TEXTURE_COMPRESSION_ASTC),
            timestamp_query: features.contains(Features::TIMESTAMP_QUERY),
            timestamp_query_inside_passes: features.contains(
                Features::TIMESTAMP_QUERY
                    | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                    | Features::TIMESTAMP_QUERY_INSIDE_PASSES,
            ),
            base_vertex_first_instance: downlevel.contains(DownlevelFlags::BASE_VERTEX),
            multi_draw_indirect: downlevel
                .contains(DownlevelFlags::INDIRECT_EXECUTION | DownlevelFlags::BASE_VERTEX)
                && features.contains(Features::INDIRECT_FIRST_INSTANCE),
            multi_draw_indirect_count: features.contains(Features::MULTI_DRAW_INDIRECT_COUNT),
            binding_arrays: features.contains(binding_array_features())
                && limits.max_binding_array_elements_per_shader_stage > 0,
            shader_f16: features.contains(Features::SHADER_F16),
            pipeline_cache: features.contains(Features::PIPELINE_CACHE),
            features,
            downlevel,
            limits,
            adapter_limits,
            adapter,
        }
    }

    /// What the device may do with `format`, given the adapter's report for it
    /// (`Adapter::get_texture_format_features`). [`crate::GpuContext::format_support`] does
    /// the lookup.
    ///
    /// This mirrors device validation rather than repeating the adapter's report: without
    /// `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, a WebGPU-compliant device only allows the
    /// guaranteed subset, so for example 2× MSAA would pass the adapter check and then fail
    /// texture creation.
    pub fn format_support(&self, format: TextureFormat, adapter: TextureFormatFeatures) -> FormatSupport {
        if !self.features.contains(format.required_features()) {
            return FormatSupport::default();
        }
        let device_uses_adapter = self.features.contains(Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            || !self.downlevel.contains(DownlevelFlags::WEBGPU_TEXTURE_FORMAT_SUPPORT);
        let mut support = FormatSupport::new(if device_uses_adapter {
            adapter
        } else {
            format.guaranteed_format_features(self.features)
        });
        // wgpu-core can only render to color formats it knows the per-sample size of. Apple
        // GPUs report Rgb9e5Ufloat as renderable, and wgpu 30 panics in pipeline creation
        // instead of failing validation, so such formats are never called renderable here.
        if !format.is_depth_stencil_format() && format.target_pixel_byte_cost().is_none() {
            support.renderable = false;
        }
        support
    }

    pub(crate) fn log(&self) {
        let info = &self.adapter;
        tracing::info!(
            target: LOG_TARGET,
            adapter = %info.name,
            backend = %info.backend,
            device_type = ?info.device_type,
            driver = %info.driver,
            driver_info = %info.driver_info,
            bc = self.texture_compression_bc,
            astc = self.texture_compression_astc,
            timestamps = self.timestamp_query,
            timestamps_in_passes = self.timestamp_query_inside_passes,
            base_vertex_first_instance = self.base_vertex_first_instance,
            mdi = self.multi_draw_indirect,
            mdi_count = self.multi_draw_indirect_count,
            binding_arrays = self.binding_arrays,
            shader_f16 = self.shader_f16,
            pipeline_cache = self.pipeline_cache,
            transient_saves_memory = ?info.transient_saves_memory,
            max_texture_2d = self.limits.max_texture_dimension_2d,
            adapter_max_storage_binding = self.adapter_limits.max_storage_buffer_binding_size,
            adapter_max_storage_buffers_per_stage = self.adapter_limits.max_storage_buffers_per_shader_stage,
            adapter_max_compute_invocations = self.adapter_limits.max_compute_invocations_per_workgroup,
            "GPU capabilities"
        );
    }
}

/// What the device may do with one texture format. See [`GpuCaps::format_support`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormatSupport {
    /// Usable as a render attachment.
    pub renderable: bool,
    /// Supports blending as a color attachment.
    pub blendable: bool,
    /// Supports linear filtering when sampled.
    pub filterable: bool,
    /// Multisampled textures of this format can be resolved.
    pub multisample_resolve: bool,
    /// Bit `n` set: `2^n` samples are supported.
    sample_counts: u8,
}

impl FormatSupport {
    fn new(features: TextureFormatFeatures) -> Self {
        let flags = features.flags;
        let mut sample_counts = 1;
        for (bit, flag) in [
            (1, TextureFormatFeatureFlags::MULTISAMPLE_X2),
            (2, TextureFormatFeatureFlags::MULTISAMPLE_X4),
            (3, TextureFormatFeatureFlags::MULTISAMPLE_X8),
            (4, TextureFormatFeatureFlags::MULTISAMPLE_X16),
        ] {
            if flags.contains(flag) {
                sample_counts |= 1 << bit;
            }
        }
        Self {
            renderable: features.allowed_usages.contains(TextureUsages::RENDER_ATTACHMENT),
            blendable: flags.contains(TextureFormatFeatureFlags::BLENDABLE),
            filterable: flags.contains(TextureFormatFeatureFlags::FILTERABLE),
            multisample_resolve: flags.contains(TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE),
            sample_counts,
        }
    }

    /// Whether render attachments with `count` samples can be created (1 always can, for a
    /// renderable format).
    pub fn supports_samples(&self, count: u32) -> bool {
        count.is_power_of_two() && count <= 16 && self.sample_counts & (1 << count.trailing_zeros()) != 0
    }
}

/// The limits every Sapphire device is created with.
///
/// The engine requests a fixed floor rather than everything the adapter offers, for two
/// reasons: device creation can't fail on a Vulkan 1.1 phone because a desktop-sized limit was
/// asked for, and wgpu validates against the *requested* limits, so code that quietly depends
/// on a desktop-only limit fails on the developer's machine instead of on a phone.
///
/// The floor is wgpu's `downlevel_defaults` (every value at or below the Vulkan 1.1 core
/// minimum: 4 bind groups, 4 storage buffers per stage, 16 KiB uniform bindings, 128 MiB
/// storage bindings, 4 color attachments) with compute workgroups lowered to the Vulkan
/// minimum of 128 invocations and 128×128×64. Three groups come from the adapter instead:
/// - texture resolution, so swapchains on 5K/6K desktop displays work (asset textures are
///   capped by the content budgets in spec §3.4, not by this limit);
/// - buffer offset alignments, where the adapter's value is always at least as permissive;
/// - binding-array sizes, when the bindless features were enabled.
pub(crate) fn required_limits(adapter: &Limits, features: Features) -> Limits {
    let mut limits = Limits {
        max_compute_invocations_per_workgroup: 128,
        max_compute_workgroup_size_x: 128,
        max_compute_workgroup_size_y: 128,
        max_compute_workgroup_size_z: 64,
        ..Limits::downlevel_defaults()
    }
    .using_resolution(adapter.clone())
    .using_alignment(adapter.clone());
    if features.contains(binding_array_features()) {
        limits.max_binding_array_elements_per_shader_stage =
            adapter.max_binding_array_elements_per_shader_stage;
        limits.max_binding_array_sampler_elements_per_shader_stage =
            adapter.max_binding_array_sampler_elements_per_shader_stage;
    }
    limits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What wgpu-hal reports for a device at exactly the Vulkan 1.1 core minimums.
    fn vulkan_1_1_minimum() -> Limits {
        Limits {
            max_texture_dimension_1d: 4096,
            max_texture_dimension_2d: 4096,
            max_texture_dimension_3d: 256,
            max_texture_array_layers: 256,
            max_bind_groups: 4,
            max_bind_groups_plus_vertex_buffers: u32::MAX,
            max_bindings_per_bind_group: 1024,
            max_dynamic_uniform_buffers_per_pipeline_layout: 8,
            max_dynamic_storage_buffers_per_pipeline_layout: 4,
            max_sampled_textures_per_shader_stage: 16,
            max_samplers_per_shader_stage: 16,
            max_storage_buffers_per_shader_stage: 4,
            max_storage_textures_per_shader_stage: 4,
            max_uniform_buffers_per_shader_stage: 12,
            max_uniform_buffer_binding_size: 16 << 10,
            max_storage_buffer_binding_size: 128 << 20,
            max_vertex_buffers: 16,
            max_buffer_size: 1 << 30,
            max_vertex_attributes: 16,
            max_vertex_buffer_array_stride: 2048,
            max_inter_stage_shader_variables: 64 / 4 - 1,
            min_uniform_buffer_offset_alignment: 256,
            min_storage_buffer_offset_alignment: 256,
            max_color_attachments: 4,
            max_color_attachment_bytes_per_sample: 4 * TextureFormat::MAX_TARGET_PIXEL_BYTE_COST,
            max_compute_workgroup_storage_size: 16 << 10,
            max_compute_invocations_per_workgroup: 128,
            max_compute_workgroup_size_x: 128,
            max_compute_workgroup_size_y: 128,
            max_compute_workgroup_size_z: 64,
            max_compute_workgroups_per_dimension: 65535,
            max_immediate_size: 128,
            max_non_sampler_bindings: u32::MAX,
            max_buffers_and_acceleration_structures_per_shader_stage: u32::MAX,
            ..Limits::defaults()
        }
    }

    #[test]
    fn floor_fits_a_minimum_vulkan_1_1_phone() {
        let adapter = vulkan_1_1_minimum();
        let required = required_limits(&adapter, Features::empty());
        let mut failures = Vec::new();
        required.check_limits_with_fail_fn(&adapter, false, |name, want, have| {
            failures.push(format!("{name}: want {want}, adapter has {have}"));
        });
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn floor_takes_resolution_from_the_adapter() {
        let desktop = Limits { max_texture_dimension_2d: 16384, ..Limits::defaults() };
        assert_eq!(required_limits(&desktop, Features::empty()).max_texture_dimension_2d, 16384);
    }

    #[test]
    fn caps_report_only_enabled_features() {
        let adapter_features = Features::TEXTURE_COMPRESSION_ASTC
            | Features::TIMESTAMP_QUERY
            | Features::TEXTURE_BINDING_ARRAY
            | Features::POLYGON_MODE_LINE;
        let caps = GpuCaps::from_parts(
            AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, Backend::Vulkan),
            adapter_features,
            DownlevelFlags::all(),
            vulkan_1_1_minimum(),
        );
        assert!(caps.texture_compression_astc && caps.timestamp_query);
        assert!(!caps.texture_compression_bc && !caps.timestamp_query_inside_passes);
        // An incomplete bindless group is not enabled at all.
        assert!(!caps.binding_arrays && !caps.features.contains(Features::TEXTURE_BINDING_ARRAY));
        // Features the engine doesn't use stay off.
        assert!(!caps.features.contains(Features::POLYGON_MODE_LINE));
        assert!(!caps.multi_draw_indirect);
    }

    /// What wgpu-hal 30's Metal backend reports for the iOS Simulator, whose device only has
    /// the Apple2 family (legacy feature-set queries all fail), using the pessimistic branch
    /// wherever the value depends on the host GPU.
    fn ios_simulator() -> (DownlevelFlags, Limits) {
        let downlevel = DownlevelFlags::all()
            - DownlevelFlags::BASE_VERTEX
            - DownlevelFlags::INDIRECT_EXECUTION
            - DownlevelFlags::CUBE_ARRAY_TEXTURES
            - DownlevelFlags::COMPARISON_SAMPLERS;
        let buffers = 29; // Metal's 31 buffer slots minus 2 wgpu reserves.
        let limits = Limits {
            max_texture_dimension_1d: 8192,
            max_texture_dimension_2d: 8192,
            max_texture_dimension_3d: 2048,
            max_texture_array_layers: 2048,
            max_bind_groups: u32::MAX,
            max_bind_groups_plus_vertex_buffers: u32::MAX,
            max_bindings_per_bind_group: u32::MAX,
            max_dynamic_uniform_buffers_per_pipeline_layout: buffers,
            max_dynamic_storage_buffers_per_pipeline_layout: buffers,
            max_samplers_per_shader_stage: 16,
            max_sampled_textures_per_shader_stage: 23,
            max_storage_textures_per_shader_stage: 8,
            max_storage_buffers_per_shader_stage: buffers,
            max_uniform_buffers_per_shader_stage: buffers,
            max_vertex_buffers: buffers,
            max_buffer_size: 256 << 20,
            max_uniform_buffer_binding_size: 256 << 20,
            max_storage_buffer_binding_size: 256 << 20,
            min_uniform_buffer_offset_alignment: 32,
            min_storage_buffer_offset_alignment: 32,
            max_vertex_attributes: 31,
            max_vertex_buffer_array_stride: 2048,
            max_inter_stage_shader_variables: 15,
            max_color_attachments: 4,
            max_color_attachment_bytes_per_sample: 32,
            max_compute_workgroup_storage_size: 16 << 10,
            max_compute_invocations_per_workgroup: 512,
            max_compute_workgroup_size_x: 512,
            max_compute_workgroup_size_y: 512,
            max_compute_workgroup_size_z: 512,
            max_compute_workgroups_per_dimension: 0xFFFF,
            max_immediate_size: 0x1000,
            max_non_sampler_bindings: u32::MAX,
            max_buffers_and_acceleration_structures_per_shader_stage: buffers,
            ..Limits::defaults()
        };
        (downlevel, limits)
    }

    #[test]
    fn ios_simulator_is_usable_without_base_vertex() {
        let (downlevel, adapter_limits) = ios_simulator();
        assert!(downlevel.contains(REQUIRED_DOWNLEVEL));
        let caps = GpuCaps::from_parts(
            AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, Backend::Metal),
            Features::TEXTURE_COMPRESSION_ASTC,
            downlevel,
            adapter_limits.clone(),
        );
        assert!(caps.limits.check_limits(&adapter_limits));
        assert!(!caps.base_vertex_first_instance && !caps.multi_draw_indirect);
    }

    #[test]
    fn format_support_follows_device_validation() {
        let format = TextureFormat::Rgba8UnormSrgb;
        let adapter = TextureFormatFeatures {
            allowed_usages: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            flags: TextureFormatFeatureFlags::MULTISAMPLE_X2
                | TextureFormatFeatureFlags::MULTISAMPLE_X4
                | TextureFormatFeatureFlags::MULTISAMPLE_X8
                | TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE
                | TextureFormatFeatureFlags::BLENDABLE
                | TextureFormatFeatureFlags::FILTERABLE,
        };
        let caps = |features| {
            GpuCaps::from_parts(
                AdapterInfo::new(wgpu::DeviceType::IntegratedGpu, Backend::Metal),
                features,
                DownlevelFlags::all(),
                vulkan_1_1_minimum(),
            )
        };

        // Without adapter-specific format features, only the WebGPU guarantees apply.
        let webgpu = caps(Features::empty()).format_support(format, adapter);
        assert!(webgpu.renderable && webgpu.supports_samples(1) && webgpu.supports_samples(4));
        assert!(!webgpu.supports_samples(2) && !webgpu.supports_samples(8));

        let native = caps(Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES).format_support(format, adapter);
        assert!(native.supports_samples(2) && native.supports_samples(8) && native.multisample_resolve);
        assert!(!native.supports_samples(16) && !native.supports_samples(3));

        // Color formats wgpu can't render to stay non-renderable whatever the adapter says.
        let rgb9e5 = caps(Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            .format_support(TextureFormat::Rgb9e5Ufloat, adapter);
        assert!(!rgb9e5.renderable && rgb9e5.filterable);

        // Formats whose feature isn't enabled aren't usable at all.
        let bc = caps(Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            .format_support(TextureFormat::Bc7RgbaUnormSrgb, adapter);
        assert_eq!(bc, FormatSupport::default());
    }
}
