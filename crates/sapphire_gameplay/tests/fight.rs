//! M2 G3: what a hit does, through real steps. Distances and ticks are worked out by hand in each test's
//! comments, from [`FIGHT`]'s numbers. World space is metres, Y up, with a root's yaw turning its +Z
//! toward +X.

use std::f32::consts::{FRAC_PI_2, PI};

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::replay;
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, ActorTransform, Buttons, CharacterWorld, CombatSet, ControllerProfile,
    CueEvent, DEFEATED, Faction, Fighter, HitOutcome, HurtboxId, MAX_HURTBOXES, POISE_BROKEN, SimConfig,
    Simulation, SlowMotion, Stick, TickActions,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::{HIT_LANDED, MoveSet, MoveSetDocument, PathSample, Weight};
use sapphire_physics::{
    BodyDesc, BodyId, ColliderId, CollisionGroups, Group, GroupMask, PhysicsWorld, Pose, Shape,
    StaticCollider, StaticShape,
};
use sapphire_time::{ScaleSource, SimDuration, SimTime};

/// A blade 0.6 m in front of the root at chest height, with a radius of 0.2: its surface reaches z = 0.8.
/// A target 1 m away has its chest's surface at z = 0.75, so a window opening on it touches at once, with
/// no travel. Every attack below opens on tick 2, so a press on step `k` lands on step `k + 2`.
const FIGHT: &str = r#"(
    type: "sapphire::moves::MoveSetDocument",
    version: 1,
    data: (
        intents: [
            (name: "poke", button: Light, buffer: 8),
            (name: "bash", button: Heavy, buffer: 8),
            (name: "brace", button: Skill, buffer: 8),
            (name: "roll", button: Dodge, buffer: 8),
            (name: "spin", button: Ultimate, buffer: 8),
            (name: "slam", button: Swap, buffer: 8),
            (name: "lunge", button: Jump, buffer: 8),
            (name: "finish", button: LockOn, buffer: 8),
        ],
        neutral: [
            (intent: "poke", move: "poke"),
            (intent: "bash", move: "bash"),
            (intent: "brace", move: "brace"),
            (intent: "roll", move: "roll"),
            (intent: "spin", move: "spin"),
            (intent: "slam", move: "slam"),
            (intent: "lunge", move: "lunge"),
            (intent: "finish", move: "finish"),
        ],
        fighter: (
            health: 100,
            poise: 30,
            poise_refill: 60,
            hurtboxes: [(name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25))],
            reactions: (light: "flinch", medium: "stagger", heavy: "down", poise_break: "broken", defeat: "defeat"),
        ),
        moves: [
            (
                // Its root goes 0.4 m over ticks 2 to 6, so a hit on tick 2 has ground left to cover.
                name: "poke",
                startup: 2,
                active: 4,
                recovery: 14,
                motion: (
                    root: [(at: 2, offset: (0.0, 0.0, 0.0)), (at: 6, offset: (0.0, 0.0, 0.4))],
                    paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])],
                ),
                hitboxes: [(
                    name: "poke_hit",
                    from: 2,
                    to: 6,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 10, poise: 5, weight: Light, hitstop: 6, knockback: (distance: 0.5, ticks: 10)),
                )],
                cancels: [(from: 3, to: 20, allow: [(intent: "poke", move: "bash")])],
            ),
            (
                name: "bash",
                startup: 2,
                active: 4,
                recovery: 14,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "bash_hit",
                    from: 2,
                    to: 6,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 25, poise: 12, weight: Medium, hitstop: 8, knockback: (distance: 1.0, ticks: 10)),
                )],
            ),
            (
                name: "slam",
                startup: 2,
                active: 4,
                recovery: 14,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "slam_hit",
                    from: 2,
                    to: 6,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (
                        damage: 40,
                        poise: 15,
                        weight: Heavy,
                        hitstop: 12,
                        knockback: (distance: 2.0, ticks: 16),
                        cue: "slam_hit",
                    ),
                )],
            ),
            (
                name: "finish",
                startup: 2,
                active: 4,
                recovery: 14,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "finish_hit",
                    from: 2,
                    to: 6,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 1000, weight: Heavy, hitstop: 10, finish_freeze: 30),
                )],
            ),
            (
                // Held on the target through its window, re-arming twice: three hits, on ticks 2, 8 and 14.
                name: "spin",
                startup: 2,
                active: 18,
                recovery: 4,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(
                    name: "spin_hit",
                    from: 2,
                    to: 20,
                    path: "blade",
                    shape: Sphere(radius: 0.2),
                    attack: (damage: 3, poise: 1, weight: Light, hitstop: 2),
                    rearm: [8, 14],
                )],
            ),
            (
                // A small volume from the root to 2 m ahead within tick 2: how far it has gone when it
                // touches says who's nearer.
                name: "lunge",
                startup: 2,
                active: 2,
                recovery: 6,
                motion: (
                    paths: [(
                        name: "blade",
                        keys: [(at: 2, offset: (0.0, 1.0, 0.0)), (at: 3, offset: (0.0, 1.0, 2.0))],
                    )],
                ),
                hitboxes: [(
                    name: "lunge_hit",
                    from: 2,
                    to: 4,
                    path: "blade",
                    shape: Sphere(radius: 0.1),
                    attack: (damage: 7, poise: 20, weight: Light, hitstop: 3, knockback: (distance: 0.25, ticks: 5)),
                )],
            ),
            (
                // As `lunge`, stopped by the first wall it meets.
                name: "thrust",
                startup: 2,
                active: 2,
                recovery: 6,
                motion: (
                    paths: [(
                        name: "blade",
                        keys: [(at: 2, offset: (0.0, 1.0, 0.0)), (at: 3, offset: (0.0, 1.0, 2.0))],
                    )],
                ),
                hitboxes: [(
                    name: "thrust_hit",
                    from: 2,
                    to: 4,
                    path: "blade",
                    shape: Sphere(radius: 0.1),
                    attack: (damage: 7),
                    blocked_by: [World],
                )],
            ),
            (
                // Touches like `bash`, and does nothing about it.
                name: "sensor",
                startup: 2,
                active: 4,
                recovery: 14,
                motion: (paths: [(name: "blade", keys: [(at: 0, offset: (0.0, 1.0, 0.6))])]),
                hitboxes: [(name: "sensor_hit", from: 2, to: 6, path: "blade", shape: Sphere(radius: 0.2))],
            ),
            (
                // Armoured against a hit taking under 6 poise, for its first 30 ticks: `poke` is, `bash` isn't.
                name: "brace",
                startup: 0,
                active: 30,
                recovery: 10,
                armour: [(from: 0, to: 30, threshold: 6)],
            ),
            (
                // The same, at exactly `poke`'s poise, which isn't below it.
                name: "guard",
                startup: 0,
                active: 30,
                recovery: 10,
                armour: [(from: 0, to: 30, threshold: 5)],
            ),
            (
                name: "roll",
                startup: 0,
                active: 10,
                recovery: 5,
                motion: (root: [(at: 10, offset: (0.0, 0.0, -1.0))]),
                invulnerable: [(from: 0, to: 10)],
            ),
            (
                // Its hurtbox is smaller on ticks 1 to 3, gone on ticks 4 to 5, and its own after.
                name: "shrink",
                startup: 0,
                active: 10,
                recovery: 0,
                hurtboxes: [
                    (name: "body", from: 1, to: 4, state: Shape(Sphere(radius: 0.1))),
                    (name: "body", from: 4, to: 6, state: Off),
                ],
            ),
            (
                name: "flinch",
                startup: 0,
                active: 4,
                recovery: 8,
                cancels: [(from: 8, to: 12, allow: [(intent: "roll", move: "roll")])],
            ),
            (name: "stagger", startup: 0, active: 10, recovery: 10),
            (name: "down", startup: 0, active: 20, recovery: 20, invulnerable: [(from: 0, to: 40)]),
            (
                name: "broken",
                startup: 0,
                active: 40,
                recovery: 20,
                armour: [(from: 0, to: 60, threshold: 1000)],
            ),
            (name: "defeat", startup: 0, active: 10, recovery: 20),
        ],
    ),
)"#;

fn name(text: &str) -> Name {
    Name::new(text)
}

fn ticks(count: u64) -> SimDuration {
    SimDuration::from_ticks(count)
}

fn moves() -> MoveSet {
    MoveSetDocument::parse(FIGHT).unwrap().compile().unwrap()
}

/// [`moves`] with some of [`FIGHT`]'s text replaced.
fn moves_with(from: &str, to: &str) -> MoveSet {
    assert!(FIGHT.contains(from), "{from}");
    MoveSetDocument::parse(&FIGHT.replacen(from, to, 1)).unwrap().compile().unwrap()
}

/// `poke` and `bash` with nothing that moves anyone, so one target stays in reach of any number of them.
fn still_text() -> String {
    let root = "root: [(at: 2, offset: (0.0, 0.0, 0.0)), (at: 6, offset: (0.0, 0.0, 0.4))],";
    let (poke, bash) = (", knockback: (distance: 0.5, ticks: 10)", ", knockback: (distance: 1.0, ticks: 10)");
    for text in [root, poke, bash] {
        assert!(FIGHT.contains(text), "{text}");
    }
    FIGHT.replacen(root, "", 1).replacen(poke, "", 1).replacen(bash, "", 1)
}

fn still() -> MoveSet {
    MoveSetDocument::parse(&still_text()).unwrap().compile().unwrap()
}

fn world_of(set: MoveSet) -> (Simulation, ActorSpawn) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(set);
    (sim, ActorSpawn { moves: Some(moves), ..ActorSpawn::default() })
}

/// An attacker at the origin facing +Z, and a target 1 m ahead facing it.
fn duel_of(set: MoveSet) -> (Simulation, ActorId, ActorId) {
    let (mut sim, spawn) = world_of(set);
    let attacker = sim.spawn(spawn).unwrap();
    let at = Vec3::new(0.0, 0.0, 1.0);
    let target = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
    (sim, attacker, target)
}

fn duel() -> (Simulation, ActorId, ActorId) {
    duel_of(moves())
}

fn press(actor: ActorId, buttons: Buttons) -> TickActions {
    press_all(&[(actor, buttons)])
}

fn press_all(presses: &[(ActorId, Buttons)]) -> TickActions {
    let mut actions = TickActions::NEUTRAL;
    for &(actor, buttons) in presses {
        actions.set(actor, ActorActions { buttons, stick: Stick::CENTER }).unwrap();
    }
    actions
}

fn idle(sim: &mut Simulation, steps: u32) {
    for _ in 0..steps {
        sim.step(&TickActions::NEUTRAL);
    }
}

/// Presses and steps until the hit lands: an attack opens on tick 2, two steps after its press.
fn land(sim: &mut Simulation, attacker: ActorId, buttons: Buttons) {
    sim.step(&press(attacker, buttons));
    idle(sim, 2);
}

fn fighter(sim: &Simulation, actor: ActorId) -> Fighter {
    sim.actor(actor).unwrap().fighter
}

fn playing(sim: &Simulation, actor: ActorId) -> Option<Name> {
    sim.actor(actor).unwrap().moves.current.map(|current| current.name)
}

fn elapsed(sim: &Simulation, actor: ActorId) -> SimDuration {
    sim.actor(actor).unwrap().moves.current.unwrap().elapsed
}

fn frozen(sim: &Simulation, actor: ActorId) -> u16 {
    sim.actor(actor).unwrap().hitstop_ticks
}

fn clock(sim: &Simulation, actor: ActorId) -> SimTime {
    sim.actor(actor).unwrap().clock_now
}

fn position(sim: &Simulation, actor: ActorId) -> Vec3 {
    sim.actor(actor).unwrap().position
}

fn hit_cues(sim: &Simulation) -> Vec<CueEvent> {
    sim.cues().filter(|cue| cue.hit.is_some()).collect()
}

fn assert_near(actual: Vec3, expected: Vec3) {
    assert!(actual.abs_diff_eq(expected, 1e-4), "{actual} is not {expected}");
}

const SPEEDS: [u32; 8] = [100, 250, 500, 1000, 2000, 3000, 5000, 9000];

// What the plan's "done" asks for.

#[test]
fn a_swing_hits_damages_reacts_and_freezes_both_clocks() {
    let (mut sim, attacker, target) = duel();
    sim.step(&press(attacker, Buttons::LIGHT));
    idle(&mut sim, 1);
    assert_eq!(sim.stats().hits_applied, 0, "not yet: the window opens on tick 2");
    assert_eq!(fighter(&sim, target).health(), 100);
    idle(&mut sim, 1);

    // It lands everywhere in this one step.
    assert_eq!(sim.hits().events().len(), 1);
    let stats = sim.stats();
    assert_eq!((stats.hits_applied, stats.reactions_forced, stats.hits_absorbed), (1, 1, 0));
    assert_eq!(fighter(&sim, target).health(), 90);
    assert_eq!(fighter(&sim, target).poise(), 25);
    assert_eq!(fighter(&sim, attacker).health(), 100, "only the target takes it");
    assert_eq!((frozen(&sim, attacker), frozen(&sim, target)), (6, 6));
    assert_eq!(hit_cues(&sim).len(), 1);
    // This step's clocks had already ticked, so nothing has stopped yet.
    assert_eq!(elapsed(&sim, attacker), ticks(2));
    assert_eq!(playing(&sim, target), None);

    // Both stop for exactly six steps. The target shows its reaction's first frame throughout, since a
    // forced start is the one thing a frozen clock fires, and the attacker holds where the hit found it.
    let (a_at, t_at) = (clock(&sim, attacker), clock(&sim, target));
    for left in (0..6).rev() {
        idle(&mut sim, 1);
        assert_eq!((frozen(&sim, attacker), frozen(&sim, target)), (left, left));
        assert_eq!((clock(&sim, attacker), clock(&sim, target)), (a_at, t_at), "{left} left");
        assert_eq!(playing(&sim, attacker), Some(name("poke")));
        assert_eq!(elapsed(&sim, attacker), ticks(2));
        assert_eq!(playing(&sim, target), Some(name("flinch")));
        assert_eq!(elapsed(&sim, target), ticks(0));
    }
    assert_eq!(fighter(&sim, target).health(), 90, "and it doesn't land again");

    // Then both run, from exactly where they stopped.
    idle(&mut sim, 1);
    assert_eq!(elapsed(&sim, attacker), ticks(3));
    assert_eq!(elapsed(&sim, target), ticks(1));
    idle(&mut sim, 16);
    assert_eq!(elapsed(&sim, attacker), ticks(19));
    assert_eq!(sim.stats().hits_applied, 1);
}

#[test]
fn a_hits_cue_says_all_of_it_in_the_step_that_applies_it() {
    let (mut sim, attacker, target) = duel();
    sim.step(&press(attacker, Buttons::LIGHT));
    let started = sim.actor(attacker).unwrap().moves.current.unwrap().started_at;
    idle(&mut sim, 2);
    let cues = hit_cues(&sim);
    assert_eq!(cues.len(), 1);
    let cue = cues[0];
    assert_eq!((cue.actor, cue.cue, cue.tick), (attacker, HIT_LANDED, sim.tick()));
    assert_eq!(cue.move_time, ticks(2));
    assert_eq!(cue.local_time, started + ticks(2));
    let hit = cue.hit.unwrap();
    assert_eq!((hit.target, hit.hurtbox, hit.weight), (target, 0, Weight::Light));
    assert_eq!((hit.damage, hit.poise, hit.hitstop, hit.outcome), (10, 5, 6, HitOutcome::Flinched));
    // On the chest's surface, toward the blade: chest at z = 1, radius 0.25.
    assert_near(hit.point, Vec3::new(0.0, 1.0, 0.75));
    assert_near(hit.normal, Vec3::new(0.0, 0.0, -1.0));

    // Gone by the next step, since a queue holds one step's.
    idle(&mut sim, 1);
    assert_eq!(hit_cues(&sim), []);

    // An authored name goes in place of the default.
    let (mut sim, attacker, _) = duel();
    land(&mut sim, attacker, Buttons::SWAP);
    assert_eq!(hit_cues(&sim)[0].cue, name("slam_hit"));
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().weight, Weight::Heavy);
}

/// What a cue carries for what happens to the target, on top of the hit's own.
#[test]
fn a_break_and_a_defeat_each_publish_a_cue_of_their_own() {
    // Two lunges take 20 poise each, of 30.
    let (mut sim, attacker, target) = duel_of(moves());
    land(&mut sim, attacker, Buttons::JUMP);
    idle(&mut sim, 1);
    assert_eq!(hit_cues(&sim).iter().map(|cue| cue.cue).collect::<Vec<_>>(), [HIT_LANDED]);
    idle(&mut sim, 30);
    land(&mut sim, attacker, Buttons::JUMP);
    idle(&mut sim, 1);
    let cues = hit_cues(&sim);
    assert_eq!(cues.iter().map(|cue| cue.cue).collect::<Vec<_>>(), [HIT_LANDED, POISE_BROKEN]);
    assert!(cues.iter().all(|cue| cue.hit.unwrap().outcome == HitOutcome::PoiseBroken));
    assert!(cues.iter().all(|cue| cue.hit.unwrap().target == target));

    let (mut sim, attacker, target) = duel();
    land(&mut sim, attacker, Buttons::LOCK_ON);
    let cues = hit_cues(&sim);
    assert_eq!(cues.iter().map(|cue| cue.cue).collect::<Vec<_>>(), [HIT_LANDED, DEFEATED]);
    assert!(cues.iter().all(|cue| cue.hit.unwrap().outcome == HitOutcome::Defeated));
    assert_eq!(cues[1].hit.unwrap().target, target);
}

// A hit lands exactly once.

#[test]
fn a_hit_lands_once_per_instance_at_any_clock_speed() {
    for per_mille in SPEEDS {
        let (mut sim, attacker, target) = duel_of(still());
        sim.clock_mut(attacker).unwrap().0.push_scale(ScaleSource::Debug, per_mille).unwrap();
        let mut landed = Vec::new();
        sim.step(&press(attacker, Buttons::HEAVY));
        for _ in 0..1000 {
            landed.extend_from_slice(sim.hits().events());
            idle(&mut sim, 1);
        }
        assert_eq!(landed.len(), 1, "at {per_mille}‰: {landed:?}");
        assert_eq!(landed[0].at, ticks(2), "at {per_mille}‰");
        assert_eq!(fighter(&sim, target).health(), 75, "at {per_mille}‰");
        assert_eq!(fighter(&sim, target).poise(), 30, "at {per_mille}‰: refilled by now");
        assert_eq!(sim.stats().hits_applied, 1, "at {per_mille}‰");

        // Another playing is another hit.
        sim.step(&press(attacker, Buttons::HEAVY));
        idle(&mut sim, 200);
        assert_eq!(sim.stats().hits_applied, 2, "at {per_mille}‰");
        assert_eq!(fighter(&sim, target).health(), 50, "at {per_mille}‰");
    }
}

#[test]
fn a_hit_group_rearms_on_its_authored_ticks_at_any_speed() {
    for attacker_speed in SPEEDS {
        for target_speed in [1000, 300, 4000] {
            let (mut sim, attacker, target) = duel();
            sim.clock_mut(attacker).unwrap().0.push_scale(ScaleSource::Debug, attacker_speed).unwrap();
            sim.clock_mut(target).unwrap().0.push_scale(ScaleSource::Debug, target_speed).unwrap();
            let what = format!("attacker {attacker_speed}‰, target {target_speed}‰");
            let mut landed = Vec::new();
            sim.step(&press(attacker, Buttons::ULTIMATE));
            for _ in 0..1000 {
                landed.extend_from_slice(sim.hits().events());
                idle(&mut sim, 1);
            }
            let beats: Vec<_> = landed.iter().map(|hit| (hit.volley, hit.at)).collect();
            assert_eq!(beats, [(0, ticks(2)), (1, ticks(8)), (2, ticks(14))], "{what}");
            assert_eq!(fighter(&sim, target).health(), 91, "{what}");
            assert_eq!(sim.stats().hits_applied, 3, "{what}");
        }
    }
}

#[test]
fn a_volume_with_no_attack_is_an_event_and_nothing_more() {
    let (mut sim, attacker, target) = duel();
    sim.force_move(attacker, name("sensor"));
    let mut events = 0;
    for _ in 0..30 {
        idle(&mut sim, 1);
        events += sim.hits().events().len();
        assert_eq!(frozen(&sim, attacker), 0);
        assert_eq!(hit_cues(&sim), []);
    }
    assert_eq!(events, 1);
    assert_eq!(fighter(&sim, target).health(), 100);
    assert_eq!(sim.stats().hits_applied, 0);
    assert_eq!(playing(&sim, target), None);
}

/// The thinnest shape a document can name is one the world takes, or a fighter would compile that nobody
/// could hit.
#[test]
fn a_documents_thinnest_shapes_are_in_the_world() {
    let thin = sapphire_moves::MIN_SHAPE_EXTENT;
    assert_eq!(thin, sapphire_colliders::document::MIN_EXTENT);
    for shape in [
        sapphire_moves::Shape::Sphere { radius: thin },
        sapphire_moves::Shape::Capsule { radius: thin, length: 0.0 },
        sapphire_moves::Shape::Capsule { radius: thin, length: thin },
        sapphire_moves::Shape::Box { half_extents: [thin; 3] },
    ] {
        assert!(shape.is_valid(), "{shape:?}");
        assert!(sapphire_gameplay::hit_shape(shape).is_valid(), "{shape:?}");
    }
    let (mut sim, spawn) = world_of(moves_with("Sphere(radius: 0.25)", "Sphere(radius: 0.001)"));
    let actor = sim.spawn(spawn).unwrap();
    idle(&mut sim, 1);
    assert!(sim.physics().body_shape(HurtboxId::new(actor, 0).unwrap().body()).is_some());
    assert_eq!(sim.stats().hurtboxes_refused, 0);
}

#[test]
fn a_set_with_no_fighter_cant_be_hit_and_has_no_bodies() {
    let mut document = MoveSetDocument::parse(FIGHT).unwrap();
    document.fighter = None;
    let (mut sim, attacker, target) = duel_of(document.compile().unwrap());
    assert!(!fighter(&sim, target).is_fighter());
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 30);
    assert_eq!(sim.physics().body_count(), 0);
    assert_eq!(sim.stats().hits_applied, 0);
    assert_eq!(sim.hits().stats().events, 0);
    assert_eq!(fighter(&sim, target).health(), 0);
}

/// A hurtbox anyone put in the world is touched like any other, but with no fighter behind it the hit is
/// an event and nothing more: no numbers, no freeze.
#[test]
fn a_hurtbox_on_something_thats_no_fighter_is_an_event_and_nothing_more() {
    let (mut sim, spawn) = world_of(moves());
    let attacker = sim.spawn(spawn).unwrap();
    let position = Vec3::new(0.0, 0.0, 1.0);
    let target = sim.spawn(ActorSpawn { position, moves: None, ..spawn }).unwrap();
    assert!(!fighter(&sim, target).is_fighter());
    let hurtbox = HurtboxId::new(target, 0).unwrap();
    sim.add_combat_system(CombatSet::Intents, move |mut characters: ResMut<'_, CharacterWorld>| {
        let root = ActorTransform { position, yaw: 0.0 };
        let local = PathSample { translation: [0.0, 1.0, 0.0], ..PathSample::IDENTITY };
        let shape = Some(Shape::Sphere { radius: 0.25 });
        hurtbox.set(characters.physics_mut(), shape, root, local, Faction::NONE).unwrap();
    });

    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(sim.hits().stats().events, 1);
    assert_eq!(sim.stats().hits_applied, 0);
    assert!(hit_cues(&sim).is_empty());
    assert_eq!(frozen(&sim, attacker), 0);
    assert_eq!(fighter(&sim, target), Fighter::default());
}

// The order hits apply in.

/// The actors of [`spawned_any_way`], by what they're there for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Near,
    Far,
    Target,
}

/// A lunge from 1 m and another from 1.5 m, at a target with 30 poise, turned so each sweeps across it
/// from its own side. Each takes 20, so only the second to apply breaks it: the near one touches after
/// 0.65 m and the far one after 1.15 m. Everything they do, by role.
fn spawned_any_way(order: [Role; 3], dead_before: u32, reverse_deaths: bool) -> Vec<String> {
    let (mut sim, spawn) = world_of(moves());
    // What dies before them decides which of the pool's entities they get, and the IDs they get.
    let mut gone: Vec<ActorId> = (0..dead_before).map(|_| sim.spawn(spawn).unwrap()).collect();
    if reverse_deaths {
        gone.reverse();
    }
    for actor in gone {
        assert!(sim.despawn(actor));
    }

    let mut cast = Vec::new();
    for role in order {
        let (position, yaw) = match role {
            Role::Near => (Vec3::new(0.0, 0.0, -1.0), 0.0),
            Role::Far => (Vec3::new(-1.5, 0.0, 0.0), FRAC_PI_2),
            Role::Target => (Vec3::ZERO, 0.0),
        };
        cast.push((role, sim.spawn(ActorSpawn { position, yaw, ..spawn }).unwrap()));
    }
    let role_of = |actor: ActorId| cast.iter().find(|(_, id)| *id == actor).unwrap().0;
    let of = |role: Role| cast.iter().find(|(other, _)| *other == role).unwrap().1;

    let mut seen = Vec::new();
    sim.step(&press_all(&[(of(Role::Near), Buttons::JUMP), (of(Role::Far), Buttons::JUMP)]));
    for _ in 0..80 {
        idle(&mut sim, 1);
        let line = hit_cues(&sim)
            .iter()
            .map(|cue| {
                let hit = cue.hit.unwrap();
                format!("{:?} {:?} {:?}", role_of(cue.actor), role_of(hit.target), hit.outcome)
            })
            .collect::<Vec<_>>()
            .join(", ");
        if !line.is_empty() {
            seen.push(line);
        }
        let target = of(Role::Target);
        seen.push(format!(
            "{} {} {} {:?} {}",
            fighter(&sim, target).health(),
            fighter(&sim, target).poise(),
            frozen(&sim, target),
            playing(&sim, target).map(Name::as_u64),
            position(&sim, target),
        ));
    }
    seen
}

/// Identical actors hide an ordering leak, so these differ, in where they stand and what they do to the
/// meter. Anything that went by creation order, or by where an entity lies in the world, would give
/// the break to whoever came first.
#[test]
fn damage_and_poise_go_the_same_way_however_actors_are_created() {
    use Role::{Far, Near, Target};
    let expected = spawned_any_way([Near, Far, Target], 0, false);
    let text = expected.join("\n");
    assert!(text.contains("Near Target Flinched"), "{text}");
    assert!(text.contains("Far Target PoiseBroken"), "the far one comes second, and breaks it: {text}");
    for order in [[Target, Far, Near], [Far, Target, Near], [Near, Target, Far], [Far, Near, Target]] {
        for (dead, reverse) in [(0, false), (5, false), (5, true), (11, true)] {
            assert_eq!(spawned_any_way(order, dead, reverse), expected, "{order:?} after {dead} dead");
        }
    }
}

#[test]
fn two_actors_hitting_each_other_on_one_tick_trade_in_the_stated_order() {
    // Same reach, same tick, so the same travel: the tie goes to the lower ID.
    for first_is_poker in [true, false] {
        let (mut sim, spawn) = world_of(moves());
        let at_one = ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn };
        let (poker, basher) = if first_is_poker {
            let poker = sim.spawn(spawn).unwrap();
            (poker, sim.spawn(at_one).unwrap())
        } else {
            let basher = sim.spawn(at_one).unwrap();
            (sim.spawn(spawn).unwrap(), basher)
        };
        sim.step(&press_all(&[(poker, Buttons::LIGHT), (basher, Buttons::HEAVY)]));
        idle(&mut sim, 2);

        let attackers: Vec<_> = hit_cues(&sim).iter().map(|cue| cue.actor).collect();
        let expected = if first_is_poker { [poker, basher] } else { [basher, poker] };
        assert_eq!(attackers, expected);

        // Both land whichever came first: each takes the other's numbers, and the longer freeze.
        assert_eq!(fighter(&sim, poker).health(), 75);
        assert_eq!(fighter(&sim, poker).poise(), 18);
        assert_eq!(fighter(&sim, basher).health(), 90);
        assert_eq!(fighter(&sim, basher).poise(), 25);
        assert_eq!((frozen(&sim, poker), frozen(&sim, basher)), (8, 8));
        idle(&mut sim, 1);
        assert_eq!(playing(&sim, poker), Some(name("stagger")));
        assert_eq!(playing(&sim, basher), Some(name("flinch")));
    }
}

#[test]
fn a_blow_struck_on_the_tick_its_attacker_falls_still_lands() {
    // The finisher has the lower ID, so it applies first and defeats the basher, whose bash has already
    // been swung.
    let (mut sim, finisher, basher) = duel();
    sim.step(&press_all(&[(finisher, Buttons::LOCK_ON), (basher, Buttons::HEAVY)]));
    idle(&mut sim, 2);
    assert!(fighter(&sim, basher).is_defeated());
    assert_eq!(fighter(&sim, finisher).health(), 75);
    assert_eq!(sim.stats().hits_applied, 2);
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, basher), Some(name("defeat")));
    assert_eq!(playing(&sim, finisher), Some(name("stagger")));
}

#[test]
fn a_target_defeated_earlier_in_a_step_takes_no_more() {
    // Both at reach, on the same tick, the finisher first by ID.
    let (mut sim, spawn) = world_of(moves());
    let finisher = sim.spawn(spawn).unwrap();
    let side = ActorSpawn { position: Vec3::new(1.0, 0.0, 1.0), yaw: -FRAC_PI_2, ..spawn };
    let poker = sim.spawn(side).unwrap();
    let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), ..spawn }).unwrap();
    sim.step(&press_all(&[(finisher, Buttons::LOCK_ON), (poker, Buttons::LIGHT)]));
    idle(&mut sim, 2);
    assert_eq!(sim.hits().events().len(), 2, "both touched");
    assert_eq!(sim.stats().hits_applied, 1, "one applied");
    let cues = hit_cues(&sim);
    assert_eq!(cues.iter().map(|cue| cue.actor).collect::<Vec<_>>(), [finisher, finisher]);
    assert_eq!(fighter(&sim, target).health(), 0);
    assert_eq!(frozen(&sim, poker), 30, "the finish freeze, and not its own 6");
}

#[test]
fn the_strongest_reaction_of_a_step_plays_with_its_push() {
    // A poke and a slam on one tick, each way round by ID: the slam's knockdown wins, and the slam's push.
    for slam_first in [true, false] {
        let (mut sim, spawn) = world_of(moves());
        let side = ActorSpawn { position: Vec3::new(1.0, 0.0, 1.0), yaw: -FRAC_PI_2, ..spawn };
        let (slammer, poker) = if slam_first {
            let slammer = sim.spawn(spawn).unwrap();
            (slammer, sim.spawn(side).unwrap())
        } else {
            let poker = sim.spawn(side).unwrap();
            (sim.spawn(spawn).unwrap(), poker)
        };
        let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), ..spawn }).unwrap();
        sim.step(&press_all(&[(slammer, Buttons::SWAP), (poker, Buttons::LIGHT)]));
        idle(&mut sim, 2);
        assert_eq!(fighter(&sim, target).health(), 50, "both did their damage");
        assert_eq!(sim.stats().reactions_forced, if slam_first { 1 } else { 2 });
        idle(&mut sim, 1);
        assert_eq!(playing(&sim, target), Some(name("down")), "slam first: {slam_first}");
        // A frozen 12 ticks for the slam, then the slam's push, away from the slammer: 2 m along +Z.
        idle(&mut sim, 30);
        assert_near(position(&sim, target), Vec3::new(0.0, 0.0, 3.0));
    }
}

/// Two slams on one tick from two sides, each taking 40 health and 15 poise: what the second comes to
/// outranks the first's, so its reaction is the one that plays.
#[test]
fn a_stronger_outcome_later_in_a_step_takes_over() {
    let meter = "health: 100,\n            poise: 30,";
    for (set, outcomes, reaction) in [
        // A heavy flinch, then a break.
        (moves(), [HitOutcome::Flinched, HitOutcome::PoiseBroken], "broken"),
        // A break, then a defeat.
        (
            moves_with(meter, "health: 50,\n            poise: 15,"),
            [HitOutcome::PoiseBroken, HitOutcome::Defeated],
            "defeat",
        ),
    ] {
        let (mut sim, spawn) = world_of(set);
        let first = sim.spawn(spawn).unwrap();
        let side = ActorSpawn { position: Vec3::new(1.0, 0.0, 1.0), yaw: -FRAC_PI_2, ..spawn };
        let second = sim.spawn(side).unwrap();
        let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), ..spawn }).unwrap();
        sim.step(&press_all(&[(first, Buttons::SWAP), (second, Buttons::SWAP)]));
        idle(&mut sim, 2);
        let landed: Vec<_> = hit_cues(&sim)
            .iter()
            .filter(|cue| cue.cue != POISE_BROKEN && cue.cue != DEFEATED)
            .map(|cue| cue.hit.unwrap().outcome)
            .collect();
        assert_eq!(landed, outcomes);
        idle(&mut sim, 1);
        assert_eq!(playing(&sim, target), Some(name(reaction)));
    }
}

#[test]
fn reactions_go_by_a_hits_weight() {
    for (buttons, reaction) in
        [(Buttons::LIGHT, "flinch"), (Buttons::HEAVY, "stagger"), (Buttons::SWAP, "down")]
    {
        let (mut sim, attacker, target) = duel();
        land(&mut sim, attacker, buttons);
        idle(&mut sim, 1);
        assert_eq!(playing(&sim, target), Some(name(reaction)));
    }
}

#[test]
fn a_reaction_left_out_is_no_reaction() {
    // The hit does the rest of what it does.
    let set = moves_with(r#"light: "flinch", "#, "");
    let (mut sim, attacker, target) = duel_of(set);
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(sim.stats().reactions_forced, 0);
    assert_eq!(fighter(&sim, target).health(), 90);
    assert_eq!(frozen(&sim, target), 6);
    idle(&mut sim, 7);
    assert_eq!(playing(&sim, target), None);
    assert!(fighter(&sim, target).is_sliding(), "and the push still comes");
}

#[test]
fn a_later_reaction_replaces_whats_playing() {
    // A bash staggers the target, and a poke from its side, once they're both moving, flinches it out of
    // that: the reaction that replaces another doesn't have to be stronger, only later. A move that
    // shouldn't be flinched out of says so with armour or `invulnerable`.
    let (mut sim, spawn) = world_of(still());
    let basher = sim.spawn(spawn).unwrap();
    let side = ActorSpawn { position: Vec3::new(1.0, 0.0, 1.0), yaw: -FRAC_PI_2, ..spawn };
    let poker = sim.spawn(side).unwrap();
    let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), ..spawn }).unwrap();
    land(&mut sim, basher, Buttons::HEAVY);
    idle(&mut sim, 8);
    assert_eq!(playing(&sim, target), Some(name("stagger")));
    let serial = sim.actor(target).unwrap().moves.started;
    land(&mut sim, poker, Buttons::LIGHT);
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, target), Some(name("flinch")));
    assert_eq!(sim.actor(target).unwrap().moves.started, serial + 1);
}

// Hitstop.

/// The attacker's root, its volume and its table: a freeze delays them and costs them nothing. Every
/// reading's position, with and without a hit, bit for bit.
#[test]
fn hitstop_loses_no_ground() {
    fn walk(reach: f32) -> Vec<(u64, u32)> {
        let (mut sim, spawn) = world_of(moves());
        let attacker = sim.spawn(spawn).unwrap();
        sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, reach), yaw: PI, ..spawn }).unwrap();
        sim.step(&press(attacker, Buttons::LIGHT));
        let mut seen = Vec::new();
        let mut last = None;
        for _ in 0..40 {
            idle(&mut sim, 1);
            let Some(current) = sim.actor(attacker).unwrap().moves.current else { break };
            let reading = current.elapsed.as_subticks();
            if last != Some(reading) {
                seen.push((reading, position(&sim, attacker).z.to_bits()));
                last = Some(reading);
            }
        }
        seen
    }
    let hit = walk(1.0);
    let missed = walk(30.0);
    assert_eq!(hit, missed);
    assert_eq!(hit.len(), 19, "every tick of the move after its first");
    assert!((f32::from_bits(hit.last().unwrap().1) - 0.4).abs() < 1e-6, "all of its root's 0.4 m");
}

#[test]
fn hitstop_freezes_for_exactly_its_authored_ticks_at_any_speed() {
    for per_mille in SPEEDS {
        let (mut sim, attacker, target) = duel();
        for actor in [attacker, target] {
            sim.clock_mut(actor).unwrap().0.push_scale(ScaleSource::Debug, per_mille).unwrap();
        }
        sim.step(&press(attacker, Buttons::SWAP));
        let mut applied_on = None;
        let (mut attacker_stood, mut target_stood) = (0, 0);
        let mut previous = (clock(&sim, attacker), clock(&sim, target));
        for step in 0..1000 {
            idle(&mut sim, 1);
            let now = (clock(&sim, attacker), clock(&sim, target));
            if applied_on.is_none() && sim.stats().hits_applied == 1 {
                applied_on = Some(step);
                assert_eq!(frozen(&sim, attacker), 12, "at {per_mille}‰");
                assert_eq!(frozen(&sim, target), 12, "at {per_mille}‰");
            }
            attacker_stood += u32::from(now.0 == previous.0);
            target_stood += u32::from(now.1 == previous.1);
            previous = now;
        }
        // A clock under 1× still moves whole sub-ticks every step, so only a freeze stands it still.
        assert!(applied_on.is_some(), "at {per_mille}‰");
        assert_eq!((attacker_stood, target_stood), (12, 12), "at {per_mille}‰");
    }
}

/// Spec §5.21's P3 note and `LocalClock::apply_hitstop`: the longest freeze asked of an actor wins, and
/// freezes never add up. Rows: `poke` freezes for 6, `bash` for 8, `spin` for 2, and a second attacker
/// lands three steps after the first.
#[test]
fn overlapping_freezes_keep_the_longest() {
    for (second, target_after, second_after, first_after) in [
        // Three of the first hit's six are left: a longer one replaces them.
        (Buttons::HEAVY, 8, 8, 3),
        (Buttons::LIGHT, 6, 6, 3),
        // A shorter one doesn't cut them.
        (Buttons::ULTIMATE, 3, 2, 3),
    ] {
        let (mut sim, spawn) = world_of(moves());
        let first = sim.spawn(spawn).unwrap();
        let side = ActorSpawn { position: Vec3::new(1.0, 0.0, 1.0), yaw: -FRAC_PI_2, ..spawn };
        let other = sim.spawn(side).unwrap();
        let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), ..spawn }).unwrap();

        sim.step(&press(first, Buttons::LIGHT));
        idle(&mut sim, 2);
        assert_eq!(frozen(&sim, target), 6);
        sim.step(&press(other, second));
        idle(&mut sim, 2);
        assert_eq!(
            (frozen(&sim, target), frozen(&sim, other), frozen(&sim, first)),
            (target_after, second_after, first_after),
            "{second:?}",
        );
    }
}

#[test]
fn a_press_during_hitstop_is_not_eaten() {
    // A freeze as long as the longest allowed, far past the press's 8 ticks of buffer. It's used at the
    // cancel window's opening, on tick 3, the first thing the attacker's clock reaches.
    let set = moves_with("hitstop: 6, knockback: (distance: 0.5", "hitstop: 120, knockback: (distance: 0.5");
    let (mut sim, attacker, _) = duel_of(set);
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(frozen(&sim, attacker), 120);
    idle(&mut sim, 40);
    sim.step(&press(attacker, Buttons::LIGHT));
    assert_eq!(sim.intents(attacker).unwrap().len(), 1, "it waits");
    idle(&mut sim, 79);
    assert_eq!(frozen(&sim, attacker), 0);
    assert_eq!(sim.intents(attacker).unwrap().len(), 1, "for as long as the freeze lasts");
    assert_eq!(playing(&sim, attacker), Some(name("poke")));

    // The attacker's first moving step reaches tick 3, and the cancel fires at that exact instant.
    let started = sim.actor(attacker).unwrap().moves.current.unwrap().started_at;
    idle(&mut sim, 1);
    let view = sim.actor(attacker).unwrap().moves.current.unwrap();
    assert_eq!(view.name, name("bash"));
    assert_eq!(view.started_at, started + ticks(3));
    assert!(sim.intents(attacker).unwrap().is_empty());
}

#[test]
fn a_cancel_out_of_hitstop_lands_at_its_exact_instant() {
    for per_mille in [100, 250, 700, 1000, 2500, 5000, 9000] {
        let (mut sim, attacker, _) = duel();
        sim.clock_mut(attacker).unwrap().0.push_scale(ScaleSource::Debug, per_mille).unwrap();
        sim.step(&press(attacker, Buttons::LIGHT));
        // Step until the hit lands: on the step that reaches tick 2, whatever that takes.
        for _ in 0..40 {
            if sim.stats().hits_applied == 1 {
                break;
            }
            idle(&mut sim, 1);
        }
        assert_eq!(sim.stats().hits_applied, 1, "at {per_mille}‰");
        let current = sim.actor(attacker).unwrap().moves.current.unwrap();
        assert_eq!(current.name, name("poke"), "at {per_mille}‰");
        // A press during the freeze: it's stamped at this reading, before the window if that's still
        // to come, so it fires as the window opens, or at the reading if that's already past it.
        idle(&mut sim, 2);
        sim.step(&press(attacker, Buttons::LIGHT));
        let reached = current.elapsed.max(ticks(3));
        for _ in 0..30 {
            if sim.actor(attacker).unwrap().moves.started != current.serial {
                break;
            }
            idle(&mut sim, 1);
        }
        let bash = sim.actor(attacker).unwrap().moves.current.unwrap();
        assert_eq!(bash.name, name("bash"), "at {per_mille}‰");
        assert_eq!(bash.started_at, current.started_at + reached, "at {per_mille}‰");
    }
}

// Armour.

#[test]
fn super_armour_takes_damage_without_a_reaction() {
    let (mut sim, attacker, target) = duel();
    sim.step(&press_all(&[(attacker, Buttons::LIGHT), (target, Buttons::SKILL)]));
    idle(&mut sim, 2);

    // Under its threshold of 6: the numbers land, and nothing else does on the target.
    assert_eq!(fighter(&sim, target).health(), 90);
    assert_eq!(fighter(&sim, target).poise(), 25);
    let stats = sim.stats();
    assert_eq!((stats.hits_applied, stats.hits_absorbed, stats.reactions_forced), (1, 1, 0));
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().outcome, HitOutcome::Absorbed);
    assert!(!fighter(&sim, target).is_sliding(), "no push");

    // And only the attacker stops, so the armoured move comes through it.
    assert_eq!((frozen(&sim, attacker), frozen(&sim, target)), (6, 0));
    let at = elapsed(&sim, target);
    idle(&mut sim, 6);
    assert_eq!(playing(&sim, target), Some(name("brace")));
    assert_eq!(elapsed(&sim, target), at + ticks(6));
    assert_eq!(elapsed(&sim, attacker), ticks(2));
}

#[test]
fn armour_stops_at_its_threshold_and_its_window() {
    // At exactly the threshold, the hit isn't below it.
    let (mut sim, attacker, target) = duel();
    sim.force_move(target, name("guard"));
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().outcome, HitOutcome::Flinched);
    assert_eq!(frozen(&sim, target), 6);
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, target), Some(name("flinch")));

    // Over it, the hit's own reaction plays.
    let (mut sim, attacker, target) = duel();
    sim.force_move(target, name("brace"));
    land(&mut sim, attacker, Buttons::HEAVY);
    idle(&mut sim, 1);
    assert_eq!(playing(&sim, target), Some(name("stagger")));

    // Inside the window to its last tick.
    let (mut sim, attacker, target) = duel();
    sim.force_move(target, name("brace"));
    idle(&mut sim, 26);
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(elapsed(&sim, target), ticks(29));
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().outcome, HitOutcome::Absorbed);

    // And out of it after.
    let (mut sim, attacker, target) = duel();
    sim.force_move(target, name("brace"));
    idle(&mut sim, 27);
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(elapsed(&sim, target), ticks(30));
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().outcome, HitOutcome::Flinched);
}

/// Windows that overlap count for their highest, whatever order they're authored in.
#[test]
fn overlapping_armour_takes_the_highest_threshold() {
    let one = "armour: [(from: 0, to: 30, threshold: 6)],";
    let two = "armour: [(from: 0, to: 30, threshold: 6), (from: 0, to: 10, threshold: 1)],";
    let (mut sim, attacker, target) = duel_of(moves_with(one, two));
    sim.force_move(target, name("brace"));
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().outcome, HitOutcome::Absorbed);
}

#[test]
fn enough_poise_breaks_armour() {
    // Each poke takes 5 of 30, under `broken`'s threshold of 1000, so five are absorbed and the sixth breaks.
    let (mut sim, attacker, target) = duel_of(still());
    let mut outcomes = Vec::new();
    for _ in 0..6 {
        sim.force_move(target, name("broken"));
        land(&mut sim, attacker, Buttons::LIGHT);
        outcomes.push(hit_cues(&sim)[0].hit.unwrap().outcome);
        idle(&mut sim, 30);
    }
    use HitOutcome::{Absorbed, PoiseBroken};
    assert_eq!(outcomes, [Absorbed, Absorbed, Absorbed, Absorbed, Absorbed, PoiseBroken]);
    assert_eq!(sim.stats().poise_breaks, 1);
    assert_eq!(fighter(&sim, target).poise(), 30, "the meter fills again");
    assert_eq!(fighter(&sim, target).health(), 40);
}

// Poise.

#[test]
fn a_poise_break_replaces_the_flinch_and_fills_the_meter() {
    // Lunges take 20 of 30: the first flinches, and the second breaks.
    let (mut sim, attacker, target) = duel();
    land(&mut sim, attacker, Buttons::JUMP);
    idle(&mut sim, 2);
    assert_eq!(fighter(&sim, target).poise(), 10);
    assert_eq!(playing(&sim, target), Some(name("flinch")));
    idle(&mut sim, 30);
    land(&mut sim, attacker, Buttons::JUMP);
    idle(&mut sim, 2);
    assert_eq!(fighter(&sim, target).poise(), 30);
    assert_eq!(playing(&sim, target), Some(name("broken")));
    assert_eq!(sim.stats().poise_breaks, 1);
}

#[test]
fn poise_fills_again_on_its_owners_clock_after_the_last_hit() {
    let (mut sim, attacker, target) = duel_of(still());
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(fighter(&sim, target).poise(), 25);
    // Six frozen steps, then 60 of its clock's ticks: the 66th step from here.
    idle(&mut sim, 65);
    assert_eq!(fighter(&sim, target).poise(), 25);
    idle(&mut sim, 1);
    assert_eq!(fighter(&sim, target).poise(), 30);

    // Frozen from outside, it waits too: four of its ticks gone, a freeze of 100, and 56 to go.
    let (mut sim, attacker, target) = duel_of(still());
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 10);
    sim.clock_mut(target).unwrap().0.apply_hitstop(100);
    idle(&mut sim, 155);
    assert_eq!(fighter(&sim, target).poise(), 25);
    idle(&mut sim, 1);
    assert_eq!(fighter(&sim, target).poise(), 30);

    // A hit that takes poise starts the wait again.
    let (mut sim, attacker, target) = duel_of(still());
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 30);
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(fighter(&sim, target).poise(), 20);
    idle(&mut sim, 65);
    assert_eq!(fighter(&sim, target).poise(), 20);
    idle(&mut sim, 1);
    assert_eq!(fighter(&sim, target).poise(), 30);

    // A hit that takes none doesn't: 27 of its ticks had gone when the bash landed, and it freezes it for
    // 8, so 41 more steps fill it, from the poke.
    let no_poise = still_text().replacen("damage: 25, poise: 12", "damage: 25, poise: 0", 1);
    let (mut sim, attacker, target) = duel_of(MoveSetDocument::parse(&no_poise).unwrap().compile().unwrap());
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 30);
    land(&mut sim, attacker, Buttons::HEAVY);
    assert_eq!(fighter(&sim, target).poise(), 25);
    idle(&mut sim, 40);
    assert_eq!(fighter(&sim, target).poise(), 25);
    idle(&mut sim, 1);
    assert_eq!(fighter(&sim, target).poise(), 30);
}

#[test]
fn no_refill_and_no_meter_are_authored_with_zeros() {
    // With no refill, only a break fills it.
    let (mut sim, attacker, target) = duel_of(moves_with("poise_refill: 60,", "poise_refill: 0,"));
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 500);
    assert_eq!(fighter(&sim, target).poise(), 25);

    // With no meter, nothing breaks it.
    let (mut sim, attacker, target) = duel_of(moves_with("poise: 30,", "poise: 0,"));
    assert_eq!(fighter(&sim, target).max_poise(), 0);
    land(&mut sim, attacker, Buttons::JUMP);
    idle(&mut sim, 1);
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().outcome, HitOutcome::Flinched);
    assert_eq!(sim.stats().poise_breaks, 0);
}

// The push.

#[test]
fn a_push_slides_away_from_the_attacker_after_the_freeze() {
    // A target 1 m off along (0.6, 0.8), and an attacker turned to face it: 0.5 m over 10 ticks.
    let (mut sim, spawn) = world_of(moves());
    let direction = Vec3::new(0.6, 0.0, 0.8);
    let yaw = det::yaw_of_direction(direction);
    let attacker = sim.spawn(ActorSpawn { yaw, ..spawn }).unwrap();
    let target = sim.spawn(ActorSpawn { position: direction, ..spawn }).unwrap();
    land(&mut sim, attacker, Buttons::LIGHT);
    assert!(fighter(&sim, target).is_sliding());
    idle(&mut sim, 6);
    assert_near(position(&sim, target), direction);

    for tick in 1..=10 {
        idle(&mut sim, 1);
        let t = tick as f32 / 10.0;
        assert_near(position(&sim, target), direction * (1.0 + 0.5 * t * (2.0 - t)));
        assert_eq!(fighter(&sim, target).is_sliding(), tick < 10, "tick {tick}");
    }
    // It stays where it ended.
    idle(&mut sim, 10);
    assert_near(position(&sim, target), direction * 1.5);
}

#[test]
fn a_push_goes_as_far_at_any_clock_speed() {
    for per_mille in SPEEDS {
        let (mut sim, attacker, target) = duel();
        sim.clock_mut(target).unwrap().0.push_scale(ScaleSource::Debug, per_mille).unwrap();
        land(&mut sim, attacker, Buttons::HEAVY);
        idle(&mut sim, 500);
        assert!(!fighter(&sim, target).is_sliding(), "at {per_mille}‰");
        assert_near(position(&sim, target), Vec3::new(0.0, 0.0, 2.0));
    }
}

/// A target standing higher than its attacker goes straight across the ground, not up. Its sphere's
/// centre is at y = 1.3 and z = 0.9, and the blade's at y = 1.0 and z = 0.6, so they're 0.42 m apart,
/// inside their radii's 0.45.
#[test]
fn a_push_goes_across_the_ground() {
    let (mut sim, spawn) = world_of(moves());
    let attacker = sim.spawn(spawn).unwrap();
    let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.3, 0.9), ..spawn }).unwrap();
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(sim.stats().hits_applied, 1);
    idle(&mut sim, 40);
    assert_near(position(&sim, target), Vec3::new(0.0, 0.3, 1.4));
}

/// A reaction's own root motion and the push both count. The target faces the way it's pushed back from,
/// so 0.5 m of push along +Z less 0.3 m of a stumble along its facing's -Z leaves it 0.2 m on.
#[test]
fn a_push_adds_to_what_a_reaction_wants_of_its_root() {
    let plain = "name: \"flinch\",\n                startup: 0,\n                active: 4,\n                recovery: 8,";
    let stumbling = format!(
        "{plain}\n                motion: (root: [(at: 0, offset: (0.0, 0.0, 0.0)), (at: 10, offset: (0.0, 0.0, 0.3))]),"
    );
    let (mut sim, attacker, target) = duel_of(moves_with(plain, &stumbling));
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 40);
    assert_near(position(&sim, target), Vec3::new(0.0, 0.0, 1.2));
}

/// A hit with no push of its own leaves a running slide alone, and its freeze only delays it. The second
/// attacker's blade stands at x = 0.4 and z = 1.3, and reaches the target's sphere for z from 1.09 to 1.51,
/// which is where the first push has it by then.
#[test]
fn a_hit_with_no_push_leaves_a_slide_running() {
    let (mut sim, spawn) = world_of(moves_with(", knockback: (distance: 1.0, ticks: 10)", ""));
    let attacker = sim.spawn(spawn).unwrap();
    let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    let beside = ActorSpawn { position: Vec3::new(1.0, 0.0, 1.3), yaw: -FRAC_PI_2, ..spawn };
    let second = sim.spawn(beside).unwrap();

    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 7);
    land(&mut sim, second, Buttons::HEAVY);
    assert_eq!(sim.stats().hits_applied, 2);
    assert!(fighter(&sim, target).is_sliding());
    assert!(position(&sim, target).z < 1.5);

    idle(&mut sim, 40);
    assert_near(position(&sim, target), Vec3::new(0.0, 0.0, 1.5));
}

#[test]
fn a_push_from_the_same_spot_goes_the_way_the_attacker_faces() {
    // `lunge` starts on the target's chest, with no direction between their roots, and faces +X.
    let (mut sim, spawn) = world_of(moves());
    let attacker = sim.spawn(ActorSpawn { yaw: FRAC_PI_2, ..spawn }).unwrap();
    let target = sim.spawn(spawn).unwrap();
    land(&mut sim, attacker, Buttons::JUMP);
    assert_eq!(sim.stats().hits_applied, 1);
    idle(&mut sim, 40);
    assert_near(position(&sim, target), Vec3::new(0.25, 0.0, 0.0));
}

// Defeat.

#[test]
fn a_defeated_actor_falls_and_does_nothing_more() {
    let (mut sim, attacker, target) = duel();
    let body = HurtboxId::new(target, 0).unwrap().body();
    land(&mut sim, attacker, Buttons::LOCK_ON);
    assert!(fighter(&sim, target).is_defeated());
    assert_eq!(fighter(&sim, target).health(), 0);
    assert_eq!(sim.stats().defeats, 1);
    assert!(sim.physics().body_shape(body).is_some(), "it leaves with the next step's movement");

    idle(&mut sim, 1);
    assert_eq!(playing(&sim, target), Some(name("defeat")));
    assert_eq!(sim.physics().body_shape(body), None);

    // Past every freeze, with its defeat played out, it starts nothing, pressed or forced to.
    idle(&mut sim, 60);
    assert_eq!(playing(&sim, target), None);
    let started = sim.actor(target).unwrap().moves.started;
    sim.step(&press(target, Buttons::LIGHT));
    idle(&mut sim, 10);
    assert_eq!(sim.actor(target).unwrap().moves.started, started);
    assert!(sim.intents(target).unwrap().is_empty());

    // Nothing touches it, and a second blow changes nothing.
    let applied = sim.stats().hits_applied;
    land(&mut sim, attacker, Buttons::LIGHT);
    idle(&mut sim, 30);
    assert_eq!(sim.stats().hits_applied, applied);
    assert_eq!(fighter(&sim, target).health(), 0);
}

/// Input goes on reaching a fallen character, and nothing comes of it, with a move playing or not.
#[test]
fn a_defeated_actor_stays_where_it_fell() {
    let (mut sim, spawn) = world_of(still());
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(100.0, 0.5, 100.0) }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    sim.set_physics_world(PhysicsWorld::new(vec![floor], sim.body_capacity()).unwrap()).unwrap();
    let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).unwrap());
    let attacker = sim.spawn(ActorSpawn { controller, ..spawn }).unwrap();
    let at = Vec3::new(0.0, 0.0, 1.0);
    let target = sim.spawn(ActorSpawn { position: at, yaw: PI, controller, ..spawn }).unwrap();
    land(&mut sim, attacker, Buttons::LOCK_ON);
    assert!(fighter(&sim, target).is_defeated());

    let held = |sim: &Simulation| {
        let view = sim.actor(target).unwrap();
        (view.position, view.yaw)
    };
    let mut pushing = TickActions::NEUTRAL;
    pushing.set(target, ActorActions { buttons: Buttons::JUMP, stick: Stick::new(127, 0) }).unwrap();
    let fell = held(&sim);
    sim.step(&pushing);
    assert_eq!(playing(&sim, target), Some(name("defeat")));
    while playing(&sim, target).is_some() {
        sim.step(&pushing);
    }
    assert_eq!(held(&sim), fell, "under its reaction");
    let mut released = TickActions::NEUTRAL;
    released.set(target, ActorActions { buttons: Buttons::NONE, stick: Stick::new(127, 0) }).unwrap();
    for _ in 0..30 {
        sim.step(&released);
        sim.step(&pushing);
    }
    assert_eq!(held(&sim), fell, "with nothing playing");
}

#[test]
fn a_defeated_actors_swing_stops_with_it() {
    // With no defeat reaction to cut it off, a spin goes on publishing after its actor falls, and hits
    // nothing. It opens a step after the finisher lands, since its clock starts two steps late.
    let (mut sim, spawn) = world_of(moves_with(r#", defeat: "defeat""#, ""));
    let finisher = sim.spawn(spawn).unwrap();
    let swinger = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    sim.force_move(swinger, name("spin"));
    sim.clock_mut(swinger).unwrap().0.apply_hitstop(2);
    land(&mut sim, finisher, Buttons::LOCK_ON);
    assert!(fighter(&sim, swinger).is_defeated());
    let mut published = 0;
    for _ in 0..60 {
        idle(&mut sim, 1);
        published += sim.actor(swinger).unwrap().moves.volumes.len();
    }
    assert!(published > 0, "it goes on swinging");
    assert_eq!(fighter(&sim, finisher).health(), 100);
    assert_eq!(sim.stats().hits_applied, 1);
}

#[test]
fn a_finish_freeze_stops_every_actor_and_the_world_runs_on() {
    let (mut sim, spawn) = world_of(moves());
    let attacker = sim.spawn(spawn).unwrap();
    let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    let bystander = sim.spawn(ActorSpawn { position: Vec3::new(30.0, 0.0, 0.0), ..spawn }).unwrap();
    sim.step(&press_all(&[(attacker, Buttons::LOCK_ON), (bystander, Buttons::DODGE)]));
    idle(&mut sim, 2);
    assert!(fighter(&sim, target).is_defeated());
    for actor in [attacker, target, bystander] {
        assert_eq!(frozen(&sim, actor), 30);
    }
    let reading = clock(&sim, bystander);
    let world_clock = |sim: &Simulation| sim.world().resource::<WorldClock>().0.now();
    for step in 1..=30 {
        let world_was = world_clock(&sim);
        idle(&mut sim, 1);
        // The world clock is what effects and the camera run on: it goes on at full speed.
        assert_eq!(world_clock(&sim), world_was + ticks(1), "step {step}");
        assert_eq!(clock(&sim, bystander), reading, "step {step}");
    }
    idle(&mut sim, 1);
    assert_eq!(clock(&sim, bystander), reading + ticks(1));
}

#[test]
fn a_finish_freeze_waits_for_a_finish() {
    let (mut sim, spawn) = world_of(moves_with("health: 100,", "health: 5000,"));
    let attacker = sim.spawn(spawn).unwrap();
    let target = sim.spawn(ActorSpawn { position: Vec3::new(0.0, 0.0, 1.0), yaw: PI, ..spawn }).unwrap();
    let bystander = sim.spawn(ActorSpawn { position: Vec3::new(30.0, 0.0, 0.0), ..spawn }).unwrap();
    land(&mut sim, attacker, Buttons::LOCK_ON);
    assert_eq!(fighter(&sim, target).health(), 4000);
    assert_eq!((frozen(&sim, attacker), frozen(&sim, target)), (10, 10));
    assert_eq!(frozen(&sim, bystander), 0);
}

// World-clock slow motion: the seam for M2 G4.

/// A running world clock's speed, in thousandths, by its latest step.
fn world_rate(sim: &Simulation) -> u64 {
    sim.world().resource::<WorldClock>().0.last_delta().as_subticks()
}

fn slow(sim: &Simulation) -> SlowMotion {
    *sim.world().resource::<SlowMotion>()
}

#[test]
fn slow_motion_slows_the_world_for_exactly_its_steps() {
    let (mut sim, ..) = duel();
    sim.request_slow_motion(200, 10);
    assert!(!slow(&sim).is_running());
    // Taken up by this step's `TimeScale`, after this step's clocks, as hitstop is.
    idle(&mut sim, 1);
    assert_eq!(world_rate(&sim), 1000);
    assert!(slow(&sim).is_running());
    for _ in 0..10 {
        idle(&mut sim, 1);
        assert_eq!(world_rate(&sim), 200);
    }
    assert!(!slow(&sim).is_running());
    idle(&mut sim, 1);
    assert_eq!(world_rate(&sim), 1000);
}

#[test]
fn a_new_request_replaces_slow_motion_and_one_for_no_steps_ends_it() {
    let (mut sim, ..) = duel();
    sim.request_slow_motion(200, 10);
    idle(&mut sim, 3);
    assert_eq!(world_rate(&sim), 200);
    sim.request_slow_motion(500, 4);
    idle(&mut sim, 1);
    assert_eq!(world_rate(&sim), 200, "this step's clocks already ticked");
    for _ in 0..4 {
        idle(&mut sim, 1);
        assert_eq!(world_rate(&sim), 500);
    }
    idle(&mut sim, 1);
    assert_eq!(world_rate(&sim), 1000);

    sim.request_slow_motion(200, 10);
    idle(&mut sim, 3);
    sim.request_slow_motion(200, 0);
    idle(&mut sim, 1);
    assert!(!slow(&sim).is_running());
    idle(&mut sim, 1);
    assert_eq!(world_rate(&sim), 1000);
    // With nothing running, it does nothing.
    sim.request_slow_motion(500, 0);
    idle(&mut sim, 2);
    assert_eq!(world_rate(&sim), 1000);
}

#[test]
fn hitstop_counts_real_steps_under_slow_motion() {
    let (mut sim, attacker, target) = duel();
    sim.request_slow_motion(200, 60);
    idle(&mut sim, 1);
    sim.step(&press(attacker, Buttons::LIGHT));
    for _ in 0..30 {
        if sim.stats().hits_applied > 0 {
            break;
        }
        idle(&mut sim, 1);
    }
    assert_eq!(sim.stats().hits_applied, 1);
    let at = (clock(&sim, attacker), clock(&sim, target));
    for left in (0..6).rev() {
        idle(&mut sim, 1);
        assert_eq!((frozen(&sim, attacker), frozen(&sim, target)), (left, left));
        assert_eq!((clock(&sim, attacker), clock(&sim, target)), at);
    }
}

#[test]
fn a_stopped_world_holds_slow_motion_and_a_full_stack_refuses_it() {
    let (mut sim, ..) = duel();
    sim.request_slow_motion(200, 5);
    idle(&mut sim, 2);
    let pause = sim.world_clock_mut().0.push_scale(ScaleSource::Pause, 0).unwrap();
    idle(&mut sim, 20);
    assert!(slow(&sim).is_running());
    sim.world_clock_mut().0.pop_scale(pause);
    idle(&mut sim, 4);
    assert_eq!(world_rate(&sim), 200);
    assert!(!slow(&sim).is_running());
    idle(&mut sim, 1);
    assert_eq!(world_rate(&sim), 1000);

    // Nothing here fills the world's stack, so this does.
    while sim.world_clock_mut().0.push_scale(ScaleSource::Debug, 1000).is_ok() {}
    sim.request_slow_motion(200, 5);
    idle(&mut sim, 2);
    assert!(!slow(&sim).is_running());
    assert_eq!(slow(&sim).refused(), 1);
    assert_eq!(world_rate(&sim), 1000);
}

/// A system asks in the step that decides on it, as G4's will.
fn slow_once(mut slow: ResMut<'_, SlowMotion>, mut asked: Local<'_, bool>) {
    if !*asked {
        slow.request(100, 3);
        *asked = true;
    }
}

#[test]
fn a_system_asks_for_slow_motion_from_inside_a_step() {
    let (mut sim, ..) = duel();
    sim.add_combat_system(CombatSet::Resolve, slow_once);
    idle(&mut sim, 2);
    assert_eq!(world_rate(&sim), 100);
}

// An actor's own hurtboxes.

#[test]
fn hurtboxes_stand_where_the_fighter_says_and_change_as_moves_say() {
    let (mut sim, spawn) = world_of(moves());
    let at = Vec3::new(1.0, 0.0, 2.0);
    let actor = sim.spawn(ActorSpawn { position: at, yaw: 0.5, ..spawn }).unwrap();
    let body = HurtboxId::new(actor, 0).unwrap().body();
    assert_eq!(sim.physics().body_count(), 0, "they go in with the first step's movement");
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_count(), 1);
    assert_eq!(sim.physics().body_shape(body), Some(Shape::Sphere { radius: 0.25 }));
    assert_near(sim.physics().body_pose(body).unwrap().position, at + Vec3::Y);

    // Tick 1 on a forced start is the first step, and the window's shape counts from there.
    sim.force_move(actor, name("shrink"));
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_shape(body), Some(Shape::Sphere { radius: 0.1 }));
    idle(&mut sim, 3);
    assert_eq!(sim.physics().body_shape(body), None);
    idle(&mut sim, 2);
    assert_eq!(sim.physics().body_shape(body), Some(Shape::Sphere { radius: 0.25 }));

    // Invulnerable, it's out of the world. It follows the root it comes back to.
    idle(&mut sim, 5);
    sim.step(&press(actor, Buttons::DODGE));
    assert_eq!(sim.physics().body_shape(body), None);
    idle(&mut sim, 20);
    assert_near(sim.physics().body_pose(body).unwrap().position, position(&sim, actor) + Vec3::Y);
}

#[test]
fn a_fighter_is_hit_on_the_hurtbox_it_lists_nearest() {
    // Something low and something at chest height, where `poke`'s blade reaches only the second.
    let set = moves_with(
        r#"hurtboxes: [(name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25))],"#,
        r#"hurtboxes: [
            (name: "low", offset: (0.0, 0.2, 0.0), shape: Sphere(radius: 0.1)),
            (name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25)),
        ],"#,
    );
    let (mut sim, attacker, target) = duel_of(set);
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(hit_cues(&sim)[0].hit.unwrap().hurtbox, 1);
    assert_eq!(sim.physics().body_count(), 4);
    assert_eq!(fighter(&sim, target).health(), 90);

    // The most a list may hold all go in.
    let list: Vec<String> = (0..MAX_HURTBOXES)
        .map(|index| format!(r#"(name: "box{index}", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.1))"#))
        .collect();
    let text = FIGHT.replace(
        r#"hurtboxes: [(name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25))],"#,
        &format!("hurtboxes: [{}],", list.join(", ")),
    );
    let mut many = MoveSetDocument::parse(&text).unwrap();
    many.moves.retain(|mv| mv.name != "shrink");
    let (mut sim, spawn) = world_of(many.compile().unwrap());
    let mut last = None;
    for _ in 0..3 {
        last = Some(sim.spawn(spawn).unwrap());
    }
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_count(), 3 * usize::from(MAX_HURTBOXES));
    assert_eq!(sim.stats().hurtboxes_refused, 0);
    assert!(sim.physics().body_shape(HurtboxId::new(last.unwrap(), 15).unwrap().body()).is_some());
}

#[test]
fn a_despawn_takes_its_hurtboxes_and_its_records() {
    let (mut sim, attacker, target) = duel();
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!(sim.hits().record_count(), 1);
    assert_eq!(sim.physics().body_count(), 2);

    assert!(sim.despawn(target));
    assert_eq!(sim.physics().body_count(), 1);
    assert_eq!(sim.hits().record_count(), 0);

    // A new actor on the same pooled entity starts whole, with nothing left from the last.
    let (mut sim, spawn) = world_of(moves());
    let attacker = sim.spawn(spawn).unwrap();
    let at = Vec3::new(0.0, 0.0, 1.0);
    let target = sim.spawn(ActorSpawn { position: at, yaw: PI, ..spawn }).unwrap();
    land(&mut sim, attacker, Buttons::LOCK_ON);
    assert!(fighter(&sim, target).is_defeated());
    sim.despawn(target);
    sim.despawn(attacker);
    assert_eq!(sim.physics().body_count(), 0);
    let new = sim.spawn(spawn).unwrap();
    assert_ne!(new, target);
    let fresh = fighter(&sim, new);
    assert_eq!((fresh.health(), fresh.poise(), fresh.is_defeated()), (100, 30, false));
    assert!(!fresh.is_sliding());
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_count(), 1);
}

#[test]
fn replacing_a_set_changes_hurtboxes_on_the_next_step() {
    // Two hurtboxes become one: the second has to go, as nothing else would take it out.
    let two = moves_with(
        r#"hurtboxes: [(name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25))],"#,
        r#"hurtboxes: [
            (name: "body", offset: (0.0, 1.0, 0.0), shape: Sphere(radius: 0.25)),
            (name: "extra", offset: (0.0, 2.0, 0.0), shape: Sphere(radius: 0.25)),
        ],"#,
    );
    let mut sim = Simulation::new(SimConfig::default());
    let id = sim.add_move_set(two);
    let actor = sim.spawn(ActorSpawn { moves: Some(id), ..ActorSpawn::default() }).unwrap();
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_count(), 2);
    assert!(sim.replace_move_set(id, moves()).is_some());
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_count(), 1);
    assert_eq!(fighter(&sim, actor).health(), 100);

    // Its numbers reach it at once, and the next spawn too.
    assert!(sim.replace_move_set(id, moves_with("health: 100,", "health: 700,")).is_some());
    idle(&mut sim, 1);
    assert_eq!((fighter(&sim, actor).health(), fighter(&sim, actor).max_health()), (700, 700));
    let next = sim.spawn(ActorSpawn { moves: Some(id), ..ActorSpawn::default() }).unwrap();
    assert_eq!(fighter(&sim, next).max_health(), 700);
}

/// A fighter's numbers in a live edit (M2 T2): the next hit reads them, and an edit neither heals
/// what a hit took nor does a hit's work itself.
#[test]
fn a_live_edit_gives_a_fighter_its_numbers_and_keeps_what_it_lost() {
    let (mut sim, attacker, target) = duel();
    let set = sim.actor(target).unwrap().moves.set.unwrap();
    land(&mut sim, attacker, Buttons::LIGHT);
    assert_eq!((fighter(&sim, target).health(), fighter(&sim, target).poise()), (90, 25));

    let bigger = moves_with("health: 100,", "health: 700,");
    assert!(sim.replace_move_set(set, bigger).is_some());
    assert_eq!((fighter(&sim, target).health(), fighter(&sim, target).max_health()), (690, 700));
    assert_eq!(fighter(&sim, attacker).health(), 700, "and everyone else on the set");

    // Smaller than what it's lost: nearly gone, and a hit has to finish it.
    let tiny = moves_with("health: 100,", "health: 5,");
    assert!(sim.replace_move_set(set, moves_with("poise: 30,", "poise: 4,")).is_some());
    assert_eq!((fighter(&sim, target).poise(), fighter(&sim, target).max_poise()), (1, 4));
    assert!(sim.replace_move_set(set, tiny).is_some());
    assert_eq!((fighter(&sim, target).health(), fighter(&sim, target).max_health()), (1, 5));
    assert!(!fighter(&sim, target).is_defeated());

    // A meter it never had starts full, and the defeated stay so.
    let (mut sim, attacker, target) = duel_of(moves_with("health: 100,", "health: 10,"));
    let set = sim.actor(target).unwrap().moves.set.unwrap();
    land(&mut sim, attacker, Buttons::LIGHT);
    assert!(fighter(&sim, target).is_defeated());
    assert!(sim.replace_move_set(set, moves()).is_some());
    assert!(fighter(&sim, target).is_defeated());
    assert_eq!((fighter(&sim, target).health(), fighter(&sim, target).max_health()), (0, 100));

    // A set with no fighter leaves nothing to land on.
    let mut document = MoveSetDocument::parse(FIGHT).unwrap();
    document.fighter = None;
    assert!(sim.replace_move_set(set, document.compile().unwrap()).is_some());
    assert!(!fighter(&sim, attacker).is_fighter());
    idle(&mut sim, 1);
    assert_eq!(sim.physics().body_count(), 0);
}

// The world around a fight.

/// A wall across +Z between an attacker and its target, 0.5 m out.
fn walled(sim: &Simulation) -> PhysicsWorld {
    let wall = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(2.0, 2.0, 0.02) }),
        pose: Pose::at(Vec3::new(0.0, 1.0, 0.5)),
        memberships: Group::World.mask(),
    };
    PhysicsWorld::new(vec![wall], sim.body_capacity()).unwrap()
}

#[test]
fn a_wall_stops_a_volume_that_says_so() {
    let (mut sim, attacker, target) = duel();
    sim.set_physics_world(walled(&sim)).unwrap();
    sim.force_move(attacker, name("thrust"));
    idle(&mut sim, 12);
    assert_eq!(sim.hits().stats().volumes_stopped, 1);
    assert_eq!(fighter(&sim, target).health(), 100, "it never reached the target");

    let (mut sim, attacker, target) = duel();
    sim.set_physics_world(walled(&sim)).unwrap();
    land(&mut sim, attacker, Buttons::JUMP);
    idle(&mut sim, 1);
    assert_eq!(sim.hits().stats().volumes_stopped, 0);
    assert_eq!(fighter(&sim, target).health(), 93, "`lunge` says nothing of walls");
}

/// Nothing in a fight fills the world, since its size counts every hurtbox. Something that does costs
/// the hurtbox, and says so.
#[test]
fn a_world_with_no_room_refuses_a_hurtbox_and_counts_it() {
    let (mut sim, spawn) = world_of(moves());
    let capacity = sim.body_capacity() as u32;
    let mut filled = false;
    sim.add_combat_system(CombatSet::Intents, move |mut characters: ResMut<'_, CharacterWorld>| {
        if !std::mem::replace(&mut filled, true) {
            let groups = CollisionGroups::new(Group::Character.mask(), GroupMask::NONE);
            for raw in 0..capacity {
                let id = BodyId(raw + 1);
                let (shape, pose) = (Shape::Sphere { radius: 0.1 }, Pose::at(Vec3::ZERO));
                characters.physics_mut().add_body(BodyDesc { id, shape, pose, groups }).unwrap();
            }
        }
    });
    let actor = sim.spawn(spawn).unwrap();
    idle(&mut sim, 3);
    assert_eq!(sim.stats().hurtboxes_refused, 3);
    assert_eq!(fighter(&sim, actor).health(), 100, "and the fight goes on");
}

#[test]
fn a_world_needs_room_for_every_hurtbox_and_comes_before_them() {
    let (mut sim, ..) = duel();
    let needed = 64 * (2 + usize::from(MAX_HURTBOXES));
    let small = PhysicsWorld::new(Vec::new(), needed - 1).unwrap();
    assert!(sim.set_physics_world(small).is_err());
    assert!(sim.set_physics_world(PhysicsWorld::new(Vec::new(), needed).unwrap()).is_ok());
    idle(&mut sim, 1);
    assert!(sim.set_physics_world(PhysicsWorld::new(Vec::new(), needed).unwrap()).is_err());
    assert_eq!(sim.physics().body_count(), 2);
}

/// The replay suite's `fight`, read back as what it says it holds: a replay that passes on a script that
/// had stopped doing these would hold nothing.
#[test]
fn the_replay_suites_fight_plays_what_it_says() {
    let scenario = replay::find_scenario(&replay::SCENARIOS, "duel").expect("listed");
    let script = replay::find_script("fight").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    let (player, opponent) = (cast[0], cast[1]);

    let mut reactions = Vec::new();
    let mut started = 0;
    let (mut trades, mut armoured, mut finish_froze) = (0, 0, false);
    for tick in 0..u64::from(script.ticks) {
        sim.step(&(script.actions)(tick, &cast));
        let view = sim.actor(opponent).expect("alive");
        if view.moves.started != started {
            started = view.moves.started;
            reactions.push(view.moves.current.expect("just started").name);
        }

        let hits: Vec<_> = sim.cues().filter(|cue| cue.cue != DEFEATED && cue.cue != POISE_BROKEN).collect();
        let lands_on =
            |target| hits.iter().filter(|cue| cue.hit.is_some_and(|hit| hit.target == target)).count();
        trades += u32::from(lands_on(player) > 0 && lands_on(opponent) > 0);
        armoured += hits
            .iter()
            .filter(|cue| {
                cue.hit.is_some_and(|hit| hit.outcome == HitOutcome::Absorbed && hit.target == player)
            })
            .count();
        if sim.cues().any(|cue| cue.cue == DEFEATED) {
            sim.step(&(script.actions)(tick + 1, &cast));
            let view = sim.actor(opponent).expect("alive");
            reactions.push(view.moves.current.expect("its reaction").name);
            finish_froze = sim.actor(player).expect("alive").hitstop_ticks > 7;
            break;
        }
    }

    for reaction in ["flinch", "stagger", "knockdown", "dazed", "defeat"] {
        assert!(reactions.contains(&name(reaction)), "no {reaction} among {reactions:?}");
    }
    assert!(trades >= 2, "{trades}");
    assert!(armoured >= 1, "the player's armour took nothing");
    assert!(finish_froze, "a defeat freezes everyone past the blow's own hitstop");
    let stats = sim.stats();
    assert!(stats.poise_breaks >= 2, "{stats:?}");
    assert_eq!(stats.defeats, 1, "{stats:?}");
    assert!(sim.actor(opponent).expect("alive").fighter.is_defeated());
    assert!(!sim.actor(player).expect("alive").fighter.is_defeated());
}
