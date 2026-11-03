//! A frame of a retained screen where nothing changed allocates nothing, whatever the screen binds,
//! wraps or masks. Installs `CountingAllocator` as this test binary's global allocator.

mod common;

use std::alloc::System;

use common::{Model, Player, document, fonts, frame, localizer, metrics};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_look::UiDrawList;
use sapphire_ui::{Atlas, UiTree};

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
