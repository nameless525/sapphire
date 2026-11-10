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
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_platform::Scancode;
use sapphire_strings::Pattern;
use sapphire_ui::{Atlas, BossBar, FighterValues, HudValues};
use support::walk::{CHARACTER, Device, SETTINGS, Walk};
use support::{
    Screens, assets, boss_names, content, document_in, menus, screens_of, table, testdata, with_help,
};

/// The aspects a screen ships at, in points: 4:3, 16:9 and 21:9, and the smallest phone that runs the game.
const SIZES: [(u32, u32); 5] = [(1024, 768), (1280, 720), (1680, 720), (844, 390), (667, 375)];

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
    walk.check(SETTINGS, pseudo);
    assert!(walk.menus.is_open(SETTINGS));
    for row in ["language", "language", "size", "sound", "rumble", "rumble", "size", "size"] {
        walk.choose(SETTINGS, row);
        walk.check(SETTINGS, pseudo);
    }
    walk.back(SETTINGS);
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

    walk.choose(SETTINGS, "sound");
    assert_eq!(walk.menus.settings().sound, 0, "past the top it comes round");
    assert!(string_of(&walk, SETTINGS, "sound-value").starts_with('0'));
    walk.choose(SETTINGS, "sound");
    assert_eq!(walk.menus.settings().sound, 10);

    walk.choose(SETTINGS, "size");
    assert_eq!(walk.menus.settings().size, "large");
    assert!(walk.menus.settings().scale() > 1.0);
}

/// A boss's bar is named by the key its kind of enemy holds, in the enemy's name's place, in both languages;
/// another foe's is named as before.
#[test]
fn a_bosss_bar_is_named_by_its_kind_of_enemy() {
    let mut values = HudValues {
        enemy: Some(FighterValues::default()),
        boss: Some(BossBar::default()),
        ..HudValues::default()
    };
    for key in boss_names() {
        for locale in ["en", "ja"] {
            let mut walk = Walk::new(assets(), locale, false, (1280, 720), Device::Keyboard);
            walk.menus.open(MenuScreen::HUD);
            values.boss = Some(BossBar::default());
            walk.menus
                .model_mut::<HudModel>(MenuScreen::HUD)
                .unwrap()
                .follow_fight(Some(&values), Some(&key));
            walk.settle();
            walk.check(MenuScreen::HUD, false);
            let table = table(locale);
            let Pattern::Plain(span) = table.entry(&key).expect("in the table").pattern else {
                panic!("{key} is a plain name")
            };
            let tree = walk.tree(MenuScreen::HUD);
            let shown = |name| tree.is_visible(tree.find(name).expect("a node"));
            let said = |name| tree.strings().find(|(node, _)| *node == name).expect("text").1;
            assert!(shown("boss-name") && !shown("enemy-name"), "{locale}: {key}");
            assert_eq!(said("boss-name"), table.str(span), "{locale}: {key}");

            // The same foe with a plain bar is named as any other.
            values.boss = None;
            walk.menus
                .model_mut::<HudModel>(MenuScreen::HUD)
                .unwrap()
                .follow_fight(Some(&values), Some(&key));
            walk.settle();
            let tree = walk.tree(MenuScreen::HUD);
            let shown = |name| tree.is_visible(tree.find(name).expect("a node"));
            assert!(!shown("boss-name") && shown("enemy-name"), "{locale}: {key}");
        }
    }
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
    let setup = MenuSetup { screens, settings, can_leave: false, pseudo: false };
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
