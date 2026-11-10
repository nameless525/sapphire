//! How hot the OS says the device is running, as plain data (spec §6.3): one [`ThermalReading`] on one
//! scale for every OS. `sapphire_platform` makes the readings and re-exports these types; they live here
//! so that what answers heat (the ladder in `sapphire_render::quality`) can take a reading without
//! linking the platform layer and SDL, as [`FormFactor`](crate::target::FormFactor) does.
//!
//! A reading is presentation: nothing in the simulation reads it, and it never reaches a checksum.

use std::fmt;

/// How far into its thermal limits the OS says the device is, on Android's scale (`AThermalStatus`),
/// which iOS's four states map into. The OS decides, by its own rules for that device, so a state means
/// the same thing to a game on one phone and says nothing across phones about temperature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThermalState {
    /// The OS doesn't say: a desktop, an Android before 11 (API 30), or a read that failed or was out of
    /// range.
    #[default]
    Unknown,
    /// Android's `NONE`, iOS's `nominal`: not throttled.
    Nominal,
    /// Android's `LIGHT`, iOS's `fair`: warm, with throttling the player doesn't notice.
    Light,
    /// Android's `MODERATE`: throttled, with little effect on the player. iOS has no such state.
    Moderate,
    /// Android's `SEVERE`, iOS's `serious`: the OS is reducing performance.
    Severe,
    /// Android's `CRITICAL` and the two past it (`EMERGENCY`, `SHUTDOWN`), iOS's `critical`: the OS is
    /// doing all it can to cool down.
    Critical,
}

impl ThermalState {
    /// Every state, coolest first.
    pub const ALL: [Self; 6] =
        [Self::Unknown, Self::Nominal, Self::Light, Self::Moderate, Self::Severe, Self::Critical];
}

/// What the OS says about heat and power at one moment, on one scale for every OS. A source the OS
/// lacks is absent (`None`), never a guess: desktops have none of them, iOS no headroom or hint
/// session, and Android no Low Power Mode.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ThermalReading {
    /// The OS's thermal state (Android's `AThermal_getCurrentThermalStatus`, iOS's `thermalState`).
    pub state: ThermalState,
    /// Android's headroom forecast (`AThermal_getThermalHeadroom`, Android 12, API 31): how much of the
    /// thermal envelope the device is forecast to use a few seconds from now, where 1.0 is the point
    /// at which the OS throttles at [`ThermalState::Severe`]. At least 0 and finite; past 1.0 there is
    /// no mapping to a state. Absent where the OS lacks it (an emulator reads NaN, which is absent),
    /// and between a failed read and the next good one.
    pub headroom: Option<f32>,
    /// Whether an ADPF performance hint session is live for the frame thread (Android 13, API 33):
    /// `Some(false)` where the OS has the API but the session failed or hasn't started yet. Absent
    /// where the OS has no such API.
    pub hint_session: Option<bool>,
    /// Whether the player has iOS's Low Power Mode on (`isLowPowerModeEnabled`). Absent elsewhere.
    pub low_power: Option<bool>,
}

impl fmt::Display for ThermalReading {
    /// One short line for stats: `Severe, headroom 0.97, hint session live`, naming only the sources
    /// the OS has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.state)?;
        if let Some(headroom) = self.headroom {
            write!(f, ", headroom {headroom:.2}")?;
        }
        match self.hint_session {
            Some(true) => f.write_str(", hint session live")?,
            Some(false) => f.write_str(", no hint session")?,
            None => {}
        }
        match self.low_power {
            Some(true) => f.write_str(", low power on")?,
            Some(false) => f.write_str(", low power off")?,
            None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotter_states_order_above_cooler_ones() {
        assert!(ThermalState::ALL.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_reading_names_only_the_sources_the_os_has() {
        assert_eq!(ThermalReading::default().to_string(), "Unknown");
        let android = ThermalReading {
            state: ThermalState::Severe,
            headroom: Some(0.974),
            hint_session: Some(true),
            low_power: None,
        };
        assert_eq!(android.to_string(), "Severe, headroom 0.97, hint session live");
        let ios =
            ThermalReading { state: ThermalState::Light, low_power: Some(true), ..ThermalReading::default() };
        assert_eq!(ios.to_string(), "Light, low power on");
    }
}
