//! Timelines, a screen's machine, and what they do to what's drawn: presentation, as a function of the
//! time a tree is handed.

mod common;

use common::{Model, at, document, fonts, frame, frame_at, glyphs, localizer, metrics};
use sapphire_look::{UiDrawList, UiPageState};
use sapphire_math::Vec2;
use sapphire_ui::{Atlas, Insets, UiFrame, UiMetrics, UiTree};

/// A panel that fades in and slides up over a quarter of a second, and fades out in a tenth, under a
/// machine. Text and a look, so a frame has both glyphs and field quads.
const SCREEN: &str = r#"
    styles: [(name: "label", role: "body", size: 16)],
    nodes: [
        (name: "root", layout: (direction: Column, align: Start, padding: (left: 100, top: 100))),
        (name: "panel", parent: "root", opacity: 0.5,
            layout: (direction: Column, width: Points(200), height: Points(100)),
            look: (shape: Slice(sprite: Button, corner: 12), fill: Color((40, 50, 90, 255)))),
        (name: "label", parent: "panel", text: (key: "title", style: "label", color: (255, 255, 255, 255))),
    ],
    timelines: [
        (name: "gone", tracks: [(node: "panel", property: Opacity, keys: [(at: 0, value: 0)])]),
        (name: "enter", tracks: [
            (node: "panel", property: Opacity, keys: [(at: 0, value: 0), (at: 0.25, value: 1)]),
            (node: "panel", property: Y, keys: [(at: 0, value: 40), (at: 0.25, value: 0, ease: Out)]),
        ]),
        (name: "leave", tracks: [
            (node: "panel", property: Opacity, keys: [(at: 0, value: 1), (at: 0.1, value: 0)]),
        ]),
        (name: "pulse", looping: true, tracks: [
            (node: "panel", property: Scale, keys: [(at: 0, value: 1), (at: 0.5, value: 2), (at: 1, value: 1)]),
        ]),
    ],
    states: [
        (name: "hidden", play: "gone"),
        (name: "entering", play: "enter", then: "open"),
        (name: "open", input: true),
        (name: "leaving", play: "leave", then: "hidden"),
    ],
    transitions: [
        (from: "hidden", on: "open", to: "entering"),
        (from: "entering", on: "close", to: "leaving"),
        (from: "open", on: "close", to: "leaving"),
    ],"#;

/// Everything needed to run frames of `SCREEN` at chosen times.
struct Show {
    tree: UiTree,
    fonts: sapphire_ui::UiFonts,
    loc: sapphire_loc::Localizer,
    atlas: Atlas,
    list: UiDrawList,
}

impl Show {
    fn new(data: &str) -> Self {
        Self {
            tree: UiTree::new(&document(data)).unwrap(),
            fonts: fonts(),
            loc: localizer("en"),
            atlas: Atlas::paint(),
            list: UiDrawList::default(),
        }
    }

    fn frame(&mut self, seconds: f32) {
        let size = metrics([1280.0, 720.0]);
        frame_at(
            at(seconds),
            &mut self.tree,
            &mut self.fonts,
            &self.loc,
            &Model::default(),
            size,
            &self.atlas,
            &mut self.list,
        );
    }

    fn panel(&self) -> sapphire_ui::UiNodeId {
        self.tree.find("panel").unwrap()
    }
}

/// Runs `SCREEN` from the clock's start, opening at 0.1 s, and writes `seconds` into the list, by steps
/// of `step` seconds.
fn opened_then_reach(seconds: f32, step: f32) -> UiDrawList {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    let mut now = 0.0;
    while now + step < 0.1 {
        now += step;
        show.frame(now);
    }
    show.frame(0.1);
    show.tree.fire("open", at(0.1));
    now = 0.1;
    while now + step < seconds {
        now += step;
        show.frame(now);
    }
    show.frame(seconds);
    std::mem::take(&mut show.list)
}

#[test]
fn a_given_time_is_the_same_bytes_however_it_got_there() {
    for seconds in [0.1, 0.2, 0.33, 0.35, 0.7] {
        let at_once = opened_then_reach(seconds, 10.0);
        for step in [1.0 / 60.0, 1.0 / 144.0, 0.031] {
            let by_frames = opened_then_reach(seconds, step);
            assert_eq!(at_once.quads.is_empty(), seconds == 0.1, "invisible only as it opens");
            assert_eq!(
                bytemuck::cast_slice::<_, u8>(&at_once.quads),
                bytemuck::cast_slice::<_, u8>(&by_frames.quads),
                "{seconds} s at steps of {step}",
            );
        }
    }
}

#[test]
fn a_machine_goes_on_when_its_timeline_ends_not_when_a_frame_notices() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    assert_eq!(show.tree.state(), Some("hidden"));
    assert!(show.tree.fire("open", at(1.0)));
    assert_eq!(show.tree.state(), Some("entering"));
    // An event with no transition from here does nothing.
    assert!(!show.tree.fire("open", at(1.0)));
    show.frame(1.2);
    assert_eq!(show.tree.state(), Some("entering"));
    // A long frame: opened at 1.25, closed at 2.0, over by 2.1.
    show.tree.fire("close", at(2.0));
    assert_eq!(show.tree.state(), Some("leaving"));
    show.frame(5.0);
    assert_eq!(show.tree.state(), Some("hidden"));
    assert!(!show.tree.is_moving());
}

#[test]
fn a_timeline_moves_what_draws_and_what_it_holds_with_no_layout() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    let panel = show.panel();
    let rest = show.tree.rect(panel);
    show.tree.fire("open", at(0.0));
    show.frame(0.125);
    assert_eq!(show.list.tree.layouts, 0);
    assert_eq!(show.tree.rect(panel), rest, "layout hasn't moved");
    // A cubic out is at seven eighths at the middle, and its node's own opacity multiplies in.
    assert_eq!(show.tree.screen_rect(panel).min.y, rest.min.y + 5.0);
    assert_eq!(show.tree.opacity(panel), 0.25);
    assert!(show.tree.is_moving());
    assert_eq!(show.list.tree.playing, 1);
    // Its children go with it.
    let label = show.tree.find("label").unwrap();
    assert_eq!(show.tree.screen_rect(label).min.y, show.tree.rect(label).min.y + 5.0);

    show.frame(0.25);
    assert_eq!(show.tree.screen_rect(panel), rest);
    assert_eq!(show.tree.opacity(panel), 0.5);
    assert!(!show.tree.is_moving());
}

#[test]
fn what_ended_stays_and_what_is_cut_short_does_not_jump() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    let panel = show.panel();
    show.tree.fire("open", at(0.0));
    show.frame(0.25);
    show.tree.fire("close", at(0.25));
    // Over, and held at its last key: no flash back to the document's opacity.
    show.frame(1.0);
    assert_eq!(show.tree.opacity(panel), 0.0);

    // Stopped where it was, it stays there, until something else plays.
    show.tree.play("pulse", at(1.0));
    show.frame(1.25);
    let halfway = show.tree.screen_rect(panel);
    assert_eq!(halfway.width(), 300.0, "a scale of 1.5 at a quarter of a second");
    assert!(show.tree.stop("pulse", at(1.25)));
    assert!(!show.tree.stop("pulse", at(1.25)));
    show.frame(9.0);
    assert_eq!(show.tree.screen_rect(panel), halfway);
}

#[test]
fn a_loop_goes_round_by_the_clock() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    let panel = show.panel();
    show.tree.play("pulse", at(0.0));
    show.frame(100.5);
    assert_eq!(show.tree.screen_rect(panel).width(), 400.0, "at the height of its 101st round");
    show.frame(100.75);
    assert_eq!(show.tree.screen_rect(panel).width(), 300.0);
    assert!(show.tree.is_moving());
    assert!(!show.tree.play("no such timeline", at(100.75)));
}

#[test]
fn a_scale_is_about_the_center_and_takes_its_quads_with_it() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    let panel = show.panel();
    let rest = show.tree.rect(panel);
    show.tree.fire("open", at(0.0));
    show.frame(0.25);
    let quads_at_rest = show.list.quads.clone();
    show.tree.play("pulse", at(0.25));
    show.frame(0.75);
    let scaled = show.tree.screen_rect(panel);
    assert_eq!(scaled.center(), rest.center());
    assert_eq!(scaled.width(), rest.width() * 2.0);
    assert_eq!(show.list.quads.len(), quads_at_rest.len());
    let (near, far) = (rest.min, scaled.min);
    let was = quads_at_rest[0].origin;
    let now = show.list.quads[0].origin;
    // The first of the nine-slice's quads goes from the panel's corner to its new one.
    assert_eq!(Vec2::from(now) - far, (Vec2::from(was) - near) * 2.0);
    assert_eq!(show.list.quads[0].axis_x[0], quads_at_rest[0].axis_x[0] * 2.0);
}

#[test]
fn a_move_lands_on_whole_pixels_so_text_stays_crisp() {
    let mut show = Show::new(
        r#"styles: [(name: "label", role: "body", size: 16)],
        nodes: [
            (name: "root"),
            (name: "label", parent: "root", text: (key: "title", style: "label", color: (255, 255, 255, 255))),
        ],
        timelines: [(name: "slide", tracks: [
            (node: "label", property: X, keys: [(at: 0, value: 0), (at: 1, value: 10)]),
        ])],"#,
    );
    show.frame(0.0);
    show.tree.play("slide", at(0.0));
    for seconds in [0.13, 0.27, 0.41, 0.77] {
        show.frame(seconds);
        for glyph in glyphs(&show.list) {
            assert_eq!(glyph.origin[0], glyph.origin[0].round(), "at {seconds} s");
            assert_eq!(glyph.origin[1], glyph.origin[1].round(), "at {seconds} s");
        }
    }
}

#[test]
fn a_mask_goes_with_what_moves_it() {
    let mut show = Show::new(
        r#"nodes: [
            (name: "root"),
            (name: "panel", parent: "root", mask: true, layout: (width: Points(100), height: Points(100))),
            (name: "inside", parent: "panel", layout: (width: Points(100), height: Points(100)),
                look: (fill: Color((255, 255, 255, 255)))),
        ],
        timelines: [(name: "slide", tracks: [
            (node: "panel", property: X, keys: [(at: 0, value: 40)]),
        ])],"#,
    );
    show.frame(0.0);
    show.tree.play("slide", at(0.0));
    show.frame(0.0);
    assert_eq!(show.list.masks[0].rect, Some([40.0, 0.0, 140.0, 100.0]));
    assert_eq!(show.list.quads[0].origin, [40.0, 0.0]);
}

#[test]
fn text_warms_while_invisible_so_its_first_frame_rasterizes_nothing() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    assert_eq!(glyphs(&show.list).count(), 0, "nothing shows");
    assert!(show.list.glyphs.rasterized > 0, "and its glyphs are on their way");
    assert!(!show.list.page_uploads.is_empty());

    show.tree.fire("open", at(1.0));
    show.frame(1.0);
    assert_eq!(glyphs(&show.list).count(), 0, "opacity is at 0 at the very start of its fade");
    show.frame(1.02);
    assert!(glyphs(&show.list).count() > 0, "now it shows");
    assert_eq!(show.list.glyphs.rasterized, 0, "with nothing left to do");
}

#[test]
fn a_big_screen_warms_a_little_each_frame() {
    // A string longer than a frame's allowance, one glyph per character at most.
    let mut show = Show::new(
        r#"styles: [(name: "label", role: "body", size: 16)],
        nodes: [
            (name: "root", opacity: 0, layout: (direction: Column)),
            (name: "a", parent: "root", text: (key: "tooltip-of-save", style: "label", color: (255, 255, 255, 255))),
            (name: "b", parent: "root", text: (key: "only-english", style: "label", color: (255, 255, 255, 255))),
            (name: "c", parent: "root", text: (key: "title", style: "label", color: (255, 255, 255, 255))),
            (name: "d", parent: "root", text: (key: "welcome-back", style: "label", color: (255, 255, 255, 255))),
            (name: "e", parent: "root", text: (key: "only-english-brand", style: "label", color: (255, 255, 255, 255))),
        ],"#,
    );
    let mut most = 0;
    let mut total = 0;
    for frame in 0..8 {
        show.frame(frame as f32);
        most = most.max(show.list.glyphs.rasterized);
        total += show.list.glyphs.rasterized;
        show.list.reset();
    }
    assert!(most <= 64, "{most} in a frame");
    assert!(total > 0);
    show.frame(9.0);
    assert_eq!(show.list.glyphs.rasterized, 0, "all there, and no frame did more than its share");
}

#[test]
fn a_hidden_screen_writes_no_quads() {
    let mut show = Show::new(SCREEN);
    show.frame(0.0);
    assert_eq!(show.list.quads.len(), 0);
}

#[test]
fn a_bound_key_reads_the_model_and_keeps_to_its_prefix() {
    let data = r#"model: "m.model.ron",
        styles: [(name: "label", role: "body", size: 16)],
        nodes: [
            (name: "root"),
            (name: "line", parent: "root", text: (
                key: "only-english", style: "label", color: (255, 255, 255, 255),
                key_from: (path: "name", prefix: "only-english"),
            )),
        ],"#;
    let mut tree = UiTree::new(&document(data)).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    let line = |tree: &UiTree| tree.strings().next().unwrap().1.to_owned();
    let size = metrics([640.0, 360.0]);

    // Nothing chosen: its own key.
    let mut model = Model::default();
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    let plain = line(&tree);
    assert!(plain.contains("no Japanese"), "{plain}");

    // A chosen one, read once, with no resolve while it stays.
    model.name = "only-english-brand".to_owned();
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_ne!(line(&tree), plain);
    assert_eq!(list.tree.resolves, 1);
    assert_eq!(tree.problems(), &[]);
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    assert_eq!(list.tree.resolves, 0);

    // One outside what a cook checked is a problem, said once, and its own key shows.
    model.name = "title".to_owned();
    for _ in 0..3 {
        frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    }
    assert_eq!(line(&tree), plain);
    assert_eq!(tree.problems().len(), 1, "{:#?}", tree.problems());
    assert_eq!(tree.problems()[0].field, "text.key_from.path");
    assert!(tree.problems()[0].problem.contains("`title`"), "{:#?}", tree.problems());
}

#[test]
fn a_bound_key_on_a_path_that_isnt_a_string_says_so() {
    let data = r#"model: "m.model.ron",
        styles: [(name: "label", role: "body", size: 16)],
        nodes: [
            (name: "root"),
            (name: "line", parent: "root", text: (
                key: "only-english", style: "label", color: (255, 255, 255, 255),
                key_from: (path: "player.health", prefix: "only-english"),
            )),
        ],"#;
    let mut tree = UiTree::new(&document(data)).unwrap();
    let (mut fonts, loc, atlas, mut list) = (fonts(), localizer("en"), Atlas::paint(), UiDrawList::default());
    frame(&mut tree, &mut fonts, &loc, &Model::default(), metrics([640.0, 360.0]), &atlas, &mut list);
    assert_eq!(tree.problems()[0].field, "text.key_from.path");
    assert!(tree.strings().next().unwrap().1.contains("no Japanese"));
}

#[test]
fn a_shadow_sits_behind_its_text_on_whole_pixels() {
    let data = |offset: &str| {
        format!(
            r#"styles: [(name: "label", role: "body", size: 16)],
            nodes: [
                (name: "root"),
                (name: "line", parent: "root", opacity: 0.5, text: (
                    key: "title", style: "label", color: (255, 255, 255, 255),
                    shadow: (color: (0, 0, 0, 200), offset: {offset}),
                )),
            ],"#
        )
    };
    let draw = |offset: &str, scale: f32| {
        let mut tree = UiTree::new(&document(&data(offset))).unwrap();
        let (mut fonts, loc, atlas) = (fonts(), localizer("en"), Atlas::paint());
        let mut list = UiDrawList::default();
        fonts.begin_frame(UiPageState::NO_RENDERER, &loc);
        let metrics = UiMetrics::of_pixels((640, 360), scale, Insets::default());
        tree.update(&UiFrame { metrics, now: at(0.0), loc: &loc, model: None }, &mut fonts);
        tree.draw(&mut fonts, &atlas, &mut list);
        list
    };
    let list = draw("(1.2, 2)", 1.0);
    let quads: Vec<_> = glyphs(&list).collect();
    assert_eq!(quads.len() % 2, 0);
    let (shadow, text) = quads.split_at(quads.len() / 2);
    for (shadow, text) in shadow.iter().zip(text) {
        assert_eq!(shadow.origin[0], text.origin[0] + 1.0);
        assert_eq!(shadow.origin[1], text.origin[1] + 2.0);
        assert_eq!(shadow.color[3], 100, "fading with the node");
        assert_eq!(text.color[3], 128);
    }
    // In points, and whole pixels: at 2 pixels a point a point and a quarter is 2.5, which rounds up.
    let list = draw("(1.25, 0)", 2.0);
    let quads: Vec<_> = glyphs(&list).collect();
    let (shadow, text) = quads.split_at(quads.len() / 2);
    assert_eq!(shadow[0].origin[0] - text[0].origin[0], 1.5);
}

#[test]
fn metrics_carry_the_scale_a_screen_snaps_to() {
    let data = r#"nodes: [
        (name: "root"),
        (name: "box", parent: "root",
            layout: (anchored: (left: Points(10.3), top: Points(10.3), right: Auto, bottom: Auto),
                     width: Points(20), height: Points(20)),
            look: (fill: Color((255, 255, 255, 255)))),
    ],"#;
    let draw = |scale: f32| {
        let mut tree = UiTree::new(&document(data)).unwrap();
        let (mut fonts, loc, atlas) = (fonts(), localizer("en"), Atlas::paint());
        let mut list = UiDrawList::default();
        fonts.begin_frame(UiPageState::NO_RENDERER, &loc);
        let metrics = UiMetrics::of_pixels((640, 360), scale, Insets::default());
        tree.update(&UiFrame { metrics, now: at(0.0), loc: &loc, model: None }, &mut fonts);
        tree.draw(&mut fonts, &atlas, &mut list);
        (list.viewport, list.quads[0].origin)
    };
    assert_eq!(draw(1.0), ([640.0, 360.0], [10.0, 10.0]));
    assert_eq!(draw(2.0), ([320.0, 180.0], [10.5, 10.5]));
}
