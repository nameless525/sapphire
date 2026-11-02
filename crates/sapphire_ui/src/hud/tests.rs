use std::fmt::Write as _;

use sapphire_look::UiQuad;

use super::*;
use crate::layout::Insets;
use crate::touch::TouchControls;

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

/// iPhone 17 Pro's landscape insets, about.
const NOTCHED: Insets = Insets { left: 62.0, top: 0.0, right: 62.0, bottom: 21.0 };
/// A notch that's on the top instead, and a tall bar on the left: nothing should cross either.
const STRANGE: Insets = Insets { left: 90.0, top: 48.0, right: 10.0, bottom: 34.0 };

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

/// A rectangle's own area plus how far `SLANT` leans it.
fn leaned(rect: Rect) -> Rect {
    let lean = Vec2::new(SLANT * rect.height() * 0.5, 0.0);
    Rect::new(rect.min - lean, rect.max + lean)
}

fn hit_circle(button: &ButtonLayout) -> Rect {
    Rect::around(button.center, Vec2::splat(button.hit))
}

/// Every problem with `layout`, for one frame.
fn problems(name: &str, layout: &HudLayout) -> String {
    let mut problems = String::new();
    let safe = layout.safe;
    let mut fail = |what: String| {
        let _ = writeln!(problems, "{name}: {what}");
    };
    let drawn = [
        ("attack", leaned(layout.attack.rect())),
        ("dodge", leaned(layout.dodge.rect())),
        ("skill", leaned(layout.skill.rect())),
        ("player's health", leaned(layout.player.hp)),
        ("player's poise", leaned(layout.player.poise)),
        ("enemy's health", leaned(layout.enemy.hp)),
        ("enemy's poise", leaned(layout.enemy.poise)),
        ("the stick at rest", Rect::around(layout.stick.rest, Vec2::splat(layout.stick.radius))),
    ];
    for (index, (what, rect)) in drawn.iter().enumerate() {
        if !safe.contains_rect(*rect) {
            fail(format!("{what} {rect:?} leaves the safe area {safe:?}"));
        }
        for (other, against) in &drawn[index + 1..] {
            if rect.overlaps(*against) {
                fail(format!("{what} {rect:?} overlaps {other} {against:?}"));
            }
        }
    }
    let buttons = [("attack", &layout.attack), ("dodge", &layout.dodge), ("skill", &layout.skill)];
    for (index, (what, button)) in buttons.iter().enumerate() {
        if !safe.contains_rect(hit_circle(button)) {
            fail(format!("{what}'s hit circle {:?} leaves the safe area", hit_circle(button)));
        }
        if button.hit < button.half {
            fail(format!("{what} can't be pressed everywhere it shows"));
        }
        for (other, against) in &buttons[index + 1..] {
            if button.center.distance(against.center) < button.hit + against.hit {
                fail(format!("{what}'s and {other}'s hit circles overlap"));
            }
        }
        if layout.stick.zone.overlaps(hit_circle(button)) {
            fail(format!("the stick's zone reaches {what}"));
        }
        // A finger's reach, on a phone: 44 points across at least (Apple's minimum). A smaller
        // frame scales everything, this too.
        if button.hit * 2.0 < 44.0 * layout.scale {
            fail(format!("{what}'s hit circle is too small for a thumb: {}", button.hit * 2.0));
        }
    }
    if !safe.contains_rect(layout.stick.zone) {
        fail(format!("the stick's zone {:?} leaves the safe area", layout.stick.zone));
    }
    if !layout.stick.zone.contains(layout.stick.rest) {
        fail("the stick rests outside its own zone".to_owned());
    }
    problems
}

#[test]
fn every_layout_holds_inside_the_safe_area_with_nothing_overlapping() {
    let failures: String =
        frames().iter().map(|(name, metrics)| problems(name, &HudLayout::new(*metrics))).collect();
    assert!(failures.is_empty(), "\n{failures}");
}

#[test]
fn anchors_keep_each_widget_s_distance_from_its_edge_across_aspect_ratios() {
    // Each width's safe area is at least as wide as a phone's, so nothing scales here. 4:3 with a
    // notch, which no device has, shrinks to fit instead: see `every_layout_holds`.
    for (first_width, insets) in [(480.0, Insets::default()), (640.0, NOTCHED)] {
        let layouts = [first_width, 720.0, 840.0, 1100.0]
            .map(|width| HudLayout::new(UiMetrics::new(Vec2::new(width, 360.0), insets)));
        let first = &layouts[0];
        for layout in &layouts {
            let safe = layout.safe;
            let first_safe = first.safe;
            assert_eq!(layout.scale, 1.0);
            // The right thumb's cluster stays on the corner, the left thumb's stick on its own.
            assert_eq!(safe.max - layout.attack.center, first_safe.max - first.attack.center);
            assert_eq!(safe.max - layout.dodge.center, first_safe.max - first.dodge.center);
            assert_eq!(safe.max - layout.skill.center, first_safe.max - first.skill.center);
            assert_eq!(layout.stick.rest - safe.min, first.stick.rest - first_safe.min);
            // The player's bars on the top left, the enemy's on the middle of the top.
            assert_eq!(layout.player.hp.min - safe.min, first.player.hp.min - first_safe.min);
            let middle = safe.center().x;
            assert!((layout.enemy.hp.center().x - middle).abs() < 1e-3);
            assert_eq!(layout.enemy.hp.min.y - safe.min.y, first.enemy.hp.min.y - first_safe.min.y);
        }
    }
}

#[test]
fn insets_move_the_controls_by_what_they_cover() {
    let plain = HudLayout::new(UiMetrics::new(Vec2::new(874.0, 402.0), Insets::default()));
    let notched = HudLayout::new(UiMetrics::new(Vec2::new(874.0, 402.0), NOTCHED));
    assert_eq!(notched.attack.center - plain.attack.center, Vec2::new(-NOTCHED.right, -NOTCHED.bottom));
    assert_eq!(notched.stick.rest - plain.stick.rest, Vec2::new(NOTCHED.left, -NOTCHED.bottom));
    assert_eq!(notched.player.hp.min.x - plain.player.hp.min.x, NOTCHED.left);
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
fn a_draw_names_its_atlas_and_viewport_and_counts_its_quads() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let mut hud = CombatHud::new();
    let actions = TouchActions::default();
    hud.update(ms(0), &values(), &actions);
    let mut list = UiDrawList::default();
    let stats = hud.draw(&layout, &actions, &mut list);
    assert_eq!(list.viewport, [640.0, 360.0]);
    assert!(Arc::ptr_eq(list.atlas.as_ref().unwrap(), hud.atlas()));
    assert_eq!(stats.quads as usize, list.quads.len());
    assert!(stats.quads < 64, "{} quads for a frame with no damage numbers", stats.quads);
    // A second draw appends, for a caller drawing more over it.
    let stats = hud.draw(&layout, &actions, &mut list);
    assert_eq!(stats.quads as usize * 2, list.quads.len());
}

#[test]
fn a_fighter_with_no_enemy_draws_fewer_bars() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let with = draw(&layout, &values(), |_| {}).quads.len();
    let alone = draw(&layout, &HudValues { enemy: None, ..values() }, |_| {}).quads.len();
    assert!(alone < with);
}

#[test]
fn the_trailing_segment_holds_then_catches_up_on_the_real_clock() {
    let mut trail = Trail::default();
    trail.update(0.8, 0.0);
    assert_eq!((trail.value, trail.trail), (0.8, 0.8), "the first value shows no loss");
    trail.update(0.5, 0.016);
    assert_eq!((trail.value, trail.trail), (0.5, 0.8), "the loss shows");
    // Inside the hold nothing moves, however frames fall.
    for _ in 0..20 {
        trail.update(0.5, 0.016);
    }
    assert_eq!(trail.trail, 0.8);
    for _ in 0..3 {
        trail.update(0.5, 0.016);
    }
    assert!(trail.hold <= TRAIL_HOLD);
    // Past it, it closes at its rate down to the value, no further.
    for _ in 0..200 {
        trail.update(0.5, 0.016);
    }
    assert_eq!(trail.trail, 0.5);
    // A second hit in the catching up holds again.
    trail.update(0.3, 0.016);
    assert_eq!(trail.trail, 0.5);
    // A heal takes the segment along at once.
    trail.update(0.9, 0.016);
    assert_eq!((trail.value, trail.trail), (0.9, 0.9));
}

#[test]
fn a_hit_shows_in_the_bar_s_quads_through_the_real_clock_alone() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let actions = TouchActions::default();
    let mut hud = CombatHud::new();
    let at = |hud: &mut CombatHud, time: u64, hp: f32| {
        let values =
            HudValues { player: FighterValues { hp, poise: 0.5, hp_points: None }, ..HudValues::default() };
        hud.update(ms(time), &values, &actions);
        let mut list = UiDrawList::default();
        hud.draw(&layout, &actions, &mut list);
        list.quads.iter().filter(|quad| quad.color == EMBER).count()
    };
    assert_eq!(at(&mut hud, 0, 0.8), 0, "nothing lost yet");
    assert_eq!(at(&mut hud, 16, 0.6), 1, "a trailing segment");
    assert_eq!(at(&mut hud, 300, 0.6), 1, "held");
    assert_eq!(at(&mut hud, 3000, 0.6), 0, "caught up, however long the frame");
}

#[test]
fn a_damage_number_punches_in_floats_up_fades_and_goes() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let actions = TouchActions::default();
    let mut hud = CombatHud::new();
    hud.update(ms(1000), &HudValues::default(), &actions);
    let baseline = {
        let mut list = UiDrawList::default();
        hud.draw(&layout, &actions, &mut list);
        list.quads.len()
    };
    let spawned_at = Vec2::new(320.0, 180.0);
    hud.spawn_damage(1284, spawned_at, DamageKind::Normal);
    // The digits' quads at `time`: their height, the row's middle and their alpha.
    let snapshot = |hud: &mut CombatHud, time: u64| {
        hud.update(ms(time), &HudValues::default(), &actions);
        let mut list = UiDrawList::default();
        let stats = hud.draw(&layout, &actions, &mut list);
        let digits = &list.quads[baseline..];
        let shape = digits.first().map(|quad| {
            let [top, _, bottom, _] = quad_corners(quad);
            (bottom.y - top.y, (top.y + bottom.y) * 0.5, quad.color[3])
        });
        (digits.len(), stats.damage_numbers, shape)
    };
    let (count, shown, shape) = snapshot(&mut hud, 1000);
    assert_eq!((count, shown), (4, 1), "one number, four digits");
    let (born_height, born_y, born_alpha) = shape.unwrap();
    assert_eq!(born_alpha, 255);
    assert!((born_y - spawned_at.y).abs() < 1e-3, "at the hit");

    let (_, _, shape) = snapshot(&mut hud, 1140);
    let (settled_height, settled_y, _) = shape.unwrap();
    assert!((born_height / settled_height - 1.7).abs() < 1e-3, "punched in at 1.7×");
    assert!(settled_y < born_y, "rising");

    let (_, _, shape) = snapshot(&mut hud, 1800);
    let (_, late_y, late_alpha) = shape.unwrap();
    assert!(late_y < settled_y, "still rising");
    assert!(late_alpha < 255, "fading");

    let (count, shown, _) = snapshot(&mut hud, 1950);
    assert_eq!((count, shown), (0, 0), "gone at its end");
}

#[test]
fn damage_numbers_past_the_limit_replace_the_oldest() {
    let actions = TouchActions::default();
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let mut hud = CombatHud::new();
    hud.update(ms(0), &HudValues::default(), &actions);
    for value in 0..MAX_DAMAGE_NUMBERS as u32 + 3 {
        hud.spawn_damage(value, Vec2::new(100.0, 100.0), DamageKind::Heavy);
    }
    let mut list = UiDrawList::default();
    let stats = hud.draw(&layout, &actions, &mut list);
    assert_eq!(stats.damage_numbers, MAX_DAMAGE_NUMBERS as u32);
    assert!(hud.damage.iter().flatten().all(|number| number.value >= 3), "the first three went");
}

#[test]
fn the_cooldown_sweep_and_its_seconds_show_until_the_skill_is_ready_then_it_flashes() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let actions = TouchActions::default();
    let mut hud = CombatHud::new();
    let quads = |hud: &mut CombatHud, time: u64, cooldown: f32| {
        let values = HudValues {
            skill: SkillValues { cooldown, seconds: (cooldown > 0.0).then_some(2) },
            ..HudValues::default()
        };
        hud.update(ms(time), &values, &actions);
        let mut list = UiDrawList::default();
        hud.draw(&layout, &actions, &mut list);
        list.quads
    };
    let cooling = quads(&mut hud, 0, 0.25);
    let sweep: Vec<f32> = cooling.iter().map(|quad| quad.sweep).filter(|&sweep| sweep > 0.0).collect();
    assert_eq!(sweep, [0.25], "one wedge, the share still to go");

    let ready = quads(&mut hud, 16, 0.0);
    assert!(ready.iter().all(|quad| quad.sweep == 0.0));
    assert!(ready.iter().any(|quad| quad.outline_color[..3] == AMBER[..3]), "the flash");
    let settled = quads(&mut hud, 16 + (READY_FLASH * 1000.0) as u64 + 1, 0.0);
    assert!(settled.iter().all(|quad| quad.outline_color[..3] != AMBER[..3]), "over");
}

#[test]
fn values_outside_their_ranges_clamp_and_nan_reads_as_zero() {
    let bad = HudValues {
        player: FighterValues { hp: f32::NAN, poise: 7.0, hp_points: None },
        enemy: Some(FighterValues { hp: -1.0, poise: f32::INFINITY, hp_points: None }),
        skill: SkillValues { cooldown: f32::NAN, seconds: None },
    };
    let good = sanitized(&bad);
    assert_eq!((good.player.hp, good.player.poise), (0.0, 1.0));
    let enemy = good.enemy.unwrap();
    assert_eq!((enemy.hp, enemy.poise), (0.0, 1.0));
    assert_eq!(good.skill.cooldown, 0.0);
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
