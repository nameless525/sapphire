//! A move, played: a real run of the simulation against a dummy, kept step by step.
//!
//! The preview doesn't work out where a hit volume would be. It starts a [`Simulation`] with the set the
//! document compiled to, forces the move on a character with a dummy in front of it, and keeps what every
//! step's public read says: roots, the volumes a step publishes, hurtbox bodies, hits, cues. So what shows
//! at a tick is what the simulation does at that tick, hitstop and knockback included, and a volume that
//! doesn't reach the dummy shows that too. A run is a few milliseconds, and is made again when the
//! document changes ([`Run::simulate`]); scrubbing and playing only read it.
//!
//! Time is a move's own clock, so a [`Frame`] says which tick of the move it reads. Under hitstop several
//! frames read one tick: [`Run::frame_at_tick`] gives the first, where scrubbing lands, and playing goes
//! through them all, so a freeze holds as it does in a fight. A run ends with the move's last tick: the
//! step that reaches its end finds it over.

use std::f32::consts::PI;

use sapphire_core::Name;
use sapphire_gameplay::{
    ActorId, ActorSpawn, ActorTransform, ControllerProfile, HurtboxId, MAX_HURTBOXES, MoveSetId, SimConfig,
    Simulation, TickActions, WarpTarget, actor_body, hit_shape, volume_pose,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::{MoveSet, Phase};
use sapphire_physics::{
    ColliderId, Group, PhysicsWorld, Pose, Shape as BodyShape, StaticCollider, StaticShape,
};
use sapphire_time::SimDuration;

const STAGE_HALF_EXTENT: f32 = 200.0;

/// How a move is played.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setup {
    /// Metres from the character to the dummy, straight ahead.
    pub dummy_distance: f32,
    /// Whether warp windows bend toward the dummy, as they would toward a target in a fight.
    pub warp_to_dummy: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self { dummy_distance: 1.5, warp_to_dummy: true }
    }
}

/// A hit volume a step published, where the simulation has it.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameVolume {
    pub name: String,
    pub path: String,
    pub shape: BodyShape,
    /// At the end of what this step covers of its arc, as hits read it.
    pub pose: Pose,
}

/// Where a hit volume of the move would be at this tick, live or not.
#[derive(Clone, Debug, PartialEq)]
pub struct Ghost {
    pub name: String,
    pub shape: BodyShape,
    pub pose: Pose,
    /// Whether this tick's frame has it live.
    pub live: bool,
}

/// A hurtbox or a capsule, where the simulation's world has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameBody {
    pub shape: BodyShape,
    pub pose: Pose,
    /// Whether it's a hurtbox a hit can land on, or only the capsule that stands in while they're off.
    pub hittable: bool,
}

/// A hit landing in a step.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameHit {
    pub name: String,
    pub point: Vec3,
    pub on_dummy: bool,
    /// The tick of the move it landed at.
    pub tick: u32,
}

/// What one step of the simulation read.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The move's whole tick at this step.
    pub tick: u32,
    pub elapsed: SimDuration,
    pub phase: Phase,
    /// The character's clock stood still this step.
    pub frozen: bool,
    pub root: ActorTransform,
    pub dummy_root: ActorTransform,
    pub volumes: Vec<FrameVolume>,
    pub ghosts: Vec<Ghost>,
    pub bodies: Vec<FrameBody>,
    pub dummy_bodies: Vec<FrameBody>,
    pub hits: Vec<FrameHit>,
    pub invulnerable: bool,
    pub cancel_open: bool,
}

/// Why a move couldn't be played.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunError(pub String);

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn run_error(what: &str, error: impl std::fmt::Display) -> RunError {
    RunError(format!("{what}: {error}"))
}

/// A run of one move, every step of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub move_name: String,
    /// The move's length.
    pub ticks: u32,
    pub frames: Vec<Frame>,
    pub setup: Setup,
}

impl Run {
    /// Plays move `name` of `set` under `setup`.
    pub fn simulate(set: &MoveSet, name: &str, setup: Setup) -> Result<Self, RunError> {
        let mv_name = Name::intern(name);
        let (_, mv) = set.find(mv_name).ok_or_else(|| RunError(format!("the set has no move `{name}`")))?;
        let ticks = mv.ticks();
        let tables = &mv.tables;
        if !setup.dummy_distance.is_finite() || setup.dummy_distance.abs() > STAGE_HALF_EXTENT {
            return Err(RunError(format!(
                "the dummy must be within the preview ground's {STAGE_HALF_EXTENT} m extent"
            )));
        }
        // A group can hit this one dummy once initially and once after each authored rearm. Local and
        // defeat freezes can both delay the clock. Include a final step to observe the move ending.
        let budget = mv
            .hitboxes
            .iter()
            .try_fold(u64::from(ticks) + 1, |steps, hitbox| {
                let freeze = hitbox
                    .attack
                    .map_or(0, |attack| u64::from(attack.hitstop) + u64::from(attack.finish_freeze));
                freeze.checked_mul(hitbox.rearm.len() as u64 + 1).and_then(|freeze| steps.checked_add(freeze))
            })
            .ok_or_else(|| RunError("the preview's step budget overflows".to_owned()))?;
        let hitboxes: Vec<_> =
            mv.hitboxes.iter().map(|hitbox| (hitbox.name(), hitbox.path(), hitbox.shape)).collect();

        let mut sim = Simulation::new(SimConfig::default());
        let (player, dummy) = stage(&mut sim, set.clone(), setup)?;
        // A first step stands them on the ground, and a move starts from there as in a fight.
        sim.step(&TickActions::NEUTRAL);
        sim.force_move(player, mv_name);

        let mut frames: Vec<Frame> = Vec::with_capacity(ticks as usize + 16);
        let mut ended = false;
        for _ in 0..budget {
            sim.step(&TickActions::NEUTRAL);
            let view = sim.actor(player).ok_or_else(|| RunError("the character went".to_owned()))?;
            let Some(current) = view.moves.current.filter(|current| current.name == mv_name) else {
                if frames.is_empty() {
                    return Err(RunError(format!("`{name}` didn't start")));
                }
                ended = true;
                break;
            };
            let dummy_view = sim.actor(dummy).ok_or_else(|| RunError("the dummy went".to_owned()))?;
            let root = ActorTransform { position: view.position, yaw: view.yaw };
            let dummy_root = ActorTransform { position: dummy_view.position, yaw: dummy_view.yaw };
            let tick = u32::try_from(current.tick()).unwrap_or(u32::MAX);
            let ghost = |root, elapsed, live: &dyn Fn(Name) -> bool| -> Vec<Ghost> {
                hitboxes
                    .iter()
                    .filter_map(|(name, path, shape)| {
                        Some(Ghost {
                            name: name.to_string(),
                            shape: hit_shape(*shape),
                            pose: volume_pose(root, tables.path(*path)?.at(elapsed)),
                            live: live(*name),
                        })
                    })
                    .collect()
            };
            if frames.is_empty() {
                // No step reads a move's very first instant: the first one starts it and runs a tick. So
                // this one's start is tick 0, what it sweeps from, by what it published.
                let start = view.moves.previous;
                let first: Vec<FrameVolume> = view
                    .moves
                    .volumes
                    .iter()
                    .filter(|volume| volume.from == SimDuration::ZERO)
                    .map(|volume| FrameVolume {
                        name: volume.name.to_string(),
                        path: volume.path.to_string(),
                        shape: hit_shape(volume.shape),
                        pose: volume_pose(start, volume.start),
                    })
                    .collect();
                let live = |name: Name| {
                    view.moves
                        .volumes
                        .iter()
                        .any(|volume| volume.from == SimDuration::ZERO && volume.name == name)
                };
                frames.push(Frame {
                    tick: 0,
                    elapsed: SimDuration::ZERO,
                    phase: mv.phase_at(SimDuration::ZERO),
                    frozen: false,
                    root: start,
                    dummy_root: dummy_view.moves.previous,
                    ghosts: ghost(start, SimDuration::ZERO, &live),
                    volumes: first,
                    bodies: bodies_at_root(&sim, player, start),
                    dummy_bodies: bodies_at_root(&sim, dummy, dummy_view.moves.previous),
                    hits: Vec::new(),
                    invulnerable: mv.invulnerable.iter().any(|span| span.contains(SimDuration::ZERO)),
                    cancel_open: mv.cancels.iter().any(|window| window.span.contains(SimDuration::ZERO)),
                });
            }
            let volumes: Vec<FrameVolume> = view
                .moves
                .volumes
                .iter()
                .map(|volume| FrameVolume {
                    name: volume.name.to_string(),
                    path: volume.path.to_string(),
                    shape: hit_shape(volume.shape),
                    pose: volume_pose(root, volume.end),
                })
                .collect();
            let live = |name: Name| view.moves.volumes.iter().any(|volume| volume.name == name);
            let ghosts = ghost(root, current.elapsed, &live);
            let hits = sim
                .hits()
                .events()
                .iter()
                .filter(|event| event.attacker == player)
                .map(|event| FrameHit {
                    name: event.group.to_string(),
                    point: event.point,
                    on_dummy: event.target == dummy,
                    tick: u32::try_from(event.at.whole_ticks()).unwrap_or(u32::MAX),
                })
                .collect();
            frames.push(Frame {
                tick,
                elapsed: current.elapsed,
                phase: current.phase,
                frozen: view.clock_delta == SimDuration::ZERO,
                root,
                dummy_root,
                volumes,
                ghosts,
                bodies: bodies_of(&sim, player),
                dummy_bodies: bodies_of(&sim, dummy),
                hits,
                invulnerable: view.moves.windows.invulnerable,
                cancel_open: view.moves.windows.cancel,
            });
        }
        if !ended {
            return Err(RunError("the move exceeded its preview step budget".to_owned()));
        }
        Ok(Self { move_name: name.to_owned(), ticks, frames, setup })
    }

    /// The first frame that reads `tick`, or the last before it, for scrubbing: what the simulation
    /// showed as the move reached that tick. `None` only for a run with no frames.
    #[must_use]
    pub fn frame_at_tick(&self, tick: u32) -> Option<usize> {
        let found = self.frames.partition_point(|frame| frame.tick < tick);
        match self.frames.get(found) {
            Some(frame) if frame.tick == tick => Some(found),
            _ => found.checked_sub(1).or_else(|| (!self.frames.is_empty()).then_some(0)),
        }
    }

    /// Every hit the run landed, in order.
    pub fn hits(&self) -> impl Iterator<Item = &FrameHit> {
        self.frames.iter().flat_map(|frame| &frame.hits)
    }
}

/// Flat ground, the character at the origin facing +Z, and the dummy ahead of it facing back.
fn stage(sim: &mut Simulation, set: MoveSet, setup: Setup) -> Result<(ActorId, ActorId), RunError> {
    let floor = StaticCollider {
        id: ColliderId(0),
        shape: StaticShape::Convex(BodyShape::Box {
            half_extents: Vec3::new(STAGE_HALF_EXTENT, 0.5, STAGE_HALF_EXTENT),
        }),
        pose: Pose::at(Vec3::new(0.0, -0.5, 0.0)),
        memberships: Group::World.mask(),
    };
    let world = PhysicsWorld::new(vec![floor], sim.body_capacity())
        .map_err(|error| run_error("the ground", error))?;
    sim.set_physics_world(world).map_err(|error| run_error("the ground", error))?;
    let controller = sim
        .add_controller_profile(&ControllerProfile::default())
        .map_err(|error| run_error("the character's controller", error))?;
    let moves: MoveSetId = sim.add_move_set(set);
    let player = sim
        .spawn(ActorSpawn {
            position: Vec3::ZERO,
            yaw: 0.0,
            moves: Some(moves),
            controller: Some(controller),
            ..ActorSpawn::default()
        })
        .map_err(|error| run_error("the character", error))?;
    let dummy = sim
        .spawn(ActorSpawn {
            position: Vec3::new(0.0, 0.0, setup.dummy_distance),
            yaw: PI,
            moves: Some(moves),
            controller: Some(controller),
            ..ActorSpawn::default()
        })
        .map_err(|error| run_error("the dummy", error))?;
    if setup.warp_to_dummy {
        sim.set_warp_target(player, WarpTarget::Actor(dummy));
    }
    Ok((player, dummy))
}

/// An actor's hurtbox bodies, or its capsule while they're out of the world: as the combat views read them.
fn bodies_of(sim: &Simulation, actor: ActorId) -> Vec<FrameBody> {
    let world = sim.physics();
    let body = |id| Some((world.body_shape(id)?, world.body_pose(id)?));
    let mut bodies: Vec<FrameBody> = (0..MAX_HURTBOXES)
        .filter_map(|index| {
            let (shape, pose) = body(HurtboxId::new(actor, index)?.body())?;
            Some(FrameBody { shape, pose, hittable: true })
        })
        .collect();
    if bodies.is_empty()
        && let Some((shape, pose)) = body(actor_body(actor))
    {
        bodies.push(FrameBody { shape, pose, hittable: false });
    }
    bodies
}

/// The synthetic tick-zero frame uses the first step's published shapes at that step's starting roots.
fn bodies_at_root(sim: &Simulation, actor: ActorId, root: ActorTransform) -> Vec<FrameBody> {
    let mut bodies = bodies_of(sim, actor);
    let Some(current) = sim.actor(actor) else { return bodies };
    if current.position == root.position && current.yaw == root.yaw {
        return bodies;
    }
    let rotation = det::quat_from_rotation_y(root.yaw - current.yaw);
    for body in &mut bodies {
        let local = det::rotate_y(body.pose.position - current.position, -current.yaw);
        body.pose.position = root.point(local);
        body.pose.rotation = det::quat_mul(rotation, body.pose.rotation);
    }
    bodies
}

#[cfg(test)]
mod tests {
    use sapphire_moves::{MoveSet, MoveSetDocument};

    use super::*;
    use crate::edits::test_files::{SLICE, WARRIOR};

    fn slice() -> MoveSet {
        MoveSetDocument::parse(SLICE).expect("parses").compile().expect("compiles")
    }

    #[test]
    fn the_synthetic_first_frames_body_is_at_its_starting_root() {
        let run = Run::simulate(&slice(), "light_1", Setup { dummy_distance: 30.0, warp_to_dummy: false })
            .expect("plays");
        let first = &run.frames[0];
        assert_eq!(first.tick, 0);
        assert!(
            run.frames[1].root.position.distance(first.root.position) > 0.01,
            "the fixture must move on its first step"
        );
        for body in &first.bodies {
            let expected = first.root.point(Vec3::new(0.0, 0.9, 0.0));
            assert!(body.pose.position.distance(expected) < 1e-5, "{body:?} versus {expected:?}");
        }
    }

    #[test]
    fn a_run_reads_every_tick_of_the_move_at_full_speed_with_no_one_in_reach() {
        let set = slice();
        let run = Run::simulate(&set, "light_1", Setup { dummy_distance: 30.0, warp_to_dummy: false })
            .expect("plays");
        let ticks: Vec<u32> = run.frames.iter().map(|frame| frame.tick).collect();
        // One frame per tick of the move, from its first instant to its last tick, none frozen, none lost.
        let expected: Vec<u32> = (0..run.ticks).collect();
        assert_eq!(ticks, expected);
        assert!(run.hits().next().is_none());
        assert!(!run.frames.iter().any(|frame| frame.frozen));
    }

    /// The move's own windows, read back out of what the simulation published: the timeline's row and the
    /// volume on screen open and close on the same ticks.
    #[test]
    fn volumes_are_live_on_exactly_the_ticks_of_their_window() {
        let set = slice();
        let (_, mv) = set.find(Name::new("light_1")).expect("the move");
        let run = Run::simulate(&set, "light_1", Setup { dummy_distance: 30.0, warp_to_dummy: false })
            .expect("plays");
        let hit = &mv.hitboxes[0];
        for frame in &run.frames {
            let live = frame.volumes.iter().any(|volume| volume.name == hit.name().to_string());
            assert_eq!(live, hit.span.contains(frame.elapsed), "tick {}", frame.tick);
            let ghost = frame.ghosts.iter().find(|ghost| ghost.name == hit.name().to_string());
            assert_eq!(ghost.map(|ghost| ghost.live), Some(live), "tick {}", frame.tick);
        }
    }

    #[test]
    fn a_hit_on_the_dummy_freezes_the_run_as_it_freezes_a_fight() {
        let set = MoveSetDocument::parse(WARRIOR).expect("parses").compile().expect("compiles");
        let run = Run::simulate(&set, "swipe", Setup::default()).expect("plays");
        let hit = run.hits().next().expect("a swipe at 1.5 m lands");
        assert!(hit.on_dummy);
        let frozen = run.frames.iter().filter(|frame| frame.frozen).count();
        assert!(frozen > 0, "hitstop holds the character's clock");
        // Scrubbing finds the first frame of a tick, however many read it.
        for tick in 0..run.ticks {
            let index = run.frame_at_tick(tick).expect("a frame");
            assert_eq!(run.frames[index].tick, tick, "tick {tick}");
            assert!(index == 0 || run.frames[index - 1].tick < tick, "tick {tick}");
        }
    }

    #[test]
    fn the_longest_valid_hitstop_does_not_truncate_the_rest_of_the_move() {
        let mut document = MoveSetDocument::parse(WARRIOR).expect("parses");
        let swipe = document.moves.iter_mut().find(|mv| mv.name == "swipe").expect("swipe");
        for volume in &mut swipe.hitboxes {
            volume.attack.as_mut().expect("attack").hitstop = sapphire_moves::MAX_HITSTOP_TICKS;
        }
        let run =
            Run::simulate(&document.compile().expect("compiles"), "swipe", Setup::default()).expect("plays");
        assert!(run.frames.iter().filter(|frame| frame.frozen).count() >= 100);
        assert_eq!(run.frames.last().expect("last frame").tick, run.ticks - 1);
    }

    #[test]
    fn a_dummy_beyond_the_stages_ground_is_refused_before_simulation() {
        for distance in [f32::MAX, f32::NAN, f32::INFINITY] {
            let error =
                Run::simulate(&slice(), "light_1", Setup { dummy_distance: distance, warp_to_dummy: false })
                    .expect_err("outside the stage");
            assert!(error.0.contains("preview ground"), "{error}");
        }
    }

    #[test]
    fn a_dummy_out_of_reach_shows_as_a_miss() {
        let set = MoveSetDocument::parse(WARRIOR).expect("parses").compile().expect("compiles");
        let far = Setup { dummy_distance: 8.0, warp_to_dummy: false };
        let run = Run::simulate(&set, "swipe", far).expect("plays");
        assert!(run.hits().next().is_none());
    }

    #[test]
    fn a_move_the_set_doesnt_have_says_so() {
        let error = Run::simulate(&slice(), "no_such_move", Setup::default()).expect_err("no move");
        assert!(error.0.contains("no_such_move"), "{error}");
    }
}
