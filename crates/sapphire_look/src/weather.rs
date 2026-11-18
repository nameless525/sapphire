//! The weather as the frame draws it.
//!
//! - **[`Weather`]** (M5 L4, grown by M6 Y2): the shares of the world's surfaces the weather covers:
//!   how wet they are, how far puddles have filled, and how much snow, sand and ice lie on them. The
//!   world's weather system accumulates them over world time toward its state's targets
//!   ([`Weather::settle`]). With no weather plan a zone's fixed weather, its day's keyed weather and
//!   a tool's lie over them, in that order; with one, they lie over the day's and the zone's, and
//!   volumes', a cutscene's and a tool's over them (m6-plan §3, spec §5.21). The environment's wet surface response draws wetness and
//!   puddles (spec §5.12); snow and sand ride the frame's uniforms for the cover's readers to come,
//!   and ice the packet alone.
//! - **[`WeatherFrame`]** (M6 W8a): the one weather state of a frame (m6-plan §3, "One wind and one
//!   weather state"), the weather-only half of at most two live states blended: falling
//!   precipitation, the wind's strength over the zone's, a lightning rate, the shares surfaces
//!   accumulate toward and how fast, the temperature, the sea's state, the audio's parameters and
//!   the environment's keyed groups. The world's `WeatherSystem` resolves it from a
//!   `WeatherDocument`'s states; their look half is a partial day, which time of day blends like any
//!   other. Its readers land one by one (the wind and the accumulation now; precipitation,
//!   lightning, the sea, audio and the environment's groups later). None of it enters the
//!   simulation.
//! - **[`Rainbow`]** (M6 Y2): whether the light and the air allow a rainbow, for the sky to draw.
//!
//! # Accumulation (M6 Y2)
//!
//! Each share moves toward its target in closed form, `target + (share - target) e^(-t/τ)`, with `τ`
//! the state's rise while the share is below its target and its drying ([`DRYING`]) while above:
//! wetness dries over 180 s of world time, puddles over 900, snow melts over 1,200, sand clears over
//! 1,800 and ice melts over 900 (m6-plan §3, "The frame's uniforms"). The couplings act on targets
//! and rates ([`Accumulation::coupled`]), never on another share's value but for one line, so a
//! stretch of unchanging weather is one exact formula however it is stepped, which is what lets a
//! zone re-warm its shares in a few steps and land where a day played frame by frame would, where
//! the shares were near dry six game hours back:
//!
//! - **Sleet:** snow above freezing (`Warm`, `Mild`) falls as water: its target and rise wetness's,
//!   when larger.
//! - **Freezing:** at `Freezing` standing water is ice: the largest of the wetness, puddles and ice
//!   targets is ice's, and wetness and puddles go to 0.
//! - **Melting:** snow and ice melt only above freezing; at `Freezing` they hold where they lie.
//! - **Sand clearing:** sand clears over 1,800 s, or as fast as rain wets the ground while it falls.
//! - **Puddles under snow:** while snow lies over [`SNOW_COVERS`] the puddles' target is 0, covered
//!   or frozen. The one coupling that reads a share: [`Weather::settle`] splits at the moment snow
//!   crosses the line, which it finds in closed form.

use glam::Vec3;
use sapphire_math::ease::smoothstep;

/// The shares of the world's surfaces the weather covers (M5 L4, M6 Y2), each 0 to 1. The default
/// is a dry, bare world, which draws exactly as the world drew before weather.
///
/// - **Wetness** darkens a porous surface (paving, plaster, wood) and lays a film of water over
///   every surface, lowering its roughness toward a wet film's, so it picks up its reflection and
///   the lights' highlights.
/// - **Puddles** fill the flat, upward-facing places a material lets water stand in, which go
///   flat and mirror-like.
/// - **Snow**, **sand** and **ice** lie on what faces the sky (M6 Y2): accumulated now, drawn by
///   the environment's cover when it lands (V).
///
/// Where these come from is the world's to decide (the stage resolves them); the renderer reads
/// only the values (rule R4).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Weather {
    /// How wet surfaces are, 0 (dry) to 1 (soaked).
    pub wetness: f32,
    /// How far puddles have filled, 0 (none) to 1 (every place a material lets water stand).
    pub puddles: f32,
    /// How much snow lies, 0 (none) to 1 (a full cover).
    pub snow: f32,
    /// How much sand and dust lie, 0 to 1.
    pub sand: f32,
    /// How much ice glazes surfaces, 0 to 1.
    pub ice: f32,
}

impl Weather {
    /// A dry, bare world: what draws as the world drew before weather.
    pub const DRY: Self = Self { wetness: 0.0, puddles: 0.0, snow: 0.0, sand: 0.0, ice: 0.0 };

    /// Whether the wet surfaces draw anything: wetness or puddles above 0.
    #[must_use]
    pub fn is_wet(&self) -> bool {
        self.wetness > 0.0 || self.puddles > 0.0
    }

    /// The same weather with each share held to 0 to 1, a NaN counting as none: what the frame
    /// carries to the GPU.
    #[must_use]
    pub fn clamped(self) -> Self {
        let share = |value: f32| if value > 0.0 { value.min(1.0) } else { 0.0 };
        Self {
            wetness: share(self.wetness),
            puddles: share(self.puddles),
            snow: share(self.snow),
            sand: share(self.sand),
            ice: share(self.ice),
        }
    }

    /// The shares `seconds` of world time on, under a state accumulating toward `toward` at
    /// `temperature` all that while (M6 Y2, the [module docs](self)): exact, so one call over a
    /// minute and sixty over its seconds agree but for rounding. Allocates nothing.
    #[must_use]
    pub fn settle(self, toward: Accumulation, temperature: Temperature, seconds: f64) -> Self {
        if seconds.is_nan() || seconds <= 0.0 {
            return self;
        }
        let toward = toward.coupled(temperature);
        let frozen = temperature == Temperature::Freezing;
        let covered = self.snow > SNOW_COVERS;
        let snow = Share::new(self.snow, toward.snow, DRYING.snow, frozen);
        match snow.crossing(SNOW_COVERS, seconds) {
            Some(at) => {
                let first = self.settle_on_one_side(toward, frozen, covered, at);
                // On the line exactly, whatever the rounding, then over it or under it.
                let first = Self { snow: SNOW_COVERS, ..first };
                first.settle_on_one_side(toward, frozen, !covered, seconds - at)
            }
            None => self.settle_on_one_side(toward, frozen, covered, seconds),
        }
    }

    /// [`Self::settle`] over a stretch in which snow keeps to one side of [`SNOW_COVERS`]: `covered`
    /// over it. `toward` is coupled already.
    fn settle_on_one_side(self, toward: Accumulation, frozen: bool, covered: bool, seconds: f64) -> Self {
        let puddles = if covered { ShareTarget::default() } else { toward.puddles };
        // Rain washes sand away as fast as it wets the ground.
        let clearing =
            if toward.wetness.target > 0.0 { DRYING.sand.min(toward.wetness.rise) } else { DRYING.sand };
        Self {
            wetness: Share::new(self.wetness, toward.wetness, DRYING.wetness, false).after(seconds),
            puddles: Share::new(self.puddles, puddles, DRYING.puddles, false).after(seconds),
            snow: Share::new(self.snow, toward.snow, DRYING.snow, frozen).after(seconds),
            sand: Share::new(self.sand, toward.sand, clearing, false).after(seconds),
            ice: Share::new(self.ice, toward.ice, DRYING.ice, frozen).after(seconds),
        }
    }
}

/// How fast each share falls toward a lower target (M6 Y2; m6-plan §3, "The frame's uniforms"), in
/// seconds of world time to fall about 63% of the way: wet surfaces dry first and puddles later, the
/// order rain on real streets keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drying {
    pub wetness: f32,
    pub puddles: f32,
    /// Melting, above freezing.
    pub snow: f32,
    /// Blown clear, when no rain washes it.
    pub sand: f32,
    /// Melting, above freezing.
    pub ice: f32,
}

/// The shares' drying ([`Drying`]): 180, 900, 1,200, 1,800 and 900 s.
pub const DRYING: Drying =
    Drying { wetness: 180.0, puddles: 900.0, snow: 1_200.0, sand: 1_800.0, ice: 900.0 };

/// The snow share over which puddles fill no further and drain (M6 Y2): the snow covers them, or the
/// cold that brought it freezes them.
pub const SNOW_COVERS: f32 = 0.5;

/// How near its target a share arrives at it (M6 Y2): a tenth of the scripted day's tolerance, so
/// that a share dries to exactly 0, and [`Weather::is_wet`] turns false, within about 12 of its
/// drying times (`ln(1e5)` is 11.5) instead of at `f32`'s underflow, some 100 of them.
const ARRIVES: f64 = 1e-5;

/// One share moving toward a target through a stretch of unchanging weather, in `f64` so that
/// rounding never adds up over a day.
#[derive(Clone, Copy, Debug)]
struct Share {
    from: f64,
    target: f64,
    /// Seconds to move 63% of the way: infinite holds it.
    tau: f64,
}

impl Share {
    /// `from` moving toward `toward`'s target: by its rise below it, by `drying` above it, or held
    /// above it while `frozen`. A rise that isn't above 0 reaches the target at once.
    fn new(from: f32, toward: ShareTarget, drying: f32, frozen: bool) -> Self {
        let (from, target) = (f64::from(from), f64::from(toward.target));
        let tau = if target > from {
            f64::from(toward.rise)
        } else if frozen {
            f64::INFINITY
        } else {
            f64::from(drying)
        };
        Self { from, target, tau }
    }

    /// Where it is `seconds` on: at the target once within [`ARRIVES`] of it, so that a share dries
    /// to exactly 0 in a bounded time rather than at `f32`'s underflow.
    fn at(&self, seconds: f64) -> f64 {
        let at = if self.from == self.target || self.tau == f64::INFINITY {
            self.from
        } else if self.tau > 0.0 {
            self.target + (self.from - self.target) * (-seconds / self.tau).exp()
        } else {
            self.target
        };
        if self.tau.is_finite() && (at - self.target).abs() < ARRIVES { self.target } else { at }
    }

    fn after(&self, seconds: f64) -> f32 {
        self.at(seconds) as f32
    }

    /// When, within `seconds`, it passes from one side of `line` to the other, if it does: it moves
    /// one way only, so once at most. Starting on the line counts as below it.
    fn crossing(&self, line: f32, seconds: f64) -> Option<f64> {
        let line = f64::from(line);
        if (self.from > line) == (self.at(seconds) > line) {
            return None;
        }
        if !(self.tau > 0.0 && self.tau.is_finite()) {
            return Some(0.0);
        }
        // The share passes the line on its way to the target, so both differences share a sign.
        let at = self.tau * ((self.from - self.target) / (line - self.target)).ln();
        Some(at.clamp(0.0, seconds))
    }
}

/// Whether the light and the air allow a rainbow (M6 Y2), for the sky to draw (V11): its strength,
/// 0 to 1, and the point its arcs circle, opposite the sun, the primary bow at 42° from it and the
/// secondary at 51°.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rainbow {
    pub strength: f32,
    /// The antisolar point: a unit direction away from the sun.
    pub antisolar: Vec3,
}

/// Where the rainbow's condition fades in and out ([`Rainbow::condition`]): each pair the values
/// between which its term goes from nothing to whole, or the other way.
mod bow {
    /// Wetness: rain fell recently enough to leave drops in the air, from 0.2, whole by 0.6.
    pub const WET: (f32, f32) = (0.2, 0.6);
    /// Rain falling now: a shower's thin tail lets the sun through, a downpour doesn't (whole up to
    /// 0.3, gone by 0.6).
    pub const RAIN: (f32, f32) = (0.3, 0.6);
    /// The sun's elevation in degrees: risen (whole by 3°), and low enough that the primary bow, 42°
    /// from the antisolar point, stands above the horizon (fading from 36°, gone at 42°).
    pub const RISEN: (f32, f32) = (0.0, 3.0);
    pub const LOW: (f32, f32) = (36.0, 42.0);
    /// The clouds' cover: the sun must break through (whole up to 0.6, gone by 0.85).
    pub const COVER: (f32, f32) = (0.6, 0.85);
}

impl Rainbow {
    /// The rainbow's condition (M6 Y2), a pure function of the moment: surfaces still wet from
    /// recent rain (`weather`'s wetness, which dries over minutes, so the bow shows as a shower ends
    /// and fades as the street dries), little `rain` falling (its density, 0 to 1), the sun up but
    /// low (`toward_sun`, a direction toward it) and the clouds' `coverage` open enough to let it
    /// through. It eases nothing over time: each input moves smoothly already.
    #[must_use]
    pub fn condition(weather: Weather, rain: f32, toward_sun: Vec3, coverage: f32) -> Self {
        let Some(toward_sun) = toward_sun.try_normalize() else { return Self::default() };
        let elevation = toward_sun.y.clamp(-1.0, 1.0).asin().to_degrees();
        let wet = smoothstep(bow::WET.0, bow::WET.1, weather.wetness);
        let clear_air = 1.0 - smoothstep(bow::RAIN.0, bow::RAIN.1, rain);
        let sun = smoothstep(bow::RISEN.0, bow::RISEN.1, elevation)
            * (1.0 - smoothstep(bow::LOW.0, bow::LOW.1, elevation));
        let open = 1.0 - smoothstep(bow::COVER.0, bow::COVER.1, coverage);
        let strength = wet * clear_air * sun * open;
        if strength > 0.0 {
            Self { strength: strength.min(1.0), antisolar: -toward_sun }
        } else {
            Self::default()
        }
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

impl Accumulation {
    /// The targets as `temperature` leaves them (M6 Y2, the [module docs](self)): above freezing snow
    /// falls as water (sleet), and at freezing standing water is ice.
    #[must_use]
    pub fn coupled(self, temperature: Temperature) -> Self {
        let larger =
            |kept: ShareTarget, other: ShareTarget| if other.target > kept.target { other } else { kept };
        match temperature {
            Temperature::Warm | Temperature::Mild => {
                Self { wetness: larger(self.wetness, self.snow), snow: ShareTarget::default(), ..self }
            }
            Temperature::Cold => self,
            Temperature::Freezing => Self {
                ice: larger(larger(self.ice, self.wetness), self.puddles),
                wetness: ShareTarget::default(),
                puddles: ShareTarget::default(),
                ..self
            },
        }
    }

    /// Whether surfaces may go wet under it at `temperature`: wetness or puddles above 0 once coupled.
    /// What a scene declares the wet surfaces' pipelines by.
    #[must_use]
    pub fn wets(self, temperature: Temperature) -> bool {
        let coupled = self.coupled(temperature);
        coupled.wetness.target > 0.0 || coupled.puddles.target > 0.0
    }
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
    /// The look's dominant state's accumulation group: the tool's or a cutscene's state while they lie
    /// over the world's. Not what the world's surfaces accumulate toward, which follows the world's own
    /// state alone (`WeatherSystem::shares` in `sapphire_world`).
    pub accumulation: Option<Accumulation>,
    /// The look's dominant state's temperature group, as [`Self::accumulation`].
    pub temperature: Option<Temperature>,
    /// The sea's state, 0 (calm) to 1 (a storm's).
    pub sea: f32,
    pub audio: WeatherAudio,
    pub environment: WeatherEnvironment,
}

impl WeatherFrame {
    /// How much rain falls, 0 to 1: its layers' densities (M6 Y2, the rainbow's condition).
    #[must_use]
    pub fn rain(&self) -> f32 {
        let rain = self.precipitation.iter().flatten().filter(|layer| layer.kind == PrecipitationKind::Rain);
        rain.map(|layer| layer.density).sum::<f32>().clamp(0.0, 1.0)
    }

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
        let zone = crate::WindField::new(crate::Wind { direction: Vec3::X, strength: 0.6, gustiness: 0.3 });
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
        assert!(Weather { puddles: 0.1, ..Weather::DRY }.is_wet());
        assert!(Weather { wetness: 0.1, ..Weather::DRY }.is_wet());
        assert!(
            !Weather { snow: 1.0, sand: 1.0, ice: 1.0, ..Weather::DRY }.is_wet(),
            "a cover isn't wet: the wet surfaces' pipelines draw water alone"
        );
    }

    #[test]
    fn shares_are_held_to_their_range() {
        let wild = Weather { wetness: 1.5, puddles: -0.5, snow: 2.0, sand: f32::NAN, ice: -1.0 };
        assert_eq!(wild.clamped(), Weather { wetness: 1.0, puddles: 0.0, snow: 1.0, sand: 0.0, ice: 0.0 });
        assert_eq!(
            Weather { wetness: f32::NAN, puddles: 0.25, ..Weather::DRY }.clamped(),
            Weather { puddles: 0.25, ..Weather::DRY }
        );
    }

    fn share(target: f32, rise: f32) -> ShareTarget {
        ShareTarget { target, rise }
    }

    /// A state that names every share, each target 0.6 and rise 100 s.
    const ALL: Accumulation = Accumulation {
        wetness: ShareTarget { target: 0.6, rise: 100.0 },
        puddles: ShareTarget { target: 0.6, rise: 100.0 },
        snow: ShareTarget { target: 0.6, rise: 100.0 },
        sand: ShareTarget { target: 0.6, rise: 100.0 },
        ice: ShareTarget { target: 0.6, rise: 100.0 },
    };

    fn close(got: f32, want: f64, what: &str) {
        assert!((f64::from(got) - want).abs() < 1e-6, "{what}: {got} against {want}");
    }

    /// Below its target a share rises by the state's time constant, 63% of the way in one of them,
    /// whatever the share.
    #[test]
    fn a_share_rises_by_its_states_time_constant() {
        let rain =
            Accumulation { wetness: share(1.0, 45.0), puddles: share(0.6, 400.0), ..Accumulation::default() };
        let after = Weather::DRY.settle(rain, Temperature::Mild, 45.0);
        let rose = 1.0 - (-1.0f64).exp();
        close(after.wetness, rose, "wetness, one time constant");
        close(after.puddles, 0.6 * (1.0 - (-45.0f64 / 400.0).exp()), "puddles, slower");
        let snow = Accumulation {
            snow: share(0.8, 900.0),
            sand: share(0.7, 600.0),
            ice: share(0.4, 600.0),
            ..Accumulation::default()
        };
        let after = Weather::DRY.settle(snow, Temperature::Cold, 600.0);
        close(after.snow, 0.8 * (1.0 - (-600.0f64 / 900.0).exp()), "snow");
        close(after.sand, 0.7 * rose, "sand");
        close(after.ice, 0.4 * rose, "ice");
    }

    /// Above its target each share falls by its own drying (m6-plan §3): 180 s for wetness, 900 for
    /// puddles, 1,200 for snow, 1,800 for sand and 900 for ice.
    #[test]
    fn each_share_dries_at_its_rate() {
        let soaked = Weather { wetness: 1.0, puddles: 1.0, snow: 0.4, sand: 1.0, ice: 1.0 };
        for (seconds, rate) in
            [(180.0, "wetness"), (900.0, "puddles, ice"), (1_200.0, "snow"), (1_800.0, "sand")]
        {
            let after = soaked.settle(Accumulation::default(), Temperature::Mild, seconds);
            let fell = |from: f64, drying: f32| from * (-seconds / f64::from(drying)).exp();
            close(after.wetness, fell(1.0, 180.0), rate);
            close(after.puddles, fell(1.0, 900.0), rate);
            close(after.snow, fell(0.4, 1_200.0), rate);
            close(after.sand, fell(1.0, 1_800.0), rate);
            close(after.ice, fell(1.0, 900.0), rate);
        }
        assert_eq!(
            DRYING,
            Drying { wetness: 180.0, puddles: 900.0, snow: 1_200.0, sand: 1_800.0, ice: 900.0 }
        );
        // Wet surfaces dry before puddles do.
        let later = soaked.settle(Accumulation::default(), Temperature::Mild, 600.0);
        assert!(later.wetness < 0.05 && later.puddles > 0.5);
    }

    /// A share arrives: within 1e-5 of its target it is there, so a soaked world dries to exactly
    /// 0 in 12 of its slowest drying times, in one step or frame by frame, and stops drawing wet.
    #[test]
    fn a_share_dries_to_exactly_nothing_in_a_bounded_time() {
        let soaked = Weather { wetness: 1.0, puddles: 1.0, snow: 0.4, sand: 1.0, ice: 1.0 };
        let bound = 12.0 * f64::from(DRYING.sand);
        assert_eq!(soaked.settle(Accumulation::default(), Temperature::Mild, bound), Weather::DRY);
        // Wetness alone, by its own bound, and the frames keep it there.
        let wetness = soaked.settle(Accumulation::default(), Temperature::Mild, 12.0 * 180.0);
        assert_eq!(wetness.wetness, 0.0);
        let mut frames = Weather { wetness: 1.0, puddles: 1.0, ..Weather::DRY };
        for _ in 0..(12 * 900 * 20) {
            frames = frames.settle(Accumulation::default(), Temperature::Mild, 0.05);
        }
        assert!(!frames.is_wet(), "{frames:?}");
        // Just short of it, still a little wet: the arrival is near the target, not a cut.
        let short = soaked.settle(Accumulation::default(), Temperature::Mild, 10.0 * 180.0);
        assert!(short.wetness > 0.0 && short.wetness < 1e-4);
        // A rise arrives the same way.
        let rain = Accumulation { wetness: share(1.0, 30.0), ..Accumulation::default() };
        assert_eq!(Weather::DRY.settle(rain, Temperature::Mild, 12.0 * 30.0).wetness, 1.0);
    }

    /// The integrator is exact for unchanging weather: ten minutes in one step, in second-long ones
    /// and in uneven ones land in the same place, through rising, falling and snow crossing its line
    /// (the shares kept in `f32` between steps round a little each time, which a frame-by-frame caller
    /// avoids by settling from where the weather last changed).
    #[test]
    fn settling_is_exact_however_it_is_stepped() {
        let start = Weather { wetness: 0.9, puddles: 0.1, snow: 0.0, sand: 0.8, ice: 0.0 };
        let snow = Accumulation {
            wetness: share(0.2, 600.0),
            puddles: share(0.4, 300.0),
            snow: share(0.9, 300.0),
            ..ALL
        };
        for temperature in [Temperature::Warm, Temperature::Mild, Temperature::Cold, Temperature::Freezing] {
            let once = start.settle(snow, temperature, 600.0);
            let mut framed = start;
            for _ in 0..600 {
                framed = framed.settle(snow, temperature, 1.0);
            }
            let mut uneven = start;
            let mut left = 600.0;
            for step in (0..).map(|step: u32| f64::from(step % 7 + 1) * 0.37) {
                let step = step.min(left);
                uneven = uneven.settle(snow, temperature, step);
                left -= step;
                if left <= 0.0 {
                    break;
                }
            }
            for (got, what) in [(framed, "by seconds"), (uneven, "uneven")] {
                for (a, b) in [
                    (got.wetness, once.wetness),
                    (got.puddles, once.puddles),
                    (got.snow, once.snow),
                    (got.sand, once.sand),
                    (got.ice, once.ice),
                ] {
                    assert!((a - b).abs() < 1e-5, "{temperature:?}, {what}: {got:?} against {once:?}");
                }
            }
        }
    }

    /// Sleet: above freezing snow falls as water, the larger target wetness's; at and below cold it lies.
    #[test]
    fn snow_above_freezing_falls_as_water() {
        let snow =
            Accumulation { wetness: share(0.2, 600.0), snow: share(0.8, 120.0), ..Accumulation::default() };
        for temperature in [Temperature::Warm, Temperature::Mild] {
            let coupled = snow.coupled(temperature);
            assert_eq!((coupled.wetness, coupled.snow), (share(0.8, 120.0), ShareTarget::default()));
            let after = Weather::DRY.settle(snow, temperature, 1_200.0);
            assert_eq!(after.snow, 0.0, "{temperature:?}: no snow lies");
            assert!(after.wetness > 0.79, "{temperature:?}: it wets as the snow would have lain");
        }
        let cold = Weather::DRY.settle(snow, Temperature::Cold, 1_200.0);
        assert!(cold.snow > 0.79 && (cold.wetness - 0.2 * (1.0 - (-2.0f32).exp())).abs() < 1e-5);
        assert!(
            snow.wets(Temperature::Mild)
                && Accumulation { snow: share(0.8, 120.0), ..Accumulation::default() }
                    .wets(Temperature::Mild)
        );
        assert!(!Accumulation { snow: share(0.8, 120.0), ..Accumulation::default() }.wets(Temperature::Cold));
    }

    /// Freezing: standing water is ice, the largest of the three targets ice's; the wet street left by
    /// rain dries as the ice comes.
    #[test]
    fn water_freezes_into_ice() {
        let freezing_rain = Accumulation {
            wetness: share(0.9, 60.0),
            puddles: share(0.5, 300.0),
            ice: share(0.3, 120.0),
            ..Accumulation::default()
        };
        let coupled = freezing_rain.coupled(Temperature::Freezing);
        assert_eq!(coupled.ice, share(0.9, 60.0));
        assert_eq!((coupled.wetness, coupled.puddles), (ShareTarget::default(), ShareTarget::default()));
        assert!(!freezing_rain.wets(Temperature::Freezing), "nothing at freezing goes wet");
        let after_rain = Weather { wetness: 1.0, puddles: 0.6, ..Weather::DRY };
        let after = after_rain.settle(freezing_rain, Temperature::Freezing, 600.0);
        assert!(after.ice > 0.89, "glazed: {after:?}");
        close(after.wetness, (-600.0f64 / 180.0).exp(), "the film dries as it freezes");
        assert_eq!(freezing_rain.coupled(Temperature::Cold), freezing_rain, "cold leaves water as it is");
    }

    /// Melting: snow and ice melt above freezing at their rates, and hold where they lie at freezing,
    /// even under a state that names none.
    #[test]
    fn snow_and_ice_melt_only_above_freezing() {
        let white = Weather { snow: 1.0, ice: 0.5, ..Weather::DRY };
        let held = white.settle(Accumulation::default(), Temperature::Freezing, 3_600.0);
        assert_eq!((held.snow, held.ice), (1.0, 0.5), "a freezing hour melts nothing");
        for temperature in [Temperature::Cold, Temperature::Mild, Temperature::Warm] {
            let melted = white.settle(Accumulation::default(), temperature, 1_200.0);
            close(melted.snow, (-1.0f64).exp(), "snow over 1,200 s");
            close(melted.ice, 0.5 * (-1_200.0f64 / 900.0).exp(), "ice over 900 s");
        }
        // Freezing still lets them rise toward a state's targets.
        let blizzard = Accumulation { snow: share(1.0, 300.0), ..Accumulation::default() };
        assert!(Weather::DRY.settle(blizzard, Temperature::Freezing, 900.0).snow > 0.95);
    }

    /// Sand clears over 1,800 s on its own and as fast as rain wets the ground while it falls.
    #[test]
    fn rain_washes_sand_away() {
        let dusty = Weather { sand: 0.8, ..Weather::DRY };
        let still = dusty.settle(Accumulation::default(), Temperature::Warm, 600.0);
        close(still.sand, 0.8 * (-600.0f64 / 1_800.0).exp(), "blown clear");
        let rain = Accumulation { wetness: share(1.0, 45.0), ..Accumulation::default() };
        let washed = dusty.settle(rain, Temperature::Mild, 120.0);
        close(washed.sand, 0.8 * (-120.0f64 / 45.0).exp(), "washed at the rain's pace");
        let slow_mist = Accumulation { wetness: share(0.3, 3_600.0), ..Accumulation::default() };
        let misted = dusty.settle(slow_mist, Temperature::Mild, 600.0);
        assert_eq!(misted.sand, still.sand, "never slower than the wind clears it");
    }

    /// Puddles under snow: past half a cover of snow the puddles drain, and fill again once it has
    /// melted below; the split falls exactly where the snow crosses.
    #[test]
    fn snow_covers_the_puddles() {
        let wet_snow =
            Accumulation { puddles: share(0.8, 120.0), snow: share(1.0, 600.0), ..Accumulation::default() };
        let flooded = Weather { puddles: 0.8, ..Weather::DRY };
        // Snow reaches 0.5 at 600 ln 2 s; until then the puddles stand.
        let crossing = 600.0 * 2.0f64.ln();
        let before = flooded.settle(wet_snow, Temperature::Cold, crossing - 1.0);
        assert!((before.puddles - 0.8).abs() < 1e-6 && before.snow < SNOW_COVERS);
        let later = flooded.settle(wet_snow, Temperature::Cold, crossing + 900.0);
        close(later.puddles, 0.8 * (-900.0f64 / 900.0).exp(), "draining from the crossing on");
        // And back: snow melting under a mild rain fills them again once it is under the line.
        let rain = Accumulation { puddles: share(0.8, 120.0), ..Accumulation::default() };
        let covered = Weather { snow: 1.0, ..Weather::DRY };
        let melt = 1_200.0 * 2.0f64.ln();
        let still = covered.settle(rain, Temperature::Mild, melt - 1.0);
        assert_eq!(still.puddles, 0.0, "covered until the snow is under the line");
        let open = covered.settle(rain, Temperature::Mild, melt + 120.0);
        close(open.puddles, 0.8 * (1.0 - (-1.0f64).exp()), "filling from the crossing on");
    }

    #[test]
    fn the_rain_falling_is_its_layers_density() {
        let layer = |kind, density| Some(Precipitation { kind, density, size: 1.0, speed: 1.0 });
        let frame = WeatherFrame {
            precipitation: [
                layer(PrecipitationKind::Rain, 0.3),
                layer(PrecipitationKind::Snow, 0.9),
                layer(PrecipitationKind::Rain, 0.2),
            ],
            ..WeatherFrame::default()
        };
        assert!((frame.rain() - 0.5).abs() < 1e-6);
        assert_eq!(WeatherFrame::default().rain(), 0.0);
    }

    /// A rainbow wants a wet world, little rain, a low sun and a gap in the clouds, and circles the
    /// point opposite the sun.
    #[test]
    fn a_rainbow_needs_the_rains_end_and_a_low_sun() {
        let wet = Weather { wetness: 0.8, ..Weather::DRY };
        let low_sun = Vec3::new(0.0, 20.0f32.to_radians().sin(), -20.0f32.to_radians().cos());
        let bow = Rainbow::condition(wet, 0.1, low_sun, 0.4);
        assert!((bow.strength - 1.0).abs() < 1e-6, "{bow:?}");
        assert!((bow.antisolar + low_sun.normalize()).length() < 1e-6);
        let high_sun = Vec3::new(0.0, 60.0f32.to_radians().sin(), -60.0f32.to_radians().cos());
        for (what, bow) in [
            ("dry", Rainbow::condition(Weather::DRY, 0.0, low_sun, 0.4)),
            ("pouring", Rainbow::condition(wet, 0.9, low_sun, 0.4)),
            ("a high sun", Rainbow::condition(wet, 0.1, high_sun, 0.4)),
            ("night", Rainbow::condition(wet, 0.1, -low_sun, 0.4)),
            ("overcast", Rainbow::condition(wet, 0.1, low_sun, 0.95)),
            ("no sun", Rainbow::condition(wet, 0.1, Vec3::ZERO, 0.4)),
        ] {
            assert_eq!(bow, Rainbow::default(), "{what}");
        }
        let fading = Rainbow::condition(Weather { wetness: 0.4, ..Weather::DRY }, 0.1, low_sun, 0.4);
        assert!(fading.strength > 0.0 && fading.strength < 1.0, "fading as the street dries");
    }
}
