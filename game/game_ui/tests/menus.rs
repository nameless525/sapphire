//! Exit test 4's walk (M3 U7): every menu, as the real documents and the real tables say, in both languages and
//! under pseudo-localization, at every aspect, reached through real input values: a pad's, a keyboard's and a
//! finger's, all through `MenuControls`. No GPU and no cooked content: the tables are cooked here from the
//! sources, and the fonts are read from where they're kept.

mod support;

use bevy_reflect::Typed;
use game_ui::menus::{
    CharacterModel, DialogueChoice, DialogueModel, GameMenus, HudModel, MenuAction, MenuCommand, MenuScreen,
    MenuSetup, PauseModel, PlayerSettings, SettingsModel, TitleModel, action_of,
};
use sapphire_assets::ui::UiModel;
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_platform::Scancode;
use sapphire_strings::Pattern;
use sapphire_ui::{Atlas, BossBar, FighterValues, HudValues};
use support::walk::{Device, Walk};
use support::{assets, boss_names, content, document, table};

/// The aspects a screen ships at, in points: 4:3, 16:9 and 21:9, and the smallest phone that runs the game.
const SIZES: [(u32, u32); 5] = [(1024, 768), (1280, 720), (1680, 720), (844, 390), (667, 375)];

/// Every menu, by one device.
fn walk_every_menu(walk: &mut Walk, pseudo: bool) {
    walk.menus.open(MenuScreen::Title);
    walk.settle();
    walk.check(MenuScreen::Title, pseudo);
    if walk.device != Device::Finger {
        assert_eq!(walk.tree(MenuScreen::Title).focus_name(), Some("new"), "with nothing to continue");
    }

    // Title to settings, each of its rows, and back.
    walk.choose(MenuScreen::Title, "settings");
    walk.check(MenuScreen::Settings, pseudo);
    assert!(walk.menus.is_open(MenuScreen::Settings));
    for row in ["language", "language", "size", "sound", "rumble", "rumble", "size", "size"] {
        walk.choose(MenuScreen::Settings, row);
        walk.check(MenuScreen::Settings, pseudo);
    }
    walk.back(MenuScreen::Settings);
    assert!(
        walk.menus.tree(MenuScreen::Settings).is_none(),
        "its machine took it out, by {:?} at {:?}",
        walk.device,
        walk.size
    );

    // A new game: the title goes, and the fight's words stand behind it.
    walk.menus.open_under(MenuScreen::Hud);
    walk.choose(MenuScreen::Title, "new");
    assert!(walk.commands.contains(&MenuCommand::NewGame), "{:?}", walk.commands);
    assert!(walk.menus.tree(MenuScreen::Title).is_none());
    assert!(!walk.menus.is_modal());
    walk.check(MenuScreen::Hud, pseudo);

    // The pause, and all it leads to.
    walk.menus.open(MenuScreen::Pause);
    walk.settle();
    walk.check(MenuScreen::Pause, pseudo);
    walk.choose(MenuScreen::Pause, "character");
    walk.check(MenuScreen::Character, pseudo);
    for skill in ["skill-dodge", "skill-art"] {
        if walk.device != Device::Finger {
            walk.focus(MenuScreen::Character, skill);
            let detail = format!("character-detail-{}", skill.strip_prefix("skill-").unwrap());
            let shown = walk.menus.model_mut::<CharacterModel>(MenuScreen::Character).unwrap().detail.clone();
            assert_eq!(shown, detail, "the line follows focus");
            walk.settle();
            walk.check(MenuScreen::Character, pseudo);
        }
    }
    walk.back(MenuScreen::Character);
    walk.choose(MenuScreen::Pause, "settings");
    walk.back(MenuScreen::Settings);
    assert!(walk.menus.is_open(MenuScreen::Pause), "and the pause is still there");
    walk.choose(MenuScreen::Pause, "resume");
    assert!(walk.commands.contains(&MenuCommand::Resume));
    assert!(walk.menus.tree(MenuScreen::Pause).is_none());

    // And once more, back to the title.
    walk.menus.open(MenuScreen::Pause);
    walk.settle();
    walk.choose(MenuScreen::Pause, "to-title");
    assert!(walk.commands.contains(&MenuCommand::ToTitle), "{:?}", walk.commands);
    assert!(walk.menus.tree(MenuScreen::Pause).is_none());

    // The dialogue: answers, then a line to go on from.
    walk.menus.open(MenuScreen::Dialogue);
    walk.settle();
    walk.check(MenuScreen::Dialogue, pseudo);
    walk.choose(MenuScreen::Dialogue, "fourth");
    assert!(walk.commands.contains(&MenuCommand::DialogueChose(3)), "{:?}", walk.commands);
    let model = walk.menus.model_mut::<DialogueModel>(MenuScreen::Dialogue).unwrap();
    model.next = true;
    for choice in [&mut model.first, &mut model.second, &mut model.third, &mut model.fourth] {
        *choice = DialogueChoice::default();
    }
    walk.settle();
    walk.check(MenuScreen::Dialogue, pseudo);
    walk.choose(MenuScreen::Dialogue, "next");
    assert!(walk.commands.contains(&MenuCommand::DialogueNext), "{:?}", walk.commands);
    walk.menus.close(MenuScreen::Dialogue);
    walk.settle();
    assert!(walk.menus.tree(MenuScreen::Dialogue).is_none());
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
    walk.menus.open(MenuScreen::Title);
    walk.settle();
    walk.choose(MenuScreen::Title, "settings");
    let string_of = |walk: &Walk, screen, node: &str| {
        walk.tree(screen).strings().find(|(name, _)| *name == node).unwrap().1.to_owned()
    };
    assert_eq!(string_of(&walk, MenuScreen::Title, "new-text"), "New game");
    assert_eq!(string_of(&walk, MenuScreen::Settings, "language-value"), "English");
    assert_eq!(walk.menus.window_title(), "Sapphire");
    walk.choose(MenuScreen::Settings, "language");
    assert_eq!(walk.menus.window_title(), "サファイア", "the window's title follows the language");
    assert_eq!(walk.menus.settings().language, "ja");
    let locale = walk.menus.settings().locale();
    assert_eq!((locale.text.as_str(), locale.voice.as_str()), ("ja", "ja"), "and the voices with it");
    assert_eq!(string_of(&walk, MenuScreen::Settings, "language-value"), "日本語");
    assert_eq!(string_of(&walk, MenuScreen::Title, "new-text"), "はじめから", "under it too");
    assert!(walk.commands.contains(&MenuCommand::SettingsChanged));

    walk.choose(MenuScreen::Settings, "sound");
    assert_eq!(walk.menus.settings().sound, 0, "past the top it comes round");
    assert!(string_of(&walk, MenuScreen::Settings, "sound-value").starts_with('0'));
    walk.choose(MenuScreen::Settings, "sound");
    assert_eq!(walk.menus.settings().sound, 10);

    walk.choose(MenuScreen::Settings, "size");
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
            walk.menus.open(MenuScreen::Hud);
            values.boss = Some(BossBar::default());
            walk.menus
                .model_mut::<HudModel>(MenuScreen::Hud)
                .unwrap()
                .follow_fight(Some(&values), Some(&key));
            walk.settle();
            walk.check(MenuScreen::Hud, false);
            let table = table(locale);
            let Pattern::Plain(span) = table.entry(&key).expect("in the table").pattern else {
                panic!("{key} is a plain name")
            };
            let tree = walk.tree(MenuScreen::Hud);
            let shown = |name| tree.is_visible(tree.find(name).expect("a node"));
            let said = |name| tree.strings().find(|(node, _)| *node == name).expect("text").1;
            assert!(shown("boss-name") && !shown("enemy-name"), "{locale}: {key}");
            assert_eq!(said("boss-name"), table.str(span), "{locale}: {key}");

            // The same foe with a plain bar is named as any other.
            values.boss = None;
            walk.menus
                .model_mut::<HudModel>(MenuScreen::Hud)
                .unwrap()
                .follow_fight(Some(&values), Some(&key));
            walk.settle();
            let tree = walk.tree(MenuScreen::Hud);
            let shown = |name| tree.is_visible(tree.find(name).expect("a node"));
            assert!(!shown("boss-name") && shown("enemy-name"), "{locale}: {key}");
        }
    }
}

#[test]
fn a_press_goes_to_the_top_screen_only_and_the_hud_takes_only_its_pause() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(MenuScreen::Hud);
    walk.settle();
    assert!(!walk.menus.is_modal());
    let model = walk.menus.model_mut::<HudModel>(MenuScreen::Hud).unwrap();
    model.touch = true;
    walk.settle();

    // A key's nothing to it: that's the fight's.
    walk.key(Scancode::RETURN);
    walk.settle();
    assert!(walk.commands.is_empty(), "{:?}", walk.commands);

    let (pause, size) = {
        let tree = walk.tree(MenuScreen::Hud);
        (tree.screen_rect(tree.find("pause").unwrap()).center(), tree.size())
    };
    assert!(walk.menus.hud_takes(pause / size));
    assert!(!walk.menus.hud_takes(Vec2::new(0.25, 0.5)), "the rest of the frame is the fight's");
    walk.tap(pause);
    assert_eq!(walk.commands, [MenuCommand::Pause]);

    walk.commands.clear();
    walk.menus.open(MenuScreen::Pause);
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
    walk.menus.open(MenuScreen::Hud);
    walk.settle();
    let skip = walk.tree(MenuScreen::Hud).find("skip").unwrap();
    let banner = walk.tree(MenuScreen::Hud).find("banner").unwrap();
    assert!(!walk.tree(MenuScreen::Hud).is_visible(skip) && !walk.tree(MenuScreen::Hud).is_visible(banner));

    let model = walk.menus.model_mut::<HudModel>(MenuScreen::Hud).unwrap();
    model.can_skip = true;
    model.has_banner = true;
    model.banner = "hud-banner-victory".to_owned();
    walk.settle();
    let tree = walk.tree(MenuScreen::Hud);
    assert!(tree.is_visible(skip) && tree.is_visible(banner));
    assert!(tree.problems().is_empty(), "{:?}", tree.problems());
    let at = tree.screen_rect(skip).center();
    walk.tap(at);
    assert_eq!(walk.commands, [MenuCommand::Skip]);
}

#[test]
fn back_means_what_each_screen_says() {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), Device::Keyboard);
    walk.menus.open(MenuScreen::Title);
    walk.settle();
    walk.key(Scancode::ESCAPE);
    walk.settle();
    assert_eq!(walk.commands, [MenuCommand::Leave], "a title has nowhere else to go");
    assert!(walk.menus.is_open(MenuScreen::Title), "and it stays until whoever holds it says");
    walk.commands.clear();
    walk.menus.close(MenuScreen::Title);
    walk.settle();

    walk.menus.open(MenuScreen::Pause);
    walk.settle();
    walk.key(Scancode::ESCAPE);
    walk.settle();
    assert_eq!(walk.commands, [MenuCommand::Resume]);
    assert!(walk.menus.tree(MenuScreen::Pause).is_none());
}

#[test]
fn a_platform_with_no_way_out_offers_none() {
    let settings = PlayerSettings::default();
    let setup = MenuSetup { settings, can_leave: false, pseudo: false };
    let menus = GameMenus::new(assets(), Atlas::paint(), setup).unwrap();
    let mut walk = Walk {
        menus,
        size: (844, 390),
        frame: 0,
        list: UiDrawList::default(),
        device: Device::Keyboard,
        commands: Vec::new(),
        wrong: Vec::new(),
    };
    walk.menus.open(MenuScreen::Title);
    walk.settle();
    let tree = walk.tree(MenuScreen::Title);
    assert!(!tree.is_visible(tree.find("leave").unwrap()));
    assert_eq!(tree.unreachable(), Vec::<&str>::new());
}

/// What the documents say and what the code does agree: each model's document is what its type has, each id
/// is its sidecar's, each node that takes focus does something, and each screen comes in and goes out.
#[test]
fn the_documents_and_the_code_agree() {
    for (screen, info) in [
        (MenuScreen::Title, TitleModel::type_info()),
        (MenuScreen::Pause, PauseModel::type_info()),
        (MenuScreen::Settings, SettingsModel::type_info()),
        (MenuScreen::Character, CharacterModel::type_info()),
        (MenuScreen::Dialogue, DialogueModel::type_info()),
        (MenuScreen::Hud, HudModel::type_info()),
    ] {
        let path = content().join(format!("ui/{}.model.ron", screen.stem()));
        let model = UiModel::from_document(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(model, UiModel::of_type(info), "{screen:?}");
    }
    for (screen, id) in MenuScreen::ALL.into_iter().zip(MenuScreen::ids()) {
        let path = content().join(format!("ui/{}.ui.ron.meta", screen.stem()));
        let sidecar = std::fs::read_to_string(path).unwrap();
        assert!(sidecar.contains(&id.to_string()), "{id} isn't in {sidecar}");
        let document = document(screen);
        for node in document.nodes.iter().filter(|node| node.focus.is_some()) {
            let action = action_of(screen, &node.name);
            assert!(action.is_some(), "nothing's written for {screen:?}'s {}", node.name);
            let fires = node.focus.as_ref().and_then(|focus| focus.fires.as_deref());
            if action == Some(MenuAction::Nothing) && !node.name.starts_with("skill-") {
                assert!(fires.is_some(), "{screen:?}'s {} does nothing at all", node.name);
            }
        }
    }
}
