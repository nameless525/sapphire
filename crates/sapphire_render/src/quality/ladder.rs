//! The thermal ladder (M4 T2; spec §6.3, m4-plan §3 "The ladder"): what a tier gives up, in order, when
//! the device runs hot or the GPU can't hold the tier's budget, and when it takes each back.
//!
//! # Rungs
//!
//! A tier's rungs are data, `ladder` in `tiers.ron` ([`Rung`]), and spend what the look can spare first.
//! Each holds what the rungs above it hold ([`LadderState`]): at depth 2 a desktop renders at most 0.85 of
//! its base scale *and* sheds its optional passes. Four kinds:
//!
//! - [`Rung::Scale`]: the render scale at most this fraction of the base. Dynamic resolution works below it
//!   ([`DynamicResolution::set_ceiling`](super::DynamicResolution::set_ceiling)), and a scale under the
//!   tier's dynamic floor holds there: Mobile-Low's 0.75 is a rung only.
//! - [`Rung::Shed`]: optional passes off, each one step ([`OptionalPass`]), in place:
//!   [`Renderer::set_shed`](crate::Renderer::set_shed) changes no pipeline, so a rung builds nothing.
//!   GTAO keeps the environment's pipelines and reads an open stand-in, its passes skipped; a bloom level
//!   fewer draws a shorter chain with the same pipelines; the sky's face rate is a schedule. The cirrus and
//!   the wind levels aren't rungs (m4-plan §3: T0 measured no saving from either, and stilled leaves
//!   show), and a ladder can't hold them: only a new renderer sheds them, the wind levels being pipeline
//!   constants.
//! - [`Rung::FrameRate`]: frames per second at most, which the platform paces to; dynamic resolution's
//!   budgets stretch to the longer frame ([`QualityTier::resolution_budget_at`]).
//! - [`Rung::Tier`]: a lower tier, the last rung. The ladder only asks: a tier change makes a new renderer,
//!   so the game takes it at a pause or a wipe, never in a fight
//!   ([`Quality::take_tier_request`](super::Quality::take_tier_request)). The new tier's ladder starts at its deepest rung short of a tier, since everything above was already given,
//!   and climbs back as its holds allow; at its top and calm for [`TIER_HOLD`] it asks for the tier it came
//!   from, which starts the same way.
//!
//! No rung touches the character family, the outlines, the ramps or the grade.
//!
//! # Going down
//!
//! - **Heat** ([`heat`]): the OS's state, Android's headroom forecast and iOS's Low Power Mode, as one level
//!   on Android's scale. `Light` asks for the first rung, `Moderate` the first two, `Severe` every rung
//!   short of a tier, and `Critical` all of them. Heat goes straight to its depth: Google's guide says to
//!   cut load "immediately" from a forecast of 0.95 (below).
//! - **Pressure:** dynamic resolution at its lowest scale and still over its budget
//!   ([`Pressure::Over`], [`DynamicResolution::pressure`](super::DynamicResolution::pressure)). Held for
//!   [`PRESSURE_DWELL`] it takes one more rung, skipping a scale the controller already renders at or
//!   below, and waits as long again before the next, so each rung's saving shows in the timings first.
//!   Desktops, which report no heat, go down the ladder only this way. Dynamic resolution is off by
//!   default, and then there's none.
//!
//! # Coming back
//!
//! One rung at a time, only while heat asks for less than the ladder holds and the GPU has room for the
//! rung back: the frame under dynamic resolution's low-water mark (85% of its budget) at the budget the
//! rung above would give it, twice as long a frame under a 30 fps cap as at 60 ([`Pressure::Within`]),
//! or nothing measured (dynamic resolution off). The hold is measured from the later of the last change
//! and the moment both became true: [`HOLD`] (10 s) for most rungs, [`FRAME_RATE_HOLD`] (60 s) for a
//! frame-rate cap, which shows the most when it goes and comes. A load that only fits the rung below
//! stays there rather than coming back to press again. A ladder that has to go down again within
//! [`REDESCENT_WINDOW`] of coming up doubles its holds, up to [`MAX_BACKOFF`] times, until it has stood at
//! its top for [`CALM_RESET`]: heat that comes and goes settles a rung lower instead of flapping.
//!
//! # Sources
//!
//! Read 2026-11-10 (`research_notes/Ship readiness for M4/thermal.md` has the scout's notes):
//!
//! - developer.android.com/games/optimize/adpf/thermal: a headroom of 1.0 is throttling at `SEVERE`;
//!   at 0.95 "reduce workload immediately"; at 0.85 "reduce workload if possible"; above 1.0 reduce it and
//!   keep it lower until headroom falls. Those three are [`HEADROOM_LIGHT`], [`HEADROOM_MODERATE`] and
//!   [`HEADROOM_SEVERE`]. It gives no hold: quality comes back "once headroom decreases to safer levels".
//! - The holds are thermal.md's ("ours", the scout's): about 10 s below a threshold, and about 60 s before
//!   a frame-rate cap comes off. m4-plan §3 settles them.
//! - Low Power Mode counts as `Severe`, as thermal.md proposes for iOS's `serious`: the player asked the
//!   phone to save power, and the frame-rate cap is what saves it.
//!
//! The controller is pure: time comes in as a [`Duration`] from the app's clock, and nothing here reads
//! the OS. It allocates nothing after it is made.

use std::time::Duration;

use bevy_reflect::Reflect;
use sapphire_core::thermal::{ThermalReading, ThermalState};
use serde::{Deserialize, Serialize};

use super::dynamic::LOW_WATER;
use super::{OptionalPass, OptionalPasses, Pressure, QualityTier, TierId};
use crate::MIN_RENDER_SCALE;

/// The hold before a rung comes back (m4-plan §3: "about 10 s").
pub const HOLD: Duration = Duration::from_secs(10);
/// The hold before a frame-rate cap comes off (m4-plan §3: 60 s).
pub const FRAME_RATE_HOLD: Duration = Duration::from_secs(60);
/// How long a lowered tier is calm at its top before it asks for the tier it came from, and before a
/// requested lower tier is given up.
pub const TIER_HOLD: Duration = Duration::from_secs(60);
/// How long dynamic resolution's pressure holds before it takes a rung, and between rungs it takes.
pub const PRESSURE_DWELL: Duration = Duration::from_secs(2);
/// Going down again this soon after coming up doubles the holds.
pub const REDESCENT_WINDOW: Duration = Duration::from_secs(30);
/// The holds grow at most this many times.
pub const MAX_BACKOFF: u32 = 8;
/// The holds go back to their own length after this long at the top.
pub const CALM_RESET: Duration = Duration::from_secs(300);
/// Headroom forecasts at or above these count as the state they name (Google's guide, above).
pub const HEADROOM_LIGHT: f32 = 0.85;
pub const HEADROOM_MODERATE: f32 = 0.95;
pub const HEADROOM_SEVERE: f32 = 1.0;
/// The most rungs a tier lists.
pub const MAX_RUNGS: usize = 8;

/// One rung of a tier's ladder (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
pub enum Rung {
    /// The render scale at most this fraction of the tier's base scale.
    Scale(f32),
    /// These optional passes off, in place.
    Shed(Vec<OptionalPass>),
    /// Frames per second at most.
    FrameRate(u32),
    /// A lower tier of the same class, at the game's next pause or wipe.
    Tier(TierId),
}

impl Rung {
    /// How long this rung holds once nothing asks for it.
    pub fn hold(&self) -> Duration {
        match self {
            Self::FrameRate(_) => FRAME_RATE_HOLD,
            Self::Tier(_) => TIER_HOLD,
            Self::Scale(_) | Self::Shed(_) => HOLD,
        }
    }
}

/// Checks `tier`'s ladder, returning what is wrong.
pub(super) fn validate(tier: &QualityTier) -> Result<(), &'static str> {
    let rungs = &tier.ladder;
    if rungs.len() > MAX_RUNGS {
        return Err("lists more than 8 rungs");
    }
    let mut scale = 1.0;
    let mut shed = OptionalPasses::NONE;
    let mut frame_rate = None;
    for (index, rung) in rungs.iter().enumerate() {
        match rung {
            Rung::Scale(next) => {
                if !(MIN_RENDER_SCALE..scale).contains(next) {
                    return Err("scales must fall, each within 0.25..1");
                }
                scale = *next;
            }
            Rung::Shed(passes) => {
                if passes.is_empty() {
                    return Err("sheds nothing");
                }
                for &pass in passes {
                    if !pass.in_place() {
                        return Err("sheds a pass only a new renderer can: the cirrus or a wind level");
                    }
                    if shed.contains(pass) {
                        return Err("sheds a pass twice");
                    }
                    shed = shed.with(pass);
                }
            }
            Rung::FrameRate(rate) => {
                if frame_rate.is_some() || !(1..tier.frame_rate).contains(rate) {
                    return Err("caps the frame rate more than once, or not below the tier's");
                }
                frame_rate = Some(*rate);
            }
            Rung::Tier(lower) => {
                if lower.class() != tier.id.class() || *lower >= tier.id || index + 1 != rungs.len() {
                    return Err("a tier must be lower, of the same class, and the last rung");
                }
            }
        }
    }
    Ok(())
}

/// What the rungs down to a depth hold together.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LadderState {
    /// Rungs taken, from the top.
    pub depth: usize,
    /// The render scale at most, as a fraction of the base: 1 for none.
    pub scale: f32,
    /// Optional passes off.
    pub shed: OptionalPasses,
    /// A frame-rate cap.
    pub frame_rate: Option<u32>,
    /// A lower tier asked for.
    pub tier: Option<TierId>,
}

impl LadderState {
    /// The ladder's top: everything the tier draws.
    pub const TOP: Self =
        Self { depth: 0, scale: 1.0, shed: OptionalPasses::NONE, frame_rate: None, tier: None };

    /// What `rungs` hold together.
    pub fn of(rungs: &[Rung]) -> Self {
        rungs.iter().fold(Self { depth: rungs.len(), ..Self::TOP }, |state, rung| match rung {
            Rung::Scale(scale) => Self { scale: state.scale.min(*scale), ..state },
            Rung::Shed(passes) => {
                Self { shed: passes.iter().fold(state.shed, |shed, &pass| shed.with(pass)), ..state }
            }
            Rung::FrameRate(rate) => {
                Self { frame_rate: Some(state.frame_rate.map_or(*rate, |cap| cap.min(*rate))), ..state }
            }
            Rung::Tier(tier) => Self { tier: Some(*tier), ..state },
        })
    }
}

/// Why the ladder moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// Heat asked for at least this depth.
    Heat(ThermalState),
    /// Dynamic resolution couldn't hold its budget at its lowest scale.
    Pressure,
    /// Nothing asked for the rung through its hold.
    Calm,
    /// `r.rung` holds the ladder at a depth.
    Forced,
}

/// A move of the ladder: what the app applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LadderStep {
    pub from: LadderState,
    pub to: LadderState,
    pub cause: Cause,
}

/// The ladder's counters (rule R9).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LadderStats {
    pub state: LadderState,
    /// Rungs the tier lists.
    pub rungs: usize,
    /// Heat as the last reading gave it.
    pub heat: ThermalState,
    /// Dynamic resolution pressed in the last update.
    pub pressed: bool,
    /// How many times the holds are stretched.
    pub backoff: u32,
    /// Moves since the ladder was made.
    pub steps: u64,
    /// A tier the ladder asks for: a lower one, or the one a lowered tier came from.
    pub tier_request: Option<TierId>,
}

/// Heat on Android's scale, from everything a reading has (see the [module docs](self)): the OS's state,
/// the headroom forecast by Google's thresholds, and Low Power Mode as `Severe`. `Unknown` reads as
/// `Nominal`: a desktop reports no heat.
pub fn heat(reading: &ThermalReading) -> ThermalState {
    let forecast = match reading.headroom {
        Some(headroom) if headroom >= HEADROOM_SEVERE => ThermalState::Severe,
        Some(headroom) if headroom >= HEADROOM_MODERATE => ThermalState::Moderate,
        Some(headroom) if headroom >= HEADROOM_LIGHT => ThermalState::Light,
        _ => ThermalState::Nominal,
    };
    let power = if reading.low_power == Some(true) { ThermalState::Severe } else { ThermalState::Nominal };
    reading.state.max(forecast).max(power)
}

/// One tier's ladder and where it stands (see the [module docs](self)).
#[derive(Clone, Debug)]
pub struct Ladder {
    rungs: Vec<Rung>,
    /// The tier's own frame rate, which a cap's room is measured against.
    frame_rate: u32,
    /// Rungs before a tier rung.
    soft: usize,
    /// The tier this one was lowered from, which it asks for again once calm.
    lowered_from: Option<TierId>,
    depth: usize,
    state: LadderState,
    heat: ThermalState,
    pressed_since: Option<Duration>,
    /// When demand fell below the depth.
    calm_since: Option<Duration>,
    /// When a lowered ladder last stood calm at its top.
    top_since: Option<Duration>,
    changed_at: Option<Duration>,
    rose_at: Option<Duration>,
    backoff: u32,
    raise: Option<TierId>,
    steps: u64,
}

impl Ladder {
    /// `tier`'s ladder, from the top.
    pub fn new(tier: &QualityTier) -> Self {
        let rungs = &tier.ladder;
        let soft = rungs.iter().take_while(|rung| !matches!(rung, Rung::Tier(_))).count();
        Self {
            rungs: rungs.clone(),
            frame_rate: tier.frame_rate,
            soft,
            lowered_from: None,
            depth: 0,
            state: LadderState::TOP,
            heat: ThermalState::Unknown,
            pressed_since: None,
            calm_since: None,
            top_since: None,
            changed_at: None,
            rose_at: None,
            backoff: 1,
            raise: None,
            steps: 0,
        }
    }

    /// The ladder of `tier`, entered at `now` from another ladder, `from`: at the deepest rung short of
    /// a tier, to climb back as the holds allow, with `from`'s stretched holds. `lowered_from` is the
    /// tier to ask for again once calm, if this one is lower.
    pub fn entered(tier: &QualityTier, from: &Self, lowered_from: Option<TierId>, now: Duration) -> Self {
        let mut ladder = Self::new(tier);
        ladder.lowered_from = lowered_from;
        ladder.depth = ladder.soft;
        ladder.state = LadderState::of(&ladder.rungs[..ladder.soft]);
        ladder.changed_at = Some(now);
        ladder.rose_at = from.rose_at;
        ladder.backoff = from.backoff;
        ladder
    }

    /// What the rungs taken hold.
    pub fn state(&self) -> LadderState {
        self.state
    }

    pub fn rungs(&self) -> &[Rung] {
        &self.rungs
    }

    /// The tier the ladder asks for: its lower tier at its deepest rung, or, lowered and calm at its top
    /// for [`TIER_HOLD`], the tier it came from. The game takes it at a pause or a wipe.
    pub fn tier_request(&self) -> Option<TierId> {
        self.state.tier.or(self.raise)
    }

    pub fn stats(&self) -> LadderStats {
        LadderStats {
            state: self.state,
            rungs: self.rungs.len(),
            heat: self.heat,
            pressed: self.pressed_since.is_some(),
            backoff: self.backoff,
            steps: self.steps,
            tier_request: self.tier_request(),
        }
    }

    /// Takes a frame's `reading` at `now` (the app's clock, which only moves forward), and how loaded
    /// dynamic resolution says the GPU is. `forced` holds the ladder at a depth (`r.rung`), clamped to
    /// its rungs. Returns the move, if it moved. Call it once a frame; it allocates nothing.
    pub fn update(
        &mut self,
        now: Duration,
        reading: &ThermalReading,
        pressure: Pressure,
        forced: Option<usize>,
    ) -> Option<LadderStep> {
        self.heat = heat(reading);
        self.raise = None;
        if self.backoff > 1
            && self.depth == 0
            && self.changed_at.is_none_or(|at| now.saturating_sub(at) >= CALM_RESET)
        {
            self.backoff = 1;
        }
        if let Some(depth) = forced {
            self.pressed_since = None;
            self.calm_since = None;
            self.top_since = None;
            return self.go(depth.min(self.rungs.len()), now, Cause::Forced);
        }
        let heat_depth = self.heat_depth();
        if heat_depth > self.depth {
            self.pressed_since = None;
            self.calm_since = None;
            self.top_since = None;
            return self.go(heat_depth, now, Cause::Heat(self.heat));
        }
        if let Pressure::Over(factor) = pressure {
            self.calm_since = None;
            self.top_since = None;
            let since = *self.pressed_since.get_or_insert(now);
            let settled = self.changed_at.is_none_or(|at| now.saturating_sub(at) >= PRESSURE_DWELL);
            if now.saturating_sub(since) < PRESSURE_DWELL || !settled {
                return None;
            }
            self.pressed_since = Some(now);
            let next = self.next_saving(self.depth, factor)?;
            return self.go(next, now, Cause::Pressure);
        }
        self.pressed_since = None;
        // A GPU without room holds every rung: one back would press again.
        if !self.room(pressure) || heat_depth >= self.depth {
            self.calm_since = None;
            self.follow_top(now, heat_depth == 0 && self.room(pressure));
            return None;
        }
        self.top_since = None;
        let fell = *self.calm_since.get_or_insert(now);
        let since = self.changed_at.map_or(fell, |at| fell.max(at));
        if now.saturating_sub(since) < self.rungs[self.depth - 1].hold() * self.backoff {
            return None;
        }
        self.calm_since = Some(now);
        self.go(self.depth - 1, now, Cause::Calm)
    }

    /// Whether the GPU has room for the rung above: the frame under the low-water mark at the budget it
    /// would have there, or nothing measured.
    fn room(&self, pressure: Pressure) -> bool {
        match pressure {
            Pressure::Over(_) => false,
            Pressure::Unknown => true,
            Pressure::Within(share) => {
                let rate = |state: LadderState| state.frame_rate.unwrap_or(self.frame_rate) as f32;
                let above = match self.depth {
                    0 => self.state,
                    depth => LadderState::of(&self.rungs[..depth - 1]),
                };
                share * rate(above) / rate(self.state) <= LOW_WATER
            }
        }
    }

    /// The depth heat asks for.
    fn heat_depth(&self) -> usize {
        match self.heat {
            ThermalState::Unknown | ThermalState::Nominal => 0,
            ThermalState::Light => self.soft.min(1),
            ThermalState::Moderate => self.soft.min(2),
            ThermalState::Severe => self.soft,
            ThermalState::Critical => self.rungs.len(),
        }
    }

    /// The depth past `from` whose last rung saves something while dynamic resolution renders at
    /// `factor`: a scale at or above it changes nothing.
    fn next_saving(&self, from: usize, factor: f32) -> Option<usize> {
        let index = (from..self.rungs.len())
            .find(|&index| !matches!(self.rungs[index], Rung::Scale(scale) if scale >= factor))?;
        Some(index + 1)
    }

    /// A lowered ladder at its top with nothing asking (`calm`) asks for the tier it came from after
    /// [`TIER_HOLD`].
    fn follow_top(&mut self, now: Duration, calm: bool) {
        let Some(from) = self.lowered_from.filter(|_| calm && self.depth == 0) else {
            self.top_since = None;
            return;
        };
        let since = *self.top_since.get_or_insert(now);
        let since = self.changed_at.map_or(since, |at| since.max(at));
        if now.saturating_sub(since) >= TIER_HOLD * self.backoff {
            self.raise = Some(from);
        }
    }

    fn go(&mut self, depth: usize, now: Duration, cause: Cause) -> Option<LadderStep> {
        if depth == self.depth {
            return None;
        }
        if cause != Cause::Forced {
            if depth > self.depth {
                if self.rose_at.is_some_and(|rose| now.saturating_sub(rose) < REDESCENT_WINDOW) {
                    self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                }
            } else {
                self.rose_at = Some(now);
            }
        }
        let from = self.state;
        self.depth = depth;
        self.state = LadderState::of(&self.rungs[..depth]);
        self.changed_at = Some(now);
        self.steps += 1;
        Some(LadderStep { from, to: self.state, cause })
    }
}

#[cfg(test)]
mod tests;
