//! A walk through the menus, a frame at a time, by a pad, a keyboard or a finger, all through
//! `MenuControls`: what exit test 4's walk and the dialogue panel's tests press with.

use game_ui::menus::{GameMenus, MenuCommand, MenuScreen, MenuSetup, PlayerSettings};
use sapphire_input::{PadButton, RawInput};
use sapphire_look::UiDrawList;
use sapphire_math::Vec2;
use sapphire_platform::{GamepadType, Scancode, Timestamp, TouchPhase};
use sapphire_ui::{Atlas, UiTree};

use super::Screens;

/// The settings, which no code opens: the game's documents do.
pub const SETTINGS: MenuScreen = MenuScreen::named("settings");
/// The character screen, likewise.
pub const CHARACTER: MenuScreen = MenuScreen::named("character");
/// The settings' pages (M4 A1), which their hub opens.
pub const DISPLAY: MenuScreen = MenuScreen::named("display");
/// The graphics page (M4 T3).
pub const GRAPHICS: MenuScreen = MenuScreen::named("graphics");
pub const SOUND: MenuScreen = MenuScreen::named("sound");
pub const SUBTITLES: MenuScreen = MenuScreen::named("subtitles");
pub const CONTROLS: MenuScreen = MenuScreen::named("controls");
/// The settings' hub and its pages, each a scrolling list of rows over a way back.
pub const SETTINGS_SCREENS: [MenuScreen; 6] = [SETTINGS, DISPLAY, GRAPHICS, SOUND, SUBTITLES, CONTROLS];

/// How a walk presses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Device {
    Pad,
    Keyboard,
    Finger,
}

/// The menus, a frame at a time, with time going by as a phone's does.
pub struct Walk {
    pub menus: GameMenus,
    pub size: (u32, u32),
    pub frame: u64,
    pub list: UiDrawList,
    pub device: Device,
    pub commands: Vec<MenuCommand>,
    /// What's gone wrong on any screen, at any step.
    pub wrong: Vec<String>,
}

impl Walk {
    pub fn new(screens: Screens, locale: &str, pseudo: bool, size: (u32, u32), device: Device) -> Self {
        let settings = PlayerSettings { language: locale.to_owned(), ..PlayerSettings::default() };
        let Screens { assets, menus: screens } = screens;
        let setup = MenuSetup { screens, settings, can_leave: true, can_mark: true, pseudo };
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

    pub fn now(&self) -> Timestamp {
        Timestamp::from_nanos(1_000_000_000 + self.frame * 16_666_667)
    }

    pub fn frame(&mut self) {
        self.frame += 1;
        let now = self.now();
        self.menus.update(now);
        self.commands.extend(self.menus.take_commands());
        self.list.reset();
        self.menus.draw(self.size, &mut self.list);
    }

    pub fn settle(&mut self) {
        for _ in 0..30 {
            self.frame();
        }
    }

    pub fn press(&mut self, down: RawInput, up: RawInput) {
        let at = self.now();
        let controls = self.menus.controls_mut();
        controls.push(at, down);
        self.frame();
        let at = self.now();
        self.menus.controls_mut().push(at, up);
        self.frame();
    }

    pub fn key(&mut self, scancode: Scancode) {
        self.press(RawInput::Key { scancode, down: true }, RawInput::Key { scancode, down: false });
    }

    pub fn pad(&mut self, button: PadButton) {
        let at = |down| RawInput::PadButton { pad: 0, button, down };
        self.press(at(true), at(false));
    }

    /// A finger down and up at `point`, in the interface's points: what the window calls it is scaled by
    /// the player's size.
    pub fn tap(&mut self, point: Vec2) {
        let point = point * self.menus.settings().scale();
        let at = |phase| RawInput::Touch { finger: 1, phase, position: point };
        self.press(at(TouchPhase::Down), at(TouchPhase::Up));
    }

    /// Moves focus on the top screen to `node`, by the walk's device: a finger needs no walk.
    pub fn focus(&mut self, screen: MenuScreen, node: &str) {
        if self.device == Device::Finger {
            return;
        }
        for _ in 0..12 {
            if self.tree(screen).focus_name() == Some(node) {
                return;
            }
            let target = {
                let tree = self.tree(screen);
                tree.screen_rect(tree.find(node).unwrap())
            };
            let from = {
                let tree = self.tree(screen);
                tree.screen_rect(tree.focus().expect("something has focus")).center()
            };
            let delta = target.center() - from;
            // On another row, the way is up or down, however far apart a right-aligned column's centers are.
            let rows_apart = from.y < target.min.y || from.y > target.max.y;
            if rows_apart || delta.y.abs() >= delta.x.abs() {
                if delta.y > 0.0 { self.down() } else { self.up() }
            } else if delta.x > 0.0 {
                self.right();
            } else {
                self.left();
            }
        }
        panic!("{node} can't be reached on {screen:?} by {:?}", self.device);
    }

    pub fn up(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadUp),
            _ => self.key(Scancode::UP),
        }
    }

    pub fn down(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadDown),
            _ => self.key(Scancode::DOWN),
        }
    }

    pub fn left(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadLeft),
            _ => self.key(Scancode::LEFT),
        }
    }

    pub fn right(&mut self) {
        match self.device {
            Device::Pad => self.pad(PadButton::DpadRight),
            _ => self.key(Scancode::RIGHT),
        }
    }

    pub fn tree(&self, screen: MenuScreen) -> &UiTree {
        self.menus
            .tree(screen)
            .unwrap_or_else(|| panic!("{screen:?} isn't showing, by {:?} at {:?}", self.device, self.size))
    }

    /// Chooses `node`: by the device's confirm with focus on it, or by a finger on it.
    pub fn choose(&mut self, screen: MenuScreen, node: &str) {
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
    pub fn drag_to(&mut self, screen: MenuScreen, node: &str) {
        let list = match node {
            "back" => return,
            _ if SETTINGS_SCREENS.contains(&screen) => "list",
            "skill-attack" | "skill-dodge" | "skill-art" if screen == CHARACTER => "skills",
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
    pub fn back(&mut self, screen: MenuScreen) {
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
    pub fn check(&mut self, screen: MenuScreen, pseudo: bool) {
        let tree = self.tree(screen);
        let (size, device) = (self.size, self.device);
        let mut wrong = Vec::new();
        let at = format!("{screen:?} at {size:?} by {device:?}");
        if !tree.problems().is_empty() {
            wrong.push(format!("{at}: {:?}", tree.problems()));
        }
        let unreachable = tree.unreachable();
        if !unreachable.is_empty() && screen != MenuScreen::HUD {
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
