//! Keyboard, gamepad, mouse and fingers all reach the one focus model, through the input layer's own
//! values and its platform events, since nothing here can open a window. The tree on the far end is a
//! real one, from a document.

use std::alloc::System;

use sapphire_assets::ui::UiDocument;
use sapphire_controls::MenuControls;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_input::{MouseButton, PadAxis, PadButton, RawInput, Timestamp};
use sapphire_loc::{LocaleSettings, Localizer};
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_math::Vec2;
use sapphire_platform::{
    Event, GamepadType, MouseButton as PlatformMouseButton, MouseButtonEvent, MouseMotionEvent, Scancode,
    TouchPhase, WindowId,
};
use sapphire_ui::{Atlas, UiEvent, UiFonts, UiFrame, UiMetrics, UiPointerKind, UiStep, UiTree};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// Two rows of two, and a list below them that overflows.
const MENU: &str = r#"(type: "sapphire::assets::UiDocument", version: 2, data: (nodes: [
    (name: "root", layout: (direction: Column, gap: 10)),
    (name: "top", parent: "root", layout: (direction: Row, gap: 10)),
    (name: "a", parent: "top", focus: (first: true), layout: (width: Points(50), height: Points(50))),
    (name: "b", parent: "top", focus: (), layout: (width: Points(50), height: Points(50))),
    (name: "bottom", parent: "root", layout: (direction: Row, gap: 10)),
    (name: "c", parent: "bottom", focus: (), layout: (width: Points(50), height: Points(50))),
    (name: "d", parent: "bottom", focus: (), layout: (width: Points(50), height: Points(50))),
    (name: "list", parent: "root", mask: true, scroll: Vertical, layout: (width: Points(110), height: Points(100))),
    (name: "tall", parent: "list", layout: (height: Points(1000)), focus: ()),
],))"#;

const ARROW_UP: Scancode = Scancode(82);
const ARROW_DOWN: Scancode = Scancode(81);
const ARROW_RIGHT: Scancode = Scancode(79);
const RETURN: Scancode = Scancode(40);
const ESCAPE: Scancode = Scancode(41);
const KEY_S: Scancode = Scancode(22);

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

struct Show {
    controls: MenuControls,
    tree: UiTree,
    fonts: UiFonts,
    loc: Localizer,
    atlas: Atlas,
    list: UiDrawList,
    events: Vec<UiEvent>,
    now: u64,
}

impl Show {
    fn new() -> Self {
        let document = UiDocument::from_document(MENU).unwrap();
        let settings = LocaleSettings { text: "en".to_owned(), voice: "en".to_owned() };
        let mut show = Self {
            controls: MenuControls::with_defaults().unwrap(),
            tree: UiTree::new(&document).unwrap(),
            fonts: UiFonts::default(),
            loc: Localizer::new(settings),
            atlas: Atlas::paint(),
            list: UiDrawList::default(),
            events: Vec::with_capacity(64),
            now: 0,
        };
        show.controls.push(ms(0), RawInput::GamepadAdded { pad: 0, kind: GamepadType::Standard });
        show.frame(0);
        show
    }

    /// A frame, at `advance` milliseconds after the last, with what's been queued.
    fn frame(&mut self, advance: u64) {
        self.now += advance;
        let now = ms(self.now);
        let input = self.controls.update(now);
        self.tree.input(&input, now);
        self.events.extend(self.tree.drain_events());
        self.list.reset();
        self.fonts.begin_frame(UiPageState::NO_RENDERER, &self.loc);
        let metrics = UiMetrics::new(Vec2::new(1280.0, 720.0), Default::default());
        self.tree.update(&UiFrame { metrics, now, loc: &self.loc, model: None }, &mut self.fonts);
        self.tree.draw(&mut self.fonts, &self.atlas, &mut self.list);
    }

    fn push(&mut self, input: RawInput) {
        self.controls.push(ms(self.now + 1), input);
    }

    fn tap_key(&mut self, scancode: Scancode) {
        self.push(RawInput::Key { scancode, down: true });
        self.frame(16);
        self.push(RawInput::Key { scancode, down: false });
        self.frame(16);
    }

    fn tap_pad(&mut self, button: PadButton) {
        self.push(RawInput::PadButton { pad: 0, button, down: true });
        self.frame(16);
        self.push(RawInput::PadButton { pad: 0, button, down: false });
        self.frame(16);
    }

    fn lean_stick(&mut self, x: f32, y: f32) {
        self.push(RawInput::PadAxis { pad: 0, axis: PadAxis::LeftX, value: x });
        self.push(RawInput::PadAxis { pad: 0, axis: PadAxis::LeftY, value: y });
    }

    fn center(&self, name: &str) -> Vec2 {
        self.tree.screen_rect(self.tree.find(name).unwrap()).center()
    }

    fn tap_finger(&mut self, position: Vec2) {
        self.push(RawInput::Touch { finger: 0, phase: TouchPhase::Down, position });
        self.frame(16);
        self.push(RawInput::Touch { finger: 0, phase: TouchPhase::Up, position });
        self.frame(16);
    }

    fn focused(&self) -> &str {
        self.tree.focus_name().unwrap()
    }

    fn activated(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|event| match event {
                UiEvent::Activated(node) => Some(self.tree.name(*node)),
                _ => None,
            })
            .collect()
    }
}

/// Which of the grid's nodes each device reaches, in the same order.
fn walk_is_abdc(step: impl Fn(&mut Show, UiStep), confirm: impl Fn(&mut Show)) {
    let mut show = Show::new();
    assert_eq!(show.focused(), "a");
    step(&mut show, UiStep::Right);
    assert_eq!(show.focused(), "b");
    step(&mut show, UiStep::Down);
    assert_eq!(show.focused(), "d");
    step(&mut show, UiStep::Left);
    assert_eq!(show.focused(), "c");
    confirm(&mut show);
    assert_eq!(show.activated(), ["c"]);
}

fn arrow(step: UiStep) -> Scancode {
    match step {
        UiStep::Up => ARROW_UP,
        UiStep::Down => ARROW_DOWN,
        UiStep::Left => Scancode(80),
        UiStep::Right => ARROW_RIGHT,
    }
}

#[test]
fn the_keyboard_reaches_focus() {
    walk_is_abdc(|show, step| show.tap_key(arrow(step)), |show| show.tap_key(RETURN));
}

#[test]
fn w_a_s_d_reach_focus_too() {
    let key = |step| match step {
        UiStep::Up => Scancode(26),
        UiStep::Down => KEY_S,
        UiStep::Left => Scancode(4),
        UiStep::Right => Scancode(7),
    };
    walk_is_abdc(move |show, step| show.tap_key(key(step)), |show| show.tap_key(Scancode(44)));
}

#[test]
fn the_d_pad_reaches_focus() {
    let button = |step| match step {
        UiStep::Up => PadButton::DpadUp,
        UiStep::Down => PadButton::DpadDown,
        UiStep::Left => PadButton::DpadLeft,
        UiStep::Right => PadButton::DpadRight,
    };
    walk_is_abdc(move |show, step| show.tap_pad(button(step)), |show| show.tap_pad(PadButton::South));
}

#[test]
fn the_stick_reaches_focus() {
    // SDL's axes, +y down.
    let lean = |step| match step {
        UiStep::Up => (0.0, -1.0),
        UiStep::Down => (0.0, 1.0),
        UiStep::Left => (-1.0, 0.0),
        UiStep::Right => (1.0, 0.0),
    };
    walk_is_abdc(
        move |show, step| {
            let (x, y) = lean(step);
            show.lean_stick(x, y);
            show.frame(16);
            show.lean_stick(0.0, 0.0);
            show.frame(16);
        },
        |show| show.tap_pad(PadButton::South),
    );
}

#[test]
fn a_finger_reaches_focus() {
    walk_is_abdc(
        |show, step| {
            let name = match (show.focused(), step) {
                ("a", UiStep::Right) => "b",
                ("b", UiStep::Down) => "d",
                ("d", UiStep::Left) => "c",
                other => panic!("{other:?}"),
            };
            // Pressed and then taken away: focus moves, and nothing's chosen.
            let position = show.center(name);
            show.push(RawInput::Touch { finger: 0, phase: TouchPhase::Down, position });
            show.frame(16);
            show.push(RawInput::Touch { finger: 0, phase: TouchPhase::Canceled, position });
            show.frame(16);
        },
        |show| {
            let position = show.center("c");
            show.tap_finger(position);
        },
    );
}

#[test]
fn a_mouse_reaches_focus_through_the_platform_s_events() {
    let window = WindowId::from_raw(1);
    let timestamp = ms(1);
    let at = |show: &Show, name: &str| show.center(name);
    let moved = move |position: Vec2| {
        Event::MouseMotion(MouseMotionEvent {
            timestamp,
            window,
            x: position.x,
            y: position.y,
            dx: 1.0,
            dy: 0.0,
        })
    };
    let button = move |down: bool, position: Vec2| {
        Event::MouseButton(MouseButtonEvent {
            timestamp,
            window,
            button: PlatformMouseButton::Left,
            down,
            clicks: 1,
            x: position.x,
            y: position.y,
        })
    };
    let mut show = Show::new();
    for name in ["b", "d", "c"] {
        let position = at(&show, name);
        show.controls.handle_event(&moved(position));
        show.frame(16);
        assert_eq!(show.focused(), name);
    }
    let position = at(&show, "a");
    show.controls.handle_event(&button(true, position));
    show.frame(16);
    assert_eq!(show.focused(), "a", "the press carries where it is, with no motion before it");
    assert_eq!(show.activated(), Vec::<&str>::new());
    show.controls.handle_event(&button(false, position));
    show.frame(16);
    assert_eq!(show.activated(), ["a"]);
}

#[test]
fn a_pointer_s_position_comes_back_in_the_screen_s_points() {
    let mut show = Show::new();
    show.controls.set_player_scale(2.0);
    let position = show.center("b") * 2.0;
    show.push(RawInput::MouseAt { position });
    show.frame(16);
    assert_eq!(show.focused(), "b");
}

#[test]
fn losing_focus_cancels_a_press() {
    let mut show = Show::new();
    let position = show.center("b");
    show.push(RawInput::Touch { finger: 0, phase: TouchPhase::Down, position });
    show.frame(16);
    show.push(RawInput::ReleaseAll);
    let input = show.controls.update(ms(40));
    assert_eq!(input.pointers.len(), 1);
    assert_eq!(input.pointers[0].kind, UiPointerKind::Touch);
    assert_eq!(input.pointers[0].phase, sapphire_ui::UiPointerPhase::Cancel);
    let input = show.controls.update(ms(50));
    assert_eq!(input.pointers.len(), 0, "and it's said once");
}

#[test]
fn back_is_the_same_from_every_device() {
    for press in [
        (|show: &mut Show| show.tap_key(ESCAPE)) as fn(&mut Show),
        |show| show.tap_key(Scancode(42)),
        |show| show.tap_key(Scancode(282)),
        |show| show.tap_pad(PadButton::East),
        |show| show.tap_pad(PadButton::Back),
    ] {
        let mut show = Show::new();
        press(&mut show);
        assert_eq!(show.events.iter().filter(|event| **event == UiEvent::Back).count(), 1);
    }
}

#[test]
fn a_held_direction_repeats_after_a_wait() {
    let mut controls = MenuControls::with_defaults().unwrap();
    controls.push(ms(0), RawInput::Key { scancode: ARROW_DOWN, down: true });
    let mut seen = Vec::new();
    for time in (0..=1000).step_by(10) {
        if controls.update(ms(time)).step == Some(UiStep::Down) {
            seen.push(time);
        }
    }
    assert_eq!(seen, [0, 400, 520, 640, 760, 880, 1000]);
    controls.push(ms(1001), RawInput::Key { scancode: ARROW_DOWN, down: false });
    assert_eq!(controls.update(ms(1010)).step, None);
    assert_eq!(controls.update(ms(5000)).step, None);
}

#[test]
fn a_late_frame_gets_one_press_not_a_burst() {
    let mut controls = MenuControls::with_defaults().unwrap();
    controls.push(ms(0), RawInput::Key { scancode: ARROW_DOWN, down: true });
    assert_eq!(controls.update(ms(0)).step, Some(UiStep::Down));
    assert_eq!(controls.update(ms(5000)).step, Some(UiStep::Down));
    assert_eq!(controls.update(ms(5001)).step, None);
}

#[test]
fn a_tap_inside_a_frame_still_counts() {
    let mut controls = MenuControls::with_defaults().unwrap();
    controls.update(ms(0));
    controls.push(ms(5), RawInput::Key { scancode: ARROW_RIGHT, down: true });
    controls.push(ms(6), RawInput::Key { scancode: ARROW_RIGHT, down: false });
    assert_eq!(controls.update(ms(16)).step, Some(UiStep::Right));
    assert_eq!(controls.update(ms(32)).step, None);
}

#[test]
fn a_stick_held_near_a_line_presses_once() {
    let mut show = Show::new();
    show.lean_stick(0.0, 0.8);
    show.controls.update(ms(100));
    // Comes back past where it started, stays past where it stops, and goes over again.
    let mut presses = 0;
    for (at, value) in [(200, 0.45), (210, 0.6), (220, 0.46), (230, 0.55)] {
        show.controls.push(ms(at), RawInput::PadAxis { pad: 0, axis: PadAxis::LeftY, value });
        presses += u32::from(show.controls.update(ms(at + 1)).step.is_some());
    }
    assert_eq!(presses, 0);
    // A little more across than along doesn't turn it, until the held axis lets go.
    show.controls.push(ms(300), RawInput::PadAxis { pad: 0, axis: PadAxis::LeftX, value: 0.7 });
    assert_eq!(show.controls.update(ms(301)).step, None);
    show.controls.push(ms(310), RawInput::PadAxis { pad: 0, axis: PadAxis::LeftY, value: 0.1 });
    assert_eq!(show.controls.update(ms(311)).step, Some(UiStep::Right));
}

#[test]
fn a_wheel_and_a_stick_go_through_a_list() {
    let mut show = Show::new();
    show.push(RawInput::MouseWheel { delta: Vec2::new(0.0, -2.0) });
    let input = show.controls.update(ms(16));
    assert_eq!(input.scroll, Vec2::new(0.0, 96.0), "two notches toward you");
    show.tree.input(&input, ms(16));
    show.frame(1000);
    let list = show.tree.find("list").unwrap();
    assert_eq!(show.tree.scroll_offset(list), Some(96.0));

    show.push(RawInput::PadAxis { pad: 0, axis: PadAxis::RightY, value: 1.0 });
    show.frame(16);
    let moved_at_first = show.controls.update(ms(show.now + 16)).scroll;
    assert!(moved_at_first.y > 10.0 && moved_at_first.y < 20.0, "{moved_at_first}");
}

#[test]
fn a_mouse_button_is_only_a_pointer_for_the_left_one() {
    let mut controls = MenuControls::with_defaults().unwrap();
    controls.update(ms(0));
    controls.push(ms(1), RawInput::MouseButton { button: MouseButton::Right, down: true });
    assert_eq!(controls.update(ms(2)).pointers.len(), 0);
    controls.push(ms(3), RawInput::MouseButton { button: MouseButton::Left, down: true });
    assert_eq!(controls.update(ms(4)).pointers.len(), 1);
}

#[test]
fn a_menu_s_frames_allocate_nothing_once_warm() {
    if !PER_THREAD_COUNTS {
        return;
    }
    let mut show = Show::new();
    let go = |show: &mut Show| {
        show.tap_key(ARROW_RIGHT);
        show.tap_key(ARROW_DOWN);
        show.tap_pad(PadButton::DpadLeft);
        show.tap_pad(PadButton::DpadUp);
        let position = show.center("b");
        show.tap_finger(position);
        show.push(RawInput::MouseAt { position: show.center("a") });
        show.frame(16);
        show.push(RawInput::MouseWheel { delta: Vec2::new(0.0, -1.0) });
        show.frame(16);
        show.events.clear();
    };
    for _ in 0..3 {
        go(&mut show);
    }
    let ((), counts) = alloc::count_in_scope(|| {
        for _ in 0..20 {
            go(&mut show);
        }
    });
    assert_eq!(counts.allocations, 0, "{counts:?}");
}
