//! A frame of a retained screen where nothing changed allocates nothing, whatever the screen binds,
//! wraps or masks. Installs `CountingAllocator` as this test binary's global allocator.

mod common;

use std::alloc::System;

use common::{Model, Player, at, document, fonts, frame, frame_at, localizer, metrics};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_ui::{Atlas, UiFonts, UiInput, UiPointer, UiPointerKind, UiPointerPhase, UiStep, UiTree};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const SCREEN: &str = r#"model: "m.model.ron",
    styles: [(name: "label", role: "body", size: 16), (name: "title", role: "display", size: 32)],
    nodes: [
        (name: "root", layout: (direction: Column, safe_area: true, gap: 8)),
        (name: "title", parent: "root", text: (key: "title", style: "title", color: (255, 255, 255, 255))),
        (name: "panel", parent: "root", mask: true,
            layout: (direction: Column, padding: (left: 8, right: 8, top: 8, bottom: 8), width: Points(320)),
            look: (shape: Slice(sprite: Button, corner: 12),
                   fill: Gradient(from: (20, 30, 60, 255), to: (60, 30, 90, 255), angle: 90)),
            binds: [(to: Opacity, path: "player.health")]),
        (name: "named", parent: "panel", text: (
            key: "welcome", style: "label", color: (255, 255, 255, 255), wrap: true,
            args: [(name: "name", path: "name")],
        )),
        (name: "left", parent: "panel", text: (
            key: "enemies-left", style: "label", color: (255, 255, 255, 255),
            args: [(name: "count", path: "player.enemies")],
        )),
        (name: "bar", parent: "panel",
            layout: (width: Percent(100), height: Points(12)),
            look: (shape: Slice(sprite: Bar, corner: 6), fill: Color((30, 200, 160, 255))),
            binds: [(to: Fill, path: "player.health"), (to: Shown, path: "player.shown")]),
    ],"#;

#[test]
fn a_frame_with_nothing_changed_allocates_nothing() {
    if !PER_THREAD_COUNTS {
        // Process-wide totals can't tell this thread's allocations from the test harness's.
        return;
    }
    let mut tree = UiTree::new(&document(SCREEN)).unwrap();
    let (mut fonts, atlas, mut list) = (fonts(), Atlas::paint(), UiDrawList::default());
    for language in ["en", "ja"] {
        let loc = localizer(language);
        let model = Model {
            player: Player { health: 0.5, shown: true, enemies: 12 },
            name: "Aoi".to_owned(),
            ..Model::default()
        };
        let size = metrics([1280.0, 720.0]);
        // The screen's first frames: shaping, rasterizing, and every buffer growing.
        for _ in 0..3 {
            frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
        }
        assert_eq!(tree.problems(), &[]);
        assert!(!list.quads.is_empty());
        let ((), counts) = alloc::count_in_scope(|| {
            for _ in 0..100 {
                frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            }
        });
        assert_eq!(counts.allocations, 0, "100 unchanged frames of {language}: {counts:?}");
        assert_eq!(list.tree.layouts, 0);
    }
}

/// A list that fades in and out and scrolls, under a machine, with focus and a pressed look: everything
/// a menu does in a frame once it's open.
const MENU: &str = r#"model: "m.model.ron",
    styles: [(name: "label", role: "body", size: 16)],
    nodes: [
        (name: "root", layout: (direction: Column, align: Start)),
        (name: "panel", parent: "root", mask: true, scroll: Vertical,
            layout: (direction: Column, gap: 8, width: Points(320), height: Points(200)),
            look: (shape: Slice(sprite: Button, corner: 12), fill: Color((20, 30, 60, 255)))),
        (name: "row0", parent: "panel", layout: (height: Points(80)),
            look: (fill: Color((60, 30, 90, 255))),
            focus: (focused: (fill: Color((90, 60, 120, 255))), pressed: (fill: Color((120, 90, 150, 255))))),
        (name: "row0-text", parent: "row0", text: (
            key: "welcome", style: "label", color: (255, 255, 255, 255),
            args: [(name: "name", path: "name")], shadow: (color: (0, 0, 0, 128), offset: (1, 1)),
        )),
        (name: "row1", parent: "panel", layout: (height: Points(80)), focus: (),
            look: (fill: Color((60, 30, 90, 255)))),
        (name: "row1-text", parent: "row1", text: (
            key: "only-english", style: "label", color: (255, 255, 255, 255),
            key_from: (path: "name", prefix: "only-english"),
        )),
        (name: "row2", parent: "panel", layout: (height: Points(80)), focus: (),
            look: (fill: Color((60, 30, 90, 255)))),
        (name: "row3", parent: "panel", layout: (height: Points(80)), focus: (),
            look: (fill: Color((60, 30, 90, 255)))),
    ],
    timelines: [
        (name: "enter", tracks: [
            (node: "panel", property: Opacity, keys: [(at: 0, value: 0), (at: 0.2, value: 1, ease: Out)]),
            (node: "panel", property: Y, keys: [(at: 0, value: 24), (at: 0.2, value: 0, ease: Back)]),
        ]),
        (name: "leave", tracks: [
            (node: "panel", property: Opacity, keys: [(at: 0, value: 1), (at: 0.2, value: 0)]),
            (node: "panel", property: Scale, keys: [(at: 0, value: 1), (at: 0.2, value: 0.9)]),
        ]),
    ],
    states: [
        (name: "open", input: true),
        (name: "entering", play: "enter", then: "open"),
        (name: "leaving", play: "leave"),
    ],
    transitions: [
        (from: "open", on: "close", to: "leaving"),
        (from: "leaving", on: "open", to: "entering"),
    ],"#;

struct Show {
    tree: UiTree,
    fonts: UiFonts,
    atlas: Atlas,
    list: UiDrawList,
    loc: sapphire_loc::Localizer,
    model: Model,
    now: f32,
}

impl Show {
    fn frame(&mut self) {
        self.now += 1.0 / 60.0;
        frame_at(
            at(self.now),
            &mut self.tree,
            &mut self.fonts,
            &self.loc,
            &self.model,
            metrics([1280.0, 720.0]),
            &self.atlas,
            &mut self.list,
        );
    }

    fn frames(&mut self, count: u32) {
        for _ in 0..count {
            self.frame();
        }
    }

    /// One full fade out and back in.
    fn fade(&mut self) {
        assert!(self.tree.fire("close", at(self.now)));
        self.frames(20);
        assert!(self.tree.fire("open", at(self.now)));
        self.frames(20);
    }

    /// Through the list by the wheel, by focus, and by a finger's drag and flick.
    fn scroll(&mut self) {
        let now = at(self.now);
        self.tree.input(&UiInput { scroll: Vec2::new(0.0, 48.0), ..UiInput::default() }, now);
        self.frames(12);
        for step in [UiStep::Down, UiStep::Down, UiStep::Down, UiStep::Up, UiStep::Up, UiStep::Up] {
            let now = at(self.now);
            self.tree.input(&UiInput { step: Some(step), ..UiInput::default() }, now);
            self.tree.drain_events().for_each(drop);
            self.frames(12);
        }
        let touch = |phase, y: f32, seconds: f32| UiPointer {
            id: 1,
            kind: UiPointerKind::Touch,
            phase,
            position: Vec2::new(40.0, y),
            at: at(seconds),
        };
        let now = self.now;
        let pointers = [
            touch(UiPointerPhase::Down, 150.0, now),
            touch(UiPointerPhase::Move, 130.0, now + 0.01),
            touch(UiPointerPhase::Move, 110.0, now + 0.02),
            touch(UiPointerPhase::Up, 110.0, now + 0.03),
        ];
        self.tree.input(&UiInput { pointers: &pointers, ..UiInput::default() }, at(now));
        self.tree.drain_events().for_each(drop);
        self.frames(30);
    }
}

fn warmed_menu(language: &str) -> Show {
    let model = Model { name: "only-english-brand".to_owned(), ..Model::default() };
    let mut show = Show {
        tree: UiTree::new(&document(MENU)).unwrap(),
        fonts: fonts(),
        atlas: Atlas::paint(),
        list: UiDrawList::default(),
        loc: localizer(language),
        model,
        now: 0.0,
    };
    // What warm means here: once through everything, with every buffer growing as far as it will.
    show.frames(3);
    show.fade();
    show.scroll();
    show.fade();
    assert_eq!(show.tree.problems(), &[]);
    show
}

#[test]
fn a_fade_allocates_nothing_once_warm() {
    if !PER_THREAD_COUNTS {
        return;
    }
    for language in ["en", "ja"] {
        let mut show = warmed_menu(language);
        let ((), counts) = alloc::count_in_scope(|| {
            for _ in 0..5 {
                show.fade();
            }
        });
        assert_eq!(counts.allocations, 0, "five fades out and in, in {language}: {counts:?}");
        assert!(!show.tree.is_moving());
    }
}

#[test]
fn a_scroll_allocates_nothing_once_warm() {
    if !PER_THREAD_COUNTS {
        return;
    }
    for language in ["en", "ja"] {
        let mut show = warmed_menu(language);
        let ((), counts) = alloc::count_in_scope(|| {
            for _ in 0..5 {
                show.scroll();
            }
        });
        assert_eq!(counts.allocations, 0, "five goes through the list, in {language}: {counts:?}");
        assert_eq!(show.list.tree.layouts, 0, "and scrolling lays nothing out");
    }
}
