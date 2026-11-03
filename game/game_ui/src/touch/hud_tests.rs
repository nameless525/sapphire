//! The HUD with fingers on the zones: what a thumb's landing draws, on every frame a layout must hold on.

use std::fmt::Write as _;

use sapphire_look::UiQuad;
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;
use sapphire_ui::{
    CombatHud, FighterValues, HudLayout, HudValues, Insets, Rect, SkillValues, UiDrawList, UiMetrics,
};

use super::TouchControls;

/// iPhone 17 Pro's landscape insets, about.
const NOTCHED: Insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
/// A notch that's on the top instead, and a tall bar on the left: nothing should cross either.
const STRANGE: Insets = Insets { left: 90.0, top: 48.0, right: 10.0, bottom: 34.0 };

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

/// Frames every layout must hold on: the three aspect ratios of spec §5.17, real devices, with and
/// without insets, and a window smaller than any phone, which shrinks to fit.
fn frames() -> Vec<(&'static str, UiMetrics)> {
    let frame = |w: f32, h: f32, insets| UiMetrics::new(Vec2::new(w, h), insets);
    vec![
        ("4:3", frame(480.0, 360.0, Insets::default())),
        ("16:9", frame(640.0, 360.0, Insets::default())),
        ("21:9", frame(840.0, 360.0, Insets::default())),
        ("4:3 with a notch", frame(480.0, 360.0, NOTCHED)),
        ("16:9 with a notch", frame(640.0, 360.0, NOTCHED)),
        ("21:9 with a notch", frame(840.0, 360.0, NOTCHED)),
        ("16:9 strange", frame(640.0, 360.0, STRANGE)),
        ("iPhone 17 Pro", frame(874.0, 402.0, NOTCHED)),
        ("iPad Air", frame(1180.0, 820.0, Insets { bottom: 20.0, ..Insets::default() })),
        ("a wide Android phone", frame(914.0, 411.0, Insets { left: 32.0, ..Insets::default() })),
        ("a desktop", frame(1920.0, 1080.0, Insets::default())),
        ("a tiny window", frame(320.0, 180.0, Insets::default())),
    ]
}

fn values() -> HudValues {
    HudValues {
        player: FighterValues { hp: 0.8, poise: 0.5, hp_points: Some(6480) },
        enemy: Some(FighterValues { hp: 0.6, poise: 0.3, hp_points: None }),
        skill: SkillValues { cooldown: 0.5, seconds: Some(3) },
    }
}

fn quad_corners(quad: &UiQuad) -> [Vec2; 4] {
    let origin = Vec2::from(quad.origin);
    let (x, y) = (Vec2::from(quad.axis_x), Vec2::from(quad.axis_y));
    [origin, origin + x, origin + y, origin + x + y]
}

/// The list a HUD with `values` draws at `layout`, with fingers in `set_up`.
fn draw(layout: &HudLayout, values: &HudValues, set_up: impl FnOnce(&mut TouchControls)) -> UiDrawList {
    let mut controls = TouchControls::new(*layout);
    set_up(&mut controls);
    let mut hud = CombatHud::new();
    let actions = controls.update(ms(0));
    hud.update(ms(0), values, actions);
    let mut list = UiDrawList::default();
    hud.draw(layout, actions, &mut list);
    list
}

#[test]
fn every_quad_stays_on_the_frame_with_nothing_held_or_with_everything_held() {
    let mut failures = String::new();
    for (name, metrics) in frames() {
        let layout = HudLayout::new(metrics);
        let held = |controls: &mut TouchControls| {
            for (id, position) in
                [layout.stick.rest, layout.attack.center, layout.dodge.center, layout.skill.center]
                    .into_iter()
                    .enumerate()
            {
                let finger = sapphire_platform::FingerId::new(
                    sapphire_platform::TouchDeviceId::from_raw(1),
                    id as u64,
                );
                controls.touch(ms(0), finger, sapphire_platform::TouchPhase::Down, position);
            }
        };
        let idle = draw(&layout, &values(), |_| {});
        let busy = draw(&layout, &values(), held);
        for quad in idle.quads.iter().chain(&busy.quads) {
            for corner in quad_corners(quad) {
                // A cell's empty margin may hang over the edge, not more.
                let slop = Vec2::splat(16.0);
                let frame = Rect::new(-slop, metrics.size + slop);
                if !frame.contains(corner) {
                    let _ = writeln!(failures, "{name}: a quad's corner at {corner}");
                }
            }
        }
    }
    assert!(failures.is_empty(), "\n{failures}");
}

#[test]
fn a_held_stick_draws_under_the_thumb_inside_the_safe_area() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(874.0, 402.0), NOTCHED));
    let finger = sapphire_platform::FingerId::new(sapphire_platform::TouchDeviceId::from_raw(1), 1);
    let landing = layout.stick.zone.center();
    let list = draw(&layout, &values(), |controls| {
        controls.touch(ms(0), finger, sapphire_platform::TouchPhase::Down, landing);
    });
    let centers: Vec<Vec2> = list
        .quads
        .iter()
        .map(|quad| {
            let [first, _, _, last] = quad_corners(quad);
            (first + last) * 0.5
        })
        .collect();
    assert!(centers.iter().any(|center| center.distance(landing) < 1e-3), "under the thumb");
    assert!(!centers.iter().any(|center| center.distance(layout.stick.rest) < 1e-3), "not at its rest");
    // A landing against the safe area's edge keeps the ring on it.
    let edge = Vec2::new(layout.safe.min.x, layout.safe.max.y);
    let list = draw(&layout, &values(), |controls| {
        controls.touch(ms(0), finger, sapphire_platform::TouchPhase::Down, edge);
    });
    let inset = edge + Vec2::new(layout.stick.radius, -layout.stick.radius);
    assert!(
        list.quads.iter().any(|quad| {
            let [first, _, _, last] = quad_corners(quad);
            ((first + last) * 0.5).distance(inset) < 1e-3
        }),
        "the ring stays whole inside the safe area"
    );
}
