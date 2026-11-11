//! Remapping (M4 A4), by the real documents and real input values through `MenuControls`: a press remaps, a
//! press another binding has asks to swap or keep, a press no player remaps is kept, a reset puts a device
//! back as the game's document has it, and the glyphs every remap screen shows follow.

mod support;

use game_ui::menus::{MenuCommand, MenuModel, MenuScreen};
use sapphire_controls::{RemapDevice, Remapped};
use sapphire_input::{Key, PadAxis, PadButton, RawInput, Source};
use sapphire_platform::{GamepadType, Scancode};
use support::assets;
use support::walk::{Device, REMAP_KEYS, REMAP_PAD, Walk};

fn walk_on(screen: MenuScreen, device: Device) -> Walk {
    let mut walk = Walk::new(assets(), "en", false, (1280, 720), device);
    walk.menus.open(screen);
    walk.settle();
    walk
}

fn shown(walk: &mut Walk, screen: MenuScreen) -> MenuModel {
    walk.menus.model_mut::<MenuModel>(screen).expect("a menu's model").clone()
}

/// What `node` on `screen` draws.
fn drawn(walk: &Walk, screen: MenuScreen, node: &str) -> String {
    let tree = walk.tree(screen);
    tree.strings()
        .find(|(name, _)| *name == node)
        .unwrap_or_else(|| panic!("{node} draws nothing"))
        .1
        .to_owned()
}

fn axis(walk: &mut Walk, axis: PadAxis, value: f32) {
    let at = walk.now();
    walk.menus.controls_mut().push(at, RawInput::PadAxis { pad: 0, axis, value });
    walk.frame();
}

#[test]
fn a_press_remaps_its_binding_and_the_glyph_follows() {
    let mut walk = walk_on(REMAP_PAD, Device::Pad);
    assert_eq!(drawn(&walk, REMAP_PAD, "ultimate-value"), "RT");
    walk.choose(REMAP_PAD, "light");
    assert!(walk.menus.is_open(MenuScreen::REMAP_LISTEN));
    assert_eq!(shown(&mut walk, MenuScreen::REMAP_LISTEN).remap.binding, "remap-action-light");
    assert_eq!(drawn(&walk, MenuScreen::REMAP_LISTEN, "cancel-hint"), "Menu to cancel");

    // A stick isn't a button: the wait goes on.
    axis(&mut walk, PadAxis::LeftX, 1.0);
    axis(&mut walk, PadAxis::LeftX, 0.0);
    assert!(walk.menus.is_remapping(), "a stick is refused");

    axis(&mut walk, PadAxis::LeftTrigger, 1.0);
    axis(&mut walk, PadAxis::LeftTrigger, 0.0);
    walk.settle();
    assert!(!walk.menus.is_remapping());
    assert!(walk.menus.tree(MenuScreen::REMAP_LISTEN).is_none());
    let remapped =
        Remapped { binding: "light.gamepad".to_owned(), source: Source::PadAxis(PadAxis::LeftTrigger) };
    assert_eq!(walk.menus.settings().remapped, [remapped]);
    assert!(walk.commands.contains(&MenuCommand::SettingsChanged), "{:?}", walk.commands);
    assert_eq!(drawn(&walk, REMAP_PAD, "light-value"), "LT", "the glyph follows");

    // The glyphs follow the pad in hand, too.
    let at = walk.now();
    walk.menus.controls_mut().push(at, RawInput::GamepadAdded { pad: 1, kind: GamepadType::PlayStation5 });
    let at = walk.now();
    walk.menus
        .controls_mut()
        .push(at, RawInput::PadButton { pad: 1, button: PadButton::DpadDown, down: true });
    walk.settle();
    assert_eq!(drawn(&walk, REMAP_PAD, "light-value"), "L2");
    assert_eq!(drawn(&walk, REMAP_PAD, "heavy-value"), "○");
}

#[test]
fn a_press_another_binding_has_swaps_or_keeps() {
    for swap in [true, false] {
        let mut walk = walk_on(REMAP_KEYS, Device::Keyboard);
        walk.choose(REMAP_KEYS, "light");
        walk.key(Scancode::K);
        walk.settle();
        assert!(walk.menus.is_open(MenuScreen::REMAP_TAKEN), "K is the heavy attack's");
        assert_eq!(drawn(&walk, MenuScreen::REMAP_TAKEN, "taken"), "K is already used for");
        assert_eq!(drawn(&walk, MenuScreen::REMAP_TAKEN, "taken-by"), "Heavy attack");
        walk.commands.clear();
        walk.choose(MenuScreen::REMAP_TAKEN, if swap { "swap" } else { "keep" });
        assert!(!walk.menus.is_remapping() && walk.menus.tree(MenuScreen::REMAP_TAKEN).is_none());
        if swap {
            let remapped =
                |binding: &str, key| Remapped { binding: binding.to_owned(), source: Source::Key(key) };
            assert_eq!(
                walk.menus.settings().remapped,
                [remapped("light.keyboard", Key::K), remapped("heavy.keyboard", Key::J)]
            );
            assert_eq!(walk.commands, [MenuCommand::SettingsChanged]);
            assert_eq!(drawn(&walk, REMAP_KEYS, "light-value"), "K");
            assert_eq!(drawn(&walk, REMAP_KEYS, "heavy-value"), "J");

            // A reset puts the keyboard back, and only the keyboard.
            walk.choose(REMAP_KEYS, "tab-gamepad");
            walk.choose(REMAP_PAD, "jump");
            walk.pad(PadButton::LeftStick);
            walk.settle();
            let pad =
                Remapped { binding: "jump.gamepad".to_owned(), source: Source::Pad(PadButton::LeftStick) };
            assert_eq!(walk.menus.settings().remapped.last(), Some(&pad));
            walk.choose(REMAP_PAD, "tab-keyboard");
            walk.choose(REMAP_KEYS, "reset");
            assert_eq!(walk.menus.settings().remapped, [pad]);
            assert_eq!(drawn(&walk, REMAP_KEYS, "light-value"), "J");
        } else {
            assert_eq!(walk.menus.settings().remapped, [], "nothing changed");
            assert!(walk.commands.is_empty(), "{:?}", walk.commands);
            assert_eq!(drawn(&walk, REMAP_KEYS, "light-value"), "J");
        }
    }
}

#[test]
fn a_press_no_player_remaps_is_kept_and_the_wait_goes_on() {
    let mut walk = walk_on(REMAP_PAD, Device::Pad);
    walk.choose(REMAP_PAD, "jump");
    walk.pad(PadButton::Back);
    walk.settle();
    assert!(walk.menus.is_remapping() && walk.menus.is_open(MenuScreen::REMAP_LISTEN), "still waiting");
    let model = shown(&mut walk, MenuScreen::REMAP_LISTEN).remap;
    assert!(model.kept);
    assert_eq!((model.pressed.as_str(), model.taken_by.as_str()), ("View", "remap-action-pause"));
    assert!(
        walk.tree(MenuScreen::REMAP_LISTEN)
            .is_visible(walk.tree(MenuScreen::REMAP_LISTEN).find("kept").unwrap())
    );

    // Start cancels, and is never a binding.
    walk.pad(PadButton::Start);
    walk.settle();
    assert!(!walk.menus.is_remapping() && walk.menus.tree(MenuScreen::REMAP_LISTEN).is_none());
    assert_eq!(walk.menus.settings().remapped, []);
    assert!(walk.menus.is_open(REMAP_PAD), "back on its tab");
}

#[test]
fn each_tab_listens_to_its_own_device() {
    // The keyboard's wait takes no pad button: the pad still moves through the screen, and back cancels.
    let mut walk = walk_on(REMAP_KEYS, Device::Keyboard);
    walk.choose(REMAP_KEYS, "jump");
    walk.pad(PadButton::LeftShoulder);
    assert!(walk.menus.is_remapping(), "a pad button isn't a key");
    walk.pad(PadButton::East);
    walk.settle();
    assert!(!walk.menus.is_remapping(), "the pad's back cancels");
    assert_eq!(walk.menus.settings().remapped, []);

    // A device's own cancel: Escape on the keyboard.
    walk.choose(REMAP_KEYS, "jump");
    walk.key(Scancode::ESCAPE);
    walk.settle();
    assert!(!walk.menus.is_remapping() && walk.menus.is_open(REMAP_KEYS));
    assert_eq!(walk.menus.settings().remapped, []);
    assert_eq!(RemapDevice::of_binding("jump.keyboard"), Some(RemapDevice::Keyboard));
}
