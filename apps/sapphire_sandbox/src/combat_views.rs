//! The combat views (spec §5.20, M2 T1): what a fight's simulation is doing, drawn through
//! `sapphire_core::debug_draw`, because nothing can be tuned unseen.
//!
//! `--debug-draw volumes|colliders|combat` turns them on, and in `dev` builds so do the cvars
//! `combat.view_volumes` and `combat.view_colliders`, which the overlay's combat panel lists. Both are
//! only ever read here, so off they cost two relaxed loads a frame, and only where a fight is running
//! or the arena stands.
//!
//! - **Hit and hurt volumes.** Each [`ActiveVolume`] as the shape it sweeps this step, along the casts
//!   the hit query makes of it ([`VolumeSweep::walk`]). Each actor's hurtbox bodies, with its facing,
//!   in the colour of what it's doing ([`BodyState`]). They draw over everything, since a hit volume is
//!   usually inside a character. An actor with no hurtbox in the world shows a stand-in: see below.
//! - **Colliders.** The simulation's world: its static colliders, depth-tested, and its bodies, over
//!   everything, with hurtboxes in those same colours. A character with no body in the world draws a
//!   capsule of [`BODY_RADIUS`] and [`BODY_HEIGHT`] instead. With no fight, `--arena` draws its own
//!   statics through [`statics`] ([`crate::arena::ArenaActors::draw_colliders`]).
//!
//! # Reading, and when
//!
//! These read the simulation through `&Simulation`'s public API, so they can't change a checksum, and
//! a test holds the committed replays to that. They draw as a frame is extracted rather than inside
//! a fixed step ([`Combat::draw_views`](crate::combat::Combat::draw_views)), which keeps them out of
//! rule R6's way altogether, and for a reason of its own: a capture runs all its steps before its one
//! collect, and a step's draws would fill the thread's ring (16,384 calls, which `melee-sweep`'s 17 a
//! tick reach in under 1,000 ticks) and lose the last step's, the one the picture is of. So a frame
//! shows the state its last step left, and a frame with several steps shows the last of them, as a
//! step's draws would.
//!
//! # What these don't show yet
//!
//! - **The visible pose (the plan's "against the visible pose").** Nothing skinned follows the
//!   simulation until M2 N3, so these draw against the simulation's own transforms. [`VisiblePose`] is
//!   the seam: give it the bone or socket a path follows, and each volume also draws a line to where
//!   the visible character has it, which is the gap the plan says a blend may open.
//! - **Hurtboxes the step hasn't set.** Nothing calls `HurtboxId::set` in a step until M2 G3, so
//!   today an actor has no hurtbox bodies, and its capsule stands in, with a changed hurtbox's shape
//!   centred on it. Once G3 sets them, those bodies draw where they are with nothing changing here,
//!   and the stand-in shows only for an actor whose hurtboxes are all out of the world, which is what
//!   being off or invulnerable does to them, so in that grey.

use std::fmt;

use sapphire_core::Name;
use sapphire_core::cvar::Cvar;
use sapphire_core::debug_draw::{self as dd, Color, Shape as Draw, Style};
use sapphire_gameplay::{
    ActiveVolume, ActorId, ActorTransform, ActorView, HurtboxId, MAX_HURTBOXES, MoveRead, OwnerStep,
    Simulation, SubStep, VolumeSweep, actor_body, hit_shape, volume_pose,
};
use sapphire_math::{Vec3, det};
use sapphire_moves::HurtboxState;
use sapphire_physics::{
    BodyId, Group, GroupMask, PhysicsWorld, Pose, Shape as BodyShape, StaticGeometryView,
};

/// A character's capsule, in metres, where it has no body in the world: `ControllerProfile`'s default.
pub(crate) const BODY_HEIGHT: f32 = 1.6;
pub(crate) const BODY_RADIUS: f32 = 0.3;

/// The alpha of where a sweep began, so which way it went reads at a glance.
const START_ALPHA: u8 = 110;

/// Metres of the arrow that shows which way an actor faces.
const FACING: f32 = 0.6;

/// Each view's colour, and the combat panel's for the same thing: what a volume is doing.
pub(crate) mod palette {
    use sapphire_core::debug_draw::Color;

    /// A hit volume, which is only ever published while live.
    pub(crate) const HIT: Color = Color::RED;
    /// A hurtbox that can be hit.
    pub(crate) const HURTBOX: Color = Color::GREEN;
    /// A hurtbox that's off, or invulnerable.
    pub(crate) const OFF: Color = Color::rgb(150, 150, 160);
    pub(crate) const PARRY: Color = Color::YELLOW;
    pub(crate) const PERFECT_DODGE: Color = Color::MAGENTA;
    /// The gap between a volume and what the visible character has there.
    pub(crate) const VISIBLE: Color = Color::WHITE;
    /// A character's capsule in the collider view.
    pub(crate) const CAPSULE: Color = Color::CYAN;
    /// A static collider.
    pub(crate) const STATIC: Color = Color::ORANGE;
    /// Any other body.
    pub(crate) const BODY: Color = Color::WHITE;
}

static VOLUMES: Cvar<bool> = Cvar::new(
    "combat.view_volumes",
    false,
    "The combat view of hit and hurt volumes (M2 T1). Needs a fight: `--record` or `--replay`.",
);
static COLLIDERS: Cvar<bool> = Cvar::new(
    "combat.view_colliders",
    false,
    "The combat view of the collision world's colliders and character capsules (M2 T1).",
);

/// These views' cvars, for the app's registry. Their names start with
/// `sapphire_debug::COMBAT_VIEW_CVARS`, which is how the combat panel finds them.
#[cfg(feature = "dev")]
pub(crate) static CVARS: [&dyn sapphire_core::cvar::AnyCvar; 2] = [&VOLUMES, &COLLIDERS];

/// Which views draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CombatViews {
    pub volumes: bool,
    pub colliders: bool,
}

impl CombatViews {
    pub(crate) const VOLUMES: Self = Self { volumes: true, colliders: false };
    pub(crate) const COLLIDERS: Self = Self { volumes: false, colliders: true };
    pub(crate) const ALL: Self = Self { volumes: true, colliders: true };

    /// Which are on now.
    pub(crate) fn current() -> Self {
        Self { volumes: VOLUMES.get(), colliders: COLLIDERS.get() }
    }

    /// Turns these on, for `--debug-draw`.
    pub(crate) fn turn_on(self) {
        let set = |cvar: &Cvar<bool>, on: bool| {
            if on {
                cvar.set(true).expect("a boolean has no range");
            }
        };
        set(&VOLUMES, self.volumes);
        set(&COLLIDERS, self.colliders);
    }

    fn any(self) -> bool {
        self.volumes || self.colliders
    }
}

/// Where the views draw: the facade in the app, and a list a test can read.
pub(crate) trait Canvas {
    fn shape(&mut self, shape: Draw, style: Style);
    fn label(&mut self, at: Vec3, style: Style, text: fmt::Arguments<'_>);
}

/// `sapphire_core::debug_draw`, for one frame.
pub(crate) struct Facade;

impl Canvas for Facade {
    fn shape(&mut self, shape: Draw, style: Style) {
        dd::shape(shape, style);
    }

    fn label(&mut self, at: Vec3, style: Style, text: fmt::Arguments<'_>) {
        dd::label(at, style, text);
    }
}

/// What the visible character shows, to compare with the simulation. M2 N3 fills this from the posed
/// skeleton it draws; see the module docs.
pub(crate) trait VisiblePose {
    /// Where `actor`'s visible character has `path`, in the world, at the tick the simulation stands
    /// at: the bone or socket the path follows. `None` draws no comparison.
    fn path_position(&self, actor: ActorId, path: Name) -> Option<Vec3>;
}

/// Nothing to compare with, until M2 N3.
pub(crate) struct NoVisiblePose;

impl VisiblePose for NoVisiblePose {
    fn path_position(&self, _actor: ActorId, _path: Name) -> Option<Vec3> {
        None
    }
}

/// Draws the views that are on, from what `sim` shows of itself.
pub(crate) fn draw_current(sim: &Simulation) {
    let views = CombatViews::current();
    if views.any() {
        draw(views, sim, &NoVisiblePose, &mut Facade);
    }
}

/// Draws `views` on `canvas`. Allocates nothing.
pub(crate) fn draw(
    views: CombatViews,
    sim: &Simulation,
    visible: &dyn VisiblePose,
    canvas: &mut impl Canvas,
) {
    // In this order because a later line draws over an earlier one, and a thread's ring keeps what fits:
    // a character's capsule would hide its hurtbox's colour, and a mesh's edges are what's worth losing.
    if views.colliders {
        bodies(views, sim, canvas);
    }
    if views.volumes {
        for actor in sim.actors() {
            for (index, volume) in actor.moves.volumes.iter().enumerate() {
                hit_volume(&actor, index, &volume, sim, visible, canvas);
            }
            hurtbox(&actor, sim, canvas);
        }
    }
    if views.colliders {
        statics(sim.physics(), canvas);
    }
}

const fn transform_of(actor: &ActorView) -> ActorTransform {
    ActorTransform { position: actor.position, yaw: actor.yaw }
}

/// What the hit query casts for a volume this step: what `HitQuery::sweep_published` makes of it, as
/// far as [`VolumeSweep::walk`] reads. `None` for a volume whose move has left the library, which that
/// skips too.
fn sweep_of<'a>(
    actor: &ActorView,
    index: usize,
    volume: &ActiveVolume,
    sim: &'a Simulation,
) -> Option<VolumeSweep<'a>> {
    let reading = match actor.moves.current {
        Some(current) if current.serial == volume.serial => current.elapsed,
        // Something cut it off this step, so the clock has no reading of it.
        _ => volume.to,
    };
    Some(VolumeSweep {
        attacker: actor.id,
        instance: volume.serial,
        volume: index as u16,
        group: volume.name,
        rearm: &[],
        shape: hit_shape(volume.shape),
        path: sim.move_library().get(volume.at)?.tables.path(volume.path)?,
        blocked_by: GroupMask::NONE,
        dodgeable: false,
        owner: OwnerStep {
            delta: actor.clock_delta,
            reading,
            root_before: actor.moves.previous,
            root_after: transform_of(actor),
        },
        from: volume.from,
        to: volume.to,
    })
}

fn hit_volume(
    actor: &ActorView,
    index: usize,
    volume: &ActiveVolume,
    sim: &Simulation,
    visible: &dyn VisiblePose,
    canvas: &mut impl Canvas,
) {
    let end = volume_pose(transform_of(actor), volume.end);
    swept(sweep_of(actor, index, volume, sim).as_ref(), hit_shape(volume.shape), end, palette::HIT, canvas);
    if let Some(at) = visible.path_position(actor.id, volume.path) {
        let style = Style::new(palette::VISIBLE).on_top();
        canvas.shape(Draw::Line { from: end.position, to: at }, style);
    }
}

/// `shape` where it ends, and where it began if that's elsewhere, joined along each cast. A frozen
/// clock's sweep is one instant, so one shape.
fn swept(
    sweep: Option<&VolumeSweep<'_>>,
    shape: BodyShape,
    end: Pose,
    color: Color,
    canvas: &mut impl Canvas,
) {
    let style = Style::new(color).on_top();
    let began = Style::new(color.with_alpha(START_ALPHA)).on_top();
    let sphere = matches!(shape, BodyShape::Sphere { .. });
    let mut moved = false;
    if let Some(sweep) = sweep {
        sweep.walk(|cast| {
            if cast.start == cast.end {
                return;
            }
            if !moved && !sphere {
                body_shape(shape, cast.start, began, canvas);
            }
            moved = true;
            joined(shape, cast, style, began, canvas);
        });
    }
    // A sphere's casts already show all of it.
    if !(moved && sphere) {
        body_shape(shape, end, style, canvas);
    }
}

/// One cast of a sweep: exactly what a sphere sweeps, whatever its turn, and for a shape that turns, a
/// line for each of its ends or corners, which together trace the arc.
fn joined(shape: BodyShape, cast: &SubStep, style: Style, began: Style, canvas: &mut impl Canvas) {
    match shape {
        BodyShape::Sphere { radius } => {
            canvas.shape(Draw::Capsule { from: cast.start.position, to: cast.end.position, radius }, style);
        }
        BodyShape::Capsule { half_height, .. } => {
            for y in [-half_height, half_height] {
                corner_line(cast, Vec3::new(0.0, y, 0.0), began, canvas);
            }
        }
        BodyShape::Box { half_extents } => {
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        corner_line(cast, half_extents * Vec3::new(x, y, z), began, canvas);
                    }
                }
            }
        }
    }
}

fn corner_line(cast: &SubStep, side: Vec3, style: Style, canvas: &mut impl Canvas) {
    let at = |pose: Pose| pose.position + pose.rotation * side;
    canvas.shape(Draw::Line { from: at(cast.start), to: at(cast.end) }, style);
}

/// What an actor's hurtbox is doing this step, the first that holds of: parrying, in a perfect dodge,
/// out of reach, there to hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyState {
    Parry,
    PerfectDodge,
    Off,
    Hurtbox,
}

impl BodyState {
    pub(crate) fn of(moves: &MoveRead) -> Self {
        let turned_off = moves.hurtboxes.iter().any(|hurtbox| hurtbox.state == HurtboxState::Off);
        if moves.windows.parry {
            Self::Parry
        } else if moves.windows.perfect_dodge {
            Self::PerfectDodge
        } else if moves.windows.invulnerable || turned_off {
            Self::Off
        } else {
            Self::Hurtbox
        }
    }

    pub(crate) const fn color(self) -> Color {
        match self {
            Self::Parry => palette::PARRY,
            Self::PerfectDodge => palette::PERFECT_DODGE,
            Self::Off => palette::OFF,
            Self::Hurtbox => palette::HURTBOX,
        }
    }
}

/// The capsule a character without a body stands in.
fn nominal_capsule(actor: &ActorView) -> (BodyShape, Pose) {
    (
        BodyShape::Capsule { half_height: BODY_HEIGHT * 0.5 - BODY_RADIUS, radius: BODY_RADIUS },
        Pose::new(
            actor.position + Vec3::new(0.0, BODY_HEIGHT * 0.5, 0.0),
            det::quat_from_rotation_y(actor.yaw),
        ),
    )
}

/// A body's shape and place in the world, if it's there.
fn body_in(world: &PhysicsWorld, body: BodyId) -> Option<(BodyShape, Pose)> {
    Some((world.body_shape(body)?, world.body_pose(body)?))
}

/// An actor's hurtbox bodies, which leave the world while they're off, or what stands in for them.
fn hurtbox(actor: &ActorView, sim: &Simulation, canvas: &mut impl Canvas) {
    let world = sim.physics();
    let moves = &actor.moves;
    let color = BodyState::of(moves).color();
    let style = Style::new(color).on_top();

    let mut any = false;
    for index in 0..MAX_HURTBOXES {
        let body = HurtboxId::new(actor.id, index).and_then(|id| body_in(world, id.body()));
        if let Some((shape, pose)) = body {
            body_shape(shape, pose, style, canvas);
            any = true;
        }
    }
    if !any {
        let (capsule, pose) = body_in(world, actor_body(actor.id)).unwrap_or_else(|| nominal_capsule(actor));
        let mut changed = false;
        for change in moves.hurtboxes.iter() {
            if let HurtboxState::Shape(shape) = change.state {
                body_shape(hit_shape(shape), pose, style, canvas);
                changed = true;
            }
        }
        if !changed {
            body_shape(capsule, pose, style, canvas);
        }
    }

    // At the feet, so it doesn't cross the volumes.
    let ground = Vec3::new(0.0, 0.03, 0.0);
    let tip = transform_of(actor).point(Vec3::new(0.0, 0.0, FACING));
    canvas.shape(Draw::Arrow { from: actor.position + ground, to: tip + ground }, style);

    let top = actor.position + Vec3::new(0.0, BODY_HEIGHT + 0.1, 0.0);
    let playing = moves.current.and_then(|current| {
        Some((sim.move_library().get(current.at)?.label(), current.tick(), current.ticks))
    });
    match playing {
        Some((name, tick, ticks)) => {
            canvas.label(top, style, format_args!("#{} {name} {tick}/{ticks}", actor.id.raw()));
        }
        None => canvas.label(top, style, format_args!("#{}", actor.id.raw())),
    }
}

/// Each character's capsule, and every body in the world, a hurtbox in the colour of what its owner
/// is doing. With the volumes on, those draw a hurtbox themselves.
fn bodies(views: CombatViews, sim: &Simulation, canvas: &mut impl Canvas) {
    let world = sim.physics();
    let capsule = Style::new(palette::CAPSULE).on_top();
    for actor in sim.actors() {
        if world.body_shape(actor_body(actor.id)).is_none() {
            let (shape, pose) = nominal_capsule(&actor);
            body_shape(shape, pose, capsule, canvas);
        }
    }
    for body in world.bodies() {
        let owner = HurtboxId::from_body(body.id);
        if owner.is_some() && views.volumes {
            continue;
        }
        let groups = body.groups.memberships;
        let color = if let Some(owner) = owner.and_then(|owner| sim.actor(owner.actor())) {
            BodyState::of(&owner.moves).color()
        } else if groups.intersects(Group::Character.mask()) {
            palette::CAPSULE
        } else if groups.intersects(Group::Hurtbox.mask()) {
            palette::HURTBOX
        } else {
            palette::BODY
        };
        body_shape(body.shape, body.pose, Style::new(color).on_top(), canvas);
    }
}

/// Every static collider, tested against the scene they're usually part of.
pub(crate) fn statics(world: &PhysicsWorld, canvas: &mut impl Canvas) {
    let fixed = Style::new(palette::STATIC);
    for collider in world.statics() {
        match collider.geometry {
            StaticGeometryView::Convex(shape) => body_shape(shape, collider.pose, fixed, canvas),
            StaticGeometryView::Mesh { vertices, triangles } => {
                let corner =
                    |index: u32| collider.pose.position + collider.pose.rotation * vertices[index as usize];
                for &[a, b, c] in triangles {
                    let (a, b, c) = (corner(a), corner(b), corner(c));
                    canvas.shape(Draw::Line { from: a, to: b }, fixed);
                    canvas.shape(Draw::Line { from: b, to: c }, fixed);
                    canvas.shape(Draw::Line { from: c, to: a }, fixed);
                }
            }
        }
    }
}

fn body_shape(shape: BodyShape, pose: Pose, style: Style, canvas: &mut impl Canvas) {
    let draw = match shape {
        BodyShape::Box { half_extents } => {
            Draw::OrientedBox { center: pose.position, half_extents, rotation: pose.rotation }
        }
        BodyShape::Sphere { radius } => Draw::Sphere { center: pose.position, radius },
        BodyShape::Capsule { half_height, radius } => {
            let half = pose.rotation * Vec3::new(0.0, half_height, 0.0);
            Draw::Capsule { from: pose.position - half, to: pose.position + half, radius }
        }
    };
    canvas.shape(draw, style);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use sapphire_core::alloc::{self, PER_THREAD_COUNTS};
    use sapphire_core::debug_draw::{Depth, LabelText};
    use sapphire_gameplay::replay::{
        ARCHITECTURE, ChecksumFile, Player, Replay, SCENARIOS, checksums_file_name, find_scenario, test_moves,
    };
    use sapphire_gameplay::{ActorSpawn, ControllerProfile, SimConfig, TickActions};
    use sapphire_math::Quat;
    use sapphire_moves::MoveSetDocument;
    use sapphire_physics::{BodyDesc, ColliderId, CollisionGroups, StaticCollider, StaticShape};
    use sapphire_time::{ScaleSource, SimDuration};

    use super::*;

    /// What the views drew, in order.
    #[derive(Default)]
    struct Sketch {
        shapes: Vec<(Draw, Style)>,
        labels: Vec<(Vec3, Color, String)>,
    }

    impl Canvas for Sketch {
        fn shape(&mut self, shape: Draw, style: Style) {
            self.shapes.push((shape, style));
        }

        fn label(&mut self, at: Vec3, style: Style, text: fmt::Arguments<'_>) {
            self.labels.push((at, style.color, text.to_string()));
        }
    }

    fn sketch(views: CombatViews, sim: &Simulation) -> Sketch {
        let mut sketch = Sketch::default();
        draw(views, sim, &NoVisiblePose, &mut sketch);
        sketch
    }

    /// One move: 2 ticks of startup, then each kind of volume along a path that holds still in the
    /// root's space, in front of a root walking 1 cm a tick along its own +Z.
    const SWING: &str = r#"(
        type: "sapphire::moves::MoveSetDocument",
        version: 1,
        data: (
            intents: [],
            neutral: [],
            moves: [(
                name: "swing",
                startup: 2,
                active: 8,
                recovery: 10,
                motion: (
                    root: [(at: 20, offset: (0.0, 0.0, 0.2))],
                    paths: [(
                        name: "blade",
                        keys: [(at: 0, offset: (0.0, 1.0, 0.0)), (at: 20, offset: (2.0, 1.0, 0.0))],
                    )],
                ),
                hitboxes: [
                    (name: "ball", from: 2, to: 10, path: "blade", shape: Sphere(radius: 0.1)),
                    (name: "rod", from: 2, to: 10, path: "blade", shape: Capsule(radius: 0.1, length: 0.5)),
                    (name: "slab", from: 2, to: 10, path: "blade", shape: Box(half_extents: (0.1, 0.2, 0.3))),
                ],
            )],
        ),
    )"#;

    const QUARTER: f32 = std::f32::consts::FRAC_PI_2;

    /// An actor at (1, 0, 2) facing +X, `ticks` into the swing.
    fn swinging(ticks: u32) -> Simulation {
        let mut sim = Simulation::new(SimConfig::default());
        let moves = sim.add_move_set(MoveSetDocument::parse(SWING).unwrap().compile().unwrap());
        let actor = sim
            .spawn(ActorSpawn {
                position: Vec3::new(1.0, 0.0, 2.0),
                yaw: QUARTER,
                moves: Some(moves),
                controller: None,
            })
            .unwrap();
        sim.force_move(actor, Name::new("swing"));
        // A forced move starts where the step does, so its first step reads tick 1.
        for _ in 0..ticks {
            sim.step(&TickActions::NEUTRAL);
        }
        sim
    }

    fn assert_near(actual: Vec3, expected: Vec3) {
        assert!(actual.abs_diff_eq(expected, 2e-5), "{actual} isn't {expected}");
    }

    fn drawn_in(sketch: &Sketch, color: Color) -> Vec<(Draw, Style)> {
        let same = |style: &Style| Color { a: 255, ..style.color } == color;
        sketch.shapes.iter().filter(|(_, style)| same(style)).copied().collect()
    }

    /// Worked by hand. At tick 3 the path has gone 0.3 m along the root's +X and the root 3 cm along its
    /// +Z, each a tenth and a hundredth of a metre a tick. Facing +X, the root's +X is the world's −Z,
    /// and its +Z the world's +X.
    #[test]
    fn a_sphere_sweeps_a_capsule_from_where_it_was_to_where_it_is() {
        let sim = swinging(3);
        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let (Draw::Capsule { from, to, radius }, style) = drawn_in(&sketch, palette::HIT)[0] else {
            panic!("the first volume is a sphere's sweep");
        };
        assert_near(from, Vec3::new(1.02, 1.0, 2.0 - 0.2));
        assert_near(to, Vec3::new(1.03, 1.0, 2.0 - 0.3));
        assert_eq!(radius, 0.1);
        assert_eq!(style.color, palette::HIT);
        assert_eq!(style.depth, Depth::OnTop, "a hit volume is usually inside a character");
    }

    #[test]
    fn a_capsule_and_a_box_show_both_ends_of_their_sweep_joined_up() {
        let sim = swinging(3);
        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let hits = drawn_in(&sketch, palette::HIT);
        let begins = |(_, style): &(Draw, Style)| style.color.a == START_ALPHA;
        let (ended, began): (Vec<_>, Vec<_>) = hits.iter().partition(|hit| !begins(hit));
        // A sphere's one capsule, then a capsule and a box as they end.
        assert_eq!(ended.len(), 3);
        // A capsule as it began, with its two ends joined; a box, with its eight corners joined.
        assert_eq!(began.len(), 3 + 9);

        // The rod stands along the root's +Y, 0.5 long, however it's turned.
        let (Draw::Capsule { from, to, .. }, _) = ended[1] else { panic!("{:?}", ended[1]) };
        assert_near(from, Vec3::new(1.03, 0.75, 1.7));
        assert_near(to, Vec3::new(1.03, 1.25, 1.7));
        let (Draw::Capsule { from, .. }, _) = began[0] else { panic!("{:?}", began[0]) };
        assert_near(from, Vec3::new(1.02, 0.75, 1.8));

        // The slab turns with the root.
        let (Draw::OrientedBox { center, half_extents, rotation }, _) = ended[2] else {
            panic!("{:?}", ended[2]);
        };
        assert_near(center, Vec3::new(1.03, 1.0, 1.7));
        assert_eq!(half_extents, Vec3::new(0.1, 0.2, 0.3));
        assert_near(rotation * Vec3::Z, Vec3::X);
        let (Draw::Line { from, to }, _) = began[4] else { panic!("{:?}", began[4]) };
        assert_near(to - from, Vec3::new(0.01, 0.0, -0.1));
    }

    /// Hitstop's sweep is one instant.
    #[test]
    fn a_frozen_volume_is_one_shape_of_each() {
        let mut sim = swinging(3);
        let actor = ActorId::from_raw(1).unwrap();
        sim.clock_mut(actor).unwrap().0.apply_hitstop(4);
        sim.step(&TickActions::NEUTRAL);
        sim.step(&TickActions::NEUTRAL);
        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let hits = drawn_in(&sketch, palette::HIT);
        assert!(hits.iter().all(|(_, style)| style.color.a == 255), "{hits:?}");
        assert!(matches!(hits[0].0, Draw::Sphere { radius: 0.1, .. }), "{hits:?}");
        assert!(matches!(hits[1].0, Draw::Capsule { .. }), "{hits:?}");
        assert!(matches!(hits[2].0, Draw::OrientedBox { .. }), "{hits:?}");
        assert_eq!(hits.len(), 3);
    }

    /// One move with `motion`, whose volumes `hitboxes` names, all 20 ticks live, at the origin facing +Z.
    fn playing(motion: &str, hitboxes: &str) -> (Simulation, ActorId) {
        let document = format!(
            r#"(
                type: "sapphire::moves::MoveSetDocument",
                version: 1,
                data: (
                    intents: [],
                    neutral: [],
                    moves: [(
                        name: "swing",
                        startup: 0,
                        active: 20,
                        recovery: 0,
                        motion: ({motion}),
                        hitboxes: [{hitboxes}],
                    )],
                ),
            )"#
        );
        let mut sim = Simulation::new(SimConfig::default());
        let moves = sim.add_move_set(MoveSetDocument::parse(&document).unwrap().compile().unwrap());
        let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).unwrap();
        sim.force_move(actor, Name::new("swing"));
        (sim, actor)
    }

    /// A blade turning a quarter turn in 5 ticks, about the point it rides: 18 degrees a tick, so its
    /// tip moves 16 cm against a thickness of 5 cm, and the hit query makes 4 casts of it.
    #[test]
    fn a_turning_capsule_traces_its_casts_where_a_straight_line_would_cut_the_arc() {
        let (mut sim, _) = playing(
            r#"paths: [(name: "blade", keys: [
                (at: 0, offset: (0.0, 1.0, 0.0)),
                (at: 5, offset: (0.0, 1.0, 0.0), rotation: (0.0, 0.0, 0.70710678, 0.70710678)),
            ])]"#,
            r#"(name: "edge", from: 0, to: 20, path: "blade", shape: Capsule(radius: 0.05, length: 1.0))"#,
        );
        sim.step(&TickActions::NEUTRAL);
        sim.step(&TickActions::NEUTRAL);

        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let hits = drawn_in(&sketch, palette::HIT);
        let (ended, began): (Vec<&(Draw, Style)>, Vec<_>) = hits.iter().partition(|hit| hit.1.color.a == 255);
        assert_eq!(ended.len(), 1, "{hits:?}");
        assert!(matches!(began[0].0, Draw::Capsule { .. }), "{hits:?}");

        let lines: Vec<(Vec3, Vec3)> = began[1..]
            .iter()
            .map(|(shape, _)| {
                let Draw::Line { from, to } = *shape else { panic!("{shape:?}") };
                (from, to)
            })
            .collect();
        assert_eq!(lines.len(), 2 * 4, "an end's line for each of 4 casts");
        let center = Vec3::new(0.0, 1.0, 0.0);
        for (index, (from, to)) in lines.iter().enumerate() {
            // Every point the lines reach is on the circle the tip turns along, which the one straight
            // line from where it began to where it ended would leave.
            assert!(((*to - center).length() - 0.5).abs() < 1e-3, "{to}");
            if index >= 2 {
                assert_near(*from, lines[index - 2].1);
            }
        }
    }

    /// A clock running at 3× covers 3 ticks in a step, and the path turns a corner at tick 5 of the
    /// root's 10 cm a tick. The step that crosses it casts along each table sample's stretch, so the
    /// corner shows.
    #[test]
    fn a_step_of_several_ticks_follows_the_corners_of_its_path() {
        let (mut sim, actor) = playing(
            r#"root: [(at: 20, offset: (0.0, 0.0, 2.0))],
               paths: [(name: "blade", keys: [
                   (at: 0, offset: (0.0, 1.0, 0.0)),
                   (at: 5, offset: (1.0, 1.0, 0.0)),
                   (at: 20, offset: (1.0, 4.0, 0.0)),
               ])]"#,
            r#"(name: "ball", from: 0, to: 20, path: "blade", shape: Sphere(radius: 0.1))"#,
        );
        sim.clock_mut(actor).unwrap().0.push_scale(ScaleSource::Gameplay, 3_000).unwrap();
        let tick = SimDuration::from_ticks(1);
        let corner_tick = SimDuration::from_ticks(5);
        let crossing = (0..10).find_map(|_| {
            sim.step(&TickActions::NEUTRAL);
            let view = sim.actor(actor).unwrap();
            let volume = view.moves.volumes.iter().next()?;
            (volume.from < corner_tick && corner_tick < volume.to).then_some((view, volume))
        });
        let (view, volume) = crossing.expect("a step crosses tick 5");
        assert_eq!(view.clock_delta.as_subticks(), 3 * tick.as_subticks());

        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let hits = drawn_in(&sketch, palette::HIT);
        assert_eq!(hits.len(), 3, "one cast for each tick: {hits:?}");
        let ends: Vec<(Vec3, Vec3)> = hits
            .iter()
            .map(|(shape, _)| {
                let Draw::Capsule { from, to, .. } = *shape else { panic!("{shape:?}") };
                (from, to)
            })
            .collect();
        assert_near(ends[0].0, volume_pose(view.moves.previous, volume.start).position);
        assert_near(ends[2].1, volume_pose(transform_of(&view), volume.end).position);
        let corner = Vec3::new(1.0, 1.0, 0.5);
        assert!(ends.iter().any(|(_, to)| to.abs_diff_eq(corner, 2e-5)), "{corner} isn't in {ends:?}");
    }

    /// `ticks` into `name` of the test content, whose numbers are in `slice.moves.ron`.
    fn state_through(name: &str, ticks: u32) -> Vec<BodyState> {
        let (mut sim, cast) = SCENARIOS[1].start(SCENARIOS[1].config).unwrap();
        sim.force_move(cast[0], Name::new(name));
        (0..ticks)
            .map(|_| {
                sim.step(&TickActions::NEUTRAL);
                BodyState::of(&sim.actor(cast[0]).unwrap().moves)
            })
            .collect()
    }

    fn runs(states: &[BodyState]) -> Vec<(BodyState, usize)> {
        let mut runs: Vec<(BodyState, usize)> = Vec::new();
        for &state in states {
            match runs.last_mut() {
                Some((last, count)) if *last == state => *count += 1,
                _ => runs.push((state, 1)),
            }
        }
        runs
    }

    /// `invulnerable: 1..14`, `perfect_dodge: 1..7`, its hurtbox off for the same 13 ticks, read from
    /// tick 1.
    #[test]
    fn a_dodges_colours_go_perfect_then_off_then_back() {
        assert_eq!(
            runs(&state_through("dodge", 16)),
            [(BodyState::PerfectDodge, 6), (BodyState::Off, 7), (BodyState::Hurtbox, 3)]
        );
    }

    /// `invulnerable: 0..8`, `parry: 2..6`, read from tick 1.
    #[test]
    fn a_skills_colours_go_off_then_parry_then_off_then_back() {
        assert_eq!(
            runs(&state_through("skill", 10)),
            [(BodyState::Off, 1), (BodyState::Parry, 4), (BodyState::Off, 2), (BodyState::Hurtbox, 3)]
        );
    }

    #[test]
    fn each_state_has_a_colour_of_its_own() {
        let colors = [
            palette::HIT,
            palette::HURTBOX,
            palette::OFF,
            palette::PARRY,
            palette::PERFECT_DODGE,
            palette::CAPSULE,
            palette::STATIC,
        ];
        for (index, color) in colors.iter().enumerate() {
            assert!(!colors[index + 1..].contains(color), "{color:?}");
        }
    }

    #[test]
    fn a_hurtbox_stands_on_its_actor_and_says_what_its_playing() {
        let sim = swinging(3);
        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let hurtboxes = drawn_in(&sketch, palette::HURTBOX);
        let (Draw::Capsule { from, to, radius }, style) = hurtboxes[0] else { panic!("{hurtboxes:?}") };
        assert_near(from, Vec3::new(1.03, BODY_RADIUS, 2.0));
        assert_near(to, Vec3::new(1.03, BODY_HEIGHT - BODY_RADIUS, 2.0));
        assert_eq!(radius, BODY_RADIUS);
        assert_eq!(style.depth, Depth::OnTop);
        // Facing +X, at its feet.
        let (Draw::Arrow { from, to }, _) = hurtboxes[1] else { panic!("{hurtboxes:?}") };
        assert_near(to - from, Vec3::new(FACING, 0.0, 0.0));
        assert_eq!(sketch.labels.len(), 1);
        assert_eq!(sketch.labels[0].2, "#1 swing 3/20");
    }

    /// A change's shape stands in for the hurtbox, centred on where it was.
    #[test]
    fn a_hurtbox_change_draws_its_shape_on_the_body() {
        const CHANGED: &str = r#"(
            type: "sapphire::moves::MoveSetDocument",
            version: 1,
            data: (
                intents: [],
                neutral: [],
                moves: [(
                    name: "crouch",
                    startup: 0,
                    active: 4,
                    recovery: 0,
                    hurtboxes: [(name: "body", from: 0, to: 4, state: Shape(Sphere(radius: 0.5)))],
                )],
            ),
        )"#;
        let mut sim = Simulation::new(SimConfig::default());
        let moves = sim.add_move_set(MoveSetDocument::parse(CHANGED).unwrap().compile().unwrap());
        let actor = sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }).unwrap();
        sim.force_move(actor, Name::new("crouch"));
        sim.step(&TickActions::NEUTRAL);
        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let hurtboxes = drawn_in(&sketch, palette::HURTBOX);
        let (Draw::Sphere { center, radius }, _) = hurtboxes[0] else { panic!("{hurtboxes:?}") };
        assert_near(center, Vec3::new(0.0, BODY_HEIGHT * 0.5, 0.0));
        assert_eq!(radius, 0.5);
        assert!(!hurtboxes.iter().any(|(shape, _)| matches!(shape, Draw::Capsule { .. })), "{hurtboxes:?}");
    }

    /// M2 N3's seam: where the visible character has a path, a line from the volume to it.
    #[test]
    fn a_visible_pose_shows_its_gap_from_the_volume() {
        struct Held(Vec3);
        impl VisiblePose for Held {
            fn path_position(&self, actor: ActorId, path: Name) -> Option<Vec3> {
                assert_eq!(actor.raw(), 1);
                (path == Name::new("blade")).then_some(self.0)
            }
        }
        let sim = swinging(3);
        let bone = Vec3::new(1.0, 1.5, 2.0);
        let mut sketch = Sketch::default();
        draw(CombatViews::VOLUMES, &sim, &Held(bone), &mut sketch);
        let gaps = drawn_in(&sketch, palette::VISIBLE);
        assert_eq!(gaps.len(), 3, "one for each volume on the path");
        let (Draw::Line { from, to }, _) = gaps[0] else { panic!("{gaps:?}") };
        assert_near(from, Vec3::new(1.03, 1.0, 1.7));
        assert_eq!(to, bone);
        assert!(drawn_in(&self::sketch(CombatViews::VOLUMES, &sim), palette::VISIBLE).is_empty());
    }

    fn turned() -> Pose {
        Pose::new(Vec3::new(5.0, 1.0, 0.0), Quat::from_rotation_y(QUARTER))
    }

    fn world(capacity: usize) -> PhysicsWorld {
        let statics = vec![
            StaticCollider {
                id: ColliderId(1),
                shape: StaticShape::Convex(BodyShape::Box { half_extents: Vec3::new(1.0, 0.5, 2.0) }),
                pose: turned(),
                memberships: Group::World.mask(),
            },
            StaticCollider {
                id: ColliderId(2),
                shape: StaticShape::Mesh {
                    vertices: vec![Vec3::ZERO, Vec3::X, Vec3::Z],
                    indices: vec![[0, 1, 2]],
                },
                pose: turned(),
                memberships: Group::World.mask(),
            },
        ];
        let mut world = PhysicsWorld::new(statics, capacity).unwrap();
        let mut body = |id, shape, pose, group: Group| {
            let groups = CollisionGroups::new(group.mask(), GroupMask::NONE);
            world.add_body(BodyDesc { id: BodyId(id), shape, pose, groups }).unwrap();
        };
        body(1, BodyShape::Capsule { half_height: 0.5, radius: 0.3 }, turned(), Group::Character);
        body(9, BodyShape::Sphere { radius: 0.2 }, Pose::at(Vec3::Y), Group::Hurtbox);
        body(10, BodyShape::Sphere { radius: 0.2 }, Pose::at(Vec3::Y), Group::CameraBlocker);
        world
    }

    fn in_world() -> Simulation {
        let mut sim = Simulation::new(SimConfig::default());
        let world = world(sim.body_capacity());
        sim.set_physics_world(world).unwrap();
        sim
    }

    #[test]
    fn colliders_draw_a_world_with_what_each_thing_is() {
        let sim = in_world();
        let sketch = sketch(CombatViews::COLLIDERS, &sim);
        let shapes = sketch.shapes;

        // Bodies first, over everything, by the group they're in.
        let (Draw::Capsule { from, to, radius }, style) = shapes[0] else { panic!("{shapes:?}") };
        assert_near(from, Vec3::new(5.0, 0.5, 0.0));
        assert_near(to, Vec3::new(5.0, 1.5, 0.0));
        assert_eq!((radius, style.color, style.depth), (0.3, palette::CAPSULE, Depth::OnTop));
        assert!(matches!(shapes[1].0, Draw::Sphere { .. }));
        assert_eq!(shapes[1].1.color, palette::HURTBOX);
        assert_eq!(shapes[2].1.color, palette::BODY);

        // Then the statics, tested against the scene.
        let (Draw::OrientedBox { center, half_extents, rotation }, style) = shapes[3] else {
            panic!("{shapes:?}");
        };
        assert_near(center, turned().position);
        assert_eq!(half_extents, Vec3::new(1.0, 0.5, 2.0));
        assert_eq!(rotation, turned().rotation);
        assert_eq!((style.color, style.depth), (palette::STATIC, Depth::Tested));

        // A mesh's triangle through its collider's pose: its +X is the world's −Z.
        let edges: Vec<_> = shapes[4..]
            .iter()
            .map(|(shape, _)| {
                let Draw::Line { from, .. } = *shape else { panic!("{shape:?}") };
                from
            })
            .collect();
        assert_eq!(edges.len(), 3);
        assert_near(edges[0], Vec3::new(5.0, 1.0, 0.0));
        assert_near(edges[1], Vec3::new(5.0, 1.0, -1.0));
        assert_near(edges[2], Vec3::new(6.0, 1.0, 0.0));
    }

    /// The real thing, from a replay's own scenario: `flat_ground`'s floor, and capsules the controller
    /// keeps standing on it.
    #[test]
    fn colliders_show_a_scenarios_floor_and_its_characters_on_it() {
        let melee = find_scenario(SCENARIOS, "melee").unwrap();
        let (mut sim, cast) = melee.start(melee.config).unwrap();
        for _ in 0..10 {
            sim.step(&TickActions::NEUTRAL);
        }
        let sketch = sketch(CombatViews::COLLIDERS, &sim);

        let floors = drawn_in(&sketch, palette::STATIC);
        assert_eq!(floors.len(), 1, "{floors:?}");
        let (Draw::OrientedBox { center, half_extents, .. }, style) = floors[0] else {
            panic!("{floors:?}");
        };
        assert_near(center, Vec3::new(0.0, -0.5, 0.0));
        assert_eq!(half_extents, Vec3::new(200.0, 0.5, 200.0));
        assert_eq!(style.depth, Depth::Tested);

        let capsules = drawn_in(&sketch, palette::CAPSULE);
        assert_eq!(capsules.len(), cast.len(), "{capsules:?}");
        for ((shape, _), id) in capsules.iter().zip(&cast) {
            let Draw::Capsule { from, to, radius } = *shape else { panic!("{shape:?}") };
            let (shape, pose) = body_in(sim.physics(), actor_body(*id)).expect("it has a body");
            let BodyShape::Capsule { half_height, radius: its_radius } = shape else { panic!("{shape:?}") };
            assert_eq!(radius, its_radius);
            assert_near((from + to) * 0.5, pose.position);
            assert!(((to - from).length() * 0.5 - half_height).abs() < 1e-5);
            // Standing on the floor, with no more under it than its skin.
            let bottom = from.y - radius;
            assert!(bottom > 0.0 && bottom < 0.02, "{bottom}");
        }
    }

    /// Until a character has a body, a capsule stands in for it, and once it has one, only the body
    /// draws. It's the size a controller gives a character by default.
    #[test]
    fn a_character_without_a_body_draws_a_capsule_where_it_stands() {
        let capsules = |sim: &Simulation| {
            sketch(CombatViews::COLLIDERS, sim)
                .shapes
                .iter()
                .filter(|(shape, style)| {
                    matches!(shape, Draw::Capsule { .. }) && style.color == palette::CAPSULE
                })
                .count()
        };
        let mut empty = Simulation::new(SimConfig::default());
        let mut full = in_world();
        for _ in 0..2 {
            empty.spawn(ActorSpawn::default()).unwrap();
            full.spawn(ActorSpawn::default()).unwrap();
        }
        assert_eq!(capsules(&empty), 2);
        // The world's first body is the first actor's: only the second is left to stand in for.
        assert_eq!(capsules(&full), 2, "the body's and the second actor's");

        let profile = ControllerProfile::default();
        assert_eq!((profile.height, profile.radius), (BODY_HEIGHT, BODY_RADIUS));
    }

    /// With no hurtbox of its own, an actor's own capsule stands in for it, in the colour of what it's
    /// doing, where the collider view has it.
    #[test]
    fn a_stand_in_for_a_hurtbox_is_the_actors_own_capsule() {
        let (mut sim, cast) = SCENARIOS[1].start(SCENARIOS[1].config).unwrap();
        sim.force_move(cast[0], Name::new("dodge"));
        for _ in 0..8 {
            sim.step(&TickActions::NEUTRAL);
        }
        let sketch = sketch(CombatViews::VOLUMES, &sim);
        let off = drawn_in(&sketch, palette::OFF);
        let (Draw::Capsule { from, to, .. }, _) = off[0] else { panic!("{off:?}") };
        let (_, pose) = body_in(sim.physics(), actor_body(cast[0])).expect("it has a body");
        assert_near((from + to) * 0.5, pose.position);
    }

    /// D4's bodies: where they are in the world, in the colour of what their owner is doing, once.
    #[test]
    fn hurtbox_bodies_draw_where_they_are_in_the_colour_of_what_their_owner_is_doing() {
        let actor = ActorId::from_raw(1).unwrap();
        let hurtbox = |actor, index| HurtboxId::new(actor, index).unwrap().body();
        let (chest, head) = (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.5, 0.0));

        let mut sim = Simulation::new(SimConfig::default());
        let mut world = PhysicsWorld::new(Vec::new(), sim.body_capacity()).unwrap();
        let groups = CollisionGroups::new(Group::Hurtbox.mask(), GroupMask::NONE);
        let mut body = |id, center| {
            let shape = BodyShape::Sphere { radius: 0.2 };
            world.add_body(BodyDesc { id, shape, pose: Pose::at(center), groups }).unwrap();
        };
        // Numbered past what the test set's fighter places for itself, which a step moves.
        body(hurtbox(actor, 1), chest);
        body(hurtbox(actor, 2), head);
        // Nobody's: no live actor owns it.
        body(hurtbox(ActorId::from_raw(7).unwrap(), 0), Vec3::X);
        sim.set_physics_world(world).unwrap();
        let moves = sim.add_move_set(test_moves().unwrap());
        assert_eq!(sim.spawn(ActorSpawn { moves: Some(moves), ..ActorSpawn::default() }), Ok(actor));

        let spheres = |sketch: &Sketch, color| -> Vec<Vec3> {
            drawn_in(sketch, color)
                .iter()
                .filter_map(|(shape, _)| match *shape {
                    Draw::Sphere { center, .. } => Some(center),
                    _ => None,
                })
                .collect()
        };

        // Standing there: can be hit, with no capsule standing in now it has hurtboxes.
        let volumes = sketch(CombatViews::VOLUMES, &sim);
        assert_eq!(spheres(&volumes, palette::HURTBOX), [chest, head]);
        let green = drawn_in(&volumes, palette::HURTBOX);
        assert!(!green.iter().any(|(shape, _)| matches!(shape, Draw::Capsule { .. })), "{green:?}");

        // `parry: 2..6`, read from tick 1.
        sim.force_move(actor, Name::new("skill"));
        for _ in 0..3 {
            sim.step(&TickActions::NEUTRAL);
        }
        let volumes = sketch(CombatViews::VOLUMES, &sim);
        assert_eq!(spheres(&volumes, palette::PARRY), [chest, head]);
        assert!(spheres(&volumes, palette::HURTBOX).is_empty());

        // The collider view says the same, and nobody's is only in that view, as a hurtbox.
        let colliders = sketch(CombatViews::COLLIDERS, &sim);
        assert_eq!(spheres(&colliders, palette::PARRY), [chest, head]);
        assert_eq!(spheres(&colliders, palette::HURTBOX), [Vec3::X]);

        // Both at once draw each once.
        let both = sketch(CombatViews::ALL, &sim);
        assert_eq!(spheres(&both, palette::PARRY), [chest, head]);
    }

    /// A later line draws over an earlier one: with both views on, a capsule that stands where a
    /// hurtbox does mustn't hide what the hurtbox is doing.
    #[test]
    fn with_both_views_on_a_hurtboxs_colour_draws_over_its_capsule_and_statics_come_last() {
        let (sim, _) = SCENARIOS[1].start(SCENARIOS[1].config).unwrap();
        let shapes = sketch(CombatViews::ALL, &sim).shapes;
        let at = |color: Color| shapes.iter().position(|(_, style)| style.color == color).expect("drawn");
        let last = |color: Color| shapes.iter().rposition(|(_, style)| style.color == color).expect("drawn");
        assert!(last(palette::CAPSULE) < at(palette::HURTBOX), "{shapes:?}");
        assert!(last(palette::HURTBOX) < at(palette::STATIC), "{shapes:?}");
    }

    #[test]
    fn views_that_are_off_draw_nothing() {
        let (sim, _) = SCENARIOS[1].start(SCENARIOS[1].config).unwrap();
        let sketch = sketch(CombatViews::default(), &sim);
        assert!(sketch.shapes.is_empty() && sketch.labels.is_empty());
    }

    fn replays() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays")
    }

    fn player(name: &str) -> Player {
        let bytes = fs::read(replays().join(format!("{name}.sreplay"))).expect("committed");
        Player::new(Replay::decode(&bytes).expect("reads"), SCENARIOS).expect("starts")
    }

    /// The property: the views look at every tick of the suite's fights, with volumes and the moves and
    /// spawns of `melee` in them, and every checksum comes out as it does with them off, and as the
    /// suite recorded where this architecture has one.
    #[test]
    fn the_views_on_change_no_checksum_of_any_replay() {
        let text = fs::read_to_string(replays().join(checksums_file_name(ARCHITECTURE))).ok();
        let recorded = text.map(|text| ChecksumFile::parse(&text).expect("reads"));
        let files = fs::read_dir(replays()).expect("they're committed");
        let files = files.map(|file| file.expect("reads").path());
        let names: Vec<String> = files
            .filter(|path| path.extension().is_some_and(|extension| extension == "sreplay"))
            .map(|path| path.file_stem().expect("named").to_string_lossy().into_owned())
            .collect();
        assert!(!names.is_empty(), "no replays");
        for name in &names {
            let mut on = player(name);
            let mut off = player(name);
            let mut drawn = 0;
            while on.step() {
                assert!(off.step());
                let mut sketch = Sketch::default();
                draw(CombatViews::ALL, on.simulation(), &NoVisiblePose, &mut sketch);
                drawn += sketch.shapes.len();
            }
            assert!(drawn > 0, "{name} drew nothing");
            let (on, off) = (on.finish(), off.finish());
            assert_eq!(on.trail, off.trail, "{name}");
            if let Some(recorded) = recorded.as_ref().and_then(|file| file.get(name)) {
                assert_eq!(recorded.first_difference(&on.trail), None, "{name}");
            }
        }
    }

    /// The whole of what a frame's view does but the facade's write, which `sapphire_core` holds to the
    /// same.
    #[test]
    fn drawing_allocates_nothing() {
        #[derive(Default)]
        struct Counted {
            shapes: u32,
            labels: u32,
        }
        impl Canvas for Counted {
            fn shape(&mut self, _shape: Draw, _style: Style) {
                self.shapes += 1;
            }
            fn label(&mut self, _at: Vec3, _style: Style, text: fmt::Arguments<'_>) {
                self.labels += u32::from(!LabelText::format(text).as_str().is_empty());
            }
        }
        assert!(alloc::is_installed());
        let mut player = player("melee-sweep");
        let mut counted = Counted::default();
        let mut allocations = 0;
        while player.step() {
            let sim = player.simulation();
            let ((), counts) = alloc::count_in_scope(|| {
                draw(CombatViews::ALL, sim, &NoVisiblePose, &mut counted);
            });
            allocations += counts.allocations;
        }
        assert!(counted.shapes > 1_000 && counted.labels > 1_000, "{} {}", counted.shapes, counted.labels);
        if PER_THREAD_COUNTS {
            assert_eq!(allocations, 0);
        }
    }

    #[cfg(feature = "dev")]
    #[test]
    #[ignore = "runs with CI's dev tests, which take --ignored"]
    fn the_panel_finds_these_cvars_by_their_prefix() {
        for cvar in &CVARS {
            assert!(cvar.name().starts_with(sapphire_debug::COMBAT_VIEW_CVARS), "{}", cvar.name());
        }
    }
}
