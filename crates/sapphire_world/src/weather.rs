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
//! # Never the simulation
//!
//! Weather is presentation. The simulation never reads it, a replay never records it and no simulation
//! digest covers it: the schedule hashes the presentation's clock, and a zone's plan lies in its
//! presentation, never its simulation.
//!
//! # Later
//!
//! - **Y2** reads the frame's accumulation into `FrameUniforms.weather`'s five shares, drying each at its
//!   rate; **V**'s packages read the environment's cover, rings, aurora and fog density, **N** the
//!   precipitation and **U** the audio parameters. Until each lands, its groups are data only.
//! - **W8b** the editor's weather panel over these documents.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use bevy_ecs::prelude::Resource;
use sapphire_look::WeatherFrame;
use sapphire_math::{Ease, Vec3};
use sapphire_time::Stopwatch;
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
}

impl WeatherClock {
    /// `time_of_day`'s day and hour `alpha` of the way through the latest step, at `seconds` of world time.
    pub fn of(time_of_day: &TimeOfDay, seconds: f64, alpha: f32) -> Self {
        let clock = time_of_day.clock();
        Self { seconds, day: clock.day(alpha), hours: clock.hours(alpha) }
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
    /// once on the next resolve: a zone arrives in its weather. An override and a tool's blend keep their
    /// states by name, found again in the new document; one naming a state it hasn't is dropped.
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
        self.snap = false;
        self.world = Some(world);
        let mut blend = world.blend(now);
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
        };
        &self.frame
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

    /// Hands the last resolve's states and blend to `time_of_day`, whose look they lie in.
    pub fn write(&self, time_of_day: &mut TimeOfDay) {
        let same = time_of_day.weather().is_some_and(|document| Arc::ptr_eq(document, &self.document));
        if !same {
            time_of_day.set_weather(Some(self.document.clone()));
        }
        time_of_day.set_weather_blend(self.blend);
        time_of_day.set_weather_shot(self.shot);
    }

    /// The last resolve's cutscene state and its weight, laid over [`Self::blend`]'s.
    pub fn shot(&self) -> Option<(usize, f32)> {
        self.shot
    }
}

#[cfg(test)]
mod tests;
