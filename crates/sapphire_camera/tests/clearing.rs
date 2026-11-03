//! Turning the view round what's in its way: what it does for the view, when it leaves it alone, and who
//! it gives way to. Every scene runs against the same thing with clearing off, so each test shows it's
//! clearing that made the difference.
//!
//! `sapphire_sandbox`'s `arena::framing` counts what it comes to over whole fights in the real arena.

mod common;

use common::{
    Moment, combat_director, enemy_at, frame_use, ground_with, lock_on_director, open_ground, player_at, run,
    without_clearing,
};
use sapphire_camera::{CameraDirector, CameraPose, Rig};
use sapphire_math::{Vec2, Vec3};
use sapphire_physics::{PhysicsWorld, Shape};

const PLAYER: Vec3 = Vec3::ZERO;
const ENEMY: Vec3 = Vec3::new(0.0, 0.0, 5.0);
/// The chest, where the arm starts.
const CHEST: Vec3 = Vec3::new(0.0, 1.3, 0.0);

/// A wall a stall's size just behind the player, as the view stands to frame an enemy at [`ENEMY`].
fn wall_behind() -> PhysicsWorld {
    ground_with(&[(Shape::Box { half_extents: Vec3::new(3.0, 2.5, 0.8) }, Vec3::new(0.0, 2.5, -1.8))])
}

fn facing_the_enemy(_: f32) -> Moment {
    Moment::against(PLAYER, ENEMY)
}

/// Both fighters inside the frame, head and feet.
fn both_framed(pose: &CameraPose, moment: &Moment) -> bool {
    let inside = |subject| {
        let used = frame_use(pose, subject);
        used.x < 1.0 && used.y < 1.0
    };
    inside(&moment.player) && moment.target.as_ref().is_some_and(inside)
}

/// Runs `seconds` of `script` at 60 Hz and gives back the last pose.
fn settle(
    camera: &mut CameraDirector,
    world: &PhysicsWorld,
    seconds: f32,
    script: impl Fn(f32) -> Moment,
) -> CameraPose {
    let mut last = camera.pose();
    run(camera, world, 60, (0.0, seconds), script, |_, pose, _| last = *pose);
    last
}

/// With a wall right behind the player the arm has nowhere to stand: the eye ends up on their chest with
/// neither fighter in the frame. Turned, it has room, and both are.
#[test]
fn a_wall_behind_the_player_turns_the_view_until_the_arm_has_room() {
    let world = wall_behind();
    let moment = facing_the_enemy(0.0);

    let mut stuck = without_clearing(combat_director());
    let pose = settle(&mut stuck, &world, 5.0, facing_the_enemy);
    assert!(pose.eye.distance(CHEST) < 1.5, "off, it should be shut in: {pose:?}");
    assert_eq!(stuck.take_stats().swings, 0);

    let mut camera = combat_director();
    let pose = settle(&mut camera, &world, 5.0, facing_the_enemy);
    assert!(pose.eye.distance(CHEST) > 2.5, "{pose:?}");
    assert!(both_framed(&pose, &moment), "{pose:?}");
    let swings = camera.take_stats().swings;
    assert!((1..=3).contains(&swings), "{swings} choices for one wall");
}

/// An eye shut in by the player's chest sees nothing where its rig looks, so it looks at who it's framing
/// until it has room. With clearing off nothing gives it any, so this holds.
#[test]
fn an_eye_shut_in_looks_at_who_its_framing() {
    let world = wall_behind();
    let mut camera = without_clearing(combat_director());
    let pose = settle(&mut camera, &world, 5.0, facing_the_enemy);
    assert!(pose.eye.distance(CHEST) < 1.0, "it should be shut in: {pose:?}");
    let chest = ENEMY + Vec3::Y * 1.3;
    let (forward, to) = (pose.forward(), (chest - pose.eye).normalize());
    assert!(forward.dot(to) > 0.95, "looking along {forward}, with the enemy along {to}");
}

/// Having turned, it stays: a settled good view isn't re-judged into another.
#[test]
fn a_good_view_stays_where_it_is() {
    let world = wall_behind();
    let mut camera = combat_director();
    settle(&mut camera, &world, 5.0, facing_the_enemy);
    camera.take_stats();
    let heading = camera.heading();
    settle(&mut camera, &world, 20.0, facing_the_enemy);
    assert_eq!(camera.take_stats().swings, 0);
    assert!((camera.heading() - heading).abs() < 0.05, "{} to {}", heading, camera.heading());
}

/// On open ground with the fight ahead of the view there's nothing to clear: it never turns, however the
/// fighters stand or walk.
#[test]
fn open_ground_never_turns() {
    let world = open_ground();
    for degrees in [0.0_f32, 30.0, -30.0, 60.0, -60.0] {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let enemy = Vec3::new(sin, 0.0, cos) * 4.0;
        let mut camera = combat_director();
        let script = |t: f32| {
            let along = Vec3::new(2.0 * (t * 0.4).sin(), 0.0, 3.0 * (t * 0.3).cos());
            Moment::against(PLAYER + along, enemy + along)
        };
        settle(&mut camera, &world, 30.0, script);
        assert_eq!(camera.take_stats().swings, 0, "with the enemy {degrees} degrees off");
    }
}

/// An enemy standing between the camera and the player: the fault that reads the worst, with nothing in
/// the world to blame.
#[test]
fn an_enemy_between_the_camera_and_the_player_turns_the_view() {
    let world = open_ground();
    // The view starts facing +Z, so the enemy's between it and the player.
    let script = |_: f32| Moment::against(PLAYER, Vec3::new(0.0, 0.0, -3.0));
    let nearer_by = |pose: &CameraPose| {
        let forward = pose.forward();
        (player_at(PLAYER).position - pose.eye).dot(forward)
            - (enemy_at(Vec3::new(0.0, 0.0, -3.0)).position - pose.eye).dot(forward)
    };

    let mut camera = combat_director();
    let pose = settle(&mut camera, &world, 5.0, script);
    assert!(nearer_by(&pose) < 1.5, "the enemy's still in front by {} m: {pose:?}", nearer_by(&pose));
    assert!(camera.take_stats().swings >= 1);
}

/// A thumb on the view wins: nothing's chosen under it or for `after_look` after, and a turn on its way
/// stops.
#[test]
fn look_input_takes_the_view_over() {
    let world = wall_behind();
    let mut camera = combat_director();
    let after_look = camera.tuning().clearing.after_look;
    let thumb = |t: f32| Moment {
        look_rate: if t < 3.0 { Vec2::new(0.01 * (t * 3.0).sin(), 0.0) } else { Vec2::ZERO },
        ..facing_the_enemy(t)
    };
    run(&mut camera, &world, 60, (0.0, 3.0 + after_look - 0.1), thumb, |_, _, _| {});
    assert_eq!(camera.take_stats().swings, 0, "nothing's chosen under a thumb or just after");
    run(&mut camera, &world, 60, (3.0 + after_look - 0.1, 6.0 + after_look), thumb, |_, _, _| {});
    assert!(camera.take_stats().swings >= 1, "and once it's let go, it's free to");

    // A turn on its way stops where the thumb found it.
    let chosen = |camera: &mut CameraDirector| {
        for _ in 0..180 {
            camera.update(&facing_the_enemy(0.0).frame(1.0 / 60.0), &world);
            if camera.take_stats().swings > 0 {
                return;
            }
        }
        panic!("no choice in 3 s");
    };
    let travel = |thumb: bool| {
        let mut camera = combat_director();
        chosen(&mut camera);
        let from = camera.heading();
        let mut touch = Moment { look_rate: Vec2::new(1e-3, 0.0), ..facing_the_enemy(0.0) };
        if !thumb {
            touch.look_rate = Vec2::ZERO;
        }
        camera.update(&touch.frame(1.0 / 60.0), &world);
        settle(&mut camera, &world, 1.0, facing_the_enemy);
        sapphire_math::det::wrap_angle(camera.heading() - from).abs()
    };
    let (left_alone, touched) = (travel(false), travel(true));
    assert!(left_alone > 0.3, "the turn should have gone on: {left_alone}");
    assert!(touched < left_alone * 0.25, "{touched} against {left_alone}");
}

/// Under lock-on the line's held, so a choice is a lean off it, and once there's room on the line itself,
/// the lean goes.
#[test]
fn lock_on_leans_off_its_line_and_comes_back() {
    let world = wall_behind();
    let script = |t: f32| {
        // Stands by the wall, then walks off it into the open.
        let away = Vec3::new(0.0, 0.0, 6.0 * ((t - 5.0) / 3.0).clamp(0.0, 1.0));
        Moment::against(PLAYER + away, ENEMY + away)
    };
    // What it does with nothing in the way: the line, as the rigs hold it.
    let mut free = lock_on_director();
    let open = open_ground();
    let at = |camera: &mut CameraDirector, world, from, to| {
        run(camera, world, 60, (from, to), script, |_, _, _| {});
        camera.heading()
    };
    let line = at(&mut free, &open, 0.0, 5.0);

    let mut held = without_clearing(lock_on_director());
    at(&mut held, &world, 0.0, 5.0);
    assert!(held.pose().eye.distance(CHEST) < 2.5, "off, it stays in: {:?}", held.pose());

    let mut camera = lock_on_director();
    let leant = at(&mut camera, &world, 0.0, 5.0);
    assert!(camera.take_stats().swings >= 1);
    assert!((leant - line).abs() > 0.1, "it should lean off the line: {leant} from {line}");
    assert!(camera.pose().eye.distance(CHEST) > 2.5, "{:?}", camera.pose());
    assert!(both_framed(&camera.pose(), &script(5.0)), "{:?}", camera.pose());

    // Off the wall, the lean comes back to the line.
    let (line, back) = (at(&mut free, &open, 5.0, 14.0), at(&mut camera, &world, 5.0, 14.0));
    assert!((back - line).abs() < 0.05, "the lean should be gone: {back} from {line}");
}

/// A pause holds everything, a turn on its way included, and costs nothing.
#[test]
fn a_pause_holds_a_turn_on_its_way() {
    let world = wall_behind();
    let mut camera = combat_director();
    settle(&mut camera, &world, 0.7, facing_the_enemy);
    assert!(camera.take_stats().swings >= 1, "the turn should be on its way");

    let (pose, heading) = (camera.pose(), camera.heading());
    let moment = facing_the_enemy(0.0);
    for _ in 0..120 {
        assert_eq!(camera.update(&moment.frame(0.0), &world), pose);
        assert_eq!(camera.heading(), heading);
    }
    let stats = camera.take_stats();
    assert_eq!((stats.casts, stats.swings), (0, 0), "{stats:?}");

    // And it carries on from there with no jump.
    let next = camera.update(&moment.frame(1.0 / 60.0), &world);
    assert!(next.eye.distance(pose.eye) < 0.3, "{} to {}", pose.eye, next.eye);
}

/// `heading()` never carries a shake, with the view turning round a wall and a shake at every step.
#[test]
fn a_shake_doesnt_reach_what_it_chooses() {
    let world = wall_behind();
    let (mut quiet, mut shaken) = (combat_director(), combat_director());
    let id = shaken.tuning().shake_id("finish").expect("a shipped preset");
    for frame in 1..=600 {
        if frame % 10 == 0 {
            shaken.fire_shake(id, 1.0);
        }
        let moment = facing_the_enemy(0.0);
        quiet.update(&moment.frame(1.0 / 60.0), &world);
        shaken.update(&moment.frame(1.0 / 60.0), &world);
        assert_eq!(quiet.heading().to_bits(), shaken.heading().to_bits(), "at frame {frame}");
    }
    assert!(quiet.take_stats().swings >= 1);
}

/// Each rate chooses at the same moments, counting from the cut, so a phone at 30 Hz turns when a desktop
/// at 60 does.
#[test]
fn it_chooses_at_the_same_moment_at_any_rate() {
    let world = wall_behind();
    let at = |rate: u32| {
        let mut camera = combat_director();
        let (dt, moment) = (1.0 / rate as f32, facing_the_enemy(0.0));
        for frame in 1..=(rate * 3) {
            camera.update(&moment.frame(dt), &world);
            if camera.take_stats().swings > 0 {
                return (frame - 1) as f32 * dt;
            }
        }
        panic!("no choice in 3 s at {rate} Hz");
    };
    let (slow, middle, fast) = (at(30), at(60), at(120));
    assert!((slow - fast).abs() < 1e-3 && (middle - fast).abs() < 1e-3, "{slow}, {middle}, {fast}");
}

/// A reach of 0 turns all of it off, and the framing rigs never ask for it when they're off.
#[test]
fn it_only_runs_under_a_framing_rig() {
    let world = wall_behind();
    let mut explore = common::director();
    settle(&mut explore, &world, 5.0, facing_the_enemy);
    assert_eq!(explore.take_stats().swings, 0);

    let mut camera = combat_director();
    settle(&mut camera, &world, 3.0, facing_the_enemy);
    assert!(camera.take_stats().swings >= 1);
    // No target, so nothing to frame: whatever it was doing is over.
    camera.set_rig(Rig::Combat, false);
    settle(&mut camera, &world, 3.0, |_| Moment::alone(PLAYER));
    assert_eq!(camera.take_stats().swings, 0);
}

/// `reach: 0` saved with a turn on its way: the turn stops there, as under a thumb, rather than going on to
/// what it chose.
#[test]
fn a_reach_of_0_ends_a_turn_on_its_way() {
    let world = wall_behind();
    let mut base = combat_director();
    for _ in 0..180 {
        base.update(&facing_the_enemy(0.0).frame(1.0 / 60.0), &world);
        if base.take_stats().swings > 0 {
            break;
        }
    }
    let travel = |mut camera: CameraDirector| {
        let from = camera.heading();
        settle(&mut camera, &world, 3.0, facing_the_enemy);
        sapphire_math::det::wrap_angle(camera.heading() - from).abs()
    };
    let off = without_clearing(base.clone());
    let (left_alone, stopped) = (travel(base), travel(off));
    assert!(left_alone > 0.3, "the turn should have gone on: {left_alone}");
    assert!(stopped < left_alone * 0.25, "{stopped} against {left_alone}");
}

/// `reach: 0` saved with a lean on: the lean comes back to the line, softly and for good.
#[test]
fn a_reach_of_0_brings_a_lean_back() {
    let world = wall_behind();
    let mut leant = lock_on_director();
    settle(&mut leant, &world, 5.0, facing_the_enemy);
    assert!(leant.take_stats().swings >= 1);
    let mut never = without_clearing(lock_on_director());
    settle(&mut never, &world, 5.0, facing_the_enemy);
    assert!((leant.heading() - never.heading()).abs() > 0.1, "it should be leaning");

    let mut leant = without_clearing(leant);
    let before = leant.pose();
    let next = leant.update(&facing_the_enemy(0.0).frame(1.0 / 60.0), &world);
    assert!(next.eye.distance(before.eye) < 0.3, "{} to {}", before.eye, next.eye);
    settle(&mut leant, &world, 10.0, facing_the_enemy);
    settle(&mut never, &world, 10.0, facing_the_enemy);
    assert!((leant.heading() - never.heading()).abs() < 0.05, "{} from {}", leant.heading(), never.heading());
    assert_eq!(leant.take_stats().swings, 0);
}
