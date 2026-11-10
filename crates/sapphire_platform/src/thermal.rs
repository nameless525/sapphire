//! What the OS says about heat and power (spec §6.3, M4 T1): one [`ThermalReading`] a frame, from
//! whichever sources the OS has, each resolved at run time and absent where it's missing.
//!
//! - **Android:** the thermal status (`AThermal_getCurrentThermalStatus`, API 30) and the headroom
//!   forecast (`AThermal_getThermalHeadroom`, API 31), both through the thermal manager, resolved with
//!   `dlsym` since the app's minimum is API 29. The frame thread's ADPF hint session (`hint.rs`, API
//!   33) belongs to the frame clock, which reports its frames; the reading says whether it's live.
//! - **iOS:** `NSProcessInfo`'s `thermalState` and Low Power Mode (`isLowPowerModeEnabled`).
//! - **Desktops:** none; the state reads [`ThermalState::Unknown`].
//!
//! The OS is asked on the frame thread, no more often than it wants: the state (a binder call on
//! Android) and Low Power Mode once a second, and headroom once every ten seconds, forecasting ten
//! seconds ahead. Android's header says calling headroom much more often than once a second may
//! return NaN; Google's game guide is stricter, at most once every ten seconds on older devices, and
//! from one thread (both read 2026-11-10). Between polls a frame gets the last reading, so reading it
//! every frame costs a clock read and a few atomic loads.
//!
//! An OS value out of range (a status Android doesn't define, a negative or infinite headroom, an iOS
//! state past `critical`) is logged once and read as absent or `Unknown`, never a panic. A first
//! headroom of NaN is Google's sign that the device lacks the forecast: headroom is then absent for
//! the run. A headroom of exactly 0 reads as absent too, since older devices that lack the forecast
//! report 0 (Google's guide), while a cool device that has it soon reads above 0.
//!
//! **Forcing a reading** (development): the `thermal.state`, `thermal.headroom` and `thermal.low_power`
//! cvars ([`CVARS`]) replace what the OS says, each for its own source, so the quality ladder can be
//! driven on a Mac and in tests. Shipping builds have no console, so they read the OS.

use sapphire_core::cvar::{AnyCvar, Cvar, CvarKind, CvarType, CvarValue};
pub use sapphire_core::thermal::{ThermalReading, ThermalState};

use crate::LOG_TARGET;

/// How often the state and Low Power Mode are asked for, in nanoseconds.
const STATE_POLL_NS: u64 = 1_000_000_000;
/// How often headroom is asked for, in nanoseconds: Google's game guide's limit (module docs).
const HEADROOM_POLL_NS: u64 = 10_000_000_000;
/// How far ahead headroom forecasts, in seconds: one poll ahead.
const HEADROOM_FORECAST_SECONDS: i32 = 10;

/// What an OS reports, raw: the thermal monitor's view of `imp`, so tests can stand in for it.
pub(crate) trait Source {
    /// The OS's thermal state, `Unknown` if it has none or the read failed.
    fn state(&mut self) -> ThermalState;
    /// Whether the OS has a headroom forecast at all.
    fn has_headroom(&self) -> bool;
    /// The headroom forecast `forecast_seconds` ahead, as the OS gave it (NaN included).
    fn headroom(&mut self, forecast_seconds: i32) -> f32;
    /// Whether Low Power Mode is on, where the OS has one.
    fn low_power(&mut self) -> Option<bool>;
}

/// The platform's thermal monitor: polls the OS's sources when they're due.
#[derive(Debug)]
pub(crate) struct Thermal<S = imp::Os> {
    source: S,
    /// What the OS said when last asked, without the cvars' overrides.
    os: ThermalReading,
    headroom: Headroom,
    /// When the state and Low Power Mode are next due, in nanoseconds; `None` before the first poll.
    state_due: Option<u64>,
    headroom_due: Option<u64>,
    /// A headroom out of range has been logged.
    warned_headroom: bool,
}

/// Whether headroom is worth asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Headroom {
    /// The OS has the call; not asked yet.
    Untried,
    /// The first read gave a number.
    Read,
    /// The OS lacks it, or its first read was NaN.
    Unsupported,
}

impl Thermal {
    pub fn new() -> Self {
        Self::with_source(imp::Os::new())
    }
}

impl<S: Source> Thermal<S> {
    fn with_source(source: S) -> Self {
        let headroom = if source.has_headroom() { Headroom::Untried } else { Headroom::Unsupported };
        Self {
            source,
            os: ThermalReading::default(),
            headroom,
            state_due: None,
            headroom_due: None,
            warned_headroom: false,
        }
    }

    /// The reading at `now` (nanoseconds on any steady clock), asking the OS for what's due, with
    /// `hint_session` (the frame clock's) and the cvars' overrides.
    pub fn read(&mut self, now: u64, hint_session: Option<bool>) -> ThermalReading {
        if self.state_due.is_none_or(|due| now >= due) {
            self.state_due = Some(now.saturating_add(STATE_POLL_NS));
            let state = self.source.state();
            if state != self.os.state {
                tracing::info!(target: LOG_TARGET, from = ?self.os.state, to = ?state, "thermal state changed");
            }
            self.os.state = state;
            self.os.low_power = self.source.low_power();
        }
        if self.headroom != Headroom::Unsupported && self.headroom_due.is_none_or(|due| now >= due) {
            self.headroom_due = Some(now.saturating_add(HEADROOM_POLL_NS));
            self.os.headroom = self.poll_headroom();
        }
        self.os.hint_session = hint_session;
        forced(self.os)
    }

    fn poll_headroom(&mut self) -> Option<f32> {
        let headroom = self.source.headroom(HEADROOM_FORECAST_SECONDS);
        let first = self.headroom == Headroom::Untried;
        if headroom.is_nan() {
            if first {
                tracing::info!(target: LOG_TARGET, "the OS has no thermal headroom forecast (its first read is NaN)");
                self.headroom = Headroom::Unsupported;
            }
            return None;
        }
        self.headroom = Headroom::Read;
        if !(headroom.is_finite() && headroom >= 0.0) {
            if !self.warned_headroom {
                self.warned_headroom = true;
                tracing::warn!(target: LOG_TARGET, headroom, "the OS gave a thermal headroom out of range; reading none");
            }
            return None;
        }
        // Older devices without the forecast report 0 (module docs).
        (headroom > 0.0).then_some(headroom)
    }
}

/// `reading` with whatever the cvars force.
fn forced(mut reading: ThermalReading) -> ThermalReading {
    if let ForcedState::State(state) = FORCE_STATE.get() {
        reading.state = state;
    }
    let headroom = FORCE_HEADROOM.get();
    if headroom >= 0.0 {
        reading.headroom = Some(headroom);
    }
    match FORCE_LOW_POWER.get() {
        ForcedSwitch::Os => {}
        ForcedSwitch::Off => reading.low_power = Some(false),
        ForcedSwitch::On => reading.low_power = Some(true),
    }
    reading
}

/// The thermal state the OS reports, or one forced in its place: `thermal.state`'s values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd)]
pub enum ForcedState {
    /// What the OS says.
    #[default]
    Os,
    State(ThermalState),
}

const STATE_CHOICES: [&str; 7] = ["os", "unknown", "nominal", "light", "moderate", "severe", "critical"];

impl CvarType for ForcedState {
    const KIND: CvarKind = CvarKind::Choice(&STATE_CHOICES);

    fn to_bits(self) -> u32 {
        match self {
            Self::Os => 0,
            Self::State(state) => {
                1 + ThermalState::ALL.iter().position(|&each| each == state).expect("ALL lists every state")
                    as u32
            }
        }
    }

    fn from_bits(bits: u32) -> Self {
        match bits.checked_sub(1) {
            Some(index) => {
                ThermalState::ALL.get(index as usize).map_or(Self::Os, |&state| Self::State(state))
            }
            None => Self::Os,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let index = STATE_CHOICES.iter().position(|name| name.eq_ignore_ascii_case(text))?;
        Some(Self::from_bits(index as u32))
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Choice { index: self.to_bits(), names: &STATE_CHOICES }
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Choice { index, .. } if (index as usize) < STATE_CHOICES.len() => {
                Some(Self::from_bits(index))
            }
            _ => None,
        }
    }
}

/// An on/off source the OS reports, or forced: `thermal.low_power`'s values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd)]
pub enum ForcedSwitch {
    /// What the OS says, absent where it has no such source.
    #[default]
    Os,
    Off,
    On,
}

const SWITCH_CHOICES: [&str; 3] = ["os", "off", "on"];

impl CvarType for ForcedSwitch {
    const KIND: CvarKind = CvarKind::Choice(&SWITCH_CHOICES);

    fn to_bits(self) -> u32 {
        self as u32
    }

    fn from_bits(bits: u32) -> Self {
        match bits {
            1 => Self::Off,
            2 => Self::On,
            _ => Self::Os,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let index = SWITCH_CHOICES.iter().position(|name| name.eq_ignore_ascii_case(text))?;
        Some(Self::from_bits(index as u32))
    }

    fn to_value(self) -> CvarValue {
        CvarValue::Choice { index: self as u32, names: &SWITCH_CHOICES }
    }

    fn from_value(value: CvarValue) -> Option<Self> {
        match value {
            CvarValue::Choice { index, .. } if (index as usize) < SWITCH_CHOICES.len() => {
                Some(Self::from_bits(index))
            }
            _ => None,
        }
    }
}

/// Forces the reading's thermal state.
pub static FORCE_STATE: Cvar<ForcedState> = Cvar::new(
    "thermal.state",
    ForcedState::Os,
    "The thermal state every reading gives (spec §6.3), for driving the quality ladder: os is what the OS says.",
);
/// Forces the reading's headroom.
pub static FORCE_HEADROOM: Cvar<f32> = Cvar::with_range(
    "thermal.headroom",
    -1.0,
    -1.0,
    4.0,
    "The thermal headroom every reading gives, where 1 is severe throttling: below 0 is what the OS says.",
);
/// Forces the reading's Low Power Mode.
pub static FORCE_LOW_POWER: Cvar<ForcedSwitch> = Cvar::new(
    "thermal.low_power",
    ForcedSwitch::Os,
    "Whether every reading has Low Power Mode on: os is what the OS says (iOS only).",
);

/// The cvars that force a thermal reading, for an app's registry.
pub static CVARS: [&dyn AnyCvar; 3] = [&FORCE_STATE, &FORCE_HEADROOM, &FORCE_LOW_POWER];

/// Android's `AThermalStatus` (`thermal.h`) on our scale; a status it doesn't define, or its error,
/// is `None`.
#[cfg(any(target_os = "android", test))]
const fn from_android_status(status: i32) -> Option<ThermalState> {
    Some(match status {
        0 => ThermalState::Nominal,
        1 => ThermalState::Light,
        2 => ThermalState::Moderate,
        3 => ThermalState::Severe,
        // CRITICAL, EMERGENCY and SHUTDOWN.
        4..=6 => ThermalState::Critical,
        // ATHERMAL_STATUS_ERROR, or past the header's statuses.
        _ => return None,
    })
}

#[cfg(target_os = "ios")]
mod imp {
    use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};

    use super::{Source, ThermalState};
    use crate::LOG_TARGET;

    #[derive(Debug, Default)]
    pub(crate) struct Os {
        /// A state out of range has been logged.
        warned: bool,
    }

    impl Os {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Source for Os {
        fn state(&mut self) -> ThermalState {
            match NSProcessInfo::processInfo().thermalState() {
                NSProcessInfoThermalState::Nominal => ThermalState::Nominal,
                NSProcessInfoThermalState::Fair => ThermalState::Light,
                NSProcessInfoThermalState::Serious => ThermalState::Severe,
                NSProcessInfoThermalState::Critical => ThermalState::Critical,
                other => {
                    if !std::mem::replace(&mut self.warned, true) {
                        tracing::warn!(target: LOG_TARGET, state = other.0, "iOS gave a thermal state out of range");
                    }
                    ThermalState::Unknown
                }
            }
        }

        fn has_headroom(&self) -> bool {
            false
        }

        fn headroom(&mut self, _forecast_seconds: i32) -> f32 {
            f32::NAN
        }

        fn low_power(&mut self) -> Option<bool> {
            Some(NSProcessInfo::processInfo().isLowPowerModeEnabled())
        }
    }
}

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::{CStr, c_int, c_void};
    use std::mem;
    use std::ptr::NonNull;
    use std::sync::OnceLock;

    use super::{Source, ThermalState, from_android_status};
    use crate::LOG_TARGET;

    type AcquireManager = unsafe extern "C" fn() -> *mut c_void;
    type CurrentStatus = unsafe extern "C" fn(*mut c_void) -> c_int;
    type Headroom = unsafe extern "C" fn(*mut c_void, c_int) -> f32;

    /// The thermal manager and its readers, newer than the app's minimum (API 30 and 31 against 29),
    /// so resolved at run time.
    struct Api {
        manager: NonNull<c_void>,
        current_status: CurrentStatus,
        /// API 31.
        headroom: Option<Headroom>,
    }

    // SAFETY: the manager is never released, and only the platform's thermal monitor, on the thread
    // that owns the platform, calls it.
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
            let headroom = symbol(c"AThermal_getThermalHeadroom");
            // SAFETY: a function symbol's address is the function, and the NDK declares these three
            // with exactly these signatures (`thermal.h`).
            let (acquire, current_status, headroom) = unsafe {
                (
                    mem::transmute::<*mut c_void, AcquireManager>(acquire.as_ptr()),
                    mem::transmute::<*mut c_void, CurrentStatus>(current_status.as_ptr()),
                    headroom.map(|f| mem::transmute::<*mut c_void, Headroom>(f.as_ptr())),
                )
            };
            // SAFETY: no arguments and no preconditions; it returns null on failure.
            let manager = NonNull::new(unsafe { acquire() })?;
            Some(Api { manager, current_status, headroom })
        })
        .as_ref()
    }

    #[derive(Debug, Default)]
    pub(crate) struct Os {
        /// A status out of range has been logged.
        warned: bool,
    }

    impl Os {
        pub fn new() -> Self {
            Self::default()
        }
    }

    impl Source for Os {
        fn state(&mut self) -> ThermalState {
            let Some(api) = api() else { return ThermalState::Unknown };
            // SAFETY: the manager came from `AThermal_acquireManager` and is never released.
            let status = unsafe { (api.current_status)(api.manager.as_ptr()) };
            from_android_status(status).unwrap_or_else(|| {
                // ATHERMAL_STATUS_ERROR is a failed read, said by the OS; anything else is out of range.
                if status != -1 && !mem::replace(&mut self.warned, true) {
                    tracing::warn!(target: LOG_TARGET, status, "Android gave a thermal status out of range");
                }
                ThermalState::Unknown
            })
        }

        fn has_headroom(&self) -> bool {
            api().is_some_and(|api| api.headroom.is_some())
        }

        fn headroom(&mut self, forecast_seconds: i32) -> f32 {
            let Some((api, headroom)) = api().and_then(|api| Some((api, api.headroom?))) else {
                return f32::NAN;
            };
            // SAFETY: as in `state`; any forecast is accepted (the OS clamps it).
            unsafe { headroom(api.manager.as_ptr(), forecast_seconds) }
        }

        fn low_power(&mut self) -> Option<bool> {
            None
        }
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod imp {
    use super::{Source, ThermalState};

    /// Desktops report nothing.
    #[derive(Debug)]
    pub(crate) struct Os;

    impl Os {
        pub const fn new() -> Self {
            Self
        }
    }

    impl Source for Os {
        fn state(&mut self) -> ThermalState {
            ThermalState::Unknown
        }

        fn has_headroom(&self) -> bool {
            false
        }

        fn headroom(&mut self, _forecast_seconds: i32) -> f32 {
            f32::NAN
        }

        fn low_power(&mut self) -> Option<bool> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// The cvars are process-wide, so the tests that read through them take turns.
    static CVAR_LOCK: Mutex<()> = Mutex::new(());

    const SECOND: u64 = 1_000_000_000;

    /// A scripted OS: the next values it gives, and how often it was asked.
    #[derive(Default)]
    struct Script {
        state: ThermalState,
        headroom: Option<Vec<f32>>,
        low_power: Option<bool>,
        state_reads: u32,
        headroom_reads: u32,
    }

    impl Source for Script {
        fn state(&mut self) -> ThermalState {
            self.state_reads += 1;
            self.state
        }

        fn has_headroom(&self) -> bool {
            self.headroom.is_some()
        }

        fn headroom(&mut self, forecast_seconds: i32) -> f32 {
            assert_eq!(forecast_seconds, HEADROOM_FORECAST_SECONDS);
            self.headroom_reads += 1;
            let values = self.headroom.as_mut().expect("asked only where the OS has it");
            if values.len() > 1 { values.remove(0) } else { values[0] }
        }

        fn low_power(&mut self) -> Option<bool> {
            self.low_power
        }
    }

    fn monitor(script: Script) -> Thermal<Script> {
        Thermal::with_source(script)
    }

    #[test]
    fn androids_statuses_map_onto_one_scale() {
        for (status, state) in [
            (-2, None),
            (-1, None),
            (0, Some(ThermalState::Nominal)),
            (1, Some(ThermalState::Light)),
            (2, Some(ThermalState::Moderate)),
            (3, Some(ThermalState::Severe)),
            (4, Some(ThermalState::Critical)),
            (5, Some(ThermalState::Critical)),
            (6, Some(ThermalState::Critical)),
            (7, None),
            (i32::MAX, None),
        ] {
            assert_eq!(from_android_status(status), state, "status {status}");
        }
    }

    #[test]
    fn the_os_is_asked_no_more_often_than_it_wants() {
        let _cvars = CVAR_LOCK.lock().unwrap();
        let mut thermal = monitor(Script { headroom: Some(vec![0.5]), ..Script::default() });
        // Ten seconds of frames at 60 fps, then one more.
        for frame in 0..=600 {
            thermal.read(frame * SECOND / 60, None);
        }
        assert_eq!(thermal.source.state_reads, 11, "once a second");
        assert_eq!(thermal.source.headroom_reads, 2, "once every ten seconds");
    }

    #[test]
    fn a_reading_carries_each_source_the_os_has() {
        let _cvars = CVAR_LOCK.lock().unwrap();
        let mut android = monitor(Script {
            state: ThermalState::Moderate,
            headroom: Some(vec![0.93]),
            ..Script::default()
        });
        assert_eq!(
            android.read(0, Some(true)),
            ThermalReading {
                state: ThermalState::Moderate,
                headroom: Some(0.93),
                hint_session: Some(true),
                low_power: None,
            }
        );
        let mut ios =
            monitor(Script { state: ThermalState::Light, low_power: Some(true), ..Script::default() });
        let reading = ios.read(0, None);
        assert_eq!(
            (reading.state, reading.headroom, reading.low_power),
            (ThermalState::Light, None, Some(true))
        );
        assert_eq!(ios.source.headroom_reads, 0, "never asked where the OS lacks it");
        assert_eq!(monitor(Script::default()).read(0, None), ThermalReading::default(), "a desktop's");
    }

    #[test]
    fn a_first_headroom_of_nan_means_the_device_has_none() {
        let _cvars = CVAR_LOCK.lock().unwrap();
        let mut thermal = monitor(Script { headroom: Some(vec![f32::NAN, 0.5]), ..Script::default() });
        assert_eq!(thermal.read(0, None).headroom, None);
        assert_eq!(thermal.read(60 * SECOND, None).headroom, None);
        assert_eq!(thermal.source.headroom_reads, 1, "not asked again");
    }

    #[test]
    fn headroom_out_of_range_reads_as_none_until_a_good_one() {
        let _cvars = CVAR_LOCK.lock().unwrap();
        let mut thermal = monitor(Script {
            headroom: Some(vec![0.0, 0.4, -0.5, f32::INFINITY, f32::NAN, 1.2]),
            ..Script::default()
        });
        let readings: Vec<_> = (0..6).map(|poll| thermal.read(poll * 10 * SECOND, None).headroom).collect();
        // 0 is what devices without the forecast give; a later NaN is a read too soon.
        assert_eq!(readings, [None, Some(0.4), None, None, None, Some(1.2)]);
    }

    #[test]
    fn the_cvars_force_any_reading() {
        let _cvars = CVAR_LOCK.lock().unwrap();
        let mut thermal = monitor(Script {
            state: ThermalState::Nominal,
            headroom: Some(vec![0.3]),
            low_power: Some(false),
            ..Script::default()
        });
        let os = thermal.read(0, Some(false));

        let mut registry = sapphire_core::cvar::CvarRegistry::new();
        registry.register_all(&CVARS).unwrap();
        registry.set("thermal.state", "severe").unwrap();
        registry.set("thermal.headroom", "1.05").unwrap();
        registry.set("thermal.low_power", "on").unwrap();
        assert!(registry.set("thermal.headroom", "5").is_err(), "out of range");
        let forced = thermal.read(SECOND / 60, Some(false));
        for cvar in CVARS {
            cvar.reset();
        }
        assert_eq!(
            forced,
            ThermalReading {
                state: ThermalState::Severe,
                headroom: Some(1.05),
                hint_session: Some(false),
                low_power: Some(true),
            }
        );
        assert_eq!(thermal.read(SECOND / 30, Some(false)), os, "back to the OS's once reset");
    }

    #[test]
    fn forced_states_round_trip_through_their_names() {
        for (index, name) in STATE_CHOICES.iter().enumerate() {
            let state = ForcedState::parse(name).unwrap();
            assert_eq!(state.to_bits(), index as u32);
            assert_eq!(state.to_value().to_string(), *name);
        }
        assert_eq!(ForcedState::parse("SEVERE"), Some(ForcedState::State(ThermalState::Severe)));
        assert_eq!(ForcedState::parse("hot"), None);
        assert_eq!(ForcedSwitch::parse("on"), Some(ForcedSwitch::On));
    }

    /// What this machine's OS says, once, and what asking for the state costs: run by hand on a
    /// device, a simulator or an emulator, where the OS has readings (`--ignored --nocapture`).
    #[test]
    #[ignore = "prints what this OS says; run on a device, simulator or emulator"]
    #[allow(clippy::print_stdout, reason = "printing what the OS says is this test's whole job")]
    fn reads_this_os() {
        let mut thermal = Thermal::new();
        let reading = thermal.read(0, None);
        println!("thermal reading: {reading} ({reading:?})");
        let mut os = imp::Os::new();
        let reads = 100;
        #[expect(clippy::disallowed_methods, reason = "timing the OS's call, in a test")]
        let start = std::time::Instant::now();
        for _ in 0..reads {
            std::hint::black_box(os.state());
            std::hint::black_box(os.low_power());
        }
        println!("asking for the state and Low Power Mode: {:?} each", start.elapsed() / reads);
    }
}
