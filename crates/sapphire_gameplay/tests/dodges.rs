//! M2 G4: dodges, perfect dodges and counters, through real steps. Ticks are worked out by hand in each
//! test's comments, from [`DODGES`]'s numbers.
//!
//! A press on step `k` reads its move's tick 0 on step `k`, so at 1× a move's tick `t` is step `k + t`,
//! and a window `a..b` counts on steps `k + a` to `k + b - 1`. The attacker stands at the origin facing +Z
//! and the dodger 1 m ahead: a blade 0.6 m in front of the attacker's root with a radius of 0.2 reaches
//! z = 0.8, and the dodger's body and its perfect-dodge volume, both spheres of 0.25 at its chest, reach
//! back to z = 0.75, so a live blade touches either at once, with no travel.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::PI;
use std::path::Path;

use sapphire_core::Name;
use sapphire_gameplay::replay::{self, Player, Replay, SCENARIOS};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, CueEvent, CurrentMove, PERFECT_DODGE, SimConfig, Simulation,
    Stick, TickActions, dodge_body,
};
use sapphire_math::Vec3;
use sapphire_moves::{MoveSet, MoveSetDocument};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::{ScaleSource, SimDuration, SimTime};

/// `roll` has i-frames on ticks 1 to 13 and a perfect-dodge window on ticks 1 to 6. A perfect dodge slows
/// the world to a fifth for 30 steps and opens a counter window of 20 ticks. `quiet` is the same dodge
/// with a window that's only a window. Each attack hurts by 10, and freezes nobody unless a test edits
/// it. `riposte` is untouchable until it has landed, which a counter wants to be: see
/// `a_counter_needs_i_frames_only_against_another_swing`.
const DODGES: &str = r#"(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [
            (name: "tap", button: Light, buffer: 8),
            (name: "sweep", button: Heavy, buffer: 8),
            (name: "roll", button: Dodge, buffer: 8, priority: 3),
            (name: "quiet", button: Skill, buffer: 8),
            (name: "answer", button: Ultimate, buffer: 8),
            (name: "sense", button: Swap, buffer: 8),
            (name: "beats", button: Jump, buffer: 8),
            (name: "selfish", button: LockOn, buffer: 8),
        ],
        neutral: [
            (intent: "tap", move: "tap"),
            (intent: "sweep", move: "sweep"),
            (intent: "roll", move: "roll"),
            (intent: "quiet", move: "quiet"),
            (intent: "sense", move: "sense"),
            (intent: "beats", move: "beats"),
            (intent: "selfish", move: "selfish"),
        ],
        fighter: (
            health: 100,
            hurtboxes: [(name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25))],
            reactions: (light: "flinch"),
        ),
        moves: [
            (
                // Live on tick 3 alone.
                name: "tap",
                startup: 3,
                active: 1,
                recovery: 6,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "tap_hit",
                    from: 3,
                    to: 4,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 10, weight: Light),
                )],
            ),
            (
                // Live on ticks 3 to 32.
                name: "sweep",
                startup: 3,
                active: 30,
                recovery: 2,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "sweep_hit",
                    from: 3,
                    to: 33,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 10, weight: Light),
                )],
            ),
            (
                // Live on ticks 3 to 32, re-arming at 10 and 20.
                name: "beats",
                startup: 3,
                active: 30,
                recovery: 2,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "beats_hit",
                    from: 3,
                    to: 33,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 10, weight: Light),
                    rearm: [10, 20],
                )],
            ),
            (
                // Touches as `sweep` does, and does nothing about it.
                name: "sense",
                startup: 3,
                active: 30,
                recovery: 2,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(name: "sense_hit", from: 3, to: 33, path: "blade", shape: Sphere(radius: 0.2))],
            ),
            (
                // From z = 0 to z = 2 in tick 3, stopped by the first wall it meets.
                name: "thrust",
                startup: 3,
                active: 2,
                recovery: 5,
                motion: (paths: [(name: "blade", keys: [(at: 3, offset: (0.0, 1.0, 0.0)), (at: 4, offset: (0.0, 1.0, 2.0))])]),
                hitboxes: [(
                    name: "thrust_hit",
                    from: 3,
                    to: 5,
                    path: "blade",
                    shape: Sphere(radius: 0.1),
                    attack: (damage: 10, weight: Light),
                    blocked_by: [World],
                )],
            ),
            (
                // A dodge whose own swing goes through its own volume, which must mean nothing.
                name: "selfish",
                startup: 0,
                active: 10,
                recovery: 0,
                motion: (paths: [(name: "fist", keys: [(at: 0, offset: (0.0, 1.0, 0.0))])]),
                hitboxes: [(
                    name: "selfish_hit",
                    from: 0,
                    to: 10,
                    path: "fist",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 10, weight: Light),
                )],
                invulnerable: [(from: 0, to: 10)],
                perfect_dodge: [(from: 0, to: 10, volume: (offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25)))],
            ),
            (
                name: "roll",
                startup: 1,
                active: 13,
                recovery: 8,
                invulnerable: [(from: 1, to: 14)],
                perfect_dodge: [(
                    from: 1,
                    to: 7,
                    volume: (offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25)),
                    slow_motion: (per_mille: 200, steps: 30),
                    counter: (ticks: 20, allow: [(intent: "answer", move: "riposte")]),
                )],
                cancels: [
                    (from: 9, to: 22, allow: [(intent: "tap", move: "tap")]),
                    (from: 16, to: 22, allow: [(intent: "roll", move: "roll", when: Instant)]),
                ],
                cues: [(name: "roll_start", at: 0)],
            ),
            (
                name: "quiet",
                startup: 1,
                active: 13,
                recovery: 8,
                invulnerable: [(from: 1, to: 14)],
                perfect_dodge: [(from: 1, to: 7)],
                cancels: [(from: 6, to: 22, allow: [(intent: "roll", move: "roll")])],
            ),
            (
                name: "riposte",
                startup: 1,
                active: 2,
                recovery: 5,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "riposte_hit",
                    from: 1,
                    to: 3,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 20, weight: Light),
                )],
                invulnerable: [(from: 0, to: 3)],
            ),
            (
                name: "flinch",
                startup: 0,
                active: 4,
                recovery: 8,
                cancels: [(from: 0, to: 12, allow: [(intent: "roll", move: "roll")])],
            ),
        ],
    ),
)"#;

const ROLL_WINDOW: &str = "counter: (ticks: 20, allow: [(intent: \"answer\", move: \"riposte\")])";

fn name(text: &str) -> Name {
    Name::new(text)
}

fn ticks(count: u64) -> SimDuration {
    SimDuration::from_ticks(count)
}

fn moves() -> MoveSet {
    MoveSetDocument::parse(DODGES).unwrap().compile().unwrap()
}

/// [`moves`] with some of [`DODGES`]' text replaced.
fn moves_with(from: &str, to: &str) -> MoveSet {
    assert!(DODGES.contains(from), "{from}");
    MoveSetDocument::parse(&DODGES.replacen(from, to, 1)).unwrap().compile().unwrap()
}

/// An attacker at the origin facing +Z, and a dodger 1 m ahead facing it.
fn duel_of(set: MoveSet) -> (Simulation, ActorId, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(set);
    let spawn = ActorSpawn { moves: Some(moves), ..ActorSpawn::default() };
    let attacker = sim.spawn(spawn).unwrap();
    let at = Vec3::new(0.0, 0.0, 1.0);
    let dodger = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
    (sim, attacker, dodger)
}

fn duel() -> (Simulation, ActorId, ActorId) {
    duel_of(moves())
}

fn press(actor: ActorId, buttons: Buttons) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).unwrap();
    actions
}

fn step_all(sim: &mut Simulation, presses: &[(ActorId, Buttons)]) {
    let mut actions = TickActions::NEUTRAL;
    for &(actor, buttons) in presses {
        actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).unwrap();
    }
    sim.step(&actions);
}

fn idle(sim: &mut Simulation, steps: u32) {
    for _ in 0..steps {
        sim.step(&TickActions::NEUTRAL);
    }
}

/// Runs steps `from..to` of a schedule of `(step, actor, buttons)`.
fn run(sim: &mut Simulation, schedule: &[(u32, ActorId, Buttons)], from: u32, to: u32) {
    for step in from..to {
        let due: Vec<_> = schedule
            .iter()
            .filter(|(at, ..)| *at == step)
            .map(|&(_, actor, buttons)| (actor, buttons))
            .collect();
        step_all(sim, &due);
    }
}

fn health(sim: &Simulation, actor: ActorId) -> u32 {
    sim.actor(actor).unwrap().fighter.health()
}

fn playing(sim: &Simulation, actor: ActorId) -> Option<Name> {
    sim.actor(actor).unwrap().moves.current.map(|current| current.name)
}

fn elapsed(sim: &Simulation, actor: ActorId) -> SimDuration {
    sim.actor(actor).unwrap().moves.current.unwrap().elapsed
}

fn clock(sim: &Simulation, actor: ActorId) -> SimTime {
    sim.actor(actor).unwrap().clock_now
}

fn perfect(sim: &Simulation) -> u64 {
    sim.stats().perfect_dodges
}

fn cue_of(sim: &Simulation, cue: Name) -> Vec<CueEvent> {
    sim.cues().filter(|event| event.cue == cue).collect()
}

/// Whether the actor's perfect-dodge volume is in the world.
fn sensing(sim: &Simulation, actor: ActorId) -> bool {
    sim.physics().body_shape(dodge_body(actor).unwrap()).is_some()
}

/// The PerfectDodge scales on an actor's clock.
fn scaled_back(sim: &mut Simulation, actor: ActorId) -> Vec<u32> {
    let clock = sim.clock_mut(actor).unwrap();
    clock.0.scales().filter(|s| s.source == ScaleSource::PerfectDodge).map(|s| s.per_mille).collect()
}

fn world_scale(sim: &mut Simulation) -> Vec<u32> {
    let clock = sim.world_clock_mut();
    clock.0.scales().filter(|s| s.source == ScaleSource::PerfectDodge).map(|s| s.per_mille).collect()
}

/// A swing against a dodge, with its blade live on one step that falls on this tick of the dodge: the
/// attacker presses on step 20, so `tap` is live on step 23 alone, and the dodger on step `23 - tick`.
/// Runs through the step the swing is live and returns the simulation there.
fn tap_on_tick(buttons: Buttons, tick: u32) -> (Simulation, ActorId, ActorId) {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(20, attacker, Buttons::LIGHT), (23 - tick, dodger, buttons)];
    run(&mut sim, &schedule, 0, 24);
    (sim, attacker, dodger)
}

// ------------------------------------------------------------------------------------------------
// Invulnerability and dodge cancels.
// ------------------------------------------------------------------------------------------------

/// A dodge through a swing takes no hit, on any tick of its i-frames, and one that's not there yet or has
/// ended takes it: ticks 1 to 13, and no others.
#[test]
fn i_frames_cover_their_ticks_and_no_others() {
    for tick in 0..=16 {
        let (sim, _, dodger) = tap_on_tick(Buttons::SKILL, tick);
        let expected = if (1..14).contains(&tick) { 100 } else { 90 };
        assert_eq!(health(&sim, dodger), expected, "a swing on tick {tick} of the dodge");
        assert_eq!(perfect(&sim), 0, "`quiet`'s window is only a window");
    }
}

/// A dodge that ends before the swing does takes it. `sweep`, pressed on step 0, is live from step 3 to
/// step 32. `quiet`, pressed on step 2, has its last i-frame on step 15 and its hurtbox back on step 16.
#[test]
fn a_dodge_that_ends_early_takes_the_swing() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::SKILL)];
    run(&mut sim, &schedule, 0, 16);
    assert_eq!(health(&sim, dodger), 100, "through step 15, and nothing was spent on it");
    assert_eq!(sim.stats().hits_applied, 0);
    idle(&mut sim, 1);
    assert_eq!(health(&sim, dodger), 90, "on step 16, the first with a hurtbox");
    assert_eq!(sim.hits().events()[0].at, ticks(15), "from where this step's sweep began");
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, dodger), Some(name("flinch")));
}

/// Which moves may be dodge-cancelled is just what their `cancels` allow. `flinch` allows a roll from its
/// first tick, and `tap` nothing, so a press through `tap` waits for its end if it lasts, and is gone if
/// its buffer isn't long enough.
#[test]
fn a_dodge_cancel_is_a_cancel_window_like_any_other() {
    // `tap` is 10 ticks long, pressed on step 0. A roll pressed on step 1 has a buffer of 8, through step 8.
    let (mut sim, attacker, _) = duel();
    let schedule = [(0, attacker, Buttons::LIGHT), (1, attacker, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 10);
    assert_eq!(playing(&sim, attacker), Some(name("tap")), "no window lets it out");
    idle(&mut sim, 2);
    assert_eq!(playing(&sim, attacker), None, "and the press ran out before the move did");

    // A roll inside `flinch`'s window starts on its press's own step.
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::LIGHT), (5, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 5);
    assert_eq!(playing(&sim, dodger), Some(name("flinch")), "the hit landed on step 3");
    assert_eq!(elapsed(&sim, dodger), ticks(1), "it began where step 3 stopped, with no hitstop to hold it");
    run(&mut sim, &schedule, 5, 6);
    assert_eq!(playing(&sim, dodger), Some(name("roll")));
    assert_eq!(elapsed(&sim, dodger), ticks(0), "at once");
    assert_eq!(sim.stats().cancels_fired, 1);
}

// ------------------------------------------------------------------------------------------------
// The perfect dodge.
// ------------------------------------------------------------------------------------------------

/// `roll`'s window is ticks 1 to 6. A swing live on one step fires it on ticks 1 and 6 and everywhere
/// between, on that very step, and not on tick 0, where it lands, nor on tick 7, where it goes through
/// with nothing to show.
#[test]
fn a_perfect_dodge_fires_on_its_windows_first_and_last_ticks_and_not_one_outside() {
    #[rustfmt::skip]
    let table = [
        (0,  false, 90),  // Not open, and no i-frames: it lands.
        (1,  true,  100), // Its first tick.
        (2,  true,  100),
        (5,  true,  100),
        (6,  true,  100), // Its last tick.
        (7,  false, 100), // One past: i-frames, and nothing else.
        (13, false, 100), // The last i-frame.
        (14, false, 90),  // Its hurtbox is back.
    ];
    for (tick, fires, left) in table {
        let (sim, _, dodger) = tap_on_tick(Buttons::DODGE, tick);
        assert_eq!(perfect(&sim), u64::from(fires), "tick {tick}");
        assert_eq!(health(&sim, dodger), left, "tick {tick}");
        let cues = cue_of(&sim, PERFECT_DODGE);
        assert_eq!(cues.len(), usize::from(fires), "the cue, in the step it fires: tick {tick}");
        for cue in cues {
            assert_eq!(cue.actor, dodger, "the dodger's");
            assert_eq!(cue.move_time, ticks(u64::from(tick)), "how far into its dodge");
            assert_eq!(cue.tick, 24, "the world's 24th step");
            assert_eq!(cue.local_time, SimTime::from_subticks(24_000), "on the dodger's clock");
            assert_eq!(cue.hit, None);
        }
        let dodged = sim.hits().dodged();
        assert_eq!(dodged.len(), usize::from(fires), "tick {tick}");
        if fires {
            assert_eq!(dodged[0].dodger, dodger);
            assert_eq!(dodged[0].group, name("tap_hit"));
            assert_eq!(dodged[0].at, ticks(3), "tick 3 of the swing");
            assert_eq!(dodged[0].travel, 0.0, "it starts touching");
        }
    }
}

/// The window is read on the dodger's own clock, pressed here on step 10. At half speed tick 1 is step 12,
/// and step 24 reads tick 7, so a swing live on steps 12 and 23 fires it and steps 11 and 24 don't. At 3×
/// steps 11 and 12 read ticks 3 and 6, and step 13 reads 9. At 9×, the step reading tick 9 passed the whole
/// window, so it counts, and the next reads 18, past the i-frames too.
#[test]
fn the_window_runs_on_the_dodgers_clock() {
    for (per_mille, swing_at, fires, left) in [
        (500, 11, false, 90), // Reading 0.5: i-frames haven't begun.
        (500, 12, true, 100),
        (500, 23, true, 100),
        (500, 24, false, 100), // Reading 7.
        (3000, 11, true, 100),
        (3000, 12, true, 100),
        (3000, 13, false, 100), // Reading 9, past the window and inside the i-frames.
        (9000, 11, true, 100),  // Reading 9, the step that passed the window whole.
        (9000, 12, false, 90),  // Reading 18: the i-frames are gone.
    ] {
        let (mut sim, attacker, dodger) = duel();
        sim.clock_mut(dodger).unwrap().0.push_scale(ScaleSource::Gameplay, per_mille).unwrap();
        let schedule = [(swing_at - 3, attacker, Buttons::LIGHT), (10, dodger, Buttons::DODGE)];
        run(&mut sim, &schedule, 0, swing_at + 1);
        assert_eq!(perfect(&sim), u64::from(fires), "{per_mille}‰ on step {swing_at}");
        assert_eq!(health(&sim, dodger), left, "{per_mille}‰ on step {swing_at}");
    }
}

/// A swing on the tick the window opens: the window wins. Hurtboxes leave the world before any sweep, on
/// the step that first reads it. Here the window opens on the move's first tick, so even a press on the
/// very step a swing is live beats it, which a dodge with a tick 0 of i-frames can do.
#[test]
fn a_swing_on_the_tick_the_window_opens_fires_it_and_lands_nothing() {
    let opening = moves_with(
        "invulnerable: [(from: 1, to: 14)],\n                perfect_dodge: [(\n                    from: 1,",
        "invulnerable: [(from: 0, to: 14)],\n                perfect_dodge: [(\n                    from: 0,",
    );
    let (mut sim, attacker, dodger) = duel_of(opening);
    let schedule = [(20, attacker, Buttons::LIGHT), (23, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 24);
    assert_eq!(elapsed(&sim, dodger), ticks(0));
    assert_eq!(perfect(&sim), 1);
    assert_eq!(health(&sim, dodger), 100);
    assert_eq!(sim.stats().hits_applied, 0, "a step never has both");
    assert!(sim.hits().events().is_empty());

    // The same swing a tick before the window opens lands, and the reaction replaces the dodge, which
    // then never reads its window: that's `roll` as written, on tick 0.
    let (mut sim, _, dodger) = tap_on_tick(Buttons::DODGE, 0);
    assert_eq!((perfect(&sim), health(&sim, dodger)), (0, 90));
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, dodger), Some(name("flinch")));
    assert!(!sensing(&sim, dodger), "no window, so no volume");
    idle(&mut sim, 10);
    assert_eq!(perfect(&sim), 0);
}

/// A long swing through the whole window fires it once, on the first step it touches. `sweep` from step 0
/// is live from step 3, and `roll` from step 2 reads tick 1 there. A second playing can fire on another
/// swing (`a_second_perfect_dodge_starts_the_slowdown_again`), and this one is spent.
#[test]
fn it_fires_once_per_dodge() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 3);
    assert_eq!(perfect(&sim), 0, "nothing's live yet");
    assert!(!sim.actor(dodger).unwrap().moves.current.unwrap().dodged);
    run(&mut sim, &schedule, 3, 4);
    assert_eq!(perfect(&sim), 1);
    assert!(sim.actor(dodger).unwrap().moves.current.unwrap().dodged);
    assert_eq!(cue_of(&sim, PERFECT_DODGE).len(), 1);
    // Ticks 2 to 6 of its window, on steps 4 to 8, with the swing touching throughout.
    for step in 4..=8 {
        idle(&mut sim, 1);
        assert_eq!(perfect(&sim), 1, "step {step}");
        assert!(cue_of(&sim, PERFECT_DODGE).is_empty(), "step {step}");
        assert!(sim.actor(dodger).unwrap().moves.windows.perfect_dodge, "the window still counts: {step}");
    }
}

/// Its volume is in the world only on the steps it can fire, and costs nothing otherwise.
#[test]
fn the_volume_is_there_only_while_the_window_can_fire() {
    let (mut sim, _, dodger) = duel();
    sim.step(&press(dodger, Buttons::DODGE));
    assert!(!sensing(&sim, dodger), "tick 0");
    for tick in 1..=6 {
        idle(&mut sim, 1);
        assert!(sensing(&sim, dodger), "tick {tick}");
        assert!(sim.actor(dodger).unwrap().moves.windows.perfect_dodge);
    }
    idle(&mut sim, 1);
    assert!(!sensing(&sim, dodger), "tick 7");
    assert_eq!(sim.physics().body_count(), 1, "the attacker's hurtbox: the dodger has none in its i-frames");

    // Fired, it's gone from the next step, with the window still counting.
    let (mut sim, _, dodger) = tap_on_tick(Buttons::DODGE, 1);
    assert_eq!(perfect(&sim), 1);
    assert!(sensing(&sim, dodger), "the step it fired on");
    idle(&mut sim, 1);
    assert!(!sensing(&sim, dodger), "tick 2");
    assert!(sim.actor(dodger).unwrap().moves.windows.perfect_dodge, "the window counts for a view");

    // A `quiet` window has none, ever.
    let (mut sim, _, dodger) = duel();
    sim.step(&press(dodger, Buttons::SKILL));
    for _ in 0..8 {
        idle(&mut sim, 1);
        assert!(!sensing(&sim, dodger));
    }

    // A despawn takes it out of the world.
    let (mut sim, _, dodger) = duel();
    sim.step(&press(dodger, Buttons::DODGE));
    idle(&mut sim, 1);
    assert!(sensing(&sim, dodger));
    assert!(sim.despawn(dodger));
    assert_eq!(sim.physics().body_shape(dodge_body(dodger).unwrap()), None);
}

// What can't fire one.

/// A plain window that overlaps one with a volume doesn't hide it: what can fire is the first with a volume.
#[test]
fn a_window_with_no_volume_doesnt_hide_one_that_has_one() {
    let plain_first = moves_with(
        "perfect_dodge: [(\n                    from: 1,",
        "perfect_dodge: [(from: 1, to: 3), (\n                    from: 1,",
    );
    let (mut sim, attacker, dodger) = duel_of(plain_first);
    let schedule = [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 24);
    assert_eq!(perfect(&sim), 1, "its tick 2, inside both");
    assert_eq!(world_scale(&mut sim), [200], "and the second's what it read");
}

/// A defeated actor has no volume, whatever its `defeat` reaction authors. A `tap` from step 0 defeats it
/// on step 3, and `roll` as that reaction reads tick 1 on step 4, so its window would count on steps 4 to
/// 9, with a second attacker's `sweep` from step 2 live from step 5.
#[test]
fn a_defeated_actor_cant_fire_it() {
    let text = DODGES.replacen("health: 100", "health: 10", 1).replacen(
        "reactions: (light: \"flinch\")",
        "reactions: (light: \"flinch\", defeat: \"roll\")",
        1,
    );
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(MoveSetDocument::parse(&text).unwrap().compile().unwrap()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let (first, second) = (sim.spawn(spawn).unwrap(), sim.spawn(spawn).unwrap());
    let at = Vec3::new(0.0, 0.0, 1.0);
    let target = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
    let schedule = [(0, first, Buttons::LIGHT), (2, second, Buttons::HEAVY)];
    run(&mut sim, &schedule, 0, 5);
    assert!(sim.actor(target).unwrap().fighter.is_defeated());
    assert_eq!(playing(&sim, target), Some(name("roll")));
    run(&mut sim, &schedule, 5, 10);
    assert_eq!(perfect(&sim), 0);
    assert!(!sensing(&sim, target));
}

/// A blade that isn't live can't, however close it stands. `tap` from step 0 is live on step 3 alone, and
/// goes through `quiet`'s i-frames from step 0. A roll pressed on step 6 cancels those, with its window on
/// steps 7 to 12, and the blade right there, in its recovery. For a swing not live yet, see the first and
/// last ticks' table above: on a tick past the window it's the dodge that has gone.
#[test]
fn a_blade_that_isnt_live_cant_fire_it() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::LIGHT), (0, dodger, Buttons::SKILL), (6, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 8);
    assert!(sensing(&sim, dodger), "the window is open");
    assert_eq!(playing(&sim, attacker), Some(name("tap")));
    run(&mut sim, &schedule, 8, 13);
    assert_eq!(elapsed(&sim, dodger), ticks(6), "its window's last tick");
    assert_eq!(perfect(&sim), 0);
    assert_eq!(sim.hits().stats().dodge_touches, 0, "nothing swept, so nothing touched");
    assert_eq!(health(&sim, dodger), 100);
}

/// A volume that does nothing when it lands has nothing to dodge.
#[test]
fn a_sensor_cant_fire_it() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::SWAP), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 12);
    assert_eq!(perfect(&sim), 0);
    assert!(sim.hits().stats().dodge_touches > 0, "it touched: this is the check that stopped it");
    assert_eq!(health(&sim, dodger), 100);
}

/// An actor's own swing can't fire its own dodge. Its filter skips its whole run of bodies, so it isn't
/// even touched.
#[test]
fn an_actors_own_swing_cant_fire_it() {
    let (mut sim, attacker, _) = duel();
    sim.step(&press(attacker, Buttons::LOCK_ON));
    idle(&mut sim, 1);
    assert!(sensing(&sim, attacker));
    assert_eq!(perfect(&sim), 0);
    assert_eq!(sim.hits().stats().dodge_touches, 0);
}

/// Its owner's clock frozen, a live volume sweeps nothing, so it can't fire one, as it can't hit. `sweep`
/// from step 0 is live from step 3, and goes through `quiet`'s i-frames from step 1 to 13. The attacker
/// freezes from step 5, and a roll cancels `quiet` on step 6 with its window open from step 7 to step 12.
#[test]
fn a_frozen_attackers_volume_cant_fire_it() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (0, dodger, Buttons::SKILL), (6, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 5);
    sim.clock_mut(attacker).unwrap().0.apply_hitstop(40);
    let skipped = sim.hits().stats().frozen_skips;
    run(&mut sim, &schedule, 5, 13);
    assert_eq!(playing(&sim, dodger), Some(name("roll")));
    assert_eq!(elapsed(&sim, dodger), ticks(6), "its window's last tick");
    assert!(sim.hits().stats().frozen_skips > skipped, "its volume is live, and passed over");
    assert_eq!(perfect(&sim), 0);
    assert_eq!(sim.hits().stats().dodge_touches, 0);
    assert_eq!(health(&sim, dodger), 100);
}

/// A swing whose hit group already has the dodger can't land on it again, so there's nothing to dodge,
/// until the group re-arms. `sweep` hits a standing dodger on step 3 and its reaction starts on step 4. A
/// roll pressed on step 5 cancels that, with its window on steps 6 to 11, and `sweep` stays live through
/// all of it and never fires. `beats` re-arms at its tick 10, so the sweep that starts there, on step 11,
/// the window's last, is a new volley.
#[test]
fn a_swing_that_has_already_hit_cant_fire_it_until_it_re_arms() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (5, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 6);
    assert_eq!(health(&sim, dodger), 90);
    assert_eq!(playing(&sim, dodger), Some(name("roll")));
    run(&mut sim, &schedule, 6, 13);
    assert!(sim.hits().stats().dodge_touches > 0, "it touched: this is the check that stopped it");
    assert_eq!(perfect(&sim), 0);

    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::JUMP), (5, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 11);
    assert_eq!(perfect(&sim), 0, "volley 0, through the sweep that ends on tick 10");
    run(&mut sim, &schedule, 11, 12);
    assert_eq!(perfect(&sim), 1, "volley 1");
    assert_eq!(sim.hits().dodged()[0].at, ticks(10));
    assert_eq!(health(&sim, dodger), 90, "and nothing's landed since");
}

/// A swing a wall stopped can't land past it, so it can't fire one past it either. The `thrust` goes from
/// z = 0 to z = 2 on its tick 3 and its radius is 0.1: the dodger's chest is in its way from z = 0.65,
/// with a wall at z = 0.4 it never gets there.
#[test]
fn a_swing_a_wall_stopped_cant_fire_it() {
    let moved = |wall: bool| {
        let swap = "(intent: \"sense\", move: \"sense\")";
        let (mut sim, attacker, dodger) = duel_of(moves_with(swap, "(intent: \"sense\", move: \"thrust\")"));
        let statics = if wall {
            vec![StaticCollider {
                id: ColliderId(0),
                shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(2.0, 2.0, 0.1) }),
                pose: Pose::at(Vec3::new(0.0, 1.0, 0.5)),
                memberships: Group::World.mask(),
            }]
        } else {
            Vec::new()
        };
        let world = PhysicsWorld::new(statics, sim.body_capacity()).unwrap();
        // Hurtboxes went in on no step yet, and the world can still be replaced.
        sim.set_physics_world(world).unwrap();
        let schedule = [(0, attacker, Buttons::SWAP), (2, dodger, Buttons::DODGE)];
        run(&mut sim, &schedule, 0, 6);
        perfect(&sim)
    };
    assert_eq!(moved(false), 1, "with nothing in its way, it fires");
    assert_eq!(moved(true), 0, "with a wall, it doesn't");
}

/// A frozen dodger reads no new ticks and fires nothing: its window waits, and a swing still touching when
/// it thaws fires it then. `roll` from step 0 reads tick 1 on step 1, and freezes for 20 steps, 2 to 21.
/// `sweep` from step 0 is live from step 3.
#[test]
fn a_frozen_dodger_doesnt_advance_or_fire() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (0, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 2);
    assert_eq!(elapsed(&sim, dodger), ticks(1));
    assert!(sensing(&sim, dodger));
    sim.clock_mut(dodger).unwrap().0.apply_hitstop(20);
    for step in 2..22 {
        idle(&mut sim, 1);
        assert_eq!(elapsed(&sim, dodger), ticks(1), "step {step}");
        assert!(!sensing(&sim, dodger), "no volume on a step that can't fire: step {step}");
        assert_eq!(perfect(&sim), 0, "step {step}");
        assert_eq!(health(&sim, dodger), 100, "still in its i-frames: step {step}");
    }
    idle(&mut sim, 1);
    assert_eq!(elapsed(&sim, dodger), ticks(2), "thawed");
    assert_eq!(perfect(&sim), 1, "and the swing's still there");
}

/// Two swings on one step fire it once, and the credit goes to the nearest, then the first.
#[test]
fn two_swings_on_one_step_fire_it_once() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let (first, second) = (sim.spawn(spawn).unwrap(), sim.spawn(spawn).unwrap());
    let at = Vec3::new(0.0, 0.0, 1.0);
    let dodger = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
    let schedule = [(0, first, Buttons::LIGHT), (0, second, Buttons::LIGHT), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(cue_of(&sim, PERFECT_DODGE).len(), 1);
    assert_eq!(sim.hits().dodged().len(), 1, "one for each dodger");
    assert_eq!(sim.hits().dodged()[0].attacker, first, "both start touching, so the first swept");
}

/// The credit goes by how far each swing's blade had come. `thrust` from step 0 reads its tick 3 on step 3,
/// with the blade at z = 0, and sweeps to z = 2 on step 4, where a radius of 0.1 meets the volume's near
/// side at z = 0.75 from z = 0.65: 0.65 m along, a share of 0.325 of the tick. `tap` pressed on step 1, from
/// the other side, is live on step 4 alone, and touches at once. So `tap` has the credit, whichever is swept
/// first, and the record says when and how far.
#[test]
fn the_nearer_swing_has_the_credit_and_the_record_says_where_it_touched() {
    let swap = "(intent: \"sense\", move: \"sense\")";
    let moved = |thrust_first: bool| {
        let mut sim = Simulation::new(SimConfig::default());
        let set = moves_with(swap, "(intent: \"sense\", move: \"thrust\")");
        let moves = Some(sim.add_move_set(set));
        let from_here = ActorSpawn { moves, ..ActorSpawn::default() };
        let from_there = ActorSpawn { position: Vec3::new(0.0, 0.0, 2.0), yaw: PI, ..from_here };
        let (thrust, tap) = if thrust_first {
            let thrust = sim.spawn(from_here).unwrap();
            (thrust, sim.spawn(from_there).unwrap())
        } else {
            let tap = sim.spawn(from_there).unwrap();
            (sim.spawn(from_here).unwrap(), tap)
        };
        let at = Vec3::new(0.0, 0.0, 1.0);
        let dodger = sim.spawn(ActorSpawn { position: at, yaw: PI, ..from_here }).unwrap();
        let schedule = [(0, thrust, Buttons::SWAP), (1, tap, Buttons::LIGHT), (2, dodger, Buttons::DODGE)];
        run(&mut sim, &schedule, 0, 5);
        assert_eq!(perfect(&sim), 1);
        assert_eq!(sim.hits().stats().dodge_touches, 2);
        assert_eq!(sim.hits().dodged()[0].attacker, tap, "thrust first: {thrust_first}");
        assert_eq!(sim.hits().dodged()[0].travel, 0.0);
    };
    moved(true);
    moved(false);

    // With the `thrust` alone.
    let (mut sim, attacker, dodger) = duel_of(moves_with(swap, "(intent: \"sense\", move: \"thrust\")"));
    let schedule = [(0, attacker, Buttons::SWAP), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 5);
    assert_eq!(perfect(&sim), 1);
    let swing = sim.hits().dodged()[0];
    assert!((swing.travel - 0.65).abs() < 1e-3, "{swing:?}");
    let reached = swing.at.as_subticks();
    assert!((3320..=3330).contains(&reached), "{swing:?}");
}

/// Sweeps go by attacker, and what they find goes by dodger. The first attacker's dodger has the larger ID,
/// so the two touches come out of ID order, and go in as cues in order.
#[test]
fn what_the_sweeps_find_goes_by_dodger() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let aside = Vec3::new(10.0, 0.0, 0.0);
    let first = sim.spawn(spawn).unwrap();
    let second = sim.spawn(ActorSpawn { position: aside, ..spawn }).unwrap();
    let second_dodger =
        sim.spawn(ActorSpawn { position: aside + Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    let first_dodger =
        sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    assert!(second_dodger < first_dodger);
    let schedule = [
        (0, first, Buttons::LIGHT),
        (0, second, Buttons::LIGHT),
        (2, first_dodger, Buttons::DODGE),
        (2, second_dodger, Buttons::DODGE),
    ];
    run(&mut sim, &schedule, 0, 4);
    let who: Vec<ActorId> = sim.hits().dodged().iter().map(|swing| swing.dodger).collect();
    assert_eq!(who, [second_dodger, first_dodger]);
    assert_eq!(sim.hits().dodged()[0].attacker, second);
    let cues: Vec<ActorId> = cue_of(&sim, PERFECT_DODGE).iter().map(|cue| cue.actor).collect();
    assert_eq!(cues, [second_dodger, first_dodger]);
}

/// Two dodgers on one step each fire theirs, in ID order, both running at speed in the world that slows.
#[test]
fn two_dodgers_on_one_step_each_fire_and_both_run_at_speed() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let attacker = sim.spawn(spawn).unwrap();
    let dodgers = [Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.2, 0.0, 1.0)]
        .map(|position| sim.spawn(ActorSpawn { position, yaw: PI, ..spawn }).unwrap());
    let schedule =
        [(0, attacker, Buttons::LIGHT), (2, dodgers[1], Buttons::DODGE), (2, dodgers[0], Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 2);
    let who: Vec<ActorId> = sim.hits().dodged().iter().map(|swing| swing.dodger).collect();
    assert_eq!(who, dodgers);
    let cues: Vec<ActorId> = cue_of(&sim, PERFECT_DODGE).iter().map(|cue| cue.actor).collect();
    assert_eq!(cues, dodgers);
    assert_eq!(scaled_back(&mut sim, dodgers[0]), [5000]);
    assert_eq!(scaled_back(&mut sim, dodgers[1]), [5000]);
    assert_eq!(scaled_back(&mut sim, attacker), [] as [u32; 0]);
    idle(&mut sim, 1);
    let run_at = |sim: &Simulation, actor| sim.actor(actor).unwrap().clock_now;
    assert_eq!(run_at(&sim, dodgers[0]), run_at(&sim, dodgers[1]));
    assert_eq!(run_at(&sim, dodgers[0]), SimTime::from_subticks(5000));
    assert_eq!(run_at(&sim, attacker), SimTime::from_subticks(4200));
}

/// The list of a step's swings holds 64 dodgers, and past that they're counted and don't fire. Three
/// attackers on islands of their own, with 22 dodgers each, are 66 touches, in ID order. Forced between
/// steps 1 and 2, because a tick's input doesn't hold that many, each roll reads tick 2 on step 3.
#[test]
fn more_swings_than_the_list_holds_are_counted() {
    let mut sim = Simulation::new(SimConfig { max_actors: 69, ..SimConfig::default() });
    let moves = Some(sim.add_move_set(moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let mut attackers = Vec::new();
    let mut dodgers = Vec::new();
    for island in 0..3 {
        let aside = Vec3::new(island as f32 * 10.0, 0.0, 0.0);
        attackers.push(sim.spawn(ActorSpawn { position: aside, ..spawn }).unwrap());
        for _ in 0..22 {
            let position = aside + Vec3::new(0.0, 0.0, 1.0);
            dodgers.push(sim.spawn(ActorSpawn { position, yaw: PI, ..spawn }).unwrap());
        }
    }
    let schedule: Vec<_> = attackers.iter().map(|&attacker| (0, attacker, Buttons::LIGHT)).collect();
    run(&mut sim, &schedule, 0, 2);
    for &dodger in &dodgers {
        sim.force_move(dodger, name("roll"));
    }
    idle(&mut sim, 2);
    assert_eq!(sim.hits().stats().swings_dropped, 2);
    assert_eq!(sim.hits().dodged().len(), 64);
    assert_eq!(perfect(&sim), 64);
    assert!(!sim.actor(dodgers[65]).unwrap().moves.current.unwrap().dodged, "the last didn't fire");
    assert_eq!(health(&sim, dodgers[65]), 100, "and its i-frames still hold");
}

// ------------------------------------------------------------------------------------------------
// What a fired perfect dodge spends.
// ------------------------------------------------------------------------------------------------

/// A perfect dodge spends the swing it went through on its dodger. `sweep` from step 0 reads tick 3 on step
/// 3, where a roll from step 2 fires, and it's slowed from there: tick 9 on step 33, then one a step, so
/// it's live until its reading of 33 on step 57. The roll's i-frames end on step 16, its tick 14, with the
/// blade still on its hurtbox. Nothing lands, and nor does that swing fire its next dodge: an instant roll
/// from the first's tick 16, step 18, has its window open on steps 19 to 24.
#[test]
fn a_perfectly_dodged_swing_is_spent_on_that_dodger() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE), (18, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(sim.hits().record_count(), 1);
    run(&mut sim, &schedule, 4, 16);
    let touches = sim.hits().stats().touches;
    run(&mut sim, &schedule, 16, 17);
    assert_eq!(sim.hits().stats().touches, touches + 1, "the blade finds its hurtbox: the record stops it");
    assert_eq!(health(&sim, dodger), 100);

    run(&mut sim, &schedule, 17, 25);
    assert_eq!(playing(&sim, dodger), Some(name("roll")));
    assert!(sim.actor(dodger).unwrap().moves.current.unwrap().elapsed >= ticks(6), "a second playing");
    assert!(!sim.actor(dodger).unwrap().moves.current.unwrap().dodged);
    assert_eq!(perfect(&sim), 1, "the same swing, spent");

    idle(&mut sim, 34);
    assert!(sim.actor(attacker).unwrap().moves.volumes.is_empty(), "and it's over");
    assert_eq!(health(&sim, dodger), 100);
    assert_eq!(sim.hits().stats().swings_unspent, 0);
}

/// D4's rule holds for everything else: i-frames take hurtboxes out and write nothing, so a swing that
/// outlasts them lands. `quiet` from step 2 has the same i-frames as a roll and no volume, and `sweep` from
/// step 0 lands when they end on step 16. A roll whose window saw nothing is the same: `sweep` from step 6
/// is live from step 9, the roll's tick 7, a tick past its window.
#[test]
fn a_swing_that_outlasts_i_frames_that_fired_nothing_lands() {
    for (attack, dodge) in [(0, Buttons::SKILL), (6, Buttons::DODGE)] {
        let (mut sim, attacker, dodger) = duel();
        let schedule = [(attack, attacker, Buttons::HEAVY), (2, dodger, dodge)];
        run(&mut sim, &schedule, 0, 16);
        assert_eq!(health(&sim, dodger), 100, "{dodge:?}");
        assert_eq!(sim.hits().record_count(), 0, "{dodge:?}");
        run(&mut sim, &schedule, 16, 17);
        assert_eq!(perfect(&sim), 0, "{dodge:?}");
        assert_eq!(health(&sim, dodger), 90, "{dodge:?}");
    }
}

/// Only the swing with the credit is spent, and only for its dodger. Two `sweep`s touch on step 3: the first
/// swept has the credit, and the second lands when the i-frames end on step 16, and nothing more does. A
/// bystander beside the dodger takes that first one on step 3 like anything else.
#[test]
fn only_the_swing_it_fired_on_is_spent_and_only_for_its_dodger() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let (first, second) = (sim.spawn(spawn).unwrap(), sim.spawn(spawn).unwrap());
    let dodger = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    let schedule = [(0, first, Buttons::HEAVY), (0, second, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 16);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(health(&sim, dodger), 100);
    run(&mut sim, &schedule, 16, 17);
    assert_eq!(sim.hits().events()[0].attacker, second);
    assert_eq!(health(&sim, dodger), 90);
    idle(&mut sim, 45);
    assert_eq!(health(&sim, dodger), 90);

    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(self::moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let attacker = sim.spawn(spawn).unwrap();
    let dodger = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    let beside = Vec3::new(0.2, 0.0, 1.0);
    let bystander = sim.spawn(ActorSpawn { position: beside, yaw: PI, ..spawn }).unwrap();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(health(&sim, bystander), 90);
    assert_eq!(health(&sim, dodger), 100);
}

/// A record is a hit group's, so another group of the same move follows its own rules, and another volume
/// of that group is spent with it, as a landed hit would leave them. `sweep` from step 0 gains a second
/// volume, live from its tick 5, and the i-frames end on step 16 with both on the hurtbox.
#[test]
fn a_record_is_a_hit_groups_as_a_landed_hits_is() {
    let second = |group: &str| {
        let hit = "name: \"sweep_hit\",\n                    from: 3,\n                    to: 33,\n                    path: \"blade\",\n                    shape: Sphere(radius: 0.2),\n                    attack: (damage: 10, weight: Light),\n                )";
        let both = format!(
            "{hit}, (\n                    name: \"{group}\",\n                    from: 5,\n                    to: 33,\n                    path: \"blade\",\n                    shape: Sphere(radius: 0.2),\n                    attack: (damage: 10, weight: Light),\n                )"
        );
        let (mut sim, attacker, dodger) = duel_of(moves_with(hit, &both));
        let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
        run(&mut sim, &schedule, 0, 17);
        assert_eq!(perfect(&sim), 1);
        let on_step_16 = health(&sim, dodger);
        idle(&mut sim, 45);
        assert_eq!(health(&sim, dodger), on_step_16, "and nothing more");
        on_step_16
    };
    assert_eq!(second("sweep_late"), 90, "another group");
    assert_eq!(second("sweep_hit"), 100, "the same group");
}

/// A record covers the volley it went through, so a swing that re-arms comes back as a landed one does.
/// `beats` from step 0 re-arms at its ticks 10 and 20. A roll from step 2 fires on step 3 in volley 0, and
/// the swing's slowed through step 33, at tick 9, so step 34 ends on tick 10 and the sweep that starts
/// there, on step 35, is volley 1, which lands. A roll pressed on step 33 has its window open then instead,
/// so the same volley fires that one, and is spent in its turn: volley 2 is the next that lands, from the
/// sweep starting at tick 20, once the second slowdown has let it get there.
#[test]
fn a_spent_swing_comes_back_when_it_re_arms() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::JUMP), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 35);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(health(&sim, dodger), 100, "through the sweep that ends on tick 10");
    run(&mut sim, &schedule, 35, 36);
    assert_eq!(sim.hits().events()[0].volley, 1);
    assert_eq!(health(&sim, dodger), 90, "volley 1");

    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::JUMP), (2, dodger, Buttons::DODGE), (33, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 35);
    assert_eq!(perfect(&sim), 1);
    run(&mut sim, &schedule, 35, 36);
    assert_eq!(perfect(&sim), 2, "volley 1 fires the next dodge");
    assert_eq!(sim.hits().dodged()[0].volley, 1);
    assert_eq!(sim.hits().dodged()[0].at, ticks(10));
    assert_eq!(health(&sim, dodger), 100);
    // Tick 11 on step 35, slowed over steps 36 to 65 to tick 17, then one a step: tick 20 on step 68.
    idle(&mut sim, 33);
    assert_eq!(health(&sim, dodger), 100, "volley 1 is spent, through the sweep that ends on tick 20");
    idle(&mut sim, 1);
    assert_eq!(sim.hits().events()[0].volley, 2);
    assert_eq!(health(&sim, dodger), 90, "volley 2");
}

// ------------------------------------------------------------------------------------------------
// Slow motion.
// ------------------------------------------------------------------------------------------------

/// A perfect dodge on step 3 slows the next 30 steps, 4 to 33, on the world clock and everything on it,
/// and scales the dodger back up over exactly those. From step 34 they all run at 1×.
#[test]
fn the_slowdown_runs_for_its_authored_time_with_the_dodger_at_speed_and_ends() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 1);
    assert!(sim.slow_motion().is_running());
    assert_eq!(world_scale(&mut sim), [200]);
    assert_eq!(scaled_back(&mut sim, dodger), [5000], "the inverse of 200‰");
    assert_eq!(scaled_back(&mut sim, attacker), [] as [u32; 0]);

    // Neither clock has been touched yet: the scales take effect from the next step's.
    assert_eq!(clock(&sim, attacker), SimTime::from_subticks(4000));
    assert_eq!(clock(&sim, dodger), SimTime::from_subticks(4000));

    let world_now = |sim: &mut Simulation| sim.world_clock_mut().0.now();
    let world_at = world_now(&mut sim);
    for slowed in 1..=30_u64 {
        idle(&mut sim, 1);
        assert_eq!(clock(&sim, attacker).as_subticks(), 4000 + slowed * 200, "step {slowed}");
        assert_eq!(clock(&sim, dodger).as_subticks(), 4000 + slowed * 1000, "step {slowed}");
        assert_eq!(world_now(&mut sim).since(world_at).as_subticks(), slowed * 200);
        assert_eq!(sim.slow_motion().is_running(), slowed < 30, "step {slowed}");
    }
    assert_eq!(world_scale(&mut sim), [] as [u32; 0], "gone in the step that ends it");
    assert_eq!(scaled_back(&mut sim, dodger), [] as [u32; 0], "and so is its counter-scale");

    idle(&mut sim, 1);
    assert_eq!(clock(&sim, attacker).as_subticks(), 4000 + 30 * 200 + 1000);
    assert_eq!(clock(&sim, dodger).as_subticks(), 4000 + 30 * 1000 + 1000);
    assert_eq!(world_now(&mut sim).since(world_at).as_subticks(), 30 * 200 + 1000);
    assert_eq!(sim.slow_motion().refused(), 0);
}

/// A window with no slow motion leaves every clock alone, and the rest of what it does still happens.
#[test]
fn slow_motion_is_optional() {
    let (mut sim, attacker, dodger) = duel_of(moves_with("slow_motion: (per_mille: 200, steps: 30),", ""));
    let schedule = [(20, attacker, Buttons::LIGHT), (22, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 24);
    assert_eq!(perfect(&sim), 1);
    assert!(!sim.slow_motion().is_running());
    assert_eq!(scaled_back(&mut sim, dodger), [] as [u32; 0]);
    assert!(sim.actor(dodger).unwrap().moves.counter_closes_at.is_some());
}

/// Something else asking for slow motion replaces what's running, and the dodger's scale follows it.
#[test]
fn a_later_request_replaces_it_and_the_dodger_follows() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(scaled_back(&mut sim, dodger), [5000]);
    sim.request_slow_motion(500, 10);
    idle(&mut sim, 1);
    assert_eq!(world_scale(&mut sim), [500]);
    assert_eq!(scaled_back(&mut sim, dodger), [2000]);
    assert_eq!(scaled_back(&mut sim, attacker), [] as [u32; 0]);
    let (a_at, d_at) = (clock(&sim, attacker), clock(&sim, dodger));
    idle(&mut sim, 1);
    assert_eq!(clock(&sim, attacker).since(a_at), SimDuration::from_subticks(500));
    assert_eq!(clock(&sim, dodger).since(d_at), SimDuration::from_subticks(1000), "still at speed");
    // It ends 10 steps after it was taken up, whoever asked for it.
    idle(&mut sim, 8);
    assert!(sim.slow_motion().is_running());
    assert_eq!(scaled_back(&mut sim, dodger), [2000]);
    idle(&mut sim, 1);
    assert!(!sim.slow_motion().is_running());
    assert_eq!(scaled_back(&mut sim, dodger), [] as [u32; 0]);

    // A request for no steps ends it at once, and the dodger with it.
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    sim.request_slow_motion(200, 0);
    idle(&mut sim, 1);
    assert_eq!(world_scale(&mut sim), [] as [u32; 0]);
    assert_eq!(scaled_back(&mut sim, dodger), [] as [u32; 0]);
}

/// A second perfect dodge starts the count again, and only one counter-scale ever sits on a clock. The
/// first attacker's `tap` from step 0 fires a roll from step 2 on step 3, for the 30 steps from 4 to 33. A
/// second attacker presses on step 8, under that slowdown from the start, so at a fifth of a tick a
/// step its reading comes to tick 3 on step 23, and is past it on step 28. An instant roll from the first
/// roll's tick 16, step 18, has its window on steps 19 to 24, so it fires on step 23, for steps 24 to 53.
/// With three more scales on its clock, the stack holds no room for a second, and none is needed.
#[test]
fn a_second_perfect_dodge_starts_the_slowdown_again() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(moves()));
    let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
    let (first, second) = (sim.spawn(spawn).unwrap(), sim.spawn(spawn).unwrap());
    let at = Vec3::new(0.0, 0.0, 1.0);
    let dodger = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
    for _ in 0..3 {
        sim.clock_mut(dodger).unwrap().0.push_scale(ScaleSource::Gameplay, 1000).unwrap();
    }
    let schedule = [
        (0, first, Buttons::LIGHT),
        (2, dodger, Buttons::DODGE),
        (8, second, Buttons::LIGHT),
        (18, dodger, Buttons::DODGE),
    ];
    run(&mut sim, &schedule, 0, 23);
    assert_eq!(perfect(&sim), 1);
    run(&mut sim, &schedule, 23, 24);
    assert_eq!(perfect(&sim), 2);
    assert_eq!(sim.hits().dodged()[0].attacker, second);
    assert_eq!(scaled_back(&mut sim, dodger), [5000], "and one scale, not two");
    assert_eq!(sim.slow_motion().refused(), 0);
    assert_eq!(world_scale(&mut sim), [200]);
    // Past where the first would have ended, on step 33, and on to the 30th step of the second, step 53.
    idle(&mut sim, 29);
    assert!(sim.slow_motion().is_running());
    assert_eq!(scaled_back(&mut sim, dodger), [5000]);
    idle(&mut sim, 1);
    assert!(!sim.slow_motion().is_running());
    assert_eq!(world_scale(&mut sim), [] as [u32; 0]);
    assert_eq!(scaled_back(&mut sim, dodger), [] as [u32; 0]);
}

/// A world's scale stack with no room refuses it, says so, and leaves nobody scaled back, since that would
/// run the dodger at five times speed. The counter window still opens.
#[test]
fn a_full_world_stack_refuses_it_and_nobody_runs_fast() {
    let (mut sim, attacker, dodger) = duel();
    for _ in 0..8 {
        sim.world_clock_mut().0.push_scale(ScaleSource::Debug, 1000).unwrap();
    }
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(sim.slow_motion().refused(), 1);
    assert!(!sim.slow_motion().is_running());
    assert_eq!(scaled_back(&mut sim, dodger), [] as [u32; 0]);
    assert!(sim.actor(dodger).unwrap().moves.counter_closes_at.is_some());
    let at = clock(&sim, dodger);
    idle(&mut sim, 1);
    assert_eq!(clock(&sim, dodger).since(at), ticks(1));
}

/// A dodger's own scale stack with no room runs it slowed with the world, and says so.
#[test]
fn a_full_clock_stack_leaves_that_dodger_slowed() {
    let (mut sim, attacker, dodger) = duel();
    for _ in 0..4 {
        sim.clock_mut(dodger).unwrap().0.push_scale(ScaleSource::Gameplay, 1000).unwrap();
    }
    let schedule = [(0, attacker, Buttons::HEAVY), (2, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 4);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(sim.slow_motion().refused(), 1);
    assert!(sim.slow_motion().is_running());
    let at = clock(&sim, dodger);
    idle(&mut sim, 1);
    assert_eq!(clock(&sim, dodger).since(at), SimDuration::from_subticks(200));
}

/// Hitstop counts real steps, so a counter's hit in slow motion freezes for as long as it says, both ways.
#[test]
fn hitstop_lasts_its_real_steps_through_slow_motion() {
    let (mut sim, attacker, dodger) = duel_of(moves_with(
        "attack: (damage: 20, weight: Light)",
        "attack: (damage: 20, weight: Light, hitstop: 5)",
    ));
    // Perfect on step 3, the answer on step 4, its volume live from its tick 1, on step 5.
    let schedule =
        [(0, attacker, Buttons::LIGHT), (2, dodger, Buttons::DODGE), (4, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 6);
    assert_eq!(health(&sim, attacker), 80);
    let (a_at, d_at) = (clock(&sim, attacker), clock(&sim, dodger));
    idle(&mut sim, 5);
    assert_eq!(clock(&sim, attacker), a_at, "frozen for all 5");
    assert_eq!(clock(&sim, dodger), d_at, "frozen for all 5");
    idle(&mut sim, 1);
    assert_eq!(clock(&sim, attacker).since(a_at), SimDuration::from_subticks(200), "slowed");
    assert_eq!(clock(&sim, dodger).since(d_at), ticks(1), "at speed");
}

// ------------------------------------------------------------------------------------------------
// The counter window.
// ------------------------------------------------------------------------------------------------

/// A swing on tick 3 of a roll pressed on step 20 fires on step 23, and opens a window of 20 ticks of the
/// dodger's clock, which is at speed, so presses on step 23 to step 42 count, and step 43 doesn't. A press
/// past the roll's end, on step 42, is fine: the window outlasts it. The press on step 23, like any before
/// it, waits for the next step's look at the window, and takes effect from the window's first instant.
#[test]
fn counters_land_only_inside_their_window() {
    #[rustfmt::skip]
    let table = [
        // The press's step, whether it starts a counter, and how far in it reads a step after its press.
        (24, true, 1),
        (30, true, 1),
        (42, true, 1),
        (43, false, 0),
        (50, false, 0),
    ];
    for (at, fires, reads) in table {
        let (mut sim, attacker, dodger) = duel();
        let schedule =
            [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (at, dodger, Buttons::ULTIMATE)];
        run(&mut sim, &schedule, 0, 24);
        assert_eq!(perfect(&sim), 1);
        run(&mut sim, &schedule, 24, at + 1);
        if fires {
            assert_eq!(playing(&sim, dodger), Some(name("riposte")), "pressed on step {at}");
            assert_eq!(elapsed(&sim, dodger), ticks(0), "on its press's own step");
            idle(&mut sim, 1);
            assert_eq!(elapsed(&sim, dodger), ticks(reads));
            assert_eq!(sim.stats().counters_fired, 1, "pressed on step {at}");
        } else {
            assert_ne!(playing(&sim, dodger), Some(name("riposte")), "pressed on step {at}");
            assert_eq!(sim.stats().counters_fired, 0, "pressed on step {at}");
            assert!(sim.actor(dodger).unwrap().moves.counter_closes_at.is_none(), "closed");
            assert_eq!(sim.intents(dodger).unwrap().len(), 1, "the press waits, for nothing");
        }
    }
}

/// A press on the very step a perfect dodge fires, or buffered before it, starts the counter from the
/// window's first instant, so a step later it's a tick in. `answer` buffers 8 ticks, so with the window
/// opening on step 23, a press on step 16 still can and one on step 15 can't.
#[test]
fn a_press_buffered_ahead_of_the_window_fires_as_it_opens() {
    for (at, fires) in [(15, false), (16, true), (22, true), (23, true)] {
        let (mut sim, attacker, dodger) = duel();
        let schedule =
            [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (at, dodger, Buttons::ULTIMATE)];
        run(&mut sim, &schedule, 0, 24);
        assert_eq!(perfect(&sim), 1, "pressed on step {at}");
        assert_eq!(playing(&sim, dodger), Some(name("roll")), "the window opened after this step's cancels");
        idle(&mut sim, 1);
        if fires {
            assert_eq!(playing(&sim, dodger), Some(name("riposte")), "pressed on step {at}");
            assert_eq!(elapsed(&sim, dodger), ticks(1), "from the instant it fired, one step ago");
        } else {
            assert_eq!(playing(&sim, dodger), Some(name("roll")), "pressed on step {at}");
        }
    }
}

/// `when: Instant` takes only a press from inside the window.
#[test]
fn an_instant_counter_takes_only_a_press_inside_its_window() {
    let instant = "counter: (ticks: 20, allow: [(intent: \"answer\", move: \"riposte\", when: Instant)])";
    for (at, fires) in [(22, false), (23, true), (24, true)] {
        let (mut sim, attacker, dodger) = duel_of(moves_with(ROLL_WINDOW, instant));
        let schedule =
            [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (at, dodger, Buttons::ULTIMATE)];
        run(&mut sim, &schedule, 0, 25);
        assert_eq!(playing(&sim, dodger) == Some(name("riposte")), fires, "pressed on step {at}");
    }
}

/// A window that allows a press the dodge's own windows or a neutral start also take beats them at one
/// instant, and they take it again once it's closed. Here `tap` is what all three want. A roll from step
/// 21 fires on step 23, ending on step 43 with its own `tap` window open from step 30.
#[test]
fn a_counter_beats_what_else_the_same_press_could_start() {
    let start = |window_ticks: u32, at: u32| {
        let window =
            format!("counter: (ticks: {window_ticks}, allow: [(intent: \"tap\", move: \"riposte\")])");
        let (mut sim, attacker, dodger) = duel_of(moves_with(ROLL_WINDOW, &window));
        let schedule =
            [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (at, dodger, Buttons::LIGHT)];
        run(&mut sim, &schedule, 0, at + 1);
        (playing(&sim, dodger).unwrap(), sim.stats().counters_fired, sim.stats().cancels_fired)
    };
    let (riposte, tap) = (name("riposte"), name("tap"));
    assert_eq!(start(20, 25), (riposte, 1, 1), "before the roll's own window opens");
    assert_eq!(start(20, 32), (riposte, 1, 1), "inside it, at the same instant");
    assert_eq!(start(20, 42), (riposte, 1, 1), "the counter window's last step");
    assert_eq!(start(20, 43), (tap, 0, 0), "both closed, and the roll over: a neutral start");

    // A window closing early hands the press back to the roll's own, as soon as that's open.
    assert_eq!(start(5, 27), (riposte, 1, 1), "the shorter window's last step");
    assert!(start(5, 28).0 == name("roll"), "closed, and the roll's own window not open yet");
    assert_eq!(start(5, 31), (tap, 0, 1), "the roll's own window");

    // Past the roll's end and still open, a counter beats a neutral start. Fired on step 23 of a roll from
    // step 18, which ends on step 40.
    let window = "counter: (ticks: 20, allow: [(intent: \"tap\", move: \"riposte\")])";
    let (mut sim, attacker, dodger) = duel_of(moves_with(ROLL_WINDOW, window));
    let schedule =
        [(20, attacker, Buttons::LIGHT), (18, dodger, Buttons::DODGE), (41, dodger, Buttons::LIGHT)];
    run(&mut sim, &schedule, 0, 41);
    assert_eq!(playing(&sim, dodger), None, "ended on step 40");
    assert!(sim.actor(dodger).unwrap().moves.counter_closes_at.is_some(), "and the window's still open");
    run(&mut sim, &schedule, 41, 42);
    assert_eq!(playing(&sim, dodger), Some(name("riposte")));
    assert_eq!(elapsed(&sim, dodger), ticks(0));
    assert_eq!(sim.stats().counters_fired, 1);
}

/// A counter window outlasts its dodge, and every sub-tick still belongs to one move. A roll from step 18
/// ends on step 40, and a press on that very step starts the counter there. At 3× a roll from step 20 reads
/// tick 3 on step 21, where it fires, and tick 21 and tick 24 on steps 27 and 28, past its end at tick 22. A
/// press on step 28 is past the roll, so the roll ends first, by itself, and the counter that follows
/// cancels nothing. At 3× the roll's i-frames end while the slowed swing is still live, which it's spent
/// against.
#[test]
fn a_counter_never_reads_a_dodge_past_its_end() {
    let (mut sim, attacker, dodger) = duel();
    let schedule =
        [(20, attacker, Buttons::LIGHT), (18, dodger, Buttons::DODGE), (40, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 41);
    assert_eq!(playing(&sim, dodger), Some(name("riposte")));
    assert_eq!(elapsed(&sim, dodger), ticks(0));
    assert_eq!(
        sim.stats().cancels_fired,
        1,
        "its press lands exactly on the roll's end, which is still a cut"
    );
    assert_eq!(sim.actor(dodger).unwrap().moves.current.unwrap().started_at, SimTime::from_subticks(41_000));

    let longer = "counter: (ticks: 40, allow: [(intent: \"answer\", move: \"riposte\")])";
    let (mut sim, attacker, dodger) = duel_of(moves_with(ROLL_WINDOW, longer));
    sim.clock_mut(dodger).unwrap().0.push_scale(ScaleSource::Gameplay, 3000).unwrap();
    let schedule =
        [(18, attacker, Buttons::LIGHT), (20, dodger, Buttons::DODGE), (28, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 28);
    assert_eq!(perfect(&sim), 1);
    assert_eq!(elapsed(&sim, dodger), ticks(21));
    run(&mut sim, &schedule, 28, 29);
    assert_eq!(playing(&sim, dodger), Some(name("riposte")));
    assert_eq!(sim.stats().counters_fired, 1);
    assert_eq!(sim.stats().cancels_fired, 0, "the roll had already ended by itself");
}

/// Starting any move closes the window: another dodge, whatever a press starts, what a hit forces.
#[test]
fn starting_any_other_move_closes_the_window() {
    // An instant roll from tick 16, on step 37 of a roll from step 21, while the window is open to step 42.
    let (mut sim, attacker, dodger) = duel();
    let schedule = [
        (20, attacker, Buttons::LIGHT),
        (21, dodger, Buttons::DODGE),
        (37, dodger, Buttons::DODGE),
        (38, dodger, Buttons::ULTIMATE),
    ];
    run(&mut sim, &schedule, 0, 37);
    assert!(sim.actor(dodger).unwrap().moves.counter_closes_at.is_some());
    run(&mut sim, &schedule, 37, 38);
    assert_eq!(elapsed(&sim, dodger), ticks(0), "a new roll");
    assert_eq!(sim.actor(dodger).unwrap().moves.counter_closes_at, None);
    run(&mut sim, &schedule, 38, 39);
    assert_eq!(playing(&sim, dodger), Some(name("roll")), "the answer finds nothing to answer");
    assert_eq!(sim.stats().counters_fired, 0);

    // A forced start, as a reaction is, the same.
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 25);
    assert!(sim.actor(dodger).unwrap().moves.counter_closes_at.is_some());
    sim.force_move(dodger, name("flinch"));
    idle(&mut sim, 1);
    assert_eq!(sim.actor(dodger).unwrap().moves.counter_closes_at, None);
    sim.step(&press(dodger, Buttons::ULTIMATE));
    assert_eq!(playing(&sim, dodger), Some(name("flinch")));
    assert_eq!(sim.stats().counters_fired, 0);
}

/// A frozen dodger's window doesn't run down: it's on the dodger's clock. A press through the freeze waits
/// in the ring, on the same frozen time, and fires when the clock moves.
#[test]
fn a_counter_window_waits_through_a_freeze() {
    let (mut sim, attacker, dodger) = duel();
    let schedule = [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE)];
    run(&mut sim, &schedule, 0, 24);
    let closes = sim.actor(dodger).unwrap().moves.counter_closes_at.unwrap();
    assert_eq!(closes, clock(&sim, dodger) + ticks(20));
    sim.clock_mut(dodger).unwrap().0.apply_hitstop(30);
    sim.step(&press(dodger, Buttons::ULTIMATE));
    idle(&mut sim, 29);
    assert_eq!(playing(&sim, dodger), Some(name("roll")), "frozen for all 30, and a window fires nothing");
    assert_eq!(sim.actor(dodger).unwrap().moves.counter_closes_at, Some(closes));
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, dodger), Some(name("riposte")), "the first step that moves");
}

/// A counter lands like any move. It's the whole loop: a dodge, a slowdown, and a hit on what swung.
#[test]
fn a_counter_lands() {
    let (mut sim, attacker, dodger) = duel();
    let schedule =
        [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (24, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 25);
    assert_eq!(elapsed(&sim, dodger), ticks(0));
    assert_eq!(health(&sim, attacker), 100);
    idle(&mut sim, 1);
    assert_eq!(health(&sim, attacker), 80, "riposte_hit, on its tick 1");
    assert_eq!(health(&sim, dodger), 100);
    assert_eq!(sim.hits().events()[0].group, name("riposte_hit"));
}

/// A swing's window runs on its owner's clock, so slow motion stretches it: `tap`'s one live tick lasts five
/// steps at a fifth of the speed. Fired on step 23, slowed from step 24, `tap` is live through its reading
/// of 3.8 ticks on step 27. An answer on step 24 with no i-frames has nothing to fear from it, because the
/// dodge spent it. A second attacker's `sweep`, touching on step 23 too, wasn't the one with the credit, so
/// the same answer takes that hit on its first step, and the reaction replaces it. With i-frames, it doesn't.
#[test]
fn a_counter_needs_i_frames_only_against_another_swing() {
    let (mut sim, attacker, dodger) = duel_of(moves_with("invulnerable: [(from: 0, to: 3)],", ""));
    let schedule =
        [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (24, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 25);
    assert_eq!(sim.actor(attacker).unwrap().moves.volumes.len(), 1, "still live, a step after it fired");
    assert_eq!(playing(&sim, dodger), Some(name("riposte")));
    idle(&mut sim, 1);
    assert_eq!(health(&sim, dodger), 100);
    assert_eq!(playing(&sim, dodger), Some(name("riposte")));

    let answer = |set: MoveSet| {
        let mut sim = Simulation::new(SimConfig::default());
        let moves = Some(sim.add_move_set(set));
        let spawn = ActorSpawn { moves, ..ActorSpawn::default() };
        let (first, second) = (sim.spawn(spawn).unwrap(), sim.spawn(spawn).unwrap());
        let at = Vec3::new(0.0, 0.0, 1.0);
        let dodger = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
        let schedule = [
            (20, second, Buttons::HEAVY),
            (20, first, Buttons::LIGHT),
            (21, dodger, Buttons::DODGE),
            (24, dodger, Buttons::ULTIMATE),
        ];
        run(&mut sim, &schedule, 0, 24);
        assert_eq!(perfect(&sim), 1);
        assert_eq!(sim.hits().record_count(), 1, "the one with the credit, and no more");
        run(&mut sim, &schedule, 24, 25);
        (health(&sim, dodger), playing(&sim, dodger))
    };
    assert_eq!(answer(moves_with("invulnerable: [(from: 0, to: 3)],", "")), (90, Some(name("riposte"))));
    assert_eq!(answer(moves()), (100, Some(name("riposte"))));
}

/// A new revision of the set is what the counter starts from, by name, like any window of an older one,
/// and a revision nothing is playing any more can't keep a window open.
#[test]
fn a_window_reads_the_revision_it_opened_in() {
    // Reload while the roll plays: its revision stays, and the counter starts the newest `riposte`.
    let (mut sim, attacker, dodger) = duel();
    let schedule =
        [(20, attacker, Buttons::LIGHT), (21, dodger, Buttons::DODGE), (26, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 25);
    let id = sim.actor(dodger).unwrap().moves.set.unwrap();
    // A move ahead of the others, so what an index meant has moved.
    let shifted = moves_with(
        "moves: [\n            (\n                // Live on tick 3 alone.",
        "moves: [\n            (name: \"ahead\", startup: 1, active: 1, recovery: 1),\n            (\n                // Live on tick 3 alone.",
    );
    assert_eq!(sim.replace_move_set(id, shifted), Some(1));
    run(&mut sim, &schedule, 25, 27);
    let current = sim.actor(dodger).unwrap().moves.current.unwrap();
    assert_eq!(current.name, name("riposte"));
    assert_eq!(current.at.revision, 1);
    assert_eq!(sim.stats().counters_fired, 1);

    // Reload after the roll has ended: nothing keeps its revision, so the window goes with it.
    let (mut sim, attacker, dodger) = duel();
    let schedule =
        [(20, attacker, Buttons::LIGHT), (18, dodger, Buttons::DODGE), (41, dodger, Buttons::ULTIMATE)];
    run(&mut sim, &schedule, 0, 41);
    assert_eq!(playing(&sim, dodger), None);
    // The attacker's slowed swing is the only thing still reading that revision.
    assert!(sim.despawn(attacker));
    let id = sim.actor(dodger).unwrap().moves.set.unwrap();
    sim.replace_move_set(id, moves());
    run(&mut sim, &schedule, 41, 42);
    assert_eq!(playing(&sim, dodger), None);
    assert_eq!(sim.stats().counters_fired, 0);
}

// ------------------------------------------------------------------------------------------------
// The test content and the replay suite.
// ------------------------------------------------------------------------------------------------

/// The replay suite's `dodge-counter`, read back as what it says it holds: a replay that passes on a
/// script that had stopped doing these would hold nothing.
#[test]
fn the_replay_suites_dodge_counter_plays_what_it_says() {
    let scenario = replay::find_scenario(SCENARIOS, "duel").expect("listed");
    let script = replay::find_script("dodge-counter").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    let (player, opponent) = (cast[0], cast[1]);

    let mut fired = Vec::new();
    let mut counters = Vec::new();
    let mut counter_hits = Vec::new();
    let mut hits_on_player = Vec::new();
    let (mut slowed_steps, mut at_speed_while_slowed) = (0, true);
    let mut last_clock = SimTime::ZERO;
    let mut started = 0;
    let mut plain_answer = None;
    for tick in 0..u64::from(script.ticks) {
        sim.step(&(script.actions)(tick, &cast));
        let view = sim.actor(player).expect("alive");
        if view.moves.started != started {
            started = view.moves.started;
            let current = view.moves.current.expect("just started");
            if current.name == name("dodge_counter") {
                counters.push(tick);
            }
            if tick == 501 {
                plain_answer = Some(current.name);
            }
        }
        for cue in sim.cues() {
            if cue.cue == PERFECT_DODGE {
                assert_eq!(cue.actor, player);
                fired.push(tick);
            }
            if let Some(hit) = cue.hit {
                if hit.target == player {
                    hits_on_player.push(tick);
                }
                if cue.cue == name("hit_counter") {
                    assert_eq!(hit.target, opponent);
                    counter_hits.push(tick);
                }
            }
        }
        if sim.slow_motion().is_running() && view.hitstop_ticks == 0 && view.clock_now != last_clock {
            slowed_steps += 1;
            at_speed_while_slowed &= view.clock_now.since(last_clock) == ticks(1);
        }
        last_clock = view.clock_now;
    }

    assert_eq!(fired, [56, 456, 656], "a perfect dodge, one left unanswered and one out of a dodge cancel");
    assert_eq!(counters, [58, 658], "answered twice");
    assert_eq!(counter_hits, [61, 661], "and both landed, on their third tick");
    assert_eq!(hits_on_player, [256], "the dodge that came too late, and only that");
    assert_eq!(plain_answer, Some(name("light_1")), "an answer a tick after its window closed");
    let stats = sim.stats();
    assert_eq!(stats.perfect_dodges, 3);
    assert_eq!(stats.counters_fired, 2);
    assert!(slowed_steps >= 3 * 20, "{slowed_steps}");
    assert!(at_speed_while_slowed, "the player ran at speed through every slowdown");
    assert_eq!(sim.slow_motion().refused(), 0);
}

/// Pillar 4: every cancel window and perfect-dodge window the test content authors is covered by a
/// committed replay. Each of `tests/replays/` is played as the runner plays it, and a window counts as
/// covered when a move's cut off inside it by the move it allows, or a perfect dodge fires inside it. A
/// counter counts as a window of the dodge that opened it, however far along that dodge has got.
#[test]
fn every_authored_window_of_the_test_content_is_covered_by_a_replay() {
    type Window = (String, u32, String);
    let set = replay::test_moves().expect("compiles");
    let mut cancels: BTreeSet<Window> = BTreeSet::new();
    let mut perfects: BTreeSet<(String, u32)> = BTreeSet::new();
    let mut counters: BTreeSet<(String, String)> = BTreeSet::new();
    for mv in set.moves() {
        for window in &mv.cancels {
            for allow in &window.allow {
                let target = set.find(allow.target_name()).expect("in the set").1.label().to_owned();
                cancels.insert((mv.label().to_owned(), window.span.from, target));
            }
        }
        for window in &mv.perfect_dodge {
            perfects.insert((mv.label().to_owned(), window.span.from));
            for allow in window.counter.iter().flat_map(|counter| &counter.allow) {
                let target = set.find(allow.target_name()).expect("in the set").1.label().to_owned();
                counters.insert((mv.label().to_owned(), target));
            }
        }
    }
    assert!(!perfects.is_empty() && !counters.is_empty());

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the replay suite")
        .map(|entry| entry.expect("a file").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sreplay"))
        .collect();
    files.sort();
    assert!(files.len() >= 6, "{files:?}");
    for file in files {
        let bytes = std::fs::read(&file).expect("reads");
        let replay = Replay::decode(&bytes).expect("decodes");
        // The fights play the shipped moves, which aren't the test content.
        if ["arena-fight", "arena-skirmish", "party", "ward-court"].contains(&replay.scenario()) {
            continue;
        }
        let mut player = Player::new(replay, SCENARIOS).expect("starts");
        let mut last: BTreeMap<ActorId, CurrentMove> = BTreeMap::new();
        let mut opened: BTreeMap<ActorId, Name> = BTreeMap::new();
        while player.step() {
            let sim = player.simulation();
            for cue in sim.cues().filter(|cue| cue.cue == PERFECT_DODGE) {
                let dodge = sim.actor(cue.actor).expect("alive").moves.current.expect("its dodge");
                let (_, mv) = set.find(dodge.name).expect("in the set");
                for window in &mv.perfect_dodge {
                    if window.span.contains(cue.move_time) {
                        perfects.remove(&(mv.label().to_owned(), window.span.from));
                    }
                }
                opened.insert(cue.actor, dodge.name);
            }
            for view in sim.actors() {
                let now = view.moves.current;
                if let (Some(before), Some(now)) = (last.get(&view.id), now)
                    && now.serial != before.serial
                {
                    let (_, mv) = set.find(before.name).expect("in the set");
                    let (_, target) = set.find(now.name).expect("in the set");
                    let into = now.started_at.since(before.started_at);
                    for window in mv.cancels.iter().filter(|window| window.span.contains(into)) {
                        if window.allow.iter().any(|allow| allow.target_name() == now.name) {
                            cancels.remove(&(
                                mv.label().to_owned(),
                                window.span.from,
                                target.label().to_owned(),
                            ));
                        }
                    }
                }
                if let (Some(&dodge), Some(now)) = (opened.get(&view.id), now)
                    && last.get(&view.id).is_none_or(|before| before.serial != now.serial)
                {
                    let (_, mv) = set.find(dodge).expect("in the set");
                    let (_, target) = set.find(now.name).expect("in the set");
                    counters.remove(&(mv.label().to_owned(), target.label().to_owned()));
                    opened.remove(&view.id);
                }
                match now {
                    Some(now) => {
                        last.insert(view.id, now);
                    }
                    None => {
                        last.remove(&view.id);
                    }
                }
            }
        }
    }
    assert!(cancels.is_empty(), "no replay fires these cancel windows: {cancels:#?}");
    assert!(perfects.is_empty(), "no replay fires these perfect-dodge windows: {perfects:#?}");
    assert!(counters.is_empty(), "no replay fires these counters: {counters:#?}");
}
