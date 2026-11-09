//! Steady-state frames allocate nothing: fingers landing, moving, holding and lifting, the update,
//! animation and drawing into a list that has grown. Installs `CountingAllocator` as this test
//! binary's global allocator.

use std::alloc::System;

use game_ui::touch::TouchControls;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_math::Vec2;
use sapphire_platform::Timestamp;
use sapphire_platform::{FingerId, TouchDeviceId, TouchPhase};
use sapphire_ui::{
    CombatHud, DamageKind, FighterValues, HudLayout, HudValues, Insets, SkillValues, UiDrawList, UiMetrics,
    UltimateValues,
};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

fn at(frame: u64, ms: u64) -> Timestamp {
    Timestamp::from_nanos((frame * 16 + ms) * 1_000_000)
}

fn frame(frame: u64, controls: &mut TouchControls, hud: &mut CombatHud, list: &mut UiDrawList) {
    let layout = *controls.layout();
    let finger = |id| FingerId::new(TouchDeviceId::from_raw(1), id);
    let x = (frame % 20) as f32;
    // The left thumb stays on the stick, moving every frame.
    if frame.is_multiple_of(40) {
        controls.touch(at(frame, 0), finger(1), TouchPhase::Down, layout.stick.rest);
    }
    controls.touch(at(frame, 1), finger(1), TouchPhase::Moved, layout.stick.rest + Vec2::new(x, -x));
    // A second finger turns the camera in bursts; the right thumb taps and holds.
    match frame % 20 {
        0 => {
            controls.touch(at(frame, 2), finger(2), TouchPhase::Down, Vec2::new(420.0, 120.0));
        }
        1..=5 => {
            controls.touch(at(frame, 2), finger(2), TouchPhase::Moved, Vec2::new(420.0 + x, 120.0));
        }
        6 => {
            controls.touch(at(frame, 2), finger(2), TouchPhase::Canceled, Vec2::new(420.0 + x, 120.0));
        }
        _ => {}
    }
    match frame % 30 {
        2 => {
            controls.touch(at(frame, 3), finger(3), TouchPhase::Down, layout.attack.center);
        }
        3..=25 => {
            controls.touch(
                at(frame, 3),
                finger(3),
                TouchPhase::Moved,
                layout.attack.center + Vec2::new(0.0, -x * 2.0),
            );
        }
        26 => {
            controls.touch(at(frame, 3), finger(3), TouchPhase::Up, layout.attack.center);
        }
        _ => {}
    }
    if frame.is_multiple_of(97) {
        controls.release_all(at(frame, 4));
    }
    let now = at(frame, 10);
    let actions = controls.update(now);
    let share = (frame % 100) as f32 / 100.0;
    let values = HudValues {
        player: FighterValues { hp: 1.0 - share, poise: share, hp_points: Some(6480 - frame as u32 % 100) },
        enemy: Some(FighterValues { hp: share, poise: 1.0 - share, hp_points: Some(frame as u32 % 100) }),
        skill: SkillValues { cooldown: 1.0 - share, seconds: Some(frame as u32 % 7) },
        ultimate: Some(UltimateValues { energy: share, ready: frame % 100 >= 90 }),
        ..HudValues::default()
    };
    hud.update(now, &values, actions);
    if frame.is_multiple_of(3) {
        hud.spawn_damage(frame as u32, Vec2::new(320.0, 180.0), DamageKind::Normal);
    }
    // A pause's frames draw more than the others, and it comes back.
    hud.show_pause(frame % 50 < 25);
    list.reset();
    hud.draw(&layout, actions, list);
}

#[test]
fn steady_state_frames_allocate_nothing() {
    if !PER_THREAD_COUNTS {
        // Process-wide totals can't tell this thread's allocations from the test harness's.
        return;
    }
    let layout = HudLayout::new(UiMetrics::new(Vec2::new(640.0, 360.0), Insets::default()));
    let mut controls = TouchControls::new(layout);
    let mut hud = CombatHud::new();
    let mut list = UiDrawList::default();
    // The lists grow to hold a frame's quads and a burst of events.
    for index in 0..200 {
        frame(index, &mut controls, &mut hud, &mut list);
    }
    let ((), counts) = alloc::count_in_scope(|| {
        for index in 200..2200 {
            frame(index, &mut controls, &mut hud, &mut list);
        }
    });
    assert_eq!(counts.allocations, 0, "{counts:?}");
}
