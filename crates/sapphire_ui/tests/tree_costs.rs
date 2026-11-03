//! What a retained screen costs, printed, against spec §3.2's UI row. `cargo test -p sapphire_ui --release
//! --test tree_costs -- --ignored --nocapture --test-threads=1`. They assert nothing but print what U4
//! and U5 report, and the same harness runs on a simulator and an emulator.
//!
//! The screen is a menu of `ROWS` rows, each a nine-slice with two strings and a bar, in the fonts as
//! shipped: about what a real settings screen holds, and more than a HUD does.

#![allow(clippy::print_stdout, reason = "printing the numbers is this test's whole job")]
#![allow(clippy::disallowed_methods, reason = "timing them needs the OS clock, and nothing here ships")]

mod common;

use std::alloc::System;
use std::fmt::Write as _;
use std::time::Instant;

use common::{Model, Player, at, document, fonts, frame, frame_at, localizer, metrics};
use sapphire_assets::ui::UiDocument;
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_ui::{Atlas, UiHit, UiInput, UiStep, UiTree};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const ROWS: usize = 20;
const SAMPLES: usize = 400;

/// The menu as a document.
fn menu() -> UiDocument {
    menu_with("", "", "")
}

/// The menu with these fields on its root and its rows, and these beside its nodes.
fn menu_with(root: &str, row_fields: &str, rest: &str) -> UiDocument {
    let mut nodes = format!(
        r#"(name: "root", {root} layout: (direction: Column, gap: 8, padding: (left: 24, right: 24, top: 24, bottom: 24))),
           (name: "title", parent: "root", text: (key: "title", style: "title", color: (255, 255, 255, 255))),"#,
    );
    for row in 0..ROWS {
        write!(
            nodes,
            r#"(name: "row{row}", parent: "root", {row_fields}
                 layout: (direction: Row, gap: 12, align: Center, padding: (left: 12, right: 12, top: 4, bottom: 4)),
                 look: (shape: Slice(sprite: Button, corner: 10), fill: Gradient(from: (40, 48, 92, 255), to: (70, 48, 110, 255), angle: 0))),
               (name: "name{row}", parent: "row{row}", text: (key: "welcome", style: "label", color: (255, 255, 255, 255),
                 args: [(name: "name", path: "name")])),
               (name: "count{row}", parent: "row{row}", text: (key: "enemies-left", style: "label", color: (255, 255, 255, 255),
                 args: [(name: "count", path: "player.enemies")])),
               (name: "bar{row}", parent: "row{row}", layout: (grow: 1, height: Points(10)),
                 look: (shape: Slice(sprite: Bar, corner: 5), fill: Color((30, 200, 160, 255))),
                 binds: [(to: Fill, path: "player.health")]),"#,
        )
        .unwrap();
    }
    document(&format!(
        r#"model: "m.model.ron",
           styles: [(name: "label", role: "body", size: 16), (name: "title", role: "display", size: 32)],
           nodes: [{nodes}],
           {rest}"#
    ))
}

fn model() -> Model {
    Model {
        player: Player { health: 0.5, shown: true, enemies: 12 },
        name: "Aoi".to_owned(),
        ..Model::default()
    }
}

fn print(label: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let at = |share: f64| samples[((samples.len() - 1) as f64 * share).round() as usize];
    println!("{label:<44} p50 {:>9.4} ms   p99 {:>9.4} ms", at(0.5), at(0.99));
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

#[test]
#[ignore = "prints costs"]
fn costs() {
    let document = menu();
    let mut loc = localizer("ja");
    let (mut fonts, atlas, mut list) = (fonts(), Atlas::paint(), UiDrawList::default());
    let size = metrics([1280.0, 1000.0]);
    let mut model = model();

    let samples = (0..100)
        .map(|_| {
            let start = Instant::now();
            std::hint::black_box(UiTree::new(&document).unwrap());
            ms(start)
        })
        .collect();
    print("a tree from its document", samples);

    let mut tree = UiTree::new(&document).unwrap();
    let start = Instant::now();
    frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    println!("{} nodes, first frame, every string shaped: {:.3} ms", list.tree.nodes, ms(start));
    for _ in 0..3 {
        frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    }
    assert_eq!(tree.problems(), &[]);
    println!("{} quads, {} glyphs", list.quads.len(), common::glyphs(&list).count());

    // A frame where nothing changed: the bound values read and compared, and the list written.
    let samples = (0..SAMPLES)
        .map(|_| {
            let start = Instant::now();
            frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            ms(start)
        })
        .collect();
    print("nothing changed", samples);
    let ((), counts) = alloc::count_in_scope(|| {
        frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    });
    println!("  allocations: {}", counts.allocations);

    // A bar moves: a value read differently, no layout, no text.
    let samples = (0..SAMPLES)
        .map(|index| {
            model.player.health = (index % 100) as f32 * 0.01;
            let start = Instant::now();
            frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            ms(start)
        })
        .collect();
    print("every row's bar moved", samples);

    // A count changes on all twenty rows: the string resolved, shaped once and measured twenty times, and laid
    // out again. Numbers past the first run of each come from the shaping cache, as a real value's do.
    let samples = (0..SAMPLES)
        .map(|index| {
            model.player.enemies = index as i32;
            let start = Instant::now();
            frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            ms(start)
        })
        .collect();
    print("every row's number changed, new numbers", samples);
    let samples = (0..SAMPLES)
        .map(|index| {
            model.player.enemies = index as i32;
            let start = Instant::now();
            frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            ms(start)
        })
        .collect();
    print("every row's number changed, numbers seen", samples);

    // The window changes size: layout and every rectangle again, and no string.
    let samples = (0..SAMPLES)
        .map(|index| {
            let wide = metrics([1000.0 + (index % 200) as f32, 1000.0]);
            let start = Instant::now();
            frame(&mut tree, &mut fonts, &loc, &model, wide, &atlas, &mut list);
            ms(start)
        })
        .collect();
    print("the window resized, layout every frame", samples);

    // The language changes: every string again, once, which is a frame a player only sees in a menu.
    let mut samples = Vec::new();
    for index in 0..40 {
        loc.set_text_locale(if index % 2 == 0 { "en" } else { "ja" }).unwrap();
        let start = Instant::now();
        frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
        samples.push(ms(start));
    }
    print("the language changed", samples);

    // And nothing again, for how much of the above is the machine.
    let samples = (0..SAMPLES)
        .map(|_| {
            let start = Instant::now();
            frame(&mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            ms(start)
        })
        .collect();
    print("nothing changed, again", samples);
    println!("§3.2's UI row: 0.3 ms desktop, 1.0 ms mobile");
}

/// The same menu moving: all of it fading, sliding and growing, then all of it scrolling past a smaller
/// frame, then what a press costs. The frames are on the clock a game hands the tree, a sixtieth apart.
#[test]
#[ignore = "prints costs"]
fn motion_costs() {
    let document = menu_with(
        "mask: true, scroll: Vertical,",
        "focus: (),",
        r#"timelines: [(name: "fade", looping: true, tracks: [
             (node: "root", property: Opacity, keys: [(at: 0, value: 0.1), (at: 0.25, value: 1, ease: Out), (at: 0.5, value: 0.1)]),
             (node: "root", property: Y, keys: [(at: 0, value: 24), (at: 0.25, value: 0, ease: Out), (at: 0.5, value: 24)]),
             (node: "root", property: Scale, keys: [(at: 0, value: 0.96), (at: 0.25, value: 1, ease: Out), (at: 0.5, value: 0.96)]),
           ])],"#,
    );
    let loc = localizer("ja");
    let (mut fonts, atlas, mut list) = (fonts(), Atlas::paint(), UiDrawList::default());
    let size = metrics([1280.0, 400.0]);
    let model = model();
    let mut tree = UiTree::new(&document).unwrap();
    let mut now = 0.0;
    let mut clock = || {
        now += 1.0 / 60.0;
        at(now)
    };
    for _ in 0..3 {
        frame_at(clock(), &mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
    }
    assert_eq!(tree.problems(), &[]);
    println!("{} nodes, {} quads in the frame", list.tree.nodes, list.quads.len());

    assert!(tree.play("fade", clock()));
    let mut samples = Vec::with_capacity(SAMPLES);
    let ((), counts) = alloc::count_in_scope(|| {
        for _ in 0..SAMPLES {
            let time = clock();
            let start = Instant::now();
            frame_at(time, &mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            samples.push(ms(start));
        }
    });
    print("all of it fading, sliding and growing", samples);
    println!(
        "  allocations: {} in {SAMPLES} frames, {} layouts in the last",
        counts.allocations, list.tree.layouts
    );
    assert!(tree.stop("fade", clock()));

    let mut samples = Vec::with_capacity(SAMPLES);
    let ((), counts) = alloc::count_in_scope(|| {
        for index in 0..SAMPLES {
            let time = clock();
            let scroll = Vec2::new(0.0, if index / 30 % 2 == 0 { 24.0 } else { -24.0 });
            let start = Instant::now();
            tree.input(&UiInput { scroll, ..UiInput::default() }, time);
            frame_at(time, &mut tree, &mut fonts, &loc, &model, size, &atlas, &mut list);
            samples.push(ms(start));
        }
    });
    print("all of it scrolling", samples);
    println!(
        "  allocations: {} in {SAMPLES} frames, {} layouts in the last",
        counts.allocations, list.tree.layouts
    );

    let mut samples = Vec::with_capacity(SAMPLES);
    let mut focusable = 0;
    let ((), counts) = alloc::count_in_scope(|| {
        for index in 0..SAMPLES {
            let point = Vec2::new(100.0 + (index % 800) as f32, (index % 400) as f32);
            let start = Instant::now();
            let hit = std::hint::black_box(tree.hit(point));
            samples.push(ms(start));
            focusable += usize::from(matches!(hit, UiHit::Focusable(_)));
        }
    });
    print("a hit test", samples);
    println!("  allocations: {} in {SAMPLES} tests, {focusable} of them on a row", counts.allocations);

    let mut samples = Vec::with_capacity(SAMPLES);
    let mut moved = 0;
    for index in 0..SAMPLES {
        let step = if index / 19 % 2 == 0 { UiStep::Down } else { UiStep::Up };
        let time = clock();
        let start = Instant::now();
        tree.input(&UiInput { step: Some(step), ..UiInput::default() }, time);
        samples.push(ms(start));
        moved += tree.drain_events().count();
    }
    print("a step of focus, reaching for the next row", samples);
    println!("  {moved} moves in {SAMPLES} steps");
    println!("§3.2's UI row: 0.3 ms desktop, 1.0 ms mobile");
}
