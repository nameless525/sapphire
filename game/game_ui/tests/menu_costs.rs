//! What the game's menus cost a frame once warm, against spec §3.2's UI line of 0.3 ms desktop and 1.0 ms
//! mobile, and that they allocate nothing. Installs `CountingAllocator` as this binary's global allocator.
//!
//! `cargo test --release -p game_ui --test menu_costs -- --ignored --nocapture` prints the costs.

#![allow(clippy::print_stdout, reason = "printing the numbers is the ignored test's whole job")]
#![allow(clippy::disallowed_methods, reason = "timing them needs the OS clock, and nothing here ships")]

mod support;

use std::alloc::System;
use std::time::Instant;

use game_ui::menus::{GameMenus, MenuScreen, MenuSetup, PlayerSettings};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_input::RawInput;
use sapphire_look::UiDrawList;
use sapphire_platform::{Scancode, Timestamp};
use sapphire_ui::Atlas;
use support::assets;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const SIZE: (u32, u32) = (1280, 720);
/// How many frames a measure takes: a good minute of a menu.
const FRAMES: u64 = 4000;
/// How often focus moves, in frames: a held stick's repeat is about this.
const STEP_EVERY: u64 = 12;

fn at(frame: u64) -> Timestamp {
    Timestamp::from_nanos(1_000_000_000 + frame * 16_666_667)
}

/// Japanese, the costlier language, with the fight's words and `over` on top.
fn menus(over: Option<MenuScreen>) -> (GameMenus, UiDrawList) {
    let settings = PlayerSettings { language: "ja".to_owned(), ..PlayerSettings::default() };
    let setup = MenuSetup { settings, can_leave: true, pseudo: false };
    let mut menus = GameMenus::new(assets(), Atlas::paint(), setup).unwrap();
    menus.open(MenuScreen::Hud);
    if let Some(screen) = over {
        menus.open(screen);
    }
    let mut list = UiDrawList::default();
    // In, and every row's look and its kick seen once.
    for frame in 0..200 {
        frame_at(&mut menus, frame, &mut list);
    }
    (menus, list)
}

/// A frame of it: focus going up and down a list, and what's shown drawn.
fn frame_at(menus: &mut GameMenus, frame: u64, list: &mut UiDrawList) {
    let scancode = if frame / STEP_EVERY % 8 < 4 { Scancode::DOWN } else { Scancode::UP };
    let down = match frame % STEP_EVERY {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    };
    if let Some(down) = down {
        menus.controls_mut().push(at(frame), RawInput::Key { scancode, down });
    }
    menus.update(at(frame));
    menus.take_commands().for_each(drop);
    list.reset();
    menus.draw(SIZE, list);
}

#[test]
fn a_menu_frame_allocates_nothing_once_warm() {
    if !PER_THREAD_COUNTS {
        // Process-wide totals can't tell this thread's allocations from the test harness's.
        return;
    }
    for over in [None, Some(MenuScreen::Pause), Some(MenuScreen::Settings)] {
        let (mut menus, mut list) = menus(over);
        let ((), counts) = alloc::count_in_scope(|| {
            for frame in 200..1200 {
                frame_at(&mut menus, frame, &mut list);
            }
        });
        assert_eq!(counts.allocations, 0, "{over:?}: {counts:?}");
    }
}

#[test]
#[ignore = "prints costs"]
fn costs() {
    for (label, over) in [
        ("the HUD's words", None),
        ("the pause over them", Some(MenuScreen::Pause)),
        ("the settings", Some(MenuScreen::Settings)),
        ("the character screen", Some(MenuScreen::Character)),
        ("the dialogue panel", Some(MenuScreen::Dialogue)),
    ] {
        let (mut menus, mut list) = menus(over);
        let mut samples: Vec<f64> = (200..200 + FRAMES)
            .map(|frame| {
                let start = Instant::now();
                frame_at(&mut menus, frame, &mut list);
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        let share = |share: f64| samples[((samples.len() - 1) as f64 * share).round() as usize];
        println!(
            "{label:<24} {:>4} quads   p50 {:>7.4} ms   p99 {:>7.4} ms",
            list.quads.len(),
            share(0.5),
            share(0.99)
        );
    }
}
