//! A boss's phases (M3 B3) through the public API: the hit that crosses one changes the brain it names and
//! says the phase's name as a cue, a brain given late is the one its phase names, and a script's
//! `SetBrain` reaches the same place. These run the warrior's real moves on a floor, with a kind of enemy
//! of this file's own, on so little health that one swipe crosses two phases.

use sapphire_core::Name;
use sapphire_gameplay::replay::{self, Player, Replay, SCENARIOS};
use sapphire_gameplay::{
    ActorId, ActorSpawn, ControllerProfile, Faction, MoveSetId, ProfileId, SimConfig, Simulation, TickActions,
};
use sapphire_math::Vec3;
use sapphire_moves::{EnemyDocument, EnemyPhaseDocument, MoveSetDocument};
use sapphire_physics::{ColliderId, Group, PhysicsWorld, Pose, Shape, StaticCollider, StaticShape};

const SIDE_A: Faction = Faction::new(1).unwrap();
const SIDE_B: Faction = Faction::new(2).unwrap();

/// The warrior as a boss: its brain, and a fiercer one its second phase names, on 40 points of health.
fn boss_kind(sim: &mut Simulation) -> MoveSetId {
    let mut moves = MoveSetDocument::parse(replay::WARRIOR_MOVES).expect("parses");
    moves.fighter.as_mut().expect("it fights").health = 40;
    let mut enemy = EnemyDocument::parse(replay::WARRIOR_ENEMY).expect("parses");
    let mut fierce = enemy.brains[&enemy.brain].clone();
    fierce.patience.min = 1;
    fierce.patience.max = 2;
    enemy.brains.insert("fierce".to_owned(), fierce);
    let phase = |at, name: Option<&str>, brain: Option<&str>| EnemyPhaseDocument {
        at,
        name: name.map(str::to_owned),
        brain: brain.map(str::to_owned),
    };
    enemy.phases = vec![
        phase(750, None, None),
        phase(500, Some("enraged"), Some("fierce")),
        phase(0, Some("fallen"), None),
    ];
    let mind = enemy.compile(&moves).expect("compiles").mind;
    let set = sim.add_move_set(moves.compile().expect("compiles"));
    sim.add_enemy(set, mind).expect("its brains fit its moves");
    set
}

struct Floor {
    sim: Simulation,
    boss: MoveSetId,
    warrior: MoveSetId,
    controller: Option<ProfileId>,
}

impl Floor {
    fn new() -> Self {
        let mut sim = Simulation::new(SimConfig::default());
        let floor = StaticCollider {
            id: ColliderId(0),
            shape: StaticShape::Convex(Shape::Box { half_extents: Vec3::new(50.0, 0.5, 50.0) }),
            pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
            memberships: Group::World.mask(),
        };
        let world = PhysicsWorld::new(vec![floor], sim.body_capacity()).expect("builds");
        sim.set_physics_world(world).expect("room");
        let controller = Some(sim.add_controller_profile(&ControllerProfile::default()).expect("good"));
        let warrior = replay::add_warrior(&mut sim).expect("compiles");
        let boss = boss_kind(&mut sim);
        Self { sim, boss, warrior, controller }
    }

    fn spawn(&mut self, moves: MoveSetId, z: f32, faction: Faction) -> ActorId {
        let spawn = ActorSpawn {
            position: Vec3::new(0.0, 0.0, z),
            yaw: 0.0,
            moves: Some(moves),
            controller: self.controller,
            faction,
        };
        self.sim.spawn(spawn).expect("room")
    }

    fn mind_of(&self, actor: ActorId) -> u8 {
        self.sim.actor(actor).expect("alive").brain.mind()
    }

    fn health_of(&self, actor: ActorId) -> u32 {
        self.sim.actor(actor).expect("alive").fighter.health()
    }

    /// A warrior facing `boss` from a step in front swipes until the boss's health moves, and the cues of
    /// the step it moved on.
    fn hit(&mut self, boss: ActorId) -> Vec<(ActorId, Name)> {
        let swinger = self.spawn(self.warrior, -1.0, SIDE_B);
        let was = self.health_of(boss);
        for _ in 0..200 {
            if self.sim.actor(swinger).expect("alive").moves.current.is_none() {
                self.sim.force_move(swinger, Name::new("swipe"));
            }
            self.sim.step(&TickActions::NEUTRAL);
            if self.health_of(boss) != was {
                let cues = self.sim.cues().map(|cue| (cue.actor, cue.cue)).collect();
                self.sim.despawn(swinger);
                return cues;
            }
        }
        panic!("the swipe never landed");
    }
}

#[test]
fn the_hit_that_crosses_a_phase_changes_the_brain_it_names_and_says_its_name() {
    let mut floor = Floor::new();
    let boss = floor.spawn(floor.boss, 0.0, SIDE_A);
    assert!(floor.sim.give_brain(boss));
    assert_eq!(floor.mind_of(boss), 0, "its first brain");
    let fierce = floor.sim.move_library().mind(floor.boss).expect("a kind").brain_named(Name::new("fierce"));

    let cues = floor.hit(boss);
    assert_eq!(floor.health_of(boss), 20, "half its health: past 750 and 500 at once");
    assert_eq!(Some(floor.mind_of(boss)), fierce, "the brain the second phase names");
    let named: Vec<_> = cues.iter().filter(|(actor, _)| *actor == boss).map(|(_, cue)| *cue).collect();
    assert_eq!(named, [Name::new("enraged")], "the named phase it crossed, as the boss's own cue");
    let phases = floor.sim.move_library().phases(Some(floor.boss));
    assert_eq!(phases.phase_of(20, 40), Some(2), "what a script's on_phase is told");
}

#[test]
fn a_fighter_given_a_brain_past_a_phase_gets_the_brain_it_names() {
    let mut floor = Floor::new();
    let boss = floor.spawn(floor.boss, 0.0, SIDE_A);
    floor.hit(boss);
    assert_eq!(floor.mind_of(boss), 0, "a fighter not thinking keeps no brain");
    assert!(floor.sim.give_brain(boss));
    let fierce = floor.sim.move_library().mind(floor.boss).expect("a kind").brain_named(Name::new("fierce"));
    assert_eq!(Some(floor.mind_of(boss)), fierce);
}

#[test]
fn a_brain_is_set_by_name_and_a_name_its_kind_hasnt_got_does_nothing() {
    let mut floor = Floor::new();
    let boss = floor.spawn(floor.boss, 0.0, SIDE_A);
    assert!(floor.sim.give_brain(boss));
    let before = floor.sim.checksum();
    assert!(floor.sim.set_brain(boss, Name::new("fierce")));
    assert_ne!(floor.mind_of(boss), 0);
    assert_ne!(floor.sim.checksum(), before, "a fighter's brain is in its checksum");
    assert!(!floor.sim.set_brain(boss, Name::new("timid")), "its kind hasn't got one");
    assert!(floor.sim.set_brain(boss, Name::new("steady")));
    assert_eq!(floor.mind_of(boss), 0);
    assert_eq!(floor.sim.checksum(), before, "and back where it was");
}

/// The committed boss fight is what its golden shows: the player takes the boss past half health, where it
/// enrages, once, saying so, and fights on with its fiercer brain.
#[test]
fn the_recorded_boss_fight_enrages_its_boss_once() {
    let bytes = include_bytes!("../../../tests/replays/arena-boss.sreplay");
    let mut player =
        Player::new(Replay::decode(bytes).expect("reads"), &SCENARIOS).expect("recorded on these");
    let boss = player.cast()[1];
    let set = player.simulation().actor(boss).expect("there").moves.set.expect("a set");
    let library = player.simulation().move_library();
    let mind = library.mind(set).expect("a kind of enemy");
    assert_eq!(mind.boss(), Some("boss-captain"));
    let fierce = mind.brain_named(Name::new("fierce")).expect("it has one");
    let (mut enraged, mut fierce_at) = (Vec::new(), None);
    while player.step() {
        let sim = player.simulation();
        let tick = sim.tick();
        enraged.extend(
            sim.cues().filter(|cue| cue.actor == boss && cue.cue == Name::new("enraged")).map(|_| tick),
        );
        let view = sim.actor(boss).expect("there");
        if fierce_at.is_none() && view.brain.mind() == fierce {
            fierce_at = Some((tick, view.fighter.health() * 2 <= view.fighter.max_health()));
        }
    }
    assert_eq!(enraged.len(), 1, "once: {enraged:?}");
    assert_eq!(fierce_at, Some((enraged[0], true)), "on the step it crossed half its health");
}

#[test]
fn a_fighter_of_no_kind_of_enemy_has_quarters() {
    let floor = Floor::new();
    let quarters = floor.sim.move_library().phases(None);
    assert_eq!(quarters.phase_of(75, 100), Some(1));
    assert_eq!(quarters.phase_of(76, 100), Some(0));
    assert_eq!(quarters.phase_of(0, 100), Some(4));
    assert_eq!(quarters.phase_of(0, 0), None, "a fighter with no health has no phases");
    let warrior = floor.sim.move_library().phases(Some(floor.warrior));
    for health in [240, 180, 179, 120, 60, 0] {
        assert_eq!(warrior.phase_of(health, 240), quarters.phase_of(health, 240), "{health}");
    }
}
