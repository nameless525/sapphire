//! Multi-finger sequences as tables: each runs its rows in order and reports every row that fails.

use std::fmt::Write as _;

use sapphire_platform::{TouchDeviceId, TouchEvent, WindowId};

use super::*;
use sapphire_ui::{Insets, UiMetrics};

/// A phone-shaped frame.
fn layout() -> HudLayout {
    HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()))
}

fn finger(id: u64) -> FingerId {
    FingerId::new(TouchDeviceId::from_raw(1), id)
}

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

/// Places the tables name: resolved from the layout, so they hold if it moves.
#[derive(Clone, Copy)]
struct Spots {
    stick: Vec2,
    attack: Vec2,
    dodge: Vec2,
    skill: Vec2,
    /// Open frame on the right, on no control.
    open: Vec2,
}

fn spots() -> Spots {
    let layout = layout();
    let spots = Spots {
        stick: layout.stick.rest,
        attack: layout.attack.center,
        dodge: layout.dodge.center,
        skill: layout.skill.center,
        open: Vec2::new(420.0, 120.0),
    };
    assert!(layout.stick.zone.contains(spots.stick));
    assert!(!layout.stick.zone.contains(spots.open));
    spots
}

/// What the state should be after an update; `None` isn't checked.
#[derive(Clone, Copy, Debug, Default)]
struct Expect {
    stick: Option<Vec2>,
    active: Option<bool>,
    camera: Option<Vec2>,
    /// Exactly these are down.
    down: Option<&'static [Button]>,
    /// Exactly these events, in order: which, what and when in ms.
    events: Option<&'static [(Button, ButtonEventKind, u64)]>,
    fingers: Option<usize>,
}

enum Row {
    Down(u64, Vec2),
    /// Like `Down`, but it must not be taken... which no landing is; for moves and ups below.
    Move(u64, Vec2),
    Up(u64, Vec2),
    Cancel(u64),
    /// Focus or the foreground is lost.
    Lost,
    /// A touch that finds no finger: not the controls' to take.
    Late(u64, TouchPhase),
    Update(Expect),
}

/// Runs `rows`, each at its time in ms, and returns every mismatch.
fn run(name: &str, rows: &[(u64, Row)]) -> String {
    let mut controls = TouchControls::new(layout());
    let mut failures = String::new();
    for (index, (at, row)) in rows.iter().enumerate() {
        let at = ms(*at);
        match row {
            Row::Down(id, p) => {
                if !controls.touch(at, finger(*id), TouchPhase::Down, *p) {
                    let _ = writeln!(failures, "{name} row {index}: a landing wasn't taken");
                }
            }
            Row::Move(id, p) => {
                if !controls.touch(at, finger(*id), TouchPhase::Moved, *p) {
                    let _ = writeln!(failures, "{name} row {index}: a known finger's move wasn't taken");
                }
            }
            Row::Up(id, p) => {
                if !controls.touch(at, finger(*id), TouchPhase::Up, *p) {
                    let _ = writeln!(failures, "{name} row {index}: a known finger's lift wasn't taken");
                }
            }
            Row::Cancel(id) => {
                if !controls.touch(at, finger(*id), TouchPhase::Canceled, Vec2::ZERO) {
                    let _ = writeln!(failures, "{name} row {index}: a known finger's cancel wasn't taken");
                }
            }
            Row::Lost => controls.release_all(at),
            Row::Late(id, phase) => {
                if controls.touch(at, finger(*id), *phase, Vec2::ZERO) {
                    let _ = writeln!(failures, "{name} row {index}: a forgotten finger's event was taken");
                }
            }
            Row::Update(expect) => {
                let fingers = controls.fingers();
                let actions = controls.update(at);
                check(name, index, expect, actions, fingers, &mut failures);
            }
        }
    }
    failures
}

fn check(
    name: &str,
    index: usize,
    expect: &Expect,
    actions: &TouchActions,
    fingers: usize,
    failures: &mut String,
) {
    let mut fail = |what: String| {
        let _ = writeln!(failures, "{name} row {index}: {what}");
    };
    if let Some(stick) = expect.stick
        && actions.stick.value.distance(stick) > 1e-3
    {
        fail(format!("stick {} expected {stick}", actions.stick.value));
    }
    if let Some(active) = expect.active
        && actions.stick.active != active
    {
        fail(format!("stick active {} expected {active}", actions.stick.active));
    }
    if let Some(camera) = expect.camera
        && actions.camera_delta.distance(camera) > 1e-3
    {
        fail(format!("camera {} expected {camera}", actions.camera_delta));
    }
    if let Some(down) = expect.down {
        for button in Button::ALL {
            if actions.button(button).down != down.contains(&button) {
                fail(format!("{button:?} down {} expected {down:?}", actions.button(button).down));
            }
        }
    }
    if let Some(events) = expect.events {
        let got: Vec<_> = actions.events.iter().map(|event| (event.button, event.kind, event.at)).collect();
        let want: Vec<_> = events.iter().map(|&(button, kind, at)| (button, kind, ms(at))).collect();
        if got != want {
            fail(format!("events {got:?} expected {want:?}"));
        }
    }
    if let Some(expected) = expect.fingers
        && fingers != expected
    {
        fail(format!("{fingers} fingers followed, expected {expected}"));
    }
}

fn update(expect: Expect) -> Row {
    Row::Update(expect)
}

#[test]
fn every_multi_finger_table_holds() {
    use Button::{Attack, Dodge, Skill};
    use ButtonEventKind::{Canceled, Held, Pressed, Released};
    let Spots { stick, attack, dodge, skill, open } = spots();
    // The stick's value with the thumb `by` (screen points) from its landing, in a frame's layout.
    let radius = layout().stick.radius;
    let value =
        |by: Vec2| stick_value(Vec2::new(by.x, -by.y) / radius, TouchTuning::default().stick_dead_zone);

    let tables: Vec<(&str, Vec<(u64, Row)>)> = vec![
        (
            "both thumbs at once: the stick and a button don't touch each other",
            vec![
                (0, Row::Down(1, stick)),
                (1, Row::Down(2, attack)),
                (
                    2,
                    update(Expect {
                        active: Some(true),
                        down: Some(&[Attack]),
                        events: Some(&[(Attack, Pressed, 1)]),
                        fingers: Some(2),
                        ..Expect::default()
                    }),
                ),
                (3, Row::Move(1, stick + Vec2::new(26.0, 0.0))),
                (
                    4,
                    update(Expect {
                        stick: Some(value(Vec2::new(26.0, 0.0))),
                        down: Some(&[Attack]),
                        events: Some(&[]),
                        ..Expect::default()
                    }),
                ),
                (5, Row::Up(2, attack)),
                (
                    6,
                    update(Expect {
                        stick: Some(value(Vec2::new(26.0, 0.0))),
                        down: Some(&[]),
                        events: Some(&[(Attack, Released, 5)]),
                        ..Expect::default()
                    }),
                ),
                (7, Row::Up(1, stick)),
                (
                    8,
                    update(Expect {
                        active: Some(false),
                        stick: Some(Vec2::ZERO),
                        fingers: Some(0),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "a thumb that slides off a button keeps it, and takes no other on the way",
            vec![
                (0, Row::Down(1, dodge)),
                (
                    1,
                    update(Expect {
                        down: Some(&[Dodge]),
                        events: Some(&[(Dodge, Pressed, 0)]),
                        ..Expect::default()
                    }),
                ),
                // Across the attack button and far past it.
                (2, Row::Move(1, attack)),
                (3, Row::Move(1, attack + Vec2::new(300.0, 300.0))),
                (4, update(Expect { down: Some(&[Dodge]), events: Some(&[]), ..Expect::default() })),
                (5, Row::Up(1, attack + Vec2::new(300.0, 300.0))),
                (
                    6,
                    update(Expect {
                        down: Some(&[]),
                        events: Some(&[(Dodge, Released, 5)]),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "the camera's finger never becomes the stick, and the stick stays free for another",
            vec![
                (0, Row::Down(1, open)),
                (1, Row::Move(1, stick)),
                (2, Row::Move(1, stick + Vec2::new(30.0, -30.0))),
                (
                    3,
                    update(Expect {
                        active: Some(false),
                        stick: Some(Vec2::ZERO),
                        camera: Some(Vec2::new(stick.x + 30.0 - open.x, -(stick.y - 30.0 - open.y))),
                        ..Expect::default()
                    }),
                ),
                // Next update, nothing moved: no delta left over.
                (4, update(Expect { camera: Some(Vec2::ZERO), ..Expect::default() })),
                (5, Row::Down(2, stick)),
                (6, Row::Move(2, stick + Vec2::new(0.0, -26.0))),
                (7, Row::Move(1, stick + Vec2::new(10.0, -30.0))),
                (
                    8,
                    update(Expect {
                        active: Some(true),
                        stick: Some(value(Vec2::new(0.0, -26.0))),
                        camera: Some(Vec2::new(-20.0, 0.0)),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "the stick's finger never turns the camera, wherever it goes",
            vec![
                (0, Row::Down(1, stick)),
                (1, Row::Move(1, open)),
                (2, update(Expect { camera: Some(Vec2::ZERO), active: Some(true), ..Expect::default() })),
            ],
        ),
        (
            "landings fall through to what's free, and past that take nothing",
            vec![
                (0, Row::Down(1, stick)),
                (1, Row::Down(2, attack)),
                // A second on the stick's zone turns the camera.
                (2, Row::Down(3, stick + Vec2::new(4.0, 0.0))),
                (3, Row::Move(3, stick + Vec2::new(14.0, 0.0))),
                (
                    4,
                    update(Expect {
                        camera: Some(Vec2::new(10.0, 0.0)),
                        fingers: Some(3),
                        ..Expect::default()
                    }),
                ),
                // A second on a held button takes nothing, and its lift lets go of nothing.
                (5, Row::Down(4, attack)),
                (6, Row::Up(4, attack)),
                (
                    7,
                    update(Expect {
                        down: Some(&[Attack]),
                        events: Some(&[]),
                        fingers: Some(3),
                        ..Expect::default()
                    }),
                ),
                // A second camera finger takes nothing.
                (8, Row::Down(5, open)),
                (9, Row::Move(5, open + Vec2::new(50.0, 0.0))),
                (10, update(Expect { camera: Some(Vec2::ZERO), fingers: Some(4), ..Expect::default() })),
            ],
        ),
        (
            "three buttons at once, each its own edges",
            vec![
                (0, Row::Down(1, attack)),
                (1, Row::Down(2, dodge)),
                (2, Row::Down(3, skill)),
                (
                    3,
                    update(Expect {
                        down: Some(&[Attack, Dodge, Skill]),
                        events: Some(&[(Attack, Pressed, 0), (Dodge, Pressed, 1), (Skill, Pressed, 2)]),
                        ..Expect::default()
                    }),
                ),
                (4, Row::Up(2, dodge)),
                (
                    5,
                    update(Expect {
                        down: Some(&[Attack, Skill]),
                        events: Some(&[(Dodge, Released, 4)]),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "the OS cancels one finger in the middle: only what it held ends, nothing fires",
            vec![
                (0, Row::Down(1, stick)),
                (1, Row::Down(2, attack)),
                (2, Row::Down(3, open)),
                (
                    3,
                    update(Expect {
                        events: Some(&[(Attack, Pressed, 1)]),
                        fingers: Some(3),
                        ..Expect::default()
                    }),
                ),
                (4, Row::Move(1, stick + Vec2::new(26.0, 0.0))),
                (5, Row::Move(3, open + Vec2::new(5.0, 0.0))),
                (6, Row::Cancel(2)),
                (7, Row::Move(3, open + Vec2::new(9.0, 0.0))),
                (
                    8,
                    update(Expect {
                        // The other two carry on.
                        active: Some(true),
                        stick: Some(value(Vec2::new(26.0, 0.0))),
                        camera: Some(Vec2::new(9.0, 0.0)),
                        down: Some(&[]),
                        events: Some(&[(Attack, Canceled, 6)]),
                        fingers: Some(2),
                    }),
                ),
                // Its release, if one comes, finds nobody.
                (9, Row::Late(2, TouchPhase::Up)),
                (10, update(Expect { events: Some(&[]), fingers: Some(2), ..Expect::default() })),
                // The camera's finger canceled keeps the motion before it, and the stick still holds.
                (11, Row::Move(3, open + Vec2::new(12.0, 0.0))),
                (12, Row::Cancel(3)),
                (
                    13,
                    update(Expect {
                        camera: Some(Vec2::new(3.0, 0.0)),
                        active: Some(true),
                        fingers: Some(1),
                        ..Expect::default()
                    }),
                ),
                // The stick's canceled returns it to zero.
                (14, Row::Cancel(1)),
                (
                    15,
                    update(Expect {
                        active: Some(false),
                        stick: Some(Vec2::ZERO),
                        fingers: Some(0),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "focus is lost: everything lets go with nothing fired, every finger is forgotten",
            vec![
                (0, Row::Down(1, stick)),
                (1, Row::Down(2, attack)),
                (2, Row::Down(3, open)),
                (3, Row::Move(3, open + Vec2::new(6.0, 0.0))),
                (4, Row::Lost),
                (
                    5,
                    update(Expect {
                        active: Some(false),
                        stick: Some(Vec2::ZERO),
                        // What the camera took stays for this update.
                        camera: Some(Vec2::new(6.0, 0.0)),
                        down: Some(&[]),
                        events: Some(&[(Attack, Pressed, 1), (Attack, Canceled, 4)]),
                        fingers: Some(0),
                    }),
                ),
                (6, Row::Late(1, TouchPhase::Moved)),
                (7, Row::Late(2, TouchPhase::Up)),
                (8, Row::Late(3, TouchPhase::Canceled)),
                (9, update(Expect { camera: Some(Vec2::ZERO), events: Some(&[]), ..Expect::default() })),
                // A finger landing after takes what's free, with a fresh start.
                (10, Row::Down(1, attack)),
                (
                    11,
                    update(Expect {
                        down: Some(&[Attack]),
                        events: Some(&[(Attack, Pressed, 10)]),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "a press and a release between two updates both count, in order",
            vec![
                (0, Row::Down(1, skill)),
                (1, Row::Up(1, skill)),
                (2, Row::Down(1, skill)),
                (3, Row::Up(1, skill)),
                (
                    4,
                    update(Expect {
                        down: Some(&[]),
                        events: Some(&[
                            (Skill, Pressed, 0),
                            (Skill, Released, 1),
                            (Skill, Pressed, 2),
                            (Skill, Released, 3),
                        ]),
                        ..Expect::default()
                    }),
                ),
                (5, update(Expect { events: Some(&[]), ..Expect::default() })),
            ],
        ),
        (
            "a hold fires at its exact instant, however frames fall, once a press",
            vec![
                (0, Row::Down(1, attack)),
                (100, update(Expect { events: Some(&[(Attack, Pressed, 0)]), ..Expect::default() })),
                (299, update(Expect { events: Some(&[]), ..Expect::default() })),
                (333, update(Expect { events: Some(&[(Attack, Held, 300)]), ..Expect::default() })),
                (500, update(Expect { events: Some(&[]), ..Expect::default() })),
                (600, Row::Up(1, attack)),
                (601, update(Expect { events: Some(&[(Attack, Released, 600)]), ..Expect::default() })),
            ],
        ),
        (
            "a hold that came due inside a frame goes before the release that ended it",
            vec![
                (0, Row::Down(1, attack)),
                (450, Row::Up(1, attack)),
                (
                    451,
                    update(Expect {
                        events: Some(&[(Attack, Pressed, 0), (Attack, Held, 300), (Attack, Released, 450)]),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "a quick tap holds nothing, and neither does the dodge",
            vec![
                (0, Row::Down(1, attack)),
                (1, Row::Down(2, dodge)),
                (100, Row::Up(1, attack)),
                (
                    1000,
                    update(Expect {
                        events: Some(&[(Attack, Pressed, 0), (Dodge, Pressed, 1), (Attack, Released, 100)]),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
        (
            "a finger landing again whose release never came ends what it held first",
            vec![
                (0, Row::Down(1, attack)),
                (5, Row::Down(1, dodge)),
                (
                    6,
                    update(Expect {
                        down: Some(&[Dodge]),
                        events: Some(&[(Attack, Pressed, 0), (Attack, Canceled, 5), (Dodge, Pressed, 5)]),
                        fingers: Some(1),
                        ..Expect::default()
                    }),
                ),
            ],
        ),
    ];
    let failures: String = tables.iter().map(|(name, rows)| run(name, rows)).collect();
    assert!(failures.is_empty(), "\n{failures}");
}

#[test]
fn a_held_button_reports_where_its_thumb_went() {
    let layout = layout();
    let mut controls = TouchControls::new(layout);
    let attack = layout.attack.center;
    controls.touch(ms(0), finger(1), TouchPhase::Down, attack);
    let state = *controls.update(ms(1)).button(Button::Attack);
    assert!(state.over && !state.dragging && state.pressed && state.down);
    assert_eq!(state.drag, Vec2::ZERO);
    assert_eq!(state.held_for(ms(21)), Some(Duration::from_millis(21)));

    controls.touch(ms(2), finger(1), TouchPhase::Moved, attack + Vec2::new(6.0, -3.0));
    let state = *controls.update(ms(3)).button(Button::Attack);
    assert!(!state.dragging, "still inside the slop");
    assert!(!state.pressed, "the press was the last update's");
    assert_eq!(state.drag, Vec2::new(6.0, 3.0), "+y up");

    // Up and past the hit circle: a drag, off the button, still held.
    controls.touch(ms(4), finger(1), TouchPhase::Moved, attack + Vec2::new(0.0, -100.0));
    let state = *controls.update(ms(5)).button(Button::Attack);
    assert!(state.dragging && !state.over && state.down);

    // Back inside: still a drag until it lifts, and the release carries the drag.
    controls.touch(ms(6), finger(1), TouchPhase::Moved, attack + Vec2::new(1.0, -1.0));
    assert!(controls.update(ms(7)).button(Button::Attack).dragging);
    controls.touch(ms(8), finger(1), TouchPhase::Up, attack + Vec2::new(2.0, -40.0));
    let actions = controls.update(ms(9));
    assert_eq!(actions.events[0].drag, Vec2::new(2.0, 40.0));
    let state = actions.button(Button::Attack);
    assert!(state.released && !state.down && state.drag == Vec2::ZERO && state.held_for(ms(10)).is_none());
}

#[test]
fn the_stick_floats_under_the_thumb_and_follows_it() {
    let layout = layout();
    let tuning = TouchTuning { stick_dead_zone: 0.0, ..TouchTuning::default() };
    let mut controls = TouchControls::with_tuning(layout, tuning);
    let radius = layout.stick.radius;
    let landing = layout.stick.zone.center();
    controls.touch(ms(0), finger(1), TouchPhase::Down, landing);
    let stick = controls.update(ms(1)).stick;
    assert_eq!((stick.center, stick.thumb, stick.value), (landing, landing, Vec2::ZERO), "under the thumb");

    // Half the radius up reads half, +y up.
    controls.touch(ms(2), finger(1), TouchPhase::Moved, landing + Vec2::new(0.0, -radius * 0.5));
    let stick = controls.update(ms(3)).stick;
    assert_eq!(stick.value, Vec2::new(0.0, 0.5));
    assert_eq!(stick.center, landing);

    // Far to the right: the value is full and the stick is dragged along behind the thumb.
    let thumb = landing + Vec2::new(radius * 3.0, 0.0);
    controls.touch(ms(4), finger(1), TouchPhase::Moved, thumb);
    let stick = controls.update(ms(5)).stick;
    assert!(stick.value.distance(Vec2::X) < 1e-5, "{stick:?}");
    assert!((stick.center.distance(thumb) - radius).abs() < 1e-3, "{stick:?}");

    // Turning straight back goes through zero at once, where it would otherwise stay pegged.
    let back = stick.center + Vec2::new(-radius, 0.0);
    controls.touch(ms(6), finger(1), TouchPhase::Moved, back);
    let value = controls.update(ms(7)).stick.value;
    assert!(value.distance(Vec2::NEG_X) < 1e-3, "{value}");
}

#[test]
fn the_dead_zone_keeps_a_resting_thumb_from_steering_and_the_rim_still_reads_one() {
    assert_eq!(stick_value(Vec2::new(0.09, 0.0), 0.1), Vec2::ZERO);
    assert!(stick_value(Vec2::new(0.11, 0.0), 0.1).length() < 0.02, "rises from zero");
    assert!((stick_value(Vec2::new(0.0, 1.0), 0.1).length() - 1.0).abs() < 1e-5);
    assert_eq!(stick_value(Vec2::new(0.5, 0.0), 1.0), Vec2::ZERO, "a dead zone of everything");
}

#[test]
fn past_the_slots_a_finger_takes_nothing_and_breaks_nothing() {
    let layout = layout();
    let mut controls = TouchControls::new(layout);
    for id in 0..MAX_TOUCHES as u64 {
        assert!(controls.touch(ms(0), finger(id), TouchPhase::Down, Vec2::new(420.0, 120.0)));
    }
    assert_eq!(controls.fingers(), MAX_TOUCHES);
    assert!(controls.touch(ms(1), finger(99), TouchPhase::Down, layout.attack.center), "still a landing");
    let actions = controls.update(ms(2));
    assert!(!actions.button(Button::Attack).down, "no slot, no press");
    assert!(!controls.touch(ms(3), finger(99), TouchPhase::Up, layout.attack.center));
    // A lift frees a slot for the next.
    controls.touch(ms(4), finger(0), TouchPhase::Up, Vec2::new(420.0, 120.0));
    controls.touch(ms(5), finger(100), TouchPhase::Down, layout.attack.center);
    assert!(controls.update(ms(6)).button(Button::Attack).down);
}

#[test]
fn a_new_layout_moves_the_zones_not_the_fingers() {
    let layout = layout();
    let mut controls = TouchControls::new(layout);
    controls.touch(ms(0), finger(1), TouchPhase::Down, layout.attack.center);
    // A rotation or a new safe area: the controls move while a thumb is down.
    let insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
    let moved = HudLayout::new(UiMetrics::new(Vec2::new(874.0, 402.0), insets));
    controls.set_layout(moved);
    assert!(controls.update(ms(1)).button(Button::Attack).down);
    controls.touch(ms(2), finger(1), TouchPhase::Moved, moved.attack.center);
    assert!(controls.update(ms(3)).button(Button::Attack).down);
    controls.touch(ms(4), finger(1), TouchPhase::Up, moved.attack.center);
    assert!(controls.update(ms(5)).button(Button::Attack).released);
    // New landings use the new zones.
    controls.touch(ms(6), finger(2), TouchPhase::Down, moved.dodge.center);
    assert!(controls.update(ms(7)).button(Button::Dodge).down);
}

#[test]
fn a_landing_between_two_hit_circles_takes_the_nearer() {
    // Hit circles of an arc of buttons could touch: the thumb's nearest wins, whichever comes first.
    let mut layout = layout();
    layout.dodge.center = layout.attack.center + Vec2::new(-80.0, 0.0);
    layout.dodge.hit = 62.0;
    let mut controls = TouchControls::new(layout);
    controls.touch(ms(0), finger(1), TouchPhase::Down, layout.attack.center + Vec2::new(-30.0, 0.0));
    assert!(controls.update(ms(1)).button(Button::Attack).down);
    controls.touch(ms(2), finger(1), TouchPhase::Up, layout.attack.center);
    controls.touch(ms(3), finger(2), TouchPhase::Down, layout.attack.center + Vec2::new(-50.0, 0.0));
    let actions = controls.update(ms(4));
    assert!(actions.button(Button::Dodge).down && !actions.button(Button::Attack).down);
}

fn touch_event(at: u64, id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
    Event::Touch(TouchEvent {
        timestamp: ms(at),
        window: WindowId::from_raw(1),
        finger: finger(id),
        phase,
        x,
        y,
        pressure: 1.0,
    })
}

#[test]
fn platform_events_land_in_points_and_say_who_took_them() {
    let layout = layout();
    let mut controls = TouchControls::new(layout);
    let share = layout.attack.center / layout.metrics.size;
    assert!(controls.handle_event(&touch_event(0, 7, TouchPhase::Down, share.x, share.y)));
    let actions = controls.update(ms(1));
    assert!(actions.button(Button::Attack).down, "shares of the window, scaled to points");
    assert_eq!(actions.events[0].at, ms(0), "the event's own time");

    let window = WindowId::from_raw(1);
    let lost = Event::Window { timestamp: ms(2), window, event: WindowEvent::FocusLost };
    assert!(!controls.handle_event(&lost), "for everyone, not just the controls");
    let actions = controls.update(ms(3));
    assert_eq!(actions.events[0].kind, ButtonEventKind::Canceled);
    assert_eq!(actions.events[0].at, ms(2));
    assert!(!controls.handle_event(&touch_event(4, 7, TouchPhase::Up, share.x, share.y)), "forgotten");

    assert!(controls.handle_event(&touch_event(5, 8, TouchPhase::Down, share.x, share.y)));
    let background = Event::Lifecycle { timestamp: ms(6), event: LifecycleEvent::WillEnterBackground };
    assert!(!controls.handle_event(&background));
    assert_eq!(controls.fingers(), 0);
    assert!(!controls.update(ms(7)).button(Button::Attack).down);

    let other = Event::Lifecycle { timestamp: ms(8), event: LifecycleEvent::LowMemory };
    assert!(!controls.handle_event(&other));
    assert!(!controls.handle_event(&Event::Quit { timestamp: ms(8) }));
    assert_eq!(controls.update(ms(9)).events, vec![], "neither of those lets go of anything");
}

#[test]
fn every_landing_inside_the_safe_area_goes_somewhere_sensible() {
    // Sweep the frame: each landing takes the button under it, else the stick in its zone, else
    // the camera, and a hit circle always beats the stick's zone.
    let layout = layout();
    for y in (0..360).step_by(9) {
        for x in (0..640).step_by(9) {
            let at = Vec2::new(x as f32, y as f32);
            let mut controls = TouchControls::new(layout);
            controls.touch(ms(0), finger(1), TouchPhase::Down, at);
            let actions = controls.update(ms(1));
            // The ultimate's isn't there: no ultimate was shown.
            let on_button = Button::ALL
                .into_iter()
                .filter(|&button| button != Button::Ultimate)
                .any(|button| layout.button(button).center.distance(at) <= layout.button(button).hit);
            let any_down = Button::ALL.into_iter().any(|button| actions.button(button).down);
            assert_eq!(any_down, on_button, "{at}");
            assert_eq!(actions.stick.active, !on_button && layout.stick.zone.contains(at), "{at}");
        }
    }
}

/// The ultimate's button is only there while the HUD shows an ultimate: until then a landing on it is the
/// camera's, and once it's shown it presses and lets go as any button does. Hidden under a thumb, it keeps
/// that thumb until it lifts, and takes no new one.
#[test]
fn the_ultimate_takes_a_thumb_only_while_it_is_shown() {
    let layout = layout();
    let ultimate = layout.ultimate.center;
    assert!(!layout.stick.zone.contains(ultimate));
    let mut controls = TouchControls::new(layout);
    assert!(!controls.takes(Button::Ultimate));
    assert!([Button::Attack, Button::Dodge, Button::Skill].into_iter().all(|button| controls.takes(button)));
    controls.touch(ms(1), finger(1), TouchPhase::Down, ultimate);
    controls.touch(ms(2), finger(1), TouchPhase::Moved, ultimate + Vec2::new(-30.0, 0.0));
    let actions = controls.update(ms(3));
    assert!(!actions.button(Button::Ultimate).down);
    assert_eq!(actions.events, vec![]);
    assert_eq!(actions.camera_delta, Vec2::new(-30.0, 0.0), "the camera's");
    assert!(!controls.holds_control(finger(1)));
    controls.touch(ms(4), finger(1), TouchPhase::Up, ultimate);

    controls.show_ultimate(true);
    controls.touch(ms(10), finger(2), TouchPhase::Down, ultimate + Vec2::new(0.0, layout.ultimate.hit - 1.0));
    let actions = controls.update(ms(11));
    assert!(actions.button(Button::Ultimate).down);
    assert_eq!(
        actions.events.iter().map(|event| (event.button, event.kind)).collect::<Vec<_>>(),
        [(Button::Ultimate, ButtonEventKind::Pressed)]
    );
    assert!(controls.holds_control(finger(2)));

    // Gone with the thumb still on it: it keeps the press, and a second thumb there takes nothing of it.
    controls.show_ultimate(false);
    controls.touch(ms(12), finger(3), TouchPhase::Down, ultimate);
    let actions = controls.update(ms(13));
    assert!(actions.button(Button::Ultimate).down);
    assert!(!controls.holds_control(finger(3)));
    controls.touch(ms(14), finger(2), TouchPhase::Up, ultimate);
    let actions = controls.update(ms(15));
    assert!(!actions.button(Button::Ultimate).down);
    assert_eq!(
        actions.events.iter().map(|event| (event.button, event.kind)).collect::<Vec<_>>(),
        [(Button::Ultimate, ButtonEventKind::Released)]
    );
}

/// What else counts fingers (the debug overlay's two-finger hold) asks this, so a thumb playing the game
/// is never a thumb holding still.
#[test]
fn a_finger_holds_a_control_from_its_landing_to_its_lift() {
    let spots = spots();
    let mut controls = TouchControls::new(layout());
    let (stick, button, camera, spare) = (finger(1), finger(2), finger(3), finger(4));
    controls.touch(ms(1), stick, TouchPhase::Down, spots.stick);
    controls.touch(ms(2), button, TouchPhase::Down, spots.attack);
    controls.touch(ms(3), camera, TouchPhase::Down, spots.open);
    controls.touch(ms(4), spare, TouchPhase::Down, spots.open);
    assert!(controls.holds_control(stick));
    assert!(controls.holds_control(button));
    assert!(!controls.holds_control(camera));
    assert!(!controls.holds_control(spare));
    assert!(!controls.holds_control(finger(9)));

    // A thumb that slides off keeps what it landed on, and lifting lets it go.
    controls.touch(ms(5), button, TouchPhase::Moved, spots.open);
    assert!(controls.holds_control(button));
    controls.touch(ms(6), button, TouchPhase::Up, spots.open);
    assert!(!controls.holds_control(button));
}

#[test]
fn resizing_rebases_retained_touches_before_a_stationary_event() {
    let old = layout();
    let mut controls = TouchControls::new(old);
    let stick = old.stick.zone.min + Vec2::splat(40.0);
    let camera = Vec2::new(400.0, 80.0);
    let button = old.attack.center;
    for (id, at) in [(1, stick), (2, camera), (3, button)] {
        controls.handle_event(&touch_event(
            0,
            id,
            TouchPhase::Down,
            at.x / old.metrics.size.x,
            at.y / old.metrics.size.y,
        ));
    }
    controls.update(ms(1));
    let new = HudLayout::new(UiMetrics::new(Vec2::new(960.0, 720.0), Insets::default()));
    controls.set_layout(new);
    for (id, at) in [(1, stick), (2, camera), (3, button)] {
        controls.handle_event(&touch_event(
            2,
            id,
            TouchPhase::Moved,
            at.x / old.metrics.size.x,
            at.y / old.metrics.size.y,
        ));
    }
    let actions = controls.update(ms(3));
    assert!(actions.camera_delta.length() < 1e-3);
    assert!(actions.stick.value.length() < 1e-5);
    assert!(actions.button(Button::Attack).drag.length() < 1e-3);
    assert!(!actions.button(Button::Attack).dragging);
    assert!(actions.button(Button::Attack).down);
    assert_eq!(controls.fingers(), 3);
}

#[test]
fn a_held_sticks_value_survives_aspect_changes_and_an_empty_frame() {
    let old = layout();
    let mut controls = TouchControls::new(old);
    let start = old.stick.rest;
    let held = start + Vec2::new(old.stick.radius * 0.5, 0.0);
    controls.touch(ms(0), finger(1), TouchPhase::Down, start);
    controls.touch(ms(1), finger(1), TouchPhase::Moved, held);
    let before = controls.update(ms(2)).stick.value;
    assert!(before.x > 0.0);
    controls.set_layout(HudLayout::new(UiMetrics::default()));
    let new = HudLayout::new(UiMetrics::new(Vec2::new(960.0, 720.0), Insets::default()));
    controls.set_layout(new);
    controls.handle_event(&touch_event(
        3,
        1,
        TouchPhase::Moved,
        held.x / old.metrics.size.x,
        held.y / old.metrics.size.y,
    ));
    let after = controls.update(ms(4)).stick.value;
    assert!(after.distance(before) < 1e-5, "{before} -> {after}");
}

#[test]
fn the_first_usable_layout_keeps_an_inactive_stick_default_and_ready_for_input() {
    // FightHud starts here before its first window/offscreen metrics arrive.
    let mut controls = TouchControls::new(HudLayout::default());
    assert_eq!(controls.update(ms(0)).stick, StickState::default());
    let usable = layout();
    controls.set_layout(usable);
    let stick = controls.update(ms(1)).stick;
    assert_eq!(stick, StickState::default());
    assert!(stick.center.is_finite() && stick.thumb.is_finite() && stick.value.is_finite());

    let start = usable.stick.rest;
    controls.touch(ms(2), finger(1), TouchPhase::Down, start);
    let held = start + Vec2::new(usable.stick.radius * 0.5, 0.0);
    controls.touch(ms(3), finger(1), TouchPhase::Moved, held);
    let stick = controls.update(ms(4)).stick;
    assert!(stick.active);
    assert_eq!(stick.center, start);
    assert_eq!(stick.thumb, held);
    let expected = (0.5 - 0.1) / (1.0 - 0.1);
    assert!(stick.value.distance(Vec2::new(expected, 0.0)) < 1e-5);
}
