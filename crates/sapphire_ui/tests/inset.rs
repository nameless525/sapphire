//! 3D insets (M3 U6): a node that shows one draws its picture's quad, names its shot, and turns by a drag
//! across it or a stick, the same way for the same input.

mod common;

use common::{Model, at, document, fonts, frame_at, localizer, metrics};
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_ui::{Atlas, UiInput, UiPointer, UiPointerKind, UiPointerPhase, UiTree};

/// An inset 400 by 300 points beside a button, which a model can hide.
const SCREEN: &str = r#"model: "m.model.ron",
    nodes: [
        (name: "root", layout: (direction: Row, gap: 20, padding: (left: 20, top: 20))),
        (name: "showcase", parent: "root", layout: (width: Points(400), height: Points(300)),
            inset: (inset: 1, turn: 180), binds: [(to: Shown, path: "player.shown")]),
        (name: "button", parent: "root", focus: (), layout: (width: Points(100), height: Points(50)),
            look: (fill: Color((60, 30, 90, 255)))),
    ],"#;

struct Show {
    tree: UiTree,
    fonts: sapphire_ui::UiFonts,
    loc: sapphire_loc::Localizer,
    atlas: Atlas,
    list: UiDrawList,
    model: Model,
    scale: f32,
}

impl Show {
    fn new(scale: f32) -> Self {
        Self::of(SCREEN, scale)
    }

    fn of(screen: &str, scale: f32) -> Self {
        let mut model = Model::default();
        model.player.shown = true;
        let mut show = Self {
            tree: UiTree::new(&document(screen)).unwrap(),
            fonts: fonts(),
            loc: localizer("en"),
            atlas: Atlas::paint(),
            list: UiDrawList::default(),
            model,
            scale,
        };
        show.frame();
        show
    }

    fn frame(&mut self) {
        let metrics = sapphire_ui::UiMetrics { scale: self.scale, ..metrics([1280.0, 720.0]) };
        frame_at(
            at(0.0),
            &mut self.tree,
            &mut self.fonts,
            &self.loc,
            &self.model,
            metrics,
            &self.atlas,
            &mut self.list,
        );
    }

    fn yaw(&self) -> f32 {
        self.tree.insets().next().expect("shown").yaw
    }

    /// A finger down at the inset's middle, through `xs` across it, and up.
    fn drag(&mut self, xs: &[f32]) {
        let start = self.tree.screen_rect(self.tree.find("showcase").unwrap()).center();
        let finger = |phase, x: f32, seconds: f32| UiPointer {
            id: 1,
            kind: UiPointerKind::Touch,
            phase,
            position: start + Vec2::new(x, 0.0),
            at: at(seconds),
        };
        let mut pointers = vec![finger(UiPointerPhase::Down, 0.0, 1.0)];
        let moves = xs.iter().enumerate();
        pointers.extend(moves.map(|(step, &x)| finger(UiPointerPhase::Move, x, 1.0 + step as f32 * 0.01)));
        pointers.push(finger(UiPointerPhase::Up, xs.last().copied().unwrap_or(0.0), 2.0));
        self.tree.input(&UiInput { pointers: &pointers, ..UiInput::default() }, at(1.0));
        assert_eq!(self.tree.drain_events().count(), 0, "a drag presses nothing");
        self.frame();
    }
}

#[test]
fn an_inset_draws_its_picture_and_names_its_shot() {
    let show = Show::new(2.0);
    let [run] = show.list.inset_runs[..] else { panic!("one run: {:?}", show.list.inset_runs) };
    assert_eq!((run.inset, run.count), (1, 1));
    let quad = show.list.quads[run.first as usize];
    assert_eq!((quad.origin, quad.axis_x, quad.axis_y), ([20.0, 20.0], [400.0, 0.0], [0.0, 300.0]));
    let shots: Vec<_> = show.tree.insets().collect();
    let [shot] = shots[..] else { panic!("one shot") };
    assert_eq!((shot.setup.inset, shot.size, shot.yaw), (1, (800, 600), 0.0), "in pixels, at 2 a point");
    assert!(show.tree.holds_insets());
}

#[test]
fn an_inset_off_whole_pixels_meets_its_plate() {
    // A plate behind the picture, laid out off whole pixels at 1.5 pixels a point.
    let screen = SCREEN.replace("padding: (left: 20, top: 20)", "padding: (left: 20.3, top: 20.3)").replace(
        "layout: (width: Points(400), height: Points(300)),\n",
        "layout: (width: Points(400.4), height: Points(300.4)), look: (fill: Color((0, 0, 0, 255))),\n",
    );
    assert_eq!(screen.matches("400.4").count(), 1, "{screen}");
    let show = Show::of(&screen, 1.5);
    let [run] = show.list.inset_runs[..] else { panic!("one run: {:?}", show.list.inset_runs) };
    let picture = show.list.quads[run.first as usize];
    let plate = show.list.quads[run.first as usize - 1];
    let edges = |quad: sapphire_look::UiQuad| (quad.origin, quad.axis_x, quad.axis_y);
    assert_eq!(edges(picture), edges(plate), "the picture's edges are the plate's");
    let pixels = |points: f32| points * 1.5;
    for value in [picture.origin[0], picture.origin[1], picture.axis_x[0], picture.axis_y[1]] {
        assert_eq!(pixels(value), pixels(value).round(), "{value} points is on a whole pixel");
    }
    let shot = show.tree.insets().next().expect("shown");
    let drawn = (pixels(picture.axis_x[0]) as u32, pixels(picture.axis_y[1]) as u32);
    assert_eq!(shot.size, drawn, "the shot is the picture's pixels, one to one");
}

#[test]
fn a_hidden_inset_shows_nothing_and_draws_nothing() {
    let mut show = Show::new(1.0);
    show.model.player.shown = false;
    show.frame();
    assert!(show.list.inset_runs.is_empty());
    assert_eq!(show.tree.insets().count(), 0);
    assert!(show.tree.holds_insets(), "it holds one still, for a renderer's warm-up");
}

#[test]
fn an_inset_off_screen_draws_nothing_and_names_no_shot() {
    // Pushed past the frame's right edge, at its full size.
    let pushed = "layout: (width: Points(400), height: Points(300), shrink: 0, margin: (left: 2000))";
    let screen = SCREEN.replace("layout: (width: Points(400), height: Points(300))", pushed);
    assert_ne!(screen, SCREEN);
    let show = Show::of(&screen, 1.0);
    assert!(show.list.inset_runs.is_empty());
    assert_eq!(show.tree.insets().count(), 0, "no showcase to stand for a picture nobody sees");
}

#[test]
fn a_drag_across_turns_it_from_where_it_passed_the_slop() {
    let mut show = Show::new(1.0);
    show.drag(&[4.0]);
    assert_eq!(show.yaw(), 0.0, "short of the slop");
    // Past the slop at 10, then 100 points more: a quarter of its width, at 180 degrees across it.
    show.drag(&[10.0, 60.0, 110.0]);
    assert!((show.yaw() - 45.0).abs() < 1e-3, "{}", show.yaw());
    // Back the other way past where it started wraps round.
    show.drag(&[-10.0, -210.0]);
    assert!((show.yaw() - 315.0).abs() < 1e-3, "{}", show.yaw());
}

#[test]
fn the_same_input_turns_it_the_same_way() {
    let turned = || {
        let mut show = Show::new(1.0);
        show.drag(&[12.0, 40.0, 90.0, 133.0]);
        show.tree.turn_insets(0.125);
        show.drag(&[-9.0, -77.0]);
        show.yaw()
    };
    let first = turned();
    assert_ne!(first, 0.0);
    assert_eq!(first.to_bits(), turned().to_bits());
}

#[test]
fn a_stick_turns_it_by_its_rate() {
    let mut show = Show::new(1.0);
    // Half a second at full tilt, at 180 degrees a second.
    show.tree.turn_insets(0.5);
    assert!((show.yaw() - 90.0).abs() < 1e-4);
    show.tree.turn_insets(-1.0);
    assert!((show.yaw() - 270.0).abs() < 1e-4, "wrapped: {}", show.yaw());
    show.tree.turn_insets(f32::NAN);
    assert!((show.yaw() - 270.0).abs() < 1e-4, "nothing from nothing");
}

#[test]
fn a_press_beside_it_still_presses_the_button() {
    let mut show = Show::new(1.0);
    let button = show.tree.screen_rect(show.tree.find("button").unwrap()).center();
    let finger = |phase, seconds| UiPointer {
        id: 1,
        kind: UiPointerKind::Touch,
        phase,
        position: button,
        at: at(seconds),
    };
    let pointers = [finger(UiPointerPhase::Down, 1.0), finger(UiPointerPhase::Up, 1.1)];
    show.tree.input(&UiInput { pointers: &pointers, ..UiInput::default() }, at(1.0));
    assert_eq!(show.tree.drain_events().count(), 1);
    assert_eq!(show.yaw(), 0.0);
}
