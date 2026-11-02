//! A touch's whole road with no window: the platform's event through the app's own routing
//! ([`game_event`], `Scene::read_touch`, `SandboxInput::update`), to what a fixed step reads, into a real
//! fight and its file. What only a window can add is the event itself, which the emulator's run supplies.

use std::path::PathBuf;

use sapphire_gameplay::replay::{Player, Replay, SCENARIOS};
use sapphire_gameplay::{ActorId, ActorView, Simulation, Stick};
use sapphire_math::{Vec2, Vec3};
use sapphire_platform::{
    Event, FingerId, KeyEvent, Keycode, LifecycleEvent, Modifiers, Scancode, Timestamp, TouchDeviceId,
    TouchEvent, TouchPhase, WindowEvent, WindowId,
};
use sapphire_render::MeshHandle;
use sapphire_ui::{HudLayout, Insets, UiMetrics};

use super::game_event;
use crate::combat::Combat;
use crate::hud::{HudMode, HudOptions, SandboxHud};
use crate::input::{PLAYER, SandboxInput};
use crate::options::CombatOption;
use crate::scene::Scene;

const SIZE: Vec2 = Vec2::new(900.0, 400.0);
const FRAME_MS: u64 = 16;
const THUMB: u64 = 1;
const OTHER_THUMB: u64 = 2;
/// Android's Back, as SDL's `SDL_SCANCODE_AC_BACK`.
const BACK: Scancode = Scancode(282);

fn ms(ms: u64) -> Timestamp {
    Timestamp::from_nanos(ms * 1_000_000)
}

/// The app's frame around a fight, with no window: everything `SandboxApp::iterate` does with a touch,
/// in its order, by the functions it calls.
struct Rig {
    scene: Scene,
    input: SandboxInput,
    combat: Combat,
    layout: HudLayout,
    now: u64,
}

impl Rig {
    fn new(combat: Combat) -> Self {
        let bounds = [[-1.0, -1.0, -1.0], [1.0, 1.0, 1.0]];
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), bounds, false);
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        scene.set_hud(SandboxHud::for_fight(live));
        let size = sapphire_platform::Size { width: SIZE.x as u32, height: SIZE.y as u32 };
        let input = SandboxInput::new(false, true, size).expect("the default bindings");
        let layout = HudLayout::new(metrics());
        let mut rig = Self { scene, input, combat, layout, now: 0 };
        rig.frame();
        rig
    }

    fn fight() -> Self {
        Self::new(
            Combat::new(&CombatOption::Fight { script: None, party: false }).expect("the scenario starts"),
        )
    }

    /// Turns the view by this much, settled: the stick must follow it.
    fn turn_camera(&mut self, yaw: f32) {
        let mut camera = self.scene.camera_mut();
        camera.rotate(yaw, 0.0);
        for _ in 0..600 {
            camera.update(1.0 / 60.0);
        }
    }

    /// A platform event, as `App::event` routes it.
    fn event(&mut self, event: &Event) {
        game_event(&mut self.scene, &mut self.input, event);
    }

    fn touch(&mut self, offset: u64, id: u64, phase: TouchPhase, at: Vec2) {
        self.event(&Event::Touch(TouchEvent {
            timestamp: ms(self.now + offset),
            window: WindowId::from_raw(1),
            finger: FingerId::new(TouchDeviceId::from_raw(1), id),
            phase,
            x: at.x / SIZE.x,
            y: at.y / SIZE.y,
            pressure: 1.0,
        }));
    }

    fn key(&mut self, offset: u64, scancode: Scancode, down: bool) {
        self.event(&Event::Key(KeyEvent {
            timestamp: ms(self.now + offset),
            window: WindowId::from_raw(1),
            scancode,
            keycode: Keycode(0),
            modifiers: Modifiers::default(),
            down,
            repeat: false,
        }));
    }

    fn back(&mut self) {
        self.key(1, BACK, true);
        self.key(2, BACK, false);
    }

    /// A thumb on the stick, pushed by `by` of its radius with +y forward.
    fn push_stick(&mut self, id: u64, by: Vec2) {
        let (rest, radius) = (self.layout.stick.rest, self.layout.stick.radius);
        self.touch(1, id, TouchPhase::Down, rest);
        self.touch(2, id, TouchPhase::Moved, rest + Vec2::new(by.x, -by.y) * radius);
    }

    /// The top of a frame: the zones, then the frame's input. Returns whether the player left.
    fn frame(&mut self) -> bool {
        self.now += FRAME_MS;
        let now = ms(self.now);
        self.scene.read_touch(now, metrics(), &mut self.input);
        self.input.update(now)
    }

    /// What a fixed step reads, unturned by any fight: it moves a press along, as a step does.
    fn read(&mut self) -> sapphire_gameplay::ActorActions {
        let heading = self.scene.camera_heading();
        let mut source = self.input.source(heading);
        sapphire_gameplay::replay::ActionSource::actions(&mut source, 0, &[PLAYER]).get(PLAYER)
    }

    /// A fixed step of the fight, as `read_combat_step` asks it.
    fn step(&mut self) {
        let heading = self.scene.camera_heading();
        let mut source = self.input.source(heading);
        crate::combat::read_bindings(Some(&mut self.combat), &mut source, PLAYER);
        self.scene.follow_simulation(self.combat.simulation(), self.combat.cast());
        self.scene.show_damage(self.combat.simulation());
    }

    fn sim(&self) -> &Simulation {
        self.combat.simulation()
    }

    fn player(&self) -> ActorView {
        let id: ActorId = self.combat.cast()[0];
        self.sim().actor(id).expect("the player")
    }

    fn running(&self) -> Option<sapphire_core::Name> {
        self.player().moves.current.map(|current| current.name)
    }
}

fn metrics() -> UiMetrics {
    UiMetrics::new(SIZE, Insets::default())
}

/// The whole road: a thumb walks the player away from a turned camera, and another presses the attack
/// while it does, which starts the real move, once, and the walk goes on.
#[test]
fn touch_walks_the_player_where_the_view_looks_and_starts_a_move() {
    let mut rig = Rig::fight();
    rig.turn_camera(1.1);
    // From the matrix that draws the frame, and not from the heading the app hands the bindings.
    let view = rig.scene.camera().camera_data(1.0).view;
    let forward = Vec3::new(-view.x_axis.z, 0.0, -view.z_axis.z).normalize();

    let start = rig.player().position;
    rig.push_stick(THUMB, Vec2::new(0.0, 1.0));
    for _ in 0..30 {
        rig.frame();
        rig.step();
    }
    let walked = rig.player().position - start;
    let along = Vec3::new(walked.x, 0.0, walked.z);
    assert!(along.length() > 0.5, "it walked: {walked}");
    assert!(along.normalize().dot(forward) > 0.99, "away from the view: {along}, the view's {forward}");
    assert_eq!(rig.running(), None, "no verb yet");

    rig.touch(1, OTHER_THUMB, TouchPhase::Down, rig.layout.attack.center);
    rig.frame();
    rig.step();
    assert_eq!(rig.running(), Some(sapphire_core::Name::new("light_1")));
    let serial = rig.player().moves.current.expect("running").serial;
    for _ in 0..8 {
        rig.frame();
        rig.step();
    }
    assert_eq!(rig.player().moves.current.expect("running").serial, serial, "one press, held");
}

#[test]
fn a_tap_inside_a_frame_is_a_press_and_a_dodge_is_a_dodge() {
    let mut rig = Rig::fight();
    rig.touch(1, THUMB, TouchPhase::Down, rig.layout.dodge.center);
    rig.touch(2, THUMB, TouchPhase::Up, rig.layout.dodge.center);
    rig.frame();
    rig.step();
    assert_eq!(rig.running(), Some(sapphire_core::Name::new("dodge")));

    let mut rig = Rig::fight();
    rig.touch(1, THUMB, TouchPhase::Down, rig.layout.skill.center);
    rig.touch(2, THUMB, TouchPhase::Up, rig.layout.skill.center);
    rig.frame();
    rig.step();
    assert_eq!(rig.running(), Some(sapphire_core::Name::new("skill")));
}

/// A finger the OS takes back, and a trip to the background, through the same routing: what the next step
/// reads is exactly centred, and letting go presses nothing.
#[test]
fn a_canceled_touch_and_the_background_let_go() {
    let window = WindowId::from_raw(1);
    let losses = [
        Event::Lifecycle { timestamp: ms(100), event: LifecycleEvent::WillEnterBackground },
        Event::Window { timestamp: ms(100), window, event: WindowEvent::FocusLost },
    ];
    for loss in &losses {
        let mut rig = Rig::fight();
        rig.push_stick(THUMB, Vec2::new(0.0, 1.0));
        rig.touch(3, OTHER_THUMB, TouchPhase::Down, rig.layout.attack.center);
        rig.frame();
        let read = rig.read();
        assert!(read.buttons.contains(sapphire_gameplay::Buttons::LIGHT) && read.stick != Stick::CENTER);

        rig.event(loss);
        rig.frame();
        let read = rig.read();
        assert_eq!((read.buttons, read.stick), (sapphire_gameplay::Buttons::NONE, Stick::CENTER), "{loss:?}");
    }

    let mut rig = Rig::fight();
    rig.push_stick(THUMB, Vec2::new(0.0, 1.0));
    rig.touch(3, OTHER_THUMB, TouchPhase::Down, rig.layout.attack.center);
    rig.frame();
    let _ = rig.read();
    rig.touch(1, OTHER_THUMB, TouchPhase::Canceled, rig.layout.attack.center);
    rig.frame();
    let read = rig.read();
    assert_eq!(read.buttons, sapphire_gameplay::Buttons::NONE, "the attack's finger was taken back");
    assert_ne!(read.stick, Stick::CENTER, "and the stick's wasn't");
}

/// A phone's only way out of the pause is a tap, and with the zones taking every landing no tap would
/// ever reach the bindings. A thumb that was down when the pause came still lifts through the zones.
#[test]
fn a_tap_ends_the_pause_and_a_thumb_held_through_it_still_lifts() {
    let mut rig = Rig::fight();
    rig.push_stick(THUMB, Vec2::new(0.0, 1.0));
    rig.frame();
    assert_ne!(rig.read().stick, Stick::CENTER);
    rig.back();
    assert!(!rig.frame());
    assert!(rig.input.is_paused());

    let rest = rig.layout.stick.rest;
    rig.touch(1, THUMB, TouchPhase::Up, rest);
    assert!(!rig.frame());
    assert!(rig.input.is_paused(), "that lift was the stick's, not a tap on the pause");

    // A tap anywhere, on a button too, is the pause's and presses nothing.
    let attack = rig.layout.attack.center;
    rig.touch(1, OTHER_THUMB, TouchPhase::Down, attack);
    rig.touch(5, OTHER_THUMB, TouchPhase::Up, attack);
    assert!(!rig.frame());
    assert!(!rig.frame());
    assert!(!rig.input.is_paused(), "the tap ended it");
    let read = rig.read();
    assert_eq!((read.buttons, read.stick), (sapphire_gameplay::Buttons::NONE, Stick::CENTER));

    // Back into it, and Back again leaves.
    rig.back();
    assert!(!rig.frame());
    assert!(rig.input.is_paused());
    rig.back();
    assert!(rig.frame(), "the second Back leaves");
}

/// A new fight is a new simulation, whose first tick has nothing before it, so to it whatever's held is a
/// press unless something says otherwise. A thumb held through the restart walks on, and a held attack
/// doesn't swing: it has to be let go and pressed again, as on the way back from the pause.
#[test]
fn a_held_attack_doesnt_swing_when_the_fight_starts_over() {
    let mut rig = Rig::fight();
    rig.push_stick(THUMB, Vec2::new(0.0, 1.0));
    rig.touch(3, OTHER_THUMB, TouchPhase::Down, rig.layout.attack.center);
    rig.frame();
    rig.step();
    assert_eq!(rig.running(), Some(sapphire_core::Name::new("light_1")));

    rig.combat.restart();
    rig.frame();
    rig.step();
    assert_eq!(rig.sim().tick(), 1, "a new fight");
    assert_eq!(rig.running(), None, "held across the restart, it isn't pressed again");
    assert_ne!(rig.read().stick, Stick::CENTER, "and the walk goes on");
    for _ in 0..5 {
        rig.frame();
        rig.step();
        assert_eq!(rig.running(), None, "still held");
    }

    // Let go and pressed again, it's a press.
    let attack = rig.layout.attack.center;
    rig.touch(1, OTHER_THUMB, TouchPhase::Up, attack);
    rig.frame();
    rig.step();
    assert_eq!(rig.running(), None);
    rig.touch(1, OTHER_THUMB, TouchPhase::Down, attack);
    rig.frame();
    rig.step();
    assert_eq!(rig.running(), Some(sapphire_core::Name::new("light_1")));
}

/// The frame after the tap that ended a pause, the steps are about to run and the combat context is back,
/// so a thumb landing then is the fight's. The pause no longer wants it.
#[test]
fn a_thumb_landing_as_the_pause_ends_is_the_fights() {
    let mut rig = Rig::fight();
    rig.back();
    assert!(!rig.frame());
    assert!(rig.input.is_paused());

    // The tap that ends it, in one frame.
    let middle = SIZE * Vec2::new(0.5, 0.4);
    rig.touch(1, OTHER_THUMB, TouchPhase::Down, middle);
    rig.touch(2, OTHER_THUMB, TouchPhase::Up, middle);
    assert!(!rig.frame());

    // Between that frame and the next, one thumb on the stick and another on the attack.
    rig.push_stick(THUMB, Vec2::new(0.0, 1.0));
    rig.touch(3, OTHER_THUMB, TouchPhase::Down, rig.layout.attack.center);
    assert!(!rig.frame());
    assert!(!rig.input.is_paused());
    let read = rig.read();
    assert!(read.buttons.contains(sapphire_gameplay::Buttons::LIGHT), "the attack was lost: {read:?}");
    assert_ne!(read.stick, Stick::CENTER, "the stick was lost: {read:?}");
}

/// While paused the zones' camera does nothing either: it isn't a fight's time, but it's the pause's.
#[test]
fn a_drag_turns_the_camera_through_the_bindings_and_not_in_the_pause() {
    let mut rig = Rig::fight();
    let middle = SIZE * Vec2::new(0.5, 0.4);
    rig.touch(1, THUMB, TouchPhase::Down, middle);
    rig.touch(2, THUMB, TouchPhase::Moved, middle + Vec2::new(20.0, 0.0));
    rig.frame();
    let (look, _) = rig.input.camera_motion();
    assert!((look.x + 0.12).abs() < 1e-5 && look.y == 0.0, "{look}");
    rig.frame();
    assert_eq!(rig.input.camera_motion().0, Vec2::ZERO, "a finger held still");

    rig.back();
    rig.frame();
    rig.touch(1, THUMB, TouchPhase::Moved, middle + Vec2::new(60.0, 0.0));
    rig.frame();
    assert_eq!(rig.input.camera_motion().0, Vec2::ZERO);
}

/// A poise break or a defeat publishes a second cue with the same hit, which must not land twice: the
/// numbers add up to what the bars lost.
#[test]
fn each_hit_counts_once() {
    let mut rig = Rig::fight();
    let mut taken = 0;
    let mut cues = 0;
    let mut followed = 0;
    for _ in 0..2000 {
        rig.frame();
        rig.step();
        for cue in rig.sim().cues() {
            cues += u32::from(cue.hit.is_some());
            match crate::hud::landed(&cue) {
                Some(hit) => taken += hit.damage,
                None => followed += u32::from(cue.hit.is_some()),
            }
        }
        if rig.player().fighter.is_defeated() {
            break;
        }
    }
    let fighter = rig.player().fighter;
    assert!(fighter.is_defeated(), "the warrior wins against someone standing still");
    assert!(
        followed > 0 && cues > followed,
        "a poise break or a defeat came with its hit: {followed} of {cues}"
    );
    assert!(taken >= fighter.max_health(), "{taken} of {}", fighter.max_health());
    // The last hit may do more than was left.
    assert!(taken < fighter.max_health() + 120, "{taken} of {}: a hit counted twice", fighter.max_health());
}

/// A session played by touch is a replay like any other: its file, played with nothing beside it, reaches
/// the checksum the app's run did. The emulator's run is this across an OS.
#[test]
fn a_session_played_by_touch_replays_to_its_checksum() {
    let dir = std::env::temp_dir().join(format!("sapphire_sandbox_touch_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let path: PathBuf = dir.join("touch.sreplay");
    let _ = std::fs::remove_file(&path);
    let option = CombatOption::Record {
        path: path.clone(),
        scenario: crate::arena::FIGHT_SCENARIO.to_owned(),
        script: None,
    };
    let mut rig = Rig::new(Combat::new(&option).expect("sets up"));
    rig.turn_camera(0.6);

    rig.push_stick(THUMB, Vec2::new(0.4, 0.9));
    for frame in 0..600_u32 {
        match frame % 40 {
            5 => rig.touch(1, OTHER_THUMB, TouchPhase::Down, rig.layout.attack.center),
            7 => rig.touch(1, OTHER_THUMB, TouchPhase::Up, rig.layout.attack.center),
            25 => {
                rig.touch(1, OTHER_THUMB, TouchPhase::Down, rig.layout.dodge.center);
                rig.touch(2, OTHER_THUMB, TouchPhase::Up, rig.layout.dodge.center);
            }
            _ => {}
        }
        rig.frame();
        rig.step();
    }
    let (ended, ticks) = (rig.sim().checksum(), rig.sim().tick());
    assert!(ticks >= 600);
    let started = rig.player().moves.started;
    assert!(started > 5, "it fought: {started} moves");

    rig.combat.finish().expect("written");
    let bytes = std::fs::read(&path).expect("written");
    let mut player = Player::new(Replay::decode(&bytes).expect("reads"), SCENARIOS).expect("starts");
    player.run_to_end();
    let played = player.finish();
    assert_eq!(played.trail.end(), ended, "the file plays to where the app got");
    let _ = std::fs::remove_dir_all(&dir);
}
