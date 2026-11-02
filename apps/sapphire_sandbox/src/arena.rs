//! The arena (`--arena`, M2 O5): the combat stage, and its one home.
//!
//! The look-dev town's square as a place to fight. It draws what the look-dev stage draws, on the
//! same day and under the same light rigs, because the town stays one mesh until M3's zones; what it
//! adds is what a fight needs of the place:
//!
//! - **Collision:** `content/arena/square.colliders.ron`. A replay's simulation compiles it in
//!   (`sapphire_gameplay::arena`), so a headless runner needs no assets. The stage loads the same
//!   document as a game would, cooked into its pack ([`COLLIDERS`]), and builds a `PhysicsWorld` from
//!   it ([`ArenaGround`]), which the camera reads. It refuses a pack whose colliders aren't this
//!   build's, and spawn points that don't stand where they should. `--debug-draw colliders` draws the
//!   set over the town, fight or no fight. `tools/sapphire_pipeline/tests/arena.rs` holds the
//!   document to the town's assembly, and this module's tests hold its trees to the stage's plantings.
//! - **The fight's space:** the square's paving between its three rows, 37 m across and 38 m deep,
//!   about 1,150 m² of it reachable. Three sides are the rows' fronts; the street's moon gate and the
//!   courtyard's are closed by blockers, and so is the paving's end on the open side, where the
//!   grass starts. The duel starts on the clear floor right of the pavilion, 11 m by 19 m with one
//!   lantern post in it, so a run or a dodge doesn't find a wall, with a camera's room behind it
//!   and the street's moon gate in the middle of the frame.
//! - **Spawn points:** `sapphire_gameplay::arena`, read from `content/arena/square.spawns.ron`.
//! - **Who stands there:** [`ArenaActors`]: the character, in the look-dev stage's held pose, at the
//!   player's spawn point, and the same character in the same pose as a stand-in at the enemy's.
//!   With `--replay` or `--record`, each actor's root follows `ActorTransform` a tick at a time.
//! - **The camera** (`--arena-shot`, [`ArenaShot`]): `sapphire_camera`'s director by default, on its
//!   combat rig, framing both fighters and colliding with the world. `follow` is a plain fixed follow,
//!   what the golden scenes pin, so a change of camera tuning moves none of them.
//!
//! # The seams
//!
//! The arena is where M2's slice meets. Each of these is the one place a package plugs in:
//!
//! - **Moving the player (G1, in):** the `arena` scenario (`sapphire_gameplay::replay`) gives
//!   everyone a capsule on the document's colliders, so a stick collides, steps and jumps.
//!   [`ArenaActors::follow`] reads `ActorView`, whatever moved it.
//! - **Live input (I1, in):** `--arena --record <file> --scenario arena` in a window plays by the
//!   bindings (`crate::combat::read_bindings`), turned by [`crate::scene::Scene::camera_heading`]. A
//!   fight with no file wants a `Combat` mode of its own, a recording with nowhere to write.
//! - **The camera (G5, in):** [`ArenaCamera`]. Look and zoom reach it through
//!   [`crate::scene::Scene::drive_arena_camera`]. Lock-on, shakes and a chosen target are the
//!   director's own calls, made on [`ArenaCamera::director`]; its target today is the enemy, always.
//! - **Moves (G2, in) and hits (G3):** the scenario loads the test moves and aims warps, so a script's
//!   `Buttons` start them; the script `arena-walk` (`replay/script.rs`) presses only jump. Hits ask the
//!   simulation's `PhysicsWorld`, in `CombatSet::Hits`.
//! - **Animation (N2, in; the graph, N3):** the two `RenderPose`s [`crate::scene::Scene::spawn_arena`]
//!   writes once. A `crate::animate::ClipPlayer` with its root motion off could write them each frame,
//!   with [`ArenaActors::present`] keeping the transform. What plays reads `ActorView::moves`.
//! - **The enemy (O4, in; G6):** its stand-in's mesh, in `spawn_arena`, where `crate::enemy`'s can go,
//!   and its brain in the scenario.
//! - **Effects (E2, `--vfx`) and the HUD (I2, `--hud`):** both draw over whatever scene is up, and
//!   read positions from [`ArenaActors`], which has what they need.

use std::f32::consts::FRAC_PI_2;

use sapphire_assets::AssetId;
use sapphire_camera::{CameraDirector, CameraFrame, CameraTuning, Rig, Subject};
use sapphire_colliders::{ArchivedColliderSet, ColliderError, Group};
use sapphire_core::debug_draw::{Color, Shape, Style};
use sapphire_ecs::prelude::*;
use sapphire_gameplay::arena::{SpawnPoint, cooked_colliders, profile, spawns};
use sapphire_gameplay::{ActorId, Simulation};
use sapphire_math::{Mat4, Quat, Vec2, Vec3};
use sapphire_physics::{ColliderSet, HitTarget, PhysicsWorld, Pose, QueryFilter, WorldError};

use crate::combat_views::{self, Canvas};
use crate::extract::RenderTransform;
use crate::orbit::OrbitCamera;

/// The arena's colliders, by their sidecar's ID (`content/arena/square.colliders.ron.meta`).
pub(crate) const COLLIDERS: AssetId = AssetId::from_text("773529bf-89bd-4e37-b9c7-1a687405b42f");

/// The scenario every replay through the arena starts from.
pub(crate) const SCENARIO: &str = "arena";

/// Which noise the director's shakes draw from. Nothing fires one yet.
const CAMERA_SEED: u64 = 7;

/// The plain camera's look: how high over the player's feet it aims, how far round from straight
/// behind, how far up, how far back, and its vertical field of view. A chest-high target and a low
/// angle keep the player in the frame's lower middle, at a distance where a dodge doesn't leave the
/// frame. The turn is the opponent's: straight behind, the player stands in front of it.
/// [`ArenaShot::camera`] says what replaces all of it.
const FOLLOW_AIM_HEIGHT: f32 = 1.35;
const FOLLOW_YAW_DEGREES: f32 = 14.0;
const FOLLOW_PITCH_DEGREES: f32 = 6.0;
const FOLLOW_DISTANCE: f32 = 5.2;
const FOLLOW_FOV_DEGREES: f32 = 48.0;

/// What frames the arena (`--arena-shot`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ArenaShot {
    /// `sapphire_camera`'s director, on its combat rig: what a player sees.
    #[default]
    Director,
    /// The plain camera: a fixed follow behind the player, looking up the square toward the street's
    /// moon gate. What the golden scenes pin, so that tuning the director moves none of them.
    Follow,
    /// From high over the square's middle, looking straight down, for checking its colliders against
    /// the town (`--debug-draw colliders`). Holds still.
    Over,
}

impl ArenaShot {
    pub const ALL: [Self; 3] = [Self::Director, Self::Follow, Self::Over];

    pub fn name(self) -> &'static str {
        match self {
            Self::Director => "director",
            Self::Follow => "follow",
            Self::Over => "over",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shot| shot.name() == name)
    }

    /// The camera, for a player whose feet are at `player`. `--view`, `--fov` and `--zoom` adjust it
    /// as they do a look-dev shot. The director replaces all of it on its first frame.
    pub fn camera(self, player: Vec3) -> OrbitCamera {
        match self {
            Self::Director | Self::Follow => OrbitCamera::shot(
                follow_target(player),
                FOLLOW_YAW_DEGREES.to_radians(),
                FOLLOW_PITCH_DEGREES.to_radians(),
                FOLLOW_DISTANCE,
                FOLLOW_FOV_DEGREES.to_radians(),
            ),
            Self::Over => {
                OrbitCamera::shot(Vec3::new(0.0, 0.0, 1.0), 0.0, FRAC_PI_2, 52.0, 50f32.to_radians())
            }
        }
    }
}

/// What the plain camera aims at, for a player whose feet are at `player`.
pub(crate) fn follow_target(player: Vec3) -> Vec3 {
    player + Vec3::new(0.0, FOLLOW_AIM_HEIGHT, 0.0)
}

/// Refuses a fight that doesn't start from the arena's scenario: its actors aren't standing here.
pub(crate) fn check_scenario(combat: &crate::combat::Combat) -> Result<(), String> {
    if combat.scenario() == SCENARIO {
        return Ok(());
    }
    Err(format!(
        "--arena plays a fight of the `{SCENARIO}` scenario, and this one starts from `{}`",
        combat.scenario(),
    ))
}

/// Why the stage can't stand on its cooked colliders.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ArenaError {
    #[error(transparent)]
    World(#[from] WorldError),
    #[error(transparent)]
    Document(#[from] ColliderError),
    #[error(
        "the pack's colliders aren't this build's, which a replay's simulation stands on: run `cargo xtask \
         content`"
    )]
    StaleCook,
    #[error("the {who}'s spawn point has nothing under it")]
    NoGround { who: &'static str },
    #[error("the {who}'s spawn point stands {height} m from the ground")]
    OffGround { who: &'static str, height: f32 },
    #[error("a character at the {who}'s spawn point would stand inside collider `{collider}`")]
    Overlapping { who: &'static str, collider: String },
}

/// The arena's collision as the stage loaded it: the statics a replay's simulation has too, for the
/// camera to read, and for `--debug-draw colliders` to draw while no fight has them.
pub(crate) struct ArenaGround {
    world: PhysicsWorld,
}

impl ArenaGround {
    /// Builds the world from the cooked `set`, holds it to this build's own document, and checks the
    /// spawn points stand on it.
    pub fn new(set: &ArchivedColliderSet) -> Result<Self, ArenaError> {
        let world = PhysicsWorld::from_cooked(set, 0)?;
        let spawns = spawns();
        check_spawn(&world, set, "player", spawns.player)?;
        check_spawn(&world, set, "enemy", spawns.enemy)?;
        let own = cooked_colliders()?;
        let own = PhysicsWorld::from_cooked(ColliderSet::read(&own)?, 0)?;
        if own.statics_digest() != world.statics_digest() {
            return Err(ArenaError::StaleCook);
        }
        Ok(Self { world })
    }

    /// Draws the spawn points, each a character's capsule and an arrow for which way it faces, and with
    /// `statics` every collider, as the combat views do.
    pub fn draw(&self, statics: bool, canvas: &mut impl Canvas) {
        if statics {
            combat_views::statics(&self.world, canvas);
        }
        let spawns = spawns();
        let profile = profile();
        for (spawn, color) in [(spawns.player, Color::GREEN), (spawns.enemy, Color::RED)] {
            let style = Style::new(color).on_top();
            let feet = spawn.position();
            let from = feet + Vec3::new(0.0, profile.radius, 0.0);
            let to = feet + Vec3::new(0.0, profile.height - profile.radius, 0.0);
            canvas.shape(Shape::Capsule { from, to, radius: profile.radius }, style);
            let facing = Quat::from_rotation_y(spawn.yaw()) * Vec3::Z;
            let from = feet + Vec3::new(0.0, profile.height * 0.6, 0.0);
            canvas.shape(Shape::Arrow { from, to: from + facing * 0.8 }, style);
        }
    }
}

/// Whether a character fits at `spawn`: on the ground, in nothing.
fn check_spawn(
    world: &PhysicsWorld,
    set: &ArchivedColliderSet,
    who: &'static str,
    spawn: SpawnPoint,
) -> Result<(), ArenaError> {
    let feet = spawn.position();
    let profile = profile().resolve().expect("the arena's profile is checked by its own test");
    let capsule = profile.capsule();
    let pose = Pose::at(feet + profile.feet_to_center());
    let everything = QueryFilter::new(Group::World.mask() | Group::CharacterBlocker.mask());
    let mut hits = [HitTarget::Static(sapphire_physics::ColliderId(0)); 1];
    if world.overlap(&capsule, pose, &everything, &mut hits).written > 0 {
        let HitTarget::Static(id) = hits[0] else { unreachable!("this world holds no bodies") };
        let collider = set.colliders[id.0 as usize].name.to_string();
        return Err(ArenaError::Overlapping { who, collider });
    }
    let ground = QueryFilter::new(Group::World.mask());
    let over = feet + Vec3::Y;
    let hit = world.cast_ray(over, Vec3::NEG_Y, 3.0, &ground).ok_or(ArenaError::NoGround { who })?;
    let height = 1.0 - hit.distance;
    if height.abs() > 1e-3 {
        return Err(ArenaError::OffGround { who, height });
    }
    Ok(())
}

/// An actor's root, as the fixed step left it, with the step before, to draw between.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Root {
    position: Vec3,
    yaw: f32,
    previous_position: Vec3,
    previous_yaw: f32,
}

impl Root {
    fn at(spawn: SpawnPoint) -> Self {
        let (position, yaw) = (spawn.position(), spawn.yaw());
        Self { position, yaw, previous_position: position, previous_yaw: yaw }
    }

    fn advance(&mut self, position: Vec3, yaw: f32) {
        (self.previous_position, self.previous_yaw) = (self.position, self.yaw);
        (self.position, self.yaw) = (position, yaw);
    }

    fn position(&self, alpha: f32) -> Vec3 {
        self.previous_position.lerp(self.position, alpha)
    }

    /// `alpha` of the way from the step before to the latest. The simulation's yaw is unwrapped, so
    /// this never sweeps the long way round.
    fn yaw(&self, alpha: f32) -> f32 {
        self.previous_yaw + (self.yaw - self.previous_yaw) * alpha
    }

    /// Where its entity draws, `alpha` of the way from the step before to the latest.
    fn transform(&self, alpha: f32) -> Mat4 {
        Mat4::from_rotation_translation(Quat::from_rotation_y(self.yaw(alpha)), self.position(alpha))
    }
}

/// `sapphire_camera`'s director as the arena runs it (M2 G5), and what the frame's clock and input
/// have given it since its last frame.
struct ArenaCamera {
    director: CameraDirector,
    dt: f32,
    look: Vec2,
    zoom: f32,
}

impl ArenaCamera {
    /// On its combat rig, behind the player, as it faces at its spawn point. Its first frame is a cut,
    /// so it opens settled.
    fn new() -> Self {
        let tuning = CameraTuning::default_tuning();
        let pitch = tuning.explore.pitch;
        let mut director =
            CameraDirector::new(tuning, CAMERA_SEED).expect("the shipped tuning is checked by its own test");
        director.set_rig(Rig::Combat, true);
        director.look_from(spawns().player.yaw(), pitch);
        Self { director, dt: 0.0, look: Vec2::ZERO, zoom: 0.0 }
    }
}

/// The player's and the enemy's entities and where the simulation has them: what the scene carries
/// from the simulation to the frame (rule R4).
pub(crate) struct ArenaActors {
    player: (Entity, Root),
    enemy: (Entity, Root),
    /// The plain camera follows the player, unless another shot holds still.
    follows: bool,
    /// The director, on its own shot.
    camera: Option<ArenaCamera>,
    ground: ArenaGround,
    /// Whether a fight has stepped, which draws the colliders itself.
    fighting: bool,
}

impl ArenaActors {
    /// The two at their spawn points, drawn by `player` and `enemy`.
    pub fn new(player: Entity, enemy: Entity, shot: ArenaShot, ground: ArenaGround) -> Self {
        let spawns = spawns();
        Self {
            player: (player, Root::at(spawns.player)),
            enemy: (enemy, Root::at(spawns.enemy)),
            follows: shot == ArenaShot::Follow,
            camera: (shot == ArenaShot::Director).then(ArenaCamera::new),
            ground,
            fighting: false,
        }
    }

    /// The enemy's entity: it draws the character's mesh and holds its pose.
    pub fn enemy_entity(&self) -> Entity {
        self.enemy.0
    }

    /// Takes the fixed step's roots from `simulation`, after each of its steps. `cast` is the
    /// scenario's, the player first.
    pub fn follow(&mut self, simulation: &Simulation, cast: &[ActorId]) {
        self.fighting = true;
        for (id, (_, root)) in cast.iter().zip([&mut self.player, &mut self.enemy]) {
            if let Some(actor) = simulation.actor(*id) {
                root.advance(actor.position, actor.yaw);
            }
        }
    }

    /// What the frame's clock and input give the director, if it's on: real seconds, unscaled, and the
    /// look and zoom `sapphire_controls` gives. They wait for its next frame. Says whether it took them.
    pub fn drive_camera(&mut self, dt: f32, look: Vec2, zoom: f32) -> bool {
        let Some(camera) = &mut self.camera else { return false };
        camera.dt += dt;
        camera.look += look;
        camera.zoom += zoom;
        true
    }

    /// Which way the director looks across the ground, if it's on, for the next tick's stick.
    pub fn heading(&self) -> Option<f32> {
        self.camera.as_ref().map(|camera| camera.director.heading())
    }

    /// Poses the entities `alpha` of the way between their last two steps, and aims the camera: at the
    /// head of a frame, as the turntable poses its own entity in `Extract`. Not a system, which would
    /// cost an allocation a frame (see [`crate::scene`]). With no `aspect` nothing draws, so the
    /// director waits.
    pub fn present(&mut self, world: &mut World, alpha: f32, aspect: Option<f32>) {
        for (entity, root) in [&self.player, &self.enemy] {
            if let Some(mut transform) = world.get_mut::<RenderTransform>(*entity) {
                transform.set_if_neq(RenderTransform(root.transform(alpha)));
            }
        }
        let feet = self.player.1.position(alpha);
        if self.follows {
            world.resource_mut::<OrbitCamera>().set_target(follow_target(feet));
        }
        if let (Some(camera), Some(aspect)) = (&mut self.camera, aspect) {
            let profile = profile();
            let subject = |position| Subject { position, height: profile.height, radius: profile.radius };
            let frame = CameraFrame {
                dt: std::mem::take(&mut camera.dt),
                player: subject(feet),
                player_yaw: self.player.1.yaw(alpha),
                target: Some(subject(self.enemy.1.position(alpha))),
                look: std::mem::take(&mut camera.look),
                zoom: std::mem::take(&mut camera.zoom),
                aspect,
            };
            let pose = camera.director.update(&frame, &self.ground.world);
            world.resource_mut::<OrbitCamera>().set_view(pose.eye, pose.forward(), pose.fov_y);
        }
    }

    /// The collider view's share of the arena (`combat.view_colliders`, which `--debug-draw colliders`
    /// turns on): the spawn points, and the colliders until a fight's own view draws those
    /// ([`crate::combat::Combat::draw_views`]), so the view works with no fight and draws nothing twice.
    pub fn draw_colliders(&self, canvas: &mut impl Canvas) {
        self.ground.draw(!self.fighting, canvas);
    }
}

#[cfg(test)]
mod tests {
    use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderSet, ColliderShape};
    use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

    use super::*;
    use crate::combat_views::{CombatViews, NoVisiblePose};
    use crate::lookdev::{PLANTINGS, Plant};

    const DOCUMENT: &str = include_str!("../../../content/arena/square.colliders.ron");

    fn document() -> ColliderDocument {
        ColliderDocument::from_ron(DOCUMENT).expect("the document reads")
    }

    fn entry_at_ground(entry: &ColliderEntry) -> Vec3 {
        Vec3::new(entry.position[0], 0.0, entry.position[2])
    }

    /// The stage plants its trees, so the town's assembly can't say where they stand: this does. Each
    /// tree on the ground in the arena, or in a planter in it, has a trunk collider at its root, and
    /// each trunk has a tree.
    #[test]
    fn trunks_stand_on_the_stages_trees() {
        let document = document();
        let trunks: Vec<&ColliderEntry> =
            document.colliders.iter().filter(|entry| entry.name.starts_with("tree_")).collect();
        let is_tree = |plant: Plant| {
            matches!(
                plant,
                Plant::GreenTree
                    | Plant::BlossomTree
                    | Plant::AutumnTree
                    | Plant::KitTree
                    | Plant::KitTreeRound
            )
        };
        let in_arena = |at: [f32; 3]| at[0].abs() < 18.6 && (-17.5..20.3).contains(&at[2]);
        let trees: Vec<([f32; 3], Plant)> = PLANTINGS
            .iter()
            .filter(|(plant, at, ..)| is_tree(*plant) && in_arena(*at))
            .map(|&(plant, at, ..)| (at, plant))
            .collect();
        assert!(trees.len() >= 4, "{trees:?}");
        for (at, plant) in &trees {
            let root = Vec3::new(at[0], 0.0, at[2]);
            assert!(
                trunks.iter().any(|trunk| entry_at_ground(trunk).distance(root) < 0.05),
                "the {plant:?} at {at:?} has no trunk",
            );
        }
        for trunk in trunks {
            let root = entry_at_ground(trunk);
            let tree = trees.iter().find(|(at, _)| Vec3::new(at[0], 0.0, at[2]).distance(root) < 0.05);
            let Some((at, _)) = tree else { panic!("{}: no tree stands at {root}", trunk.name) };
            // A trunk's cylinder starts at its root.
            let ColliderShape::Capsule { half_height, .. } = trunk.shape else {
                panic!("{}: a capsule", trunk.name)
            };
            let foot = trunk.position[1] - half_height;
            assert!((foot - at[1]).abs() < 0.05, "{}: its foot at {foot}, its root at {}", trunk.name, at[1]);
        }
    }

    #[test]
    fn the_colliders_id_is_the_sidecars() {
        let sidecar = include_str!("../../../content/arena/square.colliders.ron.meta");
        let id = sidecar.split("id: \"").nth(1).and_then(|rest| rest.split('"').next()).expect("an ID");
        assert_eq!(AssetId::from_text(id), COLLIDERS);
    }

    #[test]
    fn the_scenario_is_listed() {
        assert!(SCENARIOS.iter().any(|scenario| scenario.name == SCENARIO));
    }

    #[test]
    fn shots_parse_by_name_and_the_plain_camera_stands_behind_the_player() {
        for shot in ArenaShot::ALL {
            assert_eq!(ArenaShot::from_name(shot.name()), Some(shot));
        }
        assert_eq!(ArenaShot::from_name("near"), None);

        assert_eq!(ArenaShot::default(), ArenaShot::Director);
        let player = spawns().player.position();
        let eye = ArenaShot::Follow.camera(player).eye();
        assert!(eye.z > player.z, "behind the player, who faces -Z: {eye}");
        assert!(eye.y > FOLLOW_AIM_HEIGHT, "a little over its head's height: {eye}");
        assert!(eye.distance(follow_target(player)) > 3.0, "room for a dodge: {eye}");
        assert!(ArenaShot::Over.camera(player).eye().y > 40.0);
    }

    fn built() -> ArenaGround {
        let bytes = document().cook().expect("it cooks").to_cooked().expect("it writes");
        ArenaGround::new(ColliderSet::read(&bytes).expect("it reads")).expect("the stage stands on it")
    }

    /// What loads the stage holds spawn points to what's cooked: on the ground, and in nothing.
    #[test]
    fn a_collider_over_a_spawn_point_stops_the_stage() {
        let _ = built();
        let spawn = spawns().player;
        let mut document = document();
        document.colliders.push(ColliderEntry {
            name: "moved_here".to_owned(),
            shape: ColliderShape::Box { half_extents: [0.5, 1.0, 0.5] },
            position: [spawn.position[0], 1.0, spawn.position[2]],
            rotation: Default::default(),
            groups: vec![Group::World],
        });
        let bytes = document.cook().expect("it cooks").to_cooked().expect("it writes");
        let error = match ArenaGround::new(ColliderSet::read(&bytes).expect("it reads")) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("the spawn point is inside it"),
        };
        assert!(error.contains("moved_here"), "{error}");
    }

    /// What a canvas is asked to draw, whatever it is.
    #[derive(Default)]
    struct Drawn(usize);

    impl Canvas for Drawn {
        fn shape(&mut self, _shape: Shape, _style: Style) {
            self.0 += 1;
        }

        fn label(&mut self, _at: Vec3, _style: Style, _text: std::fmt::Arguments<'_>) {}
    }

    /// The collider view shows the arena's set with no fight running, and through a fight the fight's
    /// own view has them: the stage adds only its spawn points, so nothing draws twice.
    #[test]
    fn the_colliders_draw_once_with_or_without_a_fight() {
        const SPAWN_SHAPES: usize = 4;
        let cooked = cooked_colliders().expect("it cooks");
        let ground = ArenaGround::new(ColliderSet::read(&cooked).expect("it reads")).expect("it stands");
        let colliders = ground.world.statics().len();
        let mut world = World::new();
        let (player, enemy) = (world.spawn_empty().id(), world.spawn_empty().id());
        let mut actors = ArenaActors::new(player, enemy, ArenaShot::Follow, ground);

        let mut alone = Drawn::default();
        actors.draw_colliders(&mut alone);
        assert_eq!(alone.0, colliders + SPAWN_SHAPES);

        let scenario = find_scenario(SCENARIOS, SCENARIO).expect("listed");
        let (sim, cast) = scenario.start(scenario.config).expect("it starts");
        actors.follow(&sim, &cast);
        let mut beside = Drawn::default();
        actors.draw_colliders(&mut beside);
        assert_eq!(beside.0, SPAWN_SHAPES);

        let mut fight = Drawn::default();
        combat_views::draw(CombatViews::COLLIDERS, &sim, &NoVisiblePose, &mut fight);
        assert!(fight.0 >= colliders, "{} shapes for {colliders} colliders", fight.0);
    }

    /// A pack from another build draws and frames another square from the one the fight stands on.
    #[test]
    fn a_pack_that_isnt_this_builds_stops_the_stage() {
        let own = cooked_colliders().expect("it cooks");
        assert!(ArenaGround::new(ColliderSet::read(&own).expect("it reads")).is_ok());

        let mut document = document();
        document.colliders[1].position[0] += 0.25;
        let bytes = document.cook().expect("it cooks").to_cooked().expect("it writes");
        assert!(matches!(
            ArenaGround::new(ColliderSet::read(&bytes).expect("it reads")),
            Err(ArenaError::StaleCook)
        ));
    }

    #[test]
    fn a_root_draws_between_its_last_two_steps() {
        let mut root = Root::at(spawns().player);
        let start = spawns().player.position();
        root.advance(start + Vec3::X, 0.5);
        assert_eq!(root.position(0.0), start);
        assert_eq!(root.position(1.0), start + Vec3::X);
        assert!(root.position(0.5).distance(start + Vec3::X * 0.5) < 1e-5);
        let drawn = root.transform(1.0);
        assert_eq!(drawn.transform_point3(Vec3::ZERO), start + Vec3::X);
        let forward = drawn.transform_vector3(Vec3::Z);
        assert!((forward - Quat::from_rotation_y(0.5) * Vec3::Z).length() < 1e-5, "{forward}");
    }
}
