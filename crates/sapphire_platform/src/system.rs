//! Static facts about the device, captured once at startup.

use sdl3_sys::cpuinfo::{SDL_GetNumLogicalCPUCores, SDL_GetSystemRAM};

/// The operating system the engine was built for (spec §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
    Ios,
    Android,
}

impl Os {
    /// The target this binary was compiled for.
    pub const CURRENT: Self = if cfg!(target_os = "windows") {
        Self::Windows
    } else if cfg!(target_os = "macos") {
        Self::MacOs
    } else if cfg!(target_os = "ios") {
        Self::Ios
    } else if cfg!(target_os = "android") {
        Self::Android
    } else {
        // Other free Unixes share the Linux code paths (SDL with Wayland / X11).
        Self::Linux
    };

    /// The device class this OS implies. Tablets count as mobile.
    pub const fn device_class(self) -> DeviceClass {
        match self {
            Self::Windows | Self::MacOs | Self::Linux => DeviceClass::Desktop,
            Self::Ios | Self::Android => DeviceClass::Mobile,
        }
    }

    /// Whether a window's native surface survives a trip to the background. False only on
    /// Android, which destroys the `ANativeWindow` on every pause (see
    /// [`LifecycleEvent`](crate::LifecycleEvent)); elsewhere the renderer keeps its surface
    /// and merely stops submitting work.
    pub const fn keeps_surface_in_background(self) -> bool {
        !matches!(self, Self::Android)
    }

    /// Whether [`LifecycleEvent::LowMemory`](crate::LifecycleEvent::LowMemory) means the system
    /// is short of memory, so caches that can be rebuilt are worth dropping. True only on iOS,
    /// where it is the system's memory warning. Android's SDL reports every `onTrimMemory` as
    /// `LowMemory`, the one sent on every trip to the background included, so dropping caches
    /// on it would make every return slower; desktops never send it.
    pub const fn memory_warnings_are_pressure(self) -> bool {
        matches!(self, Self::Ios)
    }
}

/// Broad device class, which selects defaults such as input devices, texture cook and
/// quality tier. The single definition lives in `sapphire_core` so the job system can use it
/// without depending on this crate.
pub use sapphire_core::target::FormFactor as DeviceClass;

/// Device facts for budgets, job-system sizing and crash reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemInfo {
    pub os: Os,
    pub device_class: DeviceClass,
    /// Physical RAM in MiB.
    pub ram_mib: u32,
    /// Logical CPU cores (hardware threads), at least 1.
    pub logical_cores: u32,
    /// Running in the iOS Simulator or the Android emulator, whose GPU drivers differ
    /// from real devices (the Android emulator's Vulkan driver crashes on debug labels,
    /// for example). Always `false` on desktop.
    pub is_emulator: bool,
}

impl SystemInfo {
    pub(crate) fn query() -> Self {
        // SAFETY: both functions are thread-safe queries with no preconditions.
        let (ram, cores) = unsafe { (SDL_GetSystemRAM(), SDL_GetNumLogicalCPUCores()) };
        Self {
            os: Os::CURRENT,
            device_class: Os::CURRENT.device_class(),
            ram_mib: u32::try_from(ram).unwrap_or(0),
            logical_cores: u32::try_from(cores).unwrap_or(1).max(1),
            is_emulator: is_emulator(),
        }
    }
}

#[cfg(target_os = "android")]
fn is_emulator() -> bool {
    use std::ffi::{CStr, c_char, c_int};

    /// PROP_VALUE_MAX from <sys/system_properties.h>.
    const PROP_VALUE_MAX: usize = 92;
    unsafe extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    let is_one = |name: &CStr| {
        let mut value = [0 as c_char; PROP_VALUE_MAX];
        // SAFETY: `name` is a valid C string and `value` has the PROP_VALUE_MAX bytes
        // the function may write, including the terminator.
        let len = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
        len == 1 && value[0] == b'1' as c_char
    };
    // The emulator's kernel command line sets one or the other, depending on its age.
    is_one(c"ro.boot.qemu") || is_one(c"ro.kernel.qemu")
}

#[cfg(not(target_os = "android"))]
fn is_emulator() -> bool {
    // The iOS Simulator is a compile-time target of its own.
    cfg!(all(target_os = "ios", target_env = "sim"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review PA-004: only iOS's memory warnings are worth trimming caches for.
    #[test]
    fn only_ios_memory_warnings_are_pressure() {
        for os in [Os::Windows, Os::MacOs, Os::Linux, Os::Ios, Os::Android] {
            assert_eq!(os.memory_warnings_are_pressure(), os == Os::Ios, "{os:?}");
        }
    }

    #[test]
    fn query_reports_this_machine() {
        let info = SystemInfo::query();
        assert_eq!(info.os, Os::CURRENT);
        assert!(info.logical_cores >= 1);
        assert!(info.ram_mib > 0);
        if cfg!(target_os = "macos") {
            assert_eq!(info.os, Os::MacOs);
            assert_eq!(info.device_class, DeviceClass::Desktop);
            assert!(!info.is_emulator);
        }
    }
}
