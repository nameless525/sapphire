//! The enemy's moves (M2 O4, `content/combat/enemy/warrior.moves.ron`) played in the simulation, as
//! a brain and hits will drive them: each move plays out cleanly, every reaction forces from every
//! phase of every attack, and each hit volume reaches about as far as the document says.

use sapphire_core::Name;
use sapphire_gameplay::replay::compile_document;
use sapphire_gameplay::{ActorId, ActorSpawn, CurrentMove, SimConfig, Simulation, TickActions, WarpTarget};
use sapphire_math::{Vec3, det};
use sapphire_moves::{MoveSet, MoveSetDocument, PathSample, Phase, Shape, Weight};

const WARRIOR: &str = include_str!("../../../content/combat/enemy/warrior.moves.ron");

const ATTACKS: [&str; 3] = ["swipe", "overhead", "lunge"];
const REACTIONS: [&str; 5] = ["stagger_light", "stagger_heavy", "knockdown", "recover", "death"];

fn name(text: &str) -> Name {
    Name::intern(text)
}

fn warrior() -> MoveSet {
    let document = MoveSetDocument::parse(WARRIOR).expect("the document parses");
    let set = document.compile().expect("the document compiles");
    MoveSet::from_cooked(&set.to_cooked().expect("cooks")).expect("reads back")
}

/// Live tuning (M2 T2): the warrior's moves are keyed by hand, so a saved timing over what's running is
/// exactly what a build with it in its document plays, and saving it unedited is no edit.
#[test]
fn a_saved_timing_is_what_a_build_with_it_would_play() {
    let bytes = |set: &MoveSet| set.to_cooked().expect("writes").into_vec();
    let running = warrior();
    let unchanged = compile_document(WARRIOR, Some(&running)).expect("it compiles");
    assert_eq!(bytes(&unchanged.set), bytes(&running));

    let mut document = MoveSetDocument::parse(WARRIOR).expect("the document parses");
    for attack in ATTACKS {
        let mv = document.moves.iter_mut().find(|mv| mv.name == attack).expect("there");
        mv.startup += 1;
        mv.recovery -= 1;
        for hitbox in &mut mv.hitboxes {
            hitbox.from += 1;
            hitbox.to += 1;
        }
    }
    let text = document.to_text().expect("writes");
    let live = compile_document(&text, Some(&running)).expect("no cook needed");
    assert!(!live.motion_as_cooked, "nothing here names a clip");
    assert_eq!(bytes(&live.set), bytes(&document.compile().expect("it compiles")));
    assert_ne!(bytes(&live.set), bytes(&running));
}

/// One warrior at the origin, facing +Z.
fn lone() -> (Simulation, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = sim.add_move_set(warrior());
    let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).expect("room");
    (sim, actor)
}

fn step(sim: &mut Simulation) {
    sim.step(&TickActions::NEUTRAL);
}

fn current(sim: &Simulation, actor: ActorId) -> Option<CurrentMove> {
    sim.actor(actor).expect("alive").moves.current
}

/// Presses `attack` and steps until it's running.
fn start(sim: &mut Simulation, actor: ActorId, attack: &str) {
    assert!(sim.press_intent(actor, name(attack)), "{attack} is an intent of the set");
    step(sim);
    let mv = current(sim, actor).unwrap_or_else(|| panic!("{attack} started from neutral"));
    assert_eq!(mv.name, name(attack));
}

/// Steps until nothing is running, and returns the steps it took, failing if it never ends.
fn play_out(sim: &mut Simulation, actor: ActorId, limit: u32) -> u32 {
    for steps in 0..limit {
        if current(sim, actor).is_none() {
            return steps;
        }
        step(sim);
    }
    panic!("still running after {limit} steps: {:?}", current(sim, actor));
}

/// How far from the origin, along the ground, a volume on `sample` reaches when its root is at
/// `position`: the far end of a capsule along its path's +Y, or a sphere's edge.
fn ground_reach(position: Vec3, yaw: f32, sample: PathSample, shape: Shape) -> f32 {
    let center = Vec3::from(sample.translation);
    let rotation = sapphire_math::Quat::from_array(sample.rotation);
    let (axis, radius) = match shape {
        Shape::Capsule { radius, length } => ((rotation * Vec3::Y) * length * 0.5, radius),
        Shape::Sphere { radius } => (Vec3::ZERO, radius),
        Shape::Box { half_extents } => (Vec3::ZERO, Vec3::from(half_extents).length()),
    };
    [center + axis, center - axis]
        .into_iter()
        .map(|end| {
            let world = position + det::rotate_y(end, yaw);
            (world.x * world.x + world.z * world.z).sqrt() + radius
        })
        .fold(0.0, f32::max)
}

/// Every attack from neutral, with no target: the farthest its volume reaches along the ground from
/// where the warrior stood.
fn reach_of(attack: &str) -> f32 {
    let (mut sim, actor) = lone();
    start(&mut sim, actor, attack);
    let mut reach = 0.0_f32;
    let mut seen = false;
    while current(&sim, actor).is_some() {
        let view = sim.actor(actor).expect("alive");
        for volume in view.moves.volumes.iter() {
            seen = true;
            reach = reach.max(ground_reach(view.position, view.yaw, volume.end, volume.shape));
        }
        step(&mut sim);
    }
    assert!(seen, "{attack} published a volume");
    reach
}

#[test]
fn the_document_declares_what_a_brain_and_hits_will_ask_for() {
    let document = MoveSetDocument::parse(WARRIOR).expect("parses");
    for attack in ATTACKS {
        let intent = document.intents.iter().find(|intent| intent.name == attack).expect(attack);
        assert_eq!(intent.button, None, "{attack}: a brain presses it, so no button does");
        assert!(document.neutral.iter().any(|allow| allow.intent == attack && allow.target == attack));
    }
    let set = warrior();
    for reaction in REACTIONS {
        assert!(set.find(name(reaction)).is_some(), "{reaction} is a move");
    }
}

/// A telegraph has to be readable: the heavy and the lunge wind up long and say so on their first
/// tick, the heavy longest, and every attack says when the blade starts to cut.
#[test]
fn telegraphs_are_long_and_cued_at_their_start() {
    let set = warrior();
    let startup = |attack: &str| set.find(name(attack)).expect(attack).1.startup;
    let (heavy, lunge, quick) = (startup("overhead"), startup("lunge"), startup("swipe"));
    assert!(heavy >= 45 && lunge >= 25, "wind-ups of at least 750 and 417 ms: {heavy}, {lunge}");
    assert!(heavy > lunge && lunge > quick, "the heavy telegraphs longest: {heavy}, {lunge}, {quick}");
    for (attack, windup) in [("swipe", "swing_windup"), ("overhead", "telegraph"), ("lunge", "telegraph")] {
        let (mut sim, actor) = lone();
        assert!(sim.press_intent(actor, name(attack)));
        step(&mut sim);
        let cues: Vec<Name> = sim.cues().map(|event| event.cue).collect();
        assert!(cues.contains(&name(windup)), "{attack}: {windup} on the step it starts");
        let mut swung = None;
        while let Some(mv) = current(&sim, actor) {
            if sim.cues().any(|event| event.cue == name("swing")) {
                swung = Some(mv.phase);
            }
            step(&mut sim);
        }
        assert_eq!(swung, Some(Phase::Active), "{attack}: `swing` as the blade starts to cut");
    }
}

/// Every move runs from its first tick to its last and leaves the warrior free, where its root
/// motion said, with every authored cue fired once.
#[test]
fn every_move_plays_out_cleanly() {
    let set = warrior();
    for attack in ATTACKS {
        let (mut sim, actor) = lone();
        let (_, mv) = set.find(name(attack)).expect(attack);
        let mut phases = Vec::new();
        assert!(sim.press_intent(actor, name(attack)));
        step(&mut sim);
        while let Some(running) = current(&sim, actor) {
            if phases.last() != Some(&running.phase) {
                phases.push(running.phase);
            }
            step(&mut sim);
        }
        assert_eq!(phases, [Phase::Startup, Phase::Active, Phase::Recovery], "{attack}");
        let end = sim.actor(actor).expect("alive").position;
        let root = mv.tables.root_at(sapphire_time::SimDuration::from_ticks(u64::from(mv.ticks())));
        assert!((end.z - root.translation[2]).abs() < 1e-3, "{attack}: ended at {end}");
        assert!(sim.press_intent(actor, name("swipe")));
        step(&mut sim);
        assert_eq!(current(&sim, actor).expect("free").name, name("swipe"), "{attack} left it free");
    }
    for reaction in REACTIONS {
        let (mut sim, actor) = lone();
        let (_, mv) = set.find(name(reaction)).expect(reaction);
        assert!(sim.force_move(actor, name(reaction)));
        step(&mut sim);
        assert_eq!(current(&sim, actor).expect("forced").name, name(reaction));
        let taken = play_out(&mut sim, actor, mv.ticks() + 2);
        assert!(taken.abs_diff(mv.ticks()) <= 1, "{reaction}: {taken} steps for {} ticks", mv.ticks());
    }
}

/// G3 can force any reaction out of any phase of any attack, or out of another reaction: it starts
/// on the next step, its volumes end with the attack, and it plays out as it does from neutral.
#[test]
fn reactions_force_from_any_phase() {
    let set = warrior();
    let phase_ticks = |attack: &str| {
        let (_, mv) = set.find(name(attack)).expect(attack);
        [mv.startup / 2, mv.startup + mv.active / 2, mv.startup + mv.active + mv.recovery / 2]
    };
    for attack in ATTACKS {
        for tick in phase_ticks(attack) {
            for reaction in REACTIONS {
                let (mut sim, actor) = lone();
                start(&mut sim, actor, attack);
                while current(&sim, actor).expect("running").tick() < u64::from(tick) {
                    step(&mut sim);
                }
                assert!(sim.force_move(actor, name(reaction)));
                step(&mut sim);
                let view = sim.actor(actor).expect("alive");
                let running = view.moves.current.expect("forced");
                assert_eq!(running.name, name(reaction), "{attack} at {tick}");
                assert!(view.moves.volumes.iter().next().is_none(), "{attack} at {tick}: its cut ended");
                let (_, mv) = set.find(name(reaction)).expect(reaction);
                play_out(&mut sim, actor, mv.ticks() + 2);
            }
        }
    }
    let (mut sim, actor) = lone();
    assert!(sim.force_move(actor, name("knockdown")));
    step(&mut sim);
    assert!(sim.force_move(actor, name("death")));
    step(&mut sim);
    assert_eq!(current(&sim, actor).expect("forced").name, name("death"), "a reaction over a reaction");
}

/// A knockdown gets up when pressed to, only once it has lain there; and a dead warrior can't be hit.
#[test]
fn a_knockdown_rises_on_its_intent_and_a_death_stays_out_of_reach() {
    let (mut sim, actor) = lone();
    assert!(sim.force_move(actor, name("knockdown")));
    step(&mut sim);
    while current(&sim, actor).expect("lying").tick() < 58 {
        assert_eq!(current(&sim, actor).expect("lying").name, name("knockdown"));
        step(&mut sim);
    }
    assert!(sim.press_intent(actor, name("rise")));
    step(&mut sim);
    assert_eq!(current(&sim, actor).expect("rising").name, name("recover"));
    assert!(sim.actor(actor).expect("alive").moves.windows.invulnerable, "not hit while getting up");

    let (mut sim, actor) = lone();
    assert!(sim.force_move(actor, name("death")));
    step(&mut sim);
    let mut steps = 0;
    while current(&sim, actor).is_some() {
        let view = sim.actor(actor).expect("alive");
        assert!(view.moves.windows.invulnerable, "step {steps}");
        assert!(view.moves.hurtboxes.iter().next().is_some(), "step {steps}: its body is off");
        step(&mut sim);
        steps += 1;
    }
}

/// Each hit volume reaches about the distance the document's comments say, from where the warrior
/// stood: what G6's brain will pick attacks by.
#[test]
fn hit_volumes_reach_the_distance_authored() {
    for (attack, authored) in [("swipe", 2.0), ("overhead", 2.3), ("lunge", 4.3)] {
        let reach = reach_of(attack);
        assert!((reach - authored).abs() < 0.2, "{attack}: reaches {reach} m, authored {authored} m");
    }
}

/// The lunge closes on a target within its warp, stopping short of it, and a target beyond the warp's
/// limit stays out of reach; the swipe's wind-up tracks a target that moved to its side.
#[test]
fn warps_bend_toward_a_target_within_their_limits() {
    let lunge_end = |target: Vec3| {
        let (mut sim, actor) = lone();
        assert!(sim.set_warp_target(actor, WarpTarget::Point(target)));
        start(&mut sim, actor, "lunge");
        while current(&sim, actor).is_some_and(|mv| mv.tick() < 38) {
            step(&mut sim);
        }
        sim.actor(actor).expect("alive").position
    };
    // 1.4 m short of a target 5 m away is 3.6 m on: about what the table moves anyway, within reach.
    let near = lunge_end(Vec3::new(0.0, 0.0, 5.0));
    assert!((near.z - 3.6).abs() < 0.1, "stops short of it: {near}");
    // 5.5 m is the most it can move in the table's 3 m and the warp's 2.5 m.
    let far = lunge_end(Vec3::new(0.0, 0.0, 12.0));
    assert!((5.0..5.8).contains(&far.z), "stays within the warp's limit: {far}");
    // Close by, it holds back instead of running through.
    let close = lunge_end(Vec3::new(0.0, 0.0, 2.0));
    assert!(close.z < 1.0, "stops short of a target it started beside: {close}");

    let (mut sim, actor) = lone();
    assert!(sim.set_warp_target(actor, WarpTarget::Point(Vec3::new(3.0, 0.0, 3.0))));
    start(&mut sim, actor, "swipe");
    while current(&sim, actor).is_some_and(|mv| mv.tick() < 24) {
        step(&mut sim);
    }
    let yaw = sim.actor(actor).expect("alive").yaw;
    assert!((yaw - std::f32::consts::FRAC_PI_4).abs() < 0.1, "turned to it through the wind-up: {yaw}");
}

/// Two warriors a swipe's reach apart, the second facing the first, with the first pressing its swipe.
fn swiping() -> (Simulation, ActorId, ActorId) {
    let mut sim = Simulation::new(SimConfig::default());
    let moves = Some(sim.add_move_set(warrior()));
    let attacker = sim.spawn(ActorSpawn { moves, ..ActorSpawn::default() }).expect("room");
    let at = Vec3::new(0.0, 0.0, 1.5);
    let target = sim
        .spawn(ActorSpawn { position: at, yaw: std::f32::consts::PI, moves, controller: None })
        .expect("room");
    assert!(sim.press_intent(attacker, name("swipe")));
    (sim, attacker, target)
}

/// Steps until a hit has landed, failing if the swipe's window passes with none.
fn until_it_lands(sim: &mut Simulation) {
    for _ in 0..30 {
        step(sim);
        if sim.stats().hits_applied > 0 {
            return;
        }
    }
    panic!("no hit landed: {:?}", sim.stats());
}

/// G3 reads the reactions by name: a hit by weight, a break and a defeat each play one of this set's.
#[test]
fn its_fighter_names_its_own_reactions_and_every_attack_does_something() {
    let set = warrior();
    let fighter = set.fighter().expect("hits can land on it");
    let reactions = &fighter.reactions;
    assert_eq!(reactions.flinch(Weight::Light), Some(name("stagger_light")));
    assert_eq!(reactions.flinch(Weight::Medium), Some(name("stagger_light")));
    assert_eq!(reactions.flinch(Weight::Heavy), Some(name("knockdown")));
    assert_eq!(reactions.poise_break(), Some(name("stagger_heavy")));
    assert_eq!(reactions.defeat(), Some(name("death")));
    for attack in ATTACKS {
        let (_, mv) = set.find(name(attack)).expect(attack);
        assert!(
            !mv.hitboxes.is_empty() && mv.hitboxes.iter().all(|hitbox| hitbox.attack.is_some()),
            "{attack}"
        );
    }
}

#[test]
fn a_swipe_lands_on_a_warrior_and_it_reacts() {
    let (mut sim, attacker, target) = swiping();
    until_it_lands(&mut sim);
    let view = sim.actor(target).expect("alive");
    assert_eq!(view.fighter.health(), view.fighter.max_health() - 20);
    assert_eq!(view.fighter.poise(), view.fighter.max_poise() - 12);
    assert_eq!(view.hitstop_ticks, 5);
    assert_eq!(sim.actor(attacker).expect("alive").hitstop_ticks, 5);
    step(&mut sim);
    assert_eq!(current(&sim, target).expect("reacting").name, name("stagger_light"));
}

/// The telegraphed attacks aren't stopped by a hit under 20 poise, which leaves a dodge to answer them
/// with, and a flinch isn't started over by one: a swipe's 12 takes its numbers and that's all. The
/// swipe itself has no such thing, so a hit can still beat it to the punch.
#[test]
fn a_light_hit_doesnt_stop_a_telegraph_or_start_a_flinch_over_and_does_cut_a_swipe() {
    for (mv, stops) in [("overhead", false), ("lunge", false), ("stagger_light", false), ("swipe", true)] {
        let (mut sim, _, target) = swiping();
        for _ in 0..20 {
            step(&mut sim);
        }
        assert!(sim.force_move(target, name(mv)));
        until_it_lands(&mut sim);
        let view = sim.actor(target).expect("alive");
        assert_eq!(view.fighter.health(), view.fighter.max_health() - 20, "{mv}: its numbers");
        assert_eq!(view.fighter.poise(), view.fighter.max_poise() - 12, "{mv}");
        let stats = sim.stats();
        assert_eq!(
            (stats.hits_absorbed, stats.reactions_forced),
            (u64::from(!stops), u64::from(stops)),
            "{mv}"
        );
        step(&mut sim);
        let now = current(&sim, target).expect("running").name;
        assert_eq!(now, name(if stops { "stagger_light" } else { mv }), "{mv}");
    }
}

/// Lying down or broken, a hit takes its numbers and plays no reaction, so neither can be cut short.
#[test]
fn a_hit_cant_cut_a_knockdown_or_a_break_short() {
    for reaction in ["knockdown", "stagger_heavy"] {
        let (mut sim, attacker, target) = swiping();
        for _ in 0..20 {
            step(&mut sim);
        }
        assert!(sim.force_move(target, name(reaction)));
        until_it_lands(&mut sim);
        let stats = sim.stats();
        assert_eq!((stats.hits_absorbed, stats.reactions_forced), (1, 0), "{reaction}");
        assert_eq!(current(&sim, target).expect("still down").name, name(reaction));
        assert_eq!(sim.actor(attacker).expect("alive").hitstop_ticks, 5);
        assert_eq!(sim.actor(target).expect("alive").hitstop_ticks, 0, "{reaction}: armour lets it run");
    }
}
