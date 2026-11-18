//! Weather (M6 W8a; m6-plan §3, "One wind and one weather state"): the [`WeatherSystem`], which chooses
//! the world's weather state each frame and blends a change over seconds of world time, into time of
//! day's look (the states' partial days, [`TimeOfDay::set_weather_blend`]) and a [`WeatherFrame`] for
//! what time of day has no word for (precipitation, wind over the zone's, lightning, the wet shares'
//! targets, temperature, the sea, audio and the environment's groups).
//!
//! The states are a `WeatherDocument` ([`sapphire_tod::weather`]); [`default_weather`] is the shipped
//! ten. A zone's plan ([`sapphire_zone::ZoneWeatherPlan`], cooked into its presentation) names a
//! document and says when and where each state shows, which [`WeatherSetup::read`] turns into a
//! [`WeatherPlan`].
//!
//! # Choosing
//!
//! Each layer over the one before, the first four into the world's state, which changes by a timed
//! transition:
//!
//! 1. **The schedule** ([`WeatherSchedule`]): a pure function of the day and the hour on time of day's
//!    clock, so the same clock always shows the same weather, at any frame rate and after any jump.
//! 2. **A region** ([`WeatherRegion`]): the highest-priority one the probe stands in (the camera, or the
//!    player), its state over the schedule's while the probe is inside; stepping in or out changes the
//!    state by a transition like any other.
//! 3. **An override** ([`WeatherSystem::set_override`]): a story flag's state, over both, changing in
//!    its own seconds if it gives some.
//! 4. **A cutscene's key** ([`crate::tod::TodCutscene::weather`]): the shot's state by the track's eased
//!    weight over the world's whole blend ([`WeatherSystem::shot`]), so the shot's weather comes and goes
//!    with the shot and the world's states fade under it, never cut: three states are live while a shot
//!    eases in or out over a world's transition.
//! 5. **The tool** ([`WeatherSystem::set_tool`]): a blend the overlay or a capture pins, in place of all
//!    of these.
//!
//! The world has at most two states ([`WeatherBlend`]), and no change jumps. One back to where the
//! transition came from turns it round from where it is; one to a third state first finishes the
//! transition on its heavier state, four times faster from where it is, then starts toward the third.
//! The override and the tool hold their states by name, found again in a new document.
//! Every rate is in world seconds, so pause and slow motion hold or slow a transition with the rest of
//! the world, and an accelerated clock changes weather faster.
//!
//! # Accumulation (M6 Y2)
//!
//! The system integrates the world's surfaces' shares ([`Weather`]: wetness, puddles, snow, sand and
//! ice) on world time toward the targets of the world's dominant state (the heavier of its two, the
//! incoming one from an even split; none drying everything), by [`Weather::settle`]'s closed form and
//! couplings: see `sapphire_look::weather`. The world's state alone: an override moves the shares as
//! the schedule does, but a cutscene's state and the tool's blend are looks laid over the world, so
//! the street stays as wet as the world made it ([`WeatherSystem::shares`]; a capture that wants other
//! shares sets them, `Scene::set_weather`). Every rate is in world seconds, so pause holds the shares,
//! slow motion slows them and an accelerated world clock hurries them.
//!
//! Between changes of the dominant state the shares are one formula from where the last change left
//! them, never stepped frame by frame, so rounding never adds up. A change lands on the first frame
//! that sees it, so a frame rate moves them by at most a frame of the faster rate. A clock set back
//! holds them where they were and moves them on from there.
//!
//! **Re-warming, never saving.** Nothing about the shares is kept: on its first resolve, which a zone's
//! arrival and a load both bring ([`WeatherSystem::set_plan`]), the system integrates the plan's last
//! six game hours ([`REWARM_HOURS`]) at the probe from dry, in [`REWARM_STEPS`] steps, each split
//! exactly where the dominant state changes (a slot's start, plus half its transition), so a street is
//! wet on arrival if it rained an hour ago, and the shares land where a day played through would have
//! left them if they were near dry six game hours back: a fast day (six game hours of little world
//! time) or a freezing hold (snow and ice from before) leaves a played day's elsewhere. With time of day
//! standing still, the weather has stood as it is, six hours of world time. An override isn't
//! replayed: a story flag that should hold across a load asks again. A jump of time of day neither
//! snaps nor re-warms the shares (owed to Y8).
//!
//! # Never the simulation
//!
//! Weather is presentation. The simulation never reads it, a replay never records it and no simulation
//! digest covers it: the schedule hashes the presentation's clock, a zone's plan lies in its
//! presentation, never its simulation, and the shares are re-warmed rather than saved.
//!
//! # Later
//!
//! - **V**'s packages read the shares as covers (snow, sand, ice), the environment's cover, rings,
//!   aurora and fog density, and the rainbow's condition ([`WeatherSystem::rainbow`]); **N** the
//!   precipitation and **U** the audio parameters. Until each lands, its groups are data only.
//! - **W8b** the editor's weather panel over these documents.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use bevy_ecs::prelude::Resource;
use sapphire_look::{Accumulation, Rainbow, Temperature, Weather, WeatherFrame};
use sapphire_math::{Ease, Vec3};
use sapphire_time::Stopwatch;
use sapphire_tod::DAY_HOURS;
use sapphire_tod::weather::{WeatherBlend, WeatherDocument, WeatherError, WeatherSchedule};
use sapphire_zone::CookedWeatherPlan;

use crate::tod::{TimeOfDay, VolumeShape};

/// The shipped weather states' document (see [`default_weather`]).
const PRESETS: &str = include_str!("../../../content/weather/presets.weather.ron");

/// Sapphire's ten weather states (`content/weather/presets.weather.ron`), compiled in: clear, overcast,
/// drizzle, rain, storm, snow, blizzard, hail, sandstorm and mist. A zone's plan may name this file or a
/// document of its own.
/// Parsed once, on first use, and shared: a stage costs a reference count, not a parse.
pub fn default_weather() -> Arc<WeatherDocument> {
    static SHIPPED: LazyLock<Arc<WeatherDocument>> = LazyLock::new(|| {
        Arc::new(
            WeatherDocument::from_ron(PRESETS)
                .expect("the shipped weather states are checked by this module's tests"),
        )
    });
    Arc::clone(&SHIPPED)
}

/// A place with weather of its own, in the world's space: inside `shape`, `state` over the schedule's.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherRegion {
    pub shape: VolumeShape,
    /// Which wins where regions overlap: the highest, then the first.
    pub priority: u8,
    /// The state, by its index in the document.
    pub state: usize,
}

/// When and where a zone's weather is which state: its schedule and regions, over one document's states.
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherPlan {
    schedule: WeatherSchedule,
    /// Highest priority first, the plan's order kept among equals.
    regions: Vec<WeatherRegion>,
}

impl WeatherPlan {
    /// A plan of `schedule` and `regions`.
    pub fn new(schedule: WeatherSchedule, mut regions: Vec<WeatherRegion>) -> Self {
        regions.sort_by_key(|region| std::cmp::Reverse(region.priority));
        Self { schedule, regions }
    }

    /// Every hour of every day `state`, with no regions.
    pub fn always(state: usize) -> Self {
        Self::new(WeatherSchedule::always(state), Vec::new())
    }

    pub fn schedule(&self) -> &WeatherSchedule {
        &self.schedule
    }

    /// Highest priority first.
    pub fn regions(&self) -> &[WeatherRegion] {
        &self.regions
    }

    /// The state the plan shows at `day`, `hours`, seen from `probe`, and the region that chose it, if one
    /// did. Allocates nothing.
    pub fn state_at(&self, day: i64, hours: f32, probe: Vec3) -> (usize, Option<usize>) {
        match self.regions.iter().position(|region| region.shape.distance(probe) <= 0.0) {
            Some(region) => (self.regions[region].state, Some(region)),
            None => (self.schedule.state_at(day, hours), None),
        }
    }

    /// Every state the plan may show, anywhere at any hour, once each or more.
    pub fn states(&self) -> impl Iterator<Item = usize> + '_ {
        self.schedule.choices().chain(self.regions.iter().map(|region| region.state))
    }

    /// Whether any state the plan may show wets surfaces (M6 Y2, [`Accumulation::wets`]): what a stage
    /// declares the wet surfaces' pipelines by.
    pub fn may_be_wet(&self, document: &WeatherDocument) -> bool {
        self.states().any(|state| wets(document, Some(state)))
    }
}

/// What the shares move toward under `state` of `document` (none: dry), and at what temperature.
fn accumulation(document: &WeatherDocument, state: Option<usize>) -> (Accumulation, Temperature) {
    match state.and_then(|state| document.states().get(state)) {
        Some(state) => (
            state.accumulation.map(Accumulation::from).unwrap_or_default(),
            state.temperature.map(Temperature::from).unwrap_or_default(),
        ),
        None => (Accumulation::default(), Temperature::default()),
    }
}

/// Whether `state` of `document` wets surfaces.
fn wets(document: &WeatherDocument, state: Option<usize>) -> bool {
    let (toward, temperature) = accumulation(document, state);
    toward.wets(temperature)
}

/// The game hours of weather a re-warm walks through (M6 Y2; m6-plan §3, "The frame's uniforms").
pub const REWARM_HOURS: f32 = 6.0;
/// The steps it walks them in, each split where the dominant state changes inside it.
pub const REWARM_STEPS: u32 = 120;

/// The shares the world's weather under `plan` of `document` leaves at `clock`, seen from `probe`, from
/// a dry world [`REWARM_HOURS`] earlier (see the [module docs](self)): exact wherever each change of state
/// finishes before the next begins. Settled under the state at the start, as a plan arrives; a change of
/// state flips the dominant state half its transition after its slot starts, as the world's blend does.
/// With time of day standing still, the state as it is for six hours of world time. Allocates nothing.
pub fn rewarm(document: &WeatherDocument, plan: &WeatherPlan, clock: WeatherClock, probe: Vec3) -> Weather {
    let mut flips = Flips::new(document, plan, clock, probe);
    let span = f64::from(REWARM_HOURS) * flips.hour_seconds;
    let settle = |shares: Weather, state: usize, seconds: f64| {
        let (toward, temperature) = accumulation(document, Some(state));
        shares.settle(toward, temperature, seconds)
    };
    let (mut shares, mut dominant, mut cursor) = (Weather::DRY, flips.wanted, 0.0);
    let mut flip = flips.next();
    for step in 1..=REWARM_STEPS {
        let end = span * f64::from(step) / f64::from(REWARM_STEPS);
        while let Some((at, state)) = flip.filter(|&(at, _)| at <= end) {
            shares = settle(shares, dominant, at - cursor);
            (cursor, dominant) = (at, state);
            flip = flips.next();
        }
        shares = settle(shares, dominant, end - cursor);
        cursor = end;
    }
    shares
}

/// The world's dominant state changing through a re-warm's window, as the world's own blend would
/// change it: the seconds of world time into the window each change comes, and the state it brings.
/// A change past the window's end is never reached.
struct Flips<'a> {
    document: &'a WeatherDocument,
    plan: &'a WeatherPlan,
    probe: Vec3,
    /// The window's start and end, and how far the walk has come, in game hours since day 0's midnight.
    start: f64,
    end: f64,
    cursor: f64,
    /// Seconds of world time a game hour takes.
    hour_seconds: f64,
    /// Whether time of day runs: without a cycle the state never changes.
    cycling: bool,
    /// The state the world wants after the walk so far.
    wanted: usize,
    /// The last change's seconds, which no later one comes before.
    last: f64,
}

impl<'a> Flips<'a> {
    fn new(document: &'a WeatherDocument, plan: &'a WeatherPlan, clock: WeatherClock, probe: Vec3) -> Self {
        let (hour_seconds, cycling) = match clock.day_length {
            Some(length) if length.is_finite() && length > 0.0 => {
                (f64::from(length) / f64::from(DAY_HOURS), true)
            }
            _ => (3_600.0, false),
        };
        let end = clock.day as f64 * f64::from(DAY_HOURS) + f64::from(clock.hours);
        let start = if cycling { end - f64::from(REWARM_HOURS) } else { end };
        let mut flips = Self {
            document,
            plan,
            probe,
            start,
            end,
            cursor: start,
            hour_seconds,
            cycling,
            wanted: 0,
            last: 0.0,
        };
        flips.wanted = flips.state(start, flips.next_change(start).min(end));
        flips
    }

    /// `game` hours as time of day's day and hour.
    fn clock(game: f64) -> (i64, f32) {
        let day = (game / f64::from(DAY_HOURS)).floor();
        let hours = (game - day * f64::from(DAY_HOURS)) as f32;
        (day as i64, if hours >= DAY_HOURS { 0.0 } else { hours })
    }

    /// The state the plan shows between `from` and `to`, which lie in one slot.
    fn state(&self, from: f64, to: f64) -> usize {
        let (day, hours) = Self::clock(0.5 * (from + to));
        self.plan.state_at(day, hours, self.probe).0
    }

    /// Where the next slot after `game` starts, in game hours.
    fn next_change(&self, game: f64) -> f64 {
        // A hair past `game`, so a slot whose start rounds a little early in `f32` reads as begun.
        let past = game + 1e-5;
        let (day, hours) = Self::clock(past);
        let (day, hours) = self.plan.schedule().next_slot(day, hours);
        (day as f64 * f64::from(DAY_HOURS) + f64::from(hours)).max(past)
    }
}

impl Iterator for Flips<'_> {
    type Item = (f64, usize);

    fn next(&mut self) -> Option<(f64, usize)> {
        while self.cycling && self.cursor < self.end {
            let begins = self.next_change(self.cursor);
            if begins >= self.end {
                self.cursor = self.end;
                return None;
            }
            let state = self.state(begins, self.next_change(begins).min(self.end));
            self.cursor = begins;
            if state != self.wanted {
                // The blend weighs the incoming state over the outgoing halfway through its transition.
                let halfway = f64::from(self.document.seconds(self.wanted, state)) * 0.5;
                let at = ((begins - self.start) * self.hour_seconds + halfway).max(self.last);
                (self.wanted, self.last) = (state, at);
                return Some((at, state));
            }
        }
        None
    }
}

/// A zone's weather as it reads: its states and its plan over them, its regions in the zone's space (so
/// the probe a resolve takes is too).
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherSetup {
    pub document: Arc<WeatherDocument>,
    pub plan: WeatherPlan,
}

impl WeatherSetup {
    /// Reads a cooked plan: its states' document, and every name it gives held to them. A cook checked all
    /// of this; a zone that doesn't read can't come (Z3 review F4), so a reader still says why. Parses RON:
    /// off the frame's thread.
    pub fn read(plan: &CookedWeatherPlan) -> Result<Self, WeatherError> {
        let document = WeatherDocument::from_ron(&plan.states)
            .map_err(|error| WeatherError::new("weather_plan.states", error.to_string()))?;
        let state = |field: String, name: &str| {
            document
                .state(name)
                .ok_or_else(|| WeatherError::new(field, format!("`{name}` isn't one of its states")))
        };
        let mut slots = Vec::with_capacity(plan.schedule.len());
        for (index, slot) in plan.schedule.iter().enumerate() {
            let mut choices = Vec::with_capacity(slot.states.len());
            for (choice, pick) in slot.states.iter().enumerate() {
                choices.push((
                    state(format!("weather_plan.schedule[{index}].states[{choice}]"), &pick.state)?,
                    pick.weight,
                ));
            }
            slots.push((slot.from, choices));
        }
        let schedule = WeatherSchedule::new(plan.seed, slots, document.states().len())
            .map_err(|error| WeatherError::new(format!("weather_plan.{}", error.field), error.problem))?;
        let mut regions = Vec::with_capacity(plan.regions.len());
        for (index, region) in plan.regions.iter().enumerate() {
            regions.push(WeatherRegion {
                shape: VolumeShape::from_zone(region.position, region.shape),
                priority: region.priority,
                state: state(format!("weather_plan.regions[{index}].state"), &region.state)?,
            });
        }
        Ok(Self { document: Arc::new(document), plan: WeatherPlan::new(schedule, regions) })
    }

    /// Whether a state its plan may show wets surfaces (M6 Y2, [`WeatherPlan::may_be_wet`]).
    pub fn may_be_wet(&self) -> bool {
        self.plan.may_be_wet(&self.document)
    }
}

/// The clock weather reads, for one frame: the world's seconds, which transitions run on, and time of
/// day's day and hour, which the schedule picks by. Presentation's clocks alone, interpolated as
/// transforms draw (never the simulation's step).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherClock {
    /// Seconds of world time (the frame packet's `time_seconds`): pause and slow motion hold or slow it.
    pub seconds: f64,
    /// Time of day's day ([`crate::tod::TodClock::day`]).
    pub day: i64,
    /// Time of day's hour ([`crate::tod::TodClock::hours`]).
    pub hours: f32,
    /// The seconds a day takes ([`crate::tod::TodClock::day_length`]), `None` while time of day stands
    /// still: how far back a re-warm's six game hours reach (M6 Y2).
    pub day_length: Option<f32>,
}

impl WeatherClock {
    /// `time_of_day`'s day and hour `alpha` of the way through the latest step, at `seconds` of world time.
    pub fn of(time_of_day: &TimeOfDay, seconds: f64, alpha: f32) -> Self {
        let clock = time_of_day.clock();
        Self { seconds, day: clock.day(alpha), hours: clock.hours(alpha), day_length: clock.day_length() }
    }
}

/// A story flag's weather: `state` over the schedule and the regions, arriving in `seconds` of world time
/// if it gives them, or the document's for the pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherOverride {
    pub state: usize,
    pub seconds: Option<f32>,
}

/// What weather did in its last resolve (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherStats {
    /// CPU time of the last [`WeatherSystem::resolve`].
    pub resolve_time: Duration,
    /// States live, 0 to 2.
    pub live: u8,
    /// The world's transition, 0 to 1: 1 settled.
    pub transition: f32,
    /// The world's state, by index, if it has one.
    pub state: Option<usize>,
    /// The region the probe stood in, by its index in [`WeatherPlan::regions`].
    pub region: Option<usize>,
    pub overridden: bool,
    /// A cutscene's weather's weight, 0 without one.
    pub cutscene_weight: f32,
    pub tool: bool,
    pub resolves: u64,
    /// The world's surfaces' shares after the last resolve (M6 Y2).
    pub shares: Weather,
    /// Re-warms since the system was made: one a zone's arrival or a load.
    pub rewarms: u64,
}

/// The shares between changes of the world's dominant state (M6 Y2): where the last change left them,
/// when, and the state they move toward since, none drying.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Accumulated {
    from: Weather,
    since: f64,
    toward: Option<usize>,
    /// Where they are at the last resolve.
    now: Weather,
    /// The world clock's seconds at the last resolve.
    at: f64,
}

/// The world's weather (see the [module docs](self)). A resource for a world that wants one; the stage holds
/// its own as a field.
#[derive(Resource, Debug)]
pub struct WeatherSystem {
    document: Arc<WeatherDocument>,
    plan: Option<WeatherPlan>,
    /// The override and the tool's blend, held by their states' names too, which a new document
    /// resolves again ([`Self::set_plan`]).
    request: Option<(WeatherOverride, String)>,
    tool: Option<(WeatherBlend, Option<String>, String)>,
    world: Option<Transition>,
    /// The next resolve shows the world's state at once: the first, and the first in a new plan.
    snap: bool,
    blend: Option<WeatherBlend>,
    shot: Option<(usize, f32)>,
    frame: WeatherFrame,
    /// The world's surfaces' shares (M6 Y2), re-warmed whenever the world's state snaps.
    shares: Accumulated,
    stats: WeatherStats,
}

/// How many times faster than its own a transition finishes when the world wants a third state.
const TAIL_RATE: f32 = 4.0;

/// The world's state changing from `from` to `to` (either none: no state, the day as it is), from
/// `start` over `seconds`, eased slow at both ends; then, if the world wants a third state, toward `then`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transition {
    from: Option<usize>,
    to: Option<usize>,
    start: f64,
    seconds: f32,
    /// The state the world wants once this one finishes (`Some(None)`: no state).
    then: Option<Option<usize>>,
}

impl Transition {
    fn settled(state: Option<usize>, now: f64) -> Self {
        Self { from: state, to: state, start: now, seconds: 0.0, then: None }
    }

    /// Where it is headed in the end.
    fn aim(&self) -> Option<usize> {
        self.then.unwrap_or(self.to)
    }

    /// How far along it is at `now`, 0 to 1, before easing.
    fn progress(&self, now: f64) -> f32 {
        if self.seconds <= 0.0 || self.from == self.to {
            return 1.0;
        }
        (((now - self.start) / f64::from(self.seconds)) as f32).clamp(0.0, 1.0)
    }

    /// The states and weights at `now`: `to` over `from` by the eased progress.
    fn blend(&self, now: f64) -> Option<WeatherBlend> {
        let weight = Ease::Smooth.apply(self.progress(now));
        match (self.from, self.to) {
            (Some(from), Some(to)) => Some(WeatherBlend { from: Some(from), to, weight }),
            (None, Some(to)) => Some(WeatherBlend { from: None, to, weight }),
            (Some(from), None) => Some(WeatherBlend { from: None, to: from, weight: 1.0 - weight }),
            (None, None) => None,
        }
    }
}

impl Default for WeatherSystem {
    /// The shipped states ([`default_weather`]) with no plan: no weather until a zone's plan, a story
    /// flag, a cutscene or the tool names some.
    fn default() -> Self {
        Self::new(default_weather(), None)
    }
}

impl WeatherSystem {
    /// Weather of `document`'s states, chosen by `plan` if there is one.
    pub fn new(document: Arc<WeatherDocument>, plan: Option<WeatherPlan>) -> Self {
        Self {
            document,
            plan,
            request: None,
            tool: None,
            world: None,
            snap: true,
            blend: None,
            shot: None,
            frame: WeatherFrame::default(),
            shares: Accumulated::default(),
            stats: WeatherStats::default(),
        }
    }

    pub fn document(&self) -> &Arc<WeatherDocument> {
        &self.document
    }

    pub fn plan(&self) -> Option<&WeatherPlan> {
        self.plan.as_ref()
    }

    /// Takes a zone's states and plan (or the stage's own states and none), showing the plan's state at
    /// once on the next resolve, which re-warms the shares from the plan's last six game hours
    /// ([`rewarm`]): a zone arrives in its weather, its streets as wet as its last hours left them. An
    /// override and a tool's blend keep their states by name, found again in the new document; one naming
    /// a state it hasn't is dropped.
    pub fn set_plan(&mut self, document: Arc<WeatherDocument>, plan: Option<WeatherPlan>) {
        self.request = self.request.take().and_then(|(request, name)| {
            let state = document.state(&name)?;
            Some((WeatherOverride { state, ..request }, name))
        });
        self.tool = self.tool.take().and_then(|(blend, from_name, to_name)| {
            let from = match &from_name {
                Some(name) => Some(document.state(name)?),
                None => None,
            };
            let to = document.state(&to_name)?;
            Some((WeatherBlend { from, to, ..blend }, from_name, to_name))
        });
        self.document = document;
        self.plan = plan;
        self.world = None;
        self.snap = true;
    }

    /// A story flag's state over the plan's, or `None` to give the plan back, by a transition either way.
    /// One naming no state of the document is refused. It holds the state by name across a new document.
    pub fn set_override(&mut self, request: Option<WeatherOverride>) -> Result<(), WeatherError> {
        let Some(request) = request else {
            self.request = None;
            return Ok(());
        };
        let Some(state) = self.document.states().get(request.state) else {
            return Err(WeatherError::new("override.state", "isn't one of the states"));
        };
        if request.seconds.is_some_and(|seconds| !(seconds.is_finite() && seconds >= 0.0)) {
            return Err(WeatherError::new("override.seconds", "must be 0 or more"));
        }
        self.request = Some((request, state.name.clone()));
        Ok(())
    }

    pub fn override_state(&self) -> Option<WeatherOverride> {
        self.request.as_ref().map(|(request, _)| *request)
    }

    /// The tool's blend in place of everything else (a capture's, the overlay's), or `None` to give the
    /// world's back at once. One naming no state of the document is refused. It holds its states by name
    /// across a new document.
    pub fn set_tool(&mut self, blend: Option<WeatherBlend>) -> Result<(), WeatherError> {
        let Some(blend) = blend else {
            self.tool = None;
            return Ok(());
        };
        let states = self.document.states();
        let refused = || WeatherError::new("tool", "names a state the document hasn't");
        let to = states.get(blend.to).ok_or_else(refused)?.name.clone();
        let from = match blend.from {
            Some(from) => Some(states.get(from).ok_or_else(refused)?.name.clone()),
            None => None,
        };
        self.tool = Some((blend, from, to));
        Ok(())
    }

    pub fn tool(&self) -> Option<WeatherBlend> {
        self.tool.as_ref().map(|(blend, ..)| *blend)
    }

    /// Chooses and blends the weather for a frame at `clock`, seen from `probe`, with a playing cutscene's
    /// state and weight (`TimeOfDay::cutscene_weather`): see the [module docs](self). Allocates nothing.
    pub fn resolve(
        &mut self,
        clock: WeatherClock,
        probe: Vec3,
        cutscene: Option<(&str, f32)>,
    ) -> &WeatherFrame {
        profiling::scope!("WeatherSystem::resolve");
        let stopwatch = Stopwatch::start();
        let now = clock.seconds;
        let (planned, region) = match &self.plan {
            Some(plan) => {
                let (state, region) = plan.state_at(clock.day, clock.hours, probe);
                (Some(state), region)
            }
            None => (None, None),
        };
        let wanted = self.request.as_ref().map(|(request, _)| request.state).or(planned);
        let world = match self.world.filter(|_| !self.snap) {
            Some(world) => {
                let world = self.go_on(world, now);
                // A tail already done (a transition changed at its very start) goes on at once.
                if world.aim() == wanted { world } else { self.go_on(self.change(world, wanted, now), now) }
            }
            None => Transition::settled(wanted, now),
        };
        let snapped = std::mem::take(&mut self.snap);
        self.world = Some(world);
        let mut blend = world.blend(now);
        self.accumulate(clock, probe, blend.and_then(|blend| blend.dominant()), snapped);
        let mut shot = None;
        if let Some((name, weight)) = cutscene
            && let Some(state) = self.document.state(name)
            && weight > 0.0
        {
            shot = Some((state, weight.min(1.0)));
        }
        let cutscene_weight = shot.map_or(0.0, |(_, weight)| weight);
        if let Some((tool, ..)) = &self.tool {
            (blend, shot) = (Some(*tool), None);
        }
        self.blend = blend;
        self.shot = shot;
        self.frame = if blend.is_none() && shot.is_none() {
            WeatherFrame::default()
        } else {
            self.document.frame_over(blend, shot)
        };
        self.stats = WeatherStats {
            resolve_time: stopwatch.elapsed(),
            live: self.frame.live,
            transition: Ease::Smooth.apply(world.progress(now)),
            state: world.aim(),
            region,
            overridden: self.request.is_some(),
            cutscene_weight,
            tool: self.tool.is_some(),
            resolves: self.stats.resolves + 1,
            shares: self.shares.now,
            rewarms: self.stats.rewarms + u64::from(snapped),
        };
        &self.frame
    }

    /// Moves the shares to `clock`, toward the world's `dominant` state from now on (M6 Y2): re-warmed when
    /// the world's state `snapped`, else settled from where the last change of state left them.
    fn accumulate(&mut self, clock: WeatherClock, probe: Vec3, dominant: Option<usize>, snapped: bool) {
        let now = clock.seconds;
        let shares = &mut self.shares;
        if snapped {
            let warm =
                self.plan.as_ref().map_or(Weather::DRY, |plan| rewarm(&self.document, plan, clock, probe));
            *shares = Accumulated { from: warm, since: now, toward: dominant, now: warm, at: now };
            return;
        }
        // A clock set back (never the world's own) holds the shares where the last resolve left them
        // and moves them on from there: no share runs backward.
        let back = now < shares.at;
        if !back {
            let (toward, temperature) = accumulation(&self.document, shares.toward);
            shares.now = shares.from.settle(toward, temperature, now - shares.since);
        }
        shares.at = now;
        if dominant != shares.toward || back {
            *shares =
                Accumulated { from: shares.now, since: now, toward: dominant, now: shares.now, at: now };
        }
    }

    /// The world's surfaces' shares after the last resolve (M6 Y2; see the [module docs](self)): what the
    /// frame's weather is made from where nothing lies over it.
    pub fn shares(&self) -> Weather {
        self.shares.now
    }

    /// Whether the shares may go wet here (M6 Y2): wet already, or a state the plan may show or the
    /// override names wets. What a stage declares the wet surfaces' pipelines by.
    pub fn may_be_wet(&self) -> bool {
        self.shares.now.is_wet()
            || self.plan.as_ref().is_some_and(|plan| plan.may_be_wet(&self.document))
            || self.override_wets()
    }

    /// Whether the override's state wets (M6 Y2): a new plan keeps it by its name ([`Self::set_plan`]), so
    /// a trip declares the wet surfaces by it beside the coming zone's own weather.
    pub fn override_wets(&self) -> bool {
        self.request.as_ref().is_some_and(|(request, _)| wets(&self.document, Some(request.state)))
    }

    /// Whether the last resolve's air and light allow a rainbow (M6 Y2, [`Rainbow::condition`]), and where:
    /// the shares' wetness and the frame's rain, under a sun `toward_sun` (a direction toward it) and clouds
    /// covering `coverage` of the sky, both as time of day resolved them this frame. For the sky (V11).
    pub fn rainbow(&self, toward_sun: Vec3, coverage: f32) -> Rainbow {
        Rainbow::condition(self.shares.now, self.frame.rain(), toward_sun, coverage)
    }

    /// Seconds a change from `from` to `to` takes: the override's own if it is the override's state, or the
    /// document's for the pair.
    fn seconds(&self, from: Option<usize>, to: Option<usize>) -> f32 {
        let own = self
            .request
            .as_ref()
            .filter(|(request, _)| Some(request.state) == to)
            .and_then(|(request, _)| request.seconds);
        own.unwrap_or_else(|| match (from, to) {
            (Some(from), Some(to)) => self.document.seconds(from, to),
            _ => self.document.transitions().seconds,
        })
    }

    /// `world` at `now`: a finished transition with a state queued starts toward it from where it ended.
    fn go_on(&self, world: Transition, now: f64) -> Transition {
        match world.then {
            Some(then) if world.progress(now) >= 1.0 => {
                let end = world.start + f64::from(world.seconds);
                if then == world.to {
                    return Transition::settled(then, end);
                }
                Transition {
                    from: world.to,
                    to: then,
                    start: end,
                    seconds: self.seconds(world.to, then),
                    then: None,
                }
            }
            _ => world,
        }
    }

    /// The world's state changing to `wanted` from `world` at `now`, never jumping:
    ///
    /// - settled, a transition from where it is;
    /// - back the way it came, turned round from where it is (the ease is symmetric, so the weights hold);
    /// - toward the state a quickened transition is finishing on, that transition alone;
    /// - toward a third state, the transition finishes first on its heavier state, [`TAIL_RATE`] times
    ///   faster from where it is (turned round if the outgoing state is the heavier), and the change to
    ///   `wanted` follows from there. So two states at most are ever the world's.
    fn change(&self, world: Transition, wanted: Option<usize>, now: f64) -> Transition {
        let progress = world.progress(now);
        if progress >= 1.0 {
            if world.to == wanted {
                return Transition::settled(wanted, now);
            }
            let seconds = self.seconds(world.to, wanted);
            return Transition { from: world.to, to: wanted, start: now, seconds, then: None };
        }
        if wanted == world.to {
            return Transition { then: None, ..world };
        }
        if wanted == world.from && world.then.is_none() {
            let seconds = self.seconds(world.to, wanted);
            let start = now - f64::from(1.0 - progress) * f64::from(seconds);
            return Transition { from: world.to, to: wanted, start, seconds, then: None };
        }
        // A third state: finish on the heavier one, quickly, from exactly where the blend is.
        let seconds = world.seconds / TAIL_RATE;
        let (from, to, done) = if progress >= 0.5 {
            (world.from, world.to, progress)
        } else {
            (world.to, world.from, 1.0 - progress)
        };
        let start = now - f64::from(done) * f64::from(seconds);
        Transition { from, to, start, seconds, then: Some(wanted) }
    }

    /// The last resolve's blend, which [`TimeOfDay::set_weather_blend`] takes: `None` with no state live.
    pub fn blend(&self) -> Option<WeatherBlend> {
        self.blend
    }

    /// The last resolve's frame (before the first, no weather).
    pub fn frame(&self) -> &WeatherFrame {
        &self.frame
    }

    pub fn stats(&self) -> WeatherStats {
        self.stats
    }

    /// Hands the last resolve's states and blend to `time_of_day`, whose look they lie in, and whether the
    /// shares stand for its day's keyed weather: while a plan is live (M6 Y2, [`TimeOfDay::set_weather_shares`]).
    pub fn write(&self, time_of_day: &mut TimeOfDay) {
        let same = time_of_day.weather().is_some_and(|document| Arc::ptr_eq(document, &self.document));
        if !same {
            time_of_day.set_weather(Some(self.document.clone()));
        }
        time_of_day.set_weather_blend(self.blend);
        time_of_day.set_weather_shot(self.shot);
        time_of_day.set_weather_shares(self.plan.is_some());
    }

    /// The last resolve's cutscene state and its weight, laid over [`Self::blend`]'s.
    pub fn shot(&self) -> Option<(usize, f32)> {
        self.shot
    }
}

#[cfg(test)]
mod tests;
