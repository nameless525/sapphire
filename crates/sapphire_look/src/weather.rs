//! The weather as the frame draws it.
//!
//! - **[`Weather`]** (M5 L4): how wet the world's surfaces are and how far its puddles have filled,
//!   resolved outside the renderer from a zone's weather, its day's keyed weather and a tool's
//!   override, in that order (spec §5.21), and drawn by the environment's wet surface response
//!   (spec §5.12).
//! - **[`WeatherFrame`]** (M6 W8a): the one weather state of a frame (m6-plan §3, "One wind and one
//!   weather state"), the weather-only half of at most two live states blended: falling
//!   precipitation, the wind's strength over the zone's, a lightning rate, the shares surfaces
//!   accumulate toward and how fast, the temperature, the sea's state, the audio's parameters and
//!   the environment's keyed groups. The world's `WeatherSystem` resolves it from a
//!   `WeatherDocument`'s states; their look half is a partial day, which time of day blends like any
//!   other. Its readers land one by one (the wind now; precipitation, accumulation, lightning, the
//!   sea, audio and the environment's groups later), and nothing reaches the GPU's uniforms
//!   through it. None of it enters the simulation.

/// The weather's wetness inputs: what the environment's wet surfaces read (M5 L4). Both are
/// shares, 0 to 1, and the default is a dry world, which draws exactly as the world drew before
/// weather.
///
/// - **Wetness** darkens a porous surface (paving, plaster, wood) and lays a film of water over
///   every surface, lowering its roughness toward a wet film's, so it picks up its reflection and
///   the lights' highlights.
/// - **Puddles** fill the flat, upward-facing places a material lets water stand in, which go
///   flat and mirror-like.
///
/// Where these come from is the world's to decide (the stage resolves them); the renderer reads
/// only the values (rule R4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weather {
    /// How wet surfaces are, 0 (dry) to 1 (soaked).
    pub wetness: f32,
    /// How far puddles have filled, 0 (none) to 1 (every place a material lets water stand).
    pub puddles: f32,
}

impl Weather {
    /// A dry world: what draws as the world drew before weather.
    pub const DRY: Self = Self { wetness: 0.0, puddles: 0.0 };

    /// Whether it draws anything: either share above 0.
    #[must_use]
    pub fn is_wet(&self) -> bool {
        self.wetness > 0.0 || self.puddles > 0.0
    }

    /// The same weather with each share held to 0 to 1, a NaN counting as dry: what the frame
    /// carries to the GPU.
    #[must_use]
    pub fn clamped(self) -> Self {
        let share = |value: f32| if value > 0.0 { value.min(1.0) } else { 0.0 };
        Self { wetness: share(self.wetness), puddles: share(self.puddles) }
    }
}

/// What falls (M6 W8a): rain, snow, hail or dust.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PrecipitationKind {
    #[default]
    Rain,
    Snow,
    Hail,
    Dust,
}

/// One kind of falling weather (M6 W8a): how much falls and how, relative to the kind's authored
/// look. Drawn by a later package (W6); data until then.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Precipitation {
    pub kind: PrecipitationKind,
    /// 0 (none) to 1 (the kind's heaviest).
    pub density: f32,
    /// Times the kind's authored size.
    pub size: f32,
    /// Times the kind's authored fall speed.
    pub speed: f32,
}

/// The wind a weather state blows (M6 W8a), over the zone's: a multiple of its strength, the gusts and
/// turbulence in place of its.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherWind {
    /// Times the zone's strength.
    pub strength: f32,
    /// 0 (steady) to 1, as [`crate::Wind::gustiness`].
    pub gustiness: f32,
    /// 0 to 1, as [`crate::WindField::turbulence`].
    pub turbulence: f32,
}

impl Default for WeatherWind {
    /// The zone's wind as it is: its strength once over, steady, with no turbulence.
    fn default() -> Self {
        Self { strength: 1.0, gustiness: 0.0, turbulence: 0.0 }
    }
}

/// Where one accumulated share goes under a state (M6 W8a): its target, 0 to 1, and the seconds of
/// world time it takes to rise about 63% of the way there (a time constant). Y2 integrates it; data
/// until then.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShareTarget {
    pub target: f32,
    pub rise: f32,
}

/// What surfaces accumulate toward under a state (M6 W8a): wetness, puddles, snow, sand and ice. A
/// share a state leaves out goes to 0 at its drying rate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Accumulation {
    pub wetness: ShareTarget,
    pub puddles: ShareTarget,
    pub snow: ShareTarget,
    pub sand: ShareTarget,
    pub ice: ShareTarget,
}

/// How cold a state is (M6 W8a): what melts and what freezes, and breath in the cold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Temperature {
    Warm,
    #[default]
    Mild,
    Cold,
    Freezing,
}

/// The audio's weather parameters (M6 W8a), each 0 to 1: what the zone's ambience (U7) fades its
/// beds by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherAudio {
    pub precipitation: f32,
    pub wind: f32,
    pub thunder: f32,
}

/// The environment's keyed groups (M6 W8a), for readers still to land (V's cover, rain rings, the
/// aurora and the fog volumes' density).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherEnvironment {
    /// Frost's share of the cover, 0 to 1.
    pub frost: f32,
    /// Rain rings on water and puddles, 0 to 1.
    pub rings: f32,
    /// The aurora's strength, 0 to 1.
    pub aurora: f32,
    /// Times the fog volumes' density.
    pub fog_density: f32,
}

impl Default for WeatherEnvironment {
    /// Nothing: no frost, rings or aurora, the fog volumes as authored.
    fn default() -> Self {
        Self { frost: 0.0, rings: 0.0, aurora: 0.0, fog_density: 1.0 }
    }
}

/// The weather-only half of a frame's weather state (M6 W8a, see the [module docs](self)): at most three
/// states blended by their weights. Each group blends as its kind does:
///
/// - a scalar group (the wind, the lightning rate, the sea, the audio, the environment) by the weights,
///   a state that leaves it out weighing in as its neutral value ([`WeatherWind::default`],
///   [`WeatherEnvironment::default`], 0 for the rest);
/// - precipitation as a layer for each live state's kind, each's density scaled by
///   its state's weight, so one kind thins as the other thickens; the same kind blends into one layer;
/// - accumulation and the temperature switch with the dominant state (the heavier, the incoming at an
///   even split), never blending, so the integrator moves the shares at their own rates.
///
/// The default is no weather state at all, which changes nothing anything draws.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherFrame {
    /// How many states are live, 0 to 3: the world's two and a cutscene's while it eases in or out.
    pub live: u8,
    /// The incoming state's weight, 0 to 1: 1 with one state live.
    pub transition: f32,
    /// What falls, at most three kinds.
    pub precipitation: [Option<Precipitation>; 3],
    /// The wind over the zone's, and the share of the weight states that set one hold: 0 leaves the
    /// zone's wind as it is.
    pub wind: WeatherWind,
    pub wind_share: f32,
    /// Strikes a minute, on average (W8b draws them).
    pub lightning: f32,
    /// What surfaces accumulate toward, the dominant state's.
    pub accumulation: Option<Accumulation>,
    /// The dominant state's.
    pub temperature: Option<Temperature>,
    /// The sea's state, 0 (calm) to 1 (a storm's).
    pub sea: f32,
    pub audio: WeatherAudio,
    pub environment: WeatherEnvironment,
}

impl WeatherFrame {
    /// Lays the states' wind over `field`, the zone's: its strength times theirs, its gusts and
    /// turbulence toward theirs, each by their share. With no state that sets a wind, `field` stays as
    /// it is, bit for bit.
    pub fn wind_over(&self, field: &mut crate::WindField) {
        let share = self.wind_share;
        if share <= 0.0 || share.is_nan() {
            return;
        }
        let share = share.min(1.0);
        let mix = |from: f32, to: f32| from + (to - from) * share;
        let wind = self.wind;
        field.global.strength *= mix(1.0, wind.strength.max(0.0));
        field.global.gustiness = mix(field.global.gustiness, wind.gustiness.clamp(0.0, 1.0));
        field.turbulence = mix(field.turbulence, wind.turbulence.clamp(0.0, 1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_state_leaves_the_zones_wind_and_a_state_lays_its_own_over_it() {
        let zone =
            crate::WindField::new(crate::Wind { direction: glam::Vec3::X, strength: 0.6, gustiness: 0.3 });
        let mut field = zone;
        WeatherFrame::default().wind_over(&mut field);
        assert_eq!(field, zone, "no weather state, no change");
        let storm = WeatherFrame {
            wind: WeatherWind { strength: 3.0, gustiness: 0.8, turbulence: 0.5 },
            wind_share: 1.0,
            ..WeatherFrame::default()
        };
        let mut field = zone;
        storm.wind_over(&mut field);
        assert!((field.global.strength - 1.8).abs() < 1e-6);
        assert_eq!((field.global.gustiness, field.turbulence), (0.8, 0.5));
        let mut half = zone;
        WeatherFrame { wind_share: 0.5, ..storm }.wind_over(&mut half);
        assert!((half.global.strength - 1.2).abs() < 1e-6, "halfway from 1× to 3×");
        assert!((half.global.gustiness - 0.55).abs() < 1e-6 && (half.turbulence - 0.25).abs() < 1e-6);
        assert_eq!(half.global.direction, zone.global.direction, "a state never turns the wind");
    }

    #[test]
    fn dry_is_the_default_and_draws_nothing() {
        assert_eq!(Weather::default(), Weather::DRY);
        assert!(!Weather::DRY.is_wet());
        assert!(Weather { wetness: 0.0, puddles: 0.1 }.is_wet());
        assert!(Weather { wetness: 0.1, puddles: 0.0 }.is_wet());
    }

    #[test]
    fn shares_are_held_to_their_range() {
        let wild = Weather { wetness: 1.5, puddles: -0.5 };
        assert_eq!(wild.clamped(), Weather { wetness: 1.0, puddles: 0.0 });
        assert_eq!(
            Weather { wetness: f32::NAN, puddles: 0.25 }.clamped(),
            Weather { wetness: 0.0, puddles: 0.25 }
        );
    }
}
