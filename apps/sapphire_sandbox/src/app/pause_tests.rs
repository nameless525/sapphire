//! What a pause holds and says, by the app's own view of a frame ([`drive_view`]), with no GPU.

use crate::input::SandboxInput;
use game_combat::arena::{ArenaGround, ArenaShot};
use game_combat::enemy::{self, EnemyMeshes};
use game_combat::fight::Arena;
use game_ui::hud::{FightHud, HudMode, HudOptions, ShownFight};
use sapphire_math::{Mat4, Vec3};
use sapphire_render::MeshHandle;
use sapphire_stage::RenderPose;
use sapphire_stage::lookdev::{self, TownMeshes};
use sapphire_stage::scene::{Figure, LookDev, Scene};

use super::drive_view;

struct Staged {
    scene: Scene,
    show: ShownFight,
}

/// The arena's stage with stand-in meshes, framed by `shot`.
fn arena_stage(shot: ArenaShot) -> Staged {
    let plain = MeshHandle::from_raw(1);
    let town = TownMeshes {
        plants: [plain; lookdev::Plant::ALL.len()],
        mountains: [plain; lookdev::MOUNTAINS.len()],
    };
    let figure = Figure { head: Vec3::new(0.0, 1.45, 0.0), ..Figure::MANNEQUIN };
    let pose = RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] };
    let look = LookDev { town, fog_cards: None, pose, figure };
    let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
    let set = sapphire_colliders::ColliderSet::read(&cooked).expect("it reads");
    let ground = ArenaGround::new(set).expect("the stage stands on it");
    let bounds = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];
    let mut scene = Scene::with_character(MeshHandle::from_raw(0), bounds, false);
    let enemy = EnemyMeshes { warrior: MeshHandle::from_raw(2), blade: MeshHandle::from_raw(3) };
    let held = enemy::Held {
        pose: RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] },
        figure: Figure { head: Vec3::new(0.0, 2.2, 0.0), ..Figure::MANNEQUIN },
        blade: Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        hand: 0,
    };
    let mut show = ShownFight::default();
    show.fight.show.spawn_arena(&mut scene, Arena { enemy, held, shot, look, ground });
    Staged { scene, show }
}

/// The HUD says a fight is waiting for as long as it is, on a phone as on a desktop, and a capture
/// can ask for the same.
#[test]
fn a_pause_shows_on_the_hud() {
    let quads = |scene: &mut Staged| {
        let mut list = sapphire_ui::UiDrawList::default();
        scene.show.hud.as_mut().expect("set").draw((1000, 500), &mut list);
        list.quads.len()
    };
    let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
    let scene = Scene::with_character(MeshHandle::from_raw(0), [[-1.0; 3], [1.0; 3]], false);
    let show = ShownFight { hud: Some(FightHud::for_fight(live)), ..ShownFight::default() };
    let mut scene = Staged { scene, show };
    let size = sapphire_platform::Size { width: 1000, height: 500 };
    let mut input = SandboxInput::new(false, true, size).expect("the default bindings");
    let time = |ms: u64| sapphire_platform::Timestamp::from_nanos(ms * 1_000_000);
    input.update(time(1));
    drive_view(&mut scene.scene, &mut scene.show, &input, 1.0 / 60.0);
    let running = quads(&mut scene);

    input.pause();
    input.update(time(2));
    drive_view(&mut scene.scene, &mut scene.show, &input, 1.0 / 60.0);
    assert_eq!(quads(&mut scene), running + 3, "a veil and two bars");

    let key = |ms: u64, down| {
        sapphire_platform::Event::Key(sapphire_platform::KeyEvent {
            timestamp: time(ms),
            window: sapphire_platform::WindowId::from_raw(1),
            scancode: sapphire_platform::Scancode::P,
            keycode: sapphire_platform::Keycode(0),
            modifiers: sapphire_platform::Modifiers::default(),
            down,
            repeat: false,
        })
    };
    input.handle_event(&key(3, true));
    input.handle_event(&key(4, false));
    input.update(time(5));
    input.update(time(6));
    assert!(!input.is_paused());
    drive_view(&mut scene.scene, &mut scene.show, &input, 1.0 / 60.0);
    assert_eq!(quads(&mut scene), running, "gone with the pause");

    let asked = HudOptions { paused: true, ..live };
    scene.show.hud = Some(FightHud::for_fight(asked));
    assert_eq!(quads(&mut scene), running + 3, "with no input at all");
}

/// A pause holds what's on screen, a shake mid-swing and the director's springs among it, however
/// long it lasts, and the shake plays on from there when it ends.
#[test]
fn a_pause_holds_the_directors_view_and_its_shake() {
    let mut scene = arena_stage(ArenaShot::Director);
    let size = sapphire_platform::Size { width: 1000, height: 500 };
    let mut input = SandboxInput::new(true, true, size).expect("the default bindings");
    let press = |input: &mut SandboxInput, at: u64| {
        let time = |ms: u64| sapphire_platform::Timestamp::from_nanos(ms * 1_000_000);
        let key = |ms: u64, down| {
            sapphire_platform::Event::Key(sapphire_platform::KeyEvent {
                timestamp: time(ms),
                window: sapphire_platform::WindowId::from_raw(1),
                scancode: sapphire_platform::Scancode::P,
                keycode: sapphire_platform::Keycode(0),
                modifiers: sapphire_platform::Modifiers::default(),
                down,
                repeat: false,
            })
        };
        input.handle_event(&key(at, true));
        input.handle_event(&key(at + 1, false));
        input.update(time(at + 2));
    };
    let frame = |scene: &mut Staged, input: &SandboxInput| {
        drive_view(&mut scene.scene, &mut scene.show, input, 1.0 / 60.0);
        let view = scene
            .scene
            .frame_packet(1.0, Some((16, 9)), &mut scene.show)
            .main_view()
            .expect("a view")
            .camera
            .view;
        scene.scene.end_frame();
        view
    };
    let _ = frame(&mut scene, &input);
    let arena = scene.show.fight.show.arena_mut().expect("built");
    let director = arena.director().expect("this shot has one");
    let finish = director.tuning().shake_id("finish").expect("a shipped preset");
    director.fire_shake(finish, 1.0);
    for _ in 0..4 {
        let _ = frame(&mut scene, &input);
    }

    press(&mut input, 100);
    assert!(input.is_paused());
    let held = frame(&mut scene, &input);
    for _ in 0..120 {
        assert_eq!(frame(&mut scene, &input), held, "nothing moves in a pause");
    }

    press(&mut input, 200);
    input.update(sapphire_platform::Timestamp::from_nanos(210_000_000));
    assert!(!input.is_paused());
    let moved = (0..4).any(|_| frame(&mut scene, &input) != held);
    assert!(moved, "the shake plays on from where it was");
}
