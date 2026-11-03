//! The move system through the public API: what a player presses against what plays, tick by tick.

mod support;

use sapphire_core::Name;
use sapphire_gameplay::replay;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Buttons, CUE_QUEUE_LEN, CurrentMove, INTENT_RING_LEN,
    MAX_ACTIVE_HURTBOXES, MAX_ACTIVE_VOLUMES, MoveRef, SimConfig, Simulation, Stick, TickActions, WarpTarget,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::{HurtboxState, MoveSet, MoveSetDocument, Phase, RootSample};
use sapphire_time::{ScaleSource, SimDuration, SimTime};
use support::{press, slice};

fn name(text: &str) -> Name {
    Name::intern(text)
}

fn sub(subticks: u64) -> SimDuration {
    SimDuration::from_subticks(subticks)
}

fn ticks(count: u64) -> SimDuration {
    SimDuration::from_ticks(count)
}

fn compile(document: &str) -> MoveSet {
    let document = MoveSetDocument::parse(document).expect("parses");
    document.compile().expect("compiles")
}

/// One actor at the origin, facing +Z, playing `set`.
fn lone(set: MoveSet) -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(set);
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    (sim, actor)
}

fn current(sim: &Simulation, actor: ActorId) -> Option<CurrentMove> {
    sim.actor(actor).expect("alive").moves.current
}

fn current_name(sim: &Simulation, actor: ActorId) -> Option<Name> {
    current(sim, actor).map(|mv| mv.name)
}

/// The moves that started, as (step, move, how far into it the step's reading is, in sub-ticks).
type Trace = Vec<(u64, &'static str, u64)>;

/// What a scenario holds down: these buttons for these steps, `from` up to but not including `to`.
#[derive(Clone, Copy)]
struct Hold(u64, u64, Buttons);

/// A press: one step down, and up again.
const fn tap(at: u64, buttons: Buttons) -> Hold {
    Hold(at, at + 1, buttons)
}

const NAMES: &[&str] =
    &["base", "to_a", "to_b", "to_c", "to_i", "to_h", "first", "same", "light_1", "light_2"];

fn label(name: Name) -> &'static str {
    NAMES.iter().find(|text| Name::new(text) == name).copied().unwrap_or("?")
}

/// Runs a scenario for `steps`, with hitstop applied before the steps given, and traces what starts.
fn trace(set: MoveSet, holds: &[Hold], freezes: &[(u64, u16)], steps: u64) -> Trace {
    let (mut sim, actor) = lone(set);
    let mut started = 0;
    let mut trace = Vec::new();
    for step in 0..steps {
        for &(at, frozen) in freezes {
            if at == step {
                sim.clock_mut(actor).expect("alive").0.apply_hitstop(frozen);
            }
        }
        let mut buttons = Buttons::NONE;
        for &Hold(from, to, held) in holds {
            if (from..to).contains(&step) {
                buttons = buttons | held;
            }
        }
        sim.step(&press(actor, buttons));
        let view = sim.actor(actor).expect("alive").moves;
        if view.started != started {
            started = view.started;
            let mv = view.current.expect("something just started");
            trace.push((step, label(mv.name), mv.elapsed.as_subticks()));
        }
    }
    trace
}

/// Moves that exist to test windows: `base` holds its one cancel window on ticks 10 to 13.
const EDGES: &str = r#"(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [
            (name: "go", button: Light, buffer: 12, priority: 1),
            (name: "a", button: Heavy, buffer: 12, priority: 1),
            (name: "b", button: Dodge, buffer: 5, priority: 2),
            (name: "c", button: Skill, buffer: 12, priority: 1),
            (name: "i", button: Ultimate, buffer: 12, priority: 1),
            (name: "h", button: Jump, buffer: 1),
            (name: "f", button: Swap, buffer: 12),
            (name: "s", button: LockOn, buffer: 12),
        ],
        neutral: [
            (intent: "go", move: "base"),
            (intent: "f", move: "first"),
            (intent: "s", move: "same"),
        ],
        moves: [
            (
                name: "base",
                startup: 10,
                active: 0,
                recovery: 20,
                cancels: [(from: 10, to: 14, allow: [
                    (intent: "a", move: "to_a"),
                    (intent: "b", move: "to_b"),
                    (intent: "c", move: "to_c"),
                    (intent: "i", move: "to_i", when: Instant),
                    (intent: "h", move: "to_h", when: Held),
                ])],
            ),
            (name: "to_a", startup: 1, active: 0, recovery: 9),
            (name: "to_b", startup: 1, active: 0, recovery: 9),
            (name: "to_c", startup: 1, active: 0, recovery: 9),
            (name: "to_i", startup: 1, active: 0, recovery: 9),
            (name: "to_h", startup: 1, active: 0, recovery: 9),
            (
                // Windows on its very first tick and its very last.
                name: "first",
                startup: 1,
                active: 1,
                recovery: 4,
                cancels: [
                    (from: 0, to: 1, allow: [(intent: "a", move: "to_a")]),
                    (from: 5, to: 6, allow: [(intent: "c", move: "to_c")]),
                ],
            ),
            (
                name: "same",
                startup: 5,
                active: 0,
                recovery: 15,
                cancels: [(from: 5, to: 15, allow: [(intent: "a", move: "same")])],
            ),
        ],
    ),
)"#;

/// Every scenario is what a designer would ask of frame data: press this, freeze that, what plays?
#[test]
fn scenarios_of_presses_play_what_they_should() {
    struct Case<'a> {
        what: &'static str,
        holds: &'a [Hold],
        freezes: &'a [(u64, u16)],
        expect: &'a [(u64, &'static str, u64)],
    }
    const GO: Hold = tap(0, Buttons::LIGHT);
    const A: Buttons = Buttons::HEAVY;
    const B: Buttons = Buttons::DODGE;
    const C: Buttons = Buttons::SKILL;
    const I: Buttons = Buttons::ULTIMATE;
    const H: Buttons = Buttons::JUMP;
    const F: Buttons = Buttons::SWAP;
    const S: Buttons = Buttons::LOCK_ON;
    let cases: &[Case<'_>] = &[
        Case {
            what: "a press starts a move on its own step, at the move's first tick",
            holds: &[GO],
            freezes: &[],
            expect: &[(0, "base", 0)],
        },
        Case {
            what: "a buffered press fires exactly as the window opens",
            holds: &[GO, tap(8, A)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_a", 0)],
        },
        Case {
            what: "a press inside the window fires at once",
            holds: &[GO, tap(12, A)],
            freezes: &[],
            expect: &[(0, "base", 0), (12, "to_a", 0)],
        },
        Case {
            what: "a press on the window's last tick still counts",
            holds: &[GO, tap(13, A)],
            freezes: &[],
            expect: &[(0, "base", 0), (13, "to_a", 0)],
        },
        Case {
            what: "a press the tick after the window closes doesn't",
            holds: &[GO, tap(14, A)],
            freezes: &[],
            expect: &[(0, "base", 0)],
        },
        Case {
            what: "a buffer of 5 reaches 4 ticks past its press",
            holds: &[GO, tap(6, B)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_b", 0)],
        },
        Case {
            what: "and expires on the 5th",
            holds: &[GO, tap(5, B)],
            freezes: &[],
            expect: &[(0, "base", 0)],
        },
        Case {
            what: "the higher priority wins between two buffered presses",
            holds: &[GO, tap(8, A), tap(9, B)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_b", 0)],
        },
        Case {
            what: "whichever came first",
            holds: &[GO, tap(9, A), tap(8, B)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_b", 0)],
        },
        Case {
            what: "equal priorities go to the older press",
            holds: &[GO, tap(8, A), tap(9, C)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_a", 0)],
        },
        Case {
            what: "the other way round too",
            holds: &[GO, tap(8, C), tap(9, A)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_c", 0)],
        },
        Case {
            what: "two pressed on one tick go by document order at equal priority",
            holds: &[GO, tap(9, Buttons::HEAVY.union(Buttons::SKILL))],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_a", 0)],
        },
        Case {
            what: "an instant press from before the window stays out of it",
            holds: &[GO, tap(9, I)],
            freezes: &[],
            expect: &[(0, "base", 0)],
        },
        Case {
            what: "an instant press on the window's first tick fires",
            holds: &[GO, tap(10, I)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_i", 0)],
        },
        Case {
            what: "and on its last",
            holds: &[GO, tap(13, I)],
            freezes: &[],
            expect: &[(0, "base", 0), (13, "to_i", 0)],
        },
        Case { what: "and not after", holds: &[GO, tap(14, I)], freezes: &[], expect: &[(0, "base", 0)] },
        Case {
            what: "a hold down as the window opens fires, with no press in it",
            holds: &[GO, Hold(7, 12, H)],
            freezes: &[],
            expect: &[(0, "base", 0), (10, "to_h", 0)],
        },
        Case {
            what: "a hold let go before it opens doesn't",
            holds: &[GO, Hold(7, 10, H)],
            freezes: &[],
            expect: &[(0, "base", 0)],
        },
        Case {
            what: "a hold beginning inside fires at once",
            holds: &[GO, Hold(12, 13, H)],
            freezes: &[],
            expect: &[(0, "base", 0), (12, "to_h", 0)],
        },
        Case {
            what: "a window on the move's first tick takes a press from the step it started",
            holds: &[tap(0, F.union(A))],
            freezes: &[],
            expect: &[(0, "to_a", 0)],
        },
        Case {
            what: "and a press buffered before it",
            holds: &[tap(0, A), tap(1, F)],
            freezes: &[],
            expect: &[(1, "to_a", 0)],
        },
        Case {
            what: "but not one from the tick after",
            holds: &[tap(0, F), tap(1, A)],
            freezes: &[],
            expect: &[(0, "first", 0)],
        },
        Case {
            what: "a window on the move's last tick fires",
            holds: &[tap(0, F), tap(5, C)],
            freezes: &[],
            expect: &[(0, "first", 0), (5, "to_c", 0)],
        },
        Case {
            what: "from a press buffered before it too",
            holds: &[tap(0, F), tap(2, C)],
            freezes: &[],
            expect: &[(0, "first", 0), (5, "to_c", 0)],
        },
        Case {
            what: "but the move's end is past it",
            holds: &[tap(0, F), tap(6, C)],
            freezes: &[],
            expect: &[(0, "first", 0)],
        },
        Case {
            what: "a cancel into the same move starts it over",
            holds: &[tap(0, S), tap(7, A), tap(14, A)],
            freezes: &[],
            expect: &[(0, "same", 0), (7, "same", 0), (14, "same", 0)],
        },
        Case {
            what: "a press for a neutral start waits for the move's end",
            holds: &[GO, tap(25, Buttons::LIGHT)],
            freezes: &[],
            expect: &[(0, "base", 0), (30, "base", 0)],
        },
        Case {
            what: "as long as its buffer lasts",
            holds: &[GO, tap(17, Buttons::LIGHT)],
            freezes: &[],
            expect: &[(0, "base", 0)],
        },
        Case {
            what: "a neutral start on the move's very last tick plays on without a gap",
            holds: &[GO, tap(29, Buttons::LIGHT)],
            freezes: &[],
            expect: &[(0, "base", 0), (30, "base", 0)],
        },
        // Hitstop at step 8 freezes steps 8 to 11, with base's clock stopped at its tick 7. So step
        // 12 reads tick 8, and step 14 reads tick 10.
        Case {
            what: "hitstop delays a window by exactly its length",
            holds: &[GO, tap(7, A)],
            freezes: &[(8, 4)],
            expect: &[(0, "base", 0), (14, "to_a", 0)],
        },
        Case {
            what: "a press during hitstop waits for it to end, and its buffer doesn't run meanwhile",
            holds: &[GO, tap(9, B)],
            freezes: &[(8, 4)],
            expect: &[(0, "base", 0), (14, "to_b", 0)],
        },
        Case {
            what: "a press during hitstop fires on the first step that moves, with all of that step's time",
            holds: &[GO, tap(13, A)],
            freezes: &[(12, 4)],
            expect: &[(0, "base", 0), (16, "to_a", 1000)],
        },
        Case {
            what: "a press during hitstop isn't fired twice by the steps it lasts",
            holds: &[GO, Hold(12, 15, A)],
            freezes: &[(12, 3)],
            expect: &[(0, "base", 0), (15, "to_a", 1000)],
        },
    ];
    for case in cases {
        let got = trace(compile(EDGES), case.holds, case.freezes, 60);
        assert_eq!(got, case.expect, "{}", case.what);
    }
}

/// The plan's pillar 1: a cancel window never drops a buffered input. From every press tick in a
/// move to its window's end, with its buffer long enough, the cancel fires where it should.
#[test]
fn a_cancel_window_never_drops_a_buffered_input() {
    for press_tick in 0..30 {
        let got = trace(slice(), &[tap(0, Buttons::LIGHT), tap(press_tick, Buttons::LIGHT)], &[], 30);
        // Down on tick 0 and on through tick 1 is one press, not two.
        let fires = (2..24).contains(&press_tick);
        let expected: Trace = if fires {
            vec![(0, "light_1", 0), (press_tick.max(10), "light_2", 0)]
        } else if press_tick >= 26 {
            // After the move's end, it starts the string over.
            vec![(0, "light_1", 0), (press_tick, "light_1", 0)]
        } else if press_tick >= 24 {
            // Between the window's end and the move's, it waits for the end.
            vec![(0, "light_1", 0), (26, "light_1", 0)]
        } else {
            vec![(0, "light_1", 0)]
        };
        assert_eq!(got, expected, "pressed on tick {press_tick}");
    }
}

/// The same, with the clock at any speed and the press well before the window: the cancel fires at
/// the window's first tick on the actor's clock, to the sub-tick, every time.
#[test]
fn a_buffered_cancel_fires_at_the_windows_first_instant_at_any_speed() {
    for per_mille in [100, 333, 500, 1000, 1700, 3000, 5000, 9000] {
        let (mut sim, actor) = lone(slice());
        sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, per_mille).expect("room");
        sim.step(&press(actor, Buttons::LIGHT));
        let first = current(&sim, actor).expect("started");
        assert_eq!(first.name, Name::new("light_1"));
        sim.step(&press(actor, Buttons::NONE));
        // Into the buffer, 12 ticks long on this clock. Under a fast clock this step may already be
        // past the window's first tick, and the press lands on its reading.
        sim.step(&press(actor, Buttons::LIGHT));
        let pressed_at = sim.actor(actor).expect("alive").clock_now;
        let opens = first.started_at + ticks(10);
        for _ in 0..200 {
            if current_name(&sim, actor) == Some(Name::new("light_2")) {
                break;
            }
            sim.step(&TickActions::NEUTRAL);
        }
        let second = current(&sim, actor).expect("fired");
        assert_eq!(second.name, Name::new("light_2"), "at {per_mille}‰");
        assert_eq!(second.started_at, opens.max(pressed_at), "at {per_mille}‰");
        assert_eq!(sim.stats().cancels_fired, 1, "at {per_mille}‰");
    }
}

/// The plan's pillar 1: a press reaches a visibly moving frame in at most 3 ticks at 60 Hz. The test
/// content authors 1: the step after the press has moved the root or a path off the first sample, for
/// every move a press can start from nothing.
#[test]
fn a_press_reaches_a_moving_frame_within_three_ticks() {
    let set = slice();
    for allow in set.neutral() {
        let (_, mv) = set.find(allow.target_name()).expect("in the set");
        let (intent, _) = set.intent(allow.intent()).expect("declared");
        let button = set.intents()[intent].button.expect("a player's");

        let (mut sim, actor) = lone(slice());
        let start = sim.actor(actor).expect("alive").position;
        sim.step(&press(actor, Buttons::from(button)));
        assert_eq!(current(&sim, actor).expect("on its own step").name, mv.name());
        assert_eq!(sim.actor(actor).expect("alive").position, start, "nothing's moved on the press's step");

        let mut steps = 0;
        let mut pose = None;
        loop {
            sim.step(&TickActions::NEUTRAL);
            steps += 1;
            let view = sim.actor(actor).expect("alive");
            let moved = view.position != start
                || pose.is_some_and(|pose| pose != view.moves.current.expect("running").reading);
            pose = view.moves.current.map(|mv| mv.reading);
            if moved {
                break;
            }
            assert!(steps <= 3, "`{}` hasn't moved {steps} ticks after its press", mv.label());
        }
        assert_eq!(steps, 1, "`{}` authored to move on the very next tick", mv.label());
    }
}

/// The plan's pillar 1 at Mobile-Low, where two steps run per frame on the same input.
#[test]
fn at_two_steps_a_frame_a_press_lands_exactly_once() {
    // A press seen by both steps of a frame.
    let (mut sim, actor) = lone(slice());
    let light = press(actor, Buttons::LIGHT);
    for _ in 0..2 {
        sim.step(&light);
    }
    assert_eq!(sim.stats().intents_pressed, 1);
    assert_eq!(sim.stats().moves_started, 1);
    assert!(sim.intents(actor).expect("alive").is_empty(), "used, not left to fire again");
    for _ in 0..40 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(sim.stats().moves_started, 1, "and it doesn't come back");

    // A press and release inside one frame, seen by its first step only, or its second.
    for down in [[true, false], [false, true]] {
        let (mut sim, actor) = lone(slice());
        for step_down in down {
            sim.step(&press(actor, if step_down { Buttons::LIGHT } else { Buttons::NONE }));
        }
        assert_eq!(sim.stats().moves_started, 1);
        assert_eq!(current(&sim, actor).expect("started").name, Name::new("light_1"));
    }

    // The whole string, one step per tick or two per frame, gives the same state: frames are nothing
    // to the simulation. The press reaches a moving frame inside the frame it lands in.
    let frames: Vec<Buttons> =
        (0..40).map(|frame| if frame % 6 == 0 { Buttons::LIGHT } else { Buttons::NONE }).collect();
    let run = |per_frame: usize| {
        let (mut sim, actor) = lone(slice());
        for buttons in &frames {
            for _ in 0..per_frame {
                sim.step(&press(actor, *buttons));
            }
        }
        (
            sim.tick(),
            sim.stats().moves_started,
            sim.stats().cancels_fired,
            sim.actor(actor).expect("alive").position,
        )
    };
    let two_a_frame = run(2);
    assert_eq!(two_a_frame.0, 80);
    assert!(two_a_frame.2 >= 2, "the string chained: {two_a_frame:?}");

    let (mut sim, actor) = lone(slice());
    let start = sim.actor(actor).expect("alive").position;
    for _ in 0..2 {
        sim.step(&light);
    }
    assert_ne!(sim.actor(actor).expect("alive").position, start, "the press's own frame shows it moving");
}

/// A scripted string through the whole slice, from RON cooked to bytes: three hits with a buffered
/// cancel, a dodge cancel and a dash attack.
#[test]
fn a_string_a_dodge_cancel_and_a_dash_attack_play_from_a_cooked_document() {
    let cooked = support::slice_cooked();
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_cooked_move_set(&cooked).expect("loads");
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");

    let presses: &[(u64, Buttons)] = &[
        (0, Buttons::LIGHT),  // light_1 from nothing.
        (6, Buttons::LIGHT),  // Buffered 4 ticks ahead of light_1's window, so light_2 at 10.
        (22, Buttons::LIGHT), // Inside light_2's window, on its tick 12: light_3 at once.
        (30, Buttons::DODGE), // Light_3's dodge window is open from its tick 4: a dodge at once.
        (38, Buttons::LIGHT), // The dodge's tick 8, a tick ahead of its window: dash attack at 39.
        (44, Buttons::DODGE), // Dash attack's dodge window is open from its tick 2: a dodge at once.
    ];
    let mut starts = Vec::new();
    let mut cues = Vec::new();
    let mut started = 0;
    for step in 0..100 {
        let buttons =
            presses.iter().find(|(at, _)| *at == step).map_or(Buttons::NONE, |(_, buttons)| *buttons);
        sim.step(&press(actor, buttons));
        let view = sim.actor(actor).expect("alive");
        if view.moves.started != started {
            started = view.moves.started;
            let mv = view.moves.current.expect("just started");
            starts.push((step, mv.name, mv.elapsed));
        }
        for cue in sim.cues() {
            assert_eq!(cue.actor, actor);
            assert_eq!(cue.tick, step + 1, "the world's tick on this step");
            cues.push((step, cue.cue));
        }
    }
    let expected =
        [(0, "light_1"), (10, "light_2"), (22, "light_3"), (30, "dodge"), (39, "dash_attack"), (44, "dodge")]
            .map(|(step, text)| (step, Name::new(text), SimDuration::ZERO));
    assert_eq!(starts, expected);

    // Light_3 starts at 22 with its swing cue at tick 9: the dodge at 30 is a tick ahead of it, so it
    // never fires.
    let expected_cues = [
        (0, "swing_windup"),
        (5, "swing"),
        (16, "swing"), // Light_2's tick 6.
        (30, "dodge_start"),
        (43, "swing"), // The dash attack's tick 4.
        (44, "dodge_start"),
    ]
    .map(|(step, text)| (step, Name::new(text)));
    assert_eq!(cues, expected_cues);

    let stats = sim.stats();
    assert_eq!((stats.moves_started, stats.cancels_fired), (6, 5));
    assert_eq!(stats.intents_pressed, 6);
    assert_eq!((stats.intents_expired, stats.intents_overwritten), (0, 0));
    assert_eq!(stats.allocations, 0);
    // The last dodge ran out, 22 ticks long, and nothing followed.
    assert_eq!(current(&sim, actor), None);
}

/// Everything N3 and T1 read about a move, step by step.
#[test]
fn the_plain_read_follows_a_move_through_its_phases_and_windows() {
    let (mut sim, actor) = lone(slice());
    assert_eq!(current(&sim, actor), None);
    sim.step(&press(actor, Buttons::DODGE));
    let table: &[(u64, Phase, bool, bool, bool)] = &[
        // tick, phase, invulnerable, perfect, cancel window open
        (0, Phase::Startup, false, false, false),
        (1, Phase::Active, true, true, false),
        (6, Phase::Active, true, true, false),
        (7, Phase::Active, true, false, false),
        (8, Phase::Active, true, false, false),
        (9, Phase::Active, true, false, true),
        (13, Phase::Active, true, false, true),
        (14, Phase::Recovery, false, false, true),
        (21, Phase::Recovery, false, false, true),
    ];
    let mut row = 0;
    for tick in 0..22 {
        if tick > 0 {
            sim.step(&TickActions::NEUTRAL);
        }
        let view = sim.actor(actor).expect("alive").moves;
        let mv = view.current.expect("running");
        assert_eq!(mv.tick(), tick);
        assert_eq!(mv.elapsed, ticks(tick));
        assert_eq!(mv.name, Name::new("dodge"));
        assert_eq!(mv.ticks, 22);
        assert_eq!(mv.serial, 1);
        assert_eq!(mv.started_at, SimTime::from_subticks(1000), "the first step's reading");
        assert_eq!(view.started, 1);
        let hurtbox_off = view
            .hurtboxes
            .iter()
            .any(|hurtbox| hurtbox.name == Name::new("body") && hurtbox.state == HurtboxState::Off);
        assert_eq!(hurtbox_off, (1..14).contains(&tick), "tick {tick}");
        if row < table.len() && table[row].0 == tick {
            let (_, phase, invulnerable, perfect, cancel) = table[row];
            assert_eq!(mv.phase, phase, "tick {tick}");
            assert_eq!(view.windows.invulnerable, invulnerable, "tick {tick}");
            assert_eq!(view.windows.perfect_dodge, perfect, "tick {tick}");
            assert_eq!(view.windows.cancel, cancel, "tick {tick}");
            assert!(!view.windows.parry, "tick {tick}");
            row += 1;
        }
    }
    assert_eq!(row, table.len());
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(current(&sim, actor), None, "22 ticks long");
}

/// A hit sweep runs from where the root was before a step's motion to where it is now.
#[test]
fn the_read_says_where_the_root_was_before_this_step() {
    let (mut sim, actor) = lone(slice());
    sim.step(&press(actor, Buttons::DODGE));
    let mut last = sim.actor(actor).expect("alive");
    for _ in 0..20 {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(actor).expect("alive");
        assert_eq!(view.moves.previous.position, last.position);
        assert_eq!(view.moves.previous.yaw, last.yaw);
        last = view;
    }
    assert_ne!(last.position, Vec3::ZERO, "it did move");
}

/// The hitbox track, as D4 will read it: at 1×, exactly the steps that read its ticks, sweeping tick
/// to tick along the table's own path, in the root's space.
#[test]
fn hit_volumes_are_published_on_their_ticks_along_their_paths() {
    let set = slice();
    let (_, mv) = set.find(Name::new("light_1")).expect("in the set");
    let blade = mv.tables.path(Name::new("blade")).expect("in the table").clone();
    let (mut sim, actor) = lone(slice());
    sim.step(&press(actor, Buttons::LIGHT));
    let mut seen = Vec::new();
    for tick in 1..26_u64 {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(actor).expect("alive").moves;
        for volume in view.volumes.iter() {
            assert_eq!(volume.name, Name::new("light_1_hit"));
            assert_eq!(volume.path, Name::new("blade"));
            assert_eq!(volume.serial, 1);
            assert_eq!(volume.shape, sapphire_moves::Shape::Capsule { radius: 0.25, length: 0.9 });
            assert_eq!(volume.start, blade.at(volume.from));
            assert_eq!(volume.end, blade.at(volume.to));
            seen.push((tick, volume.from, volume.to));
        }
    }
    assert_eq!(
        seen,
        [(5, ticks(5), ticks(5)), (6, ticks(5), ticks(6)), (7, ticks(6), ticks(7))],
        "window 5..8 is live on ticks 5, 6 and 7, and sweeps from its first tick to its last"
    );
}

/// A move with an instant at every tick and a one-tick window somewhere for each kind of track, to run
/// at clock speeds that land on, between and far past them.
const TRACKS: &str = r#"(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [(name: "go", button: Light, buffer: 15), (name: "x", button: Heavy, buffer: 15)],
        neutral: [(intent: "go", move: "tracks")],
        moves: [
            (
                name: "tracks",
                startup: 10,
                active: 10,
                recovery: 10,
                motion: (
                    root: [(at: 30, offset: (0.0, 0.0, 3.0), yaw: 0.6)],
                    paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.0)), (at: 30, offset: (3.0, 1.0, 0.0))])],
                ),
                hitboxes: [
                    (name: "long", from: 6, to: 14, path: "blade", shape: Sphere(radius: 0.5)),
                    (name: "one", from: 21, to: 22, path: "blade", shape: Sphere(radius: 0.5)),
                ],
                hurtboxes: [(name: "body", from: 17, to: 18, state: Off)],
                invulnerable: [(from: 3, to: 4)],
                perfect_dodge: [(from: 7, to: 8)],
                parry: [(from: 11, to: 12)],
                cancels: [(from: 24, to: 25, allow: [(intent: "x", move: "after")])],
                cues: [
                    (name: "t0", at: 0), (name: "t1", at: 1), (name: "t2", at: 2), (name: "t3", at: 3),
                    (name: "t9", at: 9), (name: "t10", at: 10), (name: "t20", at: 20), (name: "t29", at: 29),
                ],
            ),
            (name: "after", startup: 1, active: 0, recovery: 5, cues: [(name: "after_t0", at: 0)]),
        ],
    ),
)"#;

const TRACK_CUES: [&str; 8] = ["t0", "t1", "t2", "t3", "t9", "t10", "t20", "t29"];

/// One run of [`TRACKS`] on an actor whose clock runs at this speed.
struct Run {
    cues: Vec<(Name, SimTime)>,
    invulnerable: u32,
    perfect: u32,
    parry: u32,
    hurtbox: u32,
    cancel: u32,
    /// Each volume's sweep, in the order published.
    long: Vec<(SimDuration, SimDuration)>,
    one: Vec<(SimDuration, SimDuration)>,
    position: Vec3,
    yaw: f32,
    started_at: SimTime,
}

/// `frozen_steps` freezes every second step.
fn run_tracks(per_mille: u32, frozen_steps: bool) -> Run {
    let (mut sim, actor) = lone(compile(TRACKS));
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, per_mille).expect("room");
    let mut run = Run {
        cues: Vec::new(),
        invulnerable: 0,
        perfect: 0,
        parry: 0,
        hurtbox: 0,
        cancel: 0,
        long: Vec::new(),
        one: Vec::new(),
        position: Vec3::ZERO,
        yaw: 0.0,
        started_at: SimTime::ZERO,
    };
    let mut buttons = Buttons::LIGHT;
    for step in 0..4000 {
        sim.step(&press(actor, buttons));
        buttons = Buttons::NONE;
        for cue in sim.cues() {
            run.cues.push((cue.cue, cue.local_time));
        }
        let view = sim.actor(actor).expect("alive");
        if let Some(mv) = view.moves.current {
            run.started_at = mv.started_at;
        }
        let windows = view.moves.windows;
        run.invulnerable += u32::from(windows.invulnerable);
        run.perfect += u32::from(windows.perfect_dodge);
        run.parry += u32::from(windows.parry);
        run.cancel += u32::from(windows.cancel);
        run.hurtbox += view.moves.hurtboxes.len() as u32;
        for volume in view.moves.volumes.iter() {
            let sweep = (volume.from, volume.to);
            if volume.name == Name::new("long") {
                run.long.push(sweep);
            } else {
                run.one.push(sweep);
            }
        }
        if view.moves.current.is_none() && view.moves.started > 0 {
            run.position = view.position;
            run.yaw = view.yaw;
            break;
        }
        if frozen_steps && step % 2 == 0 {
            sim.clock_mut(actor).expect("alive").0.apply_hitstop(1);
        }
    }
    run
}

/// Every speed the tests below try: slow, exact, fractional, and many ticks a step.
const SPEEDS: [u32; 11] = [100, 333, 500, 1000, 1700, 2000, 3000, 5000, 7000, 11000, 31000];

/// However the clock runs, an instant fires once, at exactly its time.
#[test]
fn a_cue_fires_once_at_its_exact_time_at_any_speed() {
    for per_mille in SPEEDS {
        let run = run_tracks(per_mille, false);
        let expected: Vec<(Name, SimTime)> = [0, 1, 2, 3, 9, 10, 20, 29]
            .iter()
            .zip(TRACK_CUES)
            .map(|(&tick, text)| (Name::new(text), run.started_at + ticks(tick)))
            .collect();
        assert_eq!(run.cues, expected, "at {per_mille}‰");
    }
}

/// However big the step, a window that opens and closes inside it still counts, and a window's
/// sweep still covers its whole arc.
#[test]
fn a_window_inside_one_big_step_still_counts_at_any_speed() {
    for per_mille in SPEEDS {
        let run = run_tracks(per_mille, false);
        assert!(run.invulnerable >= 1, "at {per_mille}‰");
        assert!(run.perfect >= 1, "at {per_mille}‰");
        assert!(run.parry >= 1, "at {per_mille}‰");
        assert!(run.hurtbox >= 1, "at {per_mille}‰");
        assert!(run.cancel >= 1, "at {per_mille}‰");
        // A one-tick window is live for as long as a reading lies in it, which at 1× is one step.
        if per_mille >= 1000 {
            assert_eq!(run.invulnerable, 1, "at {per_mille}‰");
            assert_eq!(run.perfect, 1, "at {per_mille}‰");
            assert_eq!(run.parry, 1, "at {per_mille}‰");
            assert_eq!(run.hurtbox, 1, "at {per_mille}‰");
            assert_eq!(run.cancel, 1, "at {per_mille}‰");
        } else {
            // However many readings land in it.
            assert!((1000 / per_mille..=1000 / per_mille + 1).contains(&run.invulnerable), "at {per_mille}‰");
        }

        // The arc from tick 6 to tick 13, end to end, however it's cut.
        let (first, last) = (run.long[0], run.long[run.long.len() - 1]);
        assert_eq!(first.0, ticks(6), "at {per_mille}‰");
        assert_eq!(last.1, ticks(13), "at {per_mille}‰");
        for pair in run.long.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "at {per_mille}‰: a gap or an overlap in {:?}", run.long);
        }
        // A one-tick window's arc is one instant.
        assert_eq!(run.one[0], (ticks(21), ticks(21)), "at {per_mille}‰");
        assert!(run.one.iter().all(|sweep| *sweep == run.one[0]), "at {per_mille}‰");
    }
}

/// At exactly 1×, a window counts on the ticks it says and no more.
#[test]
fn at_one_times_speed_a_window_counts_on_exactly_its_ticks() {
    let run = run_tracks(1000, false);
    assert_eq!(run.long.len(), 8, "6..14 is 8 ticks: {:?}", run.long);
    assert_eq!(run.one.len(), 1);
    assert_eq!(run.invulnerable, 1);
}

/// A frozen clock fires nothing twice and loses nothing: hitstop on every other step gives the same
/// cues at the same local times as none at all.
#[test]
fn a_frozen_clock_never_fires_a_track_twice() {
    let plain = run_tracks(1000, false);
    let frozen = run_tracks(1000, true);
    assert_eq!(plain.cues, frozen.cues);
    assert_eq!(plain.position, frozen.position);
    assert_eq!(plain.yaw, frozen.yaw);
    // State stays lit through a freeze, so windows take twice the steps, or a step each if they're
    // only there for one reading.
    assert_eq!(frozen.invulnerable, 2);
    let moving: Vec<_> = frozen.long.iter().copied().filter(|(from, to)| from != to).collect();
    let plain_moving: Vec<_> = plain.long.iter().copied().filter(|(from, to)| from != to).collect();
    assert_eq!(moving, plain_moving, "the same ground covered, with held steps between");
}

/// A cue on the step a clock froze, frozen for long enough to be sure.
#[test]
fn hitstop_on_a_cues_own_tick_fires_it_once() {
    let (mut sim, actor) = lone(compile(TRACKS));
    sim.step(&press(actor, Buttons::LIGHT));
    let cues_now = |sim: &Simulation| sim.cues().map(|cue| cue.cue).collect::<Vec<_>>();
    assert_eq!(cues_now(&sim), [Name::new("t0")]);
    sim.clock_mut(actor).expect("alive").0.apply_hitstop(5);
    for _ in 0..5 {
        sim.step(&TickActions::NEUTRAL);
        assert!(cues_now(&sim).is_empty(), "nothing fires while frozen");
    }
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(cues_now(&sim), [Name::new("t1")]);
}

/// However the clock runs, the root ends up where the table's last sample says.
#[test]
fn a_moves_whole_length_moves_the_root_by_its_table_at_any_speed() {
    for per_mille in SPEEDS {
        let run = run_tracks(per_mille, false);
        assert!(run.position.abs_diff_eq(Vec3::new(0.0, 0.0, 3.0), 1e-4), "{} at {per_mille}‰", run.position);
        assert!((run.yaw - 0.6).abs() < 1e-5, "{} at {per_mille}‰", run.yaw);
    }
}

/// On a clock at 5×, whose readings fall on ticks 0, 5, 10, 15, 20 and 25, the one-tick cancel window
/// on tick 24 lies inside one step, and still fires, at its first instant to the sub-tick.
#[test]
fn a_cancel_window_inside_one_big_step_fires_at_its_opening() {
    // Pressed on a reading of the clock, by how many steps after the move's first, with x's buffer of
    // 15 ticks of that clock.
    let cases: &[(u64, bool)] = &[
        (1, false), // On tick 5: its buffer ends on tick 20, before the window.
        (2, true),  // On tick 10: its buffer ends on tick 25, just after it opens.
        (3, true),
        (4, true),  // On tick 20, the last reading before it.
        (5, true),  // On tick 25: the step it came on passed the window whole.
        (6, false), // On tick 30: too late, and past the move's end anyway.
    ];
    for &(steps, fires) in cases {
        let (mut sim, actor) = lone(compile(TRACKS));
        sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 5000).expect("room");
        sim.step(&press(actor, Buttons::LIGHT));
        let started_at = current(&sim, actor).expect("started").started_at;
        for step in 1..=5 {
            sim.step(&press(actor, if step == steps { Buttons::HEAVY } else { Buttons::NONE }));
            // Readings on ticks 5 to 25: 24 lies in the last's step, with what's left the new move's.
            let mv = current(&sim, actor).expect("playing");
            if step < 5 {
                assert_eq!(mv.name, Name::new("tracks"), "pressed {steps} steps on");
            } else if fires {
                assert_eq!(mv.name, Name::new("after"), "pressed {steps} steps on");
                assert_eq!(mv.started_at, started_at + ticks(24), "pressed {steps} steps on");
                assert_eq!(mv.elapsed, ticks(1), "pressed {steps} steps on");
                // The move it left read up to there, and no further.
                let late = sim.cues().map(|cue| cue.cue).collect::<Vec<_>>();
                assert_eq!(late, [Name::new("after_t0")], "pressed {steps} steps on");
            } else {
                assert_eq!(mv.name, Name::new("tracks"), "pressed {steps} steps on");
            }
        }
        assert_eq!(sim.stats().cancels_fired, u64::from(fires), "pressed {steps} steps on");
    }
}

/// An instant press on the step that passes a window over counts as pressed inside it, and a hold down
/// on that step fires the same: it's the one step the window was there for.
#[test]
fn instant_presses_and_holds_count_on_the_step_that_passes_a_window() {
    let document = |mode: &str, button: &str| {
        compile(&format!(
            r#"(
                type: "sapphire::moves::MoveSetDocument",
                version: 1,
                data: (
                    intents: [(name: "go", button: Light, buffer: 15), (name: "x", button: {button}, buffer: 15)],
                    neutral: [(intent: "go", move: "long")],
                    moves: [
                        (
                            name: "long",
                            startup: 10,
                            active: 10,
                            recovery: 10,
                            cancels: [(from: 22, to: 24, allow: [(intent: "x", move: "after", when: {mode})])],
                        ),
                        (name: "after", startup: 1, active: 0, recovery: 5),
                    ],
                ),
            )"#
        ))
    };
    for mode in ["Instant", "Held"] {
        // Readings on ticks 0, 5, 10, 15, 20 and 25 of the move: the window, 22..24, in the last's step.
        for (press_step, fires) in [(4, false), (5, true)] {
            let (mut sim, actor) = lone(document(mode, "Heavy"));
            sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 5000).expect("room");
            sim.step(&press(actor, Buttons::LIGHT));
            let started_at = current(&sim, actor).expect("started").started_at;
            sim.step(&press(actor, Buttons::NONE));
            for step in 2..=5 {
                sim.step(&press(actor, if step == press_step { Buttons::HEAVY } else { Buttons::NONE }));
            }
            let mv = current(&sim, actor).expect("playing");
            let what = format!("{mode}, pressed on step {press_step}");
            assert_eq!(mv.name == Name::new("after"), fires, "{what}");
            if fires {
                assert_eq!(mv.started_at, started_at + ticks(22), "{what}");
                assert_eq!(mv.elapsed, ticks(3), "the rest of the step is the new move's: {what}");
            }
        }
    }
}

/// A window on the first instant of a move that a big step both started and passed still counts for
/// a hold: nothing has read that instant before.
#[test]
fn a_hold_counts_for_a_window_a_step_started_and_passed() {
    let set = compile(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 15), (name: "x", button: Heavy, buffer: 15)],
                neutral: [(intent: "go", move: "first")],
                moves: [
                    (
                        name: "first",
                        startup: 6,
                        active: 3,
                        recovery: 3,
                        cancels: [(from: 0, to: 2, allow: [(intent: "x", move: "after", when: Held)])],
                    ),
                    (name: "after", startup: 1, active: 0, recovery: 20),
                ],
            ),
        )"#,
    );
    let (mut sim, actor) = lone(set);
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 5000).expect("room");
    // Readings on ticks 0, 5, 10 and 15 of the first playing, which ends on 12.
    sim.step(&press(actor, Buttons::LIGHT));
    let started_at = current(&sim, actor).expect("started").started_at;
    sim.step(&press(actor, Buttons::NONE));
    sim.step(&press(actor, Buttons::LIGHT));
    assert_eq!(current_name(&sim, actor), Some(Name::new("first")));
    sim.step(&press(actor, Buttons::HEAVY));
    let mv = current(&sim, actor).expect("playing");
    assert_eq!(mv.name, Name::new("after"));
    assert_eq!(mv.started_at, started_at + ticks(12), "at the second playing's first instant");
    assert_eq!(mv.elapsed, ticks(3));
}

/// A move ending inside a big step hands the rest of it to a buffered neutral start, at that instant.
#[test]
fn a_moves_end_inside_a_big_step_starts_what_is_buffered() {
    let (mut sim, actor) = lone(compile(EDGES));
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 7000).expect("room");
    sim.step(&press(actor, Buttons::SWAP));
    let first = current(&sim, actor).expect("started");
    assert_eq!(first.name, Name::new("first"));
    // 6 ticks long, with a reading every 7. So the same step ends it, and the press waits for that.
    sim.step(&press(actor, Buttons::LIGHT));
    let mv = current(&sim, actor).expect("started");
    assert_eq!(mv.name, Name::new("base"));
    assert_eq!(mv.started_at, first.started_at + ticks(7), "pressed on this step's reading, after the end");

    // A press from before the end takes the instant it ended, with the rest of the step.
    let (mut sim, actor) = lone(compile(EDGES));
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 4000).expect("room");
    sim.step(&press(actor, Buttons::SWAP));
    let first = current(&sim, actor).expect("started");
    sim.step(&press(actor, Buttons::LIGHT));
    sim.step(&TickActions::NEUTRAL);
    let mv = current(&sim, actor).expect("started");
    assert_eq!(mv.name, Name::new("base"));
    assert_eq!(mv.started_at, first.started_at + ticks(6));
    assert_eq!(mv.elapsed, ticks(2), "8 ticks in, with 6 of them the first move's");
}

/// A press's buffer is exact under any clock: it can fire up to but not at its end.
#[test]
fn a_buffer_ends_exactly_under_any_clock() {
    // Light's buffer is 12 ticks on the owner's clock, and light_1's window opens at its tick 10. So a
    // press 2 ticks before the move can just reach it, and 3 can't.
    for per_mille in [1000, 2000, 5000] {
        let (mut sim, actor) = lone(compile(EDGES));
        sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, per_mille).expect("room");
        sim.step(&press(actor, Buttons::HEAVY));
        let pressed_at = sim.intents(actor).expect("alive").iter().next().expect("buffered").pressed_at;
        // Start `base` on the next step, a window 10 ticks into it, at `pressed_at + 12` or past it.
        sim.step(&press(actor, Buttons::LIGHT));
        let mv = current(&sim, actor).expect("started");
        let opens = mv.started_at + ticks(10);
        for _ in 0..80 {
            if current(&sim, actor).is_some_and(|mv| mv.name == Name::new("to_a")) {
                break;
            }
            sim.step(&press(actor, Buttons::NONE));
        }
        let fired = current(&sim, actor).is_some_and(|mv| mv.name == Name::new("to_a"));
        assert_eq!(fired, opens < pressed_at + ticks(12), "at {per_mille}‰");
    }
}

/// A hit path means nothing without its root: this is what [`sapphire_gameplay::ActorTransform::point`]
/// is for. A frame yaw turns the table's motion, and one set at a fractional scale still reads its
/// table exactly, bit for bit, at the clock's whole sub-ticks.
#[test]
fn a_fractional_time_scale_reaches_the_table_through_its_carry() {
    let set = slice();
    let table = set.find(Name::new("light_3")).expect("in the set").1.tables.clone();
    let (mut sim, actor) = lone(slice());
    assert!(sim.force_move(actor, Name::new("light_3")));
    sim.step(&TickActions::NEUTRAL);
    let start = current(&sim, actor).expect("forced").started_at;
    // World 200‰ × local 333‰ is 66.6 sub-ticks a tick.
    sim.world_clock_mut().0.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Gameplay, 333).expect("room");
    let elapsed_then = current(&sim, actor).expect("running").elapsed.as_subticks();
    for n in 1..=500_u64 {
        sim.step(&TickActions::NEUTRAL);
        let mv = current(&sim, actor).expect("running");
        // Whole sub-ticks by the floor of the exact product, which a float can't drift from.
        let expected = sub(elapsed_then + n * 666 / 10);
        assert_eq!(mv.elapsed, expected, "step {n}");
        assert_eq!(sim.actor(actor).expect("alive").clock_now, start + expected, "step {n}");
        let reading = table.root_at(expected);
        assert_eq!(
            mv.reading.translation.map(f32::to_bits),
            reading.translation.map(f32::to_bits),
            "step {n}"
        );
        assert_eq!(mv.reading.yaw.to_bits(), reading.yaw.to_bits(), "step {n}");
    }
}

/// Hitstop counts real ticks, so under slow motion it still lasts its steps, and the reading holds its
/// exact bits while it does.
#[test]
fn hitstop_holds_under_a_time_scale() {
    let (mut sim, actor) = lone(slice());
    sim.step(&press(actor, Buttons::LIGHT));
    sim.step(&TickActions::NEUTRAL);
    let elapsed = |sim: &Simulation| current(sim, actor).expect("running").elapsed.as_subticks();
    assert_eq!(elapsed(&sim), 1000);
    sim.world_clock_mut().0.push_scale(ScaleSource::PerfectDodge, 200).expect("room");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(elapsed(&sim), 1200);
    sim.clock_mut(actor).expect("alive").0.apply_hitstop(6);
    let reading = current(&sim, actor).expect("running").reading;
    for frozen in (0..6).rev() {
        sim.step(&TickActions::NEUTRAL);
        assert_eq!(elapsed(&sim), 1200);
        assert_eq!(current(&sim, actor).expect("running").reading, reading);
        assert_eq!(sim.actor(actor).expect("alive").hitstop_ticks, frozen);
    }
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(elapsed(&sim), 1400);
}

/// Warp reaches its stand-off from a place the table alone wouldn't: for a still target, exactly the
/// same point, whatever the clock does.
#[test]
fn warp_reaches_a_still_target_at_any_speed() {
    // 1.1 m short of the target along the line, with the table's own 2.6 m in it.
    let target = Vec3::new(1.5, 0.0, 3.5);
    let mut ends = Vec::new();
    for per_mille in [250, 500, 1000, 1700, 3000, 7000] {
        let (mut sim, actor) = lone(slice());
        sim.set_warp_target(actor, WarpTarget::Point(target));
        sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, per_mille).expect("room");
        sim.force_move(actor, Name::new("dash_attack"));
        // Up to its window's end at tick 8 on its clock, in whichever step reaches it.
        loop {
            sim.step(&TickActions::NEUTRAL);
            let mv = current(&sim, actor).expect("running");
            if mv.elapsed >= ticks(8) {
                // Past the window's end the table alone moves it. Take that back out.
                let view = sim.actor(actor).expect("alive");
                let table = slice().find(Name::new("dash_attack")).expect("in the set").1.tables.clone();
                let beyond = table.root_at(mv.elapsed).offset() - table.root_at(ticks(8)).offset();
                ends.push((per_mille, view.position - det::rotate_y(beyond, mv.frame_yaw), mv.frame_yaw));
                break;
            }
        }
    }
    let (_, reference, yaw) = ends[0];
    let distance = (target - reference).length();
    assert!((distance - 1.1).abs() < 1e-3, "stopped {distance} m from the target");
    assert!((det::yaw_of_direction(target - reference) - yaw).abs() < 1e-3, "and facing it: yaw {yaw}");
    for (per_mille, end, end_yaw) in &ends {
        assert!(end.abs_diff_eq(reference, 1e-3), "{end} against {reference} at {per_mille}‰");
        assert!((end_yaw - yaw).abs() < 1e-3, "{end_yaw} against {yaw} at {per_mille}‰");
    }
}

/// One move, `ticks` long, whose root walks there and partly back and has two warp windows, the first as
/// long as `window`, for a target `offset` from the start.
fn winding(ticks: u32, window: u32) -> MoveSet {
    let half = ticks / 2;
    let recovery = ticks - 1;
    compile(&format!(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 8, priority: 1)],
                neutral: [(intent: "go", move: "wind")],
                moves: [(
                    name: "wind",
                    startup: 0,
                    active: 1,
                    recovery: {recovery},
                    motion: (root: [
                        (at: {half}, offset: (1.0, 0.0, 2.0), yaw: 0.0),
                        (at: {ticks}, offset: (0.5, 0.0, 1.0), yaw: 0.0),
                    ]),
                    warps: [
                        (from: 0, to: {window}, max_distance: 9.0, max_turn: 2.0, keep: 1.0),
                        (from: {window}, to: {last}, max_distance: 9.0, max_turn: 2.0, keep: 1.0),
                    ],
                )],
            ),
        )"#,
        last = window + 10,
    ))
}

/// Where a target stands for [`winding`]'s windows to warp toward.
const WINDING_TARGET: Vec3 = Vec3::new(4.0, 0.0, 9.0);

/// Where the root stands at the end of the first window under a clock at `per_mille`, with `edit` a new
/// revision of the set at its first step in the window.
fn warped_end(set: MoveSet, window: u32, per_mille: u32, edit: Option<MoveSet>) -> Vec3 {
    let (mut sim, actor) = lone(set.clone());
    sim.set_warp_target(actor, WarpTarget::Point(WINDING_TARGET));
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, per_mille).expect("room");
    sim.force_move(actor, Name::new("wind"));
    sim.step(&TickActions::NEUTRAL);
    if let Some(edit) = edit {
        let id = sim.actor(actor).expect("alive").moves.set.expect("it has a set");
        sim.tune_move_set(id, edit).expect("it takes it");
    }
    loop {
        let mv = current(&sim, actor).expect("running");
        if mv.elapsed >= ticks(u64::from(window)) {
            let view = sim.actor(actor).expect("alive");
            let table = &set.find(Name::new("wind")).expect("in the set").1.tables;
            let beyond =
                table.root_at(mv.elapsed).offset() - table.root_at(ticks(u64::from(window))).offset();
            return view.position - det::rotate_y(beyond, mv.frame_yaw);
        }
        sim.step(&TickActions::NEUTRAL);
    }
}

/// A window long enough to read a root's path from its total goes where a walk does, at any speed, and
/// so does a move still running on the revision it began in while a new one takes over: each revision
/// has a path of its own.
#[test]
fn a_long_warp_window_ends_where_a_short_one_does() {
    for window in [40, 120, 121, 900, 3500] {
        let set = winding(3590, window);
        let tuned = winding(1000, 300);
        let ends = [
            ("at 1×", warped_end(set.clone(), window, 1000, None)),
            ("at 0.3×", warped_end(set.clone(), window, 300, None)),
            ("at 1.7×", warped_end(set.clone(), window, 1700, None)),
            ("through an edit", warped_end(set.clone(), window, 1000, Some(tuned))),
        ];
        for (what, end) in ends {
            // Its stand-off from the target, whatever the table was doing.
            let distance = Vec3::new(WINDING_TARGET.x - end.x, 0.0, WINDING_TARGET.z - end.z).length();
            assert!((distance - 1.0).abs() < 1e-2, "window {window} {what}: {distance} m away, at {end}");
        }
    }
}

/// A warp window can open partway through one big step. The table's motion up to its edge is walked in
/// the frame the move had; only what follows turns.
#[test]
fn a_warp_window_opening_inside_a_big_step_bends_only_from_its_edge() {
    let set = compile(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 12)],
                moves: [(
                    name: "dash",
                    startup: 6,
                    active: 6,
                    recovery: 6,
                    motion: (root: [(at: 12, offset: (0.0, 0.0, 2.4))]),
                    warps: [(from: 4, to: 12, max_distance: 0.0, max_turn: 1.0, keep: 0.0)],
                )],
            ),
        )"#,
    );
    let table = set.find(Name::new("dash")).expect("in the set").1.tables.clone();
    let (mut sim, actor) = lone(set);
    sim.set_warp_target(actor, WarpTarget::Point(Vec3::new(40.0, 0.0, 0.0)));
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 6000).expect("room");
    sim.force_move(actor, Name::new("dash"));
    sim.step(&TickActions::NEUTRAL);

    let view = sim.actor(actor).expect("alive");
    let mv = view.moves.current.expect("running");
    assert_eq!(mv.elapsed, ticks(6));
    assert!(mv.frame_yaw > 0.0, "it turned toward the target: {}", mv.frame_yaw);
    assert_eq!(view.yaw, mv.frame_yaw);
    let before = table.root_at(ticks(4)).offset();
    let after = table.root_at(ticks(6)).offset() - before;
    let expected = before + det::rotate_y(after, mv.frame_yaw);
    assert!(view.position.abs_diff_eq(expected, 1e-6), "{} against {expected}", view.position);
}

/// With nothing to aim at, warp leaves the table alone, exactly.
#[test]
fn without_a_target_warp_does_nothing() {
    let table = slice().find(Name::new("dash_attack")).expect("in the set").1.tables.clone();
    let (mut sim, actor) = lone(slice());
    sim.force_move(actor, Name::new("dash_attack"));
    // A forced start takes the whole of its first step.
    for tick in 1..=10 {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(actor).expect("alive");
        assert_eq!(view.moves.current.expect("running").elapsed, ticks(tick));
        assert_eq!(view.position, table.root_at(ticks(tick)).offset(), "tick {tick}");
    }
}

/// A target past the limits gets as close as they allow, and no closer.
#[test]
fn warp_stays_within_its_limits() {
    let (max_distance, max_turn) = (2.0, 1.2); // As the test content authors them.
    // Behind and far away: more than it may turn, and further than it may go.
    let target = Vec3::new(-30.0, 0.0, -30.0);
    let (mut sim, actor) = lone(slice());
    sim.set_warp_target(actor, WarpTarget::Point(target));
    sim.force_move(actor, Name::new("dash_attack"));
    let table = slice().find(Name::new("dash_attack")).expect("in the set").1.tables.clone();
    let mut before = sim.actor(actor).expect("alive").position;
    let mut warped_distance = 0.0;
    let mut warped_turn = 0.0_f32;
    // A forced start takes the whole of its first step, so this reads ticks 1 to 8.
    for tick in 1..=8 {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(actor).expect("alive");
        let mv = view.moves.current.expect("running");
        assert_eq!(mv.tick(), tick);
        let moved_by_table = det::rotate_y(
            table.root_at(ticks(tick)).offset() - table.root_at(ticks(tick - 1)).offset(),
            mv.frame_yaw,
        );
        warped_distance += (view.position - before - moved_by_table).length();
        warped_turn = warped_turn.max(mv.frame_yaw.abs());
        before = view.position;
    }
    assert!((warped_turn - max_turn).abs() < 1e-3, "turned {warped_turn}");
    assert!(warped_distance <= max_distance + 1e-3, "warped {warped_distance} m");
    assert!(warped_distance > max_distance - 0.05, "and used it: {warped_distance} m");
}

/// A hit path is in the root's space, so it rides what warp did to the root, sample for sample.
#[test]
fn hit_paths_ride_the_warp() {
    let target = Vec3::new(1.5, 0.0, 3.5);
    let (mut plain, plain_actor) = lone(slice());
    plain.force_move(plain_actor, Name::new("dash_attack"));
    let (mut warped, warped_actor) = lone(slice());
    warped.set_warp_target(warped_actor, WarpTarget::Point(target));
    warped.force_move(warped_actor, Name::new("dash_attack"));
    let mut checked = 0;
    // A forced start takes the whole of its first step, so this reads ticks 1 to 8.
    for tick in 1..=8 {
        plain.step(&TickActions::NEUTRAL);
        warped.step(&TickActions::NEUTRAL);
        let (a, b) = (plain.actor(plain_actor).expect("alive"), warped.actor(warped_actor).expect("alive"));
        assert_eq!(a.moves.volumes.is_empty(), !(4..8).contains(&tick), "tick {tick}");
        assert_eq!(b.moves.volumes.is_empty(), !(4..8).contains(&tick), "tick {tick}");
        if (4..8).contains(&tick) {
            let (first, second) = (
                a.moves.volumes.iter().next().expect("active"),
                b.moves.volumes.iter().next().expect("active"),
            );
            assert_eq!(first.end, second.end, "the path reads the same in root space");
            let transform = |view: &sapphire_gameplay::ActorView| sapphire_gameplay::ActorTransform {
                position: view.position,
                yaw: view.yaw,
            };
            let world_plain = transform(&a).point(first.end.offset());
            let world_warped = transform(&b).point(second.end.offset());
            let expected = b.position + det::rotate_y(second.end.offset(), b.yaw);
            assert_eq!(world_warped, expected);
            assert_ne!(world_warped, world_plain, "and ends up somewhere else in the world");
            checked += 1;
        }
    }
    assert_eq!(checked, 4);
}

/// A target that moves stays tracked: warp re-aims every step, reading where it stood when the step
/// began, whichever actor goes first.
#[test]
fn warp_tracks_another_actor() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(slice());
    let enemy_at = Vec3::new(0.5, 0.0, 4.0);
    let enemy = sim
        .spawn(ActorSpawn { position: enemy_at, moves: Some(moves), ..ActorSpawn::default() })
        .expect("room");
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    sim.set_warp_target(actor, WarpTarget::Actor(enemy));
    sim.force_move(actor, Name::new("dash_attack"));
    // The enemy dashes forward 2.6 m by its own tick 8, keeping out of reach.
    sim.force_move(enemy, Name::new("dash_attack"));
    for _ in 0..8 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(actor).expect("alive");
    assert_eq!(view.moves.current.expect("running").elapsed, ticks(8));
    let enemy_position = sim.actor(enemy).expect("alive").position;
    // It closes on where the enemy stood a step ago, and that's moved past where it started.
    assert!(enemy_position.z > enemy_at.z);
    let chased = (view.position - enemy_position).length();
    let table_alone = 2.6_f32;
    assert!(view.position.z > table_alone, "at {}", view.position);
    assert!(chased < (Vec3::new(0.0, 0.0, table_alone) - enemy_position).length(), "{chased}");
    // A gone target is no target.
    assert!(sim.despawn(enemy));
    let before = sim.actor(actor).expect("alive").position;
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.actor(actor).expect("alive").position.is_finite());
    assert_ne!(sim.actor(actor).expect("alive").position, before);
}

/// A warp cancelled partway stops bending at once: it applies only while its move's in its window.
#[test]
fn a_cancel_stops_a_warp() {
    let (mut sim, actor) = lone(slice());
    sim.set_warp_target(actor, WarpTarget::Point(Vec3::new(0.0, 0.0, 9.0)));
    sim.force_move(actor, Name::new("dash_attack"));
    for _ in 0..3 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert!(sim.actor(actor).expect("alive").moves.windows.warp);
    sim.step(&press(actor, Buttons::DODGE));
    let view = sim.actor(actor).expect("alive");
    assert_eq!(view.moves.current.expect("cancelled").name, Name::new("dodge"));
    assert!(!view.moves.windows.warp, "tick 3 belongs to the dodge");
    let from = view.position;
    sim.step(&TickActions::NEUTRAL);
    let view = sim.actor(actor).expect("alive");
    let table = slice().find(Name::new("dodge")).expect("in the set").1.tables.clone();
    let moved = view.position - from;
    assert!(
        moved.abs_diff_eq(table.root_at(ticks(1)).offset(), 1e-5),
        "the dodge's own motion and no more: {moved}"
    );
}

/// A move starts on the yaw the root has reached, so what it does happens in its own forward.
#[test]
fn a_move_runs_in_the_frame_of_the_root_it_started_from() {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(slice());
    let yaw = 0.75;
    let actor = sim.spawn(ActorSpawn { yaw, moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let table = slice().find(Name::new("skill")).expect("in the set").1.tables.clone();
    let last = table.root_at(ticks(42));
    sim.step(&press(actor, Buttons::SKILL));
    assert_eq!(current(&sim, actor).expect("started").frame_yaw, yaw);
    for _ in 0..42 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(actor).expect("alive");
    assert_eq!(view.moves.current, None);
    assert!(view.position.abs_diff_eq(det::rotate_y(last.offset(), yaw), 1e-4), "{}", view.position);
    assert!((view.yaw - (yaw + last.yaw)).abs() < 1e-5, "{}", view.yaw);
}

/// A move a cancel starts takes the yaw the root reached at that instant, the old move's turning on
/// that very step included, whether the cancel lands on a step's reading or inside it.
#[test]
fn a_cancel_starts_in_the_yaw_the_old_move_turned_to() {
    let document = r#"(
        type: "sapphire::moves::MoveSetDocument",
        version: 1,
        data: (
            intents: [(name: "go", button: Light, buffer: 12), (name: "x", button: Heavy, buffer: 12)],
            neutral: [(intent: "go", move: "turn")],
            moves: [
                (
                    name: "turn",
                    startup: 6,
                    active: 0,
                    recovery: 0,
                    motion: (root: [(at: 6, offset: (0.0, 0.0, 0.0), yaw: 1.2)]),
                    cancels: [(from: 3, to: 6, allow: [(intent: "x", move: "walk")])],
                ),
                (
                    name: "walk",
                    startup: 6,
                    active: 0,
                    recovery: 0,
                    motion: (root: [(at: 6, offset: (0.0, 0.0, 1.2))]),
                ),
            ],
        ),
    )"#;
    for per_mille in [1000, 5000] {
        let (mut sim, actor) = lone(compile(document));
        sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, per_mille).expect("room");
        sim.step(&press(actor, Buttons::LIGHT | Buttons::HEAVY));
        for _ in 0..8 {
            sim.step(&press(actor, Buttons::LIGHT | Buttons::HEAVY));
            if current_name(&sim, actor) == Some(Name::new("walk")) {
                break;
            }
        }
        let view = sim.actor(actor).expect("alive");
        let mv = view.moves.current.expect("running");
        assert_eq!(mv.name, Name::new("walk"), "at {per_mille}‰");
        assert!(
            (mv.frame_yaw - 0.6).abs() < 1e-6,
            "halfway through the turn: {} at {per_mille}‰",
            mv.frame_yaw
        );
        assert!((view.yaw - 0.6).abs() < 1e-6, "{} at {per_mille}‰", view.yaw);
        let walked = det::rotate_y(Vec3::new(0.0, 0.0, 0.2 * mv.tick() as f32), 0.6);
        assert!(
            view.position.abs_diff_eq(walked, 1e-5),
            "{} against {walked} at {per_mille}‰",
            view.position
        );
    }
}

/// The whole of a clock's time goes to one move or the next, with nothing lost at a cancel: chained at
/// the same sub-tick, the root ends where each table says, in turn.
#[test]
fn a_cancel_hands_over_the_root_without_losing_any_motion() {
    let tables = |text: &str| slice().find(Name::new(text)).expect("in the set").1.tables.clone();
    let (light, light_2) = (tables("light_1"), tables("light_2"));
    let (mut sim, actor) = lone(slice());
    sim.step(&press(actor, Buttons::LIGHT));
    sim.step(&press(actor, Buttons::NONE));
    sim.step(&press(actor, Buttons::LIGHT));
    for _ in 3..=15 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(actor).expect("alive");
    let mv = view.moves.current.expect("running");
    assert_eq!(mv.name, Name::new("light_2"));
    assert_eq!(mv.tick(), 5);
    // Light_1 to its tick 10, the cancel's instant, then light_2's first 5 ticks.
    let expected = light.root_at(ticks(10)).offset() + light_2.root_at(ticks(5)).offset();
    assert!(view.position.abs_diff_eq(expected, 1e-6), "{} against {expected}", view.position);
}

/// A step can start several moves, through windows on their first ticks, and each hands over at one
/// instant. A chain that could spin is cut off, counted, and goes on next step.
#[test]
fn a_chain_of_cancels_in_one_step_ends() {
    let set = compile(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 15)],
                neutral: [(intent: "go", move: "loop")],
                moves: [(
                    name: "loop",
                    startup: 1,
                    active: 0,
                    recovery: 9,
                    cancels: [(from: 0, to: 1, allow: [(intent: "go", move: "loop")])],
                    cues: [(name: "again", at: 0)],
                )],
            ),
        )"#,
    );
    let (mut sim, actor) = lone(set);
    // More presses buffered than a step can start moves from: each uses one up.
    for _ in 0..12 {
        assert!(sim.press_intent(actor, Name::new("go")));
    }
    sim.step(&TickActions::NEUTRAL);
    let stats = sim.stats();
    assert_eq!(stats.moves_started, 8);
    assert_eq!(stats.transition_limit_steps, 1);
    assert_eq!(sim.intents(actor).expect("alive").len(), 4);
    // Each cut off on its first instant, so only the one that's left reads its first tick.
    assert_eq!(sim.cues().len(), 1);
    // That one has now read past the window that would take the rest.
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().moves_started, 8);
    assert_eq!(current(&sim, actor).expect("playing").serial, 8);
}

/// A brain's intents go through the same system as a player's buttons, whichever it presses.
#[test]
fn intents_dont_need_a_button() {
    let set = compile(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "lunge", buffer: 8, priority: 4)],
                neutral: [(intent: "lunge", move: "lunge")],
                moves: [(
                    name: "lunge",
                    startup: 6,
                    active: 2,
                    recovery: 10,
                    motion: (root: [(at: 8, offset: (0.0, 0.0, 1.0))]),
                )],
            ),
        )"#,
    );
    let (mut sim, actor) = lone(set);
    assert!(!sim.press_intent(actor, Name::new("not declared")));
    assert!(sim.press_intent(actor, Name::new("lunge")));
    let press = *sim.intents(actor).expect("alive").iter().next().expect("buffered");
    assert_eq!(press.priority, 4);
    assert_eq!(press.expires_at, press.pressed_at + ticks(8));
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(current_name(&sim, actor), Some(Name::new("lunge")));
    assert!(sim.intents(actor).expect("alive").is_empty());
}

/// What a press looks like in the ring, for T1's view.
#[test]
fn a_press_is_plain_data_in_the_ring() {
    let (mut sim, actor) = lone(slice());
    sim.step(&TickActions::NEUTRAL);
    let held = ActorActions { buttons: Buttons::LIGHT, stick: Stick::CENTER };
    let mut actions = TickActions::NEUTRAL;
    actions.set(actor, held).expect("room");
    // Into the middle of a dodge, where light isn't allowed until tick 9.
    sim.step(&press(actor, Buttons::DODGE));
    sim.step(&actions);
    let ring = *sim.intents(actor).expect("alive");
    assert_eq!(ring.held(), 1, "light is intent 0, and its hold shows");
    let press_read = *ring.iter().next().expect("buffered");
    assert_eq!(press_read.name, Name::new("light"));
    assert_eq!(press_read.priority, 1);
    assert_eq!(press_read.pressed_at, SimTime::from_subticks(3000));
    assert_eq!(press_read.step_start, SimTime::from_subticks(2000));
    assert_eq!(press_read.expires_at, SimTime::from_subticks(15_000));
    sim.step(&actions);
    assert_eq!(sim.intents(actor).expect("alive").len(), 1, "holding isn't pressing");
    assert_eq!(sim.stats().intents_pressed, 2);
}

/// A press that nothing takes is dropped when its buffer ends, and counted.
#[test]
fn expiry_is_counted() {
    let (mut sim, actor) = lone(slice());
    sim.force_move(actor, Name::new("light_3"));
    sim.step(&TickActions::NEUTRAL);
    sim.step(&press(actor, Buttons::SKILL));
    let window: u64 = 10;
    for _ in 1..window {
        sim.step(&TickActions::NEUTRAL);
        assert_eq!(sim.intents(actor).expect("alive").len(), 1);
    }
    sim.step(&TickActions::NEUTRAL);
    assert!(sim.intents(actor).expect("alive").is_empty());
    assert_eq!(sim.stats().intents_expired, 1);
}

/// A full ring gives up its oldest press and keeps the rest in order, the newest last.
#[test]
fn a_full_ring_gives_up_its_oldest() {
    let set = compile(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "x", buffer: 15)],
                moves: [(name: "idle", startup: 1, active: 0, recovery: 0)],
            ),
        )"#,
    );
    let (mut sim, actor) = lone(set);
    // Slow enough that none of these expires.
    sim.clock_mut(actor).expect("alive").0.push_scale(ScaleSource::Debug, 100).expect("room");
    let mut pressed = Vec::new();
    for _ in 0..=INTENT_RING_LEN {
        sim.step(&TickActions::NEUTRAL);
        assert!(sim.press_intent(actor, Name::new("x")));
        pressed.push(sim.actor(actor).expect("alive").clock_now);
    }
    let ring: Vec<_> = sim.intents(actor).expect("alive").iter().map(|press| press.pressed_at).collect();
    assert_eq!(ring, pressed[1..], "all but the first, in order");
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.stats().intents_pressed, INTENT_RING_LEN as u64 + 1);
    assert_eq!(sim.stats().intents_overwritten, 1);
}

/// More hit volumes or hurtbox changes than a step's lists hold are counted, and the ones that fit
/// are those authored first.
#[test]
fn a_step_counts_the_volumes_it_drops() {
    let hitboxes: String = (0..MAX_ACTIVE_VOLUMES + 2)
        .map(|index| {
            format!(r#"(name: "hit_{index}", from: 0, to: 5, path: "blade", shape: Sphere(radius: 0.5)),"#)
        })
        .collect();
    let hurtboxes: String = (0..MAX_ACTIVE_HURTBOXES + 3)
        .map(|index| format!(r#"(name: "hurt_{index}", from: 0, to: 5, state: Off),"#))
        .collect();
    let set = compile(&format!(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 12)],
                neutral: [(intent: "go", move: "busy")],
                moves: [(
                    name: "busy",
                    startup: 10,
                    active: 0,
                    recovery: 0,
                    motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.0))])]),
                    hitboxes: [{hitboxes}],
                    hurtboxes: [{hurtboxes}],
                )],
            ),
        )"#
    ));
    let (mut sim, actor) = lone(set);
    sim.step(&press(actor, Buttons::LIGHT));
    let view = sim.actor(actor).expect("alive");
    assert_eq!(view.moves.volumes.len(), MAX_ACTIVE_VOLUMES);
    assert_eq!(view.moves.hurtboxes.len(), MAX_ACTIVE_HURTBOXES);
    assert_eq!(view.moves.volumes.iter().next().expect("published").name, Name::new("hit_0"));
    assert_eq!(view.moves.hurtboxes.iter().next().expect("published").name, Name::new("hurt_0"));
    assert_eq!(sim.stats().volumes_dropped, 5);
}

/// Cues past a step's room are counted, not fatal, and the step's room is back the next.
#[test]
fn a_full_cue_queue_counts_what_it_drops() {
    let cues: String = (0..CUE_QUEUE_LEN + 6).map(|_| r#"(name: "c", at: 0),"#).collect();
    let set = compile(&format!(
        r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [(name: "go", button: Light, buffer: 1)],
                neutral: [(intent: "go", move: "loud")],
                moves: [(name: "loud", startup: 1, active: 0, recovery: 4, cues: [{cues}])],
            ),
        )"#
    ));
    let (mut sim, actor) = lone(set);
    sim.step(&press(actor, Buttons::LIGHT));
    assert_eq!(sim.cues().len(), CUE_QUEUE_LEN);
    assert_eq!(sim.stats().cues_published, CUE_QUEUE_LEN as u64);
    assert_eq!(sim.stats().cues_dropped, 6);
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.cues().len(), 0, "one step's cues at a time");
}

/// A forced start needs no window and waits for no clock, and only what's in the set can start.
#[test]
fn a_forced_start_needs_no_window() {
    let (mut sim, actor) = lone(slice());
    sim.step(&press(actor, Buttons::LIGHT));
    for _ in 0..3 {
        sim.step(&TickActions::NEUTRAL);
    }
    sim.clock_mut(actor).expect("alive").0.apply_hitstop(3);
    sim.force_move(actor, Name::new("skill"));
    sim.step(&TickActions::NEUTRAL);
    let mv = current(&sim, actor).expect("forced");
    assert_eq!(mv.name, Name::new("skill"));
    assert_eq!(mv.elapsed, SimDuration::ZERO, "frozen, and on its first tick");
    assert_eq!(sim.cues().map(|cue| cue.cue).collect::<Vec<_>>(), [Name::new("skill_flash")]);

    assert!(sim.force_move(actor, Name::new("no such move")));
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(current_name(&sim, actor), Some(Name::new("skill")));
    assert_eq!(sim.cues().len(), 0, "and its first tick's cue isn't fired twice");
}

/// A set replaced mid-move: what's running finishes on what it started with, and the next move reads
/// the new one. A cancel from an old window finds its target by name.
#[test]
fn a_replaced_set_takes_effect_at_the_next_moves_start() {
    let longer = |document: &mut MoveSetDocument| {
        // Light_1 a tick longer, and light_2 two.
        document.moves[0].recovery += 1;
        document.moves[1].recovery += 2;
    };
    let mut document = MoveSetDocument::parse(support::SLICE).expect("parses");
    longer(&mut document);
    let replacement = document.compile().expect("compiles");

    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(slice());
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    let hash_before = sim.tables_hash();
    sim.step(&press(actor, Buttons::LIGHT));
    let running = current(&sim, actor).expect("started");
    assert_eq!((running.at.revision, running.ticks), (0, 26));

    assert_eq!(sim.replace_move_set(moves, replacement.clone()), Some(1));
    assert_ne!(sim.tables_hash(), hash_before, "the hash follows what the next move reads");
    assert_eq!(sim.move_library().current_revision(moves), Some(1));
    let old = MoveRef { set: moves, revision: 0, index: running.at.index };
    assert!(sim.move_library().get(old).is_some(), "kept while something plays it");

    // It finishes on its old length.
    sim.step(&press(actor, Buttons::NONE));
    assert_eq!(current(&sim, actor).expect("running").ticks, 26);
    assert_eq!(current(&sim, actor).expect("running").at.revision, 0);

    // A window of the old revision starts the new revision's move, by name.
    sim.step(&press(actor, Buttons::LIGHT));
    for _ in 3..=10 {
        sim.step(&TickActions::NEUTRAL);
    }
    let next = current(&sim, actor).expect("cancelled");
    assert_eq!(next.name, Name::new("light_2"));
    assert_eq!((next.at.revision, next.ticks), (1, 32), "the new one's two extra ticks");

    // With nothing playing it any more, the old revision goes at the next replace.
    assert_eq!(sim.replace_move_set(moves, replacement), Some(2));
    assert!(sim.move_library().get(old).is_none());
    assert!(sim.move_library().get(next.at).is_some(), "but not the one still playing");
    assert_eq!(sim.move_library().current_revision(moves), Some(2));
}

/// A replacement that removes a move an old window names: that cancel just doesn't fire.
#[test]
fn a_cancel_to_a_move_a_replacement_removed_doesnt_fire() {
    let mut document = MoveSetDocument::parse(support::SLICE).expect("parses");
    document.moves.retain(|mv| mv.name != "light_2" && mv.name != "dash_attack");
    for mv in &mut document.moves {
        for window in &mut mv.cancels {
            window.allow.retain(|allow| allow.target != "light_2" && allow.target != "dash_attack");
        }
        mv.cancels.retain(|window| !window.allow.is_empty());
    }
    let replacement = document.compile().expect("compiles");
    let (mut sim, actor) = lone(slice());
    let moves = sim.actor(actor).expect("alive").moves.set.expect("set");
    sim.step(&press(actor, Buttons::LIGHT));
    sim.replace_move_set(moves, replacement);
    sim.step(&press(actor, Buttons::NONE));
    sim.step(&press(actor, Buttons::LIGHT));
    for _ in 3..=12 {
        sim.step(&TickActions::NEUTRAL);
    }
    assert_eq!(current_name(&sim, actor), Some(Name::new("light_1")), "no light_2 to go to");
    assert_eq!(sim.stats().cancels_fired, 0);
}

#[test]
fn replacing_a_set_this_simulation_never_gave_is_refused() {
    let (mut sim, actor) = lone(slice());
    let moves = sim.actor(actor).expect("alive").moves.set.expect("set");
    assert_eq!(sim.replace_move_set(moves, slice()), Some(1));
    let other = {
        let mut sim = Simulation::new(SimConfig::default());
        sim.add_move_set(slice());
        sim.add_move_set(slice())
    };
    assert_eq!(sim.replace_move_set(other, slice()), None);
}

/// An actor gone mid-move leaves nothing, and the entity it had comes back clean.
#[test]
fn despawning_mid_move_leaves_nothing_behind() {
    let mut sim = Simulation::new(SimConfig { max_actors: 1, ..SimConfig::default() });
    let moves = sim.add_move_set(slice());
    let first = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    sim.step(&press(first, Buttons::LIGHT));
    sim.step(&TickActions::NEUTRAL);
    sim.step(&press(first, Buttons::LIGHT));
    assert!(!sim.intents(first).expect("alive").is_empty(), "a press waiting");
    assert!(current(&sim, first).is_some());
    let at = Vec3::new(9.0, 0.0, 0.0);
    sim.set_warp_target(first, WarpTarget::Point(at));

    assert!(sim.despawn(first));
    assert!(sim.actor(first).is_none());
    assert!(sim.intents(first).is_none());
    sim.step(&press(first, Buttons::LIGHT));
    assert_eq!(sim.cues().len(), 0, "no one left to publish");

    let second =
        sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("the same entity");
    let view = sim.actor(second).expect("alive");
    assert_eq!(view.moves.current, None);
    assert_eq!(view.moves.started, 0);
    assert_eq!(view.moves.volumes.len(), 0);
    assert!(sim.intents(second).expect("alive").is_empty());
    assert_eq!(view.moves.root_delta.translation, Vec3::ZERO);
    sim.step(&TickActions::NEUTRAL);
    assert_eq!(sim.actor(second).expect("alive").position, Vec3::ZERO, "it isn't warping anywhere");
    assert_eq!(current(&sim, second), None, "its predecessor's press didn't follow it");
}

/// An actor with no moves stands there, whatever is pressed.
#[test]
fn an_actor_without_moves_ignores_intents() {
    let mut sim = Simulation::new(SimConfig::default());
    let actor = sim.spawn(ActorSpawn::default()).expect("room");
    sim.step(&press(actor, Buttons::LIGHT));
    let view = sim.actor(actor).expect("alive");
    assert_eq!(view.moves.current, None);
    assert!(sim.intents(actor).expect("alive").is_empty());
    assert!(!sim.press_intent(actor, Name::new("light")));
    assert_eq!(sim.stats().intents_pressed, 0);
}

/// The replay suite's fights play what they're there to hold: `string` the buffered string, and
/// `dodge-dash` the dodge cancels, the dash attacks with their warps and the instant dodge. Its second
/// dodge goes through the opponent's swing, so the light press after it is a counter, ahead of the dash
/// attack the same press would otherwise start on tick 53. A replay that passes on a script that had
/// stopped doing these would hold nothing.
#[test]
fn the_replay_suites_scripts_play_what_they_say() {
    struct Played {
        starts: Vec<(u64, Name)>,
        warp_ticks: u32,
        opponent_started: u32,
    }
    fn play(script: &str) -> Played {
        let scenario = replay::find_scenario(replay::SCENARIOS, "duel").expect("listed");
        let script = replay::find_script(script).expect("listed");
        let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
        let mut played = Played { starts: Vec::new(), warp_ticks: 0, opponent_started: 0 };
        let mut started = 0;
        for tick in 0..u64::from(script.ticks) {
            sim.step(&(script.actions)(tick, &cast));
            let view = sim.actor(cast[0]).expect("alive");
            if view.moves.started != started {
                started = view.moves.started;
                played.starts.push((tick, view.moves.current.expect("just started").name));
            }
            played.warp_ticks += u32::from(view.moves.windows.warp);
            played.opponent_started = sim.actor(cast[1]).expect("alive").moves.started;
        }
        played
    }
    let names =
        |rows: &[(u64, &str)]| rows.iter().map(|&(at, text)| (at, Name::new(text))).collect::<Vec<_>>();

    let string = play("string");
    assert_eq!(
        string.starts[..4],
        names(&[(0, "light_1"), (10, "light_2"), (22, "light_3"), (96, "light_1")])
    );
    // The string and nothing else, every time. It reaches the opponent by its third round, and hitstop
    // then costs it a press here and there.
    let string_only = [name("light_1"), name("light_2"), name("light_3")];
    assert!(string.starts.iter().all(|(_, started)| string_only.contains(started)), "{:?}", string.starts);
    assert!(string.starts.len() >= 3 * 6, "600 ticks hold 6.25 periods: {:?}", string.starts);
    assert!(string.opponent_started > 0, "and the opponent reacts");

    let fight = play("dodge-dash");
    assert_eq!(
        fight.starts[..10],
        names(&[
            (0, "light_1"),
            (10, "light_2"),
            (22, "light_3"),
            (30, "dodge"),
            (39, "dash_attack"),
            (44, "dodge"),
            (50, "dodge_counter"),
            (120, "dodge"),
            (138, "dodge"),
            (170, "skill"),
        ]),
    );
    assert_eq!(fight.starts[10], (240, Name::new("light_1")), "and it repeats");
    assert!(fight.warp_ticks >= 3 * 2 * 3, "dash attacks warped on {} ticks", fight.warp_ticks);
    assert!(fight.opponent_started >= 10, "the opponent moves: {}", fight.opponent_started);
}

/// The test content's numbers, as a designer reads them: what the report states.
#[test]
fn the_test_contents_frame_data_is_what_it_says() {
    let set = slice();
    let row = |text: &str| {
        let (_, mv) = set.find(name(text)).expect("in the set");
        (mv.startup, mv.active, mv.recovery)
    };
    assert_eq!(row("light_1"), (5, 3, 18));
    assert_eq!(row("light_2"), (6, 4, 20));
    assert_eq!(row("light_3"), (9, 5, 28));
    assert_eq!(row("dodge"), (1, 13, 8));
    assert_eq!(row("dash_attack"), (4, 4, 20));
    assert_eq!(row("skill"), (8, 12, 22));
    assert_eq!(RootSample::ZERO, set.moves()[0].tables.root_samples()[0]);
    assert_eq!(sub(1000), ticks(1));
}
