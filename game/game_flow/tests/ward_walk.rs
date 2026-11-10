//! The whole walk (M3 G1c, exit tests 1 and 7), with no GPU: the game's flow over the Lantern Ward from a new
//! game at the title, along the ward's `tour` to the market, the garden and the court, the court's gate
//! cutscene watched, the guide's conversation, the court's fight and the terrace's boss, to the flow's victory.
//!
//! `records_the_whole_walk` plays it as a player who reads the simulation before each step ([`Walker`]):
//! along the tour's points, and at whoever's nearest while an encounter is running, dodging each kind of
//! swing once it has seen how long its wind-up is. Nothing about the fight is tuned for it. It prints what
//! hit the player, so a walk that loses says what to. With `SAPPHIRE_WALK_OUT=<file>` it writes the run's
//! replay there: that's how `tests/replays/ward-whole-walk.sreplay` was made, and how it's made again when the
//! fight changes under it (then `cargo xtask replay --update --replay ward-whole-walk`).
//! `the_whole_walk_replays_to_the_win` plays the committed file through the same flow.
//!
//! Installs `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use game_combat::combat::{Combat, CombatOption};
use game_combat::dialogue::DialogueRunner;
use game_flow::document::FlowDocument;
use game_flow::flow::{FlowCommand, FlowState};
use game_flow::game::{FlowContent, Game, GameSetup};
use sapphire_core::Name;
use sapphire_core::alloc::{CountingAllocator, PER_THREAD_COUNTS};
use sapphire_dialogue::FlagRegistry;
use sapphire_gameplay::{ActorActions, ActorId, Buttons, EncounterPhase, Simulation, Stick, TickActions};
use sapphire_moves::Phase;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Longer than the walk could take: 15 minutes of play.
const MOST_TICKS: u64 = 15 * 60 * 60;
/// Steps the recording goes on past the victory, so its file holds the step that won.
const PAST_THE_WIN: u32 = 60;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn text(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn game(setup: GameSetup) -> Game {
    let registry = FlagRegistry::from_ron(&text("content/flags.ron")).expect("the flags read");
    let registered = registry.flags.iter().map(|flag| Name::new(flag)).collect();
    let document = FlowDocument::from_ron(&text("content/game.flow.ron")).expect("the game's flow reads");
    Game::new(FlowContent { document, registered }, setup).expect("the game has what its flow names")
}

/// A directory of the test's own, gone when it is.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("game_flow-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory of its own");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The ward's `tour` spline's points, on the ground (x, z): from the start east to the market's far end and
/// back, west to the garden's and back, and south through the court and up the terrace's steps.
const TOUR: [[f32; 2]; 24] = [
    [0.0, 14.0],
    [0.0, 6.0],
    [8.0, 3.0],
    [20.0, 0.0],
    [46.0, 0.0],
    [64.0, 0.0],
    [64.0, 8.0],
    [72.0, 0.0],
    [46.0, 0.0],
    [20.0, 0.0],
    [8.0, 3.0],
    [-8.0, 3.0],
    [-20.0, 0.0],
    [-46.0, 0.0],
    [-64.0, 0.0],
    [-64.0, 8.0],
    [-46.0, 0.0],
    [-20.0, 0.0],
    [-8.0, -6.0],
    [0.0, -18.0],
    [0.0, -60.0],
    [0.0, -96.5],
    [0.0, -100.5],
    [0.0, -105.0],
];

/// Close enough to a point of the tour to go on to the next.
const REACHED: f32 = 1.5;
/// Close enough to a foe to fight it where it stands, rather than run at it: the blade's reach and a cut's
/// warp.
const IN_REACH: f32 = 2.2;
/// Close enough to a foe's swing to dodge it: the Lancer's vault comes from this far.
const IN_DANGER: f32 = 6.0;
/// How far ahead of a foe's strike the dodge goes, inside its 13 ticks out of reach and the perfect
/// window's first 6.
const DODGE_LEAD: u64 = 4;
/// A swing never seen before is dodged from this tick of its wind-up...
const UNSEEN_FIRST: u64 = 18;
/// ...and again each time a dodge's 13 ticks out of reach run out, until it strikes.
const UNSEEN_EVERY: u64 = 13;

/// What the walk is doing, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Doing {
    Walking(usize),
    Fighting,
    Waiting,
}

/// A foe's move as the walker last saw it.
#[derive(Clone, Copy, Debug)]
struct Seen {
    serial: u32,
    /// The tick it started on.
    started: u64,
    /// Whether its startup's length has been taken down.
    timed: bool,
    /// Whether the walker has dodged it.
    dodged: bool,
}

/// A player who reads the simulation before each step: along the tour, and at whoever's nearest while a
/// fight's on, as someone at the keys would:
///
/// - **Moves:** the three-hit string, each cut pressed as the next one's window opens, since a dodge can
///   cut any of them short; the ultimate whenever its meter's full; a dodge out of a reaction.
/// - **Dodges:** each kind of swing is watched once, for how long its wind-up is, and from then on dodged a
///   few ticks before it strikes, with the dash attack out of the dodge.
///
/// What it learns is a function of the run so far, so the same run teaches it the same things.
#[derive(Debug, Default)]
struct Walker {
    /// The tour's point it's walking to.
    next: usize,
    /// Each foe's move, by actor.
    seen: Vec<(ActorId, Seen)>,
    /// How long each move's startup ran, by the move's name, as first seen.
    startups: Vec<(Name, u64)>,
    /// The player's own move: its serial and the tick it started on.
    mine: (u32, u64),
}

fn stick_toward(from: [f32; 2], to: [f32; 2]) -> Stick {
    let (dx, dz) = (to[0] - from[0], to[1] - from[1]);
    let length = (dx * dx + dz * dz).sqrt();
    if length < 1e-3 {
        return Stick::CENTER;
    }
    let scale = 127.0 / length;
    Stick::new((dx * scale).round() as i8, (dz * scale).round() as i8)
}

fn ground(position: sapphire_math::Vec3) -> [f32; 2] {
    [position.x, position.z]
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Walker {
    /// Takes down each foe's move as it stands, and says whether one near enough is about to strike.
    fn watch(&mut self, sim: &Simulation, at: [f32; 2], faction: sapphire_gameplay::Faction) -> bool {
        let tick = sim.tick();
        let mut strike = false;
        for foe in sim.actors().filter(|actor| !actor.off_field && actor.fighter.is_foe_of(faction)) {
            let Some(current) = foe.moves.current else { continue };
            let place = match self.seen.iter().position(|&(id, _)| id == foe.id) {
                Some(place) => place,
                None => {
                    let fresh = Seen { serial: 0, started: tick, timed: false, dodged: false };
                    self.seen.push((foe.id, fresh));
                    self.seen.len() - 1
                }
            };
            let seen = &mut self.seen[place].1;
            if seen.serial != current.serial {
                *seen = Seen { serial: current.serial, started: tick, timed: false, dodged: false };
            }
            let since = tick - seen.started;
            if current.phase != Phase::Startup && !seen.timed {
                seen.timed = true;
                if !self.startups.iter().any(|&(name, _)| name == current.name) {
                    self.startups.push((current.name, since));
                }
            }
            let near = distance(at, ground(foe.position)) < IN_DANGER;
            if near && foe.brain.is_attacking() && current.phase == Phase::Startup && !seen.dodged {
                match self.startups.iter().find(|&&(name, _)| name == current.name) {
                    Some(&(_, startup)) => {
                        if since + DODGE_LEAD >= startup {
                            seen.dodged = true;
                            strike = true;
                        }
                    }
                    // A swing not seen before: out of reach through its wind-up from its 18th tick, a dodge
                    // at a time, as each one's 13 ticks end.
                    None => {
                        strike |= since >= UNSEEN_FIRST && (since - UNSEEN_FIRST).is_multiple_of(UNSEEN_EVERY)
                    }
                }
            }
        }
        strike
    }

    /// The button for the player's next move, from the one it's in.
    fn next_move(&mut self, sim: &Simulation, me: &sapphire_gameplay::ActorView) -> Buttons {
        let tick = sim.tick();
        let Some(current) = me.moves.current else { return Buttons::LIGHT };
        if current.serial != self.mine.0 {
            self.mine = (current.serial, tick);
        }
        let since = tick - self.mine.1;
        // Every other tick, so each is a press of its own.
        if !tick.is_multiple_of(2) {
            return Buttons::NONE;
        }
        match current.name {
            name if name == Name::new("light_1") && since >= 10 => Buttons::LIGHT,
            name if name == Name::new("light_2") && since >= 10 => Buttons::LIGHT,
            name if name == Name::new("light_3") && since >= 40 => Buttons::LIGHT,
            name if name == Name::new("dodge") && since >= 4 => Buttons::LIGHT,
            name if name == Name::new("dash_attack") && since >= 14 => Buttons::LIGHT,
            name if [Name::new("flinch"), Name::new("stagger"), Name::new("knockdown")].contains(&name) => {
                Buttons::DODGE
            }
            _ => Buttons::NONE,
        }
    }

    /// This tick's actions for `player`, from the simulation as it stands.
    fn actions(&mut self, sim: &Simulation, player: ActorId) -> (TickActions, Doing) {
        let mut actions = TickActions::NEUTRAL;
        let on_field = sim.party().on_field(player);
        let Some(me) = sim.actor(on_field) else { return (actions, Doing::Waiting) };
        let at = ground(me.position);
        let faction = me.fighter.faction();
        let fighting = sim
            .zone()
            .is_some_and(|zone| zone.encounters().iter().any(|held| held.phase() == EncounterPhase::Running));
        let strike = self.watch(sim, at, faction);
        let nearest = sim
            .actors()
            .filter(|actor| !actor.off_field && actor.fighter.is_foe_of(faction))
            .map(|actor| (distance(at, ground(actor.position)), actor))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let (buttons, stick, doing) = match nearest {
            Some((range, foe)) if fighting || range < 12.0 => {
                let ready = me.fighter.max_energy() > 0 && me.fighter.can_afford(me.fighter.max_energy());
                if strike {
                    (Buttons::DODGE, Stick::CENTER, Doing::Fighting)
                } else if range > IN_REACH {
                    (Buttons::NONE, stick_toward(at, ground(foe.position)), Doing::Fighting)
                } else if ready && sim.tick().is_multiple_of(2) {
                    (Buttons::ULTIMATE, Stick::CENTER, Doing::Fighting)
                } else {
                    (self.next_move(sim, &me), Stick::CENTER, Doing::Fighting)
                }
            }
            _ if fighting => (Buttons::NONE, Stick::CENTER, Doing::Waiting),
            _ => {
                while self.next + 1 < TOUR.len() && distance(at, TOUR[self.next]) < REACHED {
                    self.next += 1;
                }
                let stick = if distance(at, TOUR[self.next]) < REACHED {
                    Stick::CENTER
                } else {
                    stick_toward(at, TOUR[self.next])
                };
                (Buttons::NONE, stick, Doing::Walking(self.next))
            }
        };
        actions.set(player, ActorActions { buttons, stick }).expect("one actor fits a tick");
        (actions, doing)
    }
}

fn health(sim: &Simulation, player: ActorId) -> u32 {
    sim.actor(sim.party().on_field(player)).map_or(0, |me| me.fighter.health())
}

/// What hit the player, by the move of the nearest foe when it did, for the log: what a walk that loses
/// lost to.
#[derive(Debug, Default)]
struct Hits(Vec<(Name, u32, u32)>);

impl Hits {
    fn take_down(&mut self, sim: &Simulation, player: ActorId, before: u32) {
        let now = health(sim, player);
        let Some(me) = sim.actor(sim.party().on_field(player)).filter(|_| now < before) else { return };
        let at = ground(me.position);
        let by = sim
            .actors()
            .filter(|actor| !actor.off_field && actor.fighter.is_foe_of(me.fighter.faction()))
            .filter_map(|actor| Some((distance(at, ground(actor.position)), actor.moves.current?.name)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(Name::new("nobody"), |(_, name)| name);
        match self.0.iter_mut().find(|(name, ..)| *name == by) {
            Some((_, count, damage)) => {
                *count += 1;
                *damage += before - now;
            }
            None => self.0.push((by, 1, before - now)),
        }
    }
}

impl std::fmt::Display for Hits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, (name, count, damage)) in self.0.iter().enumerate() {
            let comma = if index == 0 { "" } else { ", " };
            write!(f, "{comma}{name:?} {count}x for {damage}")?;
        }
        Ok(())
    }
}

/// Walks an open conversation to its end, taking the first answer wherever there's a choice.
fn talk(runner: &mut DialogueRunner, combat: &mut Combat) {
    for _ in 0..32 {
        if !runner.is_open() {
            return;
        }
        if !runner.next(combat).expect("goes on") {
            assert!(runner.choose(0, combat).expect("answers"), "a line with no way on");
        }
    }
    panic!("the conversation never ended");
}

fn describe(sim: &Simulation, player: ActorId) -> String {
    let on_field = sim.party().on_field(player);
    let me = sim.actor(on_field).expect("the player stands");
    let phases: Vec<char> = sim.zone().map_or(Vec::new(), |zone| {
        zone.encounters()
            .iter()
            .map(|held| match held.phase() {
                EncounterPhase::Waiting => '.',
                EncounterPhase::Running => 'R',
                EncounterPhase::Cleared => 'C',
            })
            .collect()
    });
    let foes = sim
        .actors()
        .filter(|actor| !actor.off_field && actor.fighter.is_foe_of(me.fighter.faction()))
        .map(|actor| format!("{}/{}", actor.fighter.health(), actor.fighter.max_health()))
        .collect::<Vec<_>>();
    format!(
        "tick {} at ({:.1}, {:.1}, {:.1}) health {}/{} energy {} encounters {} foes {foes:?}",
        sim.tick(),
        me.position.x,
        me.position.y,
        me.position.z,
        me.fighter.health(),
        me.fighter.max_health(),
        me.fighter.energy(),
        phases.iter().collect::<String>(),
    )
}

#[test]
#[ignore = "records the whole walk with a reading player, a few seconds: `cargo test -p game_flow --test ward_walk -- --ignored`"]
fn records_the_whole_walk() {
    let dir = Scratch::new("ward-walk");
    let mut game = game(GameSetup { record: Some(dir.0.clone()), ..GameSetup::default() });
    let option = game.first_run();
    let CombatOption::Run { keep: Some(kept), .. } = &option else { panic!("a kept run: {option:?}") };
    let kept = kept.clone();
    let mut combat = Combat::new(&option).expect("the ward's run stands");
    assert_eq!(game.command(FlowCommand::NewGame), None);
    let mut runner = DialogueRunner::new(vec![Arc::new(sapphire_script::scenarios::ward_guide())]);
    let mut walker = Walker::default();
    let player = combat.cast()[0];
    let mut doing = Doing::Waiting;
    let mut past_the_win = 0;
    let mut hits = Hits::default();
    loop {
        let state = game.flow().state();
        let sim = combat.simulation();
        assert!(sim.tick() < MOST_TICKS, "the walk never ended: {}", describe(sim, player));
        match state {
            FlowState::Victory { .. } => {
                past_the_win += 1;
                if past_the_win > PAST_THE_WIN {
                    break;
                }
            }
            FlowState::Defeat { .. } => panic!("the player fell: {}; hit by {hits}", describe(sim, player)),
            _ => {}
        }
        assert!(game.flow().runs_steps(), "the flow holds its steps in {state:?}");
        let (actions, now) = if matches!(state, FlowState::Playing) {
            walker.actions(sim, player)
        } else {
            (TickActions::NEUTRAL, Doing::Waiting)
        };
        if now != doing || sim.tick().is_multiple_of(600) {
            eprintln!("{now:?} ({state:?}): {}", describe(sim, player));
            doing = now;
        }
        let before = health(sim, player);
        combat.step(&mut |_: u64, _: &[ActorId]| actions);
        hits.take_down(combat.simulation(), player, before);
        if let Some(asset) = game.after_step(&mut combat) {
            let id = game.dialogue_of(asset).expect("the run loads the beat's dialogue");
            runner.open(id, &mut combat).expect("the conversation opens");
            eprintln!("the guide's conversation: {}", describe(combat.simulation(), player));
            talk(&mut runner, &mut combat);
            assert!(runner.take_ended().is_some(), "it ended");
            game.command(FlowCommand::DialogueEnded);
        }
    }
    let sim = combat.simulation();
    eprintln!("won: {}; hit by {hits}; checksum {:016x}", describe(sim, player), sim.checksum());
    let stats = sim.zone().expect("the ward").stats();
    assert_eq!(stats.spawns_refused, 0, "every fighter came: {stats:?}");
    // The simulation's own steps: the run's recording grows its buffer between them, outside the fixed step.
    if PER_THREAD_COUNTS {
        assert_eq!(sim.stats().allocations, 0, "no step allocates");
    }
    combat.finish().expect("the run's file is kept");
    let bytes = std::fs::read(&kept).expect("the run's file");
    eprintln!("{} bytes in {}", bytes.len(), kept.display());
    if let Some(out) = std::env::var_os("SAPPHIRE_WALK_OUT") {
        std::fs::write(&out, &bytes).expect("written where it was asked for");
        eprintln!("wrote {}", Path::new(&out).display());
    }
}

#[test]
fn the_whole_walk_replays_to_the_win() {
    let replay = root().join("tests/replays/ward-whole-walk.sreplay");
    let mut game = game(GameSetup { replay: Some(replay), ..GameSetup::default() });
    let option = game.first_run();
    assert!(matches!(option, CombatOption::Replay { .. }), "the file plays in place of a run: {option:?}");
    let mut combat = Combat::new(&option).expect("the walk's file plays");
    assert_eq!(game.command(FlowCommand::NewGame), None);
    let player = combat.cast()[0];
    let mut neutral = |_: u64, _: &[ActorId]| TickActions::NEUTRAL;
    let (mut cutscenes, mut conversations, mut was_cutscene) = (0, 0, false);
    while !matches!(game.flow().state(), FlowState::Victory { .. }) {
        let sim = combat.simulation();
        assert!(combat.ticks_left() != Some(0), "the file ended short of the win: {}", describe(sim, player));
        assert!(!matches!(game.flow().state(), FlowState::Defeat { .. }), "fell: {}", describe(sim, player));
        assert!(game.flow().runs_steps(), "the flow holds its steps in {:?}", game.flow().state());
        combat.step(&mut neutral);
        if game.after_step(&mut combat).is_some() {
            conversations += 1;
            game.command(FlowCommand::DialogueEnded);
        }
        let cutscene = matches!(game.flow().state(), FlowState::Cutscene { .. });
        cutscenes += u32::from(cutscene && !was_cutscene);
        was_cutscene = cutscene;
    }
    let sim = combat.simulation();
    assert_eq!(cutscenes, 1, "the court's gate, watched");
    // The file applies the guide's answer right after the step the gate ended on, before the flow looks: the
    // beat's `unless met_guide` then passes the conversation by, and the choice stands as the file made it.
    assert_eq!(conversations, 0, "a replay opens no conversation");
    assert_eq!(sim.flags().get(Name::new("met_guide")), 1, "the conversation's choice, from the file");
    assert_eq!(sim.flags().get(Name::new("ward_lit")), 1, "its first answer lit the ward");
    let zone = sim.zone().expect("the ward");
    assert!(zone.encounters().iter().all(|held| held.phase() == EncounterPhase::Cleared), "all four cleared");
    assert_eq!(game.stats().refused, 0, "every beat's command was the file's");
    if PER_THREAD_COUNTS {
        assert_eq!(sim.stats().allocations, 0, "no step allocates");
    }
    // Played to its end, it reaches the checksums beside it: the flow changed nothing the file holds.
    while combat.ticks_left().is_some_and(|left| left > 0) {
        combat.step(&mut neutral);
    }
    combat.finish().expect("the walk reaches its checksums");
}
