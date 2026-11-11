//! Exit test 6's text (M4 A2): every screen's smallest text, in pixels, in both languages, at the player's
//! default sizes and at their smallest, on a Steam Deck's 1280×800, at 1920×1080 and on a phone. Valve's
//! floor for a Deck (its compatibility page, read 2026-11-10) is 9 pixels at 1280×800, aiming for 12; it
//! doesn't say which height it means, so a text's size here is its em, the size its style asks for times
//! every scale on it. The default is held to 12 at 1280×800 and the smallest to 9 everywhere. And at the
//! largest text, every screen still fits, at every aspect a screen ships at: on a phone's, the text grows to
//! "large" at most.

mod support;

use game_ui::menus::{
    GameMenus, HudModel, MenuScreen, MenuSetup, PlayerSettings, SubtitleSize, Subtitles, TextSize,
};
use sapphire_look::UiDrawList;
use sapphire_platform::Timestamp;
use sapphire_ui::{Atlas, HudDesign, HudLayout, Insets, UiMetrics};
use support::assets;

/// The frames measured: their size in points, and pixels per point at the display's own scale. A phone's is an
/// iPhone's 2556×1179 at 3 pixels a point, the size the M4 simulator runs at.
const FRAMES: [(&str, (u32, u32), f32); 3] =
    [("1280×800", (1280, 800), 1.0), ("1920×1080", (1920, 1080), 1.0), ("phone", (852, 393), 3.0)];

/// Valve's floor, and what it recommends.
const FLOOR: f32 = 9.0;
const AIM: f32 = 12.0;

/// The aspects a screen ships at, in points, as exit test 4's walk has them: the Deck's 16:10 too (M4 Z3c).
const SIZES: [(u32, u32); 6] = [(1024, 768), (1280, 720), (1280, 800), (1680, 720), (844, 390), (667, 375)];

/// The player's smallest sizes: the interface's, and the subtitles'. The text's smallest is its default.
fn smallest(language: &str) -> PlayerSettings {
    PlayerSettings {
        language: language.to_owned(),
        size: "small".to_owned(),
        text_size: TextSize::Normal,
        subtitles: Subtitles { size: SubtitleSize::Small, ..Subtitles::default() },
        ..PlayerSettings::default()
    }
}

/// The largest text, and the largest subtitles, at the interface's default size. (At its largest size, the
/// title, the pause, the character screen and the dialogue had no room on a phone before M4 A2.)
fn largest(language: &str) -> PlayerSettings {
    PlayerSettings {
        language: language.to_owned(),
        size: "normal".to_owned(),
        text_size: TextSize::Larger,
        subtitles: Subtitles { size: SubtitleSize::Larger, ..Subtitles::default() },
        ..PlayerSettings::default()
    }
}

/// The game's menus as `settings` have them, as a playtest build has them: its pause has every row a store
/// build's has, and the mark (M4 F1).
fn menus_with(settings: PlayerSettings) -> GameMenus {
    let support::Screens { assets, menus: screens } = assets();
    GameMenus::new(
        assets,
        Atlas::paint(),
        MenuSetup {
            screens,
            settings,
            bindings: sapphire_controls::default_bindings().unwrap(),
            can_leave: true,
            can_mark: true,
            pseudo: false,
        },
    )
    .unwrap()
}

/// Each of the game's screens, alone, settled at `size` pixels: `look` sees it. The HUD says everything it
/// can: a banner, and a cutscene's subtitle, with its speaker.
fn each_screen(menus: &mut GameMenus, size: (u32, u32), mut look: impl FnMut(&str, &GameMenus, MenuScreen)) {
    let mut list = UiDrawList::default();
    for entry in &support::menus().screens {
        let screen = MenuScreen::named(&entry.name);
        menus.close_all();
        menus.close(MenuScreen::HUD);
        menus.open(screen);
        let subtitles = menus.settings().subtitles.clone();
        if let Some(hud) = menus.model_mut::<HudModel>(screen) {
            hud.has_banner = true;
            hud.follow_subtitle(Some("subtitle-court-gate-warning"), Some("speaker-warden"));
            hud.follow_subtitle_look(&subtitles);
        }
        for frame in 1..=30u64 {
            menus.update(Timestamp::from_nanos(1_000_000_000 + frame * 16_666_667));
            list.reset();
            menus.draw(size, &mut list);
        }
        look(&entry.name, menus, screen);
    }
}

/// The smallest text on every screen and the HUD's numbers, in pixels, with what it is.
fn smallest_text(settings: &PlayerSettings, (size, display): ((u32, u32), f32)) -> (f32, String) {
    let mut menus = menus_with(settings.clone());
    let mut least = (f32::INFINITY, String::new());
    each_screen(&mut menus, size, |name, menus, screen| {
        let tree = menus.tree(screen).expect("open");
        for (node, pixels) in tree.text_pixels() {
            let pixels = pixels * display;
            if pixels < least.0 {
                least = (pixels, format!("{name}'s {node}"));
            }
        }
    });
    // The fight's bars' numbers, drawn by code at the player's size and text size.
    let pixels_per_point = settings.scale();
    let metrics = UiMetrics::of_pixels(size, pixels_per_point, Insets::default());
    let layout = HudLayout::designed(metrics, HudDesign::shipped());
    for (bars, digits) in [("player", layout.player), ("enemy", layout.enemy), ("boss", layout.boss)] {
        let pixels = digits.digit_height * settings.text_size.scale() * pixels_per_point * display;
        if pixels < least.0 {
            least = (pixels, format!("the HUD's {bars} numbers"));
        }
    }
    least
}

#[test]
#[allow(
    clippy::print_stdout,
    reason = "the sizes measured are the exit test's numbers, read with --nocapture"
)]
fn every_screens_smallest_text_is_legible_on_a_deck_and_everywhere_else() {
    let mut wrong = Vec::new();
    let mut said = Vec::new();
    for language in ["en", "ja"] {
        let default = PlayerSettings { language: language.to_owned(), ..PlayerSettings::default() };
        for (settings, sizes) in [(&default, "default"), (&smallest(language), "smallest")] {
            for (frame, size, display) in FRAMES {
                let (pixels, what) = smallest_text(settings, (size, display));
                said.push(format!("{language}, {sizes} sizes, {frame}: {pixels:.1} px, {what}"));
                let floor = if sizes == "default" && frame == "1280×800" { AIM } else { FLOOR };
                if pixels < floor {
                    wrong.push(format!("{language}, {sizes} sizes, {frame}: {what} is {pixels:.1} px"));
                }
            }
        }
    }
    println!("{}", said.join("\n"));
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Larger text is larger, and the default is as designed.
#[test]
fn the_text_size_reaches_every_screen_and_the_subtitles_theirs() {
    let at = |settings: PlayerSettings, size| {
        let mut menus = menus_with(settings);
        let mut sizes = Vec::new();
        each_screen(&mut menus, size, |name, menus, screen| {
            let tree = menus.tree(screen).unwrap();
            sizes.extend(tree.text_pixels().map(|(node, pixels)| (format!("{name}'s {node}"), pixels)));
        });
        sizes
    };
    let default = at(PlayerSettings::default(), (1280, 800));
    let larger = PlayerSettings {
        text_size: TextSize::Larger,
        subtitles: Subtitles { size: SubtitleSize::Large, ..Subtitles::default() },
        ..PlayerSettings::default()
    };
    assert!(default.iter().any(|(node, _)| node == "hud's subtitle-line"), "{default:?}");
    // The subtitles' size is on top of the text's. On a phone's frame the text's is "large" at most.
    for (size, text) in [((1280, 800), 1.5), ((844, 390), 1.25)] {
        let normal = if size == (1280, 800) { default.clone() } else { at(PlayerSettings::default(), size) };
        let larger = at(larger.clone(), size);
        assert_eq!(normal.len(), larger.len());
        for ((node, normal), (_, large)) in normal.iter().zip(&larger) {
            let times = if node.starts_with("hud's subtitle-") { text * 1.25 } else { text };
            assert!((large / normal - times).abs() < 1e-4, "{node} at {size:?}: {normal} to {large}");
        }
    }
}

#[test]
fn every_screen_fits_at_the_largest_sizes_at_every_aspect() {
    let mut wrong = Vec::new();
    for language in ["en", "ja"] {
        let mut menus = menus_with(largest(language));
        for size in SIZES {
            each_screen(&mut menus, size, |name, menus, screen| {
                let overflowing = menus.tree(screen).unwrap().overflowing();
                if !overflowing.is_empty() {
                    wrong.push(format!("{language}, {name} at {size:?}: {overflowing:?} doesn't fit"));
                }
            });
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The interface's largest size fits every screen at every aspect too, at the text's own size (M4 Z3c). On a
/// phone 375 or 390 points tall the pause's rows pushed its title off the top and the character screen's
/// column ran past its back and the dialogue's answers past the top, until each scrolled under what stays:
/// the pause's rows under its title, the character's stats with its skills, over the line that says what one
/// does and the way back, and the dialogue's answers over its line.
#[test]
fn every_screen_fits_at_the_largest_interface_size_at_every_aspect() {
    let mut wrong = Vec::new();
    for language in ["en", "ja"] {
        let settings = PlayerSettings {
            language: language.to_owned(),
            size: "large".to_owned(),
            ..PlayerSettings::default()
        };
        let mut menus = menus_with(settings);
        for size in SIZES {
            each_screen(&mut menus, size, |name, menus, screen| {
                let overflowing = menus.tree(screen).unwrap().overflowing();
                if !overflowing.is_empty() {
                    wrong.push(format!("{language}, {name} at {size:?}: {overflowing:?} doesn't fit"));
                }
            });
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Every line a subtitle shows, a cutscene's and a fight's voice lines' (M4 A2), stays on the screen at the
/// largest text and subtitles, at every aspect, in both languages (M4 Z3c): its band within the frame, and
/// nothing of the HUD out of its frame. At the default sizes, `menus.rs` holds a cutscene's to two lines as
/// well; at these, on a phone, the longest reaches a little above the frame's middle.
#[test]
fn every_subtitle_stays_on_the_screen_at_the_largest_sizes_at_every_aspect() {
    let mut lines = support::subtitles();
    let cues = sapphire_cues::CueTable::default_table();
    let spoken = cues.rows().iter().filter_map(|row| row.line.as_deref());
    lines.extend(spoken.map(|line| {
        let key = format!("{}{line}", game_combat::cues::CAPTION_PREFIX);
        (key, Some(game_ui::menus::PLAYER_SPEAKER.to_owned()))
    }));
    assert!(lines.iter().any(|(line, _)| line.starts_with("subtitle-perfect")), "{lines:?}");
    let mut wrong = Vec::new();
    for language in ["en", "ja"] {
        let settings = largest(language);
        let mut menus = menus_with(settings.clone());
        menus.open(MenuScreen::HUD);
        let mut list = UiDrawList::default();
        for size in SIZES {
            for (line, speaker) in &lines {
                let hud = menus.model_mut::<HudModel>(MenuScreen::HUD).expect("the HUD is up");
                hud.follow_subtitle(Some(line), speaker.as_deref());
                hud.follow_subtitle_look(&settings.subtitles);
                for frame in 1..=30u64 {
                    menus.update(Timestamp::from_nanos(1_000_000_000 + frame * 16_666_667));
                    list.reset();
                    menus.draw(size, &mut list);
                }
                let at = format!("{language}, {size:?}: {line}");
                let tree = menus.tree(MenuScreen::HUD).unwrap();
                let band = tree.screen_rect(tree.find("subtitle-band").unwrap());
                let (width, height) = (size.0 as f32, size.1 as f32);
                if band.min.x < 0.0 || band.max.x > width || band.max.y > height || band.min.y < 0.0 {
                    wrong.push(format!("{at}: the band's at {band:?} in {size:?}"));
                }
                let overflowing = tree.overflowing();
                if !overflowing.is_empty() {
                    wrong.push(format!("{at}: {overflowing:?} doesn't fit"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
