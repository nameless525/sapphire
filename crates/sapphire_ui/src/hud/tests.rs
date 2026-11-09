use std::fmt::Write as _;

use sapphire_look::UiQuad;

use super::*;
use crate::layout::Insets;

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
        // With its energy's ring around it.
        (
            "ultimate",
            leaned(Rect::around(layout.ultimate.center, Vec2::splat(layout.ultimate.half * ULTIMATE_RING))),
        ),
        ("player's health", leaned(layout.player.hp)),
        ("player's energy", leaned(layout.energy)),
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
    let buttons = [
        ("attack", &layout.attack),
        ("dodge", &layout.dodge),
        ("skill", &layout.skill),
        ("ultimate", &layout.ultimate),
    ];
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
            assert_eq!(safe.max - layout.ultimate.center, first_safe.max - first.ultimate.center);
            assert_eq!(layout.energy.min - safe.min, first.energy.min - first_safe.min);
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
        ..HudValues::default()
    }
}

fn quad_corners(quad: &UiQuad) -> [Vec2; 4] {
    let origin = Vec2::from(quad.origin);
    let (x, y) = (Vec2::from(quad.axis_x), Vec2::from(quad.axis_y));
    [origin, origin + x, origin + y, origin + x + y]
}

/// The list a HUD with `values` draws at `layout`, with nobody on the zones.
fn draw(layout: &HudLayout, values: &HudValues) -> UiDrawList {
    let mut hud = CombatHud::new();
    let actions = TouchActions::default();
    hud.update(ms(0), values, &actions);
    let mut list = UiDrawList::default();
    hud.draw(layout, &actions, &mut list);
    list
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
    let with = draw(&layout, &values()).quads.len();
    let alone = draw(&layout, &HudValues { enemy: None, ..values() }).quads.len();
    assert!(alone < with);
}

/// A boss's bars draw in the boss's place, wider than a foe's, with an inked mark across its health for each
/// phase it has room for, and none at either end of the bar (M3 B3).
#[test]
fn a_boss_draws_its_bars_in_its_own_place_with_a_mark_for_each_phase() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(874.0, 402.0), Insets::default()));
    assert!(layout.boss.hp.width() > layout.enemy.hp.width(), "wider than a foe's");
    assert!(layout.boss.hp.height() > layout.enemy.hp.height(), "and taller");
    let foe = draw(&layout, &values()).quads;
    let mut boss = BossBar::default();
    for share in [0.75, 0.5, 0.25, 0.0, 1.0, f32::NAN] {
        boss.mark(share);
    }
    assert_eq!(boss.marks(), [0.75, 0.5, 0.25], "an end, or no number, says nothing");
    let drawn = draw(&layout, &HudValues { boss: Some(boss), ..values() }).quads;
    assert_eq!(drawn.len(), foe.len() + 3, "a mark each");
    let inked = drawn.iter().filter(|quad| quad.color[..3] == INK[..3]).count();
    assert_eq!(inked, foe.iter().filter(|quad| quad.color[..3] == INK[..3]).count() + 3);
    let alone = draw(&layout, &HudValues { enemy: None, boss: Some(boss), ..values() }).quads;
    assert_eq!(
        alone.len(),
        draw(&layout, &HudValues { enemy: None, ..values() }).quads.len(),
        "no foe, no marks"
    );

    let mut full = BossBar::default();
    for _ in 0..MAX_PHASE_MARKS + 2 {
        full.mark(0.5);
    }
    assert_eq!(full.marks().len(), MAX_PHASE_MARKS, "what it has no room for isn't shown");
}

/// A small bar for each other foe, in a row under the enemy's, each inside it however narrow the frame. With
/// none, it draws what it always drew.
#[test]
fn other_foes_get_a_small_bar_each() {
    let drawn = |metrics: UiMetrics, count: usize| {
        let layout = HudLayout::new(metrics);
        let mut others = OtherFoes::default();
        for _ in 0..count {
            others.push(0.5);
        }
        let list = draw(&layout, &HudValues { others, ..values() });
        (layout, list.quads)
    };
    for size in [Vec2::new(640.0, 360.0), Vec2::new(480.0, 360.0), Vec2::new(2000.0, 900.0)] {
        let metrics = UiMetrics::new(size, Insets::default());
        let (layout, none) = drawn(metrics, 0);
        assert!(layout.others.min.y >= layout.enemy.poise.max.y, "it's under the bars");
        let (_, full) = drawn(metrics, MAX_OTHER_FOES);
        assert_eq!(full.len(), none.len() + 2 * MAX_OTHER_FOES, "{size}: a case and its fill");
    }

    let mut foes = OtherFoes::default();
    for _ in 0..MAX_OTHER_FOES + 3 {
        foes.push(1.0);
    }
    assert_eq!(foes.bars().len(), MAX_OTHER_FOES, "what it has no room for isn't shown");
}

/// A pause adds a veil over all of the frame and two bars in its middle, on top of what was there and
/// nothing else, and off again draws what it drew. No scene pauses, so no golden moves.
#[test]
fn a_pause_lays_a_veil_over_the_frame_and_a_mark_in_its_middle() {
    let mut frames = frames();
    frames.push(("portrait", UiMetrics::new(Vec2::new(360.0, 800.0), Insets::default())));
    let bar = Atlas::paint().sprites.bar;
    for (name, metrics) in frames {
        let layout = HudLayout::new(metrics);
        let mut hud = CombatHud::new();
        let actions = &TouchActions::default();
        hud.update(ms(0), &values(), actions);
        let draw = |hud: &CombatHud| {
            let mut list = UiDrawList::default();
            hud.draw(&layout, actions, &mut list);
            list.quads
        };
        let plain = draw(&hud);
        hud.show_pause(true);
        let paused = draw(&hud);
        assert_eq!(paused[..plain.len()], plain[..], "{name}: what was there stays");
        let [veil, first, second] = paused[plain.len()..] else {
            panic!("{name}: {} quads over the HUD", paused.len() - plain.len());
        };

        // The shape inside its cell's margin, less its corners' radius, covers the frame.
        let corners = quad_corners(&veil);
        let per_texel = veil.axis_y[1] / (bar.size[1] + 2.0 * crate::atlas::SPREAD);
        let inset = (crate::atlas::SPREAD + crate::atlas::BAR_CORNER) * per_texel;
        assert!(corners[0].x + inset <= 0.0 && corners[0].y + inset <= 0.0, "{name}: {corners:?}");
        assert!(
            corners[3].x - inset >= metrics.size.x && corners[3].y - inset >= metrics.size.y,
            "{name}: {corners:?}"
        );

        // The mark stands in the safe area's middle, a bar to each side, longer than wide and upright.
        let middle = metrics.safe().center();
        let center = |quad: &UiQuad| (quad_corners(quad)[0] + quad_corners(quad)[3]) * 0.5;
        assert!(center(&first).x < middle.x && middle.x < center(&second).x, "{name}");
        assert!(((center(&first) + center(&second)) * 0.5 - middle).length() < 1e-3, "{name}");
        assert_eq!(first.uv, bar.uv);
        assert!(
            first.axis_x[1] < 0.0 && first.axis_x[1].abs() > first.axis_y[0].abs(),
            "{name}: it stands up"
        );
        assert!(first.axis_x[0] > 0.0, "{name}: it leans as the rest do");

        hud.show_pause(false);
        assert_eq!(draw(&hud), plain, "{name}: off again");
    }
}

/// Who came in has less than who left, and no hit landed: no segment of health shows as lost, on their
/// bar alone. The same drop without a swap is a hit, and shows.
#[test]
fn someone_coming_in_with_less_shows_no_loss() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let mut hud = CombatHud::new();
    let actions = &TouchActions::default();
    let fighters = |player: f32, enemy: f32| HudValues {
        player: FighterValues { hp: player, ..FighterValues::default() },
        enemy: Some(FighterValues { hp: enemy, ..FighterValues::default() }),
        ..HudValues::default()
    };
    let losses = |hud: &CombatHud| {
        let mut list = UiDrawList::default();
        hud.draw(&layout, actions, &mut list);
        let shown = |quad: &&UiQuad| quad.color == EMBER;
        list.quads.iter().filter(shown).count()
    };
    hud.update(ms(0), &fighters(0.9, 0.8), actions);
    assert_eq!(losses(&hud), 0);
    hud.update(ms(16), &fighters(0.5, 0.8), actions);
    assert_eq!(losses(&hud), 1, "a hit shows");

    hud.fighters_came_in([true, false]);
    hud.update(ms(32), &fighters(0.3, 0.8), actions);
    assert_eq!(losses(&hud), 0, "nobody hit whoever came in, and the last one's loss went with them");
    hud.update(ms(48), &fighters(0.2, 0.8), actions);
    assert_eq!(losses(&hud), 1, "their first hit shows");

    // Each fighter's own: the enemy's loss stands through the player's swap, and goes with its own.
    hud.fighters_came_in([true, false]);
    hud.update(ms(64), &fighters(0.2, 0.6), actions);
    assert_eq!(losses(&hud), 1, "the enemy's hit, alone");
    hud.fighters_came_in([true, false]);
    hud.update(ms(80), &fighters(0.2, 0.6), actions);
    assert_eq!(losses(&hud), 1, "the enemy's trail doesn't start over with the player's");
    hud.fighters_came_in([false, true]);
    hud.update(ms(96), &fighters(0.2, 0.4), actions);
    assert_eq!(losses(&hud), 0, "its own does");
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

/// The numbers alive, by value.
fn numbers_alive(hud: &CombatHud) -> Vec<u32> {
    let mut alive: Vec<u32> = hud.damage.iter().flatten().map(|number| number.value).collect();
    alive.sort_unstable();
    alive
}

fn spawn(hud: &mut CombatHud, values: impl IntoIterator<Item = u32>) {
    for value in values {
        hud.spawn_damage(value, Vec2::new(100.0, 100.0), DamageKind::Normal);
    }
}

/// Review PLT-004: which slot a number is in says nothing of its age once one has expired and its slot
/// has been taken again.
#[test]
fn the_oldest_number_goes_whatever_slot_it_is_in() {
    let actions = TouchActions::default();
    let values = HudValues::default();
    let expired = ms(960);
    assert!(seconds_since(expired, ms(0)) >= DAMAGE_LIFE && seconds_since(expired, ms(100)) < DAMAGE_LIFE);

    // The first slot's number expires and a new one takes its place. The next past the limit replaces
    // the oldest left, and not the new one.
    let mut hud = CombatHud::new();
    hud.update(ms(0), &values, &actions);
    spawn(&mut hud, [0]);
    hud.update(ms(100), &values, &actions);
    spawn(&mut hud, 1..16);
    hud.update(expired, &values, &actions);
    assert_eq!(numbers_alive(&hud), (1..16).collect::<Vec<_>>());
    spawn(&mut hud, [100]);
    spawn(&mut hud, [101]);
    assert_eq!(numbers_alive(&hud), (2..16).chain([100, 101]).collect::<Vec<_>>());

    // Slots in the middle expire, some of them, and their refills outlive everything older.
    let mut hud = CombatHud::new();
    hud.update(ms(100), &values, &actions);
    spawn(&mut hud, 0..16);
    for number in hud.damage.iter_mut().flatten().filter(|number| (5..8).contains(&number.value)) {
        number.born = ms(0);
    }
    hud.update(expired, &values, &actions);
    assert_eq!(hud.damage.iter().flatten().count(), 13);
    spawn(&mut hud, [100, 101, 102]);
    spawn(&mut hud, [103, 104]);
    let kept: Vec<u32> = (2..5).chain(8..16).chain(100..105).collect();
    assert_eq!(numbers_alive(&hud), kept);

    // Numbers spawned in one frame have one time, and go in the order they came, however far past the
    // limit, round and round the slots.
    let mut hud = CombatHud::new();
    hud.update(ms(0), &values, &actions);
    spawn(&mut hud, 0..50);
    assert_eq!(numbers_alive(&hud), (34..50).collect::<Vec<_>>());

    // Clearing starts over from nothing.
    hud.clear_damage();
    spawn(&mut hud, 0..17);
    assert_eq!(numbers_alive(&hud), (1..17).collect::<Vec<_>>());
}

#[test]
fn clearing_takes_the_numbers_off_and_a_new_one_shows() {
    let actions = TouchActions::default();
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let mut hud = CombatHud::new();
    hud.update(ms(0), &HudValues::default(), &actions);
    for value in 0..MAX_DAMAGE_NUMBERS as u32 + 3 {
        hud.spawn_damage(value, Vec2::new(100.0, 100.0), DamageKind::Heavy);
    }
    hud.clear_damage();
    let mut list = UiDrawList::default();
    assert_eq!(hud.draw(&layout, &actions, &mut list).damage_numbers, 0);
    hud.spawn_damage(7, Vec2::new(100.0, 100.0), DamageKind::Normal);
    let mut list = UiDrawList::default();
    assert_eq!(hud.draw(&layout, &actions, &mut list).damage_numbers, 1);
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
        boss: None,
        skill: SkillValues { cooldown: f32::NAN, seconds: None },
        others: {
            let mut others = OtherFoes::default();
            others.push(f32::NAN);
            others.push(7.0);
            others
        },
        ultimate: Some(UltimateValues { energy: f32::NAN, ready: false }),
    };
    let good = sanitized(&bad);
    assert_eq!(good.others.bars(), &[0.0, 1.0]);
    assert_eq!((good.player.hp, good.player.poise), (0.0, 1.0));
    let enemy = good.enemy.unwrap();
    assert_eq!((enemy.hp, enemy.poise), (0.0, 1.0));
    assert_eq!(good.skill.cooldown, 0.0);
    assert_eq!(good.ultimate.map(|ultimate| ultimate.energy), Some(0.0));
    let full = sanitized(&HudValues {
        ultimate: Some(UltimateValues { energy: 3.0, ready: true }),
        ..HudValues::default()
    });
    assert_eq!(full.ultimate, Some(UltimateValues { energy: 1.0, ready: true }));
}

#[test]
fn a_design_with_a_field_it_doesnt_know_or_a_number_it_cant_lay_out_is_refused_by_name() {
    let shipped = include_str!("../../default_hud.ron");
    assert_eq!(&HudDesign::from_ron(shipped).expect("reads"), HudDesign::shipped());
    let refused = HudDesign::from_ron(&shipped.replacen("margin:", "marginn:", 1)).expect_err("a typo");
    assert!(refused.to_string().contains("marginn"), "{refused}");
    let refused = HudDesign::from_ron(&shipped.replacen("hp_height:", "hp_hieght:", 1)).expect_err("a typo");
    assert!(refused.to_string().contains("hp_hieght"), "{refused}");
    for (from, to, named) in [
        ("attack: (offset: (0.0, 0.0), half: 46.0", "attack: (offset: (0.0, 0.0), half: 0.0", "attack.half"),
        ("rest: (92.0, -84.0)", "rest: (NaN, -84.0)", "stick.rest.0"),
        ("poise_to: 0.86", "poise_to: 1.86", "enemy.poise_to"),
        ("mark_width: 2.5", "mark_width: 0.0", "boss.mark_width"),
    ] {
        let refused = HudDesign::from_ron(&shipped.replacen(from, to, 1)).expect_err(from);
        assert!(refused.to_string().contains(named), "{from}: {refused}");
    }
}

/// A design from before the ultimate (M3 Q3c) has no ultimate's button or energy meter, and lays them out
/// where the shipped one does: what's shipped is what a design with none gets.
#[test]
fn a_design_with_no_ultimate_gets_the_shipped_one() {
    let shipped = include_str!("../../default_hud.ron");
    let design = HudDesign::shipped();
    assert_eq!(design.ultimate, ButtonDesign::shipped_ultimate());
    assert_eq!(design.energy, MeterDesign::shipped_energy());
    let older: String = shipped
        .lines()
        .filter(|line| {
            !line.trim_start().starts_with("ultimate:") && !line.trim_start().starts_with("energy:")
        })
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(older, shipped, "both lines went");
    assert_eq!(&HudDesign::from_ron(&older).expect("reads"), design);
    for (from, to, named) in [
        (
            "ultimate: (offset: (-112.0, -112.0), half: 32.0",
            "ultimate: (offset: (-112.0, -112.0), half: -1.0",
            "ultimate.half",
        ),
        ("energy: (top: 52.0", "energy: (top: inf", "energy.top"),
        ("to: 0.62", "to: 1.62", "energy.to"),
    ] {
        let refused = HudDesign::from_ron(&shipped.replacen(from, to, 1)).expect_err(from);
        assert!(refused.to_string().contains(named), "{from}: {refused}");
    }
}

/// What the HUD draws of an ultimate: nothing without one; a violet meter and a ring swept as far as
/// it's full while it charges; a closed ring nearly white, the ink violet, once it's ready. The meter
/// draws with the bars, and the button only with the controls.
#[test]
fn an_ultimate_draws_its_meter_and_button_only_when_there_is_one() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let with = |ultimate| HudValues { ultimate, ..values() };
    let violet = |quads: &[UiQuad]| quads.iter().filter(|quad| quad.color == VIOLET).count();
    let swept = |quads: &[UiQuad]| -> Vec<f32> {
        quads.iter().map(|quad| quad.sweep).filter(|&sweep| sweep > 0.0).collect()
    };
    let outlined =
        |quads: &[UiQuad], color: [u8; 4]| quads.iter().filter(|quad| quad.outline_color == color).count();

    let none = draw(&layout, &with(None)).quads;
    assert_eq!(violet(&none), 0);
    assert_eq!(outlined(&none, VIOLET) + outlined(&none, LIT), 0);

    let charging = draw(&layout, &with(Some(UltimateValues { energy: 0.4, ready: false }))).quads;
    assert_eq!(violet(&charging), 1, "the meter's fill");
    // The skill's cooldown is the other wedge.
    assert_eq!(swept(&charging), [0.5, 0.4], "the ring, as far as it's full");
    assert_eq!(outlined(&charging, VIOLET), 1, "the ring");
    assert_eq!(outlined(&charging, LIT), 0, "not ready");
    assert!(charging.len() > none.len());
    // Everything else is the same quads with or without it, in the same order.
    let mut rest = charging.iter();
    assert!(none.iter().all(|quad| rest.any(|other| other == quad)), "it only adds");

    let empty = draw(&layout, &with(Some(UltimateValues { energy: 0.0, ready: false }))).quads;
    assert_eq!(violet(&empty), 0, "no fill for an empty meter");
    assert_eq!(swept(&empty), [0.5], "no arc either");

    let ready = draw(&layout, &with(Some(UltimateValues { energy: 1.0, ready: true }))).quads;
    assert_eq!(swept(&ready), [0.5], "the ring is closed");
    assert!(ready.iter().any(|quad| quad.color == LIT), "the meter's full and lit");
    assert!(ready.iter().any(|quad| quad.color == VIOLET_INK), "the button's ink is violet");
    assert!(outlined(&ready, LIT) >= 2, "the ring and the button's edge");

    // A pad in use: the meter stays, the button goes.
    let mut hud = CombatHud::new();
    let actions = TouchActions::default();
    hud.show_controls(false);
    hud.update(ms(0), &with(Some(UltimateValues { energy: 0.4, ready: false })), &actions);
    let mut list = UiDrawList::default();
    hud.draw(&layout, &actions, &mut list);
    assert_eq!(violet(&list.quads), 1, "the meter");
    assert_eq!(swept(&list.quads), Vec::<f32>::new(), "no button");
}

/// It flashes as it fills, as the skill does coming off cooldown, and not when it first shows up full.
#[test]
fn an_ultimate_flashes_as_it_becomes_ready_and_its_halo_breathes() {
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let actions = TouchActions::default();
    let at = |hud: &mut CombatHud, time: u64, energy: f32| {
        let values = HudValues {
            ultimate: Some(UltimateValues { energy, ready: energy >= 1.0 }),
            ..HudValues::default()
        };
        hud.update(ms(time), &values, &actions);
        let mut list = UiDrawList::default();
        hud.draw(&layout, &actions, &mut list);
        list.quads
    };
    // A flash is the one quad outlined in a faded white.
    let flashes = |quads: &[UiQuad]| {
        let flash = |quad: &&UiQuad| {
            quad.outline_color[..3] == LIT[..3] && quad.color == [0; 4] && quad.softness > 0.0
        };
        quads.iter().filter(flash).count()
    };
    let mut hud = CombatHud::new();
    assert_eq!(flashes(&at(&mut hud, 0, 1.0)), 0, "already full: no flash");
    let mut hud = CombatHud::new();
    assert_eq!(flashes(&at(&mut hud, 0, 0.9)), 0);
    assert_eq!(flashes(&at(&mut hud, 16, 1.0)), 1, "it got there");
    assert_eq!(flashes(&at(&mut hud, 16 + (READY_FLASH * 1000.0) as u64 + 1, 1.0)), 0, "over");

    // The halo breathes on the real clock, between its ends.
    let halo = |quads: &[UiQuad]| {
        quads
            .iter()
            .find(|quad| quad.color[..3] == VIOLET[..3] && quad.color[3] < 255)
            .map(|quad| quad.color[3])
    };
    let breaths: Vec<u8> = [0, 350, 700, 1050, 1400]
        .into_iter()
        .map(|time| halo(&at(&mut hud, 5600 + time, 1.0)).expect("a halo"))
        .collect();
    assert_eq!(breaths[0], breaths[4], "one breath");
    assert!(breaths[2] > breaths[1] && breaths[1] > breaths[0], "{breaths:?}");
    assert_eq!(breaths[1], breaths[3], "it falls as it rose");
}

/// A game moves a button by a document, and what's drawn and what takes a touch move with it.
#[test]
fn a_design_moves_what_it_says_and_nothing_else() {
    let metrics = UiMetrics::new(Vec2::new(874.0, 402.0), Insets::default());
    let shipped = HudLayout::new(metrics);
    let mut design = *HudDesign::shipped();
    design.dodge.offset = [-40.0, -140.0];
    design.dodge.hit = 50.0;
    let moved = HudLayout::designed(metrics, &design);
    assert_eq!(moved.dodge.center, shipped.attack.center + Vec2::new(-40.0, -140.0));
    assert_eq!(moved.dodge.hit, 50.0);
    assert_eq!((moved.attack, moved.skill, moved.stick), (shipped.attack, shipped.skill, shipped.stick));
    assert_eq!((moved.player, moved.enemy), (shipped.player, shipped.enemy));
}
