//! Backend policy (spec §2) and instance creation.
//!
//! This module is the one place where the engine picks a GPU API per platform (rule R7).
//! Everything downstream branches on [`crate::GpuCaps`], never on the OS.

use wgpu::{Backend, Backends, InstanceFlags};

use crate::{LOG_TARGET, RhiError};

/// Environment variable that forces a single backend: `vulkan`, `metal` or `dx12`.
///
/// It takes precedence over [`GpuInstanceDesc::backend`], so a shipped build can be pointed
/// at a different backend for a bug report without a rebuild.
pub const BACKEND_OVERRIDE_ENV: &str = "SAPPHIRE_GPU_BACKEND";

/// Environment variable that overrides [`GpuInstanceDesc::debug`]: `none`, `validation`,
/// `labels` or `all`. Meant for desktop debugging sessions; Android apps can't be given
/// environment variables from adb.
pub const DEBUG_OVERRIDE_ENV: &str = "SAPPHIRE_GPU_DEBUG";

/// GPU debugging aids. They are independent because they fail independently: the Android
/// emulator's Vulkan driver (gfxstream) crashes on object naming but runs validation fine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GpuDebug {
    /// The backend's validation layer: Vulkan validation layers (when installed), Metal API
    /// validation, the D3D12 debug layer.
    pub validation: bool,
    /// Debug names on GPU objects and debug info in generated shaders, for RenderDoc, Xcode
    /// and PIX captures. On Vulkan this is what enables `VK_EXT_debug_utils`, which also
    /// routes validation messages into the engine log; without it the validation layer
    /// reports through its own output (logcat on Android).
    pub object_labels: bool,
}

impl GpuDebug {
    pub const NONE: Self = Self { validation: false, object_labels: false };
    pub const VALIDATION: Self = Self { validation: true, object_labels: false };
    pub const LABELS: Self = Self { validation: false, object_labels: true };
    pub const ALL: Self = Self { validation: true, object_labels: true };

    /// Everything on in debug builds, everything off in release builds.
    pub const fn from_build_config() -> Self {
        if cfg!(debug_assertions) { Self::ALL } else { Self::NONE }
    }

    fn instance_flags(self) -> InstanceFlags {
        // Release builds keep wgpu's indirect-call validation: skipping it is undefined
        // behavior for malformed indirect draws.
        let mut flags = InstanceFlags::VALIDATION_INDIRECT_CALL;
        flags.set(InstanceFlags::VALIDATION, self.validation);
        flags.set(InstanceFlags::DEBUG, self.object_labels);
        // Without DEBUG, wgpu-hal's Vulkan backend never loads VK_EXT_debug_utils, so no object
        // is ever named; discarding labels keeps them from every other backend too.
        flags.set(InstanceFlags::DISCARD_HAL_LABELS, !self.object_labels);
        flags
    }
}

impl Default for GpuDebug {
    fn default() -> Self {
        Self::from_build_config()
    }
}

/// Instance-level GPU options.
#[derive(Clone, Debug, Default)]
pub struct GpuInstanceDesc {
    /// Forces one backend instead of the platform policy. [`BACKEND_OVERRIDE_ENV`] wins over this.
    pub backend: Option<Backend>,
    /// Validation and object labels. Defaults to both on in debug builds. [`DEBUG_OVERRIDE_ENV`]
    /// wins over this, and wgpu's own `WGPU_VALIDATION` / `WGPU_DEBUG` variables apply last,
    /// for one-off debugging.
    pub debug: GpuDebug,
}

/// A wgpu instance plus the ordered list of backends to try adapters on.
///
/// It exists before the device so the first [`crate::RenderSurface`] can be created early and
/// passed to [`crate::GpuContext::new`] as the surface the adapter must be able to present to.
#[derive(Clone, Debug)]
pub struct GpuInstance {
    instance: wgpu::Instance,
    backends: Vec<Backend>,
    debug: GpuDebug,
}

impl GpuInstance {
    /// Creates the instance for the platform policy, or for the override if one is set.
    pub fn new(desc: &GpuInstanceDesc) -> Result<Self, RhiError> {
        let env_override = std::env::var(BACKEND_OVERRIDE_ENV).ok();
        let backends = resolve_backends(desc.backend, env_override.as_deref())?;

        let compiled = wgpu::Instance::enabled_backend_features();
        if let Some(&missing) = backends.iter().find(|&&b| !compiled.contains(b.into())) {
            return Err(RhiError::BackendUnavailable(missing));
        }

        let requested = match std::env::var(DEBUG_OVERRIDE_ENV) {
            Ok(value) if !value.trim().is_empty() => parse_debug(&value)?,
            _ => desc.debug,
        };
        let flags = requested.instance_flags().with_env();
        // Report what wgpu will actually do, including its own environment overrides.
        let effective = GpuDebug {
            validation: flags.contains(InstanceFlags::VALIDATION),
            object_labels: flags.contains(InstanceFlags::DEBUG),
        };

        let mask = backends.iter().fold(Backends::empty(), |mask, &b| mask | b.into());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: mask,
            flags,
            backend_options: wgpu::BackendOptions::default().with_env(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        tracing::info!(
            target: LOG_TARGET,
            backends = ?backends,
            validation = effective.validation,
            object_labels = effective.object_labels,
            "GPU instance created"
        );
        Ok(Self { instance, backends, debug: effective })
    }

    /// The debugging aids in effect, after environment overrides.
    pub fn debug(&self) -> GpuDebug {
        self.debug
    }

    /// Backends to try, most preferred first.
    pub fn backends(&self) -> &[Backend] {
        &self.backends
    }

    /// The underlying wgpu instance.
    pub fn raw(&self) -> &wgpu::Instance {
        &self.instance
    }
}

/// The spec §2 policy, most preferred first.
fn platform_backends() -> &'static [Backend] {
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        &[Backend::Metal]
    } else if cfg!(target_os = "windows") {
        // Vulkan matches Linux and Android and is what dlss_wgpu needs; DX12 catches machines
        // whose Vulkan driver is missing or fails device creation (§2 note 2).
        &[Backend::Vulkan, Backend::Dx12]
    } else {
        // Linux, Android: Vulkan only. Android requires Vulkan 1.1 and never falls back to GLES.
        &[Backend::Vulkan]
    }
}

fn parse_backend(value: &str) -> Result<Backend, RhiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => Ok(Backend::Vulkan),
        "metal" | "mtl" => Ok(Backend::Metal),
        "dx12" | "d3d12" => Ok(Backend::Dx12),
        _ => Err(RhiError::InvalidBackendOverride { value: value.to_owned() }),
    }
}

fn parse_debug(value: &str) -> Result<GpuDebug, RhiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" | "off" | "0" => Ok(GpuDebug::NONE),
        "validation" => Ok(GpuDebug::VALIDATION),
        "labels" => Ok(GpuDebug::LABELS),
        "all" | "on" | "1" => Ok(GpuDebug::ALL),
        _ => Err(RhiError::InvalidDebugOverride { value: value.to_owned() }),
    }
}

fn resolve_backends(requested: Option<Backend>, env: Option<&str>) -> Result<Vec<Backend>, RhiError> {
    if let Some(value) = env.filter(|v| !v.trim().is_empty()) {
        return Ok(vec![parse_backend(value)?]);
    }
    Ok(match requested {
        Some(backend) => vec![backend],
        None => platform_backends().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_overrides() {
        assert_eq!(parse_backend("vulkan").ok(), Some(Backend::Vulkan));
        assert_eq!(parse_backend(" Metal ").ok(), Some(Backend::Metal));
        assert_eq!(parse_backend("DX12").ok(), Some(Backend::Dx12));
        assert!(matches!(parse_backend("gl"), Err(RhiError::InvalidBackendOverride { .. })));
    }

    #[test]
    fn parses_debug_overrides() {
        assert_eq!(parse_debug("none").ok(), Some(GpuDebug::NONE));
        assert_eq!(parse_debug(" Validation ").ok(), Some(GpuDebug::VALIDATION));
        assert_eq!(parse_debug("labels").ok(), Some(GpuDebug::LABELS));
        assert_eq!(parse_debug("ALL").ok(), Some(GpuDebug::ALL));
        assert!(matches!(parse_debug("verbose"), Err(RhiError::InvalidDebugOverride { .. })));
    }

    #[test]
    fn labels_off_never_enables_object_naming() {
        let flags = GpuDebug::VALIDATION.instance_flags();
        assert!(flags.contains(InstanceFlags::VALIDATION | InstanceFlags::DISCARD_HAL_LABELS));
        assert!(!flags.contains(InstanceFlags::DEBUG));
        let flags = GpuDebug::LABELS.instance_flags();
        assert!(flags.contains(InstanceFlags::DEBUG));
        assert!(!flags.intersects(InstanceFlags::VALIDATION | InstanceFlags::DISCARD_HAL_LABELS));
        // Indirect-call validation is a safety net, not a debugging aid: always on.
        assert!(GpuDebug::NONE.instance_flags().contains(InstanceFlags::VALIDATION_INDIRECT_CALL));
    }

    #[test]
    fn env_beats_descriptor_beats_platform() {
        assert_eq!(resolve_backends(Some(Backend::Metal), Some("dx12")).ok(), Some(vec![Backend::Dx12]));
        assert_eq!(resolve_backends(Some(Backend::Metal), Some("  ")).ok(), Some(vec![Backend::Metal]));
        assert_eq!(resolve_backends(None, None).ok(), Some(platform_backends().to_vec()));
    }

    #[test]
    fn platform_policy_is_compiled_in() {
        let compiled = wgpu::Instance::enabled_backend_features();
        assert!(!platform_backends().is_empty());
        for &backend in platform_backends() {
            assert!(compiled.contains(backend.into()), "{backend} is policy but not compiled");
        }
    }
}
