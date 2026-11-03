//! A fight on a scene: what carries a simulation to the frame.
//!
//! [`FightShow`] holds what a fight has on the stage: the arena's actors, the enemy and the blades, who the
//! party has on the field, the cues firing and the world clock's share of a fight's slow motion. All of it is
//! ordinary entities in the scene's world, placed from what the simulation says of its actors, so the scene
//! itself knows nothing of a fight (`sapphire_stage::SceneShow`).
//!
//! [`Fight`] is that beside the simulation it follows, as a stage runs them: one step of each, in spec §4.5's
//! order.

mod overhead_profile;
mod stock_arena;

use std::sync::Arc;
use std::time::Duration;

use sapphire_anim::FootContact;
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
use sapphire_gameplay::body::{BodySample, HeadAuthority};
use sapphire_gameplay::replay::LiveMoves;
use sapphire_math::{Mat4, Vec2, Vec3};
use sapphire_physics::PhysicsWorld;
use sapphire_platform::AppInitError;
use sapphire_render::quality::TierId;
use sapphire_render::{MeshHandle, PipelineInventory, PostParams, Renderer, VfxFrame};
use sapphire_rhi::GpuContext;
use sapphire_sequencer::{Origin, Playing, TimelinePlayer};
use sapphire_time::{SUBTICKS_PER_TICK, ScaleHandle, ScaleSource, Stopwatch, TICK_HZ};
use sapphire_timeline::Timeline;
use sapphire_vfx::VfxBudget;
use sapphire_world::light_rig::{CharacterLightRig, LightRigId};
#[cfg(feature = "dev")]
use sapphire_world::screen_effects::EffectLibrary;

use crate::LOG_TARGET;
use crate::arena::{ArenaActors, ArenaEnemy, ArenaGround, ArenaShot, COLLIDERS, PLAYER_EYES};
use crate::blade::{self, HeldBlade};
#[cfg(feature = "dev")]
use crate::combat::SessionRecorder;
use crate::combat::{Combat, CombatOption, no_bindings};
use crate::combat_views::{CombatViews, Facade};
use crate::cues::{CueShow, CueStage};
use crate::enemy::{self, EnemyMeshes};
#[cfg(feature = "dev")]
use sapphire_gameplay::replay::document_file_name;
use sapphire_stage::anim_graph::{ClipTable, FixedBodyRenderer, GraphDriver, GraphEvaluation};
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

const HERO_HEAD: HeadAuthority = HeadAuthority {
    movement: sapphire_core::Name::new("light_3"),
    retained: 1.0,
    source_ticks: [7.0, 11.0, 16.0, 22.0],
};

struct FixedFighter {
    renderer: FixedBodyRenderer,
    actor: sapphire_gameplay::ActorId,
}

struct ImpactPair {
    actors: [sapphire_gameplay::ActorId; 2],
    samples: [BodySample; 2],
    held: Option<[sapphire_anim::GraphStepStamp; 2]>,
    presented: [bool; 2],
    release: Option<ImpactRelease>,
}

struct ImpactRelease {
    origins: [BodySample; 2],
    start: Option<[sapphire_anim::GraphStepStamp; 2]>,
}

impl ImpactRelease {
    fn valid(&self, current: [Option<sapphire_anim::GraphStepStamp>; 2]) -> bool {
        self.start.is_some_and(|start| {
            start.into_iter().zip(current).all(|(origin, now)| {
                now.is_some_and(|now| {
                    now.step >= origin.step
                        && now.generation == origin.generation
                        && now.owner_now >= origin.owner_now
                })
            })
        })
    }
}

impl ImpactPair {
    fn is_held(&self, simulation: &Simulation) -> bool {
        self.held
            .is_some_and(|held| Self::retains(held, self.actors.map(|actor| simulation.body_stamp(actor))))
    }

    fn retains(
        held: [sapphire_anim::GraphStepStamp; 2],
        current: [Option<sapphire_anim::GraphStepStamp>; 2],
    ) -> bool {
        held.into_iter().zip(current).all(|(contact, current)| {
            current.is_some_and(|current| {
                current.step >= contact.step
                    && current.generation == contact.generation
                    && current.owner_now == contact.owner_now
            })
        })
    }

    fn follow(&mut self, simulation: &Simulation, cut: bool) {
        if cut {
            self.held = None;
            self.presented = [false; 2];
            if let Some(release) = &mut self.release {
                release.start = None;
            }
            return;
        }
        let stamps = self.actors.map(|actor| simulation.body_stamp(actor));
        if self.held.is_some_and(|held| Self::retains(held, stamps)) {
            return;
        }
        let stopped = self.held.take();
        let presented = std::mem::take(&mut self.presented);
        if let Some(release) = &mut self.release {
            if let Some(stopped) = stopped
                && presented.into_iter().all(|shown| shown)
            {
                let paired = self.actors.into_iter().zip(&mut release.origins).zip(stopped).all(
                    |((actor, origin), held)| {
                        simulation.read_body(actor, 0.0, origin)
                            && origin.span().is_some_and(|span| {
                                span.before.generation == held.generation
                                    && span.after.generation == held.generation
                                    && span.before.owner_now == held.owner_now
                            })
                    },
                );
                release.start = paired.then_some(stopped);
            }
            if let Some(start) = release.start {
                let expired = self.actors.into_iter().zip(start).all(|(actor, start)| {
                    simulation.actor(actor).is_some_and(|view| {
                        view.clock_now
                            .as_subticks()
                            .saturating_sub(view.clock_delta.as_subticks())
                            .saturating_sub(start.owner_now.as_subticks())
                            >= FixedBodyRenderer::IMPACT_RELEASE_TICKS * SUBTICKS_PER_TICK as u64
                    })
                });
                let interrupted = simulation
                    .hits()
                    .events()
                    .iter()
                    .any(|hit| self.actors.contains(&hit.attacker) || self.actors.contains(&hit.target));
                if expired || interrupted || !release.valid(stamps) {
                    release.start = None;
                }
            }
        }
        let Some(hit) = simulation.applied_overhead_contact() else { return };
        if hit.pair_stop_ticks == 0 || [hit.event.target, hit.event.attacker] != self.actors {
            return;
        }
        let spans = [hit.witness.target, hit.witness.attacker];
        let paired =
            self.actors.into_iter().zip(&mut self.samples).zip(spans).all(|((actor, buffer), span)| {
                simulation.read_body(actor, hit.witness.witnessed_fraction, buffer)
                    && buffer.span() == Some(span)
            });
        if paired {
            self.held = Some(spans.map(|span| span.after));
        }
    }
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
    fixed_fighters: [Option<FixedFighter>; 2],
    impact: Option<ImpactPair>,
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
        let player = player
            .with_ground_turn()
            // The two-handed finisher spots through its rising hilt before its arms descend.
            .with_authored_head_look(HERO_HEAD.movement, HERO_HEAD.retained, HERO_HEAD.source_ticks);
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
            for (index, fixed) in self.fixed_fighters.iter_mut().enumerate() {
                if let Some(fixed) = fixed
                    && (started_over || followed.came_in[index])
                {
                    fixed.renderer.discontinuity();
                }
            }
        }
        if let Some(impact) = &mut self.impact {
            impact.follow(simulation, started_over || followed.came_in.into_iter().any(|came_in| came_in));
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
        presentation: Option<(&Content, Option<&Simulation>)>,
    ) {
        profiling::scope!("animate fighters");
        let simulation = presentation.and_then(|(_, simulation)| simulation);
        let moves = simulation.map(Simulation::move_library);
        let warrior_versions = if self.fixed_fighters[1].is_some() {
            presentation
                .and_then(|(content, _)| self.enemy_versions(content))
                .map(|(mesh, skeleton)| (mesh, 0, skeleton))
        } else {
            None
        };
        let started = Stopwatch::start();
        let mut stats = AnimStats::default();
        let (Some(arena), Some(graph_players)) = (&mut self.arena, &mut self.graph_players) else { return };
        let view = scene.spring_view();
        let world = scene.world_mut();
        let fighters = arena.fighters();
        let stick_released = fighters[0]
            .1
            .is_some_and(|actor| actor.controls.current.stick == sapphire_gameplay::Stick::CENTER);
        let entities = fighters.map(|(entity, _)| entity);
        for (index, (entity, clips)) in entities.into_iter().zip([player, enemy]).enumerate() {
            let Ok((mut driver, render, figure, rig, _)) = graph_players.get_mut(world, entity) else {
                continue;
            };
            if !driver.fits(&render) {
                continue;
            }
            if let Some(fixed) = &mut self.fixed_fighters[index] {
                let (content, _) = presentation.expect("fixed body presentation needs its source");
                let versions =
                    if index == 0 { content.current_character_versions() } else { warrior_versions };
                fixed.renderer.check_source(clips, versions);
                let simulation = simulation.expect("simulation body presentation needs its producer");
                let ready = if let Some(impact) = &mut self.impact
                    && impact.is_held(simulation)
                {
                    let ready = fixed.renderer.read_impact(&impact.samples[index]);
                    impact.presented[index] |= ready;
                    ready
                } else {
                    let ready = fixed.renderer.read_simulation(simulation, fixed.actor, alpha);
                    if ready
                        && let Some(impact) = &self.impact
                        && let Some(release) = &impact.release
                        && release.valid(impact.actors.map(|actor| simulation.body_stamp(actor)))
                    {
                        fixed.renderer.release_impact(&impact.samples[index], &release.origins[index]);
                    }
                    ready
                };
                if ready {
                    let (mut render, mut figure, mut rig) = (render, figure, rig);
                    stats.characters += 1;
                    let (evaluated, springs, elapsed) =
                        fixed.renderer.publish(&mut driver, &mut render, &mut figure, &mut rig, &view);
                    if evaluated {
                        stats.evaluated += 1;
                        stats.joints += driver.player.pose().joint_count() as u32;
                        if index == 1 {
                            let hand = driver.player.pose().model()[arena.blade_hand()];
                            arena.hold_blade(Mat4::from(hand));
                        }
                    }
                    stats.springs += u32::from(springs.wrote_pose);
                    stats.spring_substeps += springs.substeps;
                    stats.spring_time += elapsed;
                    continue;
                }
                if simulation.tick() != 0 {
                    stats.characters += 1;
                    continue;
                }
                // The title and a freshly reset arena can draw before the first body step.
                // At tick zero the ordinary graph poses the idle fighters without advancing combat.
                driver.player.pose_mut().clear_body_presentation();
            }
            let gather = index == 0 && stick_released && driver.has_ik();
            driver.player.request_grounded_gather(gather);
            // After that, since it spends a cut and a landing, which wait for a frame the graph plays.
            let Some(input) = arena.graph_input(index, alpha) else { continue };
            stats.characters += 1;
            let place = arena.drawn(index, alpha);
            if let Some(ik) = driver.ik_mut() {
                ik.set_look_target(arena.look_target(index, alpha));
                // Read plants in the move's own root space so extra gameplay travel
                // cannot turn a stationary shoe into a skating shoe.
                ik.set_foot_contact(if input.locomotion.grounded && !input.defeated {
                    if input.combat.is_none() {
                        // Both fighters author heel and forefoot rolls. Pin the
                        // loaded sole landmark rather than the ankle between them.
                        FootContact::RollingStance
                    } else {
                        moves
                            .zip(input.combat)
                            .and_then(|(moves, combat)| {
                                arena.authored_foot_motion(index, combat.position, moves)
                            })
                            .map_or(FootContact::Sole, FootContact::Authored)
                    }
                } else {
                    FootContact::Sole
                });
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

    fn animate_with_simulation(
        &mut self,
        scene: &mut Scene,
        content: &Content,
        alpha: f32,
        simulation: Option<&Simulation>,
    ) {
        if self.fighters_posed {
            let alpha = self.alpha_of_fighters(alpha);
            self.animate_fighters(
                scene,
                &content.graph_clips(),
                &content.graph_clips_of(ENEMY_GRAPH),
                alpha,
                Some((content, simulation)),
            );
        }
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
        if content.stock_inputs_locked() {
            return Err("editing a bound stock graph requires an unconfigured fight".to_owned());
        }
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
        self.animate_with_simulation(scene, content, alpha, None);
    }

    fn present(&mut self, scene: &mut Scene, alpha: f32, aspect: Option<f32>) {
        self.present_timeline(scene, alpha);
        let alpha = self.alpha_of_fighters(alpha);
        if let Some(arena) = &mut self.arena {
            let started = Stopwatch::start();
            if let Some(root) = self.fixed_fighters[1].as_ref().and_then(|fixed| fixed.renderer.root()) {
                arena.present_with_warrior_root(scene.world_mut(), alpha, aspect, root);
            } else {
                arena.present(scene.world_mut(), alpha, aspect);
            }
            self.camera_time = started.elapsed();
        }
        if let Some(root) = self.fixed_fighters[0].as_ref().and_then(|fixed| fixed.renderer.root()) {
            let entity = scene.character();
            if let Some(mut transform) = scene.world_mut().get_mut::<RenderTransform>(entity) {
                transform.set_if_neq(RenderTransform(root));
            }
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
    stock_arena: Option<stock_arena::StockArenaAssets>,
}

impl Fight {
    /// Sets up ordinary combat using the mounted CPU content before any renderer upload.
    /// Historical replays with matching unconfigured tables never load the stock profile.
    pub fn prepare_stock_combat(
        &mut self,
        option: &CombatOption,
        content: &mut Content,
    ) -> Result<Combat, AppInitError> {
        self.prepare_stock_combat_with(option, move || Ok(content))
    }

    /// Requests CPU content only when the fight needs the stock setup.
    pub fn prepare_stock_combat_with<'a>(
        &mut self,
        option: &CombatOption,
        content: impl FnOnce() -> Result<&'a mut Content, AppInitError>,
    ) -> Result<Combat, AppInitError> {
        self.prepare_stock(option, None, content)
    }

    /// Sets up `option`'s fight as [`Self::prepare_stock_combat`] does, from `moves` rather than what the build
    /// compiled in: what a build with those edits in its documents would fight. Only a fight can.
    pub fn prepare_stock_fight_on(
        &mut self,
        option: &CombatOption,
        moves: LiveMoves,
        content: &mut Content,
    ) -> Result<Combat, AppInitError> {
        self.prepare_stock(option, Some(moves), move || Ok(content))
    }

    fn prepare_stock<'a>(
        &mut self,
        option: &CombatOption,
        moves: Option<LiveMoves>,
        content: impl FnOnce() -> Result<&'a mut Content, AppInitError>,
    ) -> Result<Combat, AppInitError> {
        #[cfg(feature = "dev")]
        let mut bound_content = None;
        let combat = Combat::new_with_setup(option, moves, |scenario, simulation, cast| {
            if scenario.name != crate::arena::FIGHT_SCENARIO {
                return Ok(());
            }
            let content = content().map_err(|error| error.to_string())?;
            if self.stock_arena.is_none() {
                self.stock_arena = Some(stock_arena::StockArenaAssets::load(content)?);
            }
            self.stock_arena
                .as_ref()
                .expect("the checked stock package was loaded")
                .install(simulation, cast)?;
            #[cfg(feature = "dev")]
            {
                bound_content = Some(content);
            }
            Ok(())
        })
        .map_err(AppInitError::msg)?;
        #[cfg(feature = "dev")]
        if let Some(content) = bound_content {
            content.lock_stock_inputs();
        }
        Ok(combat)
    }

    /// Starts a debug-overlay recording between frames, using the retained package if this arena
    /// was already bound to the ordinary stock bodies.
    #[cfg(feature = "dev")]
    pub fn start_recording_session(
        &mut self,
        scene: &mut Scene,
        content: &Content,
        recorder: &mut SessionRecorder,
        scenario: &str,
        live: &LiveMoves,
    ) {
        let stock = self.combat.is_none() && scenario == crate::arena::FIGHT_SCENARIO;
        let Some(assets) = self.stock_arena.as_ref().filter(|_| stock) else {
            recorder.start_tuned(&mut self.combat, scenario, live);
            return;
        };
        let mut recording = None;
        recorder.start_tuned_with_setup(&mut recording, scenario, live, |scenario, simulation, cast| {
            if scenario.documents.iter().any(|path| live.latest(document_file_name(path)).is_some()) {
                return Err("Live move edits require an unconfigured recording.".into());
            }
            assets.install(simulation, cast)
        });
        if let Some(combat) = recording {
            match self.attach_prepared_combat(combat, scene, content) {
                Ok(()) => {
                    self.follow_step(scene, &mut FrameLaps::default());
                }
                Err(error) => {
                    self.combat = None;
                    self.clear_recording_presentation(scene);
                    recorder.note_error(error.to_string());
                }
            }
        }
    }

    /// Ends an overlay recording and retires its renderer state in the same operation. A failed
    /// save that continues recording also keeps its body source.
    #[cfg(feature = "dev")]
    pub fn finish_recording_session(
        &mut self,
        scene: &mut Scene,
        recorder: &mut SessionRecorder,
        save: bool,
    ) {
        let running = self.combat.is_some();
        if save {
            recorder.save(&mut self.combat);
        } else {
            recorder.discard(&mut self.combat);
        }
        if running && self.combat.is_none() {
            self.clear_recording_presentation(scene);
        }
    }

    #[cfg(feature = "dev")]
    fn clear_recording_presentation(&mut self, scene: &mut Scene) {
        if let Some(arena) = &self.show.arena {
            for (entity, _) in arena.fighters() {
                if let Some(mut driver) = scene.world_mut().get_mut::<GraphDriver>(entity) {
                    driver.player.pose_mut().clear_body_presentation();
                }
            }
        }
        self.show.fixed_fighters = [None, None];
        self.show.impact = None;
        if let Some((_, handle)) = self.show.slow_motion.take() {
            scene.world_clock_mut().0.pop_scale(handle);
        }
    }

    fn prepare_impact_presentation(&mut self) -> Result<(), AppInitError> {
        let actors = self.show.fixed_fighters.each_ref().map(|fixed| fixed.as_ref().map(|fixed| fixed.actor));
        let [Some(hero), Some(warrior)] = actors else {
            return Err(AppInitError::msg("impact presentation needs both simulation body renderers"));
        };
        let simulation = self
            .combat
            .as_ref()
            .map(Combat::simulation)
            .ok_or_else(|| AppInitError::msg("impact presentation needs its simulation"))?;
        let hero_sample = simulation
            .body_sample_buffer(hero)
            .ok_or_else(|| AppInitError::msg("impact presentation needs the heroine's body binding"))?;
        let warrior_sample = simulation
            .body_sample_buffer(warrior)
            .ok_or_else(|| AppInitError::msg("impact presentation needs the warrior's body binding"))?;
        let origins = [hero, warrior].map(|actor| simulation.body_sample_buffer(actor));
        let [Some(hero_origin), Some(warrior_origin)] = origins else {
            return Err(AppInitError::msg("impact release needs both original body bindings"));
        };
        let release = Some(ImpactRelease { origins: [hero_origin, warrior_origin], start: None });
        self.show.impact = Some(ImpactPair {
            actors: [hero, warrior],
            samples: [hero_sample, warrior_sample],
            held: None,
            presented: [false; 2],
            release,
        });
        Ok(())
    }

    /// Attaches a combat after the show is built, preparing its actor-owned cue storage without following it.
    pub fn attach_combat(&mut self, combat: Option<Combat>) {
        if let (Some(cues), Some(combat)) = (&mut self.show.cues, &combat) {
            cues.prepare(combat.simulation());
        }
        self.combat = combat;
    }

    /// Attaches a checked ordinary fight after the cooked scene and graphs are ready.
    /// A configured fight uses only the simulation's existing body buffers for presentation.
    pub fn attach_prepared_combat(
        &mut self,
        combat: Combat,
        scene: &mut Scene,
        content: &Content,
    ) -> Result<(), AppInitError> {
        let simulation = combat.simulation();
        let Some(actors) = simulation.overhead_presentation_pair() else {
            if self.show.fixed_fighters.iter().any(Option::is_some) {
                return Err(AppInitError::msg("changing body authority requires a fresh arena scene"));
            }
            self.attach_combat(Some(combat));
            return Ok(());
        };
        if combat.cast() != actors.as_slice() {
            return Err(AppInitError::msg("the prepared body actors differ from the arena cast"));
        }
        let arena =
            self.show.arena.as_ref().ok_or_else(|| AppInitError::msg("the body pair needs an arena"))?;
        let [(hero, _), (warrior, _)] = arena.fighters();
        let warrior_versions = self.show.enemy_versions(content).map(|(mesh, skeleton)| (mesh, 0, skeleton));
        let sources = [
            (hero, content.graph_clips(), content.current_character_versions()),
            (warrior, content.graph_clips_of(ENEMY_GRAPH), warrior_versions),
        ];
        let mut fixed = [None, None];
        for (index, (entity, clips, versions)) in sources.into_iter().enumerate() {
            let actor = actors[index];
            let sample = simulation
                .body_sample_buffer(actor)
                .ok_or_else(|| AppInitError::msg("the prepared simulation lacks its body buffer"))?;
            let mut driver = scene
                .world_mut()
                .get_mut::<GraphDriver>(entity)
                .ok_or_else(|| AppInitError::msg("the prepared body's render graph is missing"))?;
            let mut renderer =
                driver.simulation_body_renderer(&clips, versions, sample).map_err(AppInitError::msg)?;
            renderer.discontinuity();
            fixed[index] = Some(FixedFighter { renderer, actor });
        }
        self.show.impact = None;
        self.show.fixed_fighters = fixed;
        self.attach_combat(Some(combat));
        self.prepare_impact_presentation()
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
        self.stand_prepared(combat, scene, content, renderer, gpu, tier)
    }

    /// Stands a checked combat whose CPU bootstrap already finished.
    pub fn stand_prepared(
        &mut self,
        combat: Combat,
        scene: &mut Scene,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &GpuContext,
        tier: TierId,
    ) -> Result<Followed, AppInitError> {
        crate::arena::check_scenario(&combat, combat.simulation().party().is_formed())
            .map_err(AppInitError::msg)?;
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
        self.attach_prepared_combat(combat, scene, content)?;
        scene.warm_fixed_step();
        Ok(followed)
    }

    /// Starts what's stood over, as `option`'s fight from `moves`, with nothing uploaded: see
    /// [`Self::prepare_stock_fight_on`]. What can't be set up leaves what's playing as it was, and says why.
    pub fn start_on(
        &mut self,
        option: &CombatOption,
        moves: LiveMoves,
        scene: &mut Scene,
        content: &mut Content,
    ) -> Result<Followed, String> {
        let combat =
            self.prepare_stock_fight_on(option, moves, content).map_err(|error| error.to_string())?;
        let party = matches!(option, CombatOption::Fight { party: true, .. });
        crate::arena::check_scenario(&combat, party)?;
        let followed = self.show.follow_simulation(scene, combat.simulation(), combat.cast());
        self.attach_prepared_combat(combat, scene, content).map_err(|error| error.to_string())?;
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
        let simulation = self.combat.as_ref().map(Combat::simulation);
        self.show.animate_with_simulation(scene, content, alpha, simulation);
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
mod impact_tests {
    use super::ImpactPair;
    use sapphire_anim::GraphStepStamp;
    use sapphire_time::SimTime;

    #[test]
    fn impact_pair_retains_both_clocks_and_expires_on_resume_cut_or_rewind() {
        let contact = |owner_now| GraphStepStamp {
            step: 54,
            owner_now: SimTime::from_subticks(owner_now),
            generation: 2,
        };
        let held = [contact(17_750), contact(53_000)];
        for step in 54..=64 {
            let current = held.map(|stamp| Some(GraphStepStamp { step, ..stamp }));
            assert!(ImpactPair::retains(held, current));
            assert!(ImpactPair::retains(held, current));
        }
        let [hero, warrior] = held;
        let warrior = GraphStepStamp { step: 65, ..warrior };
        let fractional_resume =
            GraphStepStamp { step: 65, owner_now: SimTime::from_subticks(18_000), ..hero };
        assert!(!ImpactPair::retains(held, [Some(fractional_resume), Some(warrior)]));
        let cut = GraphStepStamp { step: 55, generation: 3, ..hero };
        assert!(!ImpactPair::retains(held, [Some(cut), Some(warrior)]));
        assert!(!ImpactPair::retains(held, [None, Some(warrior)]));
        assert!(!ImpactPair::retains(held, [Some(GraphStepStamp { step: 53, ..hero }), Some(warrior)]));
    }
}

#[cfg(test)]
mod clock_tests;
#[cfg(all(test, feature = "dev"))]
mod content_tests;
#[cfg(test)]
mod scene_tests;
