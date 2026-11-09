//! Where a recording's actions come from: [`ActionSource`], and [`SCRIPTS`], the sources written out
//! as code.
//!
//! The app's real source is its input bindings. Most of the suite comes from scripts instead, which
//! need no one to play them: **to add one,** write a function and add a row to [`SCRIPTS`], then
//! record it with `cargo xtask replay --record`.

use crate::actions::{ActorActions, Buttons, Stick, TickActions};
use crate::actors::ActorId;
use crate::timeline::{TimelineCommand, TimelineId};

use super::ward::{COURT_GATE_ID, COURT_GATE_TICK};

/// The ward's court scripts' length: the gate, and the fight after it to the warrior's defeat or near it.
const WARD_COURT_TICKS: u32 = 1200;

/// What the `timelines` scenario loads, in order.
pub const CINEMATIC: TimelineId = TimelineId::from_raw(0);
pub const CUTSCENE: TimelineId = TimelineId::from_raw(1);

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
    /// What it does with timelines, each with the tick it comes before, in order.
    pub commands: &'static [(u32, TimelineCommand)],
}

/// Every script.
pub const SCRIPTS: &[Script] = &[
    Script {
        name: "melee-sweep",
        about: "every actor presses every button on its own rhythm while its stick sweeps: no tick repeats",
        ticks: 1200,
        actions: melee_sweep,
        commands: &[],
    },
    Script {
        name: "melee-play",
        about: "the first actor walks in legs and attacks in bursts, as a person would, the rest stand",
        ticks: 3600,
        actions: melee_play,
        commands: &[],
    },
    Script {
        name: "string",
        about: "the three-hit string, each hit buffered ahead of its cancel window, over and over",
        ticks: 600,
        actions: string,
        commands: &[],
    },
    Script {
        name: "dodge-dash",
        about: "a string cut by a dodge, dash attacks out of dodges, an instant dodge and a skill, at an \
                opponent that moves",
        ticks: 720,
        actions: dodge_dash,
        commands: &[],
    },
    Script {
        name: "fight",
        about: "two fighters close and trade: strings, heavies through armour, a knockdown, a poise break \
                and a defeat with its finish freeze",
        ticks: 1500,
        actions: fight,
        commands: &[],
    },
    Script {
        name: "dodge-counter",
        about: "perfect dodges of an opponent's swings, answered, out of a dodge cancel and left unanswered, \
                with a dodge too late and one through its i-frames alone",
        ticks: 1000,
        actions: dodge_counter,
        commands: &[],
    },
    Script {
        name: "cancel-windows",
        about: "the test content's cancel windows no other script reaches: a skill out of the second hit, a \
                string started over from the third, a dodge out of a spin and out of a knockdown",
        ticks: 900,
        actions: cancel_windows,
        commands: &[],
    },
    Script {
        name: "attempts",
        about: "the player presses light twice a second and does nothing else: the next attempts of someone \
                tuning the move it starts",
        ticks: 600,
        actions: attempts,
        commands: &[],
    },
    Script {
        name: "laps",
        about: "the player runs up and down the square and does nothing else: the next steps of someone \
                tuning how it moves",
        ticks: 600,
        actions: laps,
        commands: &[],
    },
    Script {
        name: "fight-back",
        about: "the arena's player against the warrior, with no eyes for it: the string, a dodge and a dash \
                attack, over and over, which the warps aim at whatever's in reach",
        ticks: 1700,
        actions: fight_back,
        commands: &[],
    },
    Script {
        name: "party-swap",
        about: "the party against the warrior: a swap through a freeze, some refused by the cooldown or by nobody \
                being ready, assists landing, and then nothing, while the warrior fells each in turn, each \
                defeat forcing the next in, until nobody's left",
        ticks: PARTY_SWAP_TICKS,
        actions: party_swap,
        commands: &[],
    },
    Script {
        name: "idle",
        about: "nobody touches anything: the smallest a replay can be",
        ticks: 3600,
        actions: idle,
        commands: &[],
    },
    Script {
        name: "into-the-yard",
        about: "the proving ground's player runs round the pillar and into the cart yard, whose trigger \
                starts its encounter, then does as `fight-back` does",
        ticks: 2400,
        actions: into_the_yard,
        commands: &[],
    },
    Script {
        name: "arena-walk",
        about: "the arena's player walks and runs its clear floor, into a lantern post, jumps there, and comes \
                back, by the stick alone",
        ticks: ARENA_WALK_TICKS,
        actions: arena_walk,
        commands: &[],
    },
    Script {
        name: "cinematic-watched",
        about: "`melee-sweep`'s presses, with a cinematic run under them from tick 120 and pressed all the way through",
        ticks: 600,
        actions: melee_sweep,
        commands: &[(120, TimelineCommand::Play(CINEMATIC))],
    },
    Script {
        name: "cinematic-skipped",
        about: "`cinematic-watched`, with the cinematic skipped 50 steps in: it must reach all the same checksums",
        ticks: 600,
        actions: melee_sweep,
        commands: &[(120, TimelineCommand::Play(CINEMATIC)), (170, TimelineCommand::Skip { at_ms: 833 })],
    },
    Script {
        name: "cutscene-watched",
        about: "`melee-sweep`'s presses, with a cutscene between ticks 120 and 121, watched to its end",
        ticks: 300,
        actions: melee_sweep,
        commands: &[(120, TimelineCommand::Play(CUTSCENE))],
    },
    Script {
        name: "cutscene-skipped",
        about: "`cutscene-watched`, skipped 2.5 s in: it must reach all the same checksums",
        ticks: 300,
        actions: melee_sweep,
        commands: &[(120, TimelineCommand::Play(CUTSCENE)), (120, TimelineCommand::Skip { at_ms: 2500 })],
    },
    Script {
        name: "ward-court",
        about: "`fight-back` in the Lantern Ward's court with no cutscene: what the watched and skipped gate \
                scripts must match up to the cutscene",
        ticks: WARD_COURT_TICKS,
        actions: fight_back,
        commands: &[],
    },
    Script {
        name: "ward-court-watched",
        about: "`ward-court`, with the court's gate cutscene before its step 30, watched to its end",
        ticks: WARD_COURT_TICKS,
        actions: fight_back,
        commands: &[(COURT_GATE_TICK, TimelineCommand::Play(COURT_GATE_ID))],
    },
    Script {
        name: "ward-court-skipped",
        about: "`ward-court-watched`, skipped 3.1 s in: it must reach all the same checksums",
        ticks: WARD_COURT_TICKS,
        actions: fight_back,
        commands: &[
            (COURT_GATE_TICK, TimelineCommand::Play(COURT_GATE_ID)),
            (COURT_GATE_TICK, TimelineCommand::Skip { at_ms: 3100 }),
        ],
    },
    Script {
        name: "arena-ultimate",
        about: "the arena's player waits for the warrior to come into reach and presses the ultimate, whose \
                cinematic holds the warrior while the chop lands, then fights back as `fight-back` does",
        ticks: ARENA_ULTIMATE_TICKS,
        actions: arena_ultimate,
        commands: &[],
    },
    Script {
        name: "arena-energy",
        about: "`arena-ultimate`, then the ultimate pressed again every third of a second while the fight \
                fills its energy back up: it fires once the meter is full, and not before",
        ticks: ARENA_ENERGY_TICKS,
        actions: arena_energy,
        commands: &[],
    },
    Script {
        name: "chase-laps",
        about: "the arena's player runs a lap of the square's clear floor by the stick, and stands, for \
                whoever's following a path to it",
        ticks: CHASE_LAPS_TICKS,
        actions: chase_laps,
        commands: &[],
    },
];

/// The script named `name`.
#[must_use]
pub fn find_script(name: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|script| script.name == name)
}

/// How often [`attempts`] presses, in ticks.
pub const ATTEMPT_EVERY: u64 = 30;

/// [`SCRIPTS`]' `attempts`: the player's light attack on a fixed beat, with nothing else going on, which
/// is how live tuning's tests and its measurement find the next attempt.
#[must_use]
pub fn attempts(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let buttons = if tick.is_multiple_of(ATTEMPT_EVERY) { Buttons::LIGHT } else { Buttons::NONE };
    actions.set(player, ActorActions { buttons, stick: Stick::CENTER }).expect("one actor fits a tick");
    actions
}

/// How long `laps` runs each way: 12 m at today's 6 m/s.
const LAP_TICKS: u64 = 120;

/// [`SCRIPTS`]' `laps`: the player at full tilt, turning back on a fixed beat, which is how a measurement
/// finds someone running when a saved `movement` arrives.
#[must_use]
pub fn laps(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let y = if (tick / LAP_TICKS).is_multiple_of(2) { -127 } else { 127 };
    actions
        .set(player, ActorActions { buttons: Buttons::NONE, stick: Stick::new(0, y) })
        .expect("one actor fits a tick");
    actions
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

/// Ticks of one round of [`dodge_counter`]: long enough for whatever a round starts to finish, slow motion
/// included, and for the two to walk back into each other, which is what every round starts from.
const DODGE_COUNTER_ROUND: u64 = 200;

/// Ticks of [`dodge_counter`] for the opponent: one `light_1` a round, whose blade is live on the 5th to
/// 7th ticks after each press, and reaches the player from the 6th.
const DODGE_COUNTER_OPPONENT: [(u64, Buttons); 5] = [
    (50, Buttons::LIGHT),
    (250, Buttons::LIGHT),
    (450, Buttons::LIGHT),
    (650, Buttons::LIGHT),
    (850, Buttons::LIGHT),
];

/// Ticks of [`dodge_counter`] for the player, by what the test content's moves allow.
const DODGE_COUNTER_PLAYER: [(u64, Buttons); 9] = [
    (53, Buttons::DODGE),  // The blade reaches it on tick 56, the dodge's tick 3: perfect.
    (58, Buttons::LIGHT),  // Inside the counter window: the counter at once.
    (256, Buttons::DODGE), // Too late: tick 0 has no i-frames, and it lands.
    (453, Buttons::DODGE), // Perfect again,
    (501, Buttons::LIGHT), // with the answer a tick after its window closes: a plain attack.
    (651, Buttons::LIGHT), // A swing of the player's own,
    (653, Buttons::DODGE), // dodge-cancelled as its window opens, perfect on its tick 2,
    (658, Buttons::LIGHT), // and answered.
    (848, Buttons::DODGE), // Too early: the blade goes through its tick 7 and on, i-frames and nothing more.
];

/// [`SCRIPTS`]' `dodge-counter`: pillar 4's rhythm, on the two standing against each other. Both walk at
/// each other at the start of each round, until their capsules meet, whatever the last round did to
/// where they stand.
#[must_use]
pub fn dodge_counter(tick: u64, cast: &[ActorId]) -> TickActions {
    let walking = tick % DODGE_COUNTER_ROUND <= 45;
    let sticks = if walking { [Stick::new(0, 127), Stick::new(0, -127)] } else { [Stick::new(0, 0); 2] };
    facing_off(tick, cast, sticks, &DODGE_COUNTER_PLAYER, &DODGE_COUNTER_OPPONENT)
}

/// Ticks of [`cancel_windows`] for the player. A press ahead of a window a hit's hitstop has pushed back
/// buffers until it opens, so each stays inside its window with the opponent in reach or not.
const CANCEL_WINDOWS_PLAYER: [(u64, Buttons); 10] = [
    (137, Buttons::DODGE), // `knockdown`, from the heavy's hit on tick 64, at its tick 62 after 10 of hitstop.
    (300, Buttons::LIGHT),
    (310, Buttons::LIGHT),
    (323, Buttons::SKILL), // `light_2` from tick 310, by its tick 13.
    (500, Buttons::LIGHT),
    (510, Buttons::LIGHT),
    (522, Buttons::LIGHT),
    (558, Buttons::LIGHT), // `light_3` from about tick 522, by its tick 36.
    (700, Buttons::ULTIMATE),
    (724, Buttons::DODGE), // `spin` from tick 700, at its tick 24.
];

/// Ticks of [`cancel_windows`] for the opponent: the heavy that knocks the player down.
const CANCEL_WINDOWS_OPPONENT: [(u64, Buttons); 1] = [(50, Buttons::HEAVY)];

/// [`SCRIPTS`]' `cancel-windows`: the rest of pillar 4's cancel windows, held by `tests/dodges.rs`. The two
/// walk into reach for the knockdown, and where the rest happen doesn't matter.
#[must_use]
pub fn cancel_windows(tick: u64, cast: &[ActorId]) -> TickActions {
    let sticks = if tick <= 45 { [Stick::new(0, 127), Stick::new(0, -127)] } else { [Stick::new(0, 0); 2] };
    facing_off(tick, cast, sticks, &CANCEL_WINDOWS_PLAYER, &CANCEL_WINDOWS_OPPONENT)
}

/// Ticks of [`party_swap`]: until a little after the last of the party has fallen.
const PARTY_SWAP_TICKS: u32 = 5300;

/// What [`party_swap`] presses, for the party, whose input names its first member throughout. After these
/// everyone just stands there, and the warrior gets through all three, a fall forcing each next one in.
const PARTY_SWAP: [(u64, Buttons); 6] = [
    (72, Buttons::LIGHT), // `light_1`, which reaches the warrior on tick 82 and freezes the first member for 3.
    (83, Buttons::SWAP),  // Pressed through that freeze: it waits, and the second comes in on tick 86.
    (116, Buttons::SWAP), // Far too soon after that: it runs out unused.
    (200, Buttons::SWAP), // Out of the knockdown the warrior's overhead gave the second: the third comes in.
    (290, Buttons::SWAP), // The third has stayed long enough, and the first has rested: back to it.
    (330, Buttons::LIGHT),
];

/// [`SCRIPTS`]' `party-swap`: the `party` scenario's fight, by [`PARTY_SWAP`].
#[must_use]
pub fn party_swap(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&party) = cast.first() else { return actions };
    let buttons = on_schedule(tick, u64::MAX, &PARTY_SWAP);
    actions.set(party, ActorActions { buttons, stick: Stick::CENTER }).expect("one actor fits a tick");
    actions
}

/// The two of a `duel` on their schedules, holding these sticks.
fn facing_off(
    tick: u64,
    cast: &[ActorId],
    sticks: [Stick; 2],
    player: &[(u64, Buttons)],
    opponent: &[(u64, Buttons)],
) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let schedules = [player, opponent];
    for ((&actor, stick), schedule) in cast.iter().zip(sticks).zip(schedules) {
        let buttons = on_schedule(tick, u64::MAX, schedule);
        actions.set(actor, ActorActions { buttons, stick }).expect("two actors fit a tick");
    }
    actions
}

/// One round of [`fight_back`]: a dodge from nothing, and a light press into its window, which plays
/// the dash attack, whose warp finds the warrior. Its i-frames take most of what comes.
const FIGHT_BACK: [(u64, Buttons); 5] = [
    (0, Buttons::LIGHT),
    (16, Buttons::LIGHT),
    (32, Buttons::LIGHT),
    (84, Buttons::DODGE),
    (96, Buttons::LIGHT),
];

/// Ticks of [`fight_back`]'s round.
const FIGHT_BACK_ROUND: u64 = 150;

/// [`SCRIPTS`]' `fight-back`: a script reads a tick and nothing else, so it can't aim its stick at
/// anyone. This one doesn't need to: the warrior comes to it, and the cuts warp. On the scenario's seed
/// this gets through its health with about half the player's left. A dodge on its own schedule, with
/// no eyes for the telegraphs, takes the overheads it would have dodged, which is what makes it a fight.
#[must_use]
pub fn fight_back(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let buttons = on_schedule(tick, FIGHT_BACK_ROUND, &FIGHT_BACK);
    actions.set(player, ActorActions { buttons, stick: Stick::CENTER }).expect("one actor fits a tick");
    actions
}

/// The tick [`arena_ultimate`] presses the ultimate: the warrior has come to 2.35 m, inside the chop's
/// bound and warp, and hasn't yet started its overhead.
pub const ARENA_ULTIMATE_AT: u64 = 60;

/// When [`arena_ultimate`] takes up [`fight_back`]: once the ultimate's 144 ticks are over.
const ARENA_ULTIMATE_FIGHTS_AT: u64 = ARENA_ULTIMATE_AT + 150;

/// [`arena_ultimate`]'s length: the ultimate, and a round and a half of the fight after it.
const ARENA_ULTIMATE_TICKS: u32 = 450;

/// [`SCRIPTS`]' `arena-ultimate` (M3 Q3b): the player stands until the warrior is in reach, presses the
/// ultimate, and once it's over fights back on [`fight_back`]'s schedule. Its presses under the cinematic
/// would be held, so it makes none.
#[must_use]
pub fn arena_ultimate(tick: u64, cast: &[ActorId]) -> TickActions {
    if tick >= ARENA_ULTIMATE_FIGHTS_AT {
        return fight_back(tick - ARENA_ULTIMATE_FIGHTS_AT, cast);
    }
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let buttons = if tick == ARENA_ULTIMATE_AT { Buttons::ULTIMATE } else { Buttons::NONE };
    actions.set(player, ActorActions { buttons, stick: Stick::CENTER }).expect("one actor fits a tick");
    actions
}

/// [`into_the_yard`]'s legs from the proving ground's start at (1, 1): a run along +Z, past the pillar at
/// (2, 2), then along +X until it's well inside the cart yard, whose box starts at x = 5.
const INTO_THE_YARD_LEGS: [(i8, i8, u64); 2] = [(0, 127, 90), (127, 0, 70)];

/// [`SCRIPTS`]' `into-the-yard`.
#[must_use]
pub fn into_the_yard(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut ends_at = 0;
    for (x, y, ticks) in INTO_THE_YARD_LEGS {
        ends_at += ticks;
        if tick < ends_at {
            let mut actions = TickActions::NEUTRAL;
            let Some(&player) = cast.first() else { return actions };
            let stick = Stick::new(x, y);
            actions
                .set(player, ActorActions { buttons: Buttons::NONE, stick })
                .expect("one actor fits a tick");
            return actions;
        }
    }
    fight_back(tick - ends_at, cast)
}

/// How often [`arena_energy`] presses the ultimate again, in ticks, once it's fighting back.
pub const ARENA_ENERGY_TRIES_EVERY: u64 = 20;

/// [`arena_energy`]'s length: long enough for the fight to fill the meter and the ultimate to play again.
pub const ARENA_ENERGY_TICKS: u32 = 900;

/// [`SCRIPTS`]' `arena-energy` (M3 Q3c): [`arena_ultimate`], whose ultimate empties the player's energy, and
/// then a press of the ultimate every [`ARENA_ENERGY_TRIES_EVERY`] ticks on top of [`fight_back`]. Each press
/// before the hits have filled the meter again fires nothing; the first one after that finds the player
/// between moves does, about a second after it filled.
#[must_use]
pub fn arena_energy(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = arena_ultimate(tick, cast);
    let Some(&player) = cast.first() else { return actions };
    if tick >= ARENA_ULTIMATE_FIGHTS_AT
        && (tick - ARENA_ULTIMATE_FIGHTS_AT).is_multiple_of(ARENA_ENERGY_TRIES_EVERY)
    {
        let mut held = actions.get(player);
        held.buttons = held.buttons | Buttons::ULTIMATE;
        actions.set(player, held).expect("one actor fits a tick");
    }
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

/// [`chase_laps`]' legs, as [`ARENA_WALK_LEGS`]' are, all at a run of 6 m/s, from the player's spawn point:
/// out to the square's north side, along it, down its east side, back along its south and up its west.
const CHASE_LAPS_LEGS: [(i8, i8, u32); 6] =
    [(0, 127, 70), (127, 0, 108), (0, -127, 250), (-127, 0, 240), (0, 127, 260), (127, 0, 170)];

/// [`chase_laps`]' length: its legs, and ten seconds for everyone to arrive.
const CHASE_LAPS_TICKS: u32 = {
    let mut ticks = 600;
    let mut leg = 0;
    while leg < CHASE_LAPS_LEGS.len() {
        ticks += CHASE_LAPS_LEGS[leg].2;
        leg += 1;
    }
    ticks
};

/// [`SCRIPTS`]' `chase-laps`: the player by the stick, round the square.
#[must_use]
pub fn chase_laps(tick: u64, cast: &[ActorId]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    let Some(&player) = cast.first() else { return actions };
    let mut left = tick;
    for (x, y, ticks) in CHASE_LAPS_LEGS {
        if left < u64::from(ticks) {
            let stick = Stick::new(x, y);
            actions
                .set(player, ActorActions { buttons: Buttons::NONE, stick })
                .expect("one actor fits a tick");
            break;
        }
        left -= u64::from(ticks);
    }
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
