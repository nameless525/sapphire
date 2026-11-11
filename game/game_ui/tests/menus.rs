//! Exit test 4's walk (M3 U7): every menu, as the real documents and the real tables say, in both languages and
//! under pseudo-localization, at every aspect, reached through real input values: a pad's, a keyboard's and a
//! finger's, all through `MenuControls`. No GPU and no cooked content: the tables are cooked here from the
//! sources, and the fonts are read from where they're kept.

mod support;

use std::path::Path;

use game_ui::menus::{
    CharacterModel, DialogueChoice, DialogueModel, GameMenus, HudModel, MenuCommand, MenuModel, MenuScreen,
    MenuSetup, OPENED_BY_CODE, PlayerSettings, RowAction, ScreenModel,
};
use sapphire_assets::ui::UiModel;
use sapphire_gameplay::assist::TimingAssist;
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_platform::Scancode;
use sapphire_strings::Pattern;
use sapphire_ui::{Atlas, BossBar, FighterValues, HudValues};
use support::walk::{
    CHARACTER, CONTROLS, DISPLAY, Device, GRAPHICS, REMAP_KEYS, REMAP_PAD, SETTINGS, SOUND, SUBTITLES, Walk,
};
use support::{
    Screens, assets, content, document_in, kind_names, menus, screens_of, subtitles, table, testdata,
    with_help,
};

/// The aspects a screen ships at, in points: 4:3, 16:9, the Steam Deck's 16:10 at its 1280×800 (M4 S4) and
/// 21:9, and the smallest phone that runs the game.
const SIZES: [(u32, u32); 6] = [(1024, 768), (1280, 720), (1280, 800), (1680, 720), (844, 390), (667, 375)];

/// The settings' pages (M4 A1): the hub's row that opens each, the page, and each of its rows in the order a
/// walk steps them. The interface's size and the text's come round to where they started, so the rest walks
/// at them, and the graphics page's rows come round too (M4 T3).
const SETTINGS_PAGES: [(&str, MenuScreen, &[&str]); 5] = [
    (
        "display",
        DISPLAY,
        &["size", "text", "shake", "flashes", "camera", "timing", "size", "size", "text", "text"],
    ),
    (
        "graphics",
        GRAPHICS,
        &[
            "quality", "quality", "quality", "cap", "cap", "cap", "cap", "dynamic", "dynamic", "saver",
            "saver",
        ],
    ),
    ("sound", SOUND, &["volume", "music", "effects", "voice", "mono", "rumble", "mono"]),
    ("subtitles", SUBTITLES, &["shown", "size", "background", "speakers", "shown"]),
    (
        "controls",
        CONTROLS,
        &["auto-target", "hold-light", "hold-heavy", "hold-dodge", "hold-skill", "hold-lock-on"],
    ),
];

/// The settings' hub, each of its pages and each of their rows, and back out of each.
fn walk_the_settings(walk: &mut Walk, pseudo: bool) {
    walk.check(SETTINGS, pseudo);
    assert!(walk.menus.is_open(SETTINGS));
    for row in ["language", "language"] {
        walk.choose(SETTINGS, row);
        walk.check(SETTINGS, pseudo);
    }
    for (row, page, rows) in SETTINGS_PAGES {
        walk.choose(SETTINGS, row);
        assert!(walk.menus.is_open(page), "{row} opens its page, by {:?} at {:?}", walk.device, walk.size);
        walk.check(page, pseudo);
        for row in rows {
            walk.choose(page, row);
            walk.check(page, pseudo);
        }
        if page == CONTROLS {
            walk_the_remap_screens(walk, pseudo);
        }
        walk.back(page);
        assert!(walk.menus.tree(page).is_none(), "{page:?} went, by {:?} at {:?}", walk.device, walk.size);
        assert!(walk.menus.is_open(SETTINGS), "back to the hub");
    }
    walk.back(SETTINGS);
}

/// The remap screens (M4 A4): each device's tab, a remap's wait, a press another binding has and the choice it
/// asks for, and back to the controls page. Nothing remapped stays remapped.
fn walk_the_remap_screens(walk: &mut Walk, pseudo: bool) {
    let at = format!("by {:?} at {:?}", walk.device, walk.size);
    walk.choose(CONTROLS, "remap");
    assert!(walk.menus.is_open(REMAP_PAD), "the controls page opens the remap screens, {at}");
    walk.check(REMAP_PAD, pseudo);
    walk.choose(REMAP_PAD, "tab-keyboard");
    assert!(walk.menus.is_open(REMAP_KEYS), "a tab opens its screen, {at}");
    assert!(walk.menus.tree(REMAP_PAD).is_none(), "in place of the other, {at}");
    walk.check(REMAP_KEYS, pseudo);

    // The attack's key, remapped to the heavy attack's: the wait, then what to do about it.
    walk.choose(REMAP_KEYS, "light");
    assert!(
        walk.menus.is_remapping() && walk.menus.is_open(MenuScreen::REMAP_LISTEN),
        "{at}: {:?}",
        walk.wrong
    );
    walk.check(MenuScreen::REMAP_LISTEN, pseudo);
    walk.key(Scancode::K);
    walk.settle();
    assert!(walk.menus.is_open(MenuScreen::REMAP_TAKEN), "K is the heavy attack's, {at}");
    walk.check(MenuScreen::REMAP_TAKEN, pseudo);
    walk.choose(MenuScreen::REMAP_TAKEN, "keep");
    assert!(!walk.menus.is_remapping(), "{at}");
    assert!(walk.menus.tree(MenuScreen::REMAP_TAKEN).is_none(), "{at}");
    assert_eq!(walk.menus.settings().remapped, [], "kept as it was, {at}");

    walk.choose(REMAP_KEYS, "tab-gamepad");
    assert!(walk.menus.is_open(REMAP_PAD), "{at}");
    walk.back(REMAP_PAD);
    assert!(walk.menus.tree(REMAP_PAD).is_none(), "{at}");
    assert!(walk.menus.is_open(CONTROLS), "back to the controls page, {at}");
}

/// Every menu, by one device.
fn walk_every_menu(walk: &mut Walk, pseudo: bool) {
    walk.menus.open(MenuScreen::TITLE);
    walk.settle();
    walk.check(MenuScreen::TITLE, pseudo);
    if walk.device != Device::Finger {
        assert_eq!(walk.tree(MenuScreen::TITLE).focus_name(), Some("new"), "with nothing to continue");
    }

    // Title to settings, each of its rows, and back.
    walk.choose(MenuScreen::TITLE, "settings");
    walk_the_settings(walk, pseudo);
    assert!(
        walk.menus.tree(SETTINGS).is_none(),
        "its machine took it out, by {:?} at {:?}",
        walk.device,
        walk.size
    );

    // A new game: the title goes, and the fight's words stand behind it.
    walk.menus.open_under(MenuScreen::HUD);
    walk.choose(MenuScreen::TITLE, "new");
    assert!(walk.commands.contains(&MenuCommand::NewGame), "{:?}", walk.commands);
    assert!(walk.menus.tree(MenuScreen::TITLE).is_none());
    assert!(!walk.menus.is_modal());
    walk.check(MenuScreen::HUD, pseudo);

    // The pause, and all it leads to.
    walk.menus.open(MenuScreen::PAUSE);
    walk.settle();
    walk.check(MenuScreen::PAUSE, pseudo);
    walk.choose(MenuScreen::PAUSE, "character");
    walk.check(CHARACTER, pseudo);
    for skill in ["skill-dodge", "skill-art"] {
        if walk.device != Device::Finger {
            walk.focus(CHARACTER, skill);
            let detail = format!("character-detail-{}", skill.strip_prefix("skill-").unwrap());
            let shown = walk.menus.model_mut::<CharacterModel>(CHARACTER).unwrap().detail.clone();
            assert_eq!(shown, detail, "the line follows focus");
            walk.settle();
            walk.check(CHARACTER, pseudo);
        }
    }
    walk.back(CHARACTER);
    walk.choose(MenuScreen::PAUSE, "settings");
    walk.back(SETTINGS);
    assert!(walk.menus.is_open(MenuScreen::PAUSE), "and the pause is still there");
    walk.choose(MenuScreen::PAUSE, "resume");
    assert!(walk.commands.contains(&MenuCommand::Resume));
    assert!(walk.menus.tree(MenuScreen::PAUSE).is_none());

    // And once more, back to the title.
    walk.menus.open(MenuScreen::PAUSE);
    walk.settle();
    walk.choose(MenuScreen::PAUSE, "to-title");
    assert!(walk.commands.contains(&MenuCommand::ToTitle), "{:?}", walk.commands);
    assert!(walk.menus.tree(MenuScreen::PAUSE).is_none());

    // The dialogue: answers, then a line to go on from.
    walk.menus.open(MenuScreen::DIALOGUE);
    walk.settle();
    walk.check(MenuScreen::DIALOGUE, pseudo);
    walk.choose(MenuScreen::DIALOGUE, "fourth");
    assert!(walk.commands.contains(&MenuCommand::DialogueChose(3)), "{:?}", walk.commands);
    let model = walk.menus.model_mut::<DialogueModel>(MenuScreen::DIALOGUE).unwrap();
    model.next = true;
    for choice in [&mut model.first, &mut model.second, &mut model.third, &mut model.fourth] {
        *choice = DialogueChoice::default();
    }
    walk.settle();
    walk.check(MenuScreen::DIALOGUE, pseudo);
    walk.choose(MenuScreen::DIALOGUE, "next");
    assert!(walk.commands.contains(&MenuCommand::DialogueNext), "{:?}", walk.commands);
    walk.menus.close(MenuScreen::DIALOGUE);
    walk.settle();
    assert!(walk.menus.tree(MenuScreen::DIALOGUE).is_none());
    assert!(!walk.menus.is_modal());
}

#[test]
fn every_menu_walks_fits_and_comes_from_the_tables_by_every_device_at_every_aspect() {
    let mut wrong = Vec::new();
    for (locale, pseudo) in [("en", false), ("ja", false), ("en", true)] {
        for size in SIZES {
            for device in [Device::Pad, Device::Keyboard, Device::Finger] {
                let mut walk = Walk::new(assets(), locale, pseudo, size, device);
                walk_every_menu(&mut walk, pseudo);
                wrong.extend(walk.wrong.into_iter().map(|line| format!("{locale}, pseudo {pseudo}: {line}")));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_settings_show_what_they_are_and_the_language_moves_every_screen() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(MenuScreen::TITLE);
    walk.settle();
    walk.choose(MenuScreen::TITLE, "settings");
    let string_of = |walk: &Walk, screen, node: &str| {
        walk.tree(screen).strings().find(|(name, _)| *name == node).unwrap().1.to_owned()
    };
    assert_eq!(string_of(&walk, MenuScreen::TITLE, "new-text"), "New game");
    assert_eq!(string_of(&walk, SETTINGS, "language-value"), "English");
    assert_eq!(walk.menus.window_title(), "Sapphire");
    walk.choose(SETTINGS, "language");
    assert_eq!(walk.menus.window_title(), "サファイア", "the window's title follows the language");
    assert_eq!(walk.menus.settings().language, "ja");
    let locale = walk.menus.settings().locale();
    assert_eq!((locale.text.as_str(), locale.voice.as_str()), ("ja", "ja"), "and the voices with it");
    assert_eq!(string_of(&walk, SETTINGS, "language-value"), "日本語");
    assert_eq!(string_of(&walk, MenuScreen::TITLE, "new-text"), "はじめから", "under it too");
    assert!(walk.commands.contains(&MenuCommand::SettingsChanged));

    assert_eq!(string_of(&walk, SETTINGS, "sound-value"), "›", "a page's row says it goes on");
    walk.choose(SETTINGS, "language");
    assert_eq!(walk.menus.settings().language, "en");

    walk.choose(SETTINGS, "sound");
    assert!(string_of(&walk, SOUND, "volume-value").starts_with("100"));
    walk.choose(SOUND, "volume");
    assert_eq!(walk.menus.settings().sound, 0, "past the top it comes round");
    assert!(string_of(&walk, SOUND, "volume-value").starts_with('0'));
    walk.choose(SOUND, "volume");
    assert_eq!(walk.menus.settings().sound, 10);
    walk.choose(SOUND, "music");
    assert_eq!((walk.menus.settings().music, walk.menus.settings().sound), (0, 10), "each its own");
    assert_eq!(string_of(&walk, SOUND, "mono-value"), "Off");
    walk.choose(SOUND, "mono");
    assert!(walk.menus.settings().mono);
    assert_eq!(string_of(&walk, SOUND, "mono-value"), "On");
    walk.back(SOUND);

    walk.choose(SETTINGS, "display");
    walk.choose(DISPLAY, "size");
    assert_eq!(walk.menus.settings().size, "large");
    assert!(walk.menus.settings().scale() > 1.0);
    walk.choose(DISPLAY, "flashes");
    assert_eq!(string_of(&walk, DISPLAY, "flashes-value"), "Reduced");
    walk.choose(DISPLAY, "shake");
    assert_eq!(walk.menus.settings().shake, 0);
    assert!(string_of(&walk, DISPLAY, "shake-value").starts_with('0'));
    assert_eq!(string_of(&walk, DISPLAY, "timing-value"), "Off");
    walk.choose(DISPLAY, "timing");
    assert_eq!(walk.menus.settings().timing_assist, TimingAssist::Wide);
    assert_eq!(string_of(&walk, DISPLAY, "timing-value"), "Wide");
    walk.back(DISPLAY);

    walk.choose(SETTINGS, "subtitles");
    walk.choose(SUBTITLES, "background");
    assert_eq!(string_of(&walk, SUBTITLES, "background-value"), "Solid");
    walk.choose(SUBTITLES, "speakers");
    assert!(!walk.menus.settings().subtitles.speakers);
    walk.back(SUBTITLES);

    walk.choose(SETTINGS, "controls");
    assert_eq!(string_of(&walk, CONTROLS, "auto-target-value"), "Normal");
    walk.choose(CONTROLS, "auto-target");
    assert_eq!(string_of(&walk, CONTROLS, "auto-target-value"), "Strong");
    walk.choose(CONTROLS, "hold-dodge");
    assert_eq!(string_of(&walk, CONTROLS, "hold-dodge-value"), "Toggle");
    assert_eq!(string_of(&walk, CONTROLS, "hold-light-value"), "Hold", "only the one chosen");
}

/// Each kind of enemy's foe is named by the key its kind holds, in both languages (M4 X2): a boss's on its
/// bar, in the enemy's name's place, and any other's as the enemy's name, so no kind shows another's. A foe
/// with no name shows none.
#[test]
fn each_kind_of_enemy_is_named_by_its_own_key() {
    let kinds = kind_names();
    assert!(kinds.values().any(|&boss| boss) && kinds.values().any(|&boss| !boss), "bosses and others");
    for (key, &boss) in &kinds {
        let values = HudValues {
            enemy: Some(FighterValues::default()),
            boss: boss.then(BossBar::default),
            ..HudValues::default()
        };
        let (named, unnamed) = if boss { ("boss-name", "enemy-name") } else { ("enemy-name", "boss-name") };
        for locale in ["en", "ja"] {
            let mut walk = Walk::new(assets(), locale, false, (1280, 720), Device::Keyboard);
            walk.menus.open(MenuScreen::HUD);
            walk.menus
                .model_mut::<HudModel>(MenuScreen::HUD)
                .unwrap()
                .follow_fight(Some(&values), Some(key.as_str()));
            walk.settle();
            walk.check(MenuScreen::HUD, false);
            let table = table(locale);
            let Pattern::Plain(span) = table.entry(key).expect("in the table").pattern else {
                panic!("{key} is a plain name")
            };
            let tree = walk.tree(MenuScreen::HUD);
            let shown = |name| tree.is_visible(tree.find(name).expect("a node"));
            let said = |name| tree.strings().find(|(node, _)| *node == name).expect("text").1;
            assert!(shown(named) && !shown(unnamed), "{locale}: {key}");
            assert_eq!(said(named), table.str(span), "{locale}: {key}");

            // The same foe with no name to give shows none.
            walk.menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap().follow_fight(Some(&values), None);
            walk.settle();
            let tree = walk.tree(MenuScreen::HUD);
            let shown = |name| tree.is_visible(tree.find(name).expect("a node"));
            assert!(shown("enemy") && !shown("boss-name") && !shown("enemy-name"), "{locale}: {key}");
        }
    }
}

/// A cutscene's every line shows on the HUD with its speaker, as the table says it, in both languages and under
/// pseudo-localization, at every aspect (M3 Q4): low on the screen, clear of its edges, in at most two lines,
/// and gone when nothing's said.
#[test]
fn a_cutscenes_subtitles_show_in_every_language_at_every_aspect() {
    /// The subtitle style's line, its size by its line height, in points.
    const LINE: f32 = 20.0 * 1.4;
    let lines = subtitles();
    assert!(!lines.is_empty(), "no cutscene says anything");
    let mut wrong = Vec::new();
    for (locale, pseudo) in [("en", false), ("ja", false), ("en", true)] {
        let table = table(locale);
        for size in SIZES {
            let mut walk = Walk::new(assets(), locale, pseudo, size, Device::Keyboard);
            walk.menus.open(MenuScreen::HUD);
            for (line, speaker) in &lines {
                let at = format!("{locale}, pseudo {pseudo}, {size:?}: {line}");
                let model = walk.menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap();
                model.follow_subtitle(Some(line), speaker.as_deref());
                walk.settle();
                walk.check(MenuScreen::HUD, pseudo);
                let tree = walk.tree(MenuScreen::HUD);
                let node = |name| tree.find(name).expect("a node");
                let said = |name| tree.strings().find(|(node, _)| *node == name).expect("text").1;
                assert!(tree.is_visible(node("subtitle-line")), "{at}");
                assert_eq!(tree.is_visible(node("subtitle-speaker")), speaker.is_some(), "{at}");
                if !pseudo {
                    for (name, key) in [("subtitle-line", Some(line)), ("subtitle-speaker", speaker.as_ref())]
                    {
                        let Some(key) = key else { continue };
                        let Pattern::Plain(span) = table.entry(key).expect("in the table").pattern else {
                            panic!("{key} is plain text")
                        };
                        assert_eq!(said(name), table.str(span), "{at}");
                    }
                }
                let band = tree.screen_rect(node("subtitle-band"));
                let text = tree.screen_rect(node("subtitle-line"));
                let (width, height) = (size.0 as f32, size.1 as f32);
                if band.min.x < 0.0 || band.max.x > width || band.max.y > height || band.min.y < height * 0.5
                {
                    wrong.push(format!("{at}: the band's at {band:?}"));
                }
                if text.height() > 2.0 * LINE + 1.0 {
                    wrong.push(format!("{at}: {} points tall, over two lines", text.height()));
                }
            }
            let model = walk.menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap();
            model.follow_subtitle(None, Some("speaker-wanderer"));
            walk.settle();
            let tree = walk.tree(MenuScreen::HUD);
            assert!(!tree.is_visible(tree.find("subtitle").unwrap()), "{locale}: nothing said shows nothing");
            wrong.extend(walk.wrong);
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A cutscene has the screen (M3 Q4): the fight's names and objective fade out with `fight_words`, and its
/// subtitle and its way out stay.
#[test]
fn a_cutscene_hides_the_fights_words_and_keeps_its_own() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(MenuScreen::HUD);
    let fight = ["player", "enemy", "objective"];
    for (words, cutscene) in [(1.0, false), (0.0, true)] {
        let model = walk.menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap();
        model.fight_words = words;
        model.can_skip = cutscene;
        model.follow_subtitle(cutscene.then_some("subtitle-court-gate-warning"), Some("speaker-warden"));
        walk.settle();
        walk.check(MenuScreen::HUD, false);
        let tree = walk.tree(MenuScreen::HUD);
        for name in fight {
            assert_eq!(tree.opacity(tree.find(name).unwrap()), words, "{name}");
        }
        for name in ["skip", "subtitle"] {
            assert_eq!(tree.is_visible(tree.find(name).unwrap()), cutscene, "{name}");
        }
    }
    assert!(walk.wrong.is_empty(), "{}", walk.wrong.join("\n"));
}

#[test]
fn a_press_goes_to_the_top_screen_only_and_the_hud_takes_only_its_pause() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(MenuScreen::HUD);
    walk.settle();
    assert!(!walk.menus.is_modal());
    let model = walk.menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap();
    model.touch = true;
    walk.settle();

    // A key's nothing to it: that's the fight's.
    walk.key(Scancode::RETURN);
    walk.settle();
    assert!(walk.commands.is_empty(), "{:?}", walk.commands);

    let (pause, size) = {
        let tree = walk.tree(MenuScreen::HUD);
        (tree.screen_rect(tree.find("pause").unwrap()).center(), tree.size())
    };
    assert!(walk.menus.hud_takes(pause / size));
    assert!(!walk.menus.hud_takes(Vec2::new(0.25, 0.5)), "the rest of the frame is the fight's");
    walk.tap(pause);
    assert_eq!(walk.commands, [MenuCommand::Pause]);

    walk.commands.clear();
    walk.menus.open(MenuScreen::PAUSE);
    walk.settle();
    assert!(walk.menus.is_modal());
    // Under the pause, its button doesn't answer.
    walk.tap(pause);
    walk.settle();
    assert!(!walk.commands.contains(&MenuCommand::Pause), "{:?}", walk.commands);
}

/// A cutscene's skip shows only while one can be skipped, and a tap on it asks for the skip; the banner shows
/// what it's given, from the tables.
#[test]
fn the_huds_skip_and_banner_show_when_asked() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Finger);
    walk.menus.open(MenuScreen::HUD);
    walk.settle();
    let skip = walk.tree(MenuScreen::HUD).find("skip").unwrap();
    let banner = walk.tree(MenuScreen::HUD).find("banner").unwrap();
    assert!(!walk.tree(MenuScreen::HUD).is_visible(skip) && !walk.tree(MenuScreen::HUD).is_visible(banner));

    let model = walk.menus.model_mut::<HudModel>(MenuScreen::HUD).unwrap();
    model.can_skip = true;
    model.has_banner = true;
    model.banner = "hud-banner-victory".to_owned();
    walk.settle();
    let tree = walk.tree(MenuScreen::HUD);
    assert!(tree.is_visible(skip) && tree.is_visible(banner));
    assert!(tree.problems().is_empty(), "{:?}", tree.problems());
    let at = tree.screen_rect(skip).center();
    walk.tap(at);
    assert_eq!(walk.commands, [MenuCommand::Skip]);
}

#[test]
fn back_means_what_each_screen_says() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(MenuScreen::TITLE);
    walk.settle();
    walk.key(Scancode::ESCAPE);
    walk.settle();
    assert_eq!(walk.commands, [MenuCommand::Leave], "a title has nowhere else to go");
    assert!(walk.menus.is_open(MenuScreen::TITLE), "and it stays until whoever holds it says");
    walk.commands.clear();
    walk.menus.close(MenuScreen::TITLE);
    walk.settle();

    walk.menus.open(MenuScreen::PAUSE);
    walk.settle();
    walk.key(Scancode::ESCAPE);
    walk.settle();
    assert_eq!(walk.commands, [MenuCommand::Resume]);
    assert!(walk.menus.tree(MenuScreen::PAUSE).is_none());
}

#[test]
fn a_platform_with_no_way_out_offers_none() {
    let settings = PlayerSettings::default();
    let Screens { assets, menus: screens } = assets();
    let setup = MenuSetup {
        screens,
        settings,
        bindings: sapphire_controls::default_bindings().unwrap(),
        can_leave: false,
        can_mark: false,
        pseudo: false,
    };
    let menus = GameMenus::new(assets, Atlas::paint(), setup).unwrap();
    let mut walk = Walk {
        menus,
        size: (844, 390),
        frame: 0,
        list: UiDrawList::default(),
        device: Device::Keyboard,
        commands: Vec::new(),
        wrong: Vec::new(),
    };
    walk.menus.open(MenuScreen::TITLE);
    walk.settle();
    let tree = walk.tree(MenuScreen::TITLE);
    assert!(!tree.is_visible(tree.find("leave").unwrap()));
    assert_eq!(tree.unreachable(), Vec::<&str>::new());
}

/// What the documents say and what the code does agree: each screen's document is `<name>.ui.ron` with its
/// sidecar's ID, and reads its kind of model, whose document is what its type has; each node that takes
/// focus does something; and each screen comes in and goes out.
#[test]
fn the_documents_and_the_code_agree() {
    let ui = content().join("ui");
    for kind in [ScreenModel::Menu, ScreenModel::Character, ScreenModel::Dialogue, ScreenModel::Hud] {
        let model = UiModel::from_document(&std::fs::read_to_string(ui.join(kind.file())).unwrap()).unwrap();
        assert_eq!(model, UiModel::of_type(kind.type_info()), "{kind:?}");
    }
    let menus = menus();
    menus.require(&OPENED_BY_CODE).expect("the screens the code opens");
    for screen in &menus.screens {
        let name = &screen.name;
        let sidecar = std::fs::read_to_string(ui.join(format!("{name}.ui.ron.meta"))).unwrap();
        assert!(
            sidecar.contains(&screen.document.to_string()),
            "{name}: {} isn't in {sidecar}",
            screen.document
        );
        let document = document_in(name, std::slice::from_ref(&ui));
        assert_eq!(document.model.as_deref(), Some(screen.model.file()), "{name} reads its kind's model");
        for node in document.nodes.iter().filter(|node| node.focus.is_some()) {
            let action = screen.action(&node.name);
            assert!(action.is_some(), "nothing's written for {name}'s {}", node.name);
            let fires = node.focus.as_ref().and_then(|focus| focus.fires.as_deref());
            if action == Some(&RowAction::Nothing) && !node.name.starts_with("skill-") {
                assert!(fires.is_some(), "{name}'s {} does nothing at all", node.name);
            }
        }
        for row in &screen.rows {
            assert!(document.nodes.iter().any(|node| node.name == row.node), "{name} has no {}", row.node);
        }
    }
}

/// M3's pillar 1 for screens, as exit test 2 has it for a boss: a screen added with documents alone (its
/// `.ui.ron` and a row in the menus document, here a fixture's) opens from the pause, shows the menus' own
/// values, takes its rows' actions and goes back, all by the game's own menus; and no Rust names a screen
/// but those the code opens itself. `strings.rs` holds its keys.
#[test]
fn a_new_screen_is_documents() {
    let help = MenuScreen::named("help");
    let dirs = [testdata(), content().join("ui")];
    for device in [Device::Pad, Device::Keyboard, Device::Finger] {
        let mut walk = Walk::new(screens_of(with_help(), &dirs), "ja", false, (1280, 720), device);
        walk.menus.open(MenuScreen::PAUSE);
        walk.settle();
        walk.choose(MenuScreen::PAUSE, "settings");
        assert!(walk.menus.is_open(help), "the pause's row opens it, by {device:?}");
        walk.check(help, false);
        let tree = walk.tree(help);
        let shown = tree.strings().find(|(name, _)| *name == "language").expect("the language").1;
        assert_eq!(shown, "日本語", "it shows what the menus' model holds");
        assert!(walk.menus.model_mut::<MenuModel>(help).is_some_and(|model| model.can_leave));

        walk.choose(help, "settings");
        assert!(walk.menus.is_open(SETTINGS), "its row opens another, by {device:?}");
        walk.back(SETTINGS);
        assert!(walk.menus.is_open(help) && !walk.menus.is_open(SETTINGS));
        walk.back(help);
        assert!(!walk.menus.is_open(help), "its machine takes it out, by {device:?}");
        assert!(walk.menus.is_open(MenuScreen::PAUSE));
        assert!(walk.wrong.is_empty(), "{:?}", walk.wrong);
    }

    // A screen's back says what it does: this one's resumes, as the pause's does.
    let mut walk = Walk::new(screens_of(with_help(), &dirs), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(help);
    walk.settle();
    walk.key(Scancode::ESCAPE);
    walk.settle();
    assert_eq!(walk.commands, [MenuCommand::Resume]);

    // No Rust names a screen but those the code opens: every other is the documents'.
    let root = content().join("..");
    let mut named = Vec::new();
    for dir in ["game", "apps", "tools", "crates"] {
        rust_naming_screens(&root.join(dir), &mut named);
    }
    assert!(named.is_empty(), "Rust names a screen by name outside its tests: {named:?}");
}

/// Every `.rs` file under `dir`, past its tests, that names a screen by `MenuScreen::named`, which only
/// `game_ui`'s own [`MenuScreen`] consts may: the screens in [`OPENED_BY_CODE`].
fn rust_naming_screens(dir: &Path, into: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | "tests" | "testdata") {
                rust_naming_screens(&path, into);
            }
        } else if name.ends_with(".rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            // A file's unit tests are its own: what ships is what comes before them.
            let shipped = text.split("#[cfg(test)]").next().unwrap_or_default();

            let names = shipped.matches("MenuScreen::named(").count();

            if names > 0 {
                into.push(path.display().to_string());
            }
        }
    }
}
