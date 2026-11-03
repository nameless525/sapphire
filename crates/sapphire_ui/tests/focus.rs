//! The one focus model: directions, confirm, back, and pointers, as plain values. Scrolling is here too,
//! since pointers and focus both drive it. `sapphire_controls`' tests reach the same model through the
//! input layer's own values.

mod common;

use common::{Model, at, document, fonts, frame_at, localizer, metrics};
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;
use sapphire_ui::{Atlas, UiEvent, UiHit, UiInput, UiPointer, UiPointerKind, UiPointerPhase, UiStep, UiTree};

/// Two rows of three, with one that a model can hide, and a tall thing to the right of them all.
const GRID: &str = r#"model: "m.model.ron",
    nodes: [
        (name: "root", layout: (direction: Row, gap: 10)),
        (name: "grid", parent: "root", layout: (direction: Column, gap: 10)),
        (name: "top", parent: "grid", layout: (direction: Row, gap: 10)),
        (name: "bottom", parent: "grid", layout: (direction: Row, gap: 10)),
        (name: "a", parent: "top", focus: (), layout: (width: Points(50), height: Points(50))),
        (name: "b", parent: "top", focus: (), layout: (width: Points(50), height: Points(50)),
            binds: [(to: Shown, path: "player.shown")]),
        (name: "c", parent: "top", focus: (), layout: (width: Points(50), height: Points(50))),
        (name: "d", parent: "bottom", focus: (), layout: (width: Points(50), height: Points(50))),
        (name: "e", parent: "bottom", focus: (first: true), layout: (width: Points(50), height: Points(50))),
        (name: "f", parent: "bottom", focus: (down: "side"), layout: (width: Points(50), height: Points(50))),
        (name: "side", parent: "root", focus: (), layout: (width: Points(50), height: Points(110))),
    ],"#;

struct Show {
    tree: UiTree,
    fonts: sapphire_ui::UiFonts,
    loc: sapphire_loc::Localizer,
    atlas: Atlas,
    list: UiDrawList,
    model: Model,
    size: [f32; 2],
}

impl Show {
    fn new(data: &str) -> Self {
        let mut model = Model::default();
        model.player.shown = true;
        let mut show = Self {
            tree: UiTree::new(&document(data)).unwrap(),
            fonts: fonts(),
            loc: localizer("en"),
            atlas: Atlas::paint(),
            list: UiDrawList::default(),
            model,
            size: [1280.0, 720.0],
        };
        show.frame(0.0);
        show
    }

    fn frame(&mut self, seconds: f32) {
        frame_at(
            at(seconds),
            &mut self.tree,
            &mut self.fonts,
            &self.loc,
            &self.model,
            metrics(self.size),
            &self.atlas,
            &mut self.list,
        );
    }

    fn step(&mut self, step: UiStep) -> String {
        self.tree.input(&UiInput { step: Some(step), ..UiInput::default() }, at(0.0));
        self.focused()
    }

    fn focused(&self) -> String {
        self.tree.focus_name().unwrap_or_default().to_owned()
    }

    fn center(&self, name: &str) -> Vec2 {
        self.tree.screen_rect(self.tree.find(name).unwrap()).center()
    }

    fn pointers(&mut self, seconds: f32, pointers: &[UiPointer]) -> Vec<UiEvent> {
        self.tree.input(&UiInput { pointers, ..UiInput::default() }, at(seconds));
        self.tree.drain_events().collect()
    }

    fn activated(&self, events: &[UiEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                UiEvent::Activated(node) => Some(self.tree.name(*node).to_owned()),
                _ => None,
            })
            .collect()
    }
}

fn pointer(kind: UiPointerKind, phase: UiPointerPhase, position: Vec2, seconds: f32) -> UiPointer {
    let at: Timestamp = at(seconds);
    UiPointer { id: 1, kind, phase, position, at }
}

fn finger(phase: UiPointerPhase, position: Vec2, seconds: f32) -> UiPointer {
    pointer(UiPointerKind::Touch, phase, position, seconds)
}

fn mouse(phase: UiPointerPhase, position: Vec2, seconds: f32) -> UiPointer {
    pointer(UiPointerKind::Mouse, phase, position, seconds)
}

#[test]
fn focus_starts_where_a_document_says_and_moves_to_what_is_nearest() {
    let mut show = Show::new(GRID);
    assert_eq!(show.focused(), "e");
    assert_eq!(show.step(UiStep::Left), "d");
    assert_eq!(show.step(UiStep::Left), "d", "nothing that way: no wrap");
    assert_eq!(show.step(UiStep::Up), "a");
    assert_eq!(show.step(UiStep::Right), "b");
    assert_eq!(show.step(UiStep::Right), "c");
    assert_eq!(show.step(UiStep::Right), "side");
    assert_eq!(show.step(UiStep::Left), "c", "at the height of what it left, the top row is nearest");
    assert_eq!(show.step(UiStep::Down), "f");
    assert_eq!(show.step(UiStep::Down), "side", "as the document names");
}

#[test]
fn a_hidden_node_takes_no_focus_and_gives_it_up() {
    let mut show = Show::new(GRID);
    show.step(UiStep::Up);
    assert_eq!(show.focused(), "b");
    // The model hides it: focus goes on to the next shown, and says so.
    show.model.player.shown = false;
    show.tree.drain_events().for_each(drop);
    show.frame(0.0);
    assert_eq!(show.focused(), "c");
    let events: Vec<_> = show.tree.drain_events().collect();
    let (b, c) = (show.tree.find("b"), show.tree.find("c"));
    assert_eq!(events, [UiEvent::FocusMoved { from: b, to: c }]);
    assert_eq!(show.step(UiStep::Left), "a", "and a direction goes past it");
    assert_eq!(show.tree.unreachable(), Vec::<&str>::new());
}

#[test]
fn confirm_and_back_make_events_and_tell_the_machine() {
    let data = r#"nodes: [
            (name: "root"),
            (name: "go", parent: "root", focus: (fires: "close"), layout: (width: Points(50), height: Points(50))),
        ],
        states: [(name: "open", input: true), (name: "closed"), (name: "gone")],
        transitions: [(from: "open", on: "close", to: "closed"), (from: "open", on: "back", to: "gone")],"#;
    let mut show = Show::new(data);
    show.tree.input(&UiInput { confirm: true, ..UiInput::default() }, at(0.0));
    let events: Vec<_> = show.tree.drain_events().collect();
    assert_eq!(show.activated(&events), ["go"]);
    assert_eq!(show.tree.state(), Some("closed"));

    let mut show = Show::new(data);
    show.tree.input(&UiInput { back: true, ..UiInput::default() }, at(0.0));
    let events: Vec<_> = show.tree.drain_events().collect();
    assert_eq!(events, [UiEvent::Back]);
    assert_eq!(show.tree.state(), Some("gone"));
}

#[test]
fn a_screen_not_taking_input_takes_none() {
    let data = r#"nodes: [
            (name: "root"),
            (name: "a", parent: "root", focus: (), layout: (width: Points(50), height: Points(50))),
            (name: "b", parent: "root", focus: (), layout: (width: Points(50), height: Points(50))),
        ],
        states: [(name: "shut"), (name: "open", input: true)],
        transitions: [(from: "shut", on: "open", to: "open")],"#;
    let mut show = Show::new(data);
    let at_b = show.center("b");
    let input = UiInput { step: Some(UiStep::Right), confirm: true, back: true, ..UiInput::default() };
    show.tree.input(&input, at(0.0));
    let pressed = [finger(UiPointerPhase::Down, at_b, 0.0), finger(UiPointerPhase::Up, at_b, 0.0)];
    show.tree.input(&UiInput { pointers: &pressed, ..UiInput::default() }, at(0.0));
    assert_eq!(show.tree.drain_events().count(), 0);
    assert_eq!(show.focused(), "a");

    show.tree.fire("open", at(0.0));
    show.tree.input(&input, at(0.0));
    assert_eq!(show.focused(), "b");
    assert!(show.tree.drain_events().count() > 0);
}

#[test]
fn a_hit_goes_by_what_was_drawn_last_on_top() {
    let data = r#"nodes: [
            (name: "root", look: (fill: Color((0, 0, 0, 255)))),
            (name: "button", parent: "root", focus: (),
                layout: (anchored: (left: Points(100), top: Points(100), right: Auto, bottom: Auto),
                         width: Points(100), height: Points(100)),
                look: (fill: Color((255, 255, 255, 255)))),
            (name: "icon", parent: "button", layout: (width: Points(20), height: Points(20)),
                look: (fill: Color((255, 0, 0, 255)))),
            (name: "over", parent: "root",
                layout: (anchored: (left: Points(150), top: Points(150), right: Auto, bottom: Auto),
                         width: Points(100), height: Points(100)),
                look: (fill: Color((0, 255, 0, 255)))),
            (name: "clipper", parent: "root", mask: true,
                layout: (anchored: (left: Points(400), top: Points(0), right: Auto, bottom: Auto),
                         width: Points(50), height: Points(50))),
            (name: "clipped", parent: "clipper", focus: (),
                layout: (width: Points(50), height: Points(200), shrink: 0),
                look: (fill: Color((0, 0, 255, 255)))),
            (name: "ghost", parent: "root", opacity: 0, focus: (),
                layout: (anchored: (left: Points(600), top: Points(0), right: Auto, bottom: Auto),
                         width: Points(50), height: Points(50)),
                look: (fill: Color((0, 0, 255, 255)))),
        ],
        timelines: [(name: "slide", tracks: [(node: "button", property: X, keys: [(at: 0, value: 700)])])],"#;
    let mut show = Show::new(data);
    let button = show.tree.find("button").unwrap();
    assert_eq!(
        show.tree.hit(Vec2::new(110.0, 110.0)),
        UiHit::Focusable(button),
        "a child counts as its parent"
    );
    assert_eq!(show.tree.hit(Vec2::new(140.0, 140.0)), UiHit::Focusable(button));
    assert_eq!(show.tree.hit(Vec2::new(160.0, 160.0)), UiHit::Surface, "something's on top of it");
    assert_eq!(show.tree.hit(Vec2::new(10.0, 10.0)), UiHit::Surface);
    assert_eq!(show.tree.hit(Vec2::new(420.0, 40.0)), UiHit::Focusable(show.tree.find("clipped").unwrap()));
    assert_eq!(show.tree.hit(Vec2::new(420.0, 100.0)), UiHit::Surface, "masked away");
    assert_eq!(show.tree.hit(Vec2::new(620.0, 20.0)), UiHit::Surface, "what can't be seen can't be hit");

    // By where it's drawn, not where layout put it.
    show.tree.play("slide", at(0.0));
    show.frame(0.0);
    assert_eq!(show.tree.hit(Vec2::new(810.0, 110.0)), UiHit::Focusable(button));
    assert_eq!(show.tree.hit(Vec2::new(110.0, 110.0)), UiHit::Surface);
}

#[test]
fn nothing_shows_where_a_screen_has_nothing() {
    let show = Show::new(GRID);
    assert_eq!(show.tree.hit(Vec2::new(1000.0, 600.0)), UiHit::Nothing);
}

const BUTTONS: &str = r#"nodes: [
        (name: "root", layout: (direction: Row, gap: 10)),
        (name: "a", parent: "root", layout: (width: Points(50), height: Points(50)),
            look: (fill: Color((1, 1, 1, 255))),
            focus: (focused: (fill: Color((2, 2, 2, 255))), pressed: (fill: Color((3, 3, 3, 255))))),
        (name: "b", parent: "root", layout: (width: Points(50), height: Points(50)),
            look: (fill: Color((1, 1, 1, 255))),
            focus: (focused: (fill: Color((2, 2, 2, 255))), pressed: (fill: Color((3, 3, 3, 255))))),
    ],"#;

#[test]
fn a_finger_on_a_button_presses_it_and_letting_go_there_activates_it() {
    let mut show = Show::new(BUTTONS);
    let (a, b) = (show.center("a"), show.center("b"));
    let events = show.pointers(1.0, &[finger(UiPointerPhase::Down, b, 1.0)]);
    assert_eq!(show.focused(), "b");
    assert_eq!(show.activated(&events), Vec::<String>::new());
    show.frame(1.0);
    assert_eq!(show.list.quads[0].color[0], 1, "`a` has lost focus");
    assert_eq!(show.list.quads[1].color[0], 3, "`b` looks pressed");

    let events = show.pointers(1.1, &[finger(UiPointerPhase::Up, b, 1.1)]);
    assert_eq!(show.activated(&events), ["b"]);
    show.frame(1.1);
    assert_eq!(show.list.quads[1].color[0], 1, "a finger leaves no highlight");

    // Let go somewhere else, and it's not chosen.
    let pressed = [finger(UiPointerPhase::Down, b, 2.0), finger(UiPointerPhase::Up, a, 2.1)];
    let events = show.pointers(2.1, &pressed);
    assert_eq!(show.activated(&events), Vec::<String>::new());
    // Taken away by the OS, the same.
    let pressed = [finger(UiPointerPhase::Down, b, 3.0), finger(UiPointerPhase::Cancel, b, 3.1)];
    let events = show.pointers(3.1, &pressed);
    assert_eq!(show.activated(&events), Vec::<String>::new());
    // A key or a pad brings the highlight back.
    show.step(UiStep::Left);
    show.frame(4.0);
    assert_eq!(show.list.quads[0].color[0], 2);
}

#[test]
fn a_mouse_over_a_button_moves_focus_and_shows_it() {
    let mut show = Show::new(BUTTONS);
    let b = show.center("b");
    show.pointers(1.0, &[mouse(UiPointerPhase::Move, b, 1.0)]);
    assert_eq!(show.focused(), "b");
    show.frame(1.0);
    assert_eq!(show.list.quads[1].color[0], 2);
    // Off it, focus stays for a key to use.
    show.pointers(1.1, &[mouse(UiPointerPhase::Move, Vec2::new(900.0, 500.0), 1.1)]);
    assert_eq!(show.focused(), "b");
    let events =
        show.pointers(1.2, &[mouse(UiPointerPhase::Down, b, 1.2), mouse(UiPointerPhase::Up, b, 1.2)]);
    assert_eq!(show.activated(&events), ["b"]);
}

#[test]
fn a_second_finger_changes_nothing_while_the_first_is_down() {
    let mut show = Show::new(BUTTONS);
    let (a, b) = (show.center("a"), show.center("b"));
    let second = |phase, position, seconds| UiPointer { id: 2, ..finger(phase, position, seconds) };
    let events = show.pointers(
        1.0,
        &[
            finger(UiPointerPhase::Down, b, 1.0),
            second(UiPointerPhase::Down, a, 1.0),
            second(UiPointerPhase::Up, a, 1.0),
            finger(UiPointerPhase::Up, b, 1.0),
        ],
    );
    assert_eq!(show.activated(&events), ["b"]);
}

/// A list of ten rows, each 40 high with a gap of 10, in a box that shows 100.
const LIST: &str = r#"nodes: [
        (name: "root"),
        (name: "list", parent: "root", mask: true, scroll: Vertical,
            layout: (direction: Column, gap: 10, width: Points(200), height: Points(100),
                     padding: (bottom: 6))),
        (name: "row0", parent: "list", focus: (), layout: (height: Points(40)), look: (fill: Color((255, 255, 255, 255)))),
        (name: "row1", parent: "list", focus: (), layout: (height: Points(40)), look: (fill: Color((255, 255, 255, 255)))),
        (name: "row2", parent: "list", focus: (), layout: (height: Points(40)), look: (fill: Color((255, 255, 255, 255)))),
        (name: "row3", parent: "list", focus: (), layout: (height: Points(40)), look: (fill: Color((255, 255, 255, 255)))),
        (name: "row4", parent: "list", focus: (), layout: (height: Points(40)), look: (fill: Color((255, 255, 255, 255)))),
        (name: "row5", parent: "list", focus: (), layout: (height: Points(40)), look: (fill: Color((255, 255, 255, 255)))),
    ],"#;

fn offset(show: &Show) -> f32 {
    show.tree.scroll_offset(show.tree.find("list").unwrap()).unwrap()
}

#[test]
fn what_overflows_scrolls_as_far_as_there_is_and_no_further() {
    let mut show = Show::new(LIST);
    // Rows keep their size rather than shrinking to fit: six of 40 with gaps of 10 is 290, and 6 under.
    let row5 = show.tree.find("row5").unwrap();
    assert_eq!(show.tree.rect(row5).max.y, 290.0);
    let input = UiInput { scroll: Vec2::new(0.0, 48.0), ..UiInput::default() };
    show.tree.input(&input, at(1.0));
    show.frame(1.0);
    assert_eq!(offset(&show), 0.0, "on its way");
    assert_eq!(show.list.tree.scrolling, 1);
    assert!(show.tree.is_moving());
    show.frame(2.0);
    assert_eq!(offset(&show), 48.0);
    assert_eq!(show.tree.screen_rect(row5).max.y, 242.0);
    assert_eq!(show.tree.rect(row5).max.y, 290.0, "with no layout");
    assert_eq!(show.list.tree.layouts, 0);
    assert_eq!(show.list.tree.scrolling, 0);

    let far = UiInput { scroll: Vec2::new(0.0, 9000.0), ..UiInput::default() };
    show.tree.input(&far, at(2.0));
    show.frame(3.0);
    assert_eq!(offset(&show), 196.0, "290 of rows and 6 of padding, in a box of 100");
    let back = UiInput { scroll: Vec2::new(0.0, -9000.0), ..UiInput::default() };
    show.tree.input(&back, at(3.0));
    show.frame(4.0);
    assert_eq!(offset(&show), 0.0);
}

#[test]
fn focus_going_off_the_end_brings_its_node_into_view() {
    let mut show = Show::new(LIST);
    assert_eq!(show.step(UiStep::Down), "row1");
    show.frame(1.0);
    assert_eq!(offset(&show), 0.0, "it fits: 50 to 90, and 8 of room inside 100");
    show.tree.input(&UiInput { step: Some(UiStep::Down), ..UiInput::default() }, at(1.0));
    assert_eq!(show.focused(), "row2");
    show.frame(2.0);
    assert_eq!(offset(&show), 48.0, "100 to 140, and 8 of room");
    // The walk goes by layout, so it doesn't matter that the list is still moving.
    for name in ["row3", "row4", "row5"] {
        show.tree.input(&UiInput { step: Some(UiStep::Down), ..UiInput::default() }, at(2.01));
        assert_eq!(show.focused(), name);
    }
    show.frame(3.0);
    assert_eq!(offset(&show), 196.0, "as far as there is");
    show.tree.input(&UiInput { step: Some(UiStep::Up), ..UiInput::default() }, at(3.0));
    show.tree.input(&UiInput { step: Some(UiStep::Up), ..UiInput::default() }, at(3.0));
    assert_eq!(show.focused(), "row3");
    show.frame(4.0);
    assert_eq!(offset(&show), 142.0, "150, less 8 of room");
    assert_eq!(show.tree.unreachable(), Vec::<&str>::new());
}

#[test]
fn a_finger_dragging_a_list_scrolls_it_and_presses_nothing() {
    let mut show = Show::new(LIST);
    let start = show.center("row1");
    let moved = |y: f32| start + Vec2::new(0.0, y);
    let events = show.pointers(
        1.0,
        &[finger(UiPointerPhase::Down, start, 1.0), finger(UiPointerPhase::Move, moved(-4.0), 1.01)],
    );
    assert_eq!(show.activated(&events), Vec::<String>::new());
    show.frame(1.01);
    assert_eq!(offset(&show), 0.0, "not past the slop");
    assert_eq!(show.list.quads.len(), 2, "the rows in view");

    let events = show.pointers(1.02, &[finger(UiPointerPhase::Move, moved(-30.0), 1.02)]);
    assert!(show.activated(&events).is_empty());
    show.frame(1.02);
    assert_eq!(offset(&show), 30.0, "and it follows the finger from where it started");
    let events = show.pointers(1.5, &[finger(UiPointerPhase::Up, moved(-30.0), 1.5)]);
    assert!(show.activated(&events).is_empty(), "a drag isn't a press");
    show.frame(9.0);
    assert_eq!(offset(&show), 30.0, "it had stopped before it let go, so nothing flings");
}

#[test]
fn a_flick_carries_on_by_the_clock() {
    let mut show = Show::new(LIST);
    let start = show.center("row1");
    let moved = |y: f32| start + Vec2::new(0.0, y);
    show.pointers(
        1.0,
        &[
            finger(UiPointerPhase::Down, start, 1.0),
            finger(UiPointerPhase::Move, moved(-10.0), 1.01),
            finger(UiPointerPhase::Move, moved(-20.0), 1.02),
            finger(UiPointerPhase::Up, moved(-20.0), 1.03),
        ],
    );
    show.frame(1.03);
    assert_eq!(offset(&show), 20.0);
    show.frame(1.2);
    assert!(show.list.tree.scrolling == 1 && offset(&show) > 20.0, "{}", offset(&show));
    show.frame(9.0);
    assert_eq!(offset(&show), 187.0, "a thousand points a second dying off goes 167, to a whole pixel");
    assert!(!show.tree.is_moving());

    // A finger on it stops it where it is.
    let input = UiInput { scroll: Vec2::new(0.0, -180.0), ..UiInput::default() };
    show.tree.input(&input, at(9.0));
    show.frame(9.05);
    let moving = offset(&show);
    assert!(moving > 6.0 && moving < 186.0, "{moving}");
    show.pointers(9.05, &[finger(UiPointerPhase::Down, start, 9.05)]);
    show.frame(10.0);
    assert_eq!(offset(&show), moving);
}

#[test]
fn a_wheel_moves_the_list_under_the_mouse() {
    let data = LIST.replace("(name: \"root\"),", "(name: \"root\", layout: (direction: Row)),").replace(
        "(name: \"row0\",",
        r#"(name: "other", parent: "root", mask: true, scroll: Vertical, layout: (direction: Column, width: Points(200), height: Points(100))),
           (name: "tall", parent: "other", layout: (height: Points(400))),
           (name: "row0","#,
    );
    let mut show = Show::new(&data);
    let other = show.tree.find("other").unwrap();
    let over_other = show.tree.screen_rect(other).center();
    show.pointers(1.0, &[mouse(UiPointerPhase::Move, over_other, 1.0)]);
    let input = UiInput { scroll: Vec2::new(0.0, 48.0), ..UiInput::default() };
    show.tree.input(&input, at(1.0));
    show.frame(2.0);
    assert_eq!(show.tree.scroll_offset(other), Some(48.0));
    assert_eq!(offset(&show), 0.0);
    // Away from anything that scrolls, it moves what holds focus.
    show.pointers(2.0, &[mouse(UiPointerPhase::Move, Vec2::new(900.0, 500.0), 2.0)]);
    show.tree.input(&input, at(2.0));
    show.frame(3.0);
    assert_eq!(offset(&show), 48.0);
    assert_eq!(show.tree.scroll_offset(other), Some(48.0));
}

#[test]
fn a_scrolled_child_is_hit_where_it_is_drawn() {
    let mut show = Show::new(LIST);
    let input = UiInput { scroll: Vec2::new(0.0, 100.0), ..UiInput::default() };
    show.tree.input(&input, at(0.0));
    show.frame(1.0);
    let row2 = show.tree.find("row2").unwrap();
    assert_eq!(show.tree.hit(Vec2::new(10.0, 10.0)), UiHit::Focusable(row2));
    let pressed = [
        finger(UiPointerPhase::Down, Vec2::new(10.0, 10.0), 1.0),
        finger(UiPointerPhase::Up, Vec2::new(10.0, 10.0), 1.0),
    ];
    let events = show.pointers(1.0, &pressed);
    assert_eq!(show.activated(&events), ["row2"]);
    assert_eq!(show.tree.hit(Vec2::new(10.0, 200.0)), UiHit::Nothing, "what's masked off isn't there");
}

/// A menu's shape, in shares of its frame, so it sits differently at each aspect.
const MENU: &str = r#"nodes: [
        (name: "root", layout: (direction: Row, gap: 20, padding: (left: 40, top: 40, right: 40, bottom: 40))),
        (name: "tabs", parent: "root", layout: (direction: Column, gap: 8, width: Percent(20))),
        (name: "tab0", parent: "tabs", focus: (first: true), layout: (height: Points(40))),
        (name: "tab1", parent: "tabs", focus: (), layout: (height: Points(40))),
        (name: "tab2", parent: "tabs", focus: (), layout: (height: Points(40))),
        (name: "body", parent: "root", layout: (direction: Column, gap: 8, grow: 1)),
        (name: "row0", parent: "body", focus: (), layout: (height: Points(60))),
        (name: "row1", parent: "body", focus: (), layout: (height: Points(60))),
        (name: "ok", parent: "root", focus: (), layout: (width: Percent(15), height: Points(60))),
    ],"#;

#[test]
fn a_scripted_walk_comes_out_the_same_at_every_aspect() {
    for size in [[1024.0, 768.0], [1280.0, 720.0], [2560.0, 1080.0]] {
        let mut show = Show::new(MENU);
        show.size = size;
        show.frame(0.0);
        assert_eq!(show.tree.unreachable(), Vec::<&str>::new(), "{size:?}");
        let walked = show
            .tree
            .walk(["down", "down", "right", "down", "right", "left", "up", "left", "confirm"], at(0.0))
            .unwrap();
        assert_eq!(
            walked,
            ["tab1", "tab2", "row1", "row1", "ok", "row0", "row0", "tab0", "tab0"],
            "at {size:?}"
        );
        let events: Vec<_> = show.tree.drain_events().collect();
        assert_eq!(events.iter().filter(|event| matches!(event, UiEvent::Activated(_))).count(), 1);
        assert!(show.tree.walk(["sideways"], at(0.0)).is_err());
    }
}

#[test]
fn a_node_no_direction_reaches_is_found() {
    let data = r#"nodes: [
        (name: "root"),
        (name: "a", parent: "root", focus: (right: "a", left: "a", up: "a", down: "a"), layout: (height: Points(10))),
        (name: "b", parent: "root", focus: (), layout: (height: Points(10))),
    ],"#;
    let show = Show::new(data);
    assert_eq!(show.tree.unreachable(), ["b"]);
}
