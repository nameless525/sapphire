//! An enemy's brain (M2 G6) through the public API. Most of these run the warrior's real moves with this
//! file's own brain section, whose numbers stay put when the shipped brain is tuned, so a tick worked
//! out by hand here stays worked out. A few hold the shipped brain to what its moves can do.

use sapphire_core::Name;
use sapphire_gameplay::replay::{self, Player, Replay, SCENARIOS, find_scenario};
use sapphire_gameplay::{
    ActorActions, ActorId, ActorSpawn, Brain, BrainState, Buttons, ControllerProfile, SimConfig, Simulation,
    Stick, TickActions,
};
use sapphire_math::Vec3;
use sapphire_moves::{
    Against, BrainAttackDocument, BrainDocument, BrainPace, BrainRiseDocument, MoveSet, MoveSetDocument,
    Reach, Recent, Spacing, TickRange,
};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};
use sapphire_time::SimDuration;

const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");
const SLICE: &str = include_str!("../../sapphire_moves/test_content/slice.moves.ron");

fn name(text: &str) -> Name {
    Name::intern(text)
}

/// The test moves, with one point of health.
fn replay_text_with_health_one() -> MoveSet {
    let frail = SLICE.replacen("health: 300", "health: 1", 1);
    MoveSetDocument::parse(&frail).expect("parses").compile().expect("compiles")
}

fn attack(intent: &str, weight: f32, reach: (f32, f32, f32)) -> BrainAttackDocument {
    BrainAttackDocument {
        intent: intent.to_owned(),
        weight,
        reach: Reach { near: reach.0, best: reach.1, far: reach.2 },
        against: Against::default(),
        recent: Recent::default(),
    }
}

/// This file's brain: a band of 1.6 to 2.6 m, and patience long enough that only a test wanting an attack
/// sees one.
fn brain_document() -> BrainDocument {
    BrainDocument {
        spacing: Spacing { near: 1.6, far: 2.6 },
        pace: BrainPace {
            approach: 0.7,
            retreat: 0.3,
            circle: 0.35,
            circle_share: 0.0,
            turn_rate_degrees: 300.0,
        },
        patience: TickRange { min: 3000, max: 3000 },
        rest: TickRange { min: 30, max: 30 },
        recover: TickRange { min: 10, max: 10 },
        attacks: vec![
            attack("swipe", 1.0, (0.0, 1.5, 2.7)),
            attack("overhead", 0.7, (1.0, 2.3, 3.2)),
            attack("lunge", 1.0, (3.0, 4.6, 7.0)),
        ],
        rise: Some(BrainRiseDocument {
            intent: "rise".to_owned(),
            during: "knockdown".to_owned(),
            from: 58,
            to: 72,
        }),
    }
}

/// The warrior's moves with this brain, as a cook writes them and the game reads them.
fn warrior_with(brain: BrainDocument) -> MoveSet {
    let mut document = MoveSetDocument::parse(WARRIOR).expect("parses");
    document.brain = Some(brain);
    let set = document.compile().expect("compiles");
    MoveSet::from_cooked(&set.to_cooked().expect("cooks")).expect("reads back")
}

/// A brain and its target, on flat ground and on their feet: its target is someone on the test moves.
struct Fight {
    sim: Simulation,
    brain: ActorId,
    target: ActorId,
}

impl Fight {
    /// The warrior at the origin facing +Z, and its target `distance` m along +Z facing it. One step has
    /// landed them, so a brain's first tick starts on the ground.
    fn new(brain: BrainDocument, distance: f32, seed: u64) -> Self {
        Self::with_set(warrior_with(brain), distance, seed)
    }

    fn with_set(set: MoveSet, distance: f32, seed: u64) -> Self {
        Self::with_sets(set, replay::test_moves().expect("compiles"), distance, seed)
    }

    fn with_sets(set: MoveSet, target_set: MoveSet, distance: f32, seed: u64) -> Self {
        let mut sim = Simulation::new(SimConfig { seed, max_actors: 4, ..SimConfig::default() });
        let floor = StaticCollider {
            id: ColliderId(0),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
            pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
            memberships: Group::World.mask(),
        };
        let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds");
        sim.set_physics_world(world).expect("room");
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("good"));
        let warrior = Some(sim.add_move_set(set));
        let others = Some(sim.add_move_set(target_set));
        let brain = sim
            .spawn(ActorSpawn {
                position: Vec3::ZERO,
                yaw: 0.0,
                moves: warrior,
                controller,
                ..ActorSpawn::default()
            })
            .expect("room");
        let target = sim
            .spawn(ActorSpawn {
                position: Vec3::new(0.0, 0.0, distance),
                yaw: std::f32::consts::PI,
                moves: others,
                controller,
                ..ActorSpawn::default()
            })
            .expect("room");
        sim.step(&TickActions::NEUTRAL);
        assert!(sim.give_brain(brain), "the warrior's set has a brain");
        Self { sim, brain, target }
    }

    fn step(&mut self) {
        self.sim.step(&TickActions::NEUTRAL);
    }

    fn press(&mut self, buttons: Buttons) {
        let mut actions = TickActions::NEUTRAL;
        actions.set(self.target, ActorActions { buttons, stick: Stick::CENTER }).expect("room");
        self.sim.step(&actions);
    }

    fn view(&self) -> sapphire_gameplay::ActorView {
        self.sim.actor(self.brain).expect("alive")
    }

    fn mind(&self) -> Brain {
        self.view().brain
    }

    fn distance(&self) -> f32 {
        let apart = self.sim.actor(self.target).expect("alive").position - self.view().position;
        (apart.x * apart.x + apart.z * apart.z).sqrt()
    }

    /// Steps until it's playing a move, and returns that move's name and the steps it took.
    fn until_it_moves(&mut self, limit: u32) -> (Name, u32) {
        for taken in 1..=limit {
            self.step();
            if let Some(current) = self.view().moves.current {
                return (current.name, taken);
            }
        }
        panic!("nothing started in {limit} steps: {:?}", self.mind());
    }
}

/// One line of a table: asserts within this file's tolerance for a worked distance, a tenth of a mm.
fn assert_near(what: &str, got: f32, want: f32) {
    assert!((got - want).abs() < 1e-4, "{what}: {got}, worked out {want}");
}

/// **Walking in.** A tilt of 0.7 quantizes to step 89 of 127, 0.70079, so on the default profile it's
/// `2 + (0.70079 − 0.5) / 0.4 × 4` = 4.00787 m/s. Speeding up by 1.5 m/s a tick (90 m/s²) gives tick
/// speeds of 1.5, 3 and 4.00787, and distances of `(before + after) / 2 / 60`: 0.0125, 0.0375, 0.05840,
/// so 0.10840 m after three ticks, then 0.06680 a tick. From 6 m, a brain reading the last step's
/// position still sees more than 2.6 m after 52 ticks (6 − 0.10840 − 49 × 0.06680 = 2.6185), walks once
/// more to 2.5517 (tick 53), and on tick 54 lets go. Slowing by 1 m/s a tick, it covers 0.05846,
/// 0.04180, 0.02513, 0.00846 and 0.00007 more, and rests at 2.4178.
#[test]
fn it_walks_in_and_stops_inside_its_spacing() {
    let mut fight = Fight::new(brain_document(), 6.0, 1);
    let mut positions = Vec::new();
    for _ in 0..80 {
        fight.step();
        positions.push(fight.view().position.z);
    }
    assert_near("tick 1", positions[0], 0.0125);
    assert_near("tick 2", positions[1], 0.05);
    assert_near("tick 3", positions[2], 0.10840);
    assert_near("tick 4", positions[3], 0.17520);
    assert_near("tick 52", positions[51], 6.0 - 2.6185);
    assert_near("tick 53", positions[52], 6.0 - 2.5517);
    assert_near("tick 54", positions[53], 6.0 - 2.5517 + 0.05846);
    assert_near("at rest", positions[79], 6.0 - 2.4178);
    assert_eq!(fight.view().brain.stick(), Stick::CENTER);
    assert_eq!(fight.mind().state(), BrainState::Pacing);
    assert_eq!(fight.view().yaw, 0.0, "it walked straight at its target, and never turned");
}

/// **Backing off.** Tilt 0.3 is step 38, 0.29921, under the walk's tilt, so `0.29921 / 0.5 × 2` = 1.19685
/// m/s. From 1 m it reads under 1.6 m until it's walked out to it and lets go: speed comes up within a
/// tick (1.19685 m/s), so 0.00997 m on the first tick and 0.01995 a tick after. It reads 1.6 m or more
/// after `(0.6 − 0.00997) / 0.01995` = 29.6 → 31 ticks, and walks one more. All the way, it faces its
/// target, and doesn't turn to walk.
#[test]
fn it_backs_off_from_inside_its_spacing_still_facing_its_target() {
    let mut fight = Fight::new(brain_document(), 1.0, 1);
    fight.step();
    assert_near("tick 1", fight.view().position.z, -0.00997);
    assert_eq!(fight.mind().stick(), Stick::new(0, -38));
    for _ in 1..30 {
        fight.step();
        assert_eq!(fight.mind().stick(), Stick::new(0, -38));
        assert_eq!(fight.view().yaw, 0.0);
    }
    for _ in 0..30 {
        fight.step();
        assert_eq!(fight.view().yaw, 0.0);
    }
    assert_eq!(fight.mind().stick(), Stick::CENTER);
    let rest = fight.distance();
    assert!((1.6..1.7).contains(&rest), "it came to rest inside the band: {rest}");
}

/// It holds a circle: inside the band every tick, going round its target, turning to keep facing it,
/// though the stick points across. With a share of 1 it always circles, one way or the other by a draw.
#[test]
fn it_circles_inside_the_band_facing_its_target() {
    let mut brain = brain_document();
    brain.pace.circle_share = 1.0;
    let mut fight = Fight::new(brain, 2.1, 1);
    let mut turned = 0.0_f32;
    let mut last_bearing = 0.0_f32;
    for tick in 0..240 {
        fight.step();
        let (view, target) = (fight.view(), fight.sim.actor(fight.target).expect("alive"));
        let distance = fight.distance();
        assert!((1.6..=2.6).contains(&distance), "tick {tick}: {distance} m");
        let bearing = sapphire_math::det::yaw_of_direction(target.position - view.position);
        // One tick at 1.39 m/s around 2 m moves its bearing about 0.012 rad, well under the turn rate.
        let off = sapphire_math::det::wrap_angle(view.yaw - bearing);
        assert!(off.abs() < 0.02, "tick {tick}: facing {off} rad off its target");
        turned += sapphire_math::det::wrap_angle(bearing - last_bearing);
        last_bearing = bearing;
        assert_ne!(fight.mind().stick(), Stick::CENTER, "tick {tick}");
    }
    // Four seconds at 1.39 m/s on a circle of about 2.1 m: 2.6 rad.
    assert!(turned.abs() > 2.3, "it went round: {turned} rad");
}

/// **Patience.** It's on its owner's clock: drawn 40 at a reading of 1, so it presses on the step that
/// reads 41, its 40th, and the move starts on that step.
#[test]
fn it_waits_out_its_patience_then_commits() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 40, max: 40 };
    brain.attacks = vec![attack("swipe", 1.0, (0.0, 1.5, 2.7))];
    let mut fight = Fight::new(brain, 2.0, 1);
    let (started, taken) = fight.until_it_moves(100);
    assert_eq!((started, taken), (name("swipe"), 40));
    assert_eq!(fight.mind().state(), BrainState::Attacking { serial: 1 });
    assert_eq!(fight.mind().stick(), Stick::CENTER);
}

/// The table's own arithmetic, by hand. At 2.1 m a swipe is `(2.7 − 2.1) / (2.7 − 1.5)` = 0.5, an overhead
/// `0.7 × (2.1 − 1) / (2.3 − 1)` = 0.59231, and a lunge is out of its reach at 0.
#[test]
fn a_score_is_its_weight_by_its_reach_by_its_target_by_its_recency() {
    use sapphire_moves::TargetDoing::{Free, Opening, Untouchable};
    use sapphire_time::SimDuration;

    let mut document = brain_document();
    document.attacks[0].against = Against { opening: 1.6, untouchable: 0.0, ..Against::default() };
    document.attacks[0].recent = Recent { ticks: 200, floor: 0.35 };
    let set = warrior_with(document);
    let def = set.brain().expect("it has one");
    let (swipe, overhead, lunge) = (def.attacks[0], def.attacks[1], def.attacks[2]);
    assert_near("swipe", swipe.score(2.1, Free, None), 0.5);
    assert_near("overhead", overhead.score(2.1, Free, None), 0.59231);
    assert_eq!(lunge.score(2.1, Free, None), 0.0);

    assert_near("an opening", swipe.score(2.1, Opening, None), 0.8);
    assert_eq!(swipe.score(2.1, Untouchable, None), 0.0);

    let ago = |ticks| Some(SimDuration::from_ticks(ticks));
    assert_near("just used", swipe.score(2.1, Free, ago(0)), 0.5 * 0.35);
    assert_near("half back", swipe.score(2.1, Free, ago(100)), 0.5 * 0.675);
    assert_near("all back", swipe.score(2.1, Free, ago(200)), 0.5);

    // At the edges of a reach, and past them.
    assert_eq!(lunge.score(3.0, Free, None), 0.0);
    assert_eq!(lunge.score(4.6, Free, None), 1.0);
    assert_eq!(lunge.score(7.0, Free, None), 0.0);
    assert_eq!(lunge.score(7.5, Free, None), 0.0);
    assert_eq!(lunge.score(f32::NAN, Free, None), 0.0);
}

/// Where only one attack scores, that's the one, whatever the seed.
#[test]
fn it_picks_the_attack_the_distance_calls_for() {
    for (distance, expected) in [(1.0, "swipe"), (2.9, "overhead"), (5.0, "lunge")] {
        for seed in 0..8 {
            let mut brain = brain_document();
            brain.patience = TickRange { min: 5, max: 5 };
            let mut fight = Fight::new(brain, distance, seed);
            let (started, _) = fight.until_it_moves(40);
            assert_eq!(started, name(expected), "{distance} m, seed {seed}");
        }
    }
}

/// Out of every attack's reach, it keeps walking, and draws nothing until one reaches.
#[test]
fn out_of_reach_it_walks_in_until_something_reaches() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 1, max: 1 };
    let mut fight = Fight::new(brain, 12.0, 3);
    let (started, _) = fight.until_it_moves(200);
    assert_eq!(started, name("lunge"));
    let at = 12.0 - fight.view().position.z;
    assert!((6.8..7.0).contains(&at), "at the edge of the lunge's reach: {at} m");
}

/// What the target is doing decides, each kind by what's on screen: each row's attack is the only one
/// its row scores, and patience comes up with the target where the row puts it.
#[test]
fn it_reads_what_its_target_is_doing() {
    const PATIENCE: u32 = 40;
    /// What each kind of doing scores: only the swipe scores against `doing`, and only the overhead scores
    /// against anything else.
    fn only(doing: fn(&mut Against) -> &mut f32) -> BrainDocument {
        let mut brain = brain_document();
        brain.patience = TickRange { min: PATIENCE, max: PATIENCE };
        let mut nothing = Against { free: 0.0, swinging: 0.0, opening: 0.0, reacting: 0.0, untouchable: 0.0 };
        let mut all = Against::default();
        *doing(&mut nothing) = 1.0;
        *doing(&mut all) = 0.0;
        brain.attacks[0].against = nothing;
        brain.attacks[1].against = all;
        brain
    }
    /// What a row's target does, on what step before patience is out.
    #[derive(Clone, Copy)]
    enum Does {
        Nothing,
        Press(u32, Buttons),
        React(u32, &'static str),
    }
    type Doing = fn(&mut Against) -> &mut f32;
    let rows: [(&str, Doing, Does); 5] = [
        ("standing", |a| &mut a.free, Does::Nothing),
        // `light_1` winds up for 5 ticks, then strikes for 3: 2 ticks in is winding up.
        ("winding up", |a| &mut a.swinging, Does::Press(PATIENCE - 2, Buttons::LIGHT)),
        // It's recovering after 8 ticks, and has 18 of that: 12 ticks in.
        ("coming out of it", |a| &mut a.opening, Does::Press(PATIENCE - 12, Buttons::LIGHT)),
        // Its reaction, forced as a hit would.
        ("reacting", |a| &mut a.reacting, Does::React(PATIENCE - 3, "flinch")),
        // A dodge's i-frames come after a tick of startup.
        ("dodging", |a| &mut a.untouchable, Does::Press(PATIENCE - 4, Buttons::DODGE)),
    ];
    for (what, doing, does) in rows {
        let mut fight = Fight::new(only(doing), 2.0, 1);
        let mut started = None;
        for step in 1..=PATIENCE {
            match does {
                Does::Press(at, buttons) if step == at => fight.press(buttons),
                Does::React(at, reaction) if step == at => {
                    assert!(fight.sim.force_move(fight.target, name(reaction)));
                    fight.step();
                }
                _ => fight.step(),
            }
            started = started.or(fight.view().moves.current.map(|current| current.name));
        }
        assert_eq!(started, Some(name("swipe")), "{what}");
    }
}

/// Recency: with no memory of a floor at all, an attack it just used scores nothing, so where two
/// score it alternates, whatever the draw says.
#[test]
fn it_doesnt_repeat_an_attack_it_just_used() {
    for seed in 0..4 {
        let mut brain = brain_document();
        brain.patience = TickRange { min: 1, max: 1 };
        brain.rest = TickRange { min: 1, max: 1 };
        brain.attacks.truncate(2);
        for attack in &mut brain.attacks {
            attack.reach = Reach { near: 0.0, best: 2.0, far: 9.0 };
            attack.recent = Recent { ticks: 100_000, floor: 0.0 };
        }
        let mut fight = Fight::new(brain, 2.0, seed);
        let mut seen = Vec::new();
        let mut last = 0;
        for _ in 0..600 {
            fight.step();
            let moves = fight.view().moves;
            if moves.started != last {
                last = moves.started;
                seen.push(moves.current.expect("just started").name);
            }
            if seen.len() == 2 {
                break;
            }
        }
        assert_eq!(seen.len(), 2, "seed {seed}");
        assert_ne!(seen[0], seen[1], "seed {seed}");
    }
}

/// With two attacks in reach, the draw decides: over many seeds both come up, more often the one that
/// scores higher, and the same seed comes up the same.
#[test]
fn the_draw_follows_the_scores() {
    let first_choice = |seed: u64| {
        let mut brain = brain_document();
        brain.patience = TickRange { min: 3, max: 3 };
        // 0.75 against 0.25.
        brain.attacks.truncate(2);
        brain.attacks[0] = attack("swipe", 0.75, (0.0, 2.0, 9.0));
        brain.attacks[1] = attack("overhead", 0.25, (0.0, 2.0, 9.0));
        Fight::new(brain, 2.0, seed).until_it_moves(20).0
    };
    let swipes = (0..200).filter(|&seed| first_choice(seed) == name("swipe")).count();
    assert!((130..=170).contains(&swipes), "{swipes} of 200, at odds of 3 to 1");
    for seed in 0..8 {
        assert_eq!(first_choice(seed), first_choice(seed), "seed {seed}");
    }
}

/// A seed's fight, as what it did when, for comparing whole runs.
fn fight_by_seed(seed: u64, ticks: u64) -> (Vec<(u64, Name)>, Vec<u64>) {
    let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let (mut sim, cast) = scenario.start(SimConfig { seed, ..scenario.config }).expect("sets up");
    let mut started = Vec::new();
    let mut checksums = Vec::new();
    let mut last = 0;
    for tick in 0..ticks {
        sim.step(&TickActions::NEUTRAL);
        checksums.push(sim.checksum());
        let view = sim.actor(cast[1]).expect("alive");
        if view.moves.started != last {
            last = view.moves.started;
            started.push((tick, view.moves.current.expect("just started").name));
        }
    }
    (started, checksums)
}

/// A seed changes what it does and when, and the same seed does the same, to the bit, at every tick.
#[test]
fn a_seed_changes_its_choices_and_replays_identically() {
    let (first, first_sums) = fight_by_seed(7, 1500);
    let (again, again_sums) = fight_by_seed(7, 1500);
    assert_eq!(first, again);
    assert_eq!(first_sums, again_sums);

    let others: Vec<_> = (8..12).map(|seed| fight_by_seed(seed, 1500).0).collect();
    assert!(others.iter().all(|other| *other != first), "{first:?}");
    let in_order = |run: &[(u64, Name)]| run.iter().map(|&(_, name)| name).collect::<Vec<_>>();
    assert!(others.iter().any(|other| in_order(other) != in_order(&first)), "the choices themselves differ");
    assert!(first.len() >= 4, "it did something: {first:?}");
}

/// **Ordering.** Three brains that differ, and draw from one generator, with the same IDs behind
/// different entities: histories that free the pool's entities in opposite orders. Going through a bare
/// query for its draws instead fails this, since identical actors would hide it and these aren't.
#[test]
fn the_order_brains_draw_in_is_their_ids_not_the_pools() {
    fn history(reverse: bool) -> Vec<u64> {
        let mut sim = Simulation::new(SimConfig { seed: 11, max_actors: 8, ..SimConfig::default() });
        let floor = StaticCollider {
            id: ColliderId(0),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(200.0, 0.5, 200.0) }),
            pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
            memberships: Group::World.mask(),
        };
        let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds");
        sim.set_physics_world(world).expect("room");
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("good"));
        let mut brain = brain_document();
        brain.patience = TickRange { min: 20, max: 70 };
        brain.pace.circle_share = 0.8;
        let moves = Some(sim.add_move_set(warrior_with(brain)));
        let at = |x: f32, z: f32, yaw: f32| ActorSpawn {
            position: Vec3::new(x, 0.0, z),
            yaw,
            moves,
            controller,
            ..ActorSpawn::default()
        };
        let spawned: Vec<ActorId> =
            (0..6).map(|n| sim.spawn(at(n as f32 * 3.0, 0.0, 0.0)).expect("room")).collect();
        let mut gone = [spawned[0], spawned[1], spawned[3]];
        if reverse {
            gone.reverse();
        }
        for id in gone {
            assert!(sim.despawn(id));
        }
        // The same IDs in the other order of entities, and every one of them different.
        let first = sim.spawn(at(0.0, 5.0, 0.3)).expect("room");
        let second = sim.spawn(at(-4.0, 1.0, 1.2)).expect("room");
        let third = sim.spawn(at(7.0, -3.0, 2.2)).expect("room");
        for brain in [first, second, third] {
            assert!(sim.give_brain(brain));
        }
        (0..900)
            .map(|_| {
                sim.step(&TickActions::NEUTRAL);
                sim.checksum()
            })
            .collect()
    }
    assert_eq!(history(false), history(true));
}

/// **A stagger.** While a reaction plays, it presses nothing and pushes nothing, whatever its patience
/// says, and it stands for its `recover` afterwards. `stagger_heavy` is 42 ticks, and forced, it starts on
/// the next step: reads that way from the step after, and stops reading that way 42 steps later.
#[test]
fn it_presses_nothing_while_it_reacts() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 30, max: 30 };
    let mut fight = Fight::new(brain, 2.0, 1);
    for _ in 0..10 {
        fight.step();
    }
    assert!(fight.sim.force_move(fight.brain, name("stagger_heavy")));
    let pressed = fight.sim.stats().intents_pressed;
    fight.step();
    assert_eq!(fight.view().moves.current.expect("forced").name, name("stagger_heavy"));
    let serial = fight.view().moves.current.expect("forced").serial;
    for tick in 1..42 {
        fight.step();
        assert_eq!(fight.mind().state(), BrainState::Reacting { serial, rise_at: None }, "tick {tick}");
        assert_eq!(fight.mind().stick(), Stick::CENTER, "tick {tick}");
        assert_eq!(fight.sim.stats().intents_pressed, pressed, "tick {tick}: patience had long run out");
    }
    fight.step();
    assert!(fight.view().moves.current.is_none(), "it ended");
    fight.step();
    let BrainState::Resting { .. } = fight.mind().state() else {
        panic!("stands once it's over: {:?}", fight.mind().state());
    };
    assert_eq!(fight.sim.stats().intents_pressed, pressed);
    // Its 10 ticks of `recover` go by, and its patience, long out, lets it swing at once.
    let (started, taken) = fight.until_it_moves(20);
    assert!([name("swipe"), name("overhead")].contains(&started), "{started:?}");
    assert_eq!(taken, 9, "the rest of its recover, one step gone, and then at once");
}

/// A reaction that hasn't started yet still counts: a hit with no hitstop forces it for the next step,
/// the very step patience could run out on, and a press then would read the reaction as its attack,
/// by its serial, so a knockdown would never rise.
#[test]
fn a_reaction_due_on_the_tick_it_would_press_stops_the_press() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 30, max: 30 };
    let mut fight = Fight::new(brain, 2.0, 1);
    for _ in 0..29 {
        fight.step();
    }
    assert_eq!(fight.mind().state(), BrainState::Pacing);
    assert!(fight.sim.force_move(fight.brain, name("knockdown")));
    let pressed = fight.sim.stats().intents_pressed;
    fight.step();
    assert_eq!(fight.sim.stats().intents_pressed, pressed, "no attack on that step");
    assert_eq!(fight.view().moves.current.expect("forced").name, name("knockdown"));
    fight.step();
    let BrainState::Reacting { rise_at: Some(_), .. } = fight.mind().state() else {
        panic!("it knows it's lying down: {:?}", fight.mind().state());
    };
    assert_eq!(fight.sim.stats().intents_pressed, pressed);
}

/// **Getting up.** It presses `rise` once, at a tick drawn inside the knockdown's last third: `recover`
/// starts there, and never before the window opens, on any seed. Left to the document's window of 58 to
/// 78, its 58 to 72 gives it room, so it never ends the move lying down.
#[test]
fn it_rises_inside_the_knockdowns_last_third() {
    let mut ticks = Vec::new();
    for seed in 0..32 {
        let mut fight = Fight::new(brain_document(), 2.0, seed);
        for _ in 0..5 {
            fight.step();
        }
        assert!(fight.sim.force_move(fight.brain, name("knockdown")));
        fight.step();
        let mut drawn = None;
        let mut rose = None;
        for _ in 0..100 {
            let before = fight.view().moves.current.expect("lying down").tick();
            fight.step();
            if let BrainState::Reacting { rise_at: Some(at), .. } = fight.mind().state() {
                drawn = Some(at);
            }
            let current = fight.view().moves.current.expect("lying down, then up");
            if current.name == name("recover") {
                rose = Some(before + 1);
                break;
            }
            assert_eq!(current.name, name("knockdown"));
        }
        let drawn = u64::from(drawn.expect("it drew a tick"));
        let rose = rose.unwrap_or_else(|| panic!("seed {seed}: never rose"));
        assert!((58..72).contains(&drawn), "seed {seed}: drew {drawn}");
        assert_eq!(rose, drawn, "seed {seed}: `recover` starts on the tick drawn");
        assert!(fight.view().moves.windows.invulnerable || fight.view().moves.current.is_some());
        ticks.push(drawn);
    }
    ticks.sort_unstable();
    ticks.dedup();
    assert!(ticks.len() > 3, "the draw varies it: {ticks:?}");
}

/// A poise break lying down still breaks: `stagger_heavy` replaces the knockdown, the press it had drawn
/// is forgotten, and what follows is a stand, not a rise. Real hits land here: a first swipe with the
/// warrior's poise of 12 and a meter of 10.
#[test]
fn a_poise_break_lying_down_replaces_the_knockdown_and_its_rise() {
    let mut document = MoveSetDocument::parse(WARRIOR).expect("parses");
    document.brain = Some(brain_document());
    document.fighter.as_mut().expect("a fighter").poise = 10;
    let set = document.compile().expect("compiles");

    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: 4, ..SimConfig::default() });
    let moves = Some(sim.add_move_set(set));
    let attacker = sim.spawn(ActorSpawn { moves, ..ActorSpawn::default() }).expect("room");
    // Close enough that a knockdown's 1.4 m back stays in reach.
    let at = Vec3::new(0.0, 0.0, 0.8);
    let lying = sim
        .spawn(ActorSpawn {
            position: at,
            yaw: std::f32::consts::PI,
            moves,
            controller: None,
            ..ActorSpawn::default()
        })
        .expect("room");
    assert!(sim.give_brain(lying));
    assert!(sim.force_move(lying, name("knockdown")));
    sim.step(&TickActions::NEUTRAL);
    sim.step(&TickActions::NEUTRAL);
    let BrainState::Reacting { rise_at: Some(_), .. } = sim.actor(lying).expect("alive").brain.state() else {
        panic!("{:?}", sim.actor(lying).expect("alive").brain.state());
    };
    assert!(sim.press_intent(attacker, name("swipe")));
    for _ in 0..40 {
        sim.step(&TickActions::NEUTRAL);
        if sim.stats().poise_breaks > 0 {
            break;
        }
    }
    assert_eq!(sim.stats().poise_breaks, 1, "{:?}", sim.stats());
    // Through the hit's freeze, and on.
    for _ in 0..8 {
        sim.step(&TickActions::NEUTRAL);
    }
    let view = sim.actor(lying).expect("alive");
    let current = view.moves.current.expect("it's playing the break");
    assert_eq!(current.name, name("stagger_heavy"));
    assert_eq!(view.brain.state(), BrainState::Reacting { serial: current.serial, rise_at: None });
    let mut steps = 0;
    while sim.actor(lying).expect("alive").moves.current.is_some() {
        sim.step(&TickActions::NEUTRAL);
        steps += 1;
        assert!(steps < 60, "it ends");
    }
    assert_eq!(
        sim.actor(lying).expect("alive").moves.started,
        2,
        "no `recover`: it wasn't lying down any more"
    );
}

/// **Defeat.** Real hits land: the blow takes its last health, it plays its `death`, and from then on
/// there's nothing to say: it presses nothing and pushes nothing, whatever its patience says, for good.
#[test]
fn it_stops_when_its_defeated() {
    let mut document = MoveSetDocument::parse(WARRIOR).expect("parses");
    let mut brain = brain_document();
    brain.patience = TickRange { min: 60, max: 60 };
    document.brain = Some(brain);
    document.fighter.as_mut().expect("a fighter").health = 1;
    let frail = document.compile().expect("compiles");

    let mut sim = Simulation::new(SimConfig { seed: 1, max_actors: 4, ..SimConfig::default() });
    let frail = Some(sim.add_move_set(frail));
    let attacker = sim.spawn(ActorSpawn { moves: frail, ..ActorSpawn::default() }).expect("room");
    let at = Vec3::new(0.0, 0.0, 1.5);
    let falling = sim
        .spawn(ActorSpawn {
            position: at,
            yaw: std::f32::consts::PI,
            moves: frail,
            controller: None,
            ..ActorSpawn::default()
        })
        .expect("room");
    assert!(sim.give_brain(falling));
    assert!(sim.press_intent(attacker, name("swipe")));
    while !sim.actor(falling).expect("alive").fighter.is_defeated() {
        assert!(sim.tick() < 100, "the swipe lands inside its window");
        sim.step(&TickActions::NEUTRAL);
    }
    while sim.actor(falling).expect("alive").hitstop_ticks > 0 {
        sim.step(&TickActions::NEUTRAL);
    }
    sim.step(&TickActions::NEUTRAL);
    sim.step(&TickActions::NEUTRAL);
    let view = sim.actor(falling).expect("alive");
    assert_eq!(view.brain.state(), BrainState::Defeated);
    assert_eq!(view.moves.current.expect("falling").name, name("death"));
    let pressed = sim.stats().intents_pressed;
    let started = view.moves.started;
    let position = view.position;
    for tick in 0..1000 {
        sim.step(&TickActions::NEUTRAL);
        let view = sim.actor(falling).expect("alive");
        assert_eq!(view.brain.state(), BrainState::Defeated, "tick {tick}");
        assert_eq!(view.brain.stick(), Stick::CENTER, "tick {tick}");
        if view.moves.current.is_none() {
            assert!(sim.intents(falling).expect("alive").is_empty(), "tick {tick}");
        }
    }
    assert_eq!(sim.stats().intents_pressed, pressed);
    let view = sim.actor(falling).expect("alive");
    assert_eq!(view.moves.started, started, "nothing after its death");
    assert!(view.position.distance(position) < 1.0, "only its death's own motion moved it");
}

/// With its target gone, it stands: nothing to walk at, and nothing to swing at.
#[test]
fn it_stands_once_its_target_is_gone() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 1, max: 1 };
    let mut fight = Fight::new(brain, 8.0, 1);
    fight.step();
    fight.step();
    assert_ne!(fight.mind().stick(), Stick::CENTER, "walking in");
    fight.sim.despawn(fight.target);
    for _ in 0..600 {
        fight.step();
    }
    let view = fight.view();
    assert_eq!(view.brain.stick(), Stick::CENTER);
    assert_eq!(view.moves.started, 0, "it swung at nothing");
}

/// With its target defeated, it stands too. A real swipe takes the target's one point of health, and
/// the warrior has no more to do, for as long as anyone watches.
#[test]
fn it_stands_once_its_target_is_defeated() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 5, max: 5 };
    let frail = replay_text_with_health_one();
    let mut fight = Fight::with_sets(warrior_with(brain), frail, 1.5, 1);
    while !fight.sim.actor(fight.target).expect("alive").fighter.is_defeated() {
        assert!(fight.sim.tick() < 200, "its first swing lands");
        fight.step();
    }
    assert_eq!(fight.sim.stats().defeats, 1);
    let started = fight.view().moves.started;
    for tick in 0..1200 {
        fight.step();
        assert_eq!(fight.view().moves.started, started, "tick {tick}: it swung at what's left");
    }
    assert_eq!(fight.mind().stick(), Stick::CENTER);
    assert_eq!(fight.mind().state(), BrainState::Pacing);
}

/// **Frozen.** Under hitstop, nothing runs down: its whole state is the same bits through the freeze, it
/// draws nothing, and it does what it would have done, exactly as many ticks later as it was frozen.
#[test]
fn hitstop_freezes_it_timers_and_all() {
    const FREEZE: u16 = 30;
    let brain = || {
        let mut brain = brain_document();
        brain.patience = TickRange { min: 40, max: 40 };
        brain
    };
    let mut plain = Fight::new(brain(), 2.0, 5);
    let (first, plain_taken) = plain.until_it_moves(100);

    let mut frozen = Fight::new(brain(), 2.0, 5);
    for _ in 0..10 {
        frozen.step();
    }
    let brain_before = frozen.mind();
    let position = frozen.view().position;
    frozen.sim.clock_mut(frozen.brain).expect("alive").0.apply_hitstop(FREEZE);
    for tick in 0..FREEZE {
        frozen.step();
        assert_eq!(frozen.view().clock_delta, SimDuration::ZERO, "tick {tick}");
        assert_eq!(frozen.mind(), brain_before, "tick {tick}: not a bit of it moves");
        assert_eq!(frozen.view().position, position, "tick {tick}");
    }
    let (second, frozen_taken) = frozen.until_it_moves(100);
    assert_eq!(second, first);
    assert_eq!(
        frozen_taken + 10 + u32::from(FREEZE),
        plain_taken + u32::from(FREEZE),
        "exactly that much later"
    );
}

/// **An ordinary player's path.** It doesn't touch the move system or the controller by the back door:
/// its stick arrives through `Controls` and its press through the ring, as a player's would. So the
/// world can see it in `Simulation::intents` and the view's stick.
#[test]
fn nothing_about_it_is_special_to_the_move_system() {
    let mut brain = brain_document();
    brain.patience = TickRange { min: 30, max: 30 };
    let mut fight = Fight::new(brain, 6.0, 1);
    fight.step();
    let entity_stick = fight.mind().stick();
    assert_eq!(entity_stick, Stick::new(0, 89), "toward its target, on the document's tilt");
    let (started, _) = fight.until_it_moves(100);
    let view = fight.view();
    assert_eq!(view.moves.started, 1);
    assert_eq!(started, view.moves.current.expect("running").name);
    assert_eq!(fight.sim.stats().moves_started, 1);
    assert!(fight.sim.stats().intents_pressed >= 1, "through the same counter a button goes through");
}

/// Without a brain section, a set can't be given one, and a live actor on one with a brain does nothing
/// until it's told who to fight. A brain has no effect on a checksum until then: no draws and no stick.
#[test]
fn a_brain_comes_only_from_a_set_that_has_one() {
    let mut sim = Simulation::new(SimConfig::default());
    let plain = Some(sim.add_move_set(replay::test_moves().expect("compiles")));
    let warrior = Some(sim.add_move_set(warrior_with(brain_document())));
    let (no_brain, has_one) = (
        sim.spawn(ActorSpawn { moves: plain, ..ActorSpawn::default() }).expect("room"),
        sim.spawn(ActorSpawn { moves: warrior, ..ActorSpawn::default() }).expect("room"),
    );
    assert!(!sim.give_brain(no_brain), "this set says nothing of how it fights");
    assert!(!sim.actor(no_brain).expect("alive").brain.is_on());
    assert!(!sim.actor(has_one).expect("alive").brain.is_on());
    let gone = ActorId::from_raw(99).expect("not zero");
    assert!(!sim.give_brain(gone), "nobody's there");
    assert!(sim.give_brain(has_one));
    let view = sim.actor(has_one).expect("alive");
    assert!(view.brain.is_on());
    assert_eq!(view.brain.target(), None, "it looks for a foe as it thinks");
}

/// **The shipped brain's reach:** each attack, pressed by the brain itself at its best distance and at the
/// edges of the band where it scores, lands on someone standing still. An attack whose reach claims more
/// than its volumes and warp can do would swing at air.
#[test]
fn each_shipped_attack_lands_from_where_it_says_it_suits() {
    let shipped = replay::warrior_moves().expect("compiles");
    let def = shipped.brain().expect("the warrior has a brain");
    for attack in &def.attacks {
        let intent = shipped.intents()[attack.intent() as usize].name();
        let (near, best, far) = (attack.reach.near, attack.reach.best, attack.reach.far);
        // Capsules keep them this far apart, so nearer than this can't be asked.
        let closest = near.max(0.7);
        for distance in [closest, (closest + best) * 0.5, best, (best + far) * 0.5, far - 0.05] {
            let mut document = MoveSetDocument::parse(WARRIOR).expect("parses");
            let mut brain = brain_document();
            brain.patience = TickRange { min: 1, max: 1 };
            brain.attacks = vec![BrainAttackDocument {
                intent: match intent {
                    i if i == name("swipe") => "swipe",
                    i if i == name("overhead") => "overhead",
                    _ => "lunge",
                }
                .to_owned(),
                weight: 1.0,
                reach: Reach { near: 0.0, best: distance, far: distance + 0.05 },
                against: Against::default(),
                recent: Recent::default(),
            }];
            document.brain = Some(brain);
            let mut fight = Fight::with_set(document.compile().expect("compiles"), distance, 1);
            let mut landed = false;
            for _ in 0..120 {
                fight.step();
                landed |= fight.sim.stats().hits_applied > 0;
            }
            assert!(landed, "{intent:?} at {distance} m: reach {near}, {best}, {far} and no hit");
        }
    }
}

/// The shipped brain presses only what its document can start, and gets up inside a window that takes it.
#[test]
fn the_shipped_brain_is_the_one_a_replay_runs_on() {
    let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    sim.step(&TickActions::NEUTRAL);
    let view = sim.actor(cast[1]).expect("alive");
    assert!(view.brain.is_on());
    assert_eq!(view.brain.target(), Some(cast[0]));
    let set = sim.move_library().current(view.moves.set.expect("a set")).expect("loaded");
    assert_eq!(set.brain(), replay::warrior_moves().expect("compiles").brain());
    assert!(!sim.actor(cast[0]).expect("alive").brain.is_on(), "the player's input is its own");
    assert!(!sim.actor(cast[0]).expect("alive").fighter.is_defeated());
}

/// A brain tuned differently is a different fight: a replay refuses it, by the hash it was recorded on.
#[test]
fn a_tuned_brain_changes_the_hash_a_replay_checks() {
    let hash = |patience: u32| {
        let mut sim = Simulation::new(SimConfig::default());
        let mut brain = brain_document();
        brain.patience = TickRange { min: patience, max: patience };
        sim.add_move_set(warrior_with(brain));
        sim.tables_hash()
    };
    assert_eq!(hash(40), hash(40));
    assert_ne!(hash(40), hash(41));
}

/// Plays a committed replay to its end, showing `each_tick` the simulation after each step.
fn play_committed(bytes: &[u8], mut each_tick: impl FnMut(&Simulation, &[ActorId])) {
    let replay = Replay::decode(bytes).expect("reads");
    let mut player = Player::new(replay, &SCENARIOS).expect("the moves are what it was recorded on");
    let cast = player.cast().to_vec();
    while player.step() {
        each_tick(player.simulation(), &cast);
    }
}

/// What a committed fight did.
#[derive(Default)]
struct Summary {
    enemy_moves: Vec<Name>,
    player_moves: Vec<Name>,
    hits_on_player: u32,
    hits_on_enemy: u32,
    player_defeated_at: Option<u64>,
    enemy_defeated_at: Option<u64>,
    /// Moves the warrior started after the player fell.
    enemy_moves_after_player_fell: u32,
}

fn summary_of(bytes: &[u8]) -> Summary {
    let mut summary = Summary::default();
    let (mut enemy_started, mut player_started, mut at_fall) = (0, 0, None);
    play_committed(bytes, |sim, cast| {
        let (player, enemy) = (sim.actor(cast[0]).expect("alive"), sim.actor(cast[1]).expect("alive"));
        if enemy.moves.started != enemy_started {
            enemy_started = enemy.moves.started;
            if let Some(current) = enemy.moves.current {
                summary.enemy_moves.push(current.name);
            }
        }
        if player.moves.started != player_started {
            player_started = player.moves.started;
            if let Some(current) = player.moves.current {
                summary.player_moves.push(current.name);
            }
        }
        for cue in sim.cues() {
            let Some(hit) = cue.hit else { continue };
            if cue.cue == sapphire_gameplay::DEFEATED || cue.cue == sapphire_gameplay::POISE_BROKEN {
                continue;
            }
            if hit.target == cast[0] {
                summary.hits_on_player += 1;
            } else {
                summary.hits_on_enemy += 1;
            }
        }
        if player.fighter.is_defeated() && summary.player_defeated_at.is_none() {
            summary.player_defeated_at = Some(sim.tick());
            at_fall = Some(enemy.moves.started);
        }
        if enemy.fighter.is_defeated() && summary.enemy_defeated_at.is_none() {
            summary.enemy_defeated_at = Some(sim.tick());
        }
        summary.enemy_moves_after_player_fell = at_fall.map_or(0, |at| enemy.moves.started - at);
    });
    summary
}

const STILL: &[u8] = include_bytes!("../../../tests/replays/arena-fight-still.sreplay");
const WON: &[u8] = include_bytes!("../../../tests/replays/arena-fight-won.sreplay");

/// The replay of someone standing still is that: it takes every hit there is, falls with the warrior
/// untouched, with the telegraphed heavy and the quick cut among them, a knockdown and a break, and the
/// warrior then stands, with nothing left to do.
#[test]
fn the_replay_of_a_player_standing_still_plays_what_it_says() {
    let summary = summary_of(STILL);
    assert_eq!(summary.hits_on_enemy, 0);
    assert!(summary.player_moves.iter().all(|mv| {
        [name("flinch"), name("stagger"), name("knockdown"), name("dazed"), name("defeat")].contains(mv)
    }));
    assert!(summary.player_moves.contains(&name("knockdown")), "{:?}", summary.player_moves);
    assert!(summary.player_moves.contains(&name("dazed")), "{:?}", summary.player_moves);
    assert_eq!(summary.player_moves.last(), Some(&name("defeat")));
    assert!(summary.enemy_moves.contains(&name("swipe")), "{:?}", summary.enemy_moves);
    assert!(summary.enemy_moves.contains(&name("overhead")), "{:?}", summary.enemy_moves);
    let fell = summary.player_defeated_at.expect("it fell");
    assert!(summary.hits_on_player >= 5, "{}", summary.hits_on_player);
    assert!(summary.enemy_defeated_at.is_none());
    assert_eq!(summary.enemy_moves_after_player_fell, 0, "it fell at tick {fell}, and that ended it");
    let replay = Replay::decode(STILL).expect("reads");
    assert!(u64::from(replay.ticks()) > fell + 300, "it runs on past the end, for the standing");
}

/// The replay of someone fighting back is that: the string, dodges and dash attacks, hits both ways, the
/// warrior's close attacks tried, each of its reactions played, and the warrior falls, with the player
/// standing.
#[test]
fn the_replay_of_a_player_fighting_back_plays_what_it_says() {
    let summary = summary_of(WON);
    for mv in ["light_1", "light_2", "light_3", "dodge", "dash_attack"] {
        assert!(summary.player_moves.contains(&name(mv)), "no {mv}: {:?}", summary.player_moves);
    }
    assert!(summary.hits_on_player >= 1, "it landed nothing on the player: a fight has two sides");
    assert!(summary.hits_on_enemy >= 10, "{}", summary.hits_on_enemy);
    assert!(summary.player_defeated_at.is_none(), "the player wins this one");
    assert!(summary.enemy_defeated_at.is_some(), "the warrior falls");
    for attack in ["swipe", "overhead"] {
        assert!(summary.enemy_moves.contains(&name(attack)), "no {attack}: {:?}", summary.enemy_moves);
    }
    for reaction in ["stagger_light", "stagger_heavy", "death"] {
        assert!(summary.enemy_moves.contains(&name(reaction)), "no {reaction}: {:?}", summary.enemy_moves);
    }
}

/// A dodge on the player's real moves, timed by nothing but luck, meets the warrior's swing: the world
/// slows, and a press answers with the dash attack sooner than a dodge's own cancel window opens.
#[test]
fn a_perfect_dodge_of_the_warrior_opens_a_counter_into_the_dash_attack() {
    let scenario = find_scenario(&SCENARIOS, "arena-fight").expect("listed");
    let (mut sim, cast) = scenario.start(scenario.config).expect("sets up");
    let press = |buttons| {
        let mut actions = TickActions::NEUTRAL;
        actions.set(cast[0], ActorActions { buttons, stick: Stick::CENTER }).expect("fits");
        actions
    };
    for tick in 0..3600_u64 {
        sim.step(&press(if tick % 30 == 0 { Buttons::DODGE } else { Buttons::NONE }));
        if !sim.cues().any(|cue| cue.cue == sapphire_gameplay::PERFECT_DODGE && cue.actor == cast[0]) {
            continue;
        }
        let dodge = sim.actor(cast[0]).expect("alive").moves.current.expect("its dodge");
        assert_eq!(dodge.name, name("dodge"));
        for _ in 0..2 {
            sim.step(&press(Buttons::LIGHT));
        }
        let answer = sim.actor(cast[0]).expect("alive").moves.current.expect("its answer");
        assert_eq!(answer.name, name("dash_attack"));
        let into = answer.started_at.since(dodge.started_at);
        assert!(into < SimDuration::from_ticks(9), "a dodge's own cancel opens at 9: {into:?}");
        return;
    }
    panic!("a minute of dodging met no swing");
}

/// Its fighter's reactions are plain moves: the brain is the only thing that presses, and everything
/// the document's intents can do comes by the same ring.
#[test]
fn every_intent_a_brain_can_press_has_no_button() {
    let shipped = replay::warrior_moves().expect("compiles");
    let def = shipped.brain().expect("a brain");
    let rise = def.rise.expect("it gets up").intent();
    for at in def.attacks.iter().map(|attack| attack.intent()).chain([rise]) {
        assert_eq!(shipped.intents()[at as usize].button, None);
    }
}
