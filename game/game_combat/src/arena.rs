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
//!   player's spawn point, and the enemy (`crate::enemy`) and its blade at the enemy's. With `--replay`,
//!   `--record` or `--fight`, each actor's root follows `ActorTransform` a tick at a time.
//! - **The camera** (`--arena-shot`, [`ArenaShot`]): `sapphire_camera`'s director by default, on its
//!   combat rig, framing both fighters and colliding with the world. `follow` is a plain fixed follow,
//!   what the golden scenes pin, so a change of camera tuning moves none of them. The tests' `framing`
//!   module counts what the director shows over whole fights, and holds it to its bounds.
//!
//! # The seams
//!
//! The arena is where M2's slice meets. Each of these is the one place a package plugs in:
//!
//! - **Moving the player (G1, in):** the `arena` scenario (`sapphire_gameplay::replay`) gives
//!   everyone a capsule on the document's colliders, so a stick collides, steps and jumps.
//!   [`ArenaActors::follow`] reads `ActorView`, whatever moved it.
//! - **Live input (I1 and G6, in):** `--arena --fight` in a window is player against brain with no file
//!   (`crate::combat`), played by the bindings (`crate::combat::read_bindings`), turned by
//!   [`sapphire_stage::scene::Scene::camera_heading`], and `--script` drives the same path without a window.
//!   `--arena --record <file> --scenario arena-fight` keeps one, as far as it's got at each trip to the
//!   background. A new fight is a new simulation, and [`ArenaActors::follow`] sees its tick go back:
//!   everything stands where it now is, and the camera cuts, which
//!   [`sapphire_stage::scene::Scene::follow_simulation`] passes on to the renderer as the view's cut, for temporal AA
//!   and the shadows. A party's swap doesn't: who comes in stands where the one who left stood.
//! - **The camera (G5 and G6, in):** [`ArenaCamera`]. Look and zoom reach it through
//!   [`sapphire_stage::scene::Scene::drive_arena_camera`]. Its target is `sapphire_gameplay::targeting`'s soft lock,
//!   chosen each frame by [`ArenaActors::present`], so a fallen enemy, or one out of range, stops being
//!   framed. The player's lock-on button holds the line to it, through the director's own rig
//!   ([`ArenaActors::set_lock_on`]). Shakes are the director's own calls too, for E3, made on
//!   [`ArenaCamera::director`], which [`ArenaActors::director`] hands to `crate::cues`. `OrbitCamera::set_view`
//!   keeps the view upright, so a shake's roll goes through `OrbitCamera::set_shake` ([`roll_of`]).
//! - **Moves (G2, O3, in) and hits (G3):** `arena-fight` loads the player's real moves and the
//!   `arena` scenario the test moves, and both aim warps, so a script's `Buttons` start them; the script
//!   `arena-walk` (`replay/script.rs`) presses only jump. Hits ask the simulation's `PhysicsWorld`, in
//!   `CombatSet::Hits`.
//! - **Animation (N3, in):** `sapphire_stage::scene::Scene::play_fighters` gives each fighter a graph, and each
//!   frame [`ArenaActors::graph_input`] turns what the last step said of it into what its graph plays:
//!   its move at the move's own ticks, its feet from its controller, on its own clock. No root motion comes
//!   from a clip: [`ArenaActors::present`] keeps the transform, and the blade with it, through
//!   [`ArenaActors::hold_blade`].
//! - **IK (N4, in):** each fighter's graph driver casts for its feet in this world
//!   ([`ArenaGround::share`]), the same `Arc` the camera reads, and looks at the other
//!   ([`ArenaActors::look_target`]). `--clip <name> --ik on` casts here too.
//! - **The enemy (O4 and G6, in):** `crate::enemy`'s model and blade, in `spawn_arena`, and its brain in
//!   the `arena-fight` scenario. The `arena` scenario's opponent draws as the same warrior and has no
//!   brain: it just stands there.
//! - **The party (G7, in):** `--party` with the `party` scenario. [`ArenaActors::follow`] takes the player
//!   from whoever's on the field, cutting its graph at a swap, and `crate::party_looks` draws that
//!   member's recolor.
//! - **Effects and cues (E2, E3, in):** `--cues` fires a fight's cues where the simulation has its
//!   actors (`crate::cues::CueStage::Actors`), and `--vfx` draws over whatever scene is up.
//! - **The HUD (I2, in):** `--hud live` shows what [`ArenaActors::hud_values`] reads of the fighters.
//! - **Touch (I3, in):** the HUD's zones reach `TickActions` through combat's bindings
//!   (`sapphire_controls::CombatControls::read_touch`), so a fight in a window is played on touch, a
//!   pad or a keyboard (`docs/m2-exit-run.md` has a phone's run).

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;
use std::time::Duration;

use sapphire_anim::{GraphInput, ReactionInput};
use sapphire_assets::AssetId;
use sapphire_camera::{CameraDirector, CameraFrame, CameraTuning, Rig, Subject};
use sapphire_colliders::{ArchivedColliderSet, ColliderError, Group};
use sapphire_core::debug_draw::{Color, Shape, Style};
use sapphire_ecs::prelude::*;
use sapphire_gameplay::arena::{SpawnPoint, cooked_colliders, profile, spawns};
use sapphire_gameplay::targeting::{TargetCandidate, TargetQuery, TargetingWeights, choose_target};
use sapphire_gameplay::{ActorId, ActorView, Buttons, Simulation, Stick};
use sapphire_math::{Mat4, Quat, Vec2, Vec3};
use sapphire_physics::{ColliderSet, HitTarget, PhysicsWorld, Pose, QueryFilter, WorldError};
use sapphire_time::TICK_HZ;
use sapphire_ui::{FighterValues, HudValues, SkillValues};

use crate::combat_views::{self, Canvas};
use crate::enemy;
use sapphire_stage::RenderTransform;
use sapphire_stage::anim_graph::{combat_input, duration_ticks, locomotion_of, time_ticks};
use sapphire_stage::orbit::OrbitCamera;

/// The arena's colliders, by their sidecar's ID (`content/arena/square.colliders.ron.meta`).
pub(crate) const COLLIDERS: AssetId = AssetId::from_text("773529bf-89bd-4e37-b9c7-1a687405b42f");

/// The scenario a replay of the arena on its own starts from.
pub(crate) const SCENARIO: &str = "arena";

/// The scenario of a fight with the enemy (M2 G6): `--fight`, and the replays made from it.
pub const FIGHT_SCENARIO: &str = "arena-fight";

/// The scenario of a party's fight with the enemy (M2 G7): `--fight --party`, and the replays made from it.
pub const PARTY_SCENARIO: &str = "party";

/// Who a soft lock can find before a simulation says: the second actor a scenario spawns.
const ENEMY_ID: ActorId = ActorId::from_raw(2).expect("not zero");

/// Which noise the director's shakes draw from.
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
pub enum ArenaShot {
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
    pub(crate) fn camera(self, player: Vec3) -> OrbitCamera {
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

/// Refuses a fight that doesn't start from the arena's scenarios, or with `party` the party's: its actors
/// aren't standing here, or there aren't three looks to draw them in.
pub fn check_scenario(combat: &crate::combat::Combat, party: bool) -> Result<(), String> {
    let scenario = combat.scenario();
    if party {
        if scenario == PARTY_SCENARIO {
            return Ok(());
        }
        return Err(format!(
            "--arena --party plays a fight of the `{PARTY_SCENARIO}` scenario, and this one starts from \
             `{scenario}`"
        ));
    }
    if [SCENARIO, FIGHT_SCENARIO].contains(&scenario) {
        return Ok(());
    }
    Err(format!(
        "--arena plays a fight of the `{SCENARIO}` scenario or of `{FIGHT_SCENARIO}`, and this one starts \
         from `{scenario}`",
    ))
}

/// Why the stage can't stand on its cooked colliders.
#[derive(Debug, thiserror::Error)]
pub enum ArenaError {
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
pub struct ArenaGround {
    world: Arc<PhysicsWorld>,
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
        Ok(Self { world: Arc::new(world) })
    }

    /// The world, for what else reads it: IK's casts for a character's feet ([`sapphire_stage::ik::IkDriver`]).
    pub fn share(&self) -> Arc<PhysicsWorld> {
        Arc::clone(&self.world)
    }

    /// Draws the spawn points, each a character's capsule and an arrow for which way it faces, and with
    /// `statics` every collider, as the combat views do.
    pub(crate) fn draw(&self, statics: bool, canvas: &mut impl Canvas) {
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
    let profile = sapphire_gameplay::ResolvedProfile::new(&profile())
        .expect("the arena's profile is checked by its own test");
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
        Self::standing(spawn.position(), spawn.yaw())
    }

    /// Here, with nothing to draw between.
    fn standing(position: Vec3, yaw: f32) -> Self {
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

/// What draws the enemy: its model and its blade, which is drawn on its own.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArenaEnemy {
    pub(crate) warrior: Entity,
    pub(crate) blade: Entity,
    /// The blade's mesh in the warrior's model space, where its pose has the hand.
    pub(crate) blade_in_model: Mat4,
    /// The joint that holds it.
    pub(crate) hand: usize,
}

/// How high the character's eyes stand over its feet, in metres: what the warrior looks at.
const PLAYER_EYES: f32 = 1.5;

/// What a fighter's graph hears from the steps between one frame and the next.
#[derive(Clone, Copy, Debug, Default)]
struct FighterSignals {
    /// It landed since the last frame.
    landed: bool,
    /// A new fight stood it where it stands since the last frame.
    cut: bool,
}

/// How far a view is turned about where it looks, from upright, as [`OrbitCamera::set_shake`] takes it:
/// radians about its backward.
fn roll_of(rotation: Quat) -> f32 {
    let forward = rotation * sapphire_math::FORWARD;
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let upright = right.cross(forward);
    let up = rotation * Vec3::Y;
    (-up.dot(right)).atan2(up.dot(upright))
}

/// One fighter's bars, for the HUD: `points` asks for its health in digits.
fn fighter_values(view: &ActorView, points: bool) -> FighterValues {
    let share = |left: u32, whole: u32| if whole == 0 { 1.0 } else { left as f32 / whole as f32 };
    let fighter = &view.fighter;
    FighterValues {
        hp: share(fighter.health(), fighter.max_health()),
        poise: share(fighter.poise(), fighter.max_poise()),
        hp_points: points.then(|| fighter.health()),
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

    /// Behind the player again and settled, for a new fight, whatever rigs are on, with no shake of the
    /// last one.
    fn start_over(&mut self) {
        let pitch = self.director.tuning().explore.pitch;
        self.director.look_from(spawns().player.yaw(), pitch);
        self.director.cut();
        self.director.stop_shakes();
    }
}

/// The player's and the enemy's entities and where the simulation has them: what the scene carries
/// from the simulation to the frame (rule R4).
pub struct ArenaActors {
    player: (Entity, Root),
    enemy: (Entity, Root),
    /// Its blade, and where that is in the enemy's model space.
    blade: (Entity, Mat4),
    /// The enemy's joint that holds its blade.
    hand: usize,
    /// What the last step said of each of them, for what poses them to read. Nothing before a fight.
    seen: [Option<ActorView>; 2],
    /// What each one's graph has yet to hear.
    signals: [FighterSignals; 2],
    /// The enemy, as the simulation calls it.
    enemy_id: ActorId,
    /// Who the soft lock has now.
    target: Option<ActorId>,
    /// What the soft lock weighs ([`Self::set_played_on_touch`]).
    weights: TargetingWeights,
    /// Whether the player's lock-on is on.
    locked_on: bool,
    /// The simulation's tick when it last stepped: going back, it's a new fight.
    tick: u64,
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
    pub(crate) fn new(player: Entity, enemy: ArenaEnemy, shot: ArenaShot, ground: ArenaGround) -> Self {
        let spawns = spawns();
        Self {
            player: (player, Root::at(spawns.player)),
            enemy: (enemy.warrior, Root::at(spawns.enemy)),
            blade: (enemy.blade, enemy.blade_in_model),
            hand: enemy.hand,
            seen: [None; 2],
            signals: [FighterSignals::default(); 2],
            enemy_id: ENEMY_ID,
            target: None,
            weights: TargetingWeights::SOFT_LOCK,
            locked_on: false,
            tick: 0,
            follows: shot == ArenaShot::Follow,
            camera: (shot == ArenaShot::Director).then(ArenaCamera::new),
            ground,
            fighting: false,
        }
    }

    /// The enemy's entity.
    #[cfg(test)]
    pub(crate) fn enemy_entity(&self) -> Entity {
        self.enemy.0
    }

    /// Who fights here, the player first, for whatever poses them: each one's entity, and what the
    /// last step said of it. Nothing's said before a fight.
    pub(crate) fn fighters(&self) -> [(Entity, Option<&ActorView>); 2] {
        [(self.player.0, self.seen[0].as_ref()), (self.enemy.0, self.seen[1].as_ref())]
    }

    /// What `fighter`'s graph plays this frame, the player being 0, `alpha` of the way from its last step
    /// to its latest, as its transform draws. A frozen clock asks the same thing at every `alpha`, and
    /// a defeated fighter keeps its last move's end once that move's over (`GraphInput::defeated`).
    /// Nothing before a fight.
    pub(crate) fn graph_input(&mut self, fighter: usize, alpha: f32) -> Option<GraphInput> {
        let view = self.seen[fighter].as_ref()?;
        let signals = &mut self.signals[fighter];
        let alpha = if alpha >= 0.0 { f64::from(alpha.min(1.0)) } else { 0.0 };
        let behind = (1.0 - alpha) * duration_ticks(view.clock_delta);
        let mut locomotion = locomotion_of(view);
        locomotion.landed = std::mem::take(&mut signals.landed);
        Some(GraphInput {
            ticks: time_ticks(view.clock_now) - behind,
            locomotion,
            combat: combat_input(view, behind),
            reaction: ReactionInput::default(),
            cut: std::mem::take(&mut signals.cut),
            defeated: view.fighter.is_defeated(),
        })
    }

    /// What `fighter` looks at: the other's head, while the other's up to look at.
    pub(crate) fn look_target(&self, fighter: usize, alpha: f32) -> Option<Vec3> {
        let other = 1 - fighter;
        self.seen[other].as_ref().filter(|view| !view.fighter.is_defeated())?;
        let height = if other == 0 { PLAYER_EYES } else { enemy::EYES };
        Some(self.drawn(other, alpha).w_axis.truncate() + Vec3::Y * height)
    }

    /// Where `fighter` draws, `alpha` of the way between its last two steps.
    pub(crate) fn drawn(&self, fighter: usize, alpha: f32) -> Mat4 {
        if fighter == 0 {
            self.player.1.transform(alpha)
        } else {
            self.enemy.1.transform(alpha) * Mat4::from_scale(Vec3::splat(enemy::SCALE))
        }
    }

    /// The entity that draws the enemy's blade.
    pub(crate) fn enemy_blade(&self) -> Entity {
        self.blade.0
    }

    /// The enemy's joint that holds its blade.
    pub(crate) fn blade_hand(&self) -> usize {
        self.hand
    }

    /// A new version of the enemy: the joint that holds its blade, and where that is in its held pose.
    pub(crate) fn follow_new_enemy(&mut self, hand: usize, blade_in_model: Mat4) {
        self.hand = hand;
        self.blade.1 = blade_in_model;
    }

    /// What the fighters stand on, for whatever casts for their feet.
    pub(crate) fn ground(&self) -> Arc<PhysicsWorld> {
        self.ground.share()
    }

    /// Tells `fighter`'s graph to cut, for hot reload's tests of when it hears.
    #[cfg(all(test, feature = "dev"))]
    pub(crate) fn cut_next(&mut self, fighter: usize) {
        self.signals[fighter].cut = true;
    }

    /// Whether `fighter`'s graph has yet to hear of a cut.
    #[cfg(all(test, feature = "dev"))]
    pub(crate) fn has_cut_waiting(&self, fighter: usize) -> bool {
        self.signals[fighter].cut
    }

    /// Where the enemy's pose has the hand that holds its blade, in its model space: from a pose that
    /// moves, each frame. [`Self::present`] draws the blade there.
    pub(crate) fn hold_blade(&mut self, blade_in_model: Mat4) {
        self.blade.1 = blade_in_model;
    }

    /// What the HUD shows of the fighters, once a fight has stepped.
    pub(crate) fn hud_values(&self) -> Option<HudValues> {
        let player = self.seen[0].as_ref()?;
        let enemy = self.seen[1].as_ref().map(|enemy| fighter_values(enemy, false));
        Some(HudValues { player: fighter_values(player, true), enemy, skill: SkillValues::default() })
    }

    /// How long the fight's gone on, for a HUD with no clock of its own.
    pub(crate) fn fight_time(&self) -> Duration {
        Duration::from_nanos(self.tick.saturating_mul(1_000_000_000 / u64::from(TICK_HZ)))
    }

    /// The camera director, if it's on: what a fight's shakes go to.
    pub fn director(&mut self) -> Option<&mut CameraDirector> {
        self.camera.as_mut().map(|camera| &mut camera.director)
    }

    /// The director, if it's on, and what the last step said of each fighter, the player first: what a
    /// fight's cues read together.
    pub(crate) fn cue_stage(&mut self) -> (Option<&mut CameraDirector>, [Option<&ActorView>; 2]) {
        let [player, enemy] = &self.seen;
        (self.camera.as_mut().map(|camera| &mut camera.director), [player.as_ref(), enemy.as_ref()])
    }

    /// Holds the line from the player to whoever the soft lock has, or lets go: the director's own rig,
    /// which blends in and out over its own half-life.
    pub(crate) fn set_lock_on(&mut self, on: bool) {
        self.locked_on = on;
        if let Some(camera) = &mut self.camera {
            camera.director.set_rig(Rig::LockOn, on);
        }
    }

    /// Takes the fixed step's roots from `simulation`, after each of its steps. `cast` is the
    /// scenario's, the player first. A tick that went back is a new fight, which stands where it stands
    /// with the camera cut to it, rather than sliding back across the square. Returns whether someone else
    /// came in for each of the two.
    pub(crate) fn follow(&mut self, simulation: &Simulation, cast: &[ActorId]) -> [bool; 2] {
        self.fighting = true;
        let mut came_in = [false; 2];
        let tick = simulation.tick();
        let started_over = tick < self.tick;
        self.tick = tick;
        for (index, (id, (_, root))) in cast.iter().zip([&mut self.player, &mut self.enemy]).enumerate() {
            // A cast names a party by its first member: the player is whoever's on the field.
            let actor = simulation.actor(simulation.party().on_field(*id));
            // Someone else came in: what's drawn starts on them, with no blend from who left.
            let swapped = self.seen[index].zip(actor).is_some_and(|(seen, actor)| seen.id != actor.id);
            came_in[index] = swapped;
            self.seen[index] = actor;
            let Some(actor) = actor else { continue };
            let signals = &mut self.signals[index];
            signals.landed |= actor.controller.state.events.landed;
            signals.cut |= swapped;
            if started_over {
                *signals = FighterSignals { landed: false, cut: true };
            }
            if started_over {
                *root = Root::standing(actor.position, actor.yaw);
            } else {
                root.advance(actor.position, actor.yaw);
            }
        }
        if let Some(enemy) = cast.get(1) {
            self.enemy_id = *enemy;
        }
        if started_over {
            self.target = None;
            if let Some(camera) = &mut self.camera {
                camera.start_over();
            }
        }
        // The press rather than the hold: a button that's held stays locked, and pressing again lets go.
        let pressed = self.seen[0].is_some_and(|player| player.controls.pressed().contains(Buttons::LOCK_ON));
        if pressed {
            self.set_lock_on(!self.locked_on);
        }
        came_in
    }

    /// Says whether a thumb is what's playing this fight (`InputSystem::last_device`), so the soft lock
    /// weighs [`TargetingWeights::TOUCH`] rather than a pad's aim (pillar 7). Only the camera reads the
    /// choice: nothing in a step does, since a move warps to the target its scenario gave it. So this
    /// moves no checksum, and a replay, with no device, always frames as a pad would. Whatever comes to
    /// read the choice in a step has to take these weights from the recording too.
    pub(crate) fn set_played_on_touch(&mut self, touch: bool) {
        self.weights = if touch { TargetingWeights::TOUCH } else { TargetingWeights::SOFT_LOCK };
    }

    /// Who the player's soft lock has at this frame: [`choose_target`], from where the player stands and
    /// points, among those still standing.
    fn choose_target(&mut self, alpha: f32) -> Option<Vec3> {
        let standing = self.seen[1].is_none_or(|enemy| !enemy.fighter.is_defeated());
        let enemy = TargetCandidate { id: self.enemy_id, position: self.enemy.1.position(alpha) };
        let candidates = standing.then_some(enemy);
        let query = TargetQuery {
            position: self.player.1.position(alpha),
            yaw: self.player.1.yaw(alpha),
            stick: self.seen[0].map_or(Stick::CENTER, |player| player.controls.current.stick),
            current: self.target,
            candidates: candidates.as_slice(),
        };
        self.target = choose_target(&query, &self.weights);
        self.target.map(|_| enemy.position)
    }

    /// What the frame's clock and input give the director, if it's on: real seconds, unscaled, and the
    /// look and zoom `sapphire_controls` gives. They wait for its next frame. Says whether it took them.
    pub(crate) fn drive_camera(&mut self, dt: f32, look: Vec2, zoom: f32) -> bool {
        let Some(camera) = &mut self.camera else { return false };
        camera.dt += dt;
        camera.look += look;
        camera.zoom += zoom;
        true
    }

    /// Whether the director frames this stage.
    #[cfg(feature = "dev")]
    pub(crate) fn has_director(&self) -> bool {
        self.camera.is_some()
    }

    /// Gives the director new tuning, whose goals its view eases to (M2 T2). Refuses with no director on
    /// the stage, or tuning it won't take, keeping what it has either way. Shake IDs resolved from the old
    /// tuning are resolved again by [`Scene::set_camera_tuning`](sapphire_stage::scene::Scene::set_camera_tuning),
    /// which holds the cues' dispatcher beside this.
    #[cfg(feature = "dev")]
    pub(crate) fn set_camera_tuning(&mut self, tuning: CameraTuning) -> Result<(), String> {
        let Some(camera) = &mut self.camera else {
            return Err("this shot has no camera director: `--arena-shot director` has one".to_owned());
        };
        camera.director.set_tuning(tuning).map_err(|error| error.to_string())
    }

    /// Which way the director looks across the ground, if it's on, for the next tick's stick.
    pub(crate) fn heading(&self) -> Option<f32> {
        self.camera.as_ref().map(|camera| camera.director.heading())
    }

    /// Poses the entities `alpha` of the way between their last two steps, and aims the camera: at the
    /// head of a frame, as the turntable poses its own entity in `Extract`. Not a system, which would
    /// cost an allocation a frame (see [`sapphire_stage::scene`]). With no `aspect` nothing draws, so the
    /// director waits.
    pub(crate) fn present(&mut self, world: &mut World, alpha: f32, aspect: Option<f32>) {
        let warrior = self.drawn(1, alpha);
        let drawn = [
            (self.player.0, self.drawn(0, alpha)),
            (self.enemy.0, warrior),
            (self.blade.0, warrior * self.blade.1),
        ];
        for (entity, drawn) in drawn {
            if let Some(mut transform) = world.get_mut::<RenderTransform>(entity) {
                transform.set_if_neq(RenderTransform(drawn));
            }
        }
        let feet = self.player.1.position(alpha);
        let target = self.choose_target(alpha);
        if self.follows {
            world.resource_mut::<OrbitCamera>().set_target(follow_target(feet));
        }
        if let (Some(camera), Some(aspect)) = (&mut self.camera, aspect) {
            let profile = profile();
            let radius = profile.radius;
            let frame = CameraFrame {
                dt: std::mem::take(&mut camera.dt),
                behind: crate::cues::behind_latest(alpha),
                player: Subject { position: feet, height: profile.height, radius },
                player_yaw: self.player.1.yaw(alpha),
                target: target.map(|position| Subject {
                    position,
                    height: enemy::CROWN,
                    radius: enemy::FRAMED_RADIUS,
                }),
                look: std::mem::take(&mut camera.look),
                zoom: std::mem::take(&mut camera.zoom),
                aspect,
            };
            let pose = camera.director.update(&frame, &self.ground.world);
            let mut view = world.resource_mut::<OrbitCamera>();
            view.set_view(pose.eye, pose.forward(), pose.fov_y);
            view.set_shake(Vec3::new(0.0, 0.0, roll_of(pose.rotation)), Vec3::ZERO);
        }
    }

    /// The collider view's share of the arena (`combat.view_colliders`, which `--debug-draw colliders`
    /// turns on): the spawn points, and the colliders until a fight's own view draws those
    /// ([`crate::combat::Combat::draw_views`]), so the view works with no fight and draws nothing twice.
    pub(crate) fn draw_colliders(&self, canvas: &mut impl Canvas) {
        self.ground.draw(!self.fighting, canvas);
    }
}

#[cfg(test)]
mod framing;

#[cfg(test)]
mod tests {
    use sapphire_colliders::{ColliderDocument, ColliderEntry, ColliderSet, ColliderShape};
    use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

    use super::*;
    use crate::combat_views::{CombatViews, NoVisiblePose};
    use sapphire_stage::lookdev::{PLANTINGS, Plant};

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

    /// Actors with entities of a world of their own, and that world.
    fn actors_on(ground: ArenaGround, shot: ArenaShot) -> (ArenaActors, World) {
        let mut world = World::new();
        let mut placed = || world.spawn(RenderTransform::default()).id();
        let (player, warrior, blade) = (placed(), placed(), placed());
        let enemy = ArenaEnemy { warrior, blade, blade_in_model: Mat4::IDENTITY, hand: 0 };
        (ArenaActors::new(player, enemy, shot, ground), world)
    }

    /// A fight of the `arena-fight` scenario, on a director, and what it does first.
    fn fight() -> (ArenaActors, World, Simulation, Vec<ActorId>) {
        let scenario = find_scenario(SCENARIOS, FIGHT_SCENARIO).expect("listed");
        let (sim, cast) = scenario.start(scenario.config).expect("it starts");
        let (mut actors, mut world) = actors_on(built(), ArenaShot::Director);
        world.insert_resource(ArenaShot::Director.camera(spawns().player.position()));
        actors.drive_camera(1.0 / 60.0, Vec2::ZERO, 0.0);
        actors.present(&mut world, 1.0, Some(16.0 / 9.0));
        (actors, world, sim, cast)
    }

    /// A step of a fight with these buttons held by the player, and the actors told.
    fn step_fight(actors: &mut ArenaActors, sim: &mut Simulation, cast: &[ActorId], buttons: Buttons) {
        let mut tick = sapphire_gameplay::TickActions::NEUTRAL;
        let held = sapphire_gameplay::ActorActions { buttons, stick: Stick::CENTER };
        tick.set(cast[0], held).expect("room");
        sim.step(&tick);
        actors.follow(sim, cast);
    }

    /// Before a fight a graph plays nothing. In one, a move plays at the move's own ticks, the same number
    /// its tables read, and a frame between steps reads as far behind as its transform draws.
    #[test]
    fn a_fighters_graph_reads_its_move_at_the_moves_own_ticks() {
        let (mut actors, _, mut sim, cast) = fight();
        assert_eq!(actors.graph_input(0, 1.0), None, "nothing before a fight");
        step_fight(&mut actors, &mut sim, &cast, Buttons::LIGHT);
        for _ in 0..4 {
            step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
        }
        let current = sim.actor(cast[0]).expect("alive").moves.current.expect("it swung");
        let whole = duration_ticks(current.elapsed);
        let input = actors.graph_input(0, 1.0).expect("a fight");
        let combat = input.combat.expect("in a move");
        assert_eq!(combat.name, current.name);
        assert_eq!(combat.position, whole);
        let halfway = actors.graph_input(0, 0.5).expect("a fight");
        assert_eq!(halfway.combat.expect("in a move").position, whole - 0.5);
        assert_eq!(halfway.ticks, input.ticks - 0.5);
        assert!(!input.cut);
        assert_eq!(actors.graph_input(1, 1.0).expect("a fight").combat, None, "the warrior hasn't moved");
    }

    /// Hitstop: a frozen clock asks the same thing at any `alpha`, which is what a graph skips on.
    #[test]
    fn a_frozen_fighter_asks_the_same_thing_at_every_alpha() {
        let (mut actors, _, mut sim, cast) = fight();
        let mut frozen = false;
        for _ in 0..240 {
            step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
            if sim.actor(cast[0]).expect("alive").clock_delta == sapphire_time::SimDuration::ZERO {
                frozen = true;
                break;
            }
        }
        assert!(frozen, "the warrior's overhead lands inside 4 s");
        // Whatever the steps had to say, once: then it's the same every frame.
        actors.graph_input(0, 1.0);
        let first = actors.graph_input(0, 1.0);
        assert_eq!(actors.graph_input(0, 0.25), first);
        assert_eq!(actors.graph_input(0, 0.75), first);
    }

    /// A defeated fighter's graph hears so through its fall and after it, which is what keeps it down
    /// (`GraphInput::defeated`), and the next fight is a cut.
    #[test]
    fn a_defeated_fighter_says_so_and_the_next_fight_is_a_cut() {
        let scenario = find_scenario(SCENARIOS, FIGHT_SCENARIO).expect("listed");
        let (mut actors, _, mut sim, cast) = fight();
        // An idle player falls inside a minute.
        let mut steps = 0;
        loop {
            step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
            let defeated = sim.actor(cast[0]).expect("alive").fighter.is_defeated();
            assert_eq!(actors.graph_input(0, 1.0).expect("a fight").defeated, defeated);
            if defeated {
                break;
            }
            steps += 1;
            assert!(steps < 4000, "the player never fell");
        }
        let mut fell = false;
        for _ in 0..240 {
            step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
            let input = actors.graph_input(0, 1.0).expect("a fight");
            assert!(input.defeated, "for good");
            fell |= input.combat.is_some_and(|combat| combat.name == sapphire_core::Name::new("defeat"));
        }
        assert!(fell, "its fall played");
        let over = actors.graph_input(0, 1.0).expect("a fight");
        assert_eq!(over.combat, None, "and it's over");
        assert_eq!(actors.look_target(1, 1.0), None, "there's no one to look at");
        assert!(actors.look_target(0, 1.0).is_some());

        let (mut again, cast) = scenario.start(scenario.config).expect("it starts");
        again.step(&sapphire_gameplay::TickActions::NEUTRAL);
        actors.follow(&again, &cast);
        let next = actors.graph_input(0, 1.0).expect("a fight");
        assert!(next.cut, "a new fight stands alone");
        assert_eq!(next.combat, None);
        assert!(!actors.graph_input(0, 1.0).expect("a fight").cut, "once");
    }

    /// What a landing says lasts until a frame hears it.
    #[test]
    fn a_landing_waits_for_a_frame() {
        let (mut actors, _, mut sim, cast) = fight();
        let mut landed = false;
        for tick in 0..240 {
            let buttons = if tick == 0 { Buttons::JUMP } else { Buttons::NONE };
            step_fight(&mut actors, &mut sim, &cast, buttons);
            landed |= sim.actor(cast[0]).expect("alive").controller.state.events.landed;
            if landed {
                break;
            }
        }
        assert!(landed, "it came down");
        step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
        assert!(actors.graph_input(0, 1.0).expect("a fight").locomotion.landed);
        assert!(!actors.graph_input(0, 1.0).expect("a fight").locomotion.landed, "once");
    }

    /// Whatever a move set can start, its fighter's graph has a row for: a move with none would play
    /// locomotion, so a knocked-down fighter would stand there.
    #[test]
    fn every_move_a_fighter_has_is_in_its_graph() {
        let read = |text: &str| sapphire_assets::graph::AnimGraph::from_document(text).expect("it reads");
        let warrior = read(include_str!("../../../content/combat/enemy/warrior.animgraph.ron"));
        let character = read(include_str!("../../../content/lookdev/character/character.animgraph.ron"));
        for (graph, set) in [
            (&warrior, sapphire_gameplay::replay::warrior_moves().expect("it compiles")),
            (&character, sapphire_gameplay::replay::player_moves().expect("it reads")),
            (&character, sapphire_gameplay::replay::test_moves().expect("it compiles")),
        ] {
            for mv in set.moves() {
                let label = mv.label();
                assert!(graph.combat.moves.iter().any(|row| row.name == label), "no clip plays {label}");
            }
        }
    }

    /// The bars are the simulation's, in shares, with only the player's in digits.
    #[test]
    fn the_hud_reads_the_fighters() {
        let (mut actors, _, mut sim, cast) = fight();
        assert_eq!(actors.hud_values(), None, "nothing before a fight");
        step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
        let values = actors.hud_values().expect("a fight");
        assert_eq!(values.player.hp, 1.0);
        assert_eq!(values.player.hp_points, Some(300));
        let enemy = values.enemy.expect("there");
        assert_eq!((enemy.hp, enemy.poise, enemy.hp_points), (1.0, 1.0, None));
        for _ in 0..240 {
            step_fight(&mut actors, &mut sim, &cast, Buttons::NONE);
        }
        let hurt = actors.hud_values().expect("a fight");
        let player = sim.actor(cast[0]).expect("alive").fighter;
        assert!(player.health() < 300, "the overhead landed");
        assert_eq!(hurt.player.hp, player.health() as f32 / 300.0);
        assert_eq!(hurt.player.hp_points, Some(player.health()));
        assert_eq!(hurt.player.poise, player.poise() as f32 / player.max_poise() as f32);
    }

    /// A view's roll, read back: nothing for an upright one, and either way for one that's turned.
    #[test]
    fn a_shakes_roll_is_read_about_the_views_backward() {
        let upright = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.3);
        assert!(roll_of(upright).abs() < 1e-5, "{}", roll_of(upright));
        for roll in [0.04, -0.04] {
            // The view's backward is -forward, and +Z is where a pose looks.
            let turned = upright * Quat::from_rotation_z(-roll);
            assert!((roll_of(turned) - roll).abs() < 1e-5, "{} against {roll}", roll_of(turned));
        }
    }

    /// The soft lock's choice is what the camera frames: the enemy from the first frame, and nobody once
    /// it's fallen or the player's run out of range, and back again on the way in.
    #[test]
    fn the_camera_frames_who_the_soft_lock_chooses() {
        let (mut actors, _, _, cast) = fight();
        assert_eq!(actors.target, Some(cast[1]));
        let mut world = World::new();
        world.insert_resource(ArenaShot::Director.camera(spawns().player.position()));

        // Out of range: the keep range is 16 m, and coming back in it takes 12 m to be chosen again.
        let enemy = spawns().enemy.position();
        let away = |metres: f32| enemy + Vec3::new(metres, 0.0, 0.0);
        let mut stand_at = |actors: &mut ArenaActors, position| {
            actors.player.1 = Root::standing(position, 0.0);
            actors.present(&mut world, 1.0, None);
            actors.target
        };
        assert_eq!(stand_at(&mut actors, away(15.0)), Some(cast[1]), "kept out to its keep range");
        assert_eq!(stand_at(&mut actors, away(17.0)), None);
        assert_eq!(stand_at(&mut actors, away(15.0)), None, "not chosen again yet");
        assert_eq!(stand_at(&mut actors, away(11.0)), Some(cast[1]));
    }

    /// A thumb doesn't aim, so a fight played on touch chooses by `TOUCH`, whose reach is longer, even
    /// with one enemy to choose from. A pad's or a key's press goes back to the soft lock's.
    #[test]
    fn a_thumb_chooses_with_touchs_weights_and_anything_else_with_the_soft_locks() {
        let (mut actors, _, _, cast) = fight();
        let mut world = World::new();
        world.insert_resource(ArenaShot::Director.camera(spawns().player.position()));
        let enemy = spawns().enemy.position();
        let away = |metres: f32| enemy + Vec3::new(metres, 0.0, 0.0);
        let mut stand_at = |actors: &mut ArenaActors, metres| {
            actors.player.1 = Root::standing(away(metres), 0.0);
            actors.present(&mut world, 1.0, None);
            actors.target
        };

        assert_eq!(stand_at(&mut actors, 17.0), None, "past the soft lock's keep range");
        assert_eq!(stand_at(&mut actors, 13.0), None, "and not within its reach");

        actors.set_played_on_touch(true);
        assert_eq!(stand_at(&mut actors, 13.0), Some(cast[1]), "within a thumb's reach");
        assert_eq!(stand_at(&mut actors, 17.0), Some(cast[1]), "and kept further");
        assert_eq!(stand_at(&mut actors, 19.0), None, "past a thumb's keep range");

        actors.set_played_on_touch(false);
        assert_eq!(stand_at(&mut actors, 13.0), None, "a pad's reach again");
    }

    /// Nobody stands to frame once the enemy's fallen, and a new fight stands it up again.
    #[test]
    fn a_fallen_enemy_stops_being_framed() {
        let (mut actors, mut world, mut sim, cast) = fight();
        sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
        actors.follow(&sim, &cast);
        actors.present(&mut world, 1.0, None);
        assert_eq!(actors.target, Some(cast[1]));

        let mut fallen = actors.seen[1].expect("it's there");
        assert!(!fallen.fighter.is_defeated());
        fallen.fighter = fallen_fighter(&mut sim, &cast);
        actors.seen[1] = Some(fallen);
        actors.present(&mut world, 1.0, None);
        assert_eq!(actors.target, None);
    }

    /// The warrior, felled through real hits: the player's fists on the fight's first thousands of
    /// ticks, from a dash attack's reach.
    fn fallen_fighter(sim: &mut Simulation, cast: &[ActorId]) -> sapphire_gameplay::Fighter {
        let mut source = sapphire_gameplay::replay::fight_back;
        for _ in 0..4000 {
            let actions = sapphire_gameplay::replay::ActionSource::actions(&mut source, sim.tick(), cast);
            sim.step(&actions);
            let enemy = sim.actor(cast[1]).expect("alive");
            if enemy.fighter.is_defeated() {
                return enemy.fighter;
            }
        }
        panic!("the script doesn't beat it any more");
    }

    /// A simulation whose tick went back is a new fight: everyone stands where it says, with nothing to
    /// draw between, and the camera cuts. A replay past its end, whose tick stays, isn't.
    #[test]
    fn a_new_fight_stands_where_it_stands() {
        let (mut actors, mut world, mut sim, cast) = fight();
        let mut source = sapphire_gameplay::replay::fight_back;
        for _ in 0..200 {
            let actions = sapphire_gameplay::replay::ActionSource::actions(&mut source, sim.tick(), &cast);
            sim.step(&actions);
            actors.follow(&sim, &cast);
        }
        let moved = actors.player.1.position(1.0);
        assert!(moved.distance(spawns().player.position()) > 0.5, "it fought: {moved}");
        actors.follow(&sim, &cast);
        assert_eq!(actors.player.1.previous_position, moved, "the same tick again is no new fight");

        let scenario = find_scenario(SCENARIOS, FIGHT_SCENARIO).expect("listed");
        let (fresh, cast) = scenario.start(scenario.config).expect("it starts");
        actors.follow(&fresh, &cast);
        let start = spawns().player.position();
        assert_eq!(actors.player.1.position(0.0), start, "it doesn't slide back across the square");
        assert_eq!(actors.player.1.position(1.0), start);
        assert_eq!(actors.enemy.1.position(0.0), spawns().enemy.position());

        // The cut lands the view behind the player, at once, however far off it was.
        actors.drive_camera(1.0 / 60.0, Vec2::ZERO, 0.0);
        actors.present(&mut world, 1.0, Some(16.0 / 9.0));
        let eye = world.resource::<OrbitCamera>().eye();
        assert!(eye.distance(start) < 8.0, "{eye} from {start}");
    }

    /// The player's lock-on button turns the director's rig on, on the press, and off on the next; a
    /// held button stays where it is, and a new fight keeps it.
    #[test]
    fn the_lock_on_button_turns_the_directors_rig() {
        use sapphire_gameplay::{ActorActions, TickActions};
        let (mut actors, mut world, mut sim, cast) = fight();
        let mut step = |actors: &mut ArenaActors, buttons: Buttons| {
            let mut actions = TickActions::NEUTRAL;
            actions.set(cast[0], ActorActions { buttons, ..ActorActions::NEUTRAL }).expect("room");
            sim.step(&actions);
            actors.follow(&sim, &cast);
        };
        let wanted = |actors: &mut ArenaActors, world: &mut World| {
            for _ in 0..240 {
                actors.drive_camera(1.0 / 60.0, Vec2::ZERO, 0.0);
                actors.present(world, 1.0, Some(16.0 / 9.0));
            }
            actors.camera.as_ref().expect("a director").director.weight(Rig::LockOn)
        };
        step(&mut actors, Buttons::NONE);
        assert_eq!(wanted(&mut actors, &mut world), 0.0);
        step(&mut actors, Buttons::LOCK_ON);
        step(&mut actors, Buttons::LOCK_ON);
        assert!(actors.locked_on, "held, it's one press");
        assert!(wanted(&mut actors, &mut world) > 0.99);
        step(&mut actors, Buttons::NONE);
        assert!(actors.locked_on, "it isn't held on");
        step(&mut actors, Buttons::LOCK_ON);
        assert!(!actors.locked_on);
        assert!(wanted(&mut actors, &mut world) < 0.01);
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
        let mut actors = actors_on(ground, ArenaShot::Follow).0;

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
