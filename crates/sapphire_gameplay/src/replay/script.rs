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
        name: "fight",
        about: "two fighters close and trade: strings, heavies through armour, a knockdown, a poise break \
                and a defeat with its finish freeze",
        ticks: 1500,
        actions: fight,
    },
    Script {
        name: "idle",
        about: "nobody touches anything: the smallest a replay can be",
        ticks: 3600,
        actions: idle,
    },
    Script {
        name: "arena-walk",
        about: "the arena's player walks and runs its clear floor, into a lantern post, jumps there, and comes \
                back, by the stick alone",
        ticks: ARENA_WALK_TICKS,
        actions: arena_walk,
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

/// Ticks of one round of [`fight`].
const FIGHT_ROUND: u64 = 360;

/// The player's half of a round of [`fight`]: the string, pressed often enough that a press waits in
/// the buffer through whatever hitstop does to the windows, then a heavy that knocks down, a trade,
/// and a heavy whose armour takes the opponent's light.
const FIGHT_PLAYER: [(u64, Buttons); 9] = [
    (30, Buttons::LIGHT),
    (36, Buttons::LIGHT),
    (42, Buttons::LIGHT),
    (48, Buttons::LIGHT),
    (54, Buttons::LIGHT),
    (60, Buttons::LIGHT),
    (115, Buttons::HEAVY),
    (230, Buttons::LIGHT),
    (258, Buttons::HEAVY),
];

/// The opponent's half: the trade's press, on the player's tick, and a light into that heavy's armour.
const FIGHT_OPPONENT: [(u64, Buttons); 2] = [(230, Buttons::LIGHT), (262, Buttons::LIGHT)];

/// [`SCRIPTS`]' `fight`: both walk at each other at the start of a round and again before its trade,
/// since pushes part them, and fight until one of them falls. The other then keeps swinging at what's
/// left, which takes nothing.
#[must_use]
pub fn fight(tick: u64, cast: &[ActorId]) -> TickActions {
    let at = tick % FIGHT_ROUND;
    let walking = matches!(at, 0..=25 | 205..=225);
    let mut actions = TickActions::NEUTRAL;
    let mut drive = |actor: Option<&ActorId>, buttons: Buttons, forward: i8| {
        if let Some(&actor) = actor {
            let stick = Stick::new(0, if walking { forward } else { 0 });
            actions.set(actor, ActorActions { buttons, stick }).expect("two actors fit a tick");
        }
    };
    drive(cast.first(), on_schedule(tick, FIGHT_ROUND, &FIGHT_PLAYER), 127);
    drive(cast.get(1), on_schedule(tick, FIGHT_ROUND, &FIGHT_OPPONENT), -127);
    actions
}

/// [`SCRIPTS`]' `idle`.
#[must_use]
pub fn idle(_tick: u64, _cast: &[ActorId]) -> TickActions {
    TickActions::NEUTRAL
}

/// [`arena_walk`]'s legs: a stick held for so many ticks, in the world's axes (x along +X, y along
/// +Z). The second number's 127 is a run, 6 m/s by the default profile, and 64 a walk, 2 m/s. From the
/// player's spawn point (3.2, 5):
///
/// 1. A walk 2 m toward +X, past the line the enemy stands on.
/// 2. A run 7.3 m toward −Z, to level with the post of the lantern at (8, −2.5).
/// 3. A run toward +X, into that post, held after it stops the capsule. [`ARENA_WALK_JUMP`] jumps here,
///    still pushing.
/// 4. A run back 7.3 m toward +Z, with the post behind it.
/// 5. A run toward −X, to about where it started.
pub const ARENA_WALK_LEGS: [(i8, i8, u32); 5] =
    [(64, 0, 60), (0, -127, 73), (127, 0, 90), (0, 127, 73), (-127, 0, 42)];

/// The tick [`arena_walk`] presses jump, late in its third leg: at the post, in time to land.
pub const ARENA_WALK_JUMP: u64 = 165;

/// [`arena_walk`]'s length: its legs, and a second to stop.
const ARENA_WALK_TICKS: u32 = {
    let mut ticks = 60;
    let mut leg = 0;
    while leg < ARENA_WALK_LEGS.len() {
        ticks += ARENA_WALK_LEGS[leg].2;
        leg += 1;
    }
    ticks
};

/// [`SCRIPTS`]' `arena-walk`: the arena's player by the stick alone, and its opponent standing, for the
/// golden frame of a character the controller moved and for what its collision does on the way.
#[must_use]
pub fn arena_walk(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let mut left = tick;
    let mut stick = Stick::CENTER;
    for (x, y, ticks) in ARENA_WALK_LEGS {
        if left < u64::from(ticks) {
            stick = Stick::new(x, y);
            break;
        }
        left -= u64::from(ticks);
    }
    let buttons = if tick == ARENA_WALK_JUMP { Buttons::JUMP } else { Buttons::NONE };
    actions.set(player, ActorActions { buttons, stick }).expect("one actor fits a tick");
    actions
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
