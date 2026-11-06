//! A fight on a scene: what carries a simulation to the frame.
//!
//! [`FightShow`] holds what a fight has on the stage: the arena's actors, the enemy and the blades, who the
//! party has on the field, the cues firing and the world clock's share of a fight's slow motion. All of it is
//! ordinary entities in the scene's world, placed from what the simulation says of its actors, so the scene
//! itself knows nothing of a fight (`sapphire_stage::SceneShow`).
//!
//! [`Fight`] is that beside the simulation it follows, as a stage runs them: one step of each, in spec §4.5's
//! order.

use std::sync::Arc;
use std::time::Duration;

use sapphire_anim::{AnimStats, Skeleton};
use sapphire_assets::mesh::Mesh;
use sapphire_assets::{AssetId, Handle};
#[cfg(feature = "dev")]
use sapphire_camera::CameraTuning;
use sapphire_colliders::ColliderSet;
use sapphire_cues::RumbleSink;
use sapphire_ecs::bevy_ecs::world::WorldId;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::Simulation;
use sapphire_math::{Mat4, Vec2, Vec3};
use sapphire_physics::PhysicsWorld;
use sapphire_platform::AppInitError;
use sapphire_render::quality::TierId;
use sapphire_render::{MeshHandle, PipelineInventory, PostParams, Renderer, VfxFrame};
use sapphire_rhi::GpuContext;
use sapphire_sequencer::{Origin, Playing, TimelinePlayer};
use sapphire_time::{ScaleHandle, ScaleSource, Stopwatch, TICK_HZ};
use sapphire_timeline::Timeline;
use sapphire_vfx::VfxBudget;
use sapphire_world::light_rig::{CharacterLightRig, LightRigId};
#[cfg(feature = "dev")]
use sapphire_world::screen_effects::EffectLibrary;

use crate::LOG_TARGET;
use crate::arena::{ArenaActors, ArenaEnemy, ArenaGround, ArenaShot, COLLIDERS};
use crate::blade::{self, HeldBlade};
use crate::combat::{Combat, CombatOption, no_bindings};
use crate::combat_views::{CombatViews, Facade};
use crate::cues::{CueShow, CueStage};
use crate::enemy::{self, EnemyMeshes};
use sapphire_stage::anim_graph::{ClipTable, GraphDriver, GraphEvaluation};
use sapphire_stage::animate::{self, ClipPlayer};
use sapphire_stage::audio::CueSounds;
use sapphire_stage::content::{Content, GraphKey};
use sapphire_stage::{FrameLaps, StageDesc, lookdev, vfx};
use sapphire_stage::{RenderMesh, RenderPose, RenderTransform};

use crate::party_looks::{Drawn, PartyLooks};
use crate::timeline::TimelineShow;
use sapphire_stage::scene::{DEFAULT_TIME_OF_DAY, Figure, LookDev, Scene, stream_settings};
use sapphire_stage::turntable::Turntable;
use sapphire_stage::{ContentChanges, SceneShow};

/// Whose graph is the enemy's.
pub const ENEMY_GRAPH: GraphKey = GraphKey("enemy");

/// What the arena (`--arena`) stands on and how it's framed, around what the look-dev stage draws,
/// and the enemy who fights there.
pub struct Arena {
    pub enemy: EnemyMeshes,
    pub held: enemy::Held,
    pub shot: ArenaShot,
    pub look: LookDev,
    pub ground: ArenaGround,
}

/// The characters playing a graph, with everything that follows their skeletons.
type GraphPlayers = QueryState<(
    &'static mut GraphDriver,
    &'static mut RenderPose,
    &'static mut Figure,
    &'static mut CharacterLightRig,
    &'static mut RenderTransform,
)>;

/// What loading has given a fight, which stays loaded for as long as this does.
#[derive(Default)]
struct FightAssets {
    /// The enemy's model and its blade (`--look-dev enemy`, `--arena`).
    enemy: Option<[Handle<Mesh>; 2]>,
    /// The enemy's skeleton, with those.
    enemy_skeleton: Option<Handle<Skeleton>>,
    /// The versions of the enemy's mesh and skeleton last seen ([`FightShow::take_enemy_changed`]), once
    /// there's an enemy.
    enemy_versions: Option<(u32, u32)>,
    /// The player's sword (`--blade on`).
    sword: Option<Handle<Mesh>>,
    /// The arena's colliders (`--arena`'s).
    colliders: Option<Handle<ColliderSet>>,
}

/// What a step of a fight did that an interface drawn over it has to hear.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Followed {
    /// Someone else came in for each of the two.
    pub came_in: [bool; 2],
    /// A new fight began, and everything stands somewhere else.
    pub started_over: bool,
}

/// What a fight has on the scene. Empty, it plays nothing and costs a few branches a frame.
#[derive(Default)]
pub struct FightShow {
    assets: FightAssets,
    /// The enemy and its blade, with `--look-dev enemy` or `--arena`.
    enemy: Option<(Entity, Entity)>,
    /// The character's blade, with `--blade on`.
    blade: Option<HeldBlade>,
    /// The arena's actors, with `--arena`.
    arena: Option<Box<ArenaActors>>,
    /// A fight's slow motion on the scene's world clock, while it runs: how slow, and its scale there.
    slow_motion: Option<(u32, ScaleHandle)>,
    /// Whether the arena's fighters play graphs ([`Self::play_fighters`]).
    fighters_posed: bool,
    /// A new version of the enemy landed that its other parts don't fit yet ([`Self::replace_enemy`]).
    enemy_waits: bool,
    /// Which party member the character draws as, with `--party`.
    party_looks: Option<Drawn>,
    /// A fight's cues, fired (`--cues`, M2 E3).
    cues: Option<CueShow>,
    /// The timeline playing (M3 Q1), which a fight with none spends nothing on.
    timeline: Option<Box<TimelineShow>>,
    /// Fighters draw on the latest step, not between it and the one before: a timeline has held the
    /// simulation, and the next step hasn't come. Past that, a frame can't draw earlier than this one did.
    draws_latest: bool,
    /// Made once: a query made each frame would allocate.
    graph_players: Option<GraphPlayers>,
    /// Last world, retained-storage run and simulation tick followed.
    followed_run: Option<(WorldId, u64, u64)>,
    /// How long the last frame's camera took: the arena's director or plain follow.
    camera_time: Duration,
}

impl FightShow {
    /// Loads until the enemy and its blade are on `renderer`, with their materials and the maps
    /// they name, and the warrior's skeleton is resident, and returns their meshes: the load screen
    /// of `--look-dev enemy` and `--arena`. They stay loaded from then on.
    pub fn load_enemy(
        &mut self,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<EnemyMeshes, AppInitError> {
        let failed = |error: AppInitError| AppInitError::msg(format!("the enemy didn't load: {error}"));
        content.keep_mesh_copy(enemy::WARRIOR);
        let warrior = content.hold(enemy::WARRIOR, renderer, gpu).map_err(failed)?;
        let blade = content.hold(enemy::BLADE, renderer, gpu).map_err(failed)?;
        let skeleton: Option<AssetId> =
            content.mesh_copy(enemy::WARRIOR).and_then(|mesh| mesh.skin.as_ref()?.skeleton);
        let skeleton = skeleton.map(|id| content.hold(id, renderer, gpu)).transpose().map_err(failed)?;
        self.assets.enemy = Some([warrior, blade]);
        self.assets.enemy_skeleton = skeleton;
        self.assets.enemy_versions = self.enemy_versions(content);
        let mesh = |id| content.mesh_on_renderer(id).expect("a ready mesh was uploaded here");
        Ok(EnemyMeshes { warrior: mesh(enemy::WARRIOR), blade: mesh(enemy::BLADE) })
    }

    /// Whether the enemy's mesh or skeleton has a new version since the last call or since
    /// [`Self::load_enemy`], whichever way it came; they may land frames apart. Always false with no enemy,
    /// and it allocates nothing.
    fn take_enemy_changed(&mut self, content: &Content) -> bool {
        let versions = self.enemy_versions(content);
        std::mem::replace(&mut self.assets.enemy_versions, versions) != versions
    }

    /// The versions the enemy's mesh and skeleton are at, if there's an enemy.
    fn enemy_versions(&self, content: &Content) -> Option<(u32, u32)> {
        let [warrior, _] = self.assets.enemy.as_ref()?;
        let skeleton = self.assets.enemy_skeleton.as_ref();
        let skeleton = skeleton.map_or(0, |skeleton| content.version_of(skeleton));
        Some((content.version_of(warrior), skeleton))
    }

    /// The enemy's held pose, from what [`Self::load_enemy`] loaded.
    pub fn held_enemy(&self, content: &Content) -> Result<enemy::Held, AppInitError> {
        let skin = content.mesh_copy(enemy::WARRIOR).and_then(|mesh| mesh.skin.as_ref());
        let skeleton = self.assets.enemy_skeleton.as_ref().and_then(|skeleton| content.read(skeleton));
        let (Some(skin), Some(skeleton)) = (skin, skeleton) else {
            return Err(AppInitError::msg("the enemy isn't loaded"));
        };
        enemy::held(skin, skeleton).map_err(AppInitError::msg)
    }

    /// Loads the enemy's graph as `Content::load_graph` does the character's.
    pub fn load_enemy_graph(
        &mut self,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), AppInitError> {
        let skeleton = self.assets.enemy_skeleton.clone();
        content.load_graph_of(ENEMY_GRAPH, skeleton, enemy::GRAPH, renderer, gpu)
    }

    /// A graph driver for the enemy, from what [`Self::load_enemy`] and [`Self::load_enemy_graph`] loaded.
    pub fn enemy_driver(
        &self,
        content: &Content,
        ground: Arc<PhysicsWorld>,
    ) -> Result<GraphDriver, AppInitError> {
        let binding = self.enemy_binding(content)?;
        let skin = content.mesh_copy(enemy::WARRIOR).and_then(|mesh| mesh.skin.as_ref());
        let skeleton = self.assets.enemy_skeleton.as_ref().and_then(|skeleton| content.read(skeleton));
        let (Some(skin), Some(skeleton)) = (skin, skeleton) else {
            return Err(AppInitError::msg("the enemy isn't loaded"));
        };
        enemy::driver(skin, skeleton, Arc::new(binding), ground).map_err(AppInitError::msg)
    }

    /// The enemy's graph as it stands, bound to the enemy: the one live tuning took, or the cooked one.
    fn enemy_binding(&self, content: &Content) -> Result<sapphire_anim::GraphBinding, AppInitError> {
        let graph = content.graph_document_of(ENEMY_GRAPH).map_err(AppInitError::msg)?;
        self.enemy_binding_of(content, &graph).map_err(AppInitError::msg)
    }

    /// [`Self::enemy_binding`], for `graph` rather than the one it holds.
    fn enemy_binding_of(
        &self,
        content: &Content,
        graph: &sapphire_anim::AnimGraph,
    ) -> Result<sapphire_anim::GraphBinding, String> {
        let skeleton = self.assets.enemy_skeleton.as_ref().and_then(|skeleton| content.read(skeleton));
        let skeleton = skeleton.ok_or("the enemy's skeleton isn't loaded")?;
        sapphire_anim::GraphBinding::new(graph, skeleton, None, 0, &content.graph_clips_of(ENEMY_GRAPH))
            .map_err(|error| error.to_string())
    }

    /// Loads until the player's sword is on `renderer`, with its material and the maps it names, and
    /// returns its mesh: the load screen of `--blade on`. It stays loaded from then on.
    pub fn load_sword(
        &mut self,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<MeshHandle, AppInitError> {
        let sword = content
            .hold(blade::SWORD, renderer, gpu)
            .map_err(|error| AppInitError::msg(format!("the sword didn't load: {error}")))?;
        self.assets.sword = Some(sword);
        Ok(content.mesh_on_renderer(blade::SWORD).expect("a ready mesh was uploaded here"))
    }

    /// Loads the arena's colliders until they're resident, and builds what the stage stands on from
    /// them: CPU data, which nothing uploads. Fails if the cook's colliders leave a spawn point in
    /// the air or in a wall ([`ArenaGround::new`]).
    pub fn load_arena_ground(
        &mut self,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<ArenaGround, AppInitError> {
        let handle: Handle<ColliderSet> = content
            .hold(COLLIDERS, renderer, gpu)
            .map_err(|error| AppInitError::msg(format!("the arena's colliders didn't load: {error}")))?;
        let set = content
            .read(&handle)
            .ok_or_else(|| AppInitError::msg("the arena's colliders aren't resident"))?;
        let ground = ArenaGround::new(set).map_err(AppInitError::msg)?;
        self.assets.colliders = Some(handle);
        Ok(ground)
    }

    /// Builds the arena (`--arena`, [`crate::arena`]): the look-dev stage's town, day and light rigs
    /// where the town stands, with the character at the player's spawn point and the enemy
    /// ([`crate::enemy`]) and its blade at the enemy's. Unlike the look-dev stage, the town stays still
    /// and they move, so none of them has a turntable: [`Self::follow_simulation`] carries each actor's
    /// root here.
    pub fn spawn_arena(&mut self, scene: &mut Scene, arena: Arena) {
        let Arena { enemy, held, shot, look, ground } = arena;
        scene.world_mut().insert_resource(shot.camera(sapphire_gameplay::arena::spawns().player.position()));
        scene.spawn_town(Vec3::ZERO, look);
        let character = scene.character();
        scene.world_mut().entity_mut(character).remove::<Turntable>();
        let (blade_in_model, hand) = (held.blade, held.hand);
        self.spawn_enemy(scene, enemy, held);
        let (warrior, blade) = self.enemy.expect("it just stood there");
        let enemy = ArenaEnemy { warrior, blade, blade_in_model, hand };
        self.arena = Some(Box::new(ArenaActors::new(character, enemy, shot, ground)));
    }

    /// Poses the arena's fighters by these graphs from now on, from what the simulation says of them
    /// ([`crate::arena`]). They hold what they have until a fight steps. Nothing without `--arena`.
    pub fn play_fighters(&mut self, scene: &mut Scene, player: GraphDriver, enemy: GraphDriver) {
        let Some(arena) = &self.arena else { return };
        let [(_, _), (enemy_entity, _)] = arena.fighters();
        scene.pose_character_by_graph(player);
        let world = scene.world_mut();
        world.entity_mut(enemy_entity).insert(enemy);
        self.graph_players = Some(world.query());
        self.fighters_posed = true;
    }

    /// Takes the arena's actors from `simulation`, after a step of it: nothing without `--arena`.
    /// `cast` is its scenario's, the player first. Whatever the stage, the scene's world clock takes up
    /// the fight's slow motion, as the simulation's does from its next step.
    pub fn follow_simulation(
        &mut self,
        scene: &mut Scene,
        simulation: &Simulation,
        cast: &[sapphire_gameplay::ActorId],
    ) -> Followed {
        let run = (simulation.world().id(), simulation.run_generation(), simulation.tick());
        let started_over = self
            .followed_run
            .is_some_and(|(world, generation, tick)| world != run.0 || generation != run.1 || run.2 < tick);
        self.followed_run = Some(run);
        self.follow_slow_motion(scene, simulation);
        if let Some(cues) = &mut self.cues {
            cues.prepare(simulation);
        }
        self.draws_latest = self.holds_simulation();
        if let Some(show) = &mut self.timeline {
            show.follow(simulation);
        }
        let mut followed = Followed { started_over, ..Followed::default() };
        if let Some(arena) = &mut self.arena {
            followed.came_in = arena.follow(simulation, cast);
        }
        if started_over {
            scene.cut_view();
            if let Some(cues) = &mut self.cues {
                scene.with_screen_effects(|stack, _| cues.start_over(stack));
            }
        }
        if let Some(mesh) = self.party_looks.as_mut().and_then(|drawn| drawn.follow(simulation.party())) {
            scene.set_character_mesh(mesh);
        }
        followed
    }

    /// Keeps the scene's world clock with a fight's slow motion (M2 G4) and a timeline's time scale (M3 Q1),
    /// the scales a simulation puts on its world, so what the scene's clock times (screen effects, time of
    /// day, the wind and everything else on `FramePacket::time_seconds`) stays with the fighters and the
    /// effects on theirs. It's one scale here, whatever it's made of.
    fn follow_slow_motion(&mut self, scene: &mut Scene, simulation: &Simulation) {
        let clock = &simulation.world().resource::<WorldClock>().0;
        let slowed = clock
            .scales()
            .filter(|scale| matches!(scale.source, ScaleSource::PerfectDodge | ScaleSource::Cinematic))
            .map(|scale| (scale.source, scale.per_mille))
            .reduce(|(source, whole), (_, part)| (source, whole * part / 1000));
        if slowed.map(|(_, per_mille)| per_mille) == self.slow_motion.map(|(per_mille, _)| per_mille) {
            return;
        }
        let mut clock = scene.world_clock_mut();
        if let Some((_, handle)) = self.slow_motion.take() {
            clock.0.pop_scale(handle);
        }
        if let Some((source, per_mille)) = slowed {
            match clock.0.push_scale(source, per_mille) {
                Ok(handle) => self.slow_motion = Some((per_mille, handle)),
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "the scene misses a fight's slow motion")
                }
            }
        }
    }

    /// Gives the character each party member's mesh (`--party`): at startup, and after GPU device loss or
    /// a tier change, when whoever's on the field stays there.
    pub fn set_party_meshes(
        &mut self,
        scene: &mut Scene,
        meshes: [MeshHandle; sapphire_gameplay::PARTY_SLOTS],
    ) {
        let drawn =
            self.party_looks.map_or_else(|| Drawn::new(meshes), |drawn| drawn.on_new_renderer(meshes));
        self.party_looks = Some(drawn);
        scene.set_character_mesh(drawn.mesh());
    }

    /// Each party member's mesh, given the character's, with `--party` and once the character has loaded.
    pub fn party_meshes(
        content: &Content,
        character: MeshHandle,
    ) -> Option<[MeshHandle; sapphire_gameplay::PARTY_SLOTS]> {
        content.upload_watcher::<PartyLooks>()?.meshes(character)
    }

    /// Gives the arena's camera director this much real time and input for its next frame, if one's
    /// on, and says whether it took them.
    pub fn drive_camera(&mut self, dt: f32, look: Vec2, zoom: f32) -> bool {
        if let Some(show) = &mut self.timeline {
            show.drive(dt);
        }
        self.arena.as_mut().is_some_and(|arena| arena.drive_camera(dt, look, zoom))
    }

    /// Plays a timeline's presentation from the next frame (M3 Q1), in place of any it was playing: its
    /// shots through the director, if this stage has one, its post on each frame, its cues through the
    /// fight's table. The simulation's half is the simulation's: see [`Simulation::play_timeline`].
    pub fn play_timeline(&mut self, timeline: Arc<Timeline>, origin: Origin) {
        let show = Box::new(TimelineShow::new(timeline, origin));
        self.draws_latest = show.holds_simulation();
        self.timeline = Some(show);
    }

    /// Lets go of the timeline playing, and says how far it had got, in milliseconds, for a replay to record.
    /// The camera blends back from its next frame.
    pub fn skip_timeline(&mut self) -> Option<u32> {
        self.timeline.as_mut().map(|show| show.player_mut().skip())
    }

    /// The timeline playing, for what it says at the playhead: its subtitle above all, which Q2 and U7 draw.
    #[must_use]
    pub fn timeline_player(&self) -> Option<&TimelinePlayer> {
        self.timeline.as_deref().map(TimelineShow::player)
    }

    /// Whether a timeline holds the simulation, so that a loop of its own runs the fight no steps: a
    /// cutscene, until it ends or is skipped. The scene's own step goes on, and the wind and clouds with it.
    #[must_use]
    pub fn holds_simulation(&self) -> bool {
        self.timeline.as_deref().is_some_and(TimelineShow::holds_simulation)
    }

    /// Moves the timeline to this frame and gives the director its shot, ahead of the director's own frame.
    /// A timeline over is gone from here: every post track ended on nothing, and the director's blend out
    /// goes on by itself.
    fn present_timeline(&mut self, scene: &mut Scene, alpha: f32) {
        let Some(show) = &mut self.timeline else { return };
        let shown = show.present(alpha, self.arena.as_mut().and_then(|arena| arena.director_mut()));
        if shown.cut {
            scene.cut_view();
        }
        if let Some(cues) = &mut self.cues {
            let director = self.arena.as_mut().and_then(|arena| arena.director_mut());
            let fired = show.player().fired();
            scene.with_screen_effects(|stack, presets| cues.fire_placed(fired, stack, presets, director));
            if show.holds_simulation() {
                cues.advance_held(shown.dt);
            }
        }
        if show.player().state() != Playing::Yes {
            self.timeline = None;
        }
    }

    /// Where a frame at `alpha` draws the fighters: see [`Self::draws_latest`].
    fn alpha_of_fighters(&self, alpha: f32) -> f32 {
        if self.draws_latest { 1.0 } else { alpha }
    }

    /// Where the view looks across the ground, as an actor's yaw: what the next tick's stick turns by.
    pub fn camera_heading(&self, scene: &Scene) -> f32 {
        self.arena.as_deref().and_then(ArenaActors::heading).unwrap_or_else(|| scene.camera().heading())
    }

    /// Says whether a thumb is what's playing, for who a fight's soft lock chooses
    /// ([`crate::arena::ArenaActors::set_played_on_touch`]).
    pub fn set_played_on_touch(&mut self, touch: bool) {
        if let Some(arena) = &mut self.arena {
            arena.set_played_on_touch(touch);
        }
    }

    /// Stands the enemy on the look-dev stage (`--look-dev enemy`, [`crate::enemy`]): the warrior
    /// held in its pose beside the character, and its blade at its hand. Neither moves, so they are
    /// extracted once. Call after `Scene::spawn_look_dev`, whose light rigs they take.
    pub fn spawn_enemy(&mut self, scene: &mut Scene, meshes: EnemyMeshes, held: enemy::Held) {
        scene.extract_poses();
        let place = enemy::place();
        let rig = LightRigId::DEFAULT;
        let blade_figure = enemy::blade_figure(&held);
        let world = scene.world_mut();
        let warrior = world
            .spawn((
                RenderMesh(meshes.warrior),
                RenderTransform(place),
                CharacterLightRig { rig, head: held.figure.head },
                held.figure,
                held.pose,
            ))
            .id();
        let blade = world
            .spawn((
                RenderMesh(meshes.blade),
                RenderTransform(place * held.blade),
                CharacterLightRig { rig, head: blade_figure.head },
                blade_figure,
            ))
            .id();
        self.enemy = Some((warrior, blade));
    }

    /// Sets the meshes the enemy draws again: after GPU device loss.
    pub fn set_enemy_meshes(&mut self, scene: &mut Scene, meshes: EnemyMeshes) {
        if let Some((warrior, blade)) = self.enemy {
            scene.world_mut().entity_mut(warrior).insert(RenderMesh(meshes.warrior));
            scene.world_mut().entity_mut(blade).insert(RenderMesh(meshes.blade));
        }
    }

    /// Puts the player's blade in the character's right hand (`--blade on`, M2 O3): `held_hand` is where
    /// the character's held pose has that hand, for when no clip plays. It follows the hand each frame
    /// ([`Self::follow_hand`]).
    pub fn spawn_blade(&mut self, scene: &mut Scene, mesh: MeshHandle, held_hand: Mat4) {
        let rig = LightRigId::DEFAULT;
        let figure = scene.figure().of_item_at(held_hand * blade::socket());
        let entity = scene
            .world_mut()
            .spawn((
                RenderMesh(mesh),
                RenderTransform::default(),
                CharacterLightRig { rig, head: figure.head },
                figure,
            ))
            .id();
        self.blade = Some(HeldBlade { entity, held_hand });
    }

    /// Sets the mesh the blade draws again: after GPU device loss.
    pub fn set_blade_mesh(&mut self, scene: &mut Scene, mesh: MeshHandle) {
        if let Some(held) = self.blade {
            scene.world_mut().entity_mut(held.entity).insert(RenderMesh(mesh));
        }
    }

    /// Moves the blade to where the character's hand is this frame: after whatever poses and places
    /// the character, and before extraction.
    fn follow_hand(&mut self, scene: &mut Scene) {
        let Some(held) = self.blade else { return };
        let (character, scene_figure) = (scene.character(), scene.figure());
        let world = scene.world_mut();
        let character = world.entity(character);
        let place = character.get::<RenderTransform>().map_or(Mat4::IDENTITY, |transform| transform.0);
        let hand = character
            .get::<ClipPlayer>()
            .and_then(ClipPlayer::right_hand)
            .or_else(|| character.get::<GraphDriver>().and_then(GraphDriver::right_hand))
            .unwrap_or(held.held_hand);
        let holder = character.get::<Figure>().copied().unwrap_or(scene_figure);
        let item = hand * blade::socket();
        let figure = holder.of_item_at(item);
        let mut blade = world.entity_mut(held.entity);
        if let Some(mut transform) = blade.get_mut::<RenderTransform>() {
            transform.set_if_neq(RenderTransform(place * item));
        }
        if let Some(mut own) = blade.get_mut::<Figure>() {
            own.set_if_neq(figure);
        }
        if let Some(mut rig) = blade.get_mut::<CharacterLightRig>() {
            rig.head = figure.head;
        }
    }

    /// Tells a fight's sword trails where this frame draws the blades, once they're placed: what a trail
    /// reads is what's drawn.
    fn follow_blades(&mut self, scene: &mut Scene, alpha: f32) {
        let Some(cues) = self.cues.as_mut().filter(|cues| cues.trails_are_running()) else { return };
        let world = scene.world();
        let drawn = |entity| world.get::<RenderTransform>(entity).map(|transform| transform.0);
        let player = self.blade.and_then(|held| drawn(held.entity));
        let enemy = self.arena.as_ref().and_then(|arena| drawn(arena.enemy_blade()));
        cues.follow_blades([player, enemy], alpha);
    }

    /// Fires a fight's cues from now on (`--cues`, M2 E3), within `budget`, and has the scene prewarm
    /// what they draw through. An error names what its table can't find.
    pub fn play_cues(&mut self, scene: &mut Scene, budget: VfxBudget) -> Result<(), String> {
        let (show, variants) = scene.with_screen_effects(|_, presets| {
            let show = CueShow::new(budget, presets).map_err(|error| error.to_string())?;
            let variants = show.post_variants(presets);
            Ok::<_, String>((show, variants))
        })?;
        scene.will_push_effects(variants);
        self.cues = Some(show);
        Ok(())
    }

    /// What's firing a fight's cues, if anything is.
    pub fn cue_show(&self) -> Option<&CueShow> {
        self.cues.as_ref()
    }

    /// Fires what `sim`'s latest step published (`--cues`), and does nothing without that flag. Call it
    /// after each of the simulation's steps: the next one clears them. Allocates nothing once warmed.
    pub fn show_cues(&mut self, scene: &mut Scene, sim: &Simulation) {
        let Some(cues) = &mut self.cues else { return };
        let camera = scene.camera().camera_data(scene.view_aspect());
        let view_projection = camera.proj * camera.view;
        let place = scene.world().get::<RenderTransform>(scene.character());
        let stand = place.map_or(Vec3::ZERO, |place| place.0.w_axis.truncate());
        let (posed, sword) = (self.fighters_posed, self.blade.is_some());
        let stage = match &mut self.arena {
            Some(arena) => {
                let (director, [player, enemy]) = arena.cue_stage();
                let fighters = [player.filter(|_| posed && sword), enemy.filter(|_| posed)];
                CueStage::Actors { director, fighters }
            }
            None => CueStage::OneCharacter { stand },
        };
        scene.with_screen_effects(|stack, presets| cues.step(sim, stack, presets, view_projection, stage));
    }

    /// What a fight's cues ask of the audio system this frame.
    pub fn cue_sounds(&self) -> CueSounds<'_> {
        self.cues.as_ref().map_or(CueSounds::NONE, CueShow::sounds)
    }

    /// Ends a frame's steps of a fight's cues, `dt` real seconds after the last frame's, for a frame drawn
    /// at `alpha`, after the audio system took [`Self::cue_sounds`]: tells `rumble` what changed, and
    /// shakes the camera for the frame to come by `view_dt`, what the frame gave the view.
    pub fn end_cue_frame(
        &mut self,
        scene: &mut Scene,
        dt: f32,
        view_dt: f32,
        alpha: f32,
        rumble: &mut dyn RumbleSink,
    ) {
        let Some(cues) = &mut self.cues else { return };
        let (turn, offset) = cues.end_frame(dt, view_dt, alpha, rumble);
        // With a director on, its own frame has the shakes, and a turn its roll.
        if self.arena.as_mut().is_none_or(|arena| arena.director().is_none()) {
            scene.camera_mut().set_shake(turn, offset);
        }
    }

    /// Stops a fight's rumble at once: the app lost focus or the foreground.
    pub fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        if let Some(cues) = &mut self.cues {
            cues.stop_rumble(rumble);
        }
    }

    /// What the HUD shows of the fighters, once a fight has stepped, and how long it's gone on.
    pub fn hud_values(&self) -> Option<(sapphire_ui::HudValues, Duration)> {
        let arena = self.arena.as_deref()?;
        Some((arena.hud_values()?, arena.fight_time()))
    }

    /// How long the last frame's camera took (rule R9): nothing without the arena.
    pub fn arena_mut(&mut self) -> Option<&mut ArenaActors> {
        self.arena.as_deref_mut()
    }

    pub fn camera_time(&self) -> Duration {
        self.camera_time
    }

    /// The clips `Self::hold_graph_clips` has to keep for `entity`'s player.
    fn hold_graph_clips(
        &self,
        scene: &Scene,
        content: &mut Content,
    ) -> Result<(), sapphire_assets::LoadError> {
        let character = scene.clips_in_use(Some(scene.character()));
        let enemy = scene.clips_in_use(self.enemy.map(|(warrior, _)| warrior));
        content.hold_graph_clips(&[(ENEMY_GRAPH, &enemy), (GraphKey::CHARACTER, &character)])
    }

    /// Takes in a new version of the enemy's mesh or skeleton, whose joints may be others: the pose it
    /// holds, the hand its blade follows, what lights them, and in a fight a driver that starts now from
    /// where the old one was, as the character's does (`Scene::replace_graph`). Those can land frames
    /// apart and its clips later still, so until all of it fits it keeps everything it has, and tries again
    /// as each part lands.
    fn replace_enemy(&mut self, scene: &mut Scene, content: &mut Content) {
        let Some((warrior, blade)) = self.enemy else { return };
        if let Err(error) = self.hold_graph_clips(scene, content) {
            tracing::warn!(target: LOG_TARGET, %error, "a fighter's graph lost a clip");
        }
        let ground = self.arena.as_ref().filter(|_| self.fighters_posed).map(|arena| arena.ground());
        let fitted = self.held_enemy(content).and_then(|held| {
            let driver = ground.map(|ground| self.enemy_driver(content, ground)).transpose()?;
            Ok((held, driver))
        });
        let (held, driver) = match fitted {
            Ok(fitted) => fitted,
            Err(error) => {
                self.enemy_waits = true;
                tracing::warn!(target: LOG_TARGET, %error, "the enemy keeps its pose until the rest of it lands");
                return;
            }
        };
        self.enemy_waits = false;
        let carried = enemy::blade_figure(&held);
        let world = scene.world_mut();
        let place = world.get::<RenderTransform>(warrior).map_or(Mat4::IDENTITY, |transform| transform.0);
        let mut character = world.entity_mut(warrior);
        if let (Some(mut playing), Some(mut driver)) = (character.get_mut::<GraphDriver>(), driver) {
            driver.continue_from(&playing);
            *playing = driver;
        }
        if let Some(mut rig) = character.get_mut::<CharacterLightRig>() {
            rig.head = held.figure.head;
        }
        character.insert((held.pose, held.figure));
        let mut item = world.entity_mut(blade);
        if let Some(mut rig) = item.get_mut::<CharacterLightRig>() {
            rig.head = carried.head;
        }
        item.insert((carried, RenderTransform(place * held.blade)));
        if let Some(arena) = &mut self.arena {
            arena.follow_new_enemy(held.hand, held.blade);
        }
    }

    /// Plays each fighter's graph for this frame, from what the last step said of it, solves its feet
    /// and its look at the other, and moves the enemy's blade to its hand. Two run in turn on this
    /// thread, so there's no list of jobs to make, and this allocates nothing; a crowd would want
    /// `evaluate_all`'s jobs and a retained list. A fighter whose mesh is changing keeps the pose it has
    /// until it fits.
    fn animate_fighters(
        &mut self,
        scene: &mut Scene,
        player: &ClipTable<'_>,
        enemy: &ClipTable<'_>,
        alpha: f32,
    ) {
        profiling::scope!("animate fighters");
        let started = Stopwatch::start();
        let mut stats = AnimStats::default();
        let (Some(arena), Some(graph_players)) = (&mut self.arena, &mut self.graph_players) else { return };
        let view = scene.spring_view();
        let world = scene.world_mut();
        let entities = [arena.fighters()[0].0, arena.fighters()[1].0];
        for (index, (entity, clips)) in entities.into_iter().zip([player, enemy]).enumerate() {
            let Ok((mut driver, render, figure, rig, _)) = graph_players.get_mut(world, entity) else {
                continue;
            };
            if !driver.fits(&render) {
                continue;
            }
            // After that, since it spends a cut and a landing, which wait for a frame the graph plays.
            let Some(input) = arena.graph_input(index, alpha) else { continue };
            stats.characters += 1;
            let place = arena.drawn(index, alpha);
            if let Some(ik) = driver.ik_mut() {
                ik.set_look_target(arena.look_target(index, alpha));
            }
            let mut job = GraphEvaluation::new(driver, render, figure, rig, input, place, &view);
            job.run(clips);
            stats.teleports += job.driver.player.teleports();
            if job.evaluated {
                stats.evaluated += 1;
                stats.joints += job.driver.player.pose().joint_count() as u32;
                stats.ik += u32::from(job.driver.has_ik());
                if index == 1 {
                    let hand = job.driver.player.pose().model()[arena.blade_hand()];
                    arena.hold_blade(Mat4::from(hand));
                }
            }
            stats.springs += u32::from(job.spring_work.wrote_pose);
            stats.spring_substeps += job.spring_work.substeps;
            stats.spring_time += job.spring_time;
        }
        stats.time = started.elapsed();
        scene.set_anim_stats(stats);
    }

    /// Takes new screen effect presets (live tuning, `dev` builds), if the table firing a fight's cues finds
    /// its names there, and says what they draw through. Presets that lose one a fight's cues push are
    /// refused.
    #[cfg(feature = "dev")]
    pub fn set_effect_library(&mut self, scene: &mut Scene, library: EffectLibrary) -> Result<(), String> {
        if let Some(cues) = &mut self.cues {
            cues.take_presets(&library).map_err(|error| error.to_string())?;
            scene.will_push_effects(cues.post_variants(&library));
        }
        scene.set_effect_library(library);
        Ok(())
    }

    /// Replaces the effects a fight's cues and sword trails play (live tuning, `dev` builds), if the table
    /// firing them finds its names there. With nothing firing cues, it says so.
    #[cfg(feature = "dev")]
    pub fn set_vfx_effects(
        &mut self,
        scene: &mut Scene,
        effects: sapphire_vfx::EffectLibrary,
    ) -> Result<(), String> {
        let Some(cues) = &mut self.cues else {
            return Err("nothing plays a fight's effects here: `--cues` does".to_owned());
        };
        scene
            .with_screen_effects(|_, presets| cues.take_effects(effects, presets))
            .map_err(|error| error.to_string())
    }

    /// Replaces the table a fight's cues fire through (live tuning, `dev` builds), if its names are all
    /// there. With nothing firing cues, it says so.
    #[cfg(feature = "dev")]
    pub fn set_cue_table(&mut self, scene: &mut Scene, table: sapphire_cues::CueTable) -> Result<(), String> {
        let Some(cues) = &mut self.cues else {
            return Err("nothing fires a fight's cues here: `--cues` does".to_owned());
        };
        let variants = scene.with_screen_effects(|_, presets| {
            cues.take_table(table, presets).map_err(|error| error.to_string())?;
            Ok::<_, String>(cues.post_variants(presets))
        })?;
        scene.will_push_effects(variants);
        Ok(())
    }

    /// Gives new camera tuning to what plays it (M2 T2): the arena's director and a fight's cues, whose
    /// shake IDs are places in the tuning's list and so are resolved again. All of them take it or none
    /// does: a value out of range, or a shake the cue table names and the tuning has lost, keeps everything
    /// as it was.
    #[cfg(feature = "dev")]
    pub fn set_camera_tuning(&mut self, scene: &mut Scene, tuning: CameraTuning) -> Result<(), String> {
        tuning.validate().map_err(|error| error.to_string())?;
        let director = self.arena.as_deref().is_some_and(ArenaActors::has_director);
        if !director && self.cues.is_none() {
            return Err(match self.arena {
                Some(_) => "this shot has no camera director: `--arena-shot director` has one",
                None => {
                    "no camera director runs here, and nothing fires a fight's shakes: `--arena` has one, \
                         and `--cues` does the other"
                }
            }
            .to_owned());
        }
        if let Some(cues) = &mut self.cues {
            scene
                .with_screen_effects(|_, presets| cues.take_camera_tuning(tuning.clone(), presets))
                .map_err(|error| error.to_string())?;
        }
        match self.arena.as_mut().filter(|arena| arena.has_director()) {
            Some(arena) => arena.set_camera_tuning(tuning),
            None => Ok(()),
        }
    }

    /// Takes an edited graph (M2 T2): its clips loaded, bound against `owner`, and from then on what binds
    /// it, until a pack brings another. It takes effect at the next state's start
    /// (`GraphPlayer::replace`). What fails leaves what's playing alone, and an edit that comes while
    /// `owner` waits for the rest of a new version, whose pose has joints a binding may not, either ends
    /// the wait or waits with it (`Content::end_wait_on`).
    #[cfg(feature = "dev")]
    pub fn replace_graph_document(
        &mut self,
        scene: &mut Scene,
        owner: GraphKey,
        graph: sapphire_anim::AnimGraph,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
    ) -> Result<(), String> {
        if owner == GraphKey::CHARACTER {
            return scene.replace_graph_document(graph, content, renderer, gpu);
        }
        let entity = self.enemy.map(|(warrior, _)| warrior);
        let playing = entity
            .filter(|&entity| self.fighters_posed && scene.world().get::<GraphDriver>(entity).is_some());
        let Some(entity) = playing else {
            return Err("no enemy here plays a graph: `--arena --fight` has one".to_owned());
        };
        content.hold_clips_of(owner, &graph, &scene.clips_in_use(Some(entity)), renderer, gpu)?;
        let binding = self.enemy_binding_of(content, &graph);
        if self.enemy_waits {
            return content.end_wait_on(owner, graph, binding.map(drop), |content| {
                self.replace_enemy(scene, content);
                self.enemy_waits
            });
        }
        let binding = binding?;
        content.set_live_graph(owner, Some(graph));
        if let Some(mut playing) = scene.world_mut().get_mut::<GraphDriver>(entity) {
            playing.player.replace(Arc::new(binding));
        }
        Ok(())
    }
}

impl SceneShow for FightShow {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        inventory.effects |= self.cues.is_some();
    }

    fn follow_content(&mut self, scene: &mut Scene, content: &mut Content, changes: ContentChanges) {
        if self.take_enemy_changed(content) || (self.enemy_waits && changes.graphs) {
            self.replace_enemy(scene, content);
        }
        if !self.fighters_posed || !(changes.character || changes.graphs) {
            return;
        }
        if let Err(error) = self.hold_graph_clips(scene, content) {
            tracing::warn!(target: LOG_TARGET, %error, "a fighter's graph lost a clip");
        }
        // A new character is the player's alone, and may have come in the same frame as a new graph. An
        // enemy waiting for the rest of it has the joints it had, which a new binding's aren't.
        if !changes.graphs || self.enemy_waits {
            return;
        }
        let Some(enemy) = self.enemy.map(|(warrior, _)| warrior) else { return };
        let binding = self.enemy_binding(content);
        let Some(mut playing) = scene.world_mut().get_mut::<GraphDriver>(enemy) else { return };
        match binding {
            Ok(binding) => playing.player.replace(Arc::new(binding)),
            Err(error) => tracing::warn!(target: LOG_TARGET, %error, "the enemy keeps the graph it has"),
        }
    }

    fn animate(&mut self, scene: &mut Scene, content: &Content, alpha: f32) {
        if self.fighters_posed {
            let alpha = self.alpha_of_fighters(alpha);
            self.animate_fighters(scene, &content.graph_clips(), &content.graph_clips_of(ENEMY_GRAPH), alpha);
        }
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        self.present_timeline(scene, alpha);
        let alpha = self.alpha_of_fighters(alpha);
        if let Some(arena) = &mut self.arena {
            let started = Stopwatch::start();
            arena.present(scene.world_mut(), alpha, aspect);
            self.camera_time = started.elapsed();
        }
        self.follow_hand(scene);
        self.follow_blades(scene, alpha);
    }

    fn write_effects(&mut self, frame: &mut VfxFrame, alpha: f32) {
        if let Some(cues) = &mut self.cues {
            cues.write(frame, alpha);
        }
    }

    fn adjust_post(&mut self, post: &mut PostParams) {
        if let Some(show) = &self.timeline {
            show.player().apply_post(post);
        }
    }

    fn draw_debug(&mut self) {
        if let Some(arena) = &self.arena
            && CombatViews::current().colliders
        {
            arena.draw_colliders(&mut Facade);
        }
    }

    fn end_steps(&mut self, scene: &mut Scene, dt: f32) {
        self.end_cue_frame(scene, dt, dt, 1.0, &mut ());
    }
}

/// `desc`, as [`Fight::stand`] asks of a stage: the town's character and sky, the copy of its mesh its held
/// pose comes from, and the packs' pipelines built before anything's uploaded.
pub fn fight_stage(desc: StageDesc) -> StageDesc {
    StageDesc {
        character: lookdev::CHARACTER,
        sky: lookdev::SKY,
        keep_character_mesh: true,
        prewarm_packs: true,
        ..desc
    }
}

/// A fight beside the simulation it follows.
#[derive(Default)]
pub struct Fight {
    /// A simulation that takes a tick with each fixed step.
    pub combat: Option<Combat>,
    /// What it has on the scene.
    pub show: FightShow,
}

impl Fight {
    /// Attaches a combat after the show is built, preparing its actor-owned cue storage without following it.
    pub fn attach_combat(&mut self, combat: Option<Combat>) {
        if let (Some(cues), Some(combat)) = (&mut self.show.cues, &combat) {
            cues.prepare(combat.simulation());
        }
        self.combat = combat;
    }

    /// One step of the fight with no one playing it, after the scene's: the simulation, then what follows
    /// it. What's frame-rate here is the director's: a step of time is what a frame shows.
    pub fn step_alone(&mut self, scene: &mut Scene, laps: &mut FrameLaps) -> Followed {
        let mut stopwatch = Stopwatch::start();
        self.show.drive_camera(1.0 / TICK_HZ as f32, Vec2::ZERO, 0.0);
        laps.scene_step += stopwatch.lap();
        if self.show.holds_simulation() {
            return Followed::default();
        }
        let Some(combat) = &mut self.combat else { return Followed::default() };
        combat.step(&mut no_bindings);
        laps.simulation += stopwatch.lap();
        self.follow_step(scene, laps)
    }

    /// What follows a step the simulation just took, on the scene: its fighters and its cues.
    pub fn follow_step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) -> Followed {
        let Some(combat) = &self.combat else { return Followed::default() };
        let mut stopwatch = Stopwatch::start();
        laps.physics += combat.simulation().stats().controllers_last;
        let followed = self.show.follow_simulation(scene, combat.simulation(), combat.cast());
        self.show.show_cues(scene, combat.simulation());
        laps.cues += stopwatch.lap();
        followed
    }

    /// Stands `option`'s fight on a stage made from [`fight_stage`]: the arena in the director's view, both
    /// fighters on their graphs, the blade in the player's hand, the day and the cues, each for a device of
    /// `tier`. What's stood, its fighters where the simulation starts them, draws from the first frame; a bad
    /// scenario says why before anything's loaded. The caller prewarms, once it holds all it plays
    /// ([`Scene::prewarm`]).
    pub fn stand(
        &mut self,
        option: &CombatOption,
        scene: &mut Scene,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        tier: TierId,
    ) -> Result<Followed, AppInitError> {
        let combat = Combat::new(option).map_err(AppInitError::msg)?;
        let party = matches!(option, CombatOption::Fight { party: true, .. });
        crate::arena::check_scenario(&combat, party).map_err(AppInitError::msg)?;
        let show = &mut self.show;
        let town = content.town(renderer, gpu)?;
        let fog_cards = Some(content.fog_cards(renderer, gpu)?);
        let (pose, figure) = lookdev::hold(content, renderer, gpu)?;
        let ground = show.load_arena_ground(content, renderer, gpu)?;
        let world = ground.share();
        let look = LookDev { town, fog_cards, pose, figure };
        let enemy = show.load_enemy(content, renderer, gpu)?;
        let held = show.held_enemy(content)?;
        show.spawn_arena(scene, Arena { shot: ArenaShot::Director, look, ground, enemy, held });
        scene.open_town_zone(content, stream_settings(tier), renderer, gpu)?;

        content.load_graph(lookdev::GRAPH, renderer, gpu)?;
        show.load_enemy_graph(content, renderer, gpu)?;
        let player = lookdev::graph_driver(content).map_err(AppInitError::msg)?;
        let solver =
            scene.ik_on(world.clone(), None, content, player.player.pose()).map_err(AppInitError::msg)?;
        let enemy = show.enemy_driver(content, world)?;
        show.play_fighters(scene, player.with_ik(solver), enemy);
        scene.set_spring_quality(animate::spring_quality(tier));

        let sword = show.load_sword(content, renderer, gpu)?;
        let hand = lookdev::held_hand(content).map_err(AppInitError::msg)?;
        show.spawn_blade(scene, sword, hand);
        scene.set_day(DEFAULT_TIME_OF_DAY, None);
        show.play_cues(scene, vfx::budget(tier)).map_err(AppInitError::msg)?;
        let followed = show.follow_simulation(scene, combat.simulation(), combat.cast());
        self.attach_combat(Some(combat));
        scene.warm_fixed_step();
        Ok(followed)
    }

    /// An error if a replay has left its recorded checksums so far.
    pub fn check(&self) -> Result<(), String> {
        self.combat.as_ref().map_or(Ok(()), Combat::check)
    }

    /// Ends the replay or recording, if there is one: see [`Combat::finish`].
    pub fn finish(&mut self) -> Result<(), String> {
        if let Some(cues) = self.show.cue_show() {
            tracing::info!(target: LOG_TARGET, stats = ?cues.stats(), "fired a fight's cues");
        }
        self.combat.take().map_or(Ok(()), Combat::finish)
    }
}

impl SceneShow for Fight {
    fn declare_pipelines(&self, inventory: &mut PipelineInventory) {
        self.show.declare_pipelines(inventory);
    }

    fn step(&mut self, scene: &mut Scene, laps: &mut FrameLaps) {
        self.step_alone(scene, laps);
    }

    fn end_steps(&mut self, scene: &mut Scene, dt: f32) {
        self.show.end_steps(scene, dt);
    }

    fn follow_content(&mut self, scene: &mut Scene, content: &mut Content, changes: ContentChanges) {
        self.show.follow_content(scene, content, changes);
    }

    fn animate(&mut self, scene: &mut Scene, content: &Content, alpha: f32) {
        self.show.animate(scene, content, alpha);
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        // After `Scene::animate`, so M2 N3's `VisiblePose` has the frame's pose to read.
        if let Some(combat) = &self.combat {
            combat.draw_views();
        }
        self.show.present(scene, alpha, aspect);
    }

    fn write_effects(&mut self, frame: &mut VfxFrame, alpha: f32) {
        self.show.write_effects(frame, alpha);
    }

    fn adjust_post(&mut self, post: &mut PostParams) {
        self.show.adjust_post(post);
    }

    fn draw_debug(&mut self) {
        self.show.draw_debug();
    }
}

#[cfg(test)]
mod clock_tests;
#[cfg(all(test, feature = "dev"))]
mod content_tests;
#[cfg(test)]
mod scene_tests;
