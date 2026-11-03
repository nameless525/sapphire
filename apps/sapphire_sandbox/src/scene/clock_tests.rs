//! Every clock a frame reads, through a real fight (review WA-3): a frame `alpha` of the way between its
//! last two steps reads screen effects, particles, the hour and shown time as it reads a pose, each on its
//! own clock, so nothing leads the blow it accents. Each part is tested where it lives; this holds the
//! rule where it counts, from a recorded fight's hit to what the frame's packet says.

use std::path::PathBuf;

use sapphire_cues::CueTable;
use sapphire_gameplay::{ActorId, CueEvent, HitCue};
use sapphire_math::Vec2;
use sapphire_render::VfxFrame;
use sapphire_time::SimDuration;
use sapphire_vfx::{EffectLibrary as VfxLibrary, VfxBudget};

use super::tests::arena_stage;
use super::tests::clips::{GRAPH_CLIP, clip, fighting};
use super::*;
use crate::combat::{Combat, no_bindings};
use crate::cues::CueShow;
use crate::options::CombatOption;

const TICK: f32 = 1.0 / TICK_HZ as f32;

/// A frame's reads between two steps, in the order frames come.
const ALPHAS: [f32; 3] = [0.25, 0.5, 1.0];

/// How fast the probes' particles fly, in metres a second, so where one is says how old it's drawn.
const FLIGHT: f32 = 2.0;

/// The world's probe, and the attacker's: told apart by colour.
const WORLD_COLOR: [f32; 3] = [1.0, 0.0, 0.0];
const ACTOR_COLOR: [f32; 3] = [0.0, 1.0, 0.0];

/// How many seconds the probe layers' ramps take from nothing to all they show, so what they show says
/// how old the frame draws them.
const RAMP: f32 = 60.0;

/// The most a read may differ from the rule, in seconds: a hundredth of a tick, which rounding is well
/// inside and a tick's lead is far outside.
const CLOSE: f32 = TICK * 0.01;

/// One long-lived particle that flies straight along +Y of its placement, in `color`.
fn probe_emitter(name: &str, color: [f32; 3]) -> String {
    let [r, g, b] = color;
    format!(
        "(name: \"{name}\", spawn: Burst(count: 1), lifetime: (50.0, 50.0), \
         motion: (speed: ({FLIGHT:?}, {FLIGHT:?})), size: [(0.0, 0.1)], color: [(0.0, ({r:?}, {g:?}, {b:?}))])"
    )
}

/// Two effects of a single particle: one on the world's clock, and one on the attacker's.
fn probe_effects() -> VfxLibrary {
    let (world, actor) = (probe_emitter("flies", WORLD_COLOR), probe_emitter("flies", ACTOR_COLOR));
    VfxLibrary::from_ron(&format!(
        "(type: \"sapphire::vfx::EffectLibrary\", version: 1, data: (effects: [\
         (name: \"world\", emitters: [{world}]), (name: \"actor\", emitters: [{actor}])]))"
    ))
    .expect("valid")
}

/// A preset whose layers ramp evenly for as long as any fight here runs: one on each of a layer's clocks.
fn probe_presets() -> EffectLibrary {
    EffectLibrary::from_ron(&format!(
        "(type: \"sapphire::world::EffectLibrary\", version: 1, data: (presets: [(name: \"probe\", layers: [\
         (radial_blur: (strength: 1.0), envelope: Keys([(0.0, 0.0), ({RAMP:?}, 1.0)])), \
         (inversion: (amount: 1.0), envelope: Keys([(0.0, 0.0), ({RAMP:?}, 1.0)]), clock: Real)])]))"
    ))
    .expect("valid")
}

/// Fires the probes on the cue `cue`, which has to carry a hit: nothing else fires.
fn probe_table(cue: &str) -> CueTable {
    CueTable::from_ron(&format!(
        "(type: \"sapphire::cues::CueTable\", version: 1, data: (rows: [(cue: \"{cue}\", \
         effects: [(effect: \"world\"), (effect: \"actor\", clock: Actor)], \
         screen: Some((preset: \"probe\", center: Preset)))]))"
    ))
    .expect("valid")
}

/// A recorded fight stepping beside an arena, as `Stage::step` runs them, the probes firing on `cue`.
struct Fight {
    scene: Scene,
    combat: Combat,
}

impl Fight {
    fn new(replay: &str, cue: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/replays")
            .join(format!("{replay}.sreplay"));
        let combat =
            Combat::new(&CombatOption::Replay { path, to_end: true }).expect("a committed recording");
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        let presets = probe_presets();
        let show = CueShow::with_table(probe_table(cue), probe_effects(), VfxBudget::DESKTOP_LOW, &presets)
            .expect("the table finds them");
        scene.cues = Some(show);
        scene.ecs.world_mut().insert_resource(presets);
        Self { scene, combat }
    }

    /// One fixed step, in the order the app and a capture run one.
    fn step(&mut self) {
        self.scene.step();
        self.combat.step(&mut no_bindings);
        self.scene.follow_simulation(self.combat.simulation(), self.combat.cast());
        self.scene.show_cues(self.combat.simulation());
        self.scene.end_cue_frame(TICK, TICK, 1.0, &mut ());
    }

    fn simulation(&self) -> &Simulation {
        self.combat.simulation()
    }

    /// The first of this step's cues named `cue`, with its hit.
    fn hit_cue(&self, cue: &str) -> Option<(CueEvent, HitCue)> {
        let name = sapphire_core::Name::new(cue);
        self.simulation().cues().find(|event| event.cue == name).and_then(|event| Some((event, event.hit?)))
    }

    /// Steps up to the first hit of `cue`, with a frame on each step, as a fight has, so nothing a frame
    /// owes is left to the step before.
    fn run_to_first_hit(&mut self, cue: &str) -> (CueEvent, HitCue) {
        for _ in 0..2000 {
            self.step();
            if let Some(hit) = self.hit_cue(cue) {
                return hit;
            }
            let _ = self.scene.frame_packet(1.0, Some((64, 36)));
            self.scene.end_frame();
        }
        panic!("no {cue} within 2000 ticks");
    }

    /// Where the attacker's pose draws at `alpha`, in ticks on its own clock, by what its graph is given.
    fn pose_ticks(&mut self, attacker: ActorId, alpha: f32) -> f64 {
        let index = self.combat.cast().iter().position(|id| *id == attacker).expect("in the cast");
        let arena = self.scene.arena.as_mut().expect("an arena");
        arena.graph_input(index, alpha).expect("a fight is on").ticks
    }

    /// The simulation's world clock at `alpha`, in ticks.
    fn world_ticks(&self, alpha: f32) -> f64 {
        self.simulation().world().resource::<WorldClock>().0.presented_ticks(alpha)
    }

    /// One frame's packet at `alpha`, read as ages: see [`Drawn`].
    fn drawn(&mut self, point: Vec3, alpha: f32) -> Drawn {
        let packet = self.scene.frame_packet(alpha, Some((64, 36)));
        let effects = packet.post.effects;
        let drawn = Drawn {
            world_layer: effects.radial_blur.strength * RAMP,
            real_layer: effects.inversion.amount * RAMP,
            world_particle: age_of(&packet.vfx, WORLD_COLOR, point),
            actor_particle: age_of(&packet.vfx, ACTOR_COLOR, point),
            shown_seconds: packet.time_seconds,
        };
        self.scene.end_frame();
        drawn
    }
}

/// How old a frame draws things, in seconds of each one's own clock.
#[derive(Clone, Copy, Debug, Default)]
struct Drawn {
    world_layer: f32,
    real_layer: f32,
    world_particle: f32,
    actor_particle: f32,
    shown_seconds: f64,
}

/// How far the probe in `color` has flown from `point`, as a time.
fn age_of(frame: &VfxFrame, color: [f32; 3], point: Vec3) -> f32 {
    let color = Vec3::from(color);
    let sprite = frame.sprites.iter().find(|sprite| sprite.color == color).expect("the probe's particle");
    sprite.position.distance(point) / FLIGHT
}

fn close(read: f32, rule: f64, what: &str, frame: &str) {
    assert!(
        (f64::from(read) - rule).abs() < f64::from(CLOSE),
        "{what} reads {read} s, not {rule} s: {frame}"
    );
}

/// What happens to one hit from the step it lands on: which clocks stood still, or ran slow, and whether
/// every frame read every clock the way it reads the pose.
#[derive(Clone, Copy, Debug, Default)]
struct Watched {
    frames: u32,
    /// Steps on which the attacker's clock stood still while the world's ran on.
    attacker_frozen: u32,
    /// Steps on which the world's clock ran slower than the real one.
    world_slowed: u32,
}

/// From the frame of a hit on, every read at every alpha is the rule's, to a hundredth of a tick, until
/// the next hit or `steps` have gone. It's the frame's clock against the fighters' own: a layer or a
/// particle on the world's clock against that clock read at `alpha`, one on the attacker's against the
/// time its pose is drawn at, and a real layer against the steps gone by, each from the hit on, none
/// before it. What the hit starts reads from where it begins, whatever the frame's alpha.
fn watch(replay: &str, cue: &str, steps: u32) -> Watched {
    let mut fight = Fight::new(replay, cue);
    let (event, hit) = fight.run_to_first_hit(cue);
    let attacker = event.actor;
    let tick = fight.simulation().tick();
    let (world_at_hit, pose_at_hit) = (fight.world_ticks(1.0), fight.pose_ticks(attacker, 1.0));
    let ticks_to_seconds = |ticks: f64| ticks.max(0.0) / f64::from(TICK_HZ);
    let mut watched = Watched::default();
    let mut last = Drawn::default();
    for step in 0..=steps {
        if step > 0 {
            fight.step();
            if fight.hit_cue(cue).is_some() {
                break;
            }
        }
        let world_delta = fight.simulation().world().resource::<WorldClock>().0.last_delta();
        let actor = fight.simulation().actor(attacker).expect("still there");
        watched.frames += 1;
        watched.attacker_frozen += u32::from(step > 0 && actor.clock_delta == SimDuration::ZERO);
        watched.world_slowed += u32::from(step > 0 && world_delta < SimDuration::from_ticks(1));
        for alpha in ALPHAS {
            let pose = fight.pose_ticks(attacker, alpha);
            let world = fight.world_ticks(alpha);
            let drawn = fight.drawn(hit.point, alpha);
            let frame = format!(
                "{step} steps after tick {tick}, alpha {alpha}: world step {world_delta:?}, \
                 attacker's {:?}, pose {} ticks from the hit: {drawn:?}",
                actor.clock_delta,
                pose - pose_at_hit,
            );
            let real = f64::from(step) - f64::from(1.0 - alpha);
            close(drawn.world_layer, ticks_to_seconds(world - world_at_hit), "the world's layer", &frame);
            close(drawn.world_particle, ticks_to_seconds(world - world_at_hit), "the world's spark", &frame);
            close(drawn.real_layer, ticks_to_seconds(real), "the real layer", &frame);
            close(drawn.actor_particle, ticks_to_seconds(pose - pose_at_hit), "the attacker's spark", &frame);
            // The first frame shows them all at the same tick, the pose not past it.
            if step == 0 {
                assert_eq!(drawn.world_layer, 0.0, "{frame}");
                assert_eq!(drawn.real_layer, 0.0, "{frame}");
                assert_eq!(drawn.world_particle, 0.0, "{frame}");
                assert_eq!(drawn.actor_particle, 0.0, "{frame}");
                assert!(pose <= pose_at_hit, "{frame}");
            }
            // Frozen, the attacker's spark and pose hold there to the bit, at every alpha.
            if actor.clock_delta == SimDuration::ZERO {
                assert_eq!(pose, time_ticks(actor.clock_now), "{frame}");
                assert_eq!(drawn.actor_particle, last.actor_particle, "{frame}, after {last:?}");
            }
            // And nothing a frame shows steps back.
            assert!(drawn.world_layer >= last.world_layer, "{frame}, after {last:?}");
            assert!(drawn.world_particle >= last.world_particle - CLOSE, "{frame}, after {last:?}");
            assert!(drawn.actor_particle >= last.actor_particle - CLOSE, "{frame}, after {last:?}");
            assert!(drawn.shown_seconds >= last.shown_seconds, "{frame}, after {last:?}");
            last = drawn;
        }
    }
    watched
}

fn time_ticks(time: sapphire_time::SimTime) -> f64 {
    crate::anim_graph::time_ticks(time)
}

/// A hit landing through a real fight (G3's `fight`): its first frame shows the effects and the pose at
/// one tick, its hitstop holds what's on the attacker's clock with the pose while the world's effects
/// burn on, and they go on together when it thaws.
#[test]
fn a_hit_shows_with_its_pose_and_stays_with_it_through_hitstop() {
    let watched = watch("fight", "hit_landed", 40);
    assert!(watched.attacker_frozen > 0, "the hit's hitstop is in the watch: {watched:?}");
    assert!(watched.attacker_frozen < watched.frames - 1, "and so is the thaw: {watched:?}");
}

/// The same through G4's slow motion (`dodge-counter`): a counter landing in a world slowed by the
/// perfect dodge that opened it, whose screen effect and world spark slow down together with the
/// simulation's own clock, though the screen effect is on the scene's.
#[test]
fn a_hit_shows_with_its_pose_and_stays_with_it_through_slow_motion() {
    let watched = watch("dodge-counter", "hit_counter", 40);
    assert!(watched.world_slowed > 0, "the slow motion is in the watch: {watched:?}");
    assert!(watched.attacker_frozen > 0, "and so is the hit's hitstop: {watched:?}");
}

/// Shown time never steps back through a whole fight, in frames that come as a window's do, late in each
/// step and then early in the next: slow motion's first step used to take it back by most of a tick.
#[test]
fn shown_time_never_steps_back_through_a_fight() {
    let mut fight = Fight::new("dodge-counter", "hit_counter");
    fight.scene.time_of_day_mut().clock_mut().set_day_length(Some(240.0));
    let world_ticks = fight.simulation().world().resource::<WorldClock>().0.ticks();
    assert_eq!(world_ticks, 0);
    let (mut shown, mut hours) = (f64::MIN, 0.0_f32);
    let mut slowed = 0;
    for _ in 0..1000 {
        fight.step();
        let clock = &fight.scene.ecs.world().resource::<WorldClock>().0;
        slowed += u32::from(clock.last_delta() < SimDuration::from_ticks(1));
        for alpha in [0.0, 0.9] {
            let seconds = fight.scene.frame_packet(alpha, Some((64, 36))).time_seconds;
            let hour = fight.scene.time_of_day().frame().hours;
            fight.scene.end_frame();
            assert!(seconds >= shown, "alpha {alpha}: {seconds} after {shown}");
            assert!(hour >= hours, "alpha {alpha}: the hour {hour} after {hours}");
            (shown, hours) = (seconds, hour);
        }
    }
    assert!(slowed > 0, "the fight's slow motion reached the clock these read");
}

/// The simulation's clock and the scene's take the same steps through a fight's slow motion, and the
/// scene's own is left as it found it, whoever else slowed it.
#[test]
fn the_scenes_world_clock_keeps_with_a_fights_slow_motion() {
    let mut fight = Fight::new("dodge-counter", "hit_counter");
    let mut slowed = 0;
    for _ in 0..1000 {
        fight.step();
        let scene = fight.scene.ecs.world().resource::<WorldClock>().0.last_delta();
        let fought = fight.simulation().world().resource::<WorldClock>().0.last_delta();
        assert_eq!(scene, fought, "tick {}", fight.simulation().tick());
        slowed += u32::from(fought < SimDuration::from_ticks(1));
    }
    assert!(slowed > 0, "the fight slowed");
    assert!(
        fight.scene.fight_slow_motion.is_none(),
        "and it ended within the recording: {:?}",
        fight.scene.fight_slow_motion
    );
    assert_eq!(fight.scene.ecs.world().resource::<WorldClock>().0.scales().len(), 0);

    // One the scene's own controls pushed isn't the fight's to take off.
    let mut fight = Fight::new("dodge-counter", "hit_counter");
    fight.scene.apply(SimActions { toggle_slow_motion: true, ..SimActions::default() });
    for _ in 0..1000 {
        fight.step();
    }
    assert_eq!(fight.scene.ecs.world().resource::<WorldClock>().0.scales().len(), 1);
}

/// A hit's shake is on the real clock, and starts from where it begins too: its view is the unshaken one
/// until what's drawn gets to the hit, however long the frame before took.
#[test]
fn a_hits_shake_starts_when_the_pose_gets_there() {
    let table = |shaking: bool| {
        let shake = if shaking { ", shake: Some((preset: \"hit_heavy\"))" } else { "" };
        CueTable::from_ron(&format!(
            "(type: \"sapphire::cues::CueTable\", version: 1, data: (rows: [(cue: \"hit_landed\"{shake})]))"
        ))
        .expect("valid")
    };
    // A frame a step, each halfway between that step and the one before, on the director's own view.
    let views = |shaking: bool| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/fight.sreplay");
        let mut combat =
            Combat::new(&CombatOption::Replay { path, to_end: true }).expect("a committed recording");
        let (mut scene, _) = arena_stage(ArenaShot::Director);
        let presets = EffectLibrary::default_effects();
        let show = CueShow::with_table(
            table(shaking),
            VfxLibrary::default_effects(),
            VfxBudget::DESKTOP_LOW,
            &presets,
        )
        .expect("the table finds it");
        scene.cues = Some(show);
        let mut views = Vec::new();
        let mut landed = None;
        for frame in 0..400 {
            scene.step();
            scene.drive_arena_camera(TICK, Vec2::ZERO, 0.0);
            combat.step(&mut no_bindings);
            scene.follow_simulation(combat.simulation(), combat.cast());
            scene.show_cues(combat.simulation());
            scene.end_cue_frame(TICK, TICK, 0.5, &mut ());
            let hit = combat.simulation().cues().any(|cue| cue.hit.is_some());
            if hit && landed.is_none() {
                landed = Some(frame);
            }
            let view = scene.frame_packet(0.5, Some((64, 36))).main_view().expect("a view").camera.view;
            views.push(view);
            scene.end_frame();
        }
        (views, landed.expect("the fight lands a hit"))
    };
    let (still, landed) = views(false);
    let (shaken, also) = views(true);
    assert_eq!(landed, also);
    assert_eq!(&still[..=landed], &shaken[..=landed], "up to and including the frame it lands on");
    assert_ne!(still[landed + 1], shaken[landed + 1], "and the frame the pose gets there it's begun");
}

/// A fight with a sword in hand, its graphs posing both fighters, playing the shipped effects from a table
/// that fires none: all the trails here are the blades' own.
fn armed(replay: &str, table: &ClipTable<'_>) -> (Scene, Combat) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/replays")
        .join(format!("{replay}.sreplay"));
    let combat = Combat::new(&CombatOption::Replay { path, to_end: true }).expect("a committed recording");
    let (mut scene, ..) = fighting(table);
    scene.spawn_blade(MeshHandle::from_raw(6), Mat4::IDENTITY);
    let presets = probe_presets();
    let cues = CueTable::from_ron(
        "(type: \"sapphire::cues::CueTable\", version: 1, data: (rows: [(cue: \"never\", \
         effects: [(effect: \"hit_spark\")])]))",
    )
    .expect("valid");
    let show = CueShow::with_table(cues, VfxLibrary::default_effects(), VfxBudget::DESKTOP_LOW, &presets)
        .expect("the table finds them");
    scene.cues = Some(show);
    scene.ecs.world_mut().insert_resource(presets);
    (scene, combat)
}

/// One fixed step of it, in the order the app and a capture run one.
fn armed_step(scene: &mut Scene, combat: &mut Combat) {
    scene.step();
    combat.step(&mut no_bindings);
    scene.follow_simulation(combat.simulation(), combat.cast());
    scene.show_cues(combat.simulation());
    scene.end_cue_frame(TICK, TICK, 1.0, &mut ());
}

/// Where the frame's packet has its ribbons end, and where the world draws each blade.
struct TrailFrame {
    heads: Vec<(Vec3, Vec3)>,
    blades: [Option<Mat4>; 2],
    points: Vec<sapphire_render::VfxRibbonPoint>,
}

fn trail_frame(scene: &mut Scene, table: &ClipTable<'_>, alpha: f32) -> TrailFrame {
    scene.animate_fighters(table, table, alpha);
    let packet = scene.frame_packet(alpha, Some((64, 36)));
    let points = packet.vfx.ribbon_points.clone();
    scene.end_frame();
    let heads = points.iter().filter(|point| point.ends).map(|point| (point.base, point.tip)).collect();
    let world = scene.ecs.world();
    let drawn = |entity| world.get::<RenderTransform>(entity).map(|transform| transform.0);
    let player = scene.blade.and_then(|held| drawn(held.entity));
    let enemy = scene.arena.as_ref().and_then(|arena| drawn(arena.enemy_blade()));
    TrailFrame { heads, blades: [player, enemy], points }
}

/// A trail leaves the sword as drawn: what a frame's packet ends a ribbon on is what the same packet draws
/// the blade at, base and tip, at every alpha between a fight's steps, through its hitstops. There, nothing
/// of the strip moves at any alpha. And a new fight forgets it.
#[test]
fn a_trail_leaves_the_sword_as_drawn_and_a_hitstop_holds_it() {
    let bytes = clip().to_cooked().expect("cooked");
    let cooked = sapphire_anim::Clip::read(&bytes).expect("a valid clip");
    let clips = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());
    let (mut scene, mut combat) = armed("arena-fight-won", &clips);
    let (mut drawn_on, mut began, mut held) = ([0; 2], [None; 2], 0);
    for _ in 0..combat.ticks_left().expect("a recording") {
        armed_step(&mut scene, &mut combat);
        let sim = combat.simulation();
        let frozen = combat
            .cast()
            .iter()
            .all(|id| sim.actor(*id).is_none_or(|actor| actor.clock_delta == SimDuration::ZERO));
        let before = frozen.then(|| trail_frame(&mut scene, &clips, 1.0).points);
        for alpha in ALPHAS {
            let frame = trail_frame(&mut scene, &clips, alpha);
            if let Some(before) = &before {
                assert_eq!(&frame.points, before, "alpha {alpha}");
                held += usize::from(!before.is_empty());
            }
            // A strip has two samples once the blade has moved from where its trail began. Until a frame
            // closes it, it ends where the blade's drawn, among any others still fading.
            let rails = [crate::trails::player_rails, crate::trails::warrior_rails];
            for place in 0..2 {
                let running = scene.cues.as_ref().expect("on").trail_runs_at(place);
                let Some(blade) = frame.blades[place].filter(|_| running) else {
                    began[place] = None;
                    continue;
                };
                let (base, tip) = rails[place](blade);
                let (_, began_tip) = *began[place].get_or_insert((base, tip));
                if began_tip.distance(tip) > 2e-3 {
                    let head = frame.heads.iter().find(|(head_base, head_tip)| {
                        head_base.distance(base) < 2e-3 && head_tip.distance(tip) < 2e-3
                    });
                    assert!(
                        head.is_some(),
                        "alpha {alpha}, fighter {place}: {base} {tip} in {:?}",
                        frame.heads
                    );
                    drawn_on[place] += 1;
                }
            }
        }
    }
    assert!(drawn_on[0] > 20, "the sword's: {drawn_on:?}");
    assert!(drawn_on[1] > 10, "the warrior's: {drawn_on:?}");
    assert!(held > 0, "a hitstop with a strip showing");

    // Starting over: the tick goes back, the world's effects and what the trails hold go with it.
    let (mut scene, mut combat) = armed("arena-fight-won", &clips);
    while !scene.cues.as_ref().expect("on").trails_are_running() {
        armed_step(&mut scene, &mut combat);
        let _ = trail_frame(&mut scene, &clips, 1.0);
    }
    let (_, mut combat) = {
        drop(combat);
        armed("arena-fight-won", &clips)
    };
    scene.step();
    combat.step(&mut no_bindings);
    scene.follow_simulation(combat.simulation(), combat.cast());
    assert!(!scene.cues.as_ref().expect("on").trails_are_running(), "the last fight's trail has gone");
    assert!(trail_frame(&mut scene, &clips, 1.0).points.is_empty(), "and so has its strip");
}
