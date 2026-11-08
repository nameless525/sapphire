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
use sapphire_input::{PadButton, RawInput};
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_platform::{GamepadType, Scancode, Timestamp, TouchPhase};
use sapphire_stage::content::ScreenAssets;
use sapphire_ui::{Atlas, UiTree};
use support::{assets, content, document};

/// The aspects a screen ships at, in points: 4:3, 16:9 and 21:9, and the smallest phone that runs the game.
const SIZES: [(u32, u32); 5] = [(1024, 768), (1280, 720), (1680, 720), (844, 390), (667, 375)];

/// How a walk presses.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Device {
    Pad,
    Keyboard,
    Finger,
}

/// The menus, a frame at a time, with time going by as a phone's does.
struct Walk {
    menus: GameMenus,
    size: (u32, u32),
    frame: u64,
    list: UiDrawList,
    device: Device,
    commands: Vec<MenuCommand>,
    /// What's gone wrong on any screen, at any step.
    wrong: Vec<String>,
}

impl Walk {
    fn new(assets: ScreenAssets, locale: &str, pseudo: bool, size: (u32, u32), device: Device) -> Self {
        let settings = PlayerSettings { language: locale.to_owned(), ..PlayerSettings::default() };
        let setup = MenuSetup { settings, can_leave: true, pseudo };
        let mut menus = GameMenus::new(assets, Atlas::paint(), setup).unwrap();
        let at = Timestamp::from_nanos(1);
        menus.controls_mut().push(at, RawInput::GamepadAdded { pad: 0, kind: GamepadType::XboxOne });
        Self {
            menus,
            size,
            frame: 0,
            list: UiDrawList::default(),
            device,
            commands: Vec::new(),
            wrong: Vec::new(),
        }
    }

    fn now(&self) -> Timestamp {
        Timestamp::from_nanos(1_000_000_000 + self.frame * 16_666_667)
    }

    fn frame(&mut self) {
        self.frame += 1;
        let now = self.now();
        self.menus.update(now);
        self.commands.extend(self.menus.take_commands());
        self.list.reset();
        self.menus.draw(self.size, &mut self.list);
    }

    fn settle(&mut self) {
        for _ in 0..30 {
            self.frame();
        }
    }

    fn press(&mut self, down: RawInput, up: RawInput) {
        let at = self.now();
        let controls = self.menus.controls_mut();
        controls.push(at, down);
        self.frame();
        let at = self.now();
        self.menus.controls_mut().push(at, up);
        self.frame();
    }

    fn key(&mut self, scancode: Scancode) {
        self.press(RawInput::Key { scancode, down: true }, RawInput::Key { scancode, down: false });
    }

    fn pad(&mut self, button: PadButton) {
        let at = |down| RawInput::PadButton { pad: 0, button, down };
        self.press(at(true), at(false));
    }

    /// A finger down and up at `point`, in the interface's points: what the window calls it is scaled by
    /// the player's size.
    fn tap(&mut self, point: Vec2) {
        let point = point * self.menus.settings().scale();
        let at = |phase| RawInput::Touch { finger: 1, phase, position: point };
        self.press(at(TouchPhase::Down), at(TouchPhase::Up));
    }

    /// Moves focus on the top screen to `node`, by the walk's device: a finger needs no walk.
    fn focus(&mut self, screen: MenuScreen, node: &str) {
        if self.device == Device::Finger {
            return;
        }
        for _ in 0..12 {
            if self.tree(screen).focus_name() == Some(node) {
                return;
            }
            let target = {
                let tree = self.tree(screen);
                tree.screen_rect(tree.find(node).unwrap()).center()
            };
            let from = {
                let tree = self.tree(screen);
                tree.screen_rect(tree.focus().expect("something has focus")).center()
            };
            let delta = target - from;
            if delta.y.abs() >= delta.x.abs() {
                if delta.y > 0.0 { self.down() } else { self.up() }
            } else if delta.x > 0.0 {
                self.right();
            } else {
                self.left();
            }
        }
        panic!("{node} can't be reached on {screen:?} by {:?}", self.device);
    }

    fn up(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadUp),
            _ => self.key(Scancode::UP),
        }
    }

    fn down(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadDown),
            _ => self.key(Scancode::DOWN),
        }
    }

    fn left(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadLeft),
            _ => self.key(Scancode::LEFT),
        }
    }

    fn right(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadRight),
            _ => self.key(Scancode::RIGHT),
        }
    }

    fn tree(&self, screen: MenuScreen) -> &UiTree {
        self.menus
            .tree(screen)
            .unwrap_or_else(|| panic!("{screen:?} isn't showing, by {:?} at {:?}", self.device, self.size))
    }

    /// Chooses `node`: by the device's confirm with focus on it, or by a finger on it.
    fn choose(&mut self, screen: MenuScreen, node: &str) {
        self.focus(screen, node);
        match self.device {
            Device::Pad => self.pad(PadButton::South),
            Device::Keyboard => self.key(Scancode::RETURN),
            Device::Finger => {
                self.drag_to(screen, node);
                let tree = self.tree(screen);
                let center = tree.screen_rect(tree.find(node).unwrap()).center();
                self.tap(center);
            }
        }
        self.settle();
    }

    /// A finger on a short list drags what it wants to where it can land on it.
    fn drag_to(&mut self, screen: MenuScreen, node: &str) {
        let list = match (screen, node) {
            (MenuScreen::Settings, "language" | "size" | "sound" | "rumble") => "list",
            (MenuScreen::Character, "skill-attack" | "skill-dodge" | "skill-art") => "skills",
            _ => return,
        };
        let scale = self.menus.settings().scale();
        let tree = self.tree(screen);
        let (shown, row) =
            (tree.screen_rect(tree.find(list).unwrap()), tree.screen_rect(tree.find(node).unwrap()));
        if shown.contains_rect(row) {
            return;
        }
        let from = shown.center();
        let to = from - Vec2::new(0.0, row.center().y - from.y);
        let touch = |phase, point: Vec2| RawInput::Touch { finger: 1, phase, position: point * scale };
        self.press(touch(TouchPhase::Down, from), touch(TouchPhase::Moved, to));
        let at = self.now();
        self.menus.controls_mut().push(at, touch(TouchPhase::Up, to));
        self.settle();
    }

    /// Goes back: a finger has the row, and the others their button.
    fn back(&mut self, screen: MenuScreen) {
        match self.device {
            Device::Pad => self.pad(PadButton::East),
            Device::Keyboard => self.key(Scancode::ESCAPE),
            Device::Finger => {
                let tree = self.tree(screen);
                let center = tree.screen_rect(tree.find("back").unwrap()).center();
                self.tap(center);
            }
        }
        self.settle();
    }

    /// What exit test 4 holds of a screen once it's settled.
    fn check(&mut self, screen: MenuScreen, pseudo: bool) {
        let tree = self.tree(screen);
        let (size, device) = (self.size, self.device);
        let mut wrong = Vec::new();
        let at = format!("{screen:?} at {size:?} by {device:?}");
        if !tree.problems().is_empty() {
            wrong.push(format!("{at}: {:?}", tree.problems()));
        }
        let unreachable = tree.unreachable();
        if !unreachable.is_empty() && !matches!(screen, MenuScreen::Hud) {
            wrong.push(format!("{at}: can't reach {unreachable:?}"));
        }
        let overflowing = tree.overflowing();
        if !overflowing.is_empty() {
            wrong.push(format!("{at}: {overflowing:?} doesn't fit"));
        }
        if pseudo {
            let unframed: Vec<_> = tree.unframed().collect();
            if !unframed.is_empty() {
                wrong.push(format!("{at}: outside the tables: {unframed:?}"));
            }
        }
        self.wrong.extend(wrong);
    }
}

/// Every menu, by one device.
fn walk_every_menu(walk: &mut Walk, pseudo: bool) {
    walk.menus.open(MenuScreen::Title);
    walk.settle();
    walk.check(MenuScreen::Title, pseudo);

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

    // Start: the title goes, and the fight's words stand behind it.
    walk.menus.open_under(MenuScreen::Hud);
    walk.choose(MenuScreen::Title, "start");
    assert!(walk.commands.contains(&MenuCommand::Start), "{:?}", walk.commands);
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
    assert_eq!(string_of(&walk, MenuScreen::Title, "start-text"), "Start");
    assert_eq!(string_of(&walk, MenuScreen::Settings, "language-value"), "English");
    walk.choose(MenuScreen::Settings, "language");
    assert_eq!(walk.menus.settings().language, "ja");
    assert_eq!(string_of(&walk, MenuScreen::Settings, "language-value"), "日本語");
    assert_eq!(string_of(&walk, MenuScreen::Title, "start-text"), "はじめる", "under it too");
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
