//! Where a recording's actions come from: [`ActionSource`], and [`SCRIPTS`], the sources written out
//! as code.
//!
//! The app's real source is its input bindings. Most of the suite comes from scripts instead, which
//! need no one to play them: **to add one,** write a function and add a row to [`SCRIPTS`], then
//! record it with `cargo xtask replay --record`.

use crate::actions::{ActorActions, Buttons, Stick, TickActions};
use crate::actors::ActorId;

/// Gives a recording each tick's actions: the seam between the simulation and what drives it.
///
/// `tick` counts from 0 and is the step about to run. `cast` is what the scenario's setup returned,
/// the player first. Whatever it returns is what's recorded and stepped, so a source needs no
/// determinism of its own: a live player's bindings qualify, and any closure does too.
pub trait ActionSource {
    fn actions(&mut self, tick: u64, cast: &[ActorId]) -> TickActions;
}

impl<F: FnMut(u64, &[ActorId]) -> TickActions> ActionSource for F {
    fn actions(&mut self, tick: u64, cast: &[ActorId]) -> TickActions {
        self(tick, cast)
    }
}

/// A named source written out as a function of the tick.
#[derive(Clone, Copy, Debug)]
pub struct Script {
    /// Its name on the command line.
    pub name: &'static str,
    /// A line on what it does, for listings.
    pub about: &'static str,
    /// How many ticks to record when nothing says otherwise.
    pub ticks: u32,
    /// The actions of a tick.
    pub actions: fn(u64, &[ActorId]) -> TickActions,
}

/// Every script.
pub const SCRIPTS: &[Script] = &[
    Script {
        name: "melee-sweep",
        about: "every actor presses every button on its own rhythm while its stick sweeps: no tick repeats",
        ticks: 1200,
        actions: melee_sweep,
    },
    Script {
        name: "melee-play",
        about: "the first actor walks in legs and attacks in bursts, as a person would, the rest stand",
        ticks: 3600,
        actions: melee_play,
    },
    Script {
        name: "string",
        about: "the three-hit string, each hit buffered ahead of its cancel window, over and over",
        ticks: 600,
        actions: string,
    },
    Script {
        name: "dodge-dash",
        about: "a string cut by a dodge, dash attacks out of dodges, an instant dodge and a skill, at an \
                opponent that moves",
        ticks: 720,
        actions: dodge_dash,
    },
    Script {
        name: "idle",
        about: "nobody touches anything: the smallest a replay can be",
        ticks: 3600,
        actions: idle,
    },
];

/// The script named `name`.
#[must_use]
pub fn find_script(name: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|script| script.name == name)
}

/// [`SCRIPTS`]' `melee-sweep`: every button on its own period, and sticks that sweep through their
/// whole range, for everything the melee scenario can do on a fixed schedule. The largest a replay
/// gets per actor, since no tick repeats.
#[must_use]
pub fn melee_sweep(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for (index, &actor) in cast.iter().enumerate() {
        let shifted = tick + index as u64 * 13;
        let every = |period: u64, offset: u64| shifted % period == offset;
        let mut buttons = Buttons::NONE;
        if every(19, 3) {
            buttons = buttons | Buttons::LIGHT;
        }
        if every(71, 9) {
            buttons = buttons | Buttons::HEAVY;
        }
        if every(47, 21) {
            buttons = buttons | Buttons::DODGE;
        }
        if every(131, 33) {
            buttons = buttons | Buttons::SKILL;
        }
        if every(97, 12) {
            buttons = buttons | Buttons::SWAP;
        }
        if every(173, 40) {
            buttons = buttons | Buttons::JUMP;
        }
        if index == 0 && every(211, 77) {
            buttons = buttons | Buttons::ULTIMATE;
        }
        let stick = Stick::new((shifted % 255) as u8 as i8, (shifted % 101) as u8 as i8);
        actions.set(actor, ActorActions { buttons, stick }).expect("the scenario's cast fits a tick");
    }
    actions
}

/// Ticks a leg of [`melee_play`]'s walk takes.
const LEG: u64 = 90;

/// [`SCRIPTS`]' `melee-play`: one actor walks a leg, easing the stick round over its first ticks and
/// letting go near its end, and presses in bursts that last a few ticks, as a thumb does. The stick
/// moves on about a quarter of the ticks, and the other actors stand: most ticks repeat.
#[must_use]
pub fn melee_play(tick: u64, cast: &[ActorId]) -> TickActions {
    const HEADINGS: [(i8, i8); 4] = [(127, 0), (40, 120), (-127, 10), (-30, -120)];
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };

    let into_leg = tick % LEG;
    let (x, y) = HEADINGS[(tick / LEG) as usize % HEADINGS.len()];
    // Percent of the way to the leg's heading.
    let scale = match into_leg {
        0..=24 => (into_leg as i32 + 1) * 4,
        25..=70 => 100,
        _ => 0,
    };
    let stick = Stick::new((i32::from(x) * scale / 100) as i8, (i32::from(y) * scale / 100) as i8);

    let held = |period: u64, offset: u64, ticks: u64| (tick + offset) % period < ticks;
    let mut buttons = Buttons::NONE;
    if held(47, 11, 5) {
        buttons = buttons | Buttons::LIGHT;
    }
    if held(211, 90, 8) {
        buttons = buttons | Buttons::HEAVY;
    }
    if held(137, 40, 4) {
        buttons = buttons | Buttons::DODGE;
    }
    if held(401, 200, 6) {
        buttons = buttons | Buttons::SKILL;
    }
    actions.set(player, ActorActions { buttons, stick }).expect("one actor fits a tick");
    actions
}

/// What's pressed on `tick` of a schedule that repeats every `period` ticks.
fn on_schedule(tick: u64, period: u64, schedule: &[(u64, Buttons)]) -> Buttons {
    let at = tick % period;
    schedule.iter().find(|(pressed, _)| *pressed == at).map_or(Buttons::NONE, |(_, buttons)| *buttons)
}

/// Ticks of [`string`]'s schedule: `light_1` from nothing at 0, then each hit pressed ahead of the next
/// one's window, so `light_2` fires at 10 as its window opens, and `light_3` at 22 the same way.
const STRING: [(u64, Buttons); 3] = [(0, Buttons::LIGHT), (6, Buttons::LIGHT), (16, Buttons::LIGHT)];

/// [`SCRIPTS`]' `string`: the player's three-hit string by buffered cancels, with room to finish it.
#[must_use]
pub fn string(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let buttons = on_schedule(tick, 96, &STRING);
    actions.set(player, ActorActions { buttons, stick: Stick::CENTER }).expect("one actor fits a tick");
    actions
}

/// Ticks of [`dodge_dash`]'s schedule for the player, by what the test content's moves allow.
const DODGE_DASH: [(u64, Buttons); 10] = [
    (0, Buttons::LIGHT),
    (6, Buttons::LIGHT),   // Buffered: light_2 at 10.
    (22, Buttons::LIGHT),  // Inside its window: light_3 at once.
    (30, Buttons::DODGE),  // A dodge cancel out of light_3, at once.
    (38, Buttons::LIGHT),  // Buffered a tick ahead of the dodge's window: dash attack at 39.
    (44, Buttons::DODGE),  // A dodge cancel out of the dash attack, at once.
    (50, Buttons::LIGHT),  // Buffered ahead of that dodge's window: dash attack at 53.
    (120, Buttons::DODGE), // The second half: a dodge from nothing,
    (138, Buttons::DODGE), // an instant dodge inside its own last ticks,
    (170, Buttons::SKILL), // and a skill from nothing.
];

/// Ticks of [`dodge_dash`]'s schedule for the opponent: it moves, so warps have something to track.
const OPPONENT: [(u64, Buttons); 2] = [(30, Buttons::SKILL), (90, Buttons::DODGE)];

/// [`SCRIPTS`]' `dodge-dash`: dodge cancels and dash attacks with their warps, at a moving opponent.
#[must_use]
pub fn dodge_dash(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let mut press = |actor: Option<&ActorId>, buttons: Buttons| {
        if let Some(&actor) = actor {
            let actor_actions = ActorActions { buttons, stick: Stick::CENTER };
            actions.set(actor, actor_actions).expect("two actors fit a tick");
        }
    };
    press(cast.first(), on_schedule(tick, 240, &DODGE_DASH));
    press(cast.get(1), on_schedule(tick, 120, &OPPONENT));
    actions
}

/// [`SCRIPTS`]' `idle`.
#[must_use]
pub fn idle(_tick: u64, _cast: &[ActorId]) -> TickActions {
    TickActions::NEUTRAL
}

#[cfg(test)]
mod tests {
    use super::super::is_valid_name;
    use super::*;

    #[test]
    fn every_script_has_a_good_unique_name_and_a_length() {
        for script in SCRIPTS {
            assert!(is_valid_name(script.name), "{}", script.name);
            assert!(!script.about.is_empty(), "{}", script.name);
            assert!(script.ticks > 0, "{}", script.name);
            assert_eq!(find_script(script.name).map(|found| found.name), Some(script.name));
            assert_eq!(SCRIPTS.iter().filter(|other| other.name == script.name).count(), 1);
        }
        assert!(find_script("no-such-script").is_none());
    }

    #[test]
    fn closures_are_sources() {
        fn take(source: &mut dyn ActionSource) -> TickActions {
            source.actions(3, &[])
        }
        let mut calls = 0;
        let mut source = |tick: u64, _cast: &[ActorId]| {
            calls += 1;
            assert_eq!(tick, 3);
            TickActions::NEUTRAL
        };
        assert_eq!(take(&mut source), TickActions::NEUTRAL);
        assert_eq!(calls, 1);
        assert_eq!(take(&mut idle), TickActions::NEUTRAL);
    }
}
