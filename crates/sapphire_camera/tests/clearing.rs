//! Turning the view round what's in its way: what it does for the view, when it leaves it alone, and who
//! it gives way to. Every scene runs against the same thing with clearing off, so each test shows it's
//! clearing that made the difference.
//!
//! `sapphire_stage`'s `arena::framing` counts what it comes to over whole fights in the real arena.

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

/// With who it's framing right behind it, across a wall, going over to them is a half turn, and it's as soft
/// as any other: nearly opposite, a straight line between the two ways crosses it all in a frame or two.
/// Exactly opposite, which way round is up to a float's last bits, and it stays whichever it took.
#[test]
fn an_eye_shut_in_goes_over_to_someone_behind_it_evenly() {
    let world =
        ground_with(&[(Shape::Box { half_extents: Vec3::new(4.0, 2.5, 0.2) }, Vec3::new(0.0, 2.5, -1.0))]);
    for across in [0.0, 0.05, 0.2, 0.5, -0.05, -0.2] {
        let enemy = Vec3::new(across, 0.0, -2.5);
        let mut camera = without_clearing(combat_director());
        // Facing away from where they'll be, on the first frame.
        camera.update(&Moment::alone(PLAYER).frame(1.0 / 60.0), &world);
        let mut last = camera.pose();
        let mut most = 0.0_f32;
        for _ in 0..600 {
            let pose = camera.update(&Moment::against(PLAYER, enemy).frame(1.0 / 60.0), &world);
            most = most.max(pose.rotation.angle_between(last.rotation));
            last = pose;
        }
        assert!(most < 0.2, "with the enemy {across} m across, {most} rad in a frame");
        let to = (enemy + Vec3::Y * 1.6 - last.eye).normalize();
        assert!(last.forward().dot(to) > 0.9, "it should have got there: {} and {to}", last.forward());
    }
}

/// Someone passing right behind a shut-in eye, and back. The short way round changes sides as they go by, and
/// if the view changed sides with it, part of the way over to them, it would flip in a frame, and the stick
/// with it, however soft everything else is. Walls at a few distances, so it's part of the way at some of them.
#[test]
fn an_eye_shut_in_keeps_its_way_round_as_someone_passes_behind_it() {
    for wall in [1.0, 1.2, 1.4, 1.6, 1.8] {
        let world = ground_with(&[(
            Shape::Box { half_extents: Vec3::new(8.0, 2.5, 0.2) },
            Vec3::new(0.0, 2.5, -wall),
        )]);
        let script = |t: f32| Moment::against(PLAYER, Vec3::new(1.5 * (t * 0.8).sin(), 0.0, -3.0 - wall));
        let mut camera = without_clearing(combat_director());
        let mut last = settle(&mut camera, &world, 1.0, script);
        let mut most = 0.0_f32;
        run(&mut camera, &world, 60, (1.0, 21.0), script, |_, pose, _| {
            most = most.max(pose.rotation.angle_between(last.rotation));
            last = *pose;
        });
        assert!(most < 0.2, "with the wall {wall} m behind, {most} rad in a frame");
    }
}

/// Round and round a shut-in eye that sees only them, and then the eye gets room: the view comes back by the
/// short way, however many times they went round. A turn left long would swing it right round as it let go.
#[test]
fn an_eye_given_room_comes_back_the_short_way() {
    let world =
        ground_with(&[(Shape::Box { half_extents: Vec3::new(8.0, 2.5, 0.2) }, Vec3::new(0.0, 2.5, -1.0))]);
    let script = |t: f32| {
        let player = PLAYER + Vec3::Z * (t - 10.0).max(0.0) * 2.0;
        let (sin, cos) = (t * 1.5).sin_cos();
        Moment::against(player, player + Vec3::new(sin, 0.0, cos) * 3.0)
    };
    let mut camera = without_clearing(combat_director());
    let mut last = settle(&mut camera, &world, 1.0, script);
    let mut most = 0.0_f32;
    run(&mut camera, &world, 60, (1.0, 16.0), script, |_, pose, _| {
        most = most.max(pose.rotation.angle_between(last.rotation));
        last = *pose;
    });
    assert!(most < 0.2, "{most} rad in a frame");
}

/// A hitch has no view to judge, and whatever clearing was doing is over. A thumb isn't over: what it's asked
/// for holds through one, counting the hitch's time, and a thumb on the view during one counts too.
#[test]
fn a_long_frame_doesnt_forget_a_thumb() {
    let world = wall_behind();
    let long = 0.3;
    let after_look = combat_director().tuning().clearing.after_look;
    let thumb = |t: f32| Moment {
        look_rate: if t < 3.0 { Vec2::new(0.01 * (t * 3.0).sin(), 0.0) } else { Vec2::ZERO },
        ..facing_the_enemy(t)
    };
    for thumb_in_the_hitch in [false, true] {
        let mut camera = combat_director();
        run(&mut camera, &world, 60, (0.0, 3.0), thumb, |_, _, _| {});
        let mut hitch = facing_the_enemy(3.0).frame(long);
        if thumb_in_the_hitch {
            hitch.look = Vec2::new(0.01, 0.0);
        }
        camera.update(&hitch, &world);
        // The hitch counts for a thumb that let go before it, and not for one still on the view during it.
        let from = if thumb_in_the_hitch { 3.0 + long } else { 3.0 };
        let until = from + after_look - 0.1;
        run(&mut camera, &world, 60, (3.0 + long, until), thumb, |_, _, _| {});
        assert_eq!(camera.take_stats().swings, 0, "nothing's chosen just after: {thumb_in_the_hitch}");
        run(&mut camera, &world, 60, (until, until + 3.0), thumb, |_, _, _| {});
        assert!(camera.take_stats().swings >= 1, "and then it's free to: {thumb_in_the_hitch}");
    }
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

/// Choices that go on being made with nothing moving: the view hunting, or making a choice of nothing.
fn choices_once_settled(mut camera: CameraDirector, world: &PhysicsWorld, enemy: Vec3) -> u32 {
    let script = move |_: f32| Moment::against(PLAYER, enemy);
    settle(&mut camera, world, 20.0, script);
    camera.take_stats();
    settle(&mut camera, world, 20.0, script);
    camera.take_stats().swings
}

/// A lean as far as `reach` goes has nothing further to choose. It used to choose it anyway, every `settle`,
/// each time counting as a turn and changing nothing.
#[test]
fn a_lean_at_its_reach_chooses_nothing_further() {
    let world = ground_with(&[
        (Shape::Box { half_extents: Vec3::new(1.59, 2.76, 1.55) }, Vec3::new(2.41, 2.76, -1.1)),
        (Shape::Box { half_extents: Vec3::new(3.38, 2.84, 2.86) }, Vec3::new(-3.22, 2.84, 2.62)),
        (Shape::Box { half_extents: Vec3::new(2.68, 1.29, 1.43) }, Vec3::new(-8.03, 1.29, 2.89)),
    ]);
    let swings = choices_once_settled(lock_on_director(), &world, Vec3::new(-3.0, 0.0, 12.08));
    assert!(swings <= 1, "{swings} choices in 20 s, with nothing moving");
}

/// A corner of a box on the line, thin enough that the arm stays out behind it. From there the line's trouble,
/// so the view leans out, and from out there the line looks clear, so it goes back. Coming back has to take
/// more than leaving did, or it does that for as long as the fight stands there.
#[test]
fn a_lean_doesnt_take_turns_with_the_line() {
    let world = ground_with(&[(
        Shape::Box { half_extents: Vec3::new(2.85, 2.66, 1.02) },
        Vec3::new(4.92, 2.66, 2.9),
    )]);
    let swings = choices_once_settled(lock_on_director(), &world, Vec3::new(-1.12, 0.0, -23.22));
    assert!(swings <= 1, "{swings} choices in 20 s, with nothing moving");
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

/// The same with frames longer than a check: slow frames, and a document's short `check_every`. Patience and
/// settling count the time that went by, not the checks that were made.
#[test]
fn it_counts_the_time_that_went_by_not_its_checks() {
    let world = wall_behind();
    let first_choice = |check_every: f32, rate: u32| {
        let mut camera = combat_director();
        let mut tuning = camera.tuning().clone();
        tuning.clearing.check_every = check_every;
        tuning.clearing.patience = 0.6;
        camera.set_tuning(tuning).expect("it's in range");
        let (dt, moment) = (1.0 / rate as f32, facing_the_enemy(0.0));
        for frame in 1..=(rate * 10) {
            camera.update(&moment.frame(dt), &world);
            if camera.take_stats().swings > 0 {
                return (frame - 1) as f32 * dt;
            }
        }
        panic!("no choice in 10 s, checking every {check_every} s at {rate} Hz");
    };
    // At 5 Hz it's two checks' time a frame, and at 30 Hz, for checks of 0.01 s, more than three.
    for (check_every, rate) in [(0.1, 5), (0.1, 6), (0.01, 30), (0.01, 60), (0.02, 30)] {
        let at = first_choice(check_every, rate);
        assert!(
            (0.6..0.6 + 1.5 / rate as f32).contains(&at),
            "at {at}, checking every {check_every} s at {rate} Hz"
        );
    }
}

/// A far enemy gives the rig a tight limit, and what clearing first chooses here is past it, so the rig holds
/// the view short of it, with the eye shut in by the wall. Waiting for that goal would be waiting for good:
/// it has to choose again, and there's a good view the other way.
#[test]
fn a_view_held_short_of_what_it_chose_chooses_again() {
    let world = ground_with(&[
        (Shape::Box { half_extents: Vec3::new(1.98, 2.36, 1.56) }, Vec3::new(-3.3, 2.36, 2.93)),
        (Shape::Box { half_extents: Vec3::new(0.69, 2.22, 0.3) }, Vec3::new(-1.19, 2.22, -0.41)),
        (Shape::Box { half_extents: Vec3::new(4.28, 1.99, 1.04) }, Vec3::new(6.36, 1.99, 2.06)),
    ]);
    let script = |_: f32| Moment::against(PLAYER, Vec3::new(17.94, 0.0, -6.43));
    let mut camera = combat_director();
    settle(&mut camera, &world, 20.0, script);
    let pose = camera.pose();
    assert!(pose.eye.distance(CHEST) > 2.5, "still shut in after 20 s: {pose:?}");
    camera.take_stats();
    settle(&mut camera, &world, 20.0, script);
    let swings = camera.take_stats().swings;
    assert!(swings <= 1, "{swings} more choices with nothing moving");
    assert!(camera.pose().eye.distance(CHEST) > 2.5, "{:?}", camera.pose());
}

/// Boxed in, with a far enemy: the rig holds the view short of a goal either way. Choosing the other way each
/// time would have it swinging across the player's chest and back, every couple of seconds, for good. It
/// tries again when `HELD_FOR` is up, so this isn't none.
#[test]
fn held_short_both_ways_it_doesnt_take_turns() {
    let world = ground_with(&[
        (Shape::Box { half_extents: Vec3::new(4.95, 1.05, 0.46) }, Vec3::new(-4.86, 1.05, -9.31)),
        (Shape::Box { half_extents: Vec3::new(3.28, 1.26, 2.44) }, Vec3::new(-3.73, 1.26, -1.08)),
        (Shape::Box { half_extents: Vec3::new(3.3, 1.1, 2.97) }, Vec3::new(-0.84, 1.1, 7.2)),
        (Shape::Box { half_extents: Vec3::new(2.73, 2.24, 2.35) }, Vec3::new(-3.35, 2.24, -1.21)),
    ]);
    let swings = choices_once_settled(combat_director(), &world, Vec3::new(18.95, 0.0, -1.27));
    assert!(swings <= 6, "{swings} choices in 20 s, with nothing moving");
}

/// A view at rest that's come as far as it's let, with nothing wrong with it, isn't left being pulled on:
/// what the view does next is the rig's alone, however long it's been since that goal was chosen.
#[test]
fn a_goal_held_short_of_ends_once_nothing_is_wrong() {
    let world = wall_behind();
    // Walks off the wall and round to where the rig keeps the view well short of what was chosen for it.
    let script = |t: f32| {
        let walk = t.clamp(0.0, 1.0);
        let away = Vec3::new(0.0, 0.0, 6.0) * walk;
        let enemy = Vec3::new(0.479, 0.0, 0.878) * 8.0 * walk + ENEMY * (1.0 - walk);
        Moment::against(away, away + enemy)
    };
    let mut camera = combat_director();
    for _ in 0..180 {
        camera.update(&facing_the_enemy(0.0).frame(1.0 / 60.0), &world);
        if camera.take_stats().swings > 0 {
            break;
        }
    }
    settle(&mut camera, &world, 20.0, script);
    let enemy = script(20.0).target.expect("there's an enemy");
    // Held on the rig's limit, the enemy's on the safe frame's edge, and the view stands turned. Let go, a look
    // at the view pulled nowhere shows the two the same: moving a hair of the way on the rig's own.
    let used = frame_use(&camera.pose(), &enemy).x;
    let next = camera.update(&script(20.0).frame(1.0 / 60.0), &world);
    let moved = next.forward().angle_between(camera.pose().forward());
    assert!(moved < 1e-3, "still turning by {moved} a frame, with the enemy at {used} of the frame");
    let free = camera.heading();
    let mut touched = camera.clone();
    let touch = Moment { look_rate: Vec2::new(1e-4, 0.0), ..script(20.0) };
    touched.update(&touch.frame(1.0 / 60.0), &world);
    settle(&mut touched, &world, 5.0, script);
    settle(&mut camera, &world, 5.0, script);
    let apart = sapphire_math::det::wrap_angle(camera.heading() - touched.heading()).abs();
    assert!(apart < 0.01, "{free}: a thumb's touch moved it by {apart}, so a goal was still pulling");
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
