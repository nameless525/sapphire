//! How hot the OS says the device is running (spec §6.3): a reading on one scale for every OS, so a phone's
//! soak can say whether it was throttled (M2's exit test 4). Nothing responds to it yet: the ladder in
//! spec §6.3 comes with M4's thermal tiers.

/// How far into its thermal limits the OS says the device is. The OS decides, by its own rules for that
/// device, so a state means the same thing to a game on one phone and says nothing across phones about
/// temperature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThermalState {
    /// The OS doesn't say: a desktop, an Android before API 30, or a read that failed.
    #[default]
    Unknown,
    /// iOS's `nominal`, Android's `NONE`.
    Nominal,
    /// iOS's `fair`, Android's `LIGHT` and `MODERATE`: warm, with little or no effect on performance.
    Fair,
    /// iOS's `serious`, Android's `SEVERE`: the OS is reducing performance.
    Serious,
    /// iOS's `critical`, Android's `CRITICAL` and past it: the OS is doing all it can to cool down.
    Critical,
}

/// Android's `AThermalStatus` (`thermal.h`) on our scale.
#[cfg(any(target_os = "android", test))]
const fn from_android_status(status: i32) -> ThermalState {
    match status {
        0 => ThermalState::Nominal,
        1 | 2 => ThermalState::Fair,
        3 => ThermalState::Serious,
        4.. => ThermalState::Critical,
        // ATHERMAL_STATUS_ERROR.
        _ => ThermalState::Unknown,
    }
}

/// What the OS says now.
pub(crate) fn current() -> ThermalState {
    imp::current()
}

#[cfg(target_os = "ios")]
mod imp {
    use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};

    use super::ThermalState;

    pub(super) fn current() -> ThermalState {
        match NSProcessInfo::processInfo().thermalState() {
            NSProcessInfoThermalState::Nominal => ThermalState::Nominal,
            NSProcessInfoThermalState::Fair => ThermalState::Fair,
            NSProcessInfoThermalState::Serious => ThermalState::Serious,
            NSProcessInfoThermalState::Critical => ThermalState::Critical,
            _ => ThermalState::Unknown,
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::{CStr, c_int, c_void};
    use std::mem;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use super::{ThermalState, from_android_status};

    type AcquireManager = unsafe extern "C" fn() -> *mut c_void;
    type CurrentStatus = unsafe extern "C" fn(*mut c_void) -> c_int;

    /// The thermal manager and its reader, newer than the app's minimum (API 30 against 29), so
    /// resolved at run time.
    struct Api {
        manager: NonNull<c_void>,
        current_status: CurrentStatus,
    }

    // SAFETY: the manager is never released, and only `Platform::thermal_state`, which the thread that
    // owns the platform calls, reads it.
    unsafe impl Send for Api {}
    // SAFETY: as above.
    unsafe impl Sync for Api {}

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| {
            // SAFETY: libandroid.so is a system library the app already links against (dlopen only
            // takes another reference to it), and the handle is never closed.
            let library = NonNull::new(unsafe { libc::dlopen(c"libandroid.so".as_ptr(), libc::RTLD_NOW) })?;
            // SAFETY: `library` is a live handle and `name` a C string.
            let symbol = |name: &CStr| NonNull::new(unsafe { libc::dlsym(library.as_ptr(), name.as_ptr()) });
            let acquire = symbol(c"AThermal_acquireManager")?;
            let current_status = symbol(c"AThermal_getCurrentThermalStatus")?;
            // SAFETY: a function symbol's address is the function, and the NDK declares these two with
            // exactly these signatures (`thermal.h`).
            let (acquire, current_status) = unsafe {
                (
                    mem::transmute::<*mut c_void, AcquireManager>(acquire.as_ptr()),
                    mem::transmute::<*mut c_void, CurrentStatus>(current_status.as_ptr()),
                )
            };
            // SAFETY: no arguments and no preconditions; it returns null on failure.
            let manager = NonNull::new(unsafe { acquire() })?;
            Some(Api { manager, current_status })
        })
        .as_ref()
    }

    pub(super) fn current() -> ThermalState {
        let Some(api) = api() else { return ThermalState::Unknown };
        // SAFETY: the manager came from `AThermal_acquireManager` and is never released.
        from_android_status(unsafe { (api.current_status)(api.manager.as_ptr()) })
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    use super::ThermalState;

    pub(super) const fn current() -> ThermalState {
        ThermalState::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn androids_statuses_map_onto_one_scale() {
        for (status, state) in [
            (-1, ThermalState::Unknown),
            (0, ThermalState::Nominal),
            (1, ThermalState::Fair),
            (2, ThermalState::Fair),
            (3, ThermalState::Serious),
            (4, ThermalState::Critical),
            (5, ThermalState::Critical),
            (6, ThermalState::Critical),
        ] {
            assert_eq!(from_android_status(status), state, "status {status}");
        }
    }

    #[test]
    fn hotter_states_order_above_cooler_ones() {
        assert!(ThermalState::Unknown < ThermalState::Nominal);
        assert!(ThermalState::Nominal < ThermalState::Fair);
        assert!(ThermalState::Fair < ThermalState::Serious);
        assert!(ThermalState::Serious < ThermalState::Critical);
    }
}
