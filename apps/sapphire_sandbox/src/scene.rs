//! The sandbox scene, as ECS data: the character on its turntable, the world clock, and the
//! camera that frames it. Shared by the windowed app and the offscreen modes, so a capture shows
//! exactly what the app draws.
//!
//! - **Resources:** `WorldClock` (from `sapphire_ecs`), the [`OrbitCamera`], the slow-motion
//!   handle, [`TimeOfDay`] (the default day, on a clock of its own issued by the world clock),
//!   the character light rig presets ([`LightRigLibrary`], the shipped ones), the screen effects
//!   (M1 P3: the [`EffectStack`] on a clock the world clock issued, and the sample presets,
//!   [`EffectLibrary`]), and the [`RenderExtract`] packet ring that extraction fills.
//! - **The character entity:** a [`Turntable`], its `EntityClock`, the [`RenderMesh`] and
//!   [`RenderTransform`] it draws with, and its [`CharacterLightRig`]:
//!   the `gameplay` preset unless `--character-rig` names another.
//! - **Demo lights** (`--lights`): three colored local lights around the character, entities
//!   with a [`RenderLight`] each ([`Scene::spawn_demo_lights`]).
//! - **The skinned stalk** (`--skinned`): a procedural skinned and morphed mesh beside the
//!   character, with its own `EntityClock`, a [`Sway`] and a [`RenderPose`] (see
//!   [`crate::skinned`]).
//! - **The shader swatches** (`--swatches`): three spheres in a column on the character's other
//!   side, static, each with a [`RenderMesh`] and its placement ([`Scene::spawn_swatches`],
//!   [`crate::swatches`]).
//! - **The material layer samples** (`--materials`, M1 C7): four spheres in a column on the
//!   character's right and a leg beyond them, static, each with a [`RenderMesh`] and its
//!   placement ([`Scene::spawn_layer_samples`], [`crate::layers`]).
//! - **The garment samples** (`--two-sided`, M1 C8): a collar, a skirt and a ribbon on the
//!   character's left, each one-sided and two-sided, static, each with a [`RenderMesh`] and its
//!   placement ([`Scene::spawn_garments`], [`crate::garments`]).
//! - **The face rig** (`--faces`, `--face-layout`): rows of the face test head well beside the
//!   character, static, each with a [`FaceRig`] beside its [`RenderMesh`] and placement, which
//!   the camera frames instead of the character ([`Scene::spawn_face_rig`], [`crate::faces`]).
//! - **The rim rig** (`--rim-rig`): a row of the character before dark and bright panels on its
//!   other side, static, each figure a character with a [`RimKey`] turning its key light, which
//!   the camera frames instead of the character ([`Scene::spawn_rim_rig`], [`crate::rims`]).
//! - **The courtyard** (`--environment`): the environment shader's test scene around the
//!   character, one static entity with a [`RenderMesh`] at the origin, and a camera pulled back
//!   to show it ([`Scene::spawn_environment`], [`crate::environment`]); with fog, its fog cards,
//!   static entities of their own ([`Scene::spawn_fog_cards`]); with `--probes`, its arcade,
//!   another at the origin, a camera turned to it, and the key hours of the ambient probe grid
//!   baked for both, from which time of day picks the keys each frame's bounce blends (M1 W5,
//!   [`Scene::spawn_arcade`]).
//! - **The plants** (`--foliage`): trees, bushes and grass patches in the courtyard, static
//!   entities with a [`RenderMesh`] and their placement, which the wind sways in the shader,
//!   and a camera for the view asked for ([`Scene::spawn_foliage`], [`Scene::frame_foliage`],
//!   [`crate::foliage`]).
//! - **The outline row** (`--outline-row`): more of the character behind it, out to past the
//!   outline cutoff, each a character with the default light rig; and **the hard-edged sample**
//!   (`--hard-edges`), which the camera frames instead of the character
//!   ([`Scene::spawn_outline_row`], [`Scene::spawn_hard_edges`], [`crate::outlines`]).
//! - **The stage** (`--stage`): a floor at the character's feet and a pillar beside it, static
//!   environment instances for shadows to fall on and be cast by ([`Scene::spawn_stage`],
//!   [`crate::stage`]).
//! - **The look-dev stage** (`--look-dev`): the town, one static entity, its fog cards and its
//!   lamps (each a [`RenderLight`] whose intensity [`Scene::frame_packet`] sets from time of
//!   day's night before extraction), around the VRM character held in a pose: a
//!   [`RenderPose`] written once, which `Extract` sends while it is new, and the
//!   [`Figure`] it makes, where the light rig finds the character's head, body and face
//!   ([`Scene::spawn_look_dev`], [`crate::lookdev`]). A new version of the character's mesh or
//!   rig under a running app holds it anew ([`Scene::follow_content`]).
//!   `--clip` plays a clip on the character instead (M2 N2, [`Scene::play_clip`],
//!   [`crate::animate`]): a [`ClipPlayer`] rewrites its pose and [`Figure`] each frame its clock
//!   has moved.
//!   `--look-dev enemy` stands the enemy beside the character, a character with a [`Figure`] of its
//!   own, and its blade ([`Scene::spawn_enemy`], [`crate::enemy`]).
//! - **The HUD** (`--hud`, M2 I2): not ECS data: [`SandboxHud`] draws into each packet's interface
//!   list as [`Scene::frame_packet`] ends, and survives a new renderer with the scene.
//! - **The arena** (`--arena`): the same town, its lamps and its day, with the character at the
//!   player's spawn point and the enemy at the enemy's, each posed from where the simulation has
//!   its actor rather than by a turntable, and the camera following the player
//!   ([`Scene::spawn_arena`], [`Scene::follow_simulation`], [`crate::arena`]).
//! - **Systems:** time of day and the screen effects tick in `FixedFirst`, after the clocks;
//!   turntables spin, and
//!   stalks sway, in `FixedCombat`. `Extract` poses them,
//!   then writes the instances that changed, the lights and the poses that changed into the
//!   frame's packet (see [`crate::extract`]). The stalk's systems are added only with `--skinned`.
//! - **Each frame's packet** starts with the main view and the scene constants, which
//!   [`Scene::frame_packet`] copies whole: they are a few resources, small and new every frame.
//!   Time of day resolves the scene constants, with the camera's position as its zone volumes'
//!   probe, and writes them; the player's filter strength and sharpening (`post.filter_strength`,
//!   `post.sharpening` or `--sharpening`) go on top, then the zone's sky material, and
//!   `--cloud-cover`'s coverage, `--plain-sky`'s bare gradient, `--fog off`'s clear air (M1 W3)
//!   or `--plain-post`'s post without its compositing layers (M1 P2), if set, the scene's wind
//!   (M1 W6, calm unless `--foliage` or `--wind` set one), and last the effect stack's effects for
//!   the frame (M1 P3: `--post-effect`, the overlay's effects panel). After `Extract` has posed
//!   and extracted the instances, it resolves each character's light rig for the frame's camera
//!   into the packet's characters (M1 C2), with its face (M1 C3) and its body (M1 R4): the
//!   turntable's head turning with it and its body a capsule from its feet to its head, each rim
//!   rig figure's key turned to its angles, and each face rig head lit from its own angles (or by
//!   its GI alone, in the GI layouts), its body a sphere around it. The characters other than the
//!   face rig's heads take the scene's fade (M1 C7, [`Fade`]): whole, a fixed opacity (`--fade`),
//!   or the proximity fade by the camera's distance to their bodies (`--proximity-fade`).
//!
//! `Presentation` has no systems yet. Posing is presentation work, but a multi-threaded
//! schedule that has systems costs an allocation per run plus one per system, and wakes a
//! worker; an empty one costs nothing. In `Presentation` the pose cost 2 allocations and about
//! 8 µs more per frame (`--bench`, M5), so it runs at the head of `Extract`, on the calling thread.
//! A clip's pose (M2 N2, `--clip`) doesn't run in a schedule either: [`Scene::animate`] evaluates it
//! just before the packet, through a query made once, and costs nothing without a clip.
//!
//! Queued input is applied by [`Scene::apply`] just before a frame's fixed steps, outside them:
//! it logs, and logging allocates, which the fixed step must not (rule R6).

use std::sync::Arc;
use std::time::Duration;

use sapphire_anim::{AnimStats, ArchivedClip, SpringQuality, SpringWork, evaluate_all};
use sapphire_assets::mesh::ArchivedMesh;
use sapphire_core::debug_draw::{self, Collector};
use sapphire_cues::RumbleSink;
use sapphire_ecs::bevy_ecs::system::SystemState;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::Simulation;
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
use sapphire_physics::PhysicsWorld;
use sapphire_render::{
    CharacterBody, CharacterFace, CharacterInstance, FaceExpression, Fog, FramePacket, HeadBasis, LightKind,
    LocalLight, MaterialHandle, MeshHandle, PostVariants, Renderer, SkyLayers, ViewData, Wind,
};
use sapphire_time::{ScaleHandle, ScaleSource, Stopwatch, TICK_HZ};
use sapphire_vfx::VfxBudget;
use sapphire_world::fade::ProximityFade;
use sapphire_world::light_rig::{
    CharacterLightRig, LightRig, LightRigId, LightRigLibrary, LightRigRim, NamedLightRig,
};
use sapphire_world::screen_effects::{EffectLibrary, EffectStack, tick_screen_effects};
use sapphire_world::tod::{self, TimeOfDay, tick_time_of_day};

use crate::LOG_TARGET;
use crate::anim_graph::{ClipTable, GraphDriver, GraphEvaluation, GraphShow};
use crate::animate::{ClipPlayer, Evaluation, SpringView};
use crate::arena::{ArenaActors, ArenaEnemy, ArenaGround, ArenaShot};
use crate::audio::SceneSound;
use crate::blade::{self, HeldBlade};
use crate::combat_views::{CombatViews, Facade};
use crate::content::Content;
#[cfg(feature = "dev")]
use crate::content::GraphOwner;
use crate::cues::{CueShow, CueSounds, CueStage};
use crate::debug_shapes::DebugDrawDemo;
use crate::enemy::{self, EnemyMeshes};
use crate::environment;
use crate::extract::{self, RenderExtract, RenderLight, RenderMesh, RenderPose, RenderTransform};
use crate::faces::{self, FaceLayout, FaceRig};
use crate::foliage::{self, FoliageMeshes, FoliageView, Plant};
use crate::garments;
use crate::hud::SandboxHud;
use crate::ik::{IkControls, IkDriver, IkGround, IkLook, IkOptions};
use crate::input::{SandboxInput, SimActions};
use crate::layers::{self, SAMPLES};
use crate::lookdev::{self, Shot, TownMeshes};
use crate::orbit::OrbitCamera;
use crate::outlines::{self, ROW};
use crate::party_looks::Drawn;
use crate::rims::{self, Panel, RimKey, RimLayout, RimRigMeshes};
use crate::skinned::{self, Sway};
use crate::stage::{self, Piece, StageLayout, StageMeshes, Subject};
use crate::swatches::{self, SWATCHES};
use crate::turntable::{self, Turntable};
use crate::vfx::{VfxPlayer, VfxShow};

/// World slow motion while toggled, in per-mille (perfect-dodge style, spec §4.5).
const SLOW_MOTION: u32 = 200;
/// Fixed steps the hitstop action freezes the turntable for.
const HITSTOP_TICKS: u16 = 8;
/// The hour the scene starts at unless `--time-of-day` says otherwise: the default day's 10:00
/// key, which is exactly the look the sandbox drew before time of day, so golden images that
/// don't choose a time are unchanged.
pub(crate) const DEFAULT_TIME_OF_DAY: f32 = 10.0;
/// The radius of a face rig head's body: a sphere around the head.
const HEAD_BODY_RADIUS: f32 = 0.16;

/// The characters the light rig lights: instances drawn with a [`CharacterLightRig`], and the
/// rim rig's key, if one is a figure of it.
type Characters = QueryState<
    (
        Entity,
        &'static CharacterLightRig,
        &'static RenderTransform,
        Option<&'static RimKey>,
        Option<&'static Figure>,
    ),
    With<RenderMesh>,
>;

/// The characters playing a clip, with everything that follows their skeletons.
type Players = QueryState<(
    &'static mut ClipPlayer,
    &'static EntityClock,
    &'static mut RenderPose,
    &'static mut Figure,
    &'static mut CharacterLightRig,
    &'static mut RenderTransform,
)>;

/// The characters playing a graph, with everything that follows their skeletons.
type GraphPlayers = QueryState<(
    &'static mut GraphDriver,
    &'static mut RenderPose,
    &'static mut Figure,
    &'static mut CharacterLightRig,
    &'static mut RenderTransform,
)>;

/// The face rig's heads, each a character lit from its own angles (see [`crate::faces`]).
type FaceHeads = QueryState<(Entity, &'static FaceRig, &'static RenderTransform), With<RenderMesh>>;

/// The effect stack and its presets, borrowed together in place. `World::resource_scope` would
/// take the stack off its resource entity and put it back, two archetype moves whose first use
/// builds archetypes and tables (allocating), and the overlay's effect controls run before every
/// fixed step, which must not allocate (rule R6).
type ScreenEffects = SystemState<(ResMut<'static, EffectStack>, Res<'static, EffectLibrary>)>;

/// World slow motion, while it is toggled on.
#[derive(Resource, Debug, Default)]
struct SlowMotion(Option<ScaleHandle>);

/// Where the character's light rig finds its head and body, in the character's own space. A
/// component on a character whose pose moves, which follows its skeleton every frame (M2 N2), and on
/// one of another shape than the scene's character (M2 O4).
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct Figure {
    /// The middle of the head: face and hair GI turn toward a sphere around it, and the light
    /// rig's limits are judged around it.
    pub head: Vec3,
    /// Where it stands.
    pub feet: Vec3,
    /// The radius of its body capsule (M1 R4), from its feet to its head: its torso's, which its
    /// shadow is cast from.
    pub body_radius: f32,
    /// How its head is turned from facing +Z upright: its face's basis (M1 C3).
    pub face: Quat,
    /// The expression its face holds: its blush and gloom weights (M1 C3).
    pub expression: FaceExpression,
}

impl Figure {
    /// The figure something held draws as, placed by `item` in this figure's space, in its own space:
    /// no body, so it adds no capsule to the holder's shadow or occlusion, and its head where the
    /// holder's is, so the light rig lights the two alike.
    pub fn of_item_at(&self, item: Mat4) -> Self {
        let head = item.inverse().transform_point3(self.head);
        Self { head, feet: head, body_radius: 0.0, face: Quat::IDENTITY, expression: self.expression }
    }

    /// The sample mannequin (`sapphire_pipeline`'s samples): its head sphere 1.47 m above its
    /// 0.2 m pedestal, its torso's radius, its head turning with it.
    pub const MANNEQUIN: Self = Self {
        head: Vec3::new(0.0, 1.67, 0.0),
        feet: Vec3::new(0.0, 0.2, 0.0),
        body_radius: 0.24,
        face: Quat::IDENTITY,
        expression: FaceExpression { blush: 0.0, gloom: 0.0 },
    };
}

/// A lantern's light in the look-dev town, lit by night: its [`RenderLight`]'s intensity is this
/// times time of day's `day_night` (see [`crate::lookdev`]).
#[derive(Component, Debug)]
struct Lamp {
    intensity: f32,
}

/// The look-dev town's lamps, whose intensities follow the night.
type Lamps = QueryState<(&'static Lamp, &'static mut RenderLight)>;

/// What the look-dev stage (`--look-dev`) draws: the town, its fog cards where frames draw fog,
/// and the character's held pose and the figure it makes.
pub(crate) struct LookDev {
    pub town: TownMeshes,
    pub fog_cards: Option<[MeshHandle; environment::FOG_CARDS.len()]>,
    pub pose: RenderPose,
    pub figure: Figure,
}

/// What the arena (`--arena`) stands on and how it's framed, around what the look-dev stage draws,
/// and the enemy who fights there.
pub(crate) struct Arena {
    pub enemy: EnemyMeshes,
    pub held: enemy::Held,
    pub shot: ArenaShot,
    pub look: LookDev,
    pub ground: ArenaGround,
}

/// The day time of day runs on, by the document it comes from ([`Scene::day`]), for `dev`
/// builds, which replace it from hot reload and live link. Both days are timelines
/// (`TodTimeline`), so a timeline replaces the day only from the day's own file: the default
/// day, which `cargo xtask link watch` pushes whenever it changes, can't replace the look-dev
/// stage's ([`Scene::replace_day`]).
#[cfg(any(test, feature = "dev"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Day {
    /// The engine's default day, `crates/sapphire_world/default_day.ron`.
    Default,
    /// The look-dev stage's own (`--look-dev`), `content/lookdev/lookdev_day.ron`
    /// ([`lookdev::day`]).
    LookDev,
}

#[cfg(any(test, feature = "dev"))]
impl Day {
    /// The name of the file it comes from.
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Default => "default_day.ron",
            Self::LookDev => "lookdev_day.ron",
        }
    }

    /// What it is, in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default day",
            Self::LookDev => "look-dev day",
        }
    }

    /// The name of the file the character light rig presets come from with this day: the
    /// shipped presets, or the look-dev stage's own (M1 C8, [`lookdev::light_rigs`]).
    pub fn light_rigs_file_name(self) -> &'static str {
        match self {
            Self::Default => "default_light_rigs.ron",
            Self::LookDev => "lookdev_light_rigs.ron",
        }
    }
}

/// How the scene's characters fade (M1 C7; [`CharacterInstance::opacity`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum Fade {
    /// Whole.
    #[default]
    Off,
    /// At this opacity, 0 to 1 (`--fade`).
    Fixed(f32),
    /// By the camera's distance to each body (`--proximity-fade`).
    Proximity(ProximityFade),
}

impl Fade {
    /// The opacity of a character with `body` seen from `camera`.
    fn opacity(self, camera: Vec3, body: &CharacterBody) -> f32 {
        match self {
            Self::Off => 1.0,
            Self::Fixed(opacity) => opacity,
            Self::Proximity(fade) => fade.opacity(camera, body),
        }
    }
}

pub(crate) struct Scene {
    ecs: Ecs,
    character: Entity,
    /// The skinned stalk, with `--skinned`.
    stalk: Option<Entity>,
    /// The shader swatches, with `--swatches`.
    swatches: Option<[Entity; SWATCHES]>,
    /// The material layer samples, with `--materials`.
    layer_samples: Option<[Entity; SAMPLES]>,
    /// The garment samples, with `--two-sided`.
    garments: Option<[Entity; garments::SAMPLES]>,
    /// The face rig's heads, with `--faces`, and their layout.
    faces: Vec<Entity>,
    face_layout: FaceLayout,
    /// The rim rig's figures and backdrop panels, with `--rim-rig`.
    rim_figures: Vec<Entity>,
    rim_backdrop: Vec<(Entity, Panel)>,
    /// The courtyard, with `--environment`.
    courtyard: Option<Entity>,
    /// The courtyard's fog cards, with `--environment` and fog, each with its mesh's index in
    /// [`environment::FOG_CARDS`].
    fog_cards: Vec<(Entity, usize)>,
    /// The plants, with `--foliage`, and the view it asked for.
    foliage: Vec<(Entity, Plant)>,
    foliage_view: Option<FoliageView>,
    /// The air foliage moves in: calm unless `--foliage` or `--wind` set it.
    wind: Wind,
    /// The arcade, with `--probes`.
    arcade: Option<Entity>,
    /// The hours the renderer's probe grid was baked at (`--probes`); empty without one.
    probe_hours: Vec<f32>,
    /// The outline row, with `--outline-row`.
    outline_row: Option<[Entity; ROW.len()]>,
    /// The hard-edged outline sample, with `--hard-edges`.
    hard_edges: Option<Entity>,
    /// The stage's pieces, with `--stage`.
    stage: Vec<(Entity, Piece)>,
    /// The look-dev town, with `--look-dev`, its plants and its mountains.
    town: Option<Entity>,
    town_plants: Vec<(Entity, lookdev::Plant)>,
    town_mountains: Vec<Entity>,
    /// The enemy and its blade, with `--look-dev enemy` or `--arena`.
    enemy: Option<(Entity, Entity)>,
    /// The character's blade, with `--blade on`.
    blade: Option<HeldBlade>,
    /// The arena's actors, with `--arena`.
    arena: Option<Box<ArenaActors>>,
    /// The camera jumped since the last view: the next one says so, for temporal AA and the shadows.
    view_cut: bool,
    /// A fight's slow motion on the scene's world clock, while it runs: how slow, and its scale there.
    fight_slow_motion: Option<(u32, ScaleHandle)>,
    /// Whether the arena's fighters play graphs ([`Self::play_fighters`]).
    fighters_posed: bool,
    /// A new version of the enemy landed that its other parts don't fit yet ([`Self::replace_enemy`]).
    enemy_waits: bool,
    /// A new version of the character landed that its other parts or its graph's clips don't fit yet
    /// ([`Self::replace_graph`]).
    character_waits: bool,
    /// Which party member the character draws as, with `--party`.
    party_looks: Option<Drawn>,
    /// Where the character's light rig finds its head and body, for a character without a
    /// [`Figure`] of its own.
    figure: Figure,
    /// Whether any character plays a clip ([`Self::play_clip`]). A scene without one does no
    /// animation work at all, not even a query.
    animating: bool,
    /// Made once, like the others.
    players: Players,
    /// The simulation a graph plays from (`--graph`, [`Self::play_graph`]). A scene without one does
    /// no graph work at all, not even a query.
    graph_show: Option<Box<GraphShow>>,
    /// Made once, like the others.
    graph_players: GraphPlayers,
    /// What animation did in the last frame (rule R9).
    anim_stats: AnimStats,
    /// How the tier runs spring bones ([`Self::set_spring_quality`]).
    spring_quality: SpringQuality,
    /// The shape of the last frame drawn, which [`Self::animate`] reads before this frame's.
    aspect: f32,
    /// The ground IK casts for, and the blocks drawn for it, with `--ik on`.
    ik_ground: Option<Arc<PhysicsWorld>>,
    ik_blocks: Vec<Entity>,
    /// What the character looks at, and how high it stands, with `--ik-look`.
    ik_look: Option<(IkLook, f32)>,
    /// Whether `Extract` writes poses: with `--skinned`, a posed character or the face rig, whose
    /// heads' lids and gaze are poses (M1 C6).
    poses: bool,
    /// The character's object-space bounds, which demo lights, swatches and the face rig are
    /// placed around.
    bounds: [[f32; 3]; 2],
    /// Made once: a query made each frame would allocate.
    characters: Characters,
    heads: FaceHeads,
    /// Made once, like the queries: [`Self::with_screen_effects`] runs inside fixed steps.
    screen_effects: ScreenEffects,
    lamps: Lamps,
    /// The zone's sky material on the current renderer, once loaded.
    sky_material: Option<MaterialHandle>,
    /// Both cloud layers' coverage instead of time of day's (`--cloud-cover`).
    cloud_cover: Option<f32>,
    /// The sky's gradient alone (`--plain-sky`).
    plain_sky: bool,
    /// The combat HUD over the scene (`--hud`, M2 I2).
    hud: Option<SandboxHud>,
    /// Time of day's fog, unless `--fog off` (M1 W3).
    fog: bool,
    /// Post without its compositing layers (`--plain-post`).
    plain_post: bool,
    /// Merges every thread's debug draws into each frame's packet (M2 T1).
    debug_draw: Collector,
    /// `--debug-draw`'s draws.
    debug_draw_demo: Option<DebugDrawDemo>,
    /// The player's sharpening instead of the cvar's (`--sharpening`).
    sharpening: Option<f32>,
    /// How the characters fade (`--fade`, `--proximity-fade`).
    fade: Fade,
    /// The post variants of the presets [`Self::push_effect`] pushed (`--post-effect`).
    pushed_effects: PostVariants,
    /// The effects `--vfx` plays (M2 E2).
    vfx: Option<VfxPlayer>,
    /// A fight's cues, fired (`--cues`, M2 E3).
    cues: Option<CueShow>,
    /// Width over height of the last frame drawn, which puts a hit's point on screen.
    view_aspect: f32,
    /// How long the last frame's camera took: the arena's director or plain follow.
    camera_time: Duration,
}

impl Scene {
    /// A scene around a character with the object-space `bounds`, and with the skinned stalk
    /// beside it if `skinned`. The character and stalk draw once they have meshes
    /// ([`Self::set_character_mesh`], [`Self::set_stalk_mesh`]).
    pub fn new(bounds: [[f32; 3]; 2], skinned: bool) -> Self {
        let mut ecs = Ecs::new();
        let world = ecs.world_mut();
        world.insert_resource(OrbitCamera::framing(bounds));
        world.insert_resource(SlowMotion::default());
        world.insert_resource(RenderExtract::default());
        let day_clock = world.resource_mut::<WorldClock>().0.new_local_clock();
        let time_of_day = TimeOfDay::new(tod::default_day(), day_clock, DEFAULT_TIME_OF_DAY)
            .expect("the default day sets every group");
        world.insert_resource(time_of_day);
        world.insert_resource(LightRigLibrary::default_rigs());
        let effects_clock = world.resource_mut::<WorldClock>().0.new_local_clock();
        world.insert_resource(EffectStack::new(effects_clock));
        world.insert_resource(EffectLibrary::default_effects());
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        let figure = Figure::MANNEQUIN;
        let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: figure.head };
        let character = world.spawn((Turntable::default(), clock, rig)).id();
        let stalk = skinned.then(|| {
            let clock = world.resource_mut::<WorldClock>().new_entity_clock();
            let place = RenderTransform(skinned::placement(bounds));
            world.spawn((Sway::default(), clock, place, skinned::rest_pose())).id()
        });
        ecs.add_systems(FixedFirst, (tick_time_of_day, tick_screen_effects).after(SimSet::Clocks))
            .add_systems(FixedCombat, turntable::spin)
            .add_systems(
                Extract,
                (turntable::pose, extract::extract_instances, extract::extract_lights).chain(),
            );
        if skinned {
            ecs.add_systems(FixedCombat, skinned::sway).add_systems(
                Extract,
                (skinned::pose, extract::extract_poses).chain().after(extract::extract_lights),
            );
        }
        ecs.startup();
        let characters = ecs.world_mut().query_filtered();
        let heads = ecs.world_mut().query_filtered();
        let screen_effects = SystemState::new(ecs.world_mut());
        let lamps = ecs.world_mut().query();
        let players = ecs.world_mut().query();
        let graph_players = ecs.world_mut().query();
        Self {
            ecs,
            character,
            stalk,
            swatches: None,
            layer_samples: None,
            garments: None,
            faces: Vec::new(),
            face_layout: FaceLayout::default(),
            rim_figures: Vec::new(),
            rim_backdrop: Vec::new(),
            courtyard: None,
            fog_cards: Vec::new(),
            foliage: Vec::new(),
            foliage_view: None,
            wind: Wind::default(),
            arcade: None,
            probe_hours: Vec::new(),
            outline_row: None,
            hard_edges: None,
            stage: Vec::new(),
            town: None,
            town_plants: Vec::new(),
            town_mountains: Vec::new(),
            enemy: None,
            blade: None,
            arena: None,
            view_cut: false,
            fight_slow_motion: None,
            fighters_posed: false,
            enemy_waits: false,
            character_waits: false,
            party_looks: None,
            figure,
            animating: false,
            players,
            graph_show: None,
            graph_players,
            anim_stats: AnimStats::default(),
            spring_quality: SpringQuality::DESKTOP,
            aspect: 16.0 / 9.0,
            ik_ground: None,
            ik_blocks: Vec::new(),
            ik_look: None,
            poses: skinned,
            bounds,
            characters,
            heads,
            screen_effects,
            lamps,
            sky_material: None,
            cloud_cover: None,
            plain_sky: false,
            hud: None,
            fog: true,
            plain_post: false,
            debug_draw: Collector::new(),
            debug_draw_demo: None,
            sharpening: None,
            fade: Fade::Off,
            pushed_effects: PostVariants::NONE,
            vfx: None,
            cues: None,
            view_aspect: 16.0 / 9.0,
            camera_time: Duration::ZERO,
        }
    }

    /// [`Self::new`] with the character drawing `mesh`.
    pub fn with_character(mesh: MeshHandle, bounds: [[f32; 3]; 2], skinned: bool) -> Self {
        let mut scene = Self::new(bounds, skinned);
        scene.set_character_mesh(mesh);
        scene
    }

    /// Spawns three colored local lights around the character, placed by its bounds so they
    /// fall where the sun leaves shadow: a warm lantern beside its head on the right, a cyan
    /// light behind its left arm, and a narrow pink spot from above onto the floor in front of
    /// it. The golden scene `turntable-lights` shows them.
    pub fn spawn_demo_lights(&mut self) {
        let [min, max] = self.bounds.map(Vec3::from);
        let height = (max.y - min.y).max(0.1);
        let base = Vec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
        // In units of the character's height, from the middle of its feet.
        let at = |x: f32, y: f32, z: f32| base + Vec3::new(x, y, z) * height;
        let spot_position = at(-0.3, 1.1, 0.55);
        let spot_target = at(-0.08, 0.1, 0.16);
        let lights = [
            LocalLight {
                position: at(0.32, 0.78, 0.12),
                color: Vec3::new(1.0, 0.42, 0.12),
                intensity: 1.8,
                range: 0.55 * height,
                kind: LightKind::Point,
            },
            LocalLight {
                position: at(-0.28, 0.24, -0.22),
                color: Vec3::new(0.2, 0.75, 1.0),
                intensity: 2.0,
                range: 0.42 * height,
                kind: LightKind::Point,
            },
            LocalLight {
                position: spot_position,
                color: Vec3::new(1.0, 0.3, 0.65),
                intensity: 2.5,
                range: 1.8 * height,
                kind: LightKind::Spot {
                    direction: spot_target - spot_position,
                    inner_angle: 0.12,
                    outer_angle: 0.16,
                },
            },
        ];
        for light in lights {
            self.ecs.world_mut().spawn(RenderLight(light));
        }
    }

    /// Sets the turntable's speed in radians per second (`--spin`): fast spins show how edges and
    /// outlines hold up in motion under temporal AA.
    pub fn set_spin(&mut self, speed: f32) {
        if let Some(mut turntable) = self.ecs.world_mut().get_mut::<Turntable>(self.character) {
            turntable.speed = speed;
        }
    }

    /// Sets the hour, and runs a day in `day_length` seconds of world time (or stops the cycle
    /// with `None`).
    pub fn set_day(&mut self, hours: f32, day_length: Option<f32>) {
        let mut time_of_day = self.time_of_day_mut();
        let clock = time_of_day.clock_mut();
        clock.set_hours(hours);
        clock.set_day_length(day_length);
    }

    /// Lights the character with the light rig preset called `name`, from the scene's presets.
    /// Fails with the presets' names if there is none of that name.
    pub fn set_character_rig(&mut self, name: &str) -> Result<(), String> {
        let world = self.ecs.world_mut();
        let rigs = world.resource::<LightRigLibrary>();
        let Some(rig) = rigs.find(name) else {
            let names: Vec<&str> = rigs.names().collect();
            return Err(format!("no character light rig preset {name:?}; presets: {}", names.join(", ")));
        };
        if let Some(mut character) = world.get_mut::<CharacterLightRig>(self.character) {
            character.rig = rig;
        }
        Ok(())
    }

    /// Draws the sky with `material` (a sky material on the current renderer): at startup, and
    /// again after GPU device loss.
    pub fn set_sky_material(&mut self, material: MaterialHandle) {
        self.sky_material = Some(material);
    }

    /// Holds both cloud layers at `cover` (0 to 1; 0 is a clear sky) whatever time of day says,
    /// or lets time of day drive them again with `None`.
    pub fn set_cloud_cover(&mut self, cover: Option<f32>) {
        self.cloud_cover = cover;
    }

    /// Draws the sky's gradient alone, without the discs, glows, stars and clouds time of day
    /// gives it: a backdrop that stays the same at any hour, for scenes about something else.
    pub fn set_plain_sky(&mut self, plain: bool) {
        self.plain_sky = plain;
    }

    /// Draws time of day's fog (M1 W3), or none at all with `false`: every frame then draws as it
    /// did before fog, for scenes about something else (`--fog off`).
    pub fn set_fog(&mut self, fog: bool) {
        self.fog = fog;
    }

    /// Draws post as it was before its compositing layers (M1 P2): no diffusion glow, character
    /// bloom, gradient layers, lens flare, vignette tint or sharpening, whatever time of day and
    /// the player's settings give. For scenes about something else, as `--plain-sky` is for the
    /// sky.
    pub fn set_plain_post(&mut self, plain: bool) {
        self.plain_post = plain;
    }

    /// Sharpens by `amount` (0 to 1) instead of the `post.sharpening` cvar, or by the cvar again
    /// with `None`.
    pub fn set_sharpening(&mut self, amount: Option<f32>) {
        self.sharpening = amount;
    }

    /// Fades the characters (but the face rig's heads) as `fade` says (M1 C7).
    pub fn set_fade(&mut self, fade: Fade) {
        self.fade = fade;
    }

    /// Pushes the screen effect preset called `name` onto the effect stack (M1 P3): playing from
    /// its start, or held `at` seconds into it, which shows that frame of it however long the
    /// scene runs (`--post-effect <preset>@<seconds>`, for captures and benches). Fails with the
    /// presets' names if there is none of that name.
    pub fn push_effect(&mut self, name: &str, at: Option<f32>) -> Result<(), String> {
        let variants = self.with_screen_effects(|stack, library| {
            let Some(preset) = library.find(name).and_then(|id| library.preset(id)) else {
                let names: Vec<&str> = library.names().collect();
                return Err(format!("no screen effect preset {name:?}; presets: {}", names.join(", ")));
            };
            let push = stack.push(preset);
            if at.is_some() {
                stack.hold(push, at);
            }
            Ok(preset.post_variants())
        })?;
        self.pushed_effects |= variants;
        Ok(())
    }

    /// The post variants of the screen effects this scene may push (M1 P3), which the app has
    /// the renderer prewarm ([`Renderer::prewarm_post`](sapphire_render::Renderer::prewarm_post))
    /// whenever it makes one or the presets change, so an effect shows on the frame it fires: in
    /// `dev` builds every preset's, since the overlay's effects panel fires any of them, and
    /// otherwise those [`Self::push_effect`] pushed (`--post-effect`), the only ones the sandbox
    /// fires without the overlay. A golden scene without effects so builds none of their
    /// pipelines.
    pub fn effect_variants(&self) -> PostVariants {
        if cfg!(feature = "dev") {
            self.ecs.world().resource::<EffectLibrary>().post_variants() | self.pushed_effects
        } else {
            self.pushed_effects
        }
    }

    /// Runs `f` on the effect stack and the presets. Allocation-free, so it may run inside fixed
    /// steps (the overlay's effect controls do, rule R6).
    pub fn with_screen_effects<T>(&mut self, f: impl FnOnce(&mut EffectStack, &EffectLibrary) -> T) -> T {
        let (mut stack, library) = self
            .screen_effects
            .get_mut(self.ecs.world_mut())
            .expect("the scene inserts the effect stack and its presets");
        f(&mut stack, &library)
    }

    /// The effect stack and the presets, for the overlay.
    #[cfg(feature = "dev")]
    pub fn screen_effects(&self) -> (&EffectStack, &EffectLibrary) {
        let world = self.ecs.world();
        (world.resource(), world.resource())
    }

    /// Replaces the screen effect presets (hot reload, `dev` builds). Effects playing keep the
    /// layers they were pushed with. Presets that lose one a fight's cues push are refused.
    #[cfg(feature = "dev")]
    pub fn set_effect_library(&mut self, library: EffectLibrary) -> Result<(), String> {
        if let Some(cues) = &mut self.cues {
            cues.take_presets(&library).map_err(|error| error.to_string())?;
            self.pushed_effects |= cues.post_variants(&library);
        }
        self.ecs.world_mut().insert_resource(library);
        Ok(())
    }

    /// Replaces the table a fight's cues fire through (live tuning, `dev` builds), if its names are all
    /// there. With nothing firing cues, it says so.
    #[cfg(feature = "dev")]
    pub fn set_cue_table(&mut self, table: sapphire_cues::CueTable) -> Result<(), String> {
        let presets: &EffectLibrary = self.ecs.world().resource();
        let Some(cues) = &mut self.cues else {
            return Err("nothing fires a fight's cues here: `--cues` does".to_owned());
        };
        cues.take_table(table, presets).map_err(|error| error.to_string())?;
        self.pushed_effects |= cues.post_variants(presets);
        Ok(())
    }

    /// Replaces the character light rig presets with `rigs`, read from `origin` (a path or a file
    /// name), if `origin` is the presets' file for the day the scene runs on
    /// ([`Day::light_rigs_file_name`]: the shipped presets, or the look-dev stage's own), and says
    /// which; otherwise keeps them. Characters keep their preset IDs; one past the new presets' end
    /// is lit with their default. Hot reload and live link both come through here.
    #[cfg(any(test, feature = "dev"))]
    pub fn replace_light_rigs(&mut self, origin: &str, rigs: LightRigLibrary) -> Result<Day, String> {
        let day = self.day();
        let name = std::path::Path::new(origin).file_name().and_then(std::ffi::OsStr::to_str);
        if name != Some(day.light_rigs_file_name()) {
            return Err(format!(
                "the scene's light rigs come from {}, which alone replaces them",
                day.light_rigs_file_name()
            ));
        }
        self.ecs.world_mut().insert_resource(rigs);
        Ok(day)
    }

    /// Places the shader swatches (`--swatches`) in a column beside the character, drawing
    /// `meshes`, top to bottom. They never move, so they are extracted once.
    pub fn spawn_swatches(&mut self, meshes: [MeshHandle; SWATCHES]) {
        let bounds = self.bounds;
        let world = self.ecs.world_mut();
        let mut index = 0;
        self.swatches = Some(meshes.map(|mesh| {
            let place = RenderTransform(swatches::placement(bounds, index));
            index += 1;
            world.spawn((RenderMesh(mesh), place)).id()
        }));
    }

    /// Sets the meshes the swatches draw again: after GPU device loss.
    pub fn set_swatch_meshes(&mut self, meshes: [MeshHandle; SWATCHES]) {
        debug_assert!(self.swatches.is_some(), "no swatches were spawned");
        for (entity, mesh) in self.swatches.into_iter().flatten().zip(meshes) {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(mesh));
        }
    }

    /// Places the material layer samples (`--materials`, M1 C7) on the character's right,
    /// drawing `meshes` in [`layers::MATERIALS`] order. They never move, so they are extracted
    /// once.
    pub fn spawn_layer_samples(&mut self, meshes: [MeshHandle; SAMPLES]) {
        let bounds = self.bounds;
        let world = self.ecs.world_mut();
        let mut index = 0;
        self.layer_samples = Some(meshes.map(|mesh| {
            let place = RenderTransform(layers::placement(bounds, index));
            index += 1;
            world.spawn((RenderMesh(mesh), place)).id()
        }));
    }

    /// Sets the meshes the material layer samples draw again: after GPU device loss.
    pub fn set_layer_sample_meshes(&mut self, meshes: [MeshHandle; SAMPLES]) {
        debug_assert!(self.layer_samples.is_some(), "no material samples were spawned");
        for (entity, mesh) in self.layer_samples.into_iter().flatten().zip(meshes) {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(mesh));
        }
    }

    /// Places the garment samples (`--two-sided`, M1 C8) on the character's left, drawing
    /// `meshes` in [`garments::SAMPLES`] order. They never move, so they are extracted once.
    pub fn spawn_garments(&mut self, meshes: [MeshHandle; garments::SAMPLES]) {
        let bounds = self.bounds;
        let world = self.ecs.world_mut();
        let mut index = 0;
        self.garments = Some(meshes.map(|mesh| {
            let place = RenderTransform(garments::placement(bounds, index));
            index += 1;
            world.spawn((RenderMesh(mesh), place)).id()
        }));
    }

    /// Sets the meshes the garment samples draw again: after GPU device loss.
    pub fn set_garment_meshes(&mut self, meshes: [MeshHandle; garments::SAMPLES]) {
        debug_assert!(self.garments.is_some(), "no garment samples were spawned");
        for (entity, mesh) in self.garments.into_iter().flatten().zip(meshes) {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(mesh));
        }
    }

    /// Places the face look-dev rig (`--faces`) in `layout` well beside the character, its heads
    /// drawing `mesh` posed by their rigs' lids and gaze (M1 C6), and frames it with the camera
    /// instead of the character. The heads never move, so they and their poses are extracted
    /// once.
    pub fn spawn_face_rig(&mut self, mesh: MeshHandle, layout: FaceLayout) {
        self.extract_poses();
        let camera = faces::camera(self.bounds, layout);
        let heads = faces::heads(self.bounds, layout, &camera);
        let world = self.ecs.world_mut();
        world.insert_resource(camera);
        self.face_layout = layout;
        self.faces = heads
            .into_iter()
            .map(|(rig, place)| {
                world.spawn((rig, RenderMesh(mesh), RenderTransform(place), rig.render_pose())).id()
            })
            .collect();
    }

    /// Places the rim look-dev rig (`--rim-rig`) in `layout` on the character's other side,
    /// drawing `meshes`: its figures, each a character lit by the default light rig with its key
    /// turned by `layout`, before the backdrop's panels. The camera frames it instead of the
    /// character. Nothing moves, so it is extracted once.
    pub fn spawn_rim_rig(&mut self, layout: RimLayout, meshes: RimRigMeshes) {
        let camera = rims::camera(self.bounds);
        let figures = rims::figures(self.bounds, layout, &camera);
        let panels = rims::panels(self.bounds, &camera);
        let world = self.ecs.world_mut();
        world.insert_resource(camera);
        let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: self.figure.head };
        let figure = RenderMesh(meshes.figure);
        self.rim_figures = figures
            .into_iter()
            .map(|(key, place)| world.spawn((key, rig, figure, RenderTransform(place))).id())
            .collect();
        self.rim_backdrop = panels
            .into_iter()
            .map(|(panel, place)| {
                (world.spawn((RenderMesh(meshes.panel(panel)), RenderTransform(place))).id(), panel)
            })
            .collect();
    }

    /// Sets the meshes the rim rig draws again: after GPU device loss.
    pub fn set_rim_rig_meshes(&mut self, meshes: RimRigMeshes) {
        let world = self.ecs.world_mut();
        for &entity in &self.rim_figures {
            world.entity_mut(entity).insert(RenderMesh(meshes.figure));
        }
        for &(entity, panel) in &self.rim_backdrop {
            world.entity_mut(entity).insert(RenderMesh(meshes.panel(panel)));
        }
    }

    /// Builds the stage (`--stage`) around the subject, the face rig with `--faces` or else the
    /// character, drawing `meshes`: the character without its pedestal, which stands on the
    /// floor of its own (or on a stone plinth, with [`StageLayout::Plinth`]), and with
    /// [`StageLayout::Pillar`] a pillar placed for the key light at the scene's hour.
    /// Call after the hour is set. The pieces never move, so they are extracted once.
    pub fn spawn_stage(&mut self, layout: StageLayout, meshes: StageMeshes) {
        self.set_character_mesh(meshes.figure);
        let floor = self.bounds[0][1];
        let subject = match self.faces.is_empty() {
            // Every head's shadow from far enough to stay out of the rig's view.
            false => {
                let (center, half_height) = faces::rig_center(self.bounds, self.face_layout);
                Subject {
                    base: Vec3::new(center.x, floor, center.z),
                    shade_height: center.y + half_height + 0.1,
                    pillar_distance: 4.5,
                    pillar_width: 0.3,
                }
            }
            // From the feet to the chest, so the shadow's edge crosses the body.
            true => Subject {
                base: Vec3::new(0.0, floor, 0.0),
                shade_height: self.figure.feet.y + 0.65 * (self.figure.head.y - self.figure.feet.y),
                pillar_distance: 2.2,
                pillar_width: 0.5,
            },
        };
        let mut packet = FramePacket::default();
        let eye = self.ecs.world().resource::<OrbitCamera>().eye();
        self.time_of_day_mut().resolve(eye, 1.0).write(&mut packet);
        let world = self.ecs.world_mut();
        let pedestal = meshes.pedestal.map(|(_, bounds)| bounds);
        for (piece, place) in stage::placements(layout, &subject, pedestal, -packet.sun.direction) {
            let entity = world.spawn((RenderMesh(meshes.get(piece)), RenderTransform(place))).id();
            self.stage.push((entity, piece));
        }
    }

    /// Sets the meshes the stage draws again, the character's figure among them: after GPU device
    /// loss.
    pub fn set_stage_meshes(&mut self, meshes: StageMeshes) {
        self.set_character_mesh(meshes.figure);
        for &(entity, piece) in &self.stage {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(meshes.get(piece)));
        }
    }

    /// Sets the mesh the face rig's heads draw again: after GPU device loss.
    pub fn set_face_mesh(&mut self, mesh: MeshHandle) {
        debug_assert!(!self.faces.is_empty(), "no face rig was spawned");
        for &head in &self.faces {
            self.ecs.world_mut().entity_mut(head).insert(RenderMesh(mesh));
        }
    }

    /// Places the courtyard (`--environment`), drawing `mesh`, around the character, and frames the
    /// character from farther back to show it. It never moves, so it is extracted once.
    pub fn spawn_environment(&mut self, mesh: MeshHandle) {
        let camera = environment::camera(self.bounds);
        let world = self.ecs.world_mut();
        world.insert_resource(camera);
        self.courtyard = Some(world.spawn((RenderMesh(mesh), RenderTransform(Mat4::IDENTITY))).id());
    }

    /// Sets the mesh the courtyard draws again: after GPU device loss.
    pub fn set_courtyard_mesh(&mut self, mesh: MeshHandle) {
        debug_assert!(self.courtyard.is_some(), "no courtyard was spawned");
        if let Some(courtyard) = self.courtyard {
            self.ecs.world_mut().entity_mut(courtyard).insert(RenderMesh(mesh));
        }
    }

    /// Places the courtyard's fog cards (M1 W3), drawing `meshes` (in
    /// [`environment::FOG_CARDS`] order). They never move, so they are extracted once.
    pub fn spawn_fog_cards(&mut self, meshes: [MeshHandle; environment::FOG_CARDS.len()]) {
        let world = self.ecs.world_mut();
        self.fog_cards = environment::fog_cards()
            .map(|(mesh, place)| (world.spawn((RenderMesh(meshes[mesh]), RenderTransform(place))).id(), mesh))
            .collect();
    }

    /// Sets the meshes the fog cards draw again: after GPU device loss.
    pub fn set_fog_card_meshes(&mut self, meshes: [MeshHandle; environment::FOG_CARDS.len()]) {
        debug_assert!(!self.fog_cards.is_empty(), "no fog cards were placed");
        for &(card, mesh) in &self.fog_cards {
            self.ecs.world_mut().entity_mut(card).insert(RenderMesh(meshes[mesh]));
        }
    }

    /// Plants the foliage samples (`--foliage`) in the courtyard, drawing `meshes`, for `view`,
    /// which [`Self::frame_foliage`] frames. They never move but for the wind, so they are
    /// extracted once.
    pub fn spawn_foliage(&mut self, view: FoliageView, meshes: FoliageMeshes) {
        let world = self.ecs.world_mut();
        for (plant, place) in foliage::placements() {
            let entity = world.spawn((RenderMesh(meshes.get(plant)), RenderTransform(place))).id();
            self.foliage.push((entity, plant));
        }
        self.foliage_view = Some(view);
    }

    /// Points the camera at the plants as their view asks, the backlit view toward the key light
    /// at the scene's hour; nothing without foliage. Call after the hour is set.
    pub fn frame_foliage(&mut self) {
        let Some(view) = self.foliage_view else { return };
        let mut packet = FramePacket::default();
        let eye = self.ecs.world().resource::<OrbitCamera>().eye();
        self.time_of_day_mut().resolve(eye, 1.0).write(&mut packet);
        self.ecs.world_mut().insert_resource(foliage::camera(view, -packet.sun.direction));
    }

    /// Sets the meshes the plants draw again: after GPU device loss.
    pub fn set_foliage_meshes(&mut self, meshes: FoliageMeshes) {
        for &(entity, plant) in &self.foliage {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(meshes.get(plant)));
        }
    }

    /// Sets the wind foliage moves in.
    pub fn set_wind(&mut self, wind: Wind) {
        self.wind = wind;
    }

    /// Places the courtyard's arcade (`--probes`), drawing `mesh`, where its probe grid was baked
    /// with it, turns the camera to it, and takes the grid's key `hours`. It never moves, so it is
    /// extracted once.
    pub fn spawn_arcade(&mut self, mesh: MeshHandle, hours: &[f32]) {
        let camera = environment::arcade_camera(self.bounds);
        let world = self.ecs.world_mut();
        world.insert_resource(camera);
        self.arcade = Some(world.spawn((RenderMesh(mesh), RenderTransform(Mat4::IDENTITY))).id());
        self.set_probe_hours(hours);
    }

    /// Sets the mesh the arcade draws again: after GPU device loss.
    pub fn set_arcade_mesh(&mut self, mesh: MeshHandle) {
        if let Some(arcade) = self.arcade {
            self.ecs.world_mut().entity_mut(arcade).insert(RenderMesh(mesh));
        }
    }

    /// The hours the renderer's probe grid was baked at, which each frame's packet picks its
    /// bounce keys by: after a new grid (hot reload). Allocates nothing while they stay the same.
    pub fn set_probe_hours(&mut self, hours: &[f32]) {
        if self.probe_hours != hours {
            self.probe_hours.clear();
            self.probe_hours.extend_from_slice(hours);
        }
    }

    /// Takes in what this frame's loading changed, after [`Content::update`] and before the
    /// frame's packet: the probe grid's key hours; a mesh replaced in place, whose instances the
    /// renderer starts over in their bind pose, sends every pose again with this frame's packet,
    /// so the frame that first draws the new mesh already draws it posed; and a new version of the
    /// look-dev character's mesh, rig or skeleton holds it anew, since its palette indexes their
    /// joints, and a new version of them or of the clip gives the clip a new pose to play on. While
    /// only some have landed the character may not fit: it keeps the pose it has, which the renderer
    /// ignores if the new mesh's palette is another length, until the others land. The enemy's mesh and
    /// skeleton do the same for it (`Self::replace_enemy`). Allocates nothing while nothing changed.
    pub fn follow_content(&mut self, content: &mut Content) {
        self.set_probe_hours(content.probe_hours());
        if content.take_meshes_replaced() {
            self.ecs.world_mut().resource_mut::<RenderExtract>().resend_poses();
        }
        let clip_changed = content.take_clip_changed();
        let character_changed = content.take_character_changed();
        if character_changed {
            match lookdev::held(content) {
                Ok((pose, figure)) => self.hold_character(pose, figure),
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, error, "the look-dev character keeps its pose");
                }
            }
        }
        if self.animating && (character_changed || clip_changed) {
            self.replace_player(content);
        }
        let graph_changed = content.take_graph_changed();
        if content.take_enemy_changed() || (self.enemy_waits && graph_changed) {
            self.replace_enemy(content);
        }
        if (self.graph_show.is_some() || self.fighters_posed) && (character_changed || graph_changed) {
            self.replace_graph(content, character_changed, graph_changed);
        }
    }

    /// Plays a clip on the character from now on, instead of holding it still: from its transform
    /// as it stands, with its turntable gone, since root motion writes its transform now. The clip
    /// runs on its clock: hitstop and pause freeze its pose.
    pub fn play_clip(&mut self, player: ClipPlayer) {
        self.start_clip(self.character, player);
    }

    /// Starts `player`'s clip on `entity`, which draws a skinned mesh and is lit by a light rig.
    fn start_clip(&mut self, entity: Entity, mut player: ClipPlayer) {
        let figure = self.figure;
        let world = self.ecs.world_mut();
        let mut character = world.entity_mut(entity);
        let start = character.get::<EntityClock>().map_or(0.0, |clock| clock.0.presented_ticks(1.0));
        let base = character.get::<RenderTransform>().map_or(Mat4::IDENTITY, |transform| transform.0);
        player.start_at(start, base);
        let pose = player.render_pose();
        character.remove::<Turntable>();
        character.insert((player, pose, figure));
        self.animating = true;
        self.extract_poses();
    }

    /// Sets the character up for IK (`--ik on`, [`crate::ik`]): the ground's blocks drawn with `block`,
    /// and the character standing on them where the controller's capsule would. Call it before
    /// [`Self::play_clip`], whose root motion starts from where the character stands, and give the
    /// player it returns to that.
    pub fn start_ik(
        &mut self,
        options: &IkOptions,
        block: MeshHandle,
        content: &Content,
        pose: &sapphire_anim::CharacterPose,
    ) -> Result<IkDriver, String> {
        let ground = Arc::new(options.ground.world());
        let height = IkGround::stand(&ground, 0.0, 0.0);
        let world = self.ecs.world_mut();
        self.ik_blocks = options
            .ground
            .blocks()
            .into_iter()
            .map(|place| world.spawn((RenderMesh(block), RenderTransform(place))).id())
            .collect();
        if let Some(mut transform) = world.get_mut::<RenderTransform>(self.character) {
            transform.0 = Mat4::from_translation(Vec3::new(0.0, height, 0.0)) * transform.0;
        }
        world.resource_mut::<OrbitCamera>().raise_target(height);
        self.ik_on(ground, options.look.map(|look| (look, height)), content, pose)
    }

    /// IK for the character, with `pose` a pose of it, casting in `ground`, looking at `look` for a
    /// character standing at its height: what [`Self::start_ik`] does, for a stage with a ground of its
    /// own. Give it to what plays the character.
    pub fn ik_on(
        &mut self,
        ground: Arc<PhysicsWorld>,
        look: Option<(IkLook, f32)>,
        content: &Content,
        pose: &sapphire_anim::CharacterPose,
    ) -> Result<IkDriver, String> {
        let driver = IkDriver::new(lookdev::ik_rig(content, pose)?, ground.clone(), IkControls::default());
        self.ik_ground = Some(ground);
        self.ik_look = look;
        Ok(driver)
    }

    /// Sets the mesh the IK ground's blocks draw again: after GPU device loss.
    pub fn set_ik_block_mesh(&mut self, mesh: MeshHandle) {
        for &block in &self.ik_blocks {
            self.ecs.world_mut().entity_mut(block).insert(RenderMesh(mesh));
        }
    }

    /// A clip player for the character's new mesh, rig or skeleton: its clip plays on from where it was,
    /// with what its IK was told.
    fn replace_player(&mut self, content: &Content) {
        let replacement = lookdev::player(content);
        let character = self.character;
        let ground = self.ik_ground.clone();
        let Some(mut playing) = self.ecs.world_mut().get_mut::<ClipPlayer>(character) else { return };
        match replacement {
            Ok(mut player) => {
                player.continue_from(&playing);
                if let (Some(ground), Some(controls)) = (ground, playing.ik_mut().map(|ik| ik.controls())) {
                    match lookdev::ik_rig(content, player.pose()) {
                        Ok(rig) => player = player.with_ik(IkDriver::new(rig, ground, controls)),
                        Err(error) => {
                            tracing::warn!(target: LOG_TARGET, error, "the look-dev character's new rig has no IK");
                        }
                    }
                }
                *playing = player;
            }
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, error, "the look-dev character keeps its clip's pose");
            }
        }
    }

    /// Plays a graph on the character from now on, instead of holding it still, from what `show`'s
    /// simulation says ([`crate::anim_graph`]): from its transform as it stands, with its turntable gone,
    /// since its actor places it now.
    pub fn play_graph(&mut self, driver: GraphDriver, show: GraphShow) {
        let figure = self.figure;
        let character = self.character;
        let world = self.ecs.world_mut();
        let mut character = world.entity_mut(character);
        let base = character.get::<RenderTransform>().map_or(Mat4::IDENTITY, |transform| transform.0);
        let driver = driver.at(base);
        let pose = driver.render_pose();
        character.remove::<Turntable>();
        character.insert((driver, pose, figure));
        self.graph_show = Some(Box::new(show));
        self.extract_poses();
    }

    /// The simulation a graph plays from, with `--graph`.
    #[cfg(feature = "dev")]
    pub fn graph_show(&self) -> Option<&GraphShow> {
        self.graph_show.as_deref()
    }

    /// Gives the graph show a live edit of the document with this file name, if it plays that (M2 T2).
    #[cfg(feature = "dev")]
    pub fn tune_show(
        &mut self,
        document: &str,
        set: sapphire_moves::MoveSet,
    ) -> Option<(sapphire_gameplay::MoveSetId, sapphire_gameplay::Tuned)> {
        self.graph_show.as_mut()?.tune(document, set)
    }

    /// Gives new camera tuning to what plays it (M2 T2): the arena's director and a fight's cues, whose
    /// shake IDs are places in the tuning's list and so are resolved again. All of them take it or none
    /// does: a value out of range, or a shake the cue table names and the tuning has lost, keeps everything
    /// as it was.
    #[cfg(feature = "dev")]
    pub fn set_camera_tuning(&mut self, tuning: sapphire_camera::CameraTuning) -> Result<(), String> {
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
        let presets: &EffectLibrary = self.ecs.world().resource();
        if let Some(cues) = &mut self.cues {
            cues.take_camera_tuning(tuning.clone(), presets).map_err(|error| error.to_string())?;
        }
        match self.arena.as_mut().filter(|arena| arena.has_director()) {
            Some(arena) => arena.set_camera_tuning(tuning),
            None => Ok(()),
        }
    }

    /// Takes an edited graph (M2 T2): its clips loaded, bound against `owner`, and from then on what binds
    /// it, until a pack brings another. It takes effect at the next state's start
    /// ([`GraphPlayer::replace`]). What fails leaves what's playing alone, and so does an edit that comes
    /// while a new version of `owner` is still landing, whose pose has joints a binding may not.
    ///
    /// [`GraphPlayer::replace`]: sapphire_anim::GraphPlayer::replace
    #[cfg(feature = "dev")]
    pub fn replace_graph_document(
        &mut self,
        owner: GraphOwner,
        graph: sapphire_anim::AnimGraph,
        content: &mut Content,
        renderer: &mut Renderer,
        gpu: &sapphire_rhi::GpuContext,
    ) -> Result<(), String> {
        let (entity, posed, waits, none) = match owner {
            GraphOwner::Character => (
                Some(self.character),
                self.graph_show.is_some() || self.fighters_posed,
                self.character_waits,
                "no character here plays a graph: `--look-dev <shot> --graph <script>` and `--arena --fight` \
                 have one",
            ),
            GraphOwner::Enemy => (
                self.enemy.map(|(warrior, _)| warrior),
                self.fighters_posed,
                self.enemy_waits,
                "no enemy here plays a graph: `--arena --fight` has one",
            ),
        };
        let playing = entity.filter(|&entity| posed && self.ecs.world().get::<GraphDriver>(entity).is_some());
        let Some(entity) = playing else { return Err(none.to_owned()) };
        if waits {
            return Err("a new version of this character is still landing: push again once it has".to_owned());
        }
        content.hold_clips_of(owner, &graph, &self.clips_in_use(Some(entity)), renderer, gpu)?;
        let binding = match owner {
            GraphOwner::Character => lookdev::graph_binding_of(content, &graph)?,
            GraphOwner::Enemy => content.enemy_binding_of(&graph)?,
        };
        content.set_live_graph(owner, graph);
        if let Some(mut playing) = self.ecs.world_mut().get_mut::<GraphDriver>(entity) {
            playing.player.replace(Arc::new(binding));
        }
        Ok(())
    }

    /// The clips `entity`'s graph player may still read, which whatever replaces its graph has to keep.
    fn clips_in_use(&self, entity: Option<Entity>) -> Vec<sapphire_assets::AssetId> {
        let playing = entity.and_then(|entity| self.ecs.world().get::<GraphDriver>(entity));
        playing.map_or_else(Vec::new, |driver| driver.player.clips_in_use().collect())
    }

    /// Has `content` hold what the graphs name now, with what their players still read.
    fn hold_graph_clips(&self, content: &mut Content) -> Result<(), sapphire_assets::LoadError> {
        let character = self.clips_in_use(Some(self.character));
        let enemy = self.clips_in_use(self.enemy.map(|(warrior, _)| warrior));
        content.hold_graph_clips((&character, &enemy))
    }

    /// Takes in a new version of the character, the graph or a clip it names. A new character's joints
    /// are others, so its driver starts from where the old one was, once it can: its parts and its clips
    /// can land frames apart, so until all of them fit it keeps what it has, tries again as each lands,
    /// and hands its old pose no binding to other joints. Otherwise a new graph or clip binds now and
    /// takes effect at the next state's start ([`GraphPlayer::replace`]). What doesn't bind leaves
    /// what's playing alone until something else lands.
    ///
    /// [`GraphPlayer::replace`]: sapphire_anim::GraphPlayer::replace
    fn replace_graph(&mut self, content: &mut Content, character_changed: bool, graph_changed: bool) {
        if let Err(error) = self.hold_graph_clips(content) {
            tracing::warn!(target: LOG_TARGET, %error, "the character's graph lost a clip");
        }
        let character = self.character;
        let ground = self.ik_ground.clone();
        let start_over = character_changed || self.character_waits;
        let Some(mut playing) = self.ecs.world_mut().get_mut::<GraphDriver>(character) else { return };
        if start_over {
            match lookdev::graph_driver(content) {
                Ok(mut driver) => {
                    self.character_waits = false;
                    driver.continue_from(&playing);
                    if let (Some(ground), Some(controls)) = (ground, playing.ik_mut().map(|ik| ik.controls()))
                    {
                        match lookdev::ik_rig(content, driver.player.pose()) {
                            Ok(rig) => driver = driver.with_ik(IkDriver::new(rig, ground, controls)),
                            Err(error) => {
                                tracing::warn!(target: LOG_TARGET, error, "the look-dev character's new rig has no IK");
                            }
                        }
                    }
                    *playing = driver;
                }
                Err(error) => {
                    self.character_waits = true;
                    tracing::warn!(target: LOG_TARGET, error, "the character keeps its pose until the rest of it lands");
                }
            }
        } else {
            match lookdev::graph_binding(content) {
                Ok(binding) => playing.player.replace(Arc::new(binding)),
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, error, "the character keeps the graph it has")
                }
            }
        }
        // A new character is the player's alone, and may have come in the same frame as a new graph. An
        // enemy waiting for the rest of it has the joints it had, which a new binding's aren't.
        if !self.fighters_posed || !graph_changed || self.enemy_waits {
            return;
        }
        let Some(enemy) = self.enemy.map(|(warrior, _)| warrior) else { return };
        let Some(mut playing) = self.ecs.world_mut().get_mut::<GraphDriver>(enemy) else { return };
        match content.enemy_binding() {
            Ok(binding) => playing.player.replace(Arc::new(binding)),
            Err(error) => tracing::warn!(target: LOG_TARGET, %error, "the enemy keeps the graph it has"),
        }
    }

    /// Takes in a new version of the enemy's mesh or skeleton, whose joints may be others: the pose it
    /// holds, the hand its blade follows, what lights them, and in a fight a driver that starts now from
    /// where the old one was, as the character's does ([`Self::replace_graph`]). Those can land frames
    /// apart and its clips later still, so until all of it fits it keeps everything it has, and tries again
    /// as each part lands.
    fn replace_enemy(&mut self, content: &mut Content) {
        let Some((warrior, blade)) = self.enemy else { return };
        if let Err(error) = self.hold_graph_clips(content) {
            tracing::warn!(target: LOG_TARGET, %error, "the enemy's graph lost a clip");
        }
        let ground = self.arena.as_ref().filter(|_| self.fighters_posed).map(|arena| arena.ground());
        let fitted = content.held_enemy().and_then(|held| {
            let driver = ground.map(|ground| content.enemy_driver(ground)).transpose()?;
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
        let world = self.ecs.world_mut();
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

    /// Evaluates every character playing a clip, one job each, for a frame `alpha` of the way from
    /// the previous fixed step to the latest: at the position their own clock's presented time gives, so
    /// a clock that stands still costs nothing and holds the pose. Results reach this frame's packet
    /// through what extraction reads: each [`RenderPose`], [`RenderTransform`], [`Figure`] and light
    /// rig (see [`crate::animate`]). Call it just before [`Self::frame_packet`]. A scene that plays
    /// no clip returns at once. It runs here rather than as a `Presentation` system, which would cost
    /// allocations and a worker's wake-up per frame (the module docs).
    pub fn animate(&mut self, content: &Content, alpha: f32) {
        if self.animating {
            self.animate_clip(content.clip(), alpha);
        }
        if self.graph_show.is_some() {
            self.animate_graph(&content.graph_clips(), alpha);
        }
        if self.fighters_posed {
            self.animate_fighters(&content.graph_clips(), &content.enemy_graph_clips(), alpha);
        }
    }

    /// Plays each fighter's graph for this frame, from what the last step said of it, solves its feet
    /// and its look at the other, and moves the enemy's blade to its hand. Two run in turn on this
    /// thread, so there's no list of jobs to make, and this allocates nothing; a crowd would want
    /// `evaluate_all`'s jobs and a retained list. A fighter whose mesh is changing keeps the pose it has
    /// until it fits.
    fn animate_fighters(&mut self, player: &ClipTable<'_>, enemy: &ClipTable<'_>, alpha: f32) {
        profiling::scope!("animate fighters");
        let started = Stopwatch::start();
        let mut stats = AnimStats::default();
        let Some(arena) = &mut self.arena else { return };
        let world = self.ecs.world_mut();
        let camera = world.resource::<OrbitCamera>().camera_data(self.aspect);
        let view = SpringView::new(&camera, self.spring_quality);
        let entities = [arena.fighters()[0].0, arena.fighters()[1].0];
        for (index, (entity, clips)) in entities.into_iter().zip([player, enemy]).enumerate() {
            let Ok((mut driver, render, figure, rig, _)) = self.graph_players.get_mut(world, entity) else {
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
                let look_target = arena.look_target(index, alpha);
                ik.set_controls(IkControls { look_target, ..ik.controls() });
            }
            let springs_run = driver.springs_run(&view, &RenderTransform(place), &figure);
            let mut job = GraphEvaluation {
                driver,
                render,
                figure,
                rig,
                input,
                world_from_model: Affine3A::from_mat4(place),
                place,
                springs_run,
                evaluated: false,
                spring_work: SpringWork::default(),
                spring_time: Duration::ZERO,
            };
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
        self.anim_stats = stats;
    }

    /// Plays the show's frame on the character. Allocates nothing: the show has one character, so
    /// this frame has no list of jobs to make.
    fn animate_graph(&mut self, clips: &ClipTable<'_>, alpha: f32) {
        profiling::scope!("animate graph");
        let started = Stopwatch::start();
        let mut stats = AnimStats::default();
        let Some(show) = &mut self.graph_show else { return };
        let input = show.frame(alpha);
        let place = show.transform(alpha);
        let travel = show.travel(alpha);
        let world = self.ecs.world_mut();
        let camera = world.resource::<OrbitCamera>().camera_data(self.aspect);
        let view = SpringView::new(&camera, self.spring_quality);
        if let Ok((mut driver, render, figure, rig, mut transform)) =
            self.graph_players.get_mut(world, self.character)
        {
            stats.characters = 1;
            transform.set_if_neq(RenderTransform(driver.base() * place));
            if driver.fits(&render) {
                if let (Some(ik), Some((look, height))) = (driver.ik_mut(), self.ik_look) {
                    let target = Some(look.at(input.ticks, height));
                    ik.set_controls(IkControls { look_target: target, ..ik.controls() });
                }
                let springs_run = driver.springs_run(&view, &transform, &figure);
                let mut job = GraphEvaluation {
                    driver,
                    render,
                    figure,
                    rig,
                    input,
                    world_from_model: Affine3A::from_translation(travel) * Affine3A::from_mat4(transform.0),
                    place: transform.0,
                    springs_run,
                    evaluated: false,
                    spring_work: SpringWork::default(),
                    spring_time: Duration::ZERO,
                };
                job.run(clips);
                stats.teleports = job.driver.player.teleports();
                if job.evaluated {
                    stats.evaluated = 1;
                    stats.joints = job.driver.player.pose().joint_count() as u32;
                    stats.ik = u32::from(job.driver.has_ik());
                }
                stats.springs = u32::from(job.spring_work.wrote_pose);
                stats.spring_substeps = job.spring_work.substeps;
                stats.spring_time = job.spring_time;
            }
        }
        stats.time = started.elapsed();
        self.anim_stats = stats;
    }

    /// [`Self::animate`], given the clip.
    #[cfg(test)]
    fn animate_if_playing(&mut self, clip: Option<(&ArchivedClip, u32)>, alpha: f32) {
        if self.animating {
            self.animate_clip(clip, alpha);
        }
    }

    /// [`Self::animate`], with its clip and the clip's version.
    fn animate_clip(&mut self, clip: Option<(&ArchivedClip, u32)>, alpha: f32) {
        profiling::scope!("animate");
        let started = Stopwatch::start();
        let mut stats = AnimStats::default();
        if let Some((clip, version)) = clip {
            let mut jobs = Vec::new();
            let world = self.ecs.world_mut();
            let camera = world.resource::<OrbitCamera>().camera_data(self.aspect);
            let view = SpringView::new(&camera, self.spring_quality);
            for (mut player, clock, render, figure, rig, transform) in self.players.iter_mut(world) {
                stats.characters += 1;
                stats.teleports += player.teleports();
                if !player.fits(clip, &render) {
                    continue;
                }
                let ticks = clock.0.presented_ticks(alpha);
                if let (Some(ik), Some((look, height))) = (player.ik_mut(), self.ik_look) {
                    let target = Some(look.at(ticks, height));
                    ik.set_controls(IkControls { look_target: target, ..ik.controls() });
                }
                let position = player.next_position(clip, version, ticks, transform.0);
                let springs_run = player.springs_run(&view, &transform, &figure);
                if position.is_none() && !(springs_run && player.springs_due(ticks, &transform)) {
                    continue;
                }
                if position.is_some() {
                    stats.evaluated += 1;
                    stats.joints += player.joint_count() as u32;
                    stats.ik += u32::from(player.has_ik());
                }
                jobs.push(Evaluation {
                    player,
                    render,
                    figure,
                    rig,
                    transform,
                    position,
                    ticks,
                    springs_run,
                    spring_work: SpringWork::default(),
                    spring_time: Duration::ZERO,
                });
            }
            evaluate_all(&mut jobs, |job| job.run(clip));
            for job in &jobs {
                stats.springs += u32::from(job.spring_work.wrote_pose);
                stats.spring_substeps += job.spring_work.substeps;
                stats.spring_time += job.spring_time;
            }
        }
        stats.time = started.elapsed();
        self.anim_stats = stats;
    }

    /// The character's graph player, for hot reload's tests.
    #[cfg(all(test, feature = "dev"))]
    pub fn graph_player(&self) -> Option<&sapphire_anim::GraphPlayer> {
        self.ecs.world().get::<GraphDriver>(self.character).map(|driver| &driver.player)
    }

    /// The arena's actors, for hot reload's tests.
    #[cfg(all(test, feature = "dev"))]
    pub fn arena_mut(&mut self) -> Option<&mut ArenaActors> {
        self.arena.as_deref_mut()
    }

    /// Draws the enemy with this pose, for hot reload's tests: as a mesh that landed before its driver.
    #[cfg(all(test, feature = "dev"))]
    pub fn set_enemy_pose(&mut self, pose: RenderPose) {
        let (warrior, _) = self.enemy.expect("an enemy");
        self.ecs.world_mut().entity_mut(warrior).insert(pose);
    }

    /// The enemy's graph player, for hot reload's tests.
    #[cfg(all(test, feature = "dev"))]
    pub fn enemy_graph_player(&self) -> Option<&sapphire_anim::GraphPlayer> {
        let (warrior, _) = self.enemy?;
        self.ecs.world().get::<GraphDriver>(warrior).map(|driver| &driver.player)
    }

    /// What animation did in the last frame.
    pub fn anim_stats(&self) -> AnimStats {
        self.anim_stats
    }

    /// How long the last frame's camera took (rule R9): nothing without the arena.
    pub fn camera_time(&self) -> Duration {
        self.camera_time
    }

    /// Runs spring bones as a quality tier does, from the next frame on.
    pub fn set_spring_quality(&mut self, quality: SpringQuality) {
        self.spring_quality = quality;
    }

    /// Holds the character in `pose`, making `figure`: `Extract` sends it if it differs from the
    /// one the character has.
    fn hold_character(&mut self, pose: RenderPose, figure: Figure) {
        self.figure = figure;
        let mut character = self.ecs.world_mut().entity_mut(self.character);
        if let Some(mut held) = character.get_mut::<RenderPose>() {
            held.set_if_neq(pose);
        }
        if let Some(mut rig) = character.get_mut::<CharacterLightRig>() {
            rig.head = figure.head;
        }
    }

    /// Makes `Extract` write poses from now on, once: scenes without any pose don't run it.
    fn extract_poses(&mut self) {
        if !self.poses {
            self.ecs.add_systems(Extract, extract::extract_poses.after(extract::extract_lights));
            self.poses = true;
        }
    }

    /// Builds the look-dev stage (`--look-dev`, [`crate::lookdev`]): the town, its plants and its
    /// far mountains around the character, its fog cards and lamps, the character held in
    /// `look.pose` (its turntable stopped, its light rig finding its head and body by
    /// `look.figure`) and the `shot`'s camera. Time of day runs on the stage's own day
    /// ([`lookdev::day`]), characters are lit by the stage's light rigs
    /// ([`lookdev::light_rigs`], M1 C8) and the wind blows the stage's breeze. The town, its
    /// plants, mountains and cards never move, so they are extracted once.
    pub fn spawn_look_dev(&mut self, shot: Shot, look: LookDev) {
        self.ecs.world_mut().insert_resource(shot.camera());
        self.spawn_town(shot.stand(), look);
    }

    /// Builds the arena (`--arena`, [`crate::arena`]): the look-dev stage's town, day and light rigs
    /// where the town stands, with the character at the player's spawn point and the enemy
    /// ([`crate::enemy`]) and its blade at the enemy's. Unlike the look-dev stage, the town stays still
    /// and they move, so none of them has a turntable: [`Self::follow_simulation`] carries each actor's
    /// root here.
    pub fn spawn_arena(&mut self, arena: Arena) {
        let Arena { enemy, held, shot, look, ground } = arena;
        self.ecs
            .world_mut()
            .insert_resource(shot.camera(sapphire_gameplay::arena::spawns().player.position()));
        self.spawn_town(Vec3::ZERO, look);
        self.ecs.world_mut().entity_mut(self.character).remove::<Turntable>();
        let (blade_in_model, hand) = (held.blade, held.hand);
        self.spawn_enemy(enemy, held);
        let (warrior, blade) = self.enemy.expect("it just stood there");
        let enemy = ArenaEnemy { warrior, blade, blade_in_model, hand };
        self.arena = Some(Box::new(ArenaActors::new(self.character, enemy, shot, ground)));
    }

    /// Poses the arena's fighters by these graphs from now on, from what the simulation says of them
    /// ([`crate::arena`]). They hold what they have until a fight steps. Nothing without `--arena`.
    pub fn play_fighters(&mut self, player: GraphDriver, enemy: GraphDriver) {
        let Some(arena) = &self.arena else { return };
        let [(player_entity, _), (enemy_entity, _)] = arena.fighters();
        let figure = self.figure;
        let world = self.ecs.world_mut();
        world.entity_mut(player_entity).insert((player, figure));
        world.entity_mut(enemy_entity).insert(enemy);
        self.fighters_posed = true;
    }

    /// Poses the character as [`Self::play_fighters`] does, with no arena to fight in, for tests.
    #[cfg(all(test, feature = "dev"))]
    pub fn pose_as_fighter(&mut self, driver: GraphDriver) {
        let figure = self.figure;
        self.ecs.world_mut().entity_mut(self.character).insert((driver, figure));
        self.fighters_posed = true;
    }

    /// Takes the arena's actors from `simulation`, after a step of it: nothing without `--arena`.
    /// `cast` is its scenario's, the player first. Whatever the stage, the scene's world clock takes up
    /// the fight's slow motion, as the simulation's does from its next step.
    pub fn follow_simulation(&mut self, simulation: &Simulation, cast: &[sapphire_gameplay::ActorId]) {
        self.follow_slow_motion(simulation);
        if let Some(arena) = &mut self.arena {
            let fought = arena.fight_time();
            let came_in = arena.follow(simulation, cast);
            if let Some(hud) = &mut self.hud {
                hud.fighters_came_in(came_in);
            }
            // A clock that went back is a new fight, and everything stands somewhere else: the frame
            // draws nothing of the last one. A party's swap needs none of this, since who comes in
            // stands where the one who left stood, and the camera goes on.
            if arena.fight_time() < fought {
                self.view_cut = true;
                if let Some(cues) = &mut self.cues {
                    cues.start_over(&mut self.ecs.world_mut().resource_mut::<EffectStack>());
                }
                if let Some(hud) = &mut self.hud {
                    hud.start_over();
                }
            }
        }
        if let Some(mesh) = self.party_looks.as_mut().and_then(|drawn| drawn.follow(simulation.party())) {
            self.ecs.world_mut().entity_mut(self.character).insert(RenderMesh(mesh));
        }
    }

    /// Keeps the scene's world clock with a fight's slow motion (M2 G4), the only scale a simulation puts
    /// on its world, so what the scene's clock times (screen effects, time of day, the wind and everything
    /// else on `FramePacket::time_seconds`) stays with the fighters and the effects on theirs.
    fn follow_slow_motion(&mut self, simulation: &Simulation) {
        let clock = &simulation.world().resource::<WorldClock>().0;
        let slowed = clock.scales().find(|scale| scale.source == ScaleSource::PerfectDodge);
        let slowed = slowed.map(|scale| scale.per_mille);
        if slowed == self.fight_slow_motion.map(|(per_mille, _)| per_mille) {
            return;
        }
        let mut clock = self.ecs.world_mut().resource_mut::<WorldClock>();
        if let Some((_, handle)) = self.fight_slow_motion.take() {
            clock.0.pop_scale(handle);
        }
        if let Some(per_mille) = slowed {
            match clock.0.push_scale(ScaleSource::PerfectDodge, per_mille) {
                Ok(handle) => self.fight_slow_motion = Some((per_mille, handle)),
                Err(error) => {
                    tracing::warn!(target: LOG_TARGET, %error, "the scene misses a fight's slow motion")
                }
            }
        }
    }

    /// Gives the character each party member's mesh (`--party`): at startup, and after GPU device loss or
    /// a tier change, when whoever's on the field stays there.
    pub fn set_party_meshes(&mut self, meshes: [MeshHandle; sapphire_gameplay::PARTY_SLOTS]) {
        let drawn =
            self.party_looks.map_or_else(|| Drawn::new(meshes), |drawn| drawn.on_new_renderer(meshes));
        self.party_looks = Some(drawn);
        self.ecs.world_mut().entity_mut(self.character).insert(RenderMesh(drawn.mesh()));
    }

    /// The view's share of a windowed frame, `dt` real seconds after the last: `input`'s orbit and zoom,
    /// on the real clock so the view stays responsive in slow motion. A pause holds a fight's view, as the
    /// director's frame asks, shakes and all: only the sandbox's own orbit still moves, for looking
    /// around at what's held, and the HUD says it's waiting. Returns the seconds the view moved by, for
    /// [`Self::end_cue_frame`].
    pub fn drive_view(&mut self, input: &SandboxInput, dt: f32) -> f32 {
        let view_dt = if input.is_paused() { 0.0 } else { dt };
        if let Some(hud) = &mut self.hud {
            hud.show_pause(input.is_paused());
        }
        let mut camera = self.camera_mut();
        input.drive_camera(&mut camera);
        camera.update(dt);
        let (look, zoom) = input.camera_motion();
        self.drive_arena_camera(view_dt, look, zoom);
        view_dt
    }

    /// Gives the arena's camera director this much real time and input for its next frame, if one's
    /// on, and says whether it took them.
    pub fn drive_arena_camera(&mut self, dt: f32, look: sapphire_math::Vec2, zoom: f32) -> bool {
        self.arena.as_mut().is_some_and(|arena| arena.drive_camera(dt, look, zoom))
    }

    /// Where the view looks across the ground, as an actor's yaw: what the next tick's stick turns by.
    pub fn camera_heading(&self) -> f32 {
        self.arena.as_deref().and_then(ArenaActors::heading).unwrap_or_else(|| self.camera().heading())
    }

    /// The town, its plants and its far mountains around the character, its fog cards and lamps, on
    /// the stage's day under its light rigs, the character held in `look.pose`. It's drawn moved by
    /// `-stand`, so the character, at the origin, stands at `stand` in it.
    fn spawn_town(&mut self, stand: Vec3, look: LookDev) {
        self.figure = look.figure;
        self.extract_poses();
        self.time_of_day_mut().set_timeline(lookdev::day()).expect("the look-dev day sets every group");
        self.wind = lookdev::WIND;
        let world = self.ecs.world_mut();
        world.insert_resource(lookdev::light_rigs());
        let mut character = world.entity_mut(self.character);
        character.insert(look.pose);
        if let Some(mut turntable) = character.get_mut::<Turntable>() {
            turntable.speed = 0.0;
        }
        if let Some(mut rig) = character.get_mut::<CharacterLightRig>() {
            rig.head = look.figure.head;
        }
        // The town moves so the character, at the origin, stands where the shot has it.
        let town = Mat4::from_translation(-stand);
        self.town = Some(world.spawn((RenderMesh(look.town.town), RenderTransform(town))).id());
        self.town_plants = lookdev::plantings()
            .map(|(plant, place)| {
                (world.spawn((RenderMesh(look.town.plant(plant)), RenderTransform(town * place))).id(), plant)
            })
            .collect();
        self.town_mountains = look
            .town
            .mountains
            .iter()
            .map(|&mesh| world.spawn((RenderMesh(mesh), RenderTransform(town))).id())
            .collect();
        for mut light in lookdev::lamps() {
            light.position -= stand;
            world.spawn((Lamp { intensity: light.intensity }, RenderLight(light)));
        }
        if let Some(meshes) = look.fog_cards {
            self.fog_cards = environment::place_cards(&lookdev::CARDS)
                .map(|(mesh, place)| {
                    (world.spawn((RenderMesh(meshes[mesh]), RenderTransform(town * place))).id(), mesh)
                })
                .collect();
        }
    }

    /// Sets the meshes the look-dev town, its plants and its mountains draw again: after GPU
    /// device loss.
    pub fn set_town_mesh(&mut self, meshes: TownMeshes) {
        if let Some(town) = self.town {
            self.ecs.world_mut().entity_mut(town).insert(RenderMesh(meshes.town));
        }
        for &(entity, plant) in &self.town_plants {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(meshes.plant(plant)));
        }
        for (&entity, &mesh) in self.town_mountains.iter().zip(&meshes.mountains) {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(mesh));
        }
    }

    /// Stands the enemy on the look-dev stage (`--look-dev enemy`, [`crate::enemy`]): the warrior
    /// held in its pose beside the character, and its blade at its hand. Neither moves, so they are
    /// extracted once. Call after [`Self::spawn_look_dev`], whose light rigs they take.
    pub fn spawn_enemy(&mut self, meshes: EnemyMeshes, held: enemy::Held) {
        self.extract_poses();
        let place = enemy::place();
        let rig = LightRigId::DEFAULT;
        let blade_figure = enemy::blade_figure(&held);
        let world = self.ecs.world_mut();
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

    /// Puts the player's blade in the character's right hand (`--blade on`, M2 O3): `held_hand` is where
    /// the character's held pose has that hand, for when no clip plays. It follows the hand each frame
    /// ([`Self::follow_hand`]).
    pub fn spawn_blade(&mut self, mesh: MeshHandle, held_hand: Mat4) {
        let rig = LightRigId::DEFAULT;
        let figure = self.figure.of_item_at(held_hand * blade::socket());
        let entity = self
            .ecs
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
    pub fn set_blade_mesh(&mut self, mesh: MeshHandle) {
        if let Some(held) = self.blade {
            self.ecs.world_mut().entity_mut(held.entity).insert(RenderMesh(mesh));
        }
    }

    /// Moves the blade to where the character's hand is this frame: after whatever poses and places
    /// the character, and before extraction.
    fn follow_hand(&mut self) {
        let Some(held) = self.blade else { return };
        let world = self.ecs.world_mut();
        let character = world.entity(self.character);
        let place = character.get::<RenderTransform>().map_or(Mat4::IDENTITY, |transform| transform.0);
        let hand = character
            .get::<ClipPlayer>()
            .and_then(ClipPlayer::right_hand)
            .or_else(|| character.get::<GraphDriver>().and_then(GraphDriver::right_hand))
            .unwrap_or(held.held_hand);
        let holder = character.get::<Figure>().copied().unwrap_or(self.figure);
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

    /// Sets the meshes the enemy draws again: after GPU device loss.
    pub fn set_enemy_meshes(&mut self, meshes: EnemyMeshes) {
        if let Some((warrior, blade)) = self.enemy {
            self.ecs.world_mut().entity_mut(warrior).insert(RenderMesh(meshes.warrior));
            self.ecs.world_mut().entity_mut(blade).insert(RenderMesh(meshes.blade));
        }
    }

    /// Places the outline row (`--outline-row`) behind the character, drawing `mesh` (the
    /// character's), each a character lit by the default light rig, and lowers the camera to
    /// see them all. They never move, so they are extracted once.
    pub fn spawn_outline_row(&mut self, mesh: MeshHandle) {
        let places = outlines::row(self.bounds);
        let world = self.ecs.world_mut();
        world.insert_resource(outlines::row_camera(self.bounds));
        let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: self.figure.head };
        self.outline_row =
            Some(places.map(|place| world.spawn((RenderMesh(mesh), RenderTransform(place), rig)).id()));
    }

    /// Sets the mesh the outline row draws again: after GPU device loss.
    pub fn set_outline_row_mesh(&mut self, mesh: MeshHandle) {
        for entity in self.outline_row.into_iter().flatten() {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(mesh));
        }
    }

    /// Places the hard-edged outline sample (`--hard-edges`) well beside the character, drawing
    /// `mesh`, and frames it with the camera instead of the character.
    pub fn spawn_hard_edges(&mut self, mesh: MeshHandle) {
        let world = self.ecs.world_mut();
        world.insert_resource(outlines::hard_edges_camera(self.bounds));
        let place = RenderTransform(outlines::hard_edges(self.bounds));
        self.hard_edges = Some(world.spawn((RenderMesh(mesh), place)).id());
    }

    /// Sets the mesh the hard-edged sample draws again: after GPU device loss.
    pub fn set_hard_edges_mesh(&mut self, mesh: MeshHandle) {
        if let Some(entity) = self.hard_edges {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(mesh));
        }
    }

    /// Applies input queued since the last fixed step. Call before the frame's steps.
    pub fn apply(&mut self, actions: SimActions) {
        let world = self.ecs.world_mut();
        if actions.toggle_slow_motion {
            world.resource_scope(|world, mut slow_motion: Mut<'_, SlowMotion>| {
                let mut clock = world.resource_mut::<WorldClock>();
                match slow_motion.0.take() {
                    Some(handle) => {
                        clock.0.pop_scale(handle);
                    }
                    None => slow_motion.0 = clock.0.push_scale(ScaleSource::PerfectDodge, SLOW_MOTION).ok(),
                }
                tracing::info!(target: LOG_TARGET, world_scale = clock.0.effective_scale(), "slow motion toggled");
            });
        }
        if actions.hitstop
            && let Some(mut clock) = world.get_mut::<EntityClock>(self.character)
        {
            clock.0.apply_hitstop(HITSTOP_TICKS);
            tracing::debug!(target: LOG_TARGET, ticks = HITSTOP_TICKS, "hitstop");
        }
    }

    /// One fixed simulation step. Must not allocate (rule R6): debug draws inside it are held for
    /// the step (`debug_draw::begin_step`), and allocate nothing on this thread, which turned
    /// drawing on.
    pub fn step(&mut self) {
        debug_draw::begin_step();
        self.ecs.fixed_step();
        if let Some(demo) = self.debug_draw_demo {
            demo.draw_in_step();
        }
        if self.vfx.is_some() {
            self.step_vfx();
        }
        if let Some(show) = &mut self.graph_show {
            show.step();
        }
        debug_draw::end_step();
    }

    /// Runs the schedules of a fixed step once, when everything's spawned and before the first step
    /// anything holds to rule R6. A query learns of the archetypes made since it last ran when it next
    /// runs, and its lists can grow there: a town's and a fight's spawns are enough to, which made the
    /// first step of every debug build's window fail its count. It costs the clocks one tick and
    /// plays no show, and captures don't call it, so their frames stay as they were.
    pub fn warm_fixed_step(&mut self) {
        self.ecs.fixed_step();
    }

    /// Turns debug draw on and draws `demo` from now on (`--debug-draw`).
    pub fn set_debug_draw_demo(&mut self, demo: DebugDrawDemo) {
        debug_draw::set_enabled(true);
        self.debug_draw_demo = Some(demo);
        demo.draw_once();
    }

    /// Plays `show` within `budget` from now on (`--vfx`, M2 E2).
    pub fn play_vfx(&mut self, show: VfxShow, budget: VfxBudget) {
        self.vfx = Some(VfxPlayer::new(show, budget));
    }

    /// Fires a fight's cues from now on (`--cues`, M2 E3), within `budget`, and has the scene prewarm
    /// what they draw through. An error names what its table can't find.
    pub fn play_cues(&mut self, budget: VfxBudget) -> Result<(), String> {
        let presets: &EffectLibrary = self.ecs.world().resource();
        let show = CueShow::new(budget, presets).map_err(|error| error.to_string())?;
        self.pushed_effects |= show.post_variants(presets);
        self.cues = Some(show);
        Ok(())
    }

    /// What's firing a fight's cues, if anything is.
    pub fn cue_show(&self) -> Option<&CueShow> {
        self.cues.as_ref()
    }

    /// Fires what `sim`'s latest step published (`--cues`), and does nothing without that flag. Call it
    /// after each of the simulation's steps: the next one clears them. Allocates nothing once warmed.
    pub fn show_cues(&mut self, sim: &Simulation) {
        let Some(cues) = &mut self.cues else { return };
        let world = self.ecs.world();
        let camera = world.resource::<OrbitCamera>().camera_data(self.view_aspect);
        let view_projection = camera.proj * camera.view;
        let place = world.get::<RenderTransform>(self.character);
        let stand = place.map_or(Vec3::ZERO, |place| place.0.w_axis.truncate());
        let stage = match &mut self.arena {
            Some(arena) => CueStage::Actors { director: arena.director() },
            None => CueStage::OneCharacter { stand },
        };
        let (mut stack, presets) = self
            .screen_effects
            .get_mut(self.ecs.world_mut())
            .expect("the scene inserts the effect stack and its presets");
        cues.step(sim, &mut stack, &presets, view_projection, stage);
    }

    /// What a fight's cues ask of the audio system this frame.
    pub fn cue_sounds(&self) -> CueSounds<'_> {
        self.cues.as_ref().map_or(CueSounds::NONE, CueShow::sounds)
    }

    /// Ends a frame's steps of a fight's cues, `dt` real seconds after the last frame's, for a frame drawn
    /// at `alpha`, after the audio system took [`Self::cue_sounds`]: tells `rumble` what changed, and
    /// shakes the camera for the frame to come by `view_dt`, what [`Self::drive_view`] gave the view.
    pub fn end_cue_frame(&mut self, dt: f32, view_dt: f32, alpha: f32, rumble: &mut dyn RumbleSink) {
        let Some(cues) = &mut self.cues else { return };
        let (turn, offset) = cues.end_frame(dt, view_dt, alpha, rumble);
        // With a director on, its own frame has the shakes, and a turn its roll.
        if self.arena.as_mut().is_none_or(|arena| arena.director().is_none()) {
            self.ecs.world_mut().resource_mut::<OrbitCamera>().set_shake(turn, offset);
        }
    }

    /// Stops a fight's rumble at once: the app lost focus or the foreground.
    pub fn stop_rumble(&mut self, rumble: &mut dyn RumbleSink) {
        if let Some(cues) = &mut self.cues {
            cues.stop_rumble(rumble);
        }
    }

    /// One step of the effects, by the clocks' last steps: the world's, and the character's for the
    /// attacker's and the victim's, which stops through its hitstop. A hit starts one on it.
    fn step_vfx(&mut self) {
        let world = self.ecs.world();
        let world_step = world.resource::<WorldClock>().0.last_delta().as_secs_f32();
        let clock = world.get::<EntityClock>(self.character);
        let character_step = clock.map_or(world_step, |clock| clock.0.last_delta().as_secs_f32());
        let Some(player) = self.vfx.as_mut() else { return };
        let hitstop = player.step(world_step, character_step);
        if let Some(ticks) = hitstop
            && let Some(mut clock) = self.ecs.world_mut().get_mut::<EntityClock>(self.character)
        {
            clock.0.apply_hitstop(ticks);
        }
    }

    /// Readies `renderer` for what this scene may draw: the post variants of its screen effects,
    /// and, if it plays effects, their pipelines, so neither compiles on the frame it first shows.
    /// The app calls it whenever it makes a renderer.
    pub fn prewarm(&self, renderer: &mut Renderer) {
        renderer.prewarm_post(self.effect_variants());
        if self.vfx.is_some() || self.cues.is_some() {
            renderer.prewarm_vfx();
        }
    }

    /// Runs presentation and extraction, then returns this frame's packet, with the main view
    /// drawing into a target of `target_size`. Call it every frame: on frames that draw nothing,
    /// pass `None`; their packets have no view but still carry instance deltas, which the renderer
    /// must see.
    ///
    /// `alpha` means one thing (review WA-3): `alpha` of the way from the previous fixed step to the
    /// latest, for transforms, poses and clocks alike (time of day, cutscenes, screen effects, effects'
    /// particles and trails, `FramePacket::time_seconds`). So an effect stays with the pose it accents,
    /// and shown time never steps back on entering pause or slow motion.
    pub fn frame_packet(&mut self, alpha: f32, target_size: Option<(u32, u32)>) -> &FramePacket {
        self.aspect = drawable_aspect(target_size).unwrap_or(self.aspect);
        self.ecs.presentation(alpha);
        if let Some(arena) = &mut self.arena {
            let started = Stopwatch::start();
            arena.present(self.ecs.world_mut(), alpha, drawable_aspect(target_size));
            self.camera_time = started.elapsed();
        }
        self.follow_hand();
        let (sky_material, cloud_cover, plain_sky) = (self.sky_material, self.cloud_cover, self.plain_sky);
        let fog = self.fog;
        let (plain_post, sharpening, wind) = (self.plain_post, self.sharpening, self.wind);
        let probe_hours = &self.probe_hours;
        let (vfx, cues) = (&mut self.vfx, &mut self.cues);
        let view_cut = &mut self.view_cut;
        if let Some(aspect) = drawable_aspect(target_size) {
            self.view_aspect = aspect;
        }
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.begin();
            if let Some(vfx) = vfx {
                vfx.write(&mut packet.vfx, alpha);
            }
            if let Some(cues) = cues {
                cues.write(&mut packet.vfx, alpha);
            }
            let camera = world.resource::<OrbitCamera>();
            if let Some(aspect) = drawable_aspect(target_size) {
                let cut = std::mem::take(view_cut);
                packet.views.push(ViewData { cut, ..ViewData::new(camera.camera_data(aspect)) });
            }
            let probe = camera.eye();
            let clock = &world.resource::<WorldClock>().0;
            // Interpolated like everything else drawn this frame.
            packet.time_seconds = clock.presented_ticks(alpha) / f64::from(TICK_HZ);
            world.resource_mut::<TimeOfDay>().resolve(probe, alpha).write(packet);
            // The probe grid's bounce keys for the hour (M1 W5): none without a grid.
            packet.probe_keys = world.resource::<TimeOfDay>().probe_keys(probe_hours);
            packet.post.filter_strength = crate::cvars::filter_strength();
            packet.post.sharpening = sharpening.unwrap_or_else(crate::cvars::sharpening);
            packet.sky_material = sky_material;
            if let Some(cover) = cloud_cover {
                packet.sky_layers.clouds.coverage = cover;
                packet.sky_layers.clouds.cirrus = cover;
            }
            if plain_sky {
                packet.sky_layers = SkyLayers::default();
            }
            if !fog {
                packet.fog = Fog::default();
            }
            if plain_post {
                packet.post = packet.post.plain();
            }
            packet.wind = wind;
            // The effects gameplay pushed, over whatever post's look is (M1 P3).
            packet.post.effects = world.resource_mut::<EffectStack>().resolve(alpha);
        });
        // Lamps by night, before extraction writes the lights.
        let day_night = self.ecs.world().resource::<TimeOfDay>().frame().look.day_night;
        for (lamp, mut light) in self.lamps.iter_mut(self.ecs.world_mut()) {
            light.0.intensity = lamp.intensity * day_night;
        }
        self.ecs.extract();
        let scene_figure = self.figure;
        let (characters, heads, face_layout, fade) =
            (&mut self.characters, &mut self.heads, self.face_layout, self.fade);
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.packet_mut();
            // The rigs are resolved for the camera; without one nothing draws.
            let Some(camera) = packet.main_view().map(|view| view.camera) else { return };
            let frame = world.resource::<TimeOfDay>().frame();
            let rigs = world.resource::<LightRigLibrary>();
            for (entity, character, transform, key, figure) in characters.iter(world) {
                let figure = figure.copied().unwrap_or(scene_figure);
                let head = transform.0.transform_point3(character.head);
                let mut lighting = frame.character_lighting(rigs.rig(character.rig), &camera, head);
                // A rim rig figure's key light turns to its angles around it, or for a folded
                // key, as far as its rig's limits let it (M1 C8).
                if let Some(key) = key {
                    let direction = -key.toward(camera.position, head);
                    lighting.key.direction = if key.folded {
                        rigs.rig(character.rig).key.direction(direction, &camera, head)
                    } else {
                        direction
                    };
                }
                // The head turns with the model, and by the figure's held pose.
                let pose = HeadBasis {
                    forward: transform.0.transform_vector3(figure.face * Vec3::Z),
                    up: transform.0.transform_vector3(figure.face * Vec3::Y),
                };
                let face = CharacterFace { head: pose, expression: figure.expression };
                let body = CharacterBody::capsule(
                    figure.feet + Vec3::new(0.0, figure.body_radius, 0.0),
                    figure.head,
                    figure.body_radius,
                )
                .transformed(transform.0);
                let opacity = fade.opacity(camera.position, &body);
                packet.characters.push(CharacterInstance {
                    id: extract::instance_id(entity),
                    lighting,
                    face,
                    body,
                    opacity,
                });
            }
            // The face rig's heads, in the default preset turned to each head's angles, or by its
            // GI alone in the GI layouts.
            let preset = rigs.rig(LightRigId::DEFAULT);
            for (entity, rig, transform) in heads.iter(world) {
                let center = transform.0.transform_point3(Vec3::ZERO);
                let lighting = frame.character_lighting(preset, &camera, center);
                let (lighting, face) = faces::character(face_layout, rig, transform.0, lighting);
                let body = CharacterBody::capsule(center, center, HEAD_BODY_RADIUS);
                packet.characters.push(CharacterInstance {
                    id: extract::instance_id(entity),
                    lighting,
                    face,
                    body,
                    opacity: 1.0,
                });
            }
        });
        if let (Some(hud), Some(size)) = (&mut self.hud, target_size) {
            if let Some(arena) = &self.arena
                && let Some(values) = arena.hud_values()
            {
                hud.show_fight(values, arena.fight_time());
            }
            let mut extract = self.ecs.world_mut().resource_mut::<RenderExtract>();
            hud.draw(size, &mut extract.packet_mut().ui);
        }
        // Debug draws, from every thread (M2 T1). Timed ones run on the world clock, which pausing
        // and single-stepping the world holds, so what they mark stays to be looked at.
        profiling::scope!("debug draw");
        if let Some(demo) = self.debug_draw_demo {
            demo.draw_in_frame();
        }
        if let Some(arena) = &self.arena
            && CombatViews::current().colliders
        {
            arena.draw_colliders(&mut Facade);
        }
        let now = self.ecs.world().resource::<WorldClock>().0.now().as_secs_f64();
        let mut extract = self.ecs.world_mut().resource_mut::<RenderExtract>();
        self.debug_draw.collect(now, &mut extract.packet_mut().debug);
        self.ecs.world().resource::<RenderExtract>().packet()
    }

    /// Shows the combat HUD over the scene (`--hud`, M2 I2).
    pub fn set_hud(&mut self, hud: SandboxHud) {
        self.hud = Some(hud);
    }

    /// The HUD, if the scene shows one.
    pub fn hud(&self) -> Option<&SandboxHud> {
        self.hud.as_ref()
    }

    /// A platform event for the HUD's touch zones: whether they took it.
    pub fn hud_event(&mut self, event: &sapphire_platform::Event) -> bool {
        self.hud.as_mut().is_some_and(|hud| hud.handle_event(event))
    }

    /// Whether the HUD's stick or a button holds this finger.
    #[cfg(feature = "dev")]
    pub fn hud_holds_control(&self, finger: sapphire_platform::FingerId) -> bool {
        self.hud.as_ref().is_some_and(|hud| hud.holds_control(finger))
    }

    /// Says whether a thumb is what's playing, for who a fight's soft lock chooses
    /// ([`crate::arena::ArenaActors::set_played_on_touch`]).
    pub fn set_played_on_touch(&mut self, touch: bool) {
        if let Some(arena) = &mut self.arena {
            arena.set_played_on_touch(touch);
        }
    }

    /// The HUD's frame on the real clock, where a window has its metrics: what its touch zones read
    /// goes to `input`, before that judges the frame.
    pub fn update_hud(
        &mut self,
        now: sapphire_platform::Timestamp,
        window: &sapphire_platform::Window,
        input: &mut SandboxInput,
    ) {
        let Some(hud) = &mut self.hud else { return };
        let metrics = hud.metrics_of(window);
        hud.follow_devices(input.desktop(), input.active_gamepad().is_some());
        self.read_touch(now, metrics, input);
    }

    /// What the HUD's touch zones read this frame, on a frame of `metrics`, to `input`.
    pub fn read_touch(
        &mut self,
        now: sapphire_platform::Timestamp,
        metrics: sapphire_ui::UiMetrics,
        input: &mut SandboxInput,
    ) {
        let Some(hud) = &mut self.hud else { return };
        if let Some(touch) = hud.update(now, metrics) {
            input.read_touch(touch, now);
        }
    }

    /// The hits `sim`'s latest step landed, as the HUD's numbers, once: a run that's stopped takes no more
    /// steps, and its last one's cues stand.
    pub fn show_damage(&mut self, sim: &Simulation) {
        let Some(hud) = &mut self.hud else { return };
        if !hud.reads_hits_of(sim.tick()) {
            return;
        }
        let camera = self.ecs.world().resource::<OrbitCamera>().camera_data(self.view_aspect);
        let view_projection = camera.proj * camera.view;
        for cue in sim.cues() {
            let Some(hit) = crate::hud::landed(&cue) else { continue };
            hud.show_hit(hit.damage, hit.point, hit.weight, view_projection);
            // So a run on a device can be read from its log, as the HUD's buttons are.
            tracing::info!(
                target: LOG_TARGET,
                tick = cue.tick,
                attacker = ?cue.actor,
                target_actor = ?hit.target,
                damage = hit.damage,
                outcome = ?hit.outcome,
                "hit landed"
            );
        }
    }

    /// Makes the next packet add every instance again, changed or not, for a renderer that
    /// starts empty (after GPU device loss).
    pub fn resend_instances(&mut self) {
        self.ecs.world_mut().resource_mut::<RenderExtract>().resend_all();
    }

    /// Ends the frame (the ECS `Cleanup` schedule). Call once per frame, after rendering.
    pub fn end_frame(&mut self) {
        self.ecs.cleanup();
    }

    pub fn camera(&self) -> &OrbitCamera {
        self.ecs.world().resource()
    }

    pub fn camera_mut(&mut self) -> Mut<'_, OrbitCamera> {
        self.ecs.world_mut().resource_mut()
    }

    /// What the audio system hears of the scene this frame (M2 E1): the camera, the world's clock,
    /// and where the character stands and whether hitstop holds it.
    pub fn sound(&self) -> SceneSound {
        let world = self.ecs.world();
        let place = world.get::<RenderTransform>(self.character);
        SceneSound {
            listener: world.resource::<OrbitCamera>().listener(),
            world_time_scale: world.resource::<WorldClock>().0.effective_scale(),
            character: place.map_or(Vec3::ZERO, |place| place.0.w_axis.truncate()),
            hitstop: world
                .get::<EntityClock>(self.character)
                .is_some_and(|clock| clock.0.hitstop_ticks() > 0),
        }
    }

    /// The world clock, for lifecycle pauses and time controls.
    pub fn world_clock_mut(&mut self) -> Mut<'_, WorldClock> {
        self.ecs.world_mut().resource_mut()
    }

    #[cfg(feature = "dev")]
    pub fn world_clock(&self) -> &sapphire_time::ScaledClock {
        &self.ecs.world().resource::<WorldClock>().0
    }

    pub fn time_of_day(&self) -> &TimeOfDay {
        self.ecs.world().resource()
    }

    /// The characters of the last packet, with their resolved light rigs.
    #[cfg(feature = "dev")]
    pub fn characters(&self) -> &[CharacterInstance] {
        &self.ecs.world().resource::<RenderExtract>().packet().characters
    }

    /// This frame's packet, for what the overlay shows of it.
    #[cfg(feature = "dev")]
    pub fn packet(&self) -> &FramePacket {
        self.ecs.world().resource::<RenderExtract>().packet()
    }

    pub fn time_of_day_mut(&mut self) -> Mut<'_, TimeOfDay> {
        self.ecs.world_mut().resource_mut()
    }

    /// The day time of day runs on: the look-dev stage's own once the stage is built (it sets
    /// the timeline with the town, [`Scene::spawn_look_dev`]), the default day otherwise.
    #[cfg(any(test, feature = "dev"))]
    pub fn day(&self) -> Day {
        if self.town.is_some() { Day::LookDev } else { Day::Default }
    }

    /// Replaces the day's timeline with `timeline`, read from `origin` (a path or a file name),
    /// if `origin` is the day's own document ([`Day::file_name`]) and the timeline is complete,
    /// and says which day it replaced; otherwise keeps the day. Hot reload and live link both
    /// come through here.
    #[cfg(any(test, feature = "dev"))]
    pub fn replace_day(&mut self, origin: &str, timeline: tod::TodTimeline) -> Result<Day, String> {
        let day = self.day();
        let name = std::path::Path::new(origin).file_name().and_then(std::ffi::OsStr::to_str);
        if name != Some(day.file_name()) {
            return Err(format!(
                "the scene runs on the {} ({}), which only that file replaces",
                day.name(),
                day.file_name()
            ));
        }
        self.time_of_day_mut().set_timeline(timeline).map_err(|error| error.to_string())?;
        Ok(day)
    }

    /// Gives every light rig preset a camera rim (`--camera-rim`, M1 C8) taking `share` of the
    /// rim's gating, from the presets' own direction (behind the character on the camera's right,
    /// and above, unless a preset says otherwise).
    pub fn set_camera_rim(&mut self, share: f32) -> Result<(), String> {
        let library = self.ecs.world().resource::<LightRigLibrary>();
        let rigs = library
            .names()
            .map(|name| {
                let rig = library.rig(library.find(name).expect("a listed preset"));
                let camera_rim = LightRigRim { share, ..rig.camera_rim };
                NamedLightRig { name: name.to_owned(), rig: LightRig { camera_rim, ..*rig } }
            })
            .collect();
        let library = LightRigLibrary::new(rigs).map_err(|error| error.to_string())?;
        self.ecs.world_mut().insert_resource(library);
        Ok(())
    }

    /// Keys characters' shadow sides to keep `shade`'s hue by its share (`--shade-hue`, M1 C8):
    /// sets it in every key of the day the scene runs on.
    pub fn set_shade_hue(&mut self, shade: tod::CharacterShadeKey) -> Result<(), String> {
        let keys = self.time_of_day().timeline().keys().iter();
        let keys = keys.map(|key| tod::TodKey { character_shade: Some(shade), ..key.clone() }).collect();
        let timeline = tod::TodTimeline::new(keys).map_err(|error| error.to_string())?;
        self.time_of_day_mut().set_timeline(timeline).map_err(|error| error.to_string())
    }

    /// Sets the mesh the character draws: at startup, and again after GPU device loss.
    pub fn set_character_mesh(&mut self, mesh: MeshHandle) {
        self.ecs.world_mut().entity_mut(self.character).insert(RenderMesh(mesh));
    }

    /// Sets the mesh the skinned stalk draws (the scene has one with `--skinned`): at startup,
    /// and again after GPU device loss.
    pub fn set_stalk_mesh(&mut self, mesh: MeshHandle) {
        debug_assert!(self.stalk.is_some(), "the scene was made without the skinned stalk");
        if let Some(stalk) = self.stalk {
            self.ecs.world_mut().entity_mut(stalk).insert(RenderMesh(mesh));
        }
    }
}

/// Width over height of a target the main view can draw into: none without a target, or for
/// an empty one (a minimized window).
fn drawable_aspect(target_size: Option<(u32, u32)>) -> Option<f32> {
    let (width, height) = target_size.filter(|&(width, height)| width > 0 && height > 0)?;
    Some(width as f32 / height as f32)
}

/// A mesh's object-space bounds as the scene frames them: `[min, max]`.
pub(crate) fn mesh_bounds(mesh: &ArchivedMesh) -> [[f32; 3]; 2] {
    let bounds = mesh.streams.bounds.to_native();
    [bounds.min, bounds.max]
}

#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod spring_tests;

#[cfg(test)]
mod tests {
    use sapphire_render::InstanceId;

    use super::*;

    const BOUNDS: [[f32; 3]; 2] = [[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]];

    /// The sun the sandbox drew with before time of day.
    const SUN: sapphire_render::DirectionalLight = sapphire_render::DirectionalLight {
        direction: Vec3::new(0.55, -0.7, -0.45),
        color: Vec3::new(1.0, 0.97, 0.92),
        intensity: 1.0,
    };

    fn scene() -> Scene {
        Scene::new(BOUNDS, false)
    }

    /// A scene moves by value through the app's startup on the main thread, which has 1 MiB on iOS,
    /// as a renderer does (`a_renderer_is_small_enough_to_move_on_the_main_thread`). What only some
    /// runs use is boxed: box the next one rather than raise this.
    #[test]
    fn a_scene_is_small_enough_to_move_on_the_main_thread() {
        let size = size_of::<Scene>();
        assert!(size <= 12 * 1024, "a scene takes {size} bytes");
    }

    fn angle(scene: &Scene) -> f32 {
        scene.ecs.world().get::<Turntable>(scene.character).expect("character").angle(1.0)
    }

    #[test]
    fn hitstop_and_slow_motion_scale_the_spin() {
        let mut scene = scene();
        scene.apply(SimActions { hitstop: true, ..SimActions::default() });
        for _ in 0..HITSTOP_TICKS {
            scene.step();
        }
        assert_eq!(angle(&scene), 0.0, "frozen by hitstop");

        scene.apply(SimActions { toggle_slow_motion: true, ..SimActions::default() });
        for _ in 0..TICK_HZ {
            scene.step();
        }
        assert!((angle(&scene) - 0.35 * 0.2).abs() < 1e-4, "one second at 0.2× is 0.2 s of spin");
    }

    /// A show's hit stops the character's clock, as a game's would: its turntable and its effects
    /// hold through the hitstop, and only that long.
    #[test]
    fn a_shows_hit_holds_the_character_and_its_effects() {
        let mut scene = scene();
        scene.play_vfx(VfxShow::Hit, VfxBudget::DESKTOP_LOW);
        let hit = 6;
        for _ in 0..=hit {
            scene.step();
        }
        // The hit's own step, which a frame draws up to the end of; what freezes holds there.
        let on_its_way = scene.frame_packet(0.5, Some((1, 1))).vfx.clone();
        scene.end_frame();
        let before = scene.frame_packet(1.0, Some((1, 1))).vfx.clone();
        scene.end_frame();
        assert!(!before.ribbon_points.is_empty() && !before.sprites.is_empty(), "the trail and the spark");
        assert!(on_its_way.ribbon_points.len() < before.ribbon_points.len(), "the blade's still on its way");
        let spun = angle(&scene);
        // Five of its six ticks, which the world's dust goes on through.
        for _ in 0..5 {
            scene.step();
        }
        let held = scene.frame_packet(0.5, Some((1, 1))).vfx.clone();
        scene.end_frame();
        assert_eq!(angle(&scene), spun, "the turntable");
        assert_eq!(held.ribbon_points, before.ribbon_points, "the trail");
        assert_ne!(held.sprites, before.sprites, "the dust moves on");
        for _ in 0..3 {
            scene.step();
        }
        assert!(angle(&scene) > spun, "and it moves on after them");
        let after = scene.frame_packet(0.5, Some((1, 1))).vfx.clone();
        assert_ne!(after.ribbon_points, before.ribbon_points);
    }

    #[test]
    fn a_shows_steps_do_not_allocate_from_the_first() {
        for show in VfxShow::ALL {
            let mut scene = scene();
            scene.play_vfx(show, VfxBudget::DESKTOP_LOW);
            let ((), steps) = sapphire_core::alloc::count_in_scope(|| {
                for _ in 0..240 {
                    scene.step();
                }
            });
            assert_eq!(steps.allocations, 0, "rule R6: {show:?}");
        }
    }

    #[test]
    fn fixed_steps_do_not_allocate_from_the_first() {
        // As the app runs it: steps from the first frame on, with frames in between.
        let mut scene = Scene::new(BOUNDS, true);
        scene.set_stalk_mesh(MeshHandle::from_raw(1));
        scene.spawn_face_rig(MeshHandle::from_raw(2), FaceLayout::Turn);
        scene.spawn_environment(MeshHandle::from_raw(3));
        scene.spawn_demo_lights();
        let mut allocations = 0;
        for _ in 0..60 {
            let ((), steps) = sapphire_core::alloc::count_in_scope(|| {
                scene.step();
                scene.step();
            });
            allocations += steps.allocations;
            let _ = scene.frame_packet(0.5, Some((1, 1)));
            scene.end_frame();
        }
        assert_eq!(allocations, 0, "rule R6");
    }

    /// A town's and a fight's spawns make more shapes of entity than a query's lists start with. The
    /// first step after them is what `SandboxApp` warms: without it, a debug build's window panicked on
    /// its first frame (found on the iOS simulator, I3), because nothing that counted steps had spawned
    /// this much.
    #[test]
    fn a_window_warms_its_steps_once_everything_is_spawned() {
        #[derive(Component)]
        struct Shape<const N: u8>;
        fn spawn_every_shape(scene: &mut Scene) {
            for bits in 0u8..64 {
                let clock = scene.world_clock_mut().new_entity_clock();
                let mut entity = scene.ecs.world_mut().spawn(clock);
                if bits & 1 != 0 {
                    entity.insert(Shape::<0>);
                }
                if bits & 2 != 0 {
                    entity.insert(Shape::<1>);
                }
                if bits & 4 != 0 {
                    entity.insert(Shape::<2>);
                }
                if bits & 8 != 0 {
                    entity.insert(Shape::<3>);
                }
                if bits & 16 != 0 {
                    entity.insert(Shape::<4>);
                }
                if bits & 32 != 0 {
                    entity.insert(Shape::<5>);
                }
            }
        }

        let mut cold = scene();
        spawn_every_shape(&mut cold);
        let ((), first) = sapphire_core::alloc::count_in_scope(|| cold.step());
        assert!(first.allocations > 0, "the case this exists for has gone: {first:?}");

        let mut warmed = scene();
        spawn_every_shape(&mut warmed);
        warmed.warm_fixed_step();
        let ((), steps) = sapphire_core::alloc::count_in_scope(|| {
            for _ in 0..60 {
                warmed.step();
            }
        });
        assert_eq!(steps.allocations, 0, "rule R6");
    }

    #[test]
    fn screen_effect_controls_before_fixed_steps_do_not_allocate_from_the_first() {
        // As `dev` builds run the steps: the overlay's screen effect controls reach the stack
        // before every one (`DevTools::before_fixed_step`), from the very first, and one fires a
        // preset partway through.
        let mut scene = scene();
        let library = EffectLibrary::default_effects();
        let name = library.names().next().expect("the default presets");
        let fire = library.find(name).expect("a preset of that name");
        let layers = library.preset(fire).expect("a preset").layers.len() as u64;
        let mut allocations = 0;
        for frame in 0..30 {
            let ((), steps) = sapphire_core::alloc::count_in_scope(|| {
                for step in 0..2 {
                    scene.with_screen_effects(|stack, library| {
                        if (frame, step) == (10, 0)
                            && let Some(preset) = library.preset(fire)
                        {
                            stack.push(preset);
                        }
                    });
                    scene.step();
                }
            });
            allocations += steps.allocations;
            let _ = scene.frame_packet(0.5, Some((1, 1)));
            scene.end_frame();
        }
        assert_eq!(scene.with_screen_effects(|stack, _| stack.stats().pushed), layers, "the preset fired");
        assert_eq!(allocations, 0, "rule R6");
    }

    /// The arena's stage with stand-in meshes, framed by `shot`, and the figure it holds the character by.
    pub(super) fn arena_stage(shot: ArenaShot) -> (Scene, Figure) {
        let plain = MeshHandle::from_raw(1);
        let town = TownMeshes {
            town: plain,
            plants: [plain; lookdev::Plant::ALL.len()],
            mountains: [plain; lookdev::MOUNTAINS.len()],
        };
        let figure = Figure { head: Vec3::new(0.0, 1.45, 0.0), ..Figure::MANNEQUIN };
        let pose = RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] };
        let look = LookDev { town, fog_cards: None, pose, figure };
        let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
        let set = sapphire_colliders::ColliderSet::read(&cooked).expect("it reads");
        let ground = ArenaGround::new(set).expect("the stage stands on it");
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let enemy = EnemyMeshes { warrior: MeshHandle::from_raw(2), blade: MeshHandle::from_raw(3) };
        let blade_in_model = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        let held = enemy::Held {
            pose: RenderPose { palette: vec![Default::default(); 2], morph_weights: vec![] },
            figure: Figure { head: Vec3::new(0.0, 2.2, 0.0), ..Figure::MANNEQUIN },
            blade: blade_in_model,
            hand: 0,
        };
        scene.spawn_arena(Arena { enemy, held, shot, look, ground });
        (scene, figure)
    }

    /// A new fight stands everyone somewhere else, so the renderer hears of it once: temporal AA drops
    /// what it drew of the last fight, and velocity counts the camera as unmoved. A swap, which stands
    /// someone new in the same place with the camera going on, isn't a cut.
    #[test]
    fn a_new_fight_cuts_the_view_once_and_a_swap_doesnt() {
        use sapphire_gameplay::replay::{Recording, SCENARIOS, arena_walk, find_scenario, find_script};

        let scenario = find_scenario(SCENARIOS, crate::arena::SCENARIO).expect("listed");
        let cut_of = |scene: &mut Scene| {
            let cut = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").cut;
            scene.end_frame();
            cut
        };
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        assert!(!cut_of(&mut scene), "a first frame has nothing to drop");
        let mut first = Recording::new(scenario, scenario.config).expect("sets up");
        let cast = first.cast().to_vec();
        let mut source = arena_walk;
        for _ in 0..80 {
            first.step(&mut source).expect("short");
            scene.follow_simulation(first.simulation(), &cast);
        }
        assert!(!cut_of(&mut scene), "a fight going on");

        let mut next = Recording::new(scenario, scenario.config).expect("sets up");
        next.step(&mut source).expect("short");
        scene.follow_simulation(next.simulation(), &cast);
        // A frame with no target draws no view, and leaves the cut for the one that does.
        let _ = scene.frame_packet(1.0, None);
        scene.end_frame();
        assert!(cut_of(&mut scene), "the new fight's first view");
        assert!(!cut_of(&mut scene), "and only that");

        let party = find_scenario(SCENARIOS, crate::arena::PARTY_SCENARIO).expect("listed");
        let mut swapping = Recording::new(party, party.config).expect("sets up");
        let cast = swapping.cast().to_vec();
        let mut source = find_script("party-swap").expect("listed").actions;
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        for _ in 0..240 {
            swapping.step(&mut source).expect("short");
            scene.follow_simulation(swapping.simulation(), &cast);
            assert!(!cut_of(&mut scene), "tick {}", swapping.simulation().tick());
        }
        assert!(swapping.simulation().party().swaps() > 0, "it swapped");
    }

    /// A shake's roll, which `OrbitCamera::set_view` can't take, reaches the frame's view: everything
    /// sums on the director's pose, so the roll is read back off it.
    #[test]
    fn a_directors_shake_rolls_the_view_the_frame_draws() {
        let (mut scene, _) = arena_stage(ArenaShot::Director);
        // How far the view's right has left level, from a frame `dt` seconds on.
        let tilt = |scene: &mut Scene, dt: f32| {
            assert!(scene.drive_arena_camera(dt, sapphire_math::Vec2::ZERO, 0.0));
            let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera.view;
            scene.end_frame();
            view.inverse().x_axis.y.abs()
        };
        assert!(tilt(&mut scene, 1.0 / 60.0) < 1e-5, "level with no shake");
        let director = scene.arena.as_mut().expect("built").director().expect("this shot has one");
        let finish = director.tuning().shake_id("finish").expect("a shipped preset");
        director.fire_shake(finish, 1.0);
        let most = (0..30).map(|_| tilt(&mut scene, 1.0 / 60.0)).fold(0.0_f32, f32::max);
        assert!(most > 1e-3, "it rolls: {most}");
    }

    /// A pause holds what's on screen, a shake mid-swing and the director's springs among it, however
    /// long it lasts, and the shake plays on from there when it ends.
    /// A real party fight, by the script that swaps back to someone hurt: the bar shows a loss for a hit,
    /// and none for a swap to someone with less.
    #[test]
    fn a_swap_doesnt_read_as_a_hit_on_the_huds_bar() {
        use sapphire_gameplay::replay::{Recording, SCENARIOS, find_scenario, find_script};
        use sapphire_gameplay::{ActorActions, ActorId, Buttons};
        use sapphire_math::Vec2;
        use sapphire_ui::{HudLayout, Insets, UiMetrics};

        use crate::hud::{HudMode, HudOptions};

        let target = (1280, 720);
        let bar = HudLayout::new(UiMetrics::of_pixels(target, 1.0, Insets::default())).player.hp;
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        scene.set_hud(SandboxHud::for_fight(live));
        let party = find_scenario(SCENARIOS, crate::arena::PARTY_SCENARIO).expect("listed");
        let mut fight = Recording::new(party, party.config).expect("sets up");
        let cast = fight.cast().to_vec();
        // Its schedule goes round to the first again, and pressing on brings back those the warrior hurt.
        let script = find_script("party-swap").expect("listed").actions;
        let mut source = |tick: u64, cast: &[ActorId]| {
            let mut actions = script(tick, cast);
            if tick >= 352 && tick.is_multiple_of(2) {
                let swap = ActorActions { buttons: Buttons::SWAP, ..ActorActions::NEUTRAL };
                actions.set(cast[0], swap).expect("one actor fits a tick");
            }
            actions
        };
        let mut on_field = cast[0];
        let (mut swaps_down, mut losses) = (0, 0);
        for _ in 0..1200 {
            fight.step(&mut source).expect("short");
            let sim = fight.simulation();
            scene.follow_simulation(sim, &cast);
            let quads = &scene.frame_packet(1.0, Some(target)).ui.quads;
            // What the bar shows lost: a range that starts past its left end, on the bar itself.
            let lost = quads
                .iter()
                .filter(|quad| {
                    let center =
                        Vec2::from(quad.origin) + (Vec2::from(quad.axis_x) + Vec2::from(quad.axis_y)) * 0.5;
                    quad.clip[0] > 0.0 && (center - bar.center()).length() < 1e-2
                })
                .count();
            scene.end_frame();

            let share = |id| {
                let fighter = sim.actor(id).expect("in the party").fighter;
                fighter.health() as f32 / fighter.max_health() as f32
            };
            let now = sim.party().on_field(cast[0]);
            if now != on_field {
                if share(now) < share(on_field) {
                    swaps_down += 1;
                    assert_eq!(lost, 0, "tick {}: a swap to someone with less", sim.tick());
                }
                on_field = now;
            }
            losses += lost;
        }
        assert!(swaps_down > 0, "the script swaps to someone hurt");
        assert!(losses > 0, "and the bar does show a hit");
    }

    /// The HUD says a fight is waiting for as long as it is, on a phone as on a desktop, and a capture
    /// can ask for the same.
    #[test]
    fn a_pause_shows_on_the_hud() {
        use crate::hud::{HudMode, HudOptions};

        let quads = |scene: &mut Scene| {
            let mut list = sapphire_ui::UiDrawList::default();
            scene.hud.as_mut().expect("set").draw((1000, 500), &mut list);
            list.quads.len()
        };
        let live = HudOptions { mode: HudMode::Live, ..HudOptions::default() };
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), [[-1.0; 3], [1.0; 3]], false);
        scene.set_hud(SandboxHud::for_fight(live));
        let size = sapphire_platform::Size { width: 1000, height: 500 };
        let mut input = SandboxInput::new(false, true, size).expect("the default bindings");
        let time = |ms: u64| sapphire_platform::Timestamp::from_nanos(ms * 1_000_000);
        input.update(time(1));
        scene.drive_view(&input, 1.0 / 60.0);
        let running = quads(&mut scene);

        input.pause();
        input.update(time(2));
        scene.drive_view(&input, 1.0 / 60.0);
        assert_eq!(quads(&mut scene), running + 3, "a veil and two bars");

        let key = |ms: u64, down| {
            sapphire_platform::Event::Key(sapphire_platform::KeyEvent {
                timestamp: time(ms),
                window: sapphire_platform::WindowId::from_raw(1),
                scancode: sapphire_platform::Scancode::P,
                keycode: sapphire_platform::Keycode(0),
                modifiers: sapphire_platform::Modifiers::default(),
                down,
                repeat: false,
            })
        };
        input.handle_event(&key(3, true));
        input.handle_event(&key(4, false));
        input.update(time(5));
        input.update(time(6));
        assert!(!input.is_paused());
        scene.drive_view(&input, 1.0 / 60.0);
        assert_eq!(quads(&mut scene), running, "gone with the pause");

        let asked = HudOptions { paused: true, ..live };
        scene.set_hud(SandboxHud::for_fight(asked));
        assert_eq!(quads(&mut scene), running + 3, "with no input at all");
    }

    #[test]
    fn a_pause_holds_the_directors_view_and_its_shake() {
        let (mut scene, _) = arena_stage(ArenaShot::Director);
        let size = sapphire_platform::Size { width: 1000, height: 500 };
        let mut input = SandboxInput::new(true, true, size).expect("the default bindings");
        let press = |input: &mut SandboxInput, at: u64| {
            let time = |ms: u64| sapphire_platform::Timestamp::from_nanos(ms * 1_000_000);
            let key = |ms: u64, down| {
                sapphire_platform::Event::Key(sapphire_platform::KeyEvent {
                    timestamp: time(ms),
                    window: sapphire_platform::WindowId::from_raw(1),
                    scancode: sapphire_platform::Scancode::P,
                    keycode: sapphire_platform::Keycode(0),
                    modifiers: sapphire_platform::Modifiers::default(),
                    down,
                    repeat: false,
                })
            };
            input.handle_event(&key(at, true));
            input.handle_event(&key(at + 1, false));
            input.update(time(at + 2));
        };
        let frame = |scene: &mut Scene, input: &SandboxInput| {
            scene.drive_view(input, 1.0 / 60.0);
            let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera.view;
            scene.end_frame();
            view
        };
        let _ = frame(&mut scene, &input);
        let director = scene.arena.as_mut().expect("built").director().expect("this shot has one");
        let finish = director.tuning().shake_id("finish").expect("a shipped preset");
        director.fire_shake(finish, 1.0);
        for _ in 0..4 {
            let _ = frame(&mut scene, &input);
        }

        press(&mut input, 100);
        assert!(input.is_paused());
        let held = frame(&mut scene, &input);
        for _ in 0..120 {
            assert_eq!(frame(&mut scene, &input), held, "nothing moves in a pause");
        }

        press(&mut input, 200);
        input.update(sapphire_platform::Timestamp::from_nanos(210_000_000));
        assert!(!input.is_paused());
        let moved = (0..4).any(|_| frame(&mut scene, &input) != held);
        assert!(moved, "the shake plays on from where it was");
    }

    /// The arena stands its two actors where the simulation has them, and no turntable moves them:
    /// at their spawn points before a replay moves them, then each step's position, and the camera
    /// follows the player. Moved or not, its town is the look-dev stage's, on its day.
    #[test]
    fn the_arena_stands_its_actors_where_the_simulation_has_them() {
        use sapphire_gameplay::arena::spawns;
        use sapphire_gameplay::replay::{Recording, SCENARIOS, arena_walk, find_scenario};

        let (mut scene, figure) = arena_stage(ArenaShot::Follow);
        assert_eq!(scene.time_of_day().timeline(), &lookdev::day(), "the look-dev stage's day");
        assert!(scene.ecs.world().get::<Turntable>(scene.character).is_none());

        let spawns = spawns();
        let (character, enemy) = (scene.character, scene.arena.as_ref().expect("built").enemy_entity());
        let origin = |scene: &Scene, entity: Entity| {
            scene.ecs.world().get::<RenderTransform>(entity).expect("placed").0.transform_point3(Vec3::ZERO)
        };
        let _ = scene.frame_packet(1.0, Some((16, 9)));
        scene.end_frame();
        assert_eq!(origin(&scene, character), spawns.player.position());
        assert_eq!(origin(&scene, enemy), spawns.enemy.position());
        assert_eq!(scene.figure, figure);
        // Drawn at its own size, with its blade where the model holds it, facing where it faces.
        let (warrior, blade) = scene.enemy.expect("it stands there");
        let scale = enemy::SCALE;
        let transform =
            |scene: &Scene, entity| scene.ecs.world().get::<RenderTransform>(entity).expect("placed").0;
        let upright = transform(&scene, warrior).transform_vector3(Vec3::Y).length();
        assert!((upright - scale).abs() < 1e-5, "{upright}");
        let hand = transform(&scene, blade).transform_point3(Vec3::ZERO);
        let expected = spawns.enemy.position() + Vec3::new(0.0, scale, 0.0);
        assert!(hand.distance(expected) < 1e-5, "{hand}");
        let packet = scene.ecs.world().resource::<RenderExtract>().packet();
        assert_eq!(packet.characters.len(), 3, "all three are lit as characters");

        let scenario = find_scenario(SCENARIOS, crate::arena::SCENARIO).expect("listed");
        let mut recording = Recording::new(scenario, scenario.config).expect("sets up");
        let mut source = arena_walk;
        let cast = recording.cast().to_vec();
        for _ in 0..80 {
            recording.step(&mut source).expect("short");
            scene.follow_simulation(recording.simulation(), &cast);
        }
        let walked = recording.simulation().actor(cast[0]).expect("alive").position;
        let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera;
        let eye = view.position;
        scene.end_frame();
        assert_eq!(origin(&scene, character), walked);
        assert_eq!(origin(&scene, enemy), spawns.enemy.position(), "the enemy stands");
        assert!(walked.distance(spawns.player.position()) > 1.5, "it walked: {walked}");
        let target = crate::arena::follow_target(walked);
        assert!(
            (eye.distance(target) - ArenaShot::Follow.camera(walked).eye().distance(target)).abs() < 1e-4
        );
        assert!(eye.z > walked.z, "the camera came with it: {eye}");

        // The director's view is the frame's and the stick's, behind the player and framing both.
        let (mut scene, _) = arena_stage(ArenaShot::Director);
        let player = spawns.player.position();
        let view = scene.frame_packet(1.0, Some((16, 9))).main_view().expect("a view").camera;
        let eye = view.position;
        scene.end_frame();
        assert!(eye.z > player.z, "behind the player, who faces -Z: {eye}");
        assert!(eye.distance(player) > 2.0, "{eye}");
        assert!((scene.camera().eye() - eye).length() < 1e-3, "{eye}");
        let heading = scene.camera_heading();
        assert!(sapphire_math::det::cos(heading) < -0.8, "it looks along -Z: {heading}");
        assert!(
            (sapphire_math::det::sin(heading - scene.camera().heading())).abs() < 1e-3,
            "the camera's and the director's headings agree: {heading}, {}",
            scene.camera().heading()
        );
        // A look turns it, and nothing moves it without time.
        assert!(scene.drive_arena_camera(1.0 / 60.0, sapphire_math::Vec2::new(0.2, 0.0), 0.0));
        let _ = scene.frame_packet(1.0, Some((16, 9)));
        scene.end_frame();
        assert_ne!(scene.camera_heading(), heading);
        // With another shot it takes nothing, and the plain camera's heading is the one.
        let (mut scene, _) = arena_stage(ArenaShot::Follow);
        assert!(!scene.drive_arena_camera(1.0, sapphire_math::Vec2::ZERO, 0.0));
        assert_eq!(scene.camera_heading(), scene.camera().heading());
    }

    #[test]
    fn the_stalk_is_posed_while_it_sways_and_not_while_paused() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, true);
        scene.set_stalk_mesh(MeshHandle::from_raw(1));
        let stalk = InstanceId(scene.stalk.expect("skinned").to_bits());
        let pose = |scene: &mut Scene| {
            scene.step();
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            let poses: Vec<_> = packet
                .skinned
                .iter()
                .map(|skinned| (skinned.id, packet.palette(skinned).expect("in range").to_vec()))
                .collect();
            scene.end_frame();
            poses
        };
        let first = pose(&mut scene);
        assert_eq!(first.len(), 1);
        assert_eq!((first[0].0, first[0].1.len()), (stalk, 6));
        let second = pose(&mut scene);
        assert_ne!(second[0].1, first[0].1, "it sways every step");

        scene.world_clock_mut().0.push_scale(ScaleSource::Pause, 0).expect("a free scale slot");
        pose(&mut scene);
        assert!(pose(&mut scene).is_empty(), "paused, it holds its pose and sends none");
    }

    #[test]
    fn demo_lights_surround_the_character() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        scene.spawn_demo_lights();
        for _ in 0..2 {
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            assert_eq!(packet.lights.len(), 3, "every frame");
            let [min, max] = BOUNDS.map(Vec3::from);
            for light in &packet.lights {
                let nearest = light.position.clamp(min, max);
                assert!(
                    light.position.distance(nearest) < light.range * 0.5,
                    "{light:?} reaches the character"
                );
            }
            let LightKind::Spot { direction, .. } = packet.lights[2].kind else { panic!("a spot") };
            let to_floor = Vec3::new(-0.16, 0.2, 0.32) - packet.lights[2].position;
            assert!(direction.normalize().dot(to_floor.normalize()) > 0.999, "aimed in front of the feet");
            scene.end_frame();
        }
    }

    /// The enemy stands beside the character in a shape of its own, and its blade lights as it does
    /// but casts no capsule, so it adds nothing to shadows or occlusion. Each keeps its pose and
    /// transform through a device loss's new meshes.
    #[test]
    fn the_enemy_stands_beside_the_character_in_a_shape_of_its_own() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let figure = Figure {
            head: Vec3::new(0.0, 1.45, 0.0),
            feet: Vec3::ZERO,
            body_radius: 0.16,
            face: Quat::IDENTITY,
            expression: FaceExpression::default(),
        };
        let town = TownMeshes {
            town: MeshHandle::from_raw(1),
            plants: [MeshHandle::from_raw(2); 8],
            mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
        };
        let pose = RenderPose { palette: vec![Affine3A::IDENTITY; 2], morph_weights: Vec::new() };
        scene.spawn_look_dev(Shot::Enemy, LookDev { town, fog_cards: None, pose: pose.clone(), figure });
        let warrior_figure = Figure {
            head: Vec3::new(0.0, 2.0, 0.0),
            feet: Vec3::ZERO,
            body_radius: 0.3,
            face: Quat::IDENTITY,
            expression: FaceExpression::default(),
        };
        let held = enemy::Held {
            pose,
            figure: warrior_figure,
            blade: Mat4::from_translation(Vec3::new(-0.8, 1.2, 0.0)),
            hand: 0,
        };
        let meshes = EnemyMeshes { warrior: MeshHandle::from_raw(4), blade: MeshHandle::from_raw(5) };
        scene.spawn_enemy(meshes, held);
        scene.step();
        let (warrior, blade) = scene.enemy.expect("it stands there");
        let (warrior, blade) = (InstanceId(warrior.to_bits()), InstanceId(blade.to_bits()));
        let character = InstanceId(scene.character.to_bits());
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_eq!(packet.characters.len(), 3, "the character, the warrior and its blade");
        assert_eq!(
            packet.skinned.iter().filter(|skinned| skinned.id == warrior).count(),
            1,
            "the pose is sent"
        );
        let listed = |id| packet.characters.iter().find(|character| character.id == id).expect("listed");
        let scale = enemy::SCALE;
        assert!(
            (listed(warrior).body.radius - warrior_figure.body_radius * scale).abs() < 1e-4,
            "its own body, placed: {:?}",
            listed(warrior).body
        );
        assert!((listed(character).body.radius - figure.body_radius).abs() < 1e-5, "the character's own");
        assert!(!listed(blade).body.casts(), "the blade adds no capsule");
        let (warrior_key, blade_key) = (listed(warrior).lighting.key, listed(blade).lighting.key);
        assert!(warrior_key.direction.abs_diff_eq(blade_key.direction, 1e-4), "the two are lit alike");
        scene.end_frame();
        scene.set_enemy_meshes(EnemyMeshes {
            warrior: MeshHandle::from_raw(6),
            blade: MeshHandle::from_raw(7),
        });
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        let drawn: Vec<MeshHandle> = packet.instances.changed.iter().map(|instance| instance.mesh).collect();
        assert!(
            drawn.contains(&MeshHandle::from_raw(6)) && drawn.contains(&MeshHandle::from_raw(7)),
            "{drawn:?}"
        );
    }

    #[test]
    fn the_look_dev_stage_holds_the_pose_and_lights_its_lamps_by_night() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let lifted = Affine3A::from_translation(Vec3::Y);
        let pose = RenderPose { palette: vec![lifted; 2], morph_weights: vec![0.5] };
        let figure = Figure {
            head: Vec3::new(0.0, 1.45, 0.0),
            feet: Vec3::ZERO,
            body_radius: 0.16,
            face: Quat::IDENTITY,
            expression: FaceExpression::default(),
        };
        let town = TownMeshes {
            town: MeshHandle::from_raw(1),
            plants: [MeshHandle::from_raw(2); 8],
            mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
        };
        let look = LookDev { town, fog_cards: None, pose: pose.clone(), figure };
        scene.spawn_look_dev(Shot::Hero, look);
        assert_eq!(scene.time_of_day().timeline(), &lookdev::day(), "the stage runs on its own day");
        assert_eq!(scene.town_plants.len(), lookdev::PLANTINGS.len());
        assert_eq!(scene.town_mountains.len(), lookdev::MOUNTAINS.len());
        let character = InstanceId(scene.character.to_bits());
        let frame = |scene: &mut Scene, hours: f32| {
            scene.set_day(hours, None);
            scene.step();
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            let lamps: f32 = packet.lights.iter().map(|light| light.intensity).sum();
            let posed: Vec<_> = packet.skinned.iter().map(|skinned| skinned.id).collect();
            let head = packet.characters[0].lighting.gi.head;
            scene.end_frame();
            (lamps, posed, head)
        };
        let (noon, posed, head) = frame(&mut scene, 13.0);
        assert_eq!(noon, 0.0, "lamps are out by day");
        assert_eq!(posed, [character], "the held pose, sent once");
        assert_eq!(head, Some(figure.head), "the rig finds the figure's head");
        let (midnight, posed, _) = frame(&mut scene, 0.0);
        let full = lookdev::LAMP_INTENSITY * lookdev::LAMPS.len() as f32;
        assert!((midnight - full).abs() < 1e-3, "lamps at full by night: {midnight}");
        assert!(posed.is_empty(), "a held pose isn't sent again");
    }

    /// Review RC-1: the character's palette indexes its mesh's joints, and its figure comes from
    /// them.
    #[test]
    fn a_character_held_anew_sends_a_new_pose_once_and_takes_its_figure() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let lifted = Affine3A::from_translation(Vec3::Y);
        let pose = RenderPose { palette: vec![lifted; 2], morph_weights: vec![0.5] };
        let figure = Figure {
            head: Vec3::new(0.0, 1.45, 0.0),
            feet: Vec3::ZERO,
            body_radius: 0.16,
            face: Quat::IDENTITY,
            expression: FaceExpression::default(),
        };
        let town = TownMeshes {
            town: MeshHandle::from_raw(1),
            plants: [MeshHandle::from_raw(2); 8],
            mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
        };
        scene.spawn_look_dev(Shot::Hero, LookDev { town, fog_cards: None, pose: pose.clone(), figure });
        let frame = |scene: &mut Scene| {
            scene.step();
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            let palettes: Vec<usize> = packet
                .skinned
                .iter()
                .map(|skinned| packet.palette(skinned).expect("in range").len())
                .collect();
            let head = packet.characters[0].lighting.gi.head;
            scene.end_frame();
            (palettes, head)
        };
        assert_eq!(frame(&mut scene), (vec![2], Some(figure.head)));
        assert_eq!(frame(&mut scene), (vec![], Some(figure.head)), "a held pose is sent once");

        // The same pose again sends nothing, only a replaced mesh does.
        scene.hold_character(pose.clone(), figure);
        assert_eq!(frame(&mut scene).0, Vec::<usize>::new());
        scene.ecs.world_mut().resource_mut::<RenderExtract>().resend_poses();
        assert_eq!(frame(&mut scene).0, [2], "a replaced mesh's instance starts over");
        assert_eq!(frame(&mut scene).0, Vec::<usize>::new());

        // Another skeleton's pose and figure go out in the next frame, once.
        let taller = Figure { head: Vec3::new(0.0, 1.6, 0.0), ..figure };
        scene.hold_character(RenderPose { palette: vec![lifted; 3], morph_weights: vec![0.5] }, taller);
        assert_eq!(frame(&mut scene), (vec![3], Some(taller.head)), "the rig finds the new head");
        assert_eq!(frame(&mut scene).0, Vec::<usize>::new());
    }

    /// A small humanoid for clips: hips, a spine, a head, and two feet, each a joint of the palette.
    mod clips {
        use sapphire_anim::{
            CharacterPose, Clip, Joint, JointTransform, PackedRotation, RotationTrack, Skeleton, SpringBones,
            VectorRange, VectorTrack,
        };
        use sapphire_assets::AssetId;
        use sapphire_assets::clip::{DEFAULT_RATE, RootMotion, WeightRange, WeightTrack};
        use sapphire_assets::mesh::SkinBind;
        use sapphire_assets::rig::{CharacterRig, Spring, SpringJoint, SpringTip};
        use sapphire_math::{Affine3A, Mat4, Quat, Vec3};
        use sapphire_render::{InstanceId, MeshHandle};
        use sapphire_world::light_rig::{CharacterLightRig, LightRigId};

        use super::*;
        use crate::anim_graph::GraphScript;
        use crate::animate::ClipPlayer;
        use crate::extract::{RenderMesh, RenderPose};
        use crate::lookdev::FigureJoints;

        const HIPS: [f32; 3] = [0.0, 0.8, 0.0];

        fn skeleton() -> Skeleton {
            let joint = |name: &str, parent: Option<u16>, translation: [f32; 3]| Joint {
                name: name.to_owned(),
                parent,
                rest: JointTransform { translation, ..JointTransform::IDENTITY },
            };
            Skeleton::new(&[
                joint("hips", None, HIPS),
                joint("spine", Some(0), [0.0, 0.4, 0.0]),
                joint("head", Some(1), [0.0, 0.3, 0.0]),
                joint("left foot", Some(0), [0.1, -0.7, 0.0]),
                joint("right foot", Some(0), [-0.1, -0.7, 0.0]),
            ])
        }

        /// Over 60 ticks: the head turning a quarter about Y, the hips dropping 0.2 m and
        /// leaning, the second morph target from 0 to 1, and 3 m of root motion.
        fn clip() -> Clip {
            let half = core::f32::consts::FRAC_1_SQRT_2;
            let drop = VectorRange::enclosing([HIPS, [0.0, 0.6, 0.0]]);
            let root = VectorRange::enclosing([[0.0; 3], [0.0, 0.0, 3.0]]);
            let weight = WeightRange::enclosing([0.0, 1.0]);
            Clip {
                name: "turn".into(),
                skeleton: Some(AssetId::from_bytes([9; 16])),
                joint_count: 5,
                morph_target_count: 2,
                rate: DEFAULT_RATE,
                ticks: 60,
                rotations: vec![
                    RotationTrack { joint: 1, count: 2, first: 0 },
                    RotationTrack { joint: 2, count: 2, first: 2 },
                ],
                translations: vec![VectorTrack { joint: 0, count: 2, first: 0, range: drop }],
                weights: vec![WeightTrack { target: 1, count: 2, first: 0, range: weight }],
                root: Some(RootMotion {
                    joint: 0,
                    translation: VectorTrack { joint: 0, count: 2, first: 2, range: root },
                    rotation: RotationTrack { joint: 0, count: 1, first: 4 },
                }),
                rotation_ticks: vec![0, 60, 0, 60, 0],
                rotation_keys: vec![
                    PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                    PackedRotation::encode(Quat::from_rotation_x(0.4).to_array()),
                    PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                    PackedRotation::encode([0.0, half, 0.0, half]),
                    PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                ],
                vector_ticks: vec![0, 60, 0, 60],
                vector_keys: vec![
                    drop.encode(HIPS),
                    drop.encode([0.0, 0.6, 0.0]),
                    root.encode([0.0; 3]),
                    root.encode([0.0, 0.0, 3.0]),
                ],
                weight_ticks: vec![0, 60],
                weight_keys: vec![weight.encode(0.0), weight.encode(1.0)],
                ..Clip::default()
            }
        }

        fn player() -> ClipPlayer {
            let bytes = skeleton().to_cooked().expect("cooked");
            let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
            let binds: Vec<SkinBind> = (0..5)
                .map(|joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
                .collect();
            let pose = CharacterPose::new(skeleton, &binds, &[], vec![0.25, 0.0]);
            ClipPlayer::new(pose, FigureJoints::for_test(&skeleton_pose_rest(), 2, 0.0, [3, 4]))
        }

        fn skeleton_pose_rest() -> CharacterPose {
            let bytes = skeleton().to_cooked().expect("cooked");
            let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
            CharacterPose::new(skeleton, &[], &[], Vec::new())
        }

        /// The look-dev stage holding the character still.
        fn stage() -> Scene {
            let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
            let pose = RenderPose { palette: vec![Affine3A::IDENTITY; 5], morph_weights: vec![0.25, 0.0] };
            let figure = Figure {
                head: Vec3::new(0.0, 1.6, 0.0),
                feet: Vec3::ZERO,
                body_radius: 0.16,
                face: Quat::IDENTITY,
                expression: FaceExpression::default(),
            };
            let town = TownMeshes {
                town: MeshHandle::from_raw(1),
                plants: [MeshHandle::from_raw(2); 8],
                mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
            };
            scene.spawn_look_dev(Shot::Hero, LookDev { town, fog_cards: None, pose, figure });
            scene
        }

        /// What one frame's packet says of `id`.
        #[derive(Clone, Debug, PartialEq)]
        struct Drawn {
            palette: Option<Vec<[f32; 12]>>,
            weights: Option<Vec<f32>>,
            moved_to: Option<Vec3>,
            head: Option<Vec3>,
            forward: Vec3,
            body: CharacterBody,
            removed: bool,
        }

        fn frame(scene: &mut Scene, clip: &ArchivedClip, alpha: f32, id: InstanceId) -> Drawn {
            scene.animate_clip(Some((clip, 0)), alpha);
            let drawn = drawn(scene, alpha, id);
            scene.end_frame();
            drawn
        }

        /// What this frame's packet says of `id`.
        fn drawn(scene: &mut Scene, alpha: f32, id: InstanceId) -> Drawn {
            let packet = scene.frame_packet(alpha, Some((1920, 1080)));
            let posed = packet.skinned.iter().find(|skinned| skinned.id == id);
            let palette = posed.map(|skinned| {
                packet.palette(skinned).expect("in range").iter().map(Affine3A::to_cols_array).collect()
            });
            let weights = posed.map(|skinned| packet.morph_weights(skinned).expect("in range").to_vec());
            let moved_to = packet
                .instances
                .changed
                .iter()
                .find(|instance| instance.id == id)
                .map(|instance| instance.transform.transform_point3(Vec3::ZERO));
            let entry = packet.characters.iter().find(|character| character.id == id);
            Drawn {
                palette,
                weights,
                moved_to,
                head: entry.and_then(|entry| entry.lighting.gi.head),
                forward: entry.map_or(Vec3::ZERO, |entry| entry.face.head.forward),
                body: entry.map_or(CharacterBody::default(), |entry| entry.body),
                removed: packet.instances.removed.contains(&id),
            }
        }

        /// A graph with `clip()` for its only clip, playing it standing and for the show's moves.
        fn graph() -> sapphire_anim::AnimGraph {
            let mut graph = sapphire_anim::AnimGraph::from_document(
                r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (
                    clips: [(name: "turn", source: "15af86bb-67b3-4208-bc54-1618aabf450b")],
                    locomotion: (
                        start: "ground",
                        states: [(name: "ground", plays: Clip(clip: "turn", looping: true))],
                    ),
                    combat: (decay: 5, exit: 5, moves: [
                        (move: "strike", clip: "turn"),
                        (move: "dodge_back", clip: "turn"),
                    ]),
                ))"#,
            )
            .expect("a good document");
            graph.resolve(|_| GRAPH_CLIP);
            graph
        }

        const GRAPH_CLIP: AssetId = AssetId::from_bytes([8; 16]);

        fn driver(table: &ClipTable<'_>) -> GraphDriver {
            let bytes = skeleton().to_cooked().expect("cooked");
            let skeleton = Skeleton::read(&bytes).expect("a valid skeleton");
            let binds: Vec<SkinBind> = (0..5)
                .map(|joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
                .collect();
            let binding =
                sapphire_anim::GraphBinding::new(&graph(), skeleton, None, 2, table).expect("it binds");
            let pose = CharacterPose::new(skeleton, &binds, &[], vec![0.25, 0.0]);
            let player = sapphire_anim::GraphPlayer::new(Arc::new(binding), pose, 1);
            GraphDriver::new(player, FigureJoints::for_test(&skeleton_pose_rest(), 2, 0.0, [3, 4]))
        }

        fn graph_frame(scene: &mut Scene, table: &ClipTable<'_>, alpha: f32, id: InstanceId) -> Drawn {
            scene.animate_graph(table, alpha);
            let drawn = drawn(scene, alpha, id);
            scene.end_frame();
            drawn
        }

        /// The graph writes what a clip does, and its actor places the character: the first frame
        /// is posed, and a move's root motion comes from the simulation, with no clip's in it.
        #[test]
        fn a_graph_poses_the_character_and_its_actor_places_it() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            let table = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());
            let mut scene = stage();
            scene.play_graph(driver(&table), GraphShow::new(GraphScript::Cancel));
            let id = extract::instance_id(scene.character);

            let first = graph_frame(&mut scene, &table, 1.0, id);
            let palette = first.palette.clone().expect("posed at once");
            assert_eq!(palette.len(), 5);
            assert_eq!(scene.anim_stats().characters, 1);
            assert_eq!(scene.anim_stats().evaluated, 1);
            assert_eq!(scene.anim_stats().joints, 5);

            // The clip's own root motion is 3 m over its 60 ticks: none of it shows. The strike's root
            // has it 0.35 m forward by tick 20 of the move, and then the dodge takes it 1.33 m back.
            let mut farthest = 0.0_f32;
            for _ in 0..50 {
                scene.step();
                let drawn = graph_frame(&mut scene, &table, 1.0, id);
                assert_ne!(drawn.palette.clone().expect("every tick"), palette, "the pose moved");
                if let Some(moved) = drawn.moved_to {
                    farthest = farthest.min(moved.z);
                    assert!(moved.z.abs() < 1.5, "the simulation's, not the clip's: {moved}");
                }
            }
            assert!(farthest < -0.5, "the dodge's travel: {farthest}");
        }

        /// A stopped clock sends the renderer nothing: no skin, no move, and nothing evaluated.
        #[test]
        fn a_stopped_clock_costs_a_graph_nothing() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            let table = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());
            let mut scene = stage();
            scene.play_graph(driver(&table), GraphShow::new(GraphScript::Hitstop));
            let id = extract::instance_id(scene.character);
            for _ in 0..31 {
                scene.step();
                graph_frame(&mut scene, &table, 1.0, id);
            }
            scene.step();
            for alpha in [1.0, 0.25, 0.5, 1.0] {
                let drawn = graph_frame(&mut scene, &table, alpha, id);
                assert_eq!(drawn.palette, None, "nothing to skin");
                assert_eq!(drawn.weights, None);
                assert_eq!(drawn.moved_to, None, "nothing moved");
                assert_eq!(scene.anim_stats().characters, 1);
                assert_eq!(scene.anim_stats().evaluated, 0);
            }
        }

        /// An arena whose fighters both play `driver`'s graph, standing at the start of a fight, and that
        /// fight: the warrior's blade hangs from joint 2.
        fn fighting(table: &ClipTable<'_>) -> (Scene, Simulation, Vec<sapphire_gameplay::ActorId>) {
            use sapphire_gameplay::replay::{SCENARIOS, find_scenario};

            let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
            let pose = RenderPose { palette: vec![Affine3A::IDENTITY; 5], morph_weights: vec![0.25, 0.0] };
            let figure = Figure { head: Vec3::new(0.0, 1.6, 0.0), ..Figure::MANNEQUIN };
            let town = TownMeshes {
                town: MeshHandle::from_raw(1),
                plants: [MeshHandle::from_raw(2); lookdev::Plant::ALL.len()],
                mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
            };
            let cooked = sapphire_gameplay::arena::cooked_colliders().expect("it cooks");
            let set = sapphire_colliders::ColliderSet::read(&cooked).expect("it reads");
            let held = enemy::Held { pose: pose.clone(), figure, blade: Mat4::IDENTITY, hand: 2 };
            scene.spawn_arena(Arena {
                enemy: EnemyMeshes { warrior: MeshHandle::from_raw(4), blade: MeshHandle::from_raw(5) },
                held,
                shot: ArenaShot::Follow,
                look: LookDev { town, fog_cards: None, pose, figure },
                ground: ArenaGround::new(set).expect("the stage stands on it"),
            });
            scene.play_fighters(driver(table).with_right_hand(1), driver(table));
            let scenario = find_scenario(SCENARIOS, crate::arena::FIGHT_SCENARIO).expect("listed");
            let (sim, cast) = scenario.start(scenario.config).expect("it starts");
            (scene, sim, cast)
        }

        /// The character's sword stays in the hand of the held pose until its graph has made a pose.
        #[test]
        fn a_graphs_hand_says_nothing_until_it_has_a_pose() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            let table = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());
            let (mut scene, mut sim, cast) = fighting(&table);
            let character = scene.character;
            let hand =
                |scene: &Scene| scene.ecs.world().get::<GraphDriver>(character).expect("on").right_hand();
            assert_eq!(hand(&scene), None, "no pose yet");
            sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
            scene.follow_simulation(&sim, &cast);
            scene.animate_fighters(&table, &table, 1.0);
            let held = scene.ecs.world().get::<GraphDriver>(character).expect("on").player.pose().model()[1];
            assert_eq!(hand(&scene), Some(Mat4::from(held)));
        }

        /// Both fighters play from the steps' views, the enemy's blade goes where its hand does, a frozen
        /// clock asks nothing of them, and a frame allocates nothing.
        #[test]
        fn both_fighters_play_their_graphs_and_the_blade_stays_in_hand() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            let table = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());
            let (mut scene, mut sim, cast) = fighting(&table);
            let (warrior, blade) = scene.enemy.expect("it stands there");
            let palette = |scene: &Scene, entity| {
                scene.ecs.world().get::<RenderPose>(entity).expect("posed").palette.clone()
            };
            let held = palette(&scene, warrior);

            scene.animate_fighters(&table, &table, 1.0);
            assert_eq!(scene.anim_stats().characters, 0, "nothing before a fight");
            assert_eq!(palette(&scene, warrior), held, "it holds what it has");

            sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
            scene.follow_simulation(&sim, &cast);
            scene.animate_fighters(&table, &table, 1.0);
            assert_eq!(scene.anim_stats().characters, 2);
            assert_eq!(scene.anim_stats().evaluated, 2);
            assert_eq!(scene.anim_stats().joints, 10);
            assert_ne!(palette(&scene, warrior), held, "the graph has it");
            assert_ne!(palette(&scene, scene.character), held);

            let _ = scene.frame_packet(1.0, Some((16, 9)));
            scene.end_frame();
            let world = scene.ecs.world();
            let drawn = |entity| world.get::<RenderTransform>(entity).expect("placed").0;
            let driver = world.get::<GraphDriver>(warrior).expect("playing");
            let hand = Mat4::from(driver.player.pose().model()[2]);
            let expected = drawn(warrior) * hand;
            assert!(drawn(blade).abs_diff_eq(expected, 1e-5), "{} against {expected}", drawn(blade));

            scene.animate_fighters(&table, &table, 1.0);
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                for _ in 0..200 {
                    sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
                    scene.follow_simulation(&sim, &cast);
                    scene.animate_fighters(&table, &table, 0.5);
                }
            });
            if sapphire_core::alloc::PER_THREAD_COUNTS {
                assert_eq!(counts.allocations, 0, "{counts:?}");
            }

            let mut frozen = false;
            for _ in 0..240 {
                sim.step(&sapphire_gameplay::TickActions::NEUTRAL);
                scene.follow_simulation(&sim, &cast);
                frozen = sim.actor(cast[0]).expect("alive").clock_delta == sapphire_time::SimDuration::ZERO;
                scene.animate_fighters(&table, &table, 1.0);
                if frozen {
                    break;
                }
            }
            assert!(frozen, "a hit stops its target's clock within 4 s");
            for alpha in [0.25, 0.75] {
                scene.animate_fighters(&table, &table, alpha);
                assert!(scene.anim_stats().evaluated <= 1, "at most the one still moving");
            }
        }

        /// A scene with no graph does no graph work, and a graph's frame allocates nothing once it's
        /// made: every step and frame of a script, a cancel's transition among them.
        #[test]
        fn a_graph_costs_nothing_unasked_and_allocates_nothing_when_it_plays() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            let table = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());

            let mut unasked = stage();
            unasked.animate_graph(&table, 1.0);
            assert_eq!(unasked.anim_stats(), AnimStats::default());

            let mut scene = stage();
            scene.play_graph(driver(&table), GraphShow::new(GraphScript::Cancel));
            scene.step();
            scene.animate_graph(&table, 1.0);
            let ((), counts) = sapphire_core::alloc::count_in_scope(|| {
                for _ in 0..70 {
                    scene.step();
                    scene.animate_graph(&ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter()), 0.5);
                }
            });
            if sapphire_core::alloc::PER_THREAD_COUNTS {
                assert_eq!(counts.allocations, 0, "{counts:?}");
            }
        }

        /// Springs under a graph: they run while the clock does and write the pose, a stopped clock
        /// runs none and sends nothing, `--springs off` leaves them out, and a cut starts them over.
        #[test]
        fn a_graphs_springs_run_on_its_clock() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            let table = ClipTable::new([(GRAPH_CLIP, Some(cooked), 0)].into_iter());
            let rig = CharacterRig {
                joint_count: 5,
                springs: vec![Spring {
                    name: "strand".into(),
                    joints: [1, 2]
                        .map(|joint| SpringJoint {
                            joint,
                            hit_radius: 0.01,
                            stiffness: 0.5,
                            gravity_power: 0.1,
                            gravity_dir: [0.0, -1.0, 0.0],
                            drag_force: 0.4,
                        })
                        .into(),
                    tip: SpringTip::Joint,
                    collider_groups: Vec::new(),
                    center: None,
                }],
                ..CharacterRig::default()
            };
            let rig_bytes = rig.to_cooked().expect("cooked");
            let rig = CharacterRig::read(&rig_bytes).expect("a valid rig");
            let sprung = || {
                let driver = driver(&table);
                let springs = SpringBones::new(rig, driver.player.pose());
                assert!(springs.is_some(), "the rig has a chain");
                driver.with_springs(springs)
            };

            let mut scene = stage();
            scene.play_graph(sprung(), GraphShow::new(GraphScript::Hitstop));
            let id = extract::instance_id(scene.character);
            for frame in 0..31 {
                scene.step();
                graph_frame(&mut scene, &table, 1.0, id);
                // Their first frame only takes up where the pose is.
                assert_eq!(scene.anim_stats().spring_substeps > 0, frame > 0, "{:?}", scene.anim_stats());
                assert_eq!(scene.anim_stats().springs, 1, "they write the pose");
            }
            scene.step();
            let drawn = graph_frame(&mut scene, &table, 1.0, id);
            assert_eq!(scene.anim_stats().spring_substeps, 0, "the clock's stopped");
            assert_eq!(drawn.palette, None, "and nothing's sent");

            let mut off = stage();
            off.play_graph(sprung().with_springs_on(false), GraphShow::new(GraphScript::Stand));
            for _ in 0..3 {
                off.step();
                off.animate_graph(&table, 1.0);
                assert_eq!(off.anim_stats().spring_substeps, 0);
            }

            let mut cut = stage();
            cut.play_graph(sprung(), GraphShow::new(GraphScript::Cut));
            for _ in 0..30 {
                cut.step();
                cut.animate_graph(&table, 1.0);
            }
            assert_eq!(cut.anim_stats().teleports, 1, "the script's cut");
            let character = cut.character;
            let driver = cut.ecs.world_mut().get::<GraphDriver>(character).expect("playing");
            assert_eq!(driver.springs_saw(), 1, "the springs started over for it");
        }

        /// A loop of a clip with root motion jumps the character back, and says so; one without has
        /// nothing to jump.
        #[test]
        fn a_clips_loop_is_a_teleport_only_with_root_motion() {
            let bytes = clip().to_cooked().expect("cooked");
            let cooked = Clip::read(&bytes).expect("a valid clip");
            for (root_motion, teleports) in [(true, 2), (false, 0)] {
                let mut scene = stage();
                scene.play_clip(player().with_root_motion(root_motion));
                let id = extract::instance_id(scene.character);
                for _ in 0..150 {
                    scene.step();
                    frame(&mut scene, cooked, 1.0, id);
                }
                assert_eq!(scene.anim_stats().teleports, teleports, "root motion: {root_motion}");
            }
        }

        /// The blade is where the hand is, held or playing, and goes with the character when root
        /// motion moves it. It draws as a character with no body, lit like the one holding it, and
        /// keeps its place through a device loss's new mesh.
        #[test]
        fn the_blade_follows_the_hand_and_adds_no_body() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            let held_hand = Mat4::from_translation(Vec3::new(-0.2, 0.9, 0.1));
            scene.spawn_blade(MeshHandle::from_raw(9), held_hand);
            let entity = scene.blade.expect("in the hand").entity;
            let id = extract::instance_id(entity);
            let character = extract::instance_id(scene.character);
            let transform = |scene: &Scene, entity: Entity| {
                scene.ecs.world().get::<RenderTransform>(entity).expect("it draws").0
            };

            // Held: the held pose's hand.
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            let drawn = packet.instances.changed.iter().find(|instance| instance.id == id).expect("it draws");
            assert!(drawn.transform.abs_diff_eq(held_hand * blade::socket(), 1e-5), "{:?}", drawn.transform);
            let listed = |id| packet.characters.iter().find(|entry| entry.id == id).expect("listed");
            assert!(!listed(id).body.casts(), "it adds no capsule");
            let (own, holder) = (listed(id).lighting.key, listed(character).lighting.key);
            assert!(own.direction.abs_diff_eq(holder.direction, 1e-4), "the two are lit alike");
            scene.end_frame();

            // Playing: the pose's hand, with the root's place.
            let rest = skeleton_pose_rest();
            let hand = 1;
            let joints = FigureJoints::for_test(&rest, 2, 0.0, [3, 4]).with_right_hand(hand);
            let bytes = skeleton().to_cooked().expect("cooked");
            let binds: Vec<SkinBind> = (0..5)
                .map(|joint| SkinBind { joint, inverse_bind_matrix: Mat4::IDENTITY.to_cols_array_2d() })
                .collect();
            let pose =
                CharacterPose::new(Skeleton::read(&bytes).expect("valid"), &binds, &[], vec![0.25, 0.0]);
            scene.play_clip(ClipPlayer::new(pose, joints));
            let mut places = Vec::new();
            for _ in 0..30 {
                scene.step();
                frame(&mut scene, clip, 1.0, id);
                let holder = scene.ecs.world().get::<ClipPlayer>(scene.character).expect("playing");
                let expected = transform(&scene, scene.character)
                    * holder.right_hand().expect("found")
                    * blade::socket();
                let place = transform(&scene, entity);
                assert!(place.abs_diff_eq(expected, 1e-4), "{place:?} against {expected:?}");
                places.push(place.transform_point3(Vec3::ZERO));
            }
            assert!(places[0].distance(places[29]) > 0.5, "root motion carried it: {places:?}");

            // A device loss's new mesh keeps its place.
            let before = transform(&scene, entity);
            scene.set_blade_mesh(MeshHandle::from_raw(10));
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            let drawn =
                packet.instances.changed.iter().find(|instance| instance.id == id).expect("re-meshed");
            assert_eq!(drawn.mesh, MeshHandle::from_raw(10));
            assert!(drawn.transform.abs_diff_eq(before, 1e-5));
        }

        #[test]
        fn a_clip_poses_the_character_each_frame_and_everything_follows_the_skeleton() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(player());
            let id = extract::instance_id(scene.character);

            // The first frame already has the clip's pose, not the pose it was held in.
            let first = frame(&mut scene, clip, 1.0, id);
            let palette = first.palette.clone().expect("posed at once");
            assert_eq!(palette.len(), 5);
            assert_eq!(first.weights, Some(vec![0.25, 0.0]), "the rest weights, and the track's start");
            let rest_head = first.head.expect("the light rig finds the head");
            assert!(first.forward.abs_diff_eq(Vec3::Z, 1e-3), "{}", first.forward);

            for _ in 0..30 {
                scene.step();
            }
            let halfway = frame(&mut scene, clip, 1.0, id);
            assert_ne!(halfway.palette.clone().expect("posed"), palette, "the pose moved");
            let weights = halfway.weights.clone().expect("posed");
            assert_eq!(weights[0], 0.25);
            assert!((weights[1] - 0.5).abs() < 1e-3, "the track's weight: {weights:?}");
            // Root motion moves its transform: 1.5 m of the 3 by now.
            let moved = halfway.moved_to.expect("it moved");
            assert!(moved.abs_diff_eq(Vec3::new(0.0, 0.0, 1.5), 1e-3), "{moved}");
            // The head dropped with the hips and turned: the rig's head and the face's basis, in the
            // world, follow it and the root.
            let head = halfway.head.expect("the head");
            assert!((head.z - moved.z).abs() < 0.2, "{head} with the root at {moved}");
            assert!(head.y < rest_head.y - 0.05, "the head dropped: {head} from {rest_head}");
            let forward = halfway.forward.normalize();
            let eighth = Quat::from_rotation_y(core::f32::consts::FRAC_PI_4) * Vec3::Z;
            let tilt = Quat::from_rotation_x(0.2);
            let expected = (tilt * eighth).normalize();
            assert!(forward.abs_diff_eq(expected, 2e-2), "{forward} against {expected}");
            // The body runs from the feet, under the root, to the head.
            assert!((halfway.body.top - head).length() < 1e-4, "{:?} to {head}", halfway.body);
            assert!(
                (halfway.body.base - Vec3::new(0.0, halfway.body.radius, moved.z)).length() < 0.15,
                "{:?} with the root at {moved}",
                halfway.body
            );

            // Halfway through a step the pose lies halfway between that step's two poses.
            scene.step();
            let at = |scene: &mut Scene, alpha: f32| {
                frame(scene, clip, alpha, id);
                let character = scene.character;
                let transform = scene.ecs.world().get::<RenderTransform>(character).expect("placed");
                transform.0.transform_point3(Vec3::ZERO)
            };
            let (before, after) = (at(&mut scene, 0.0), at(&mut scene, 1.0));
            let middle = at(&mut scene, 0.5);
            assert!(middle.abs_diff_eq((before + after) * 0.5, 1e-4), "{before} {middle} {after}");
        }

        /// Hitstop and pause freeze the pose at its clock's time, and cost nothing: the job doesn't
        /// run, and the pose isn't sent, so the renderer doesn't skin the character either.
        #[test]
        fn a_frozen_clock_holds_the_pose() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(player());
            let id = extract::instance_id(scene.character);
            for _ in 0..10 {
                scene.step();
            }
            frame(&mut scene, clip, 1.0, id);
            assert_eq!(scene.anim_stats().evaluated, 1);

            scene.apply(SimActions { hitstop: true, ..SimActions::default() });
            // The hitstop's first step: time goes on to where it stopped, at most, whatever alpha.
            scene.step();
            let frozen = frame(&mut scene, clip, 0.25, id);
            assert_eq!(frozen.palette, None, "at the time the last frame drew, it doesn't move");
            for alpha in [0.0, 0.5, 1.0] {
                let held = frame(&mut scene, clip, alpha, id);
                assert_eq!(held.palette, None);
                assert_eq!(held.moved_to, None);
                assert_eq!(scene.anim_stats().evaluated, 0);
                assert_eq!(scene.anim_stats().characters, 1);
            }
            for _ in 1..HITSTOP_TICKS {
                scene.step();
                assert_eq!(frame(&mut scene, clip, 0.5, id).palette, None, "still frozen");
            }
            scene.step();
            assert!(frame(&mut scene, clip, 1.0, id).palette.is_some(), "the hitstop ended");

            // The world stopping does the same.
            scene.world_clock_mut().0.push_scale(ScaleSource::Debug, 0).expect("room");
            scene.step();
            frame(&mut scene, clip, 1.0, id);
            scene.step();
            let paused = frame(&mut scene, clip, 0.5, id);
            assert_eq!(paused.palette, None);
            assert_eq!(scene.anim_stats().evaluated, 0);
        }

        /// [`player`], looking at `target` over its clip, its head's joints turning with its neck.
        fn looking_player(target: Vec3) -> ClipPlayer {
            let plain = player();
            let bones = [
                (sapphire_assets::rig::HumanBone::Hips, 0),
                (sapphire_assets::rig::HumanBone::Neck, 1),
                (sapphire_assets::rig::HumanBone::Head, 2),
            ];
            let rig = sapphire_anim::IkRig::new(plain.pose(), &bones, Vec3::ZERO, None, 0.0);
            let ground = Arc::new(IkGround::Flat.world());
            let controls = IkControls { look_target: Some(target), ..IkControls::default() };
            plain.with_ik(IkDriver::new(rig, ground, controls))
        }

        /// Tells the character's IK to look at `target`.
        fn look_at(scene: &mut Scene, target: Vec3) {
            let character = scene.character;
            let world = scene.ecs.world_mut();
            let mut player = world.get_mut::<ClipPlayer>(character).expect("it plays");
            let ik = player.ik_mut().expect("it solves IK");
            let controls = ik.controls();
            ik.set_controls(IkControls { look_target: Some(target), ..controls });
        }

        /// The character's head joint's rotation, as its pose last had it.
        fn head_turn(scene: &Scene) -> Quat {
            let player = scene.ecs.world().get::<ClipPlayer>(scene.character).expect("it plays");
            let (_, rotation, _) = player.pose().model()[2].to_scale_rotation_translation();
            rotation
        }

        /// Hitstop's rule holds with IK on: a frozen clock evaluates nothing and sends nothing, however
        /// the target moves, and when it thaws the head takes up the new target at its limited speed
        /// rather than having snapped behind the freeze's back.
        #[test]
        fn a_frozen_clock_holds_ik_however_its_target_moves() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(looking_player(Vec3::new(3.0, 1.6, 3.0)));
            let id = extract::instance_id(scene.character);
            for _ in 0..30 {
                scene.step();
                frame(&mut scene, clip, 1.0, id);
                assert_eq!(scene.anim_stats().ik, 1);
            }
            scene.apply(SimActions { hitstop: true, ..SimActions::default() });
            scene.step();
            frame(&mut scene, clip, 0.25, id);
            let held = head_turn(&scene);
            for tick in 1..HITSTOP_TICKS {
                look_at(&mut scene, Vec3::new(-3.0 - tick as f32, 1.6, -1.0));
                scene.step();
                let drawn = frame(&mut scene, clip, 0.5, id);
                assert_eq!(drawn.palette, None, "tick {tick}: still frozen, nothing sent");
                assert_eq!(scene.anim_stats().evaluated, 0, "tick {tick}: nothing evaluated");
                assert_eq!(scene.anim_stats().ik, 0);
                assert_eq!(head_turn(&scene), held, "tick {tick}");
            }
            scene.step();
            assert!(frame(&mut scene, clip, 1.0, id).palette.is_some(), "the hitstop ended");
            let settings = sapphire_anim::LookSettings::default();
            // At most a tick's worth of the head's speed, though the clip turned the head as well.
            let turned = head_turn(&scene).angle_between(held);
            assert!(turned < settings.rate / 60.0 + 0.06, "it turned {turned} rad on thawing");
        }

        /// A held position skips evaluation until a clock's running target has something new to say:
        /// a clip of no ticks costs nothing, then follows a moving target, then costs nothing again.
        #[test]
        fn a_target_moving_under_a_held_position_has_its_own_key() {
            let held = Clip {
                name: "held".into(),
                skeleton: Some(AssetId::from_bytes([9; 16])),
                joint_count: 5,
                morph_target_count: 2,
                rate: DEFAULT_RATE,
                ticks: 0,
                ..Clip::default()
            };
            let bytes = held.to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(looking_player(Vec3::new(3.0, 1.6, 3.0)));
            let id = extract::instance_id(scene.character);
            let mut last = 1;
            for _ in 0..240 {
                scene.step();
                frame(&mut scene, clip, 1.0, id);
                last = scene.anim_stats().evaluated;
            }
            assert_eq!(last, 0, "settled on its target, with nothing to do");
            assert_eq!(frame(&mut scene, clip, 1.0, id).palette, None);
            look_at(&mut scene, Vec3::new(-3.0, 1.6, 3.0));
            scene.step();
            assert!(frame(&mut scene, clip, 1.0, id).palette.is_some(), "the target moved");
            assert_eq!(scene.anim_stats().evaluated, 1);
            for _ in 0..240 {
                scene.step();
                frame(&mut scene, clip, 1.0, id);
                last = scene.anim_stats().evaluated;
            }
            assert_eq!(last, 0, "settled again");
        }

        /// IK that has nothing to do leaves a clip's palette exactly as it would be with no IK at all.
        #[test]
        fn ik_with_no_weight_draws_the_same_bytes_as_none() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let run = |player: ClipPlayer| {
                let mut scene = stage();
                scene.play_clip(player);
                let id = extract::instance_id(scene.character);
                let mut palettes = Vec::new();
                for _ in 0..10 {
                    scene.step();
                    palettes.push(frame(&mut scene, clip, 0.5, id).palette);
                }
                palettes
            };
            let mut weightless = looking_player(Vec3::new(3.0, 1.6, 3.0));
            let ik = weightless.ik_mut().expect("it solves IK");
            ik.set_controls(IkControls { feet: 0.0, look: 0.0, ..ik.controls() });
            assert_eq!(run(weightless), run(player()));
        }

        /// The character's figure, which its light rig and face read, follows what IK did.
        #[test]
        fn a_figure_follows_the_solved_head() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let id_of = |scene: &Scene| extract::instance_id(scene.character);
            let run = |player: ClipPlayer| {
                let mut scene = stage();
                scene.play_clip(player);
                let id = id_of(&scene);
                for _ in 0..30 {
                    scene.step();
                    frame(&mut scene, clip, 1.0, id);
                }
                *scene.ecs.world().get::<Figure>(scene.character).expect("it moves")
            };
            let plain = run(player());
            let looking = run(looking_player(Vec3::new(3.0, 1.6, 1.0)));
            assert!(
                looking.face.angle_between(plain.face) > 0.3,
                "{:?} against {:?}",
                looking.face,
                plain.face
            );
        }

        /// A clip's pose and the effects a character owns stop on the same steps: an effect's owner
        /// step is its clock's last delta (`Scene::step_vfx`), and a delta of 0 is also what holds
        /// presented time on the clock's latest step. Here a hit from `--vfx hit` starts the hitstop.
        #[test]
        fn a_hit_stops_the_pose_on_the_steps_that_stop_the_characters_effects() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(player());
            scene.play_vfx(VfxShow::Hit, VfxBudget::DESKTOP_LOW);
            let id = extract::instance_id(scene.character);
            let character = scene.character;
            let mut frozen = 0;
            for tick in 0..120 {
                scene.step();
                let clock = scene.ecs.world().get::<EntityClock>(character).expect("a clock");
                let step = clock.0.last_delta().as_subticks();
                let drawn = frame(&mut scene, clip, 1.0, id);
                assert_eq!(drawn.palette.is_some(), step > 0, "tick {tick}: a step of {step}");
                assert_eq!(scene.anim_stats().evaluated == 1, step > 0, "tick {tick}");
                frozen += u32::from(step == 0);
            }
            assert!(frozen > 0, "the show's hit landed");
        }

        /// A character with no clip is never sampled: a scene without any does no animation work,
        /// and in a scene with one the others keep the pose they have.
        #[test]
        fn pose_work_costs_nothing_for_a_character_with_no_clip() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            for _ in 0..3 {
                scene.step();
                scene.animate_if_playing(Some((clip, 0)), 1.0);
                scene.frame_packet(1.0, Some((1920, 1080)));
                scene.end_frame();
                assert_eq!(scene.anim_stats(), AnimStats::default(), "no player, no work");
            }

            // Another character beside this one holds a pose of its own.
            let still = RenderPose { palette: vec![Affine3A::IDENTITY; 2], morph_weights: Vec::new() };
            let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: Vec3::Y };
            let beside = scene.ecs.world_mut().spawn((RenderMesh(MeshHandle::from_raw(0)), still, rig)).id();
            let beside = extract::instance_id(beside);
            scene.play_clip(player());
            assert!(frame(&mut scene, clip, 1.0, beside).palette.is_some(), "its pose, once");
            for _ in 0..5 {
                scene.step();
                assert_eq!(frame(&mut scene, clip, 1.0, beside).palette, None, "held");
                assert_eq!(scene.anim_stats().characters, 1, "only the one that plays");
                assert_eq!(scene.anim_stats().evaluated, 1);
            }
        }

        /// A character that joins mid-run starts its clip from its own first tick, posed in the
        /// frame it first draws; one that leaves stops costing anything and is reported gone once.
        #[test]
        fn a_character_can_join_and_leave_mid_run() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(player());
            let character = extract::instance_id(scene.character);
            for _ in 0..20 {
                scene.step();
            }
            frame(&mut scene, clip, 1.0, character);

            let clock = scene.ecs.world_mut().resource_mut::<WorldClock>().new_entity_clock();
            let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: Vec3::Y };
            let other = scene
                .ecs
                .world_mut()
                .spawn((RenderMesh(MeshHandle::from_raw(0)), RenderTransform::default(), rig, clock))
                .id();
            scene.start_clip(other, player());
            let id = extract::instance_id(other);
            let joined = frame(&mut scene, clip, 1.0, id);
            assert_eq!(scene.anim_stats().characters, 2);
            let weights = joined.weights.clone().expect("posed in its first frame");
            assert_eq!(weights[1], 0.0, "at its own first tick, not the first character's: {weights:?}");
            assert_eq!(joined.moved_to, Some(Vec3::ZERO), "drawn from where it stood");
            for _ in 0..30 {
                scene.step();
            }
            let later = frame(&mut scene, clip, 1.0, id);
            assert!((later.weights.expect("posed")[1] - 0.5).abs() < 1e-3);

            scene.ecs.world_mut().despawn(other);
            scene.step();
            let gone = frame(&mut scene, clip, 1.0, id);
            assert!(gone.removed, "reported gone");
            assert_eq!(gone.palette, None);
            assert_eq!(scene.anim_stats().characters, 1);
            assert_eq!(scene.anim_stats().evaluated, 1, "the first plays on");
            scene.step();
            assert!(!frame(&mut scene, clip, 1.0, id).removed, "once");
        }

        /// While a new version lands piece by piece, a clip for another skeleton holds the character.
        #[test]
        fn a_clip_for_another_skeleton_holds_the_character() {
            let bytes = Clip { joint_count: 6, ..clip() }.to_cooked().expect("cooked");
            let other = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(player());
            let id = extract::instance_id(scene.character);
            scene.step();
            let held = frame(&mut scene, other, 1.0, id);
            assert_eq!(scene.anim_stats().characters, 1);
            assert_eq!(scene.anim_stats().evaluated, 0);
            assert_eq!(held.moved_to, Some(Vec3::ZERO), "added at the place it stood");
        }

        /// A clip's new version is drawn even by a frozen character.
        #[test]
        fn a_new_version_of_the_clip_moves_a_frozen_character() {
            let bytes = clip().to_cooked().expect("cooked");
            let clip = Clip::read(&bytes).expect("a valid clip");
            let mut scene = stage();
            scene.play_clip(player());
            scene.step();
            scene.animate_clip(Some((clip, 0)), 1.0);
            assert_eq!(scene.anim_stats().evaluated, 1);
            scene.animate_clip(Some((clip, 0)), 1.0);
            assert_eq!(scene.anim_stats().evaluated, 0, "drawn already");
            scene.animate_clip(Some((clip, 1)), 1.0);
            assert_eq!(scene.anim_stats().evaluated, 1, "the new version");
        }
    }

    #[test]
    fn a_timeline_replaces_the_day_only_from_the_days_own_file() {
        let look = || {
            let town = TownMeshes {
                town: MeshHandle::from_raw(1),
                plants: [MeshHandle::from_raw(2); 8],
                mountains: [MeshHandle::from_raw(3); lookdev::MOUNTAINS.len()],
            };
            let pose = RenderPose { palette: Vec::new(), morph_weights: Vec::new() };
            let figure = Figure {
                head: Vec3::Y,
                feet: Vec3::ZERO,
                body_radius: 0.16,
                face: Quat::IDENTITY,
                expression: FaceExpression::default(),
            };
            LookDev { town, fog_cards: None, pose, figure }
        };
        // Two complete timelines that differ, standing for an edit of either file.
        let (default_day, stage_day) = (tod::default_day(), lookdev::day());
        assert_ne!(default_day, stage_day);

        // A scene on the default day: its own file replaces it (by name, from any directory); the
        // look-dev day's doesn't.
        let mut scene = scene();
        assert_eq!(scene.day(), Day::Default);
        let error =
            scene.replace_day("lookdev_day.ron", stage_day.clone()).expect_err("not this scene's day");
        assert!(error.contains("default day (default_day.ron)"), "names the day's file: {error}");
        assert_eq!(scene.time_of_day().timeline(), &default_day, "kept");
        let replaced = scene.replace_day("crates/sapphire_world/default_day.ron", stage_day.clone());
        assert_eq!(replaced, Ok(Day::Default));
        assert_eq!(scene.time_of_day().timeline(), &stage_day);

        // The look-dev stage runs on its own day: the default day's file, which the tool's `watch`
        // pushes whenever it changes, leaves it alone, and the stage's own file replaces it.
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        scene.spawn_look_dev(Shot::Hero, look());
        assert_eq!(scene.day(), Day::LookDev);
        let revision = scene.time_of_day().revision();
        let error =
            scene.replace_day("default_day.ron", default_day.clone()).expect_err("not the stage's day");
        assert!(error.contains("look-dev day (lookdev_day.ron)"), "{error}");
        assert_eq!(scene.time_of_day().timeline(), &stage_day, "the stage keeps its day");
        assert_eq!(scene.time_of_day().revision(), revision, "untouched");
        let replaced = scene.replace_day("content/lookdev/lookdev_day.ron", default_day.clone());
        assert_eq!(replaced, Ok(Day::LookDev));
        assert_eq!(scene.time_of_day().timeline(), &default_day);

        // The stage lights its character by its own light rigs (M1 C8), which the shipped
        // presets' file, which `watch` pushes too, leaves alone.
        let rigs = |scene: &Scene| scene.ecs.world().resource::<LightRigLibrary>().clone();
        assert_eq!(rigs(&scene), lookdev::light_rigs());
        let shipped = LightRigLibrary::default_rigs();
        let error =
            scene.replace_light_rigs("default_light_rigs.ron", shipped.clone()).expect_err("not the stage's");
        assert!(error.contains("lookdev_light_rigs.ron"), "{error}");
        assert_eq!(rigs(&scene), lookdev::light_rigs(), "kept");
        let replaced = scene.replace_light_rigs("content/lookdev/lookdev_light_rigs.ron", shipped.clone());
        assert_eq!((replaced, rigs(&scene)), (Ok(Day::LookDev), shipped.clone()));
        let mut plain = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        assert_eq!(rigs(&plain), shipped, "elsewhere, the shipped presets");
        plain
            .replace_light_rigs("lookdev_light_rigs.ron", lookdev::light_rigs())
            .expect_err("not this scene's");
        let replaced =
            plain.replace_light_rigs("crates/sapphire_world/default_light_rigs.ron", lookdev::light_rigs());
        assert_eq!(replaced, Ok(Day::Default));
    }

    #[test]
    fn shown_time_is_where_transforms_draw_and_never_steps_back() {
        // Review WA-3: clocks ran up to a tick ahead of the latest step, by the last step's delta,
        // so when that delta shrank, on a pause or into slow motion, shown time stepped back.
        fn shown(scene: &mut Scene, alpha: f32) -> f64 {
            let seconds = scene.frame_packet(alpha, Some((1920, 1080))).time_seconds;
            scene.end_frame();
            seconds
        }
        let tick = 1.0 / f64::from(TICK_HZ);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let mut scene = scene();
        for _ in 0..10 {
            scene.step();
        }
        assert!(close(shown(&mut scene, 1.0), 10.0 * tick), "the latest step");
        assert!(close(shown(&mut scene, 0.0), 9.0 * tick), "the one before");
        assert!(close(shown(&mut scene, 0.5), 9.5 * tick));

        let before = shown(&mut scene, 0.9);
        let pause = scene.world_clock_mut().0.push_scale(ScaleSource::Pause, 0).expect("room");
        scene.step();
        for alpha in [0.0, 0.5, 1.0] {
            let paused = shown(&mut scene, alpha);
            assert!(paused >= before, "paused at alpha {alpha}: {paused} after {before}");
            assert!(close(paused, 10.0 * tick), "and it holds on the latest step: {paused}");
        }
        scene.world_clock_mut().0.pop_scale(pause);

        scene.step();
        let before = shown(&mut scene, 0.9);
        scene.world_clock_mut().0.push_scale(ScaleSource::PerfectDodge, 100).expect("room");
        scene.step();
        let slowed = shown(&mut scene, 0.0);
        assert!(slowed >= before, "slowed: {slowed} after {before}");
    }

    #[test]
    fn time_of_day_runs_on_the_world_clock() {
        let mut scene = scene();
        scene.set_day(18.0, Some(24.0));
        for _ in 0..TICK_HZ {
            scene.step();
        }
        let hours = |scene: &Scene| scene.time_of_day().clock().hours(1.0);
        assert!((hours(&scene) - 19.0).abs() < 1e-4, "an hour a second");
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert!(packet.day_night > 0.0, "past sunset");
        scene.end_frame();
        scene.apply(SimActions { toggle_slow_motion: true, ..SimActions::default() });
        for _ in 0..TICK_HZ {
            scene.step();
        }
        assert!((hours(&scene) - 19.2).abs() < 1e-4, "slowed with the world");
    }

    #[test]
    fn a_day_that_keys_the_emission_reaches_the_packet() {
        let mut scene = scene();
        let emission = |scene: &mut Scene, hours: f32| {
            scene.set_day(hours, None);
            scene.step();
            let scale = scene.frame_packet(1.0, Some((1920, 1080))).emission_scale;
            scene.end_frame();
            scale
        };
        assert_eq!(emission(&mut scene, 13.0), 1.0, "the default day shows emission as authored");
        // An edited day replaces it as hot reload and live link apply one
        // (`reload::Document::DefaultDay`): lanterns a tenth as bright from sunrise to sunset
        // (M1 W1's keyed emission).
        let day = tod::default_day();
        let keys = day.keys().iter().map(|key| {
            let scale = if (6.5..=18.75).contains(&key.time) { 0.1 } else { 1.0 };
            tod::TodKey { emission: Some(scale), ..key.clone() }
        });
        let keyed = tod::TodTimeline::new(keys.collect()).expect("valid");
        scene.time_of_day_mut().set_timeline(keyed).expect("complete");
        assert_eq!(emission(&mut scene, 13.0), 0.1, "dim by day");
        assert_eq!(emission(&mut scene, 23.0), 1.0, "full at night");
    }

    #[test]
    fn the_character_is_lit_by_its_light_rig() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let character = InstanceId(scene.character.to_bits());
        scene.step();
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        let [lit] = packet.characters[..] else { panic!("one character: {:?}", packet.characters) };
        assert_eq!(lit.id, character);
        // At the default hour the gameplay preset's key is the sun the sandbox always drew with,
        // in time of day's character key, which equals it at 10:00.
        assert_eq!(lit.lighting.key, SUN);
        assert_eq!(lit.lighting.gi.head, Some(Figure::MANNEQUIN.head), "on the turntable's axis");
        let rigs = LightRigLibrary::default_rigs();
        assert_eq!(lit.lighting.gi.camera_bias, rigs.rig(LightRigId::DEFAULT).gi.camera_bias);
        scene.end_frame();

        // A portrait key holds to the camera, wherever the sun is.
        scene.set_character_rig("portrait").expect("shipped");
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_ne!(packet.characters[0].lighting.key.direction, SUN.direction);
        scene.end_frame();
        let error = scene.set_character_rig("noir").unwrap_err();
        assert!(error.contains("gameplay, portrait"), "{error}");

        // Without a view there is no camera to resolve the rig for, and nothing draws.
        assert!(scene.frame_packet(1.0, None).characters.is_empty());
        scene.end_frame();
    }

    #[test]
    fn the_sky_material_and_cloud_cover_reach_the_packet() {
        let mut scene = scene();
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_eq!(packet.sky_material, None, "none until loaded");
        assert!(packet.sky_layers.clouds.coverage > 0.0, "the default day's clouds at 10:00");
        scene.end_frame();
        scene.set_sky_material(MaterialHandle::from_raw(4));
        scene.set_cloud_cover(Some(0.0));
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_eq!(packet.sky_material, Some(MaterialHandle::from_raw(4)));
        assert_eq!(
            (packet.sky_layers.clouds.coverage, packet.sky_layers.clouds.cirrus),
            (0.0, 0.0),
            "a clear sky"
        );
        assert_eq!(packet.sky, sapphire_render::SkyGradient::default(), "the gradient is time of day's");
        scene.end_frame();
        scene.set_day(22.0, None);
        scene.set_plain_sky(true);
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_eq!(packet.sky_layers, SkyLayers::default(), "no stars or moon at night either");
        scene.end_frame();
    }

    #[test]
    fn plain_post_drops_the_compositing_layers_and_sharpening_reaches_the_packet() {
        let mut scene = scene();
        scene.set_day(18.5, None);
        let layered = scene.frame_packet(1.0, Some((1920, 1080))).post;
        assert!(layered.diffusion.intensity > 0.0, "the default day's sunset glows");
        assert_eq!(layered.sharpening, 0.0, "off by default");
        scene.end_frame();
        scene.set_sharpening(Some(0.5));
        assert_eq!(scene.frame_packet(1.0, Some((1920, 1080))).post.sharpening, 0.5);
        scene.end_frame();
        scene.set_plain_post(true);
        let plain = scene.frame_packet(1.0, Some((1920, 1080))).post;
        assert_eq!(plain, layered.plain(), "time of day's post without the layers");
        scene.end_frame();
    }

    #[test]
    fn the_face_rig_lists_each_head_lit_from_its_own_angles() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        scene.spawn_face_rig(MeshHandle::from_raw(1), FaceLayout::Lights);
        let heads: Vec<_> = scene.faces.iter().map(|head| InstanceId(head.to_bits())).collect();
        let count = FaceLayout::Lights.heads();
        let (turntable, turntable_id) = (scene.character, InstanceId(scene.character.to_bits()));
        for frame in 0..2 {
            scene.step();
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            assert_eq!(packet.characters.len(), count + 1, "the heads and the turntable");
            let added =
                packet.instances.changed.iter().filter(|instance| heads.contains(&instance.id)).count();
            assert_eq!(added, if frame == 0 { count } else { 0 }, "the heads are extracted once");
            let rigs = FaceLayout::Lights.rows().iter().flat_map(|row| row.iter());
            for (&id, rig) in heads.iter().zip(rigs) {
                let character = packet.characters.iter().find(|c| c.id == id).expect("listed every frame");
                assert!(character.is_valid());
                let (forward, up) = character.face.head.orthonormalized().expect("a basis");
                let toward = -character.lighting.key.direction.normalize();
                assert!(toward.abs_diff_eq(rig.toward_key(forward, up), 1e-5), "{rig:?}");
                assert_eq!(character.face.expression, rig.expression);
                assert!(character.lighting.gi.head.is_some(), "the GI spherizes around the head");
            }
            // The turntable's head turns with it.
            let head = packet.characters.iter().find(|c| c.id == turntable_id).expect("listed").face.head;
            let spun = scene.ecs.world().get::<RenderTransform>(turntable).expect("placed").0;
            assert!(head.forward.abs_diff_eq(spun.transform_vector3(Vec3::Z), 1e-6));
            scene.end_frame();
        }
    }

    #[test]
    fn the_gi_face_layouts_light_the_heads_by_their_gi_alone() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        scene.spawn_face_rig(MeshHandle::from_raw(1), FaceLayout::GiClamped);
        let heads: Vec<_> = scene.faces.iter().map(|head| InstanceId(head.to_bits())).collect();
        let turntable = InstanceId(scene.character.to_bits());
        scene.step();
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        let preset = LightRigLibrary::default_rigs().rig(LightRigId::DEFAULT).gi;
        for id in &heads {
            let lighting = packet.characters.iter().find(|c| c.id == *id).expect("listed").lighting;
            assert_eq!(lighting.key.intensity, 0.0, "the key light is off");
            assert_eq!(lighting.gi.intensity, preset.intensity * faces::GI_EXPOSURE);
            let clamp = (lighting.gi.max_saturation, lighting.gi.max_value);
            assert_eq!(clamp, (faces::CLAMPED_GI.saturation, faces::CLAMPED_GI.value));
            assert_eq!(
                (lighting.gi.camera_bias, lighting.gi.skin_tint),
                (preset.camera_bias, preset.skin_tint.into())
            );
        }
        let character = packet.characters.iter().find(|c| c.id == turntable).expect("listed");
        assert!(character.lighting.key.intensity > 0.0, "the turntable keeps its key light");
        scene.end_frame();
    }

    #[test]
    fn screen_effects_reach_the_packet_and_end() {
        let mut scene = scene();
        scene.step();
        let effects = |scene: &mut Scene| scene.frame_packet(1.0, Some((64, 36))).post.effects;
        assert_eq!(effects(&mut scene), sapphire_render::PostEffects::NONE, "an empty stack draws nothing");
        scene.end_frame();

        let error = scene.push_effect("impact", None).expect_err("no such preset");
        assert!(error.contains("impact_frame"), "names the presets: {error}");

        // Held, the impact frame stays on its frame however long the scene runs, plain post or not.
        scene.push_effect("impact_frame", Some(0.03)).expect("a sample preset");
        scene.set_plain_post(true);
        let held = effects(&mut scene);
        assert!(
            held.duotone.amount > 0.0 && held.speed_lines.opacity > 0.0 && held.radial_blur.strength > 0.0
        );
        scene.end_frame();
        for _ in 0..TICK_HZ {
            scene.step();
        }
        assert_eq!(effects(&mut scene), held);
        scene.end_frame();

        // Played, the radial blur rises, then ends within half a second, leaving the held one.
        scene.with_screen_effects(|stack, _| stack.clear());
        scene.push_effect("radial_blur", None).expect("a sample preset");
        scene.step();
        assert!(effects(&mut scene).radial_blur.strength > 0.0);
        scene.end_frame();
        for _ in 0..TICK_HZ / 2 {
            scene.step();
        }
        assert_eq!(effects(&mut scene), sapphire_render::PostEffects::NONE);
        scene.end_frame();
    }

    /// G3's recording, from its start.
    fn recorded_fight() -> crate::combat::Combat {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/fight.sreplay");
        let option = crate::options::CombatOption::Replay { path, to_end: true };
        crate::combat::Combat::new(&option).expect("a committed recording")
    }

    /// A recorded fight fired as `--cues` does: the frame the first hit lands in draws its spark and its
    /// screen effect and fires its shake, which moves the view from the next, the frames before it draw
    /// none of them, and the simulation still reaches its recorded checksums.
    #[test]
    fn a_fights_first_hit_reaches_the_frame_it_lands_in() {
        let mut combat = recorded_fight();
        let mut scene = scene();
        scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
        let view_at =
            |scene: &Scene| scene.ecs.world().resource::<OrbitCamera>().camera_data(16.0 / 9.0).view;
        let mut landed = false;
        for _ in 0..200 {
            scene.step();
            let still = view_at(&scene);
            combat.step(&mut crate::combat::no_bindings);
            scene.show_cues(combat.simulation());
            let hit = combat.simulation().cues().any(|cue| cue.hit.is_some());
            scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
            let shaken = view_at(&scene) != still;
            let packet = scene.frame_packet(1.0, Some((640, 360)));
            let (spark, effects) = (!packet.vfx.is_empty(), packet.post.effects);
            scene.end_frame();
            if hit {
                assert!(spark, "the hit's spark");
                assert_ne!(effects, sapphire_render::PostEffects::NONE, "the hit's screen effect");
                assert!(!shaken, "its shake is fired, and reads from where it begins, which is nothing");
                scene.step();
                scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
                assert!(view_at(&scene) != still, "the hit's shake, a frame on");
                landed = true;
                break;
            }
            assert_eq!(effects, sapphire_render::PostEffects::NONE, "nothing's landed yet");
        }
        assert!(landed, "the fight's first hit comes within 200 ticks");
        combat.check().expect("a fight fired is the fight recorded");
    }

    /// What a fight set going ends with it. A spark in the air, a screen effect, a shake and a hit's number
    /// all belong to where the last fight's actors stood, so the new fight's first frame, the one the
    /// view cuts on, shows none of them.
    #[test]
    fn a_new_fight_shows_nothing_the_last_one_set_going() {
        use crate::hud::{HudMode, HudOptions};

        let option = crate::options::CombatOption::Fight { script: Some("idle".to_owned()), party: false };
        let mut combat = crate::combat::Combat::new(&option).expect("sets up");
        let cast = combat.cast().to_vec();
        let (mut scene, _) = arena_stage(ArenaShot::Director);
        scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
        scene.set_hud(SandboxHud::for_fight(HudOptions { mode: HudMode::Live, ..HudOptions::default() }));
        let step = |scene: &mut Scene, combat: &mut crate::combat::Combat| {
            combat.step(&mut crate::combat::no_bindings);
            scene.follow_simulation(combat.simulation(), &cast);
            scene.show_cues(combat.simulation());
            scene.show_damage(combat.simulation());
            scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
        };
        // How far the view's right has left level, whether the frame draws no effects, its screen
        // effects, and the numbers its HUD draws.
        let frame = |scene: &mut Scene| {
            scene.drive_arena_camera(1.0 / TICK_HZ as f32, sapphire_math::Vec2::ZERO, 0.0);
            let packet = scene.frame_packet(1.0, Some((640, 360)));
            let view = packet.main_view().expect("a view").camera.view;
            let shown = (view.inverse().x_axis.y.abs(), packet.vfx.is_empty(), packet.post.effects);
            let (hud, _) = scene.hud().expect("set").last_frame();
            scene.end_frame();
            (shown, hud.damage_numbers)
        };
        while !combat.simulation().cues().any(|cue| cue.hit.is_some()) {
            let _ = frame(&mut scene);
            step(&mut scene, &mut combat);
            assert!(combat.simulation().tick() < 2000, "someone lands a hit by now");
        }
        let director = scene.arena.as_mut().expect("built").director().expect("this shot has one");
        let finish = director.tuning().shake_id("finish").expect("a shipped preset");
        director.fire_shake(finish, 1.0);
        let ((_, empty, effects), numbers) = frame(&mut scene);
        assert!(!empty, "the spark's up");
        assert_ne!(effects, sapphire_render::PostEffects::NONE, "the screen effect's up");
        assert!(numbers > 0, "the number's up");
        let most = (0..6).map(|_| frame(&mut scene).0.0).fold(0.0_f32, f32::max);
        assert!(most > 1e-3, "the shake's up: {most}");

        combat.restart();
        step(&mut scene, &mut combat);
        assert_eq!(combat.simulation().tick(), 1, "the new fight's first step");
        let ((tilt, empty, effects), numbers) = frame(&mut scene);
        assert!(empty, "no spark of the last fight");
        assert_eq!(effects, sapphire_render::PostEffects::NONE, "no screen effect of the last fight");
        assert!(tilt < 1e-5, "no shake of the last fight: {tilt}");
        assert_eq!(numbers, 0, "no number of the last fight");
    }

    /// A step's cues stand until the next step, and a recording stopped at its limit or a replay past its
    /// end takes none, so the frames after it read the same hits. They show once, and a new fight's show
    /// whatever tick they land on.
    #[test]
    fn a_steps_hits_show_once_however_often_theyre_read() {
        use crate::hud::{HudMode, HudOptions};

        let option = crate::options::CombatOption::Fight { script: Some("idle".to_owned()), party: false };
        let mut combat = crate::combat::Combat::new(&option).expect("sets up");
        let cast = combat.cast().to_vec();
        let (mut scene, _) = arena_stage(ArenaShot::Director);
        scene.set_hud(SandboxHud::for_fight(HudOptions { mode: HudMode::Live, ..HudOptions::default() }));
        let numbers = |scene: &mut Scene| {
            scene.drive_arena_camera(1.0 / TICK_HZ as f32, sapphire_math::Vec2::ZERO, 0.0);
            let _ = scene.frame_packet(1.0, Some((640, 360)));
            let (hud, _) = scene.hud().expect("set").last_frame();
            scene.end_frame();
            hud.damage_numbers
        };
        while !combat.simulation().cues().any(|cue| cue.hit.is_some()) {
            let _ = numbers(&mut scene);
            combat.step(&mut crate::combat::no_bindings);
            scene.follow_simulation(combat.simulation(), &cast);
            assert!(combat.simulation().tick() < 2000, "someone lands a hit by now");
        }

        scene.show_damage(combat.simulation());
        let once = numbers(&mut scene);
        assert!(once > 0, "the number's up");
        for _ in 0..5 {
            scene.show_damage(combat.simulation());
        }
        assert_eq!(numbers(&mut scene), once, "the same tick's hits again");

        // A script's fight lands the same hit on the same tick, and it's a new one.
        let tick = combat.simulation().tick();
        combat.restart();
        combat.step(&mut crate::combat::no_bindings);
        scene.follow_simulation(combat.simulation(), &cast);
        while combat.simulation().tick() < tick {
            let _ = numbers(&mut scene);
            combat.step(&mut crate::combat::no_bindings);
            scene.follow_simulation(combat.simulation(), &cast);
        }
        scene.show_damage(combat.simulation());
        assert_eq!(numbers(&mut scene), once, "the new fight's");
    }

    /// A step's firing allocates nothing once a whole fight has warmed it (rule R6). The simulation's
    /// own steps are left out: a replay's trail of checksums grows.
    #[test]
    fn a_fights_cues_do_not_allocate_once_warmed() {
        let mut scene = scene();
        scene.play_cues(VfxBudget::DESKTOP_LOW).expect("the shipped table resolves");
        let mut fight = || {
            let mut combat = recorded_fight();
            let mut allocations = 0;
            for _ in 0..1500 {
                scene.step();
                combat.step(&mut crate::combat::no_bindings);
                let ((), fired) = sapphire_core::alloc::count_in_scope(|| {
                    scene.show_cues(combat.simulation());
                    scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
                });
                allocations += fired.allocations;
                let _ = scene.frame_packet(1.0, Some((640, 360)));
                scene.end_frame();
            }
            allocations
        };
        let _ = fight();
        assert_eq!(fight(), 0, "rule R6");
    }

    /// Without `--cues`, a fight stepping beside the scene fires nothing, whatever it publishes.
    #[test]
    fn a_fight_without_the_flag_fires_nothing() {
        let mut combat = recorded_fight();
        let mut scene = scene();
        for _ in 0..200 {
            scene.step();
            combat.step(&mut crate::combat::no_bindings);
            scene.show_cues(combat.simulation());
            scene.end_cue_frame(1.0 / TICK_HZ as f32, 1.0 / TICK_HZ as f32, 1.0, &mut ());
            let packet = scene.frame_packet(1.0, Some((640, 360)));
            assert!(packet.vfx.is_empty());
            assert_eq!(packet.post.effects, sapphire_render::PostEffects::NONE);
            scene.end_frame();
        }
        assert!(scene.cue_show().is_none());
        assert!(scene.cue_sounds().requests.is_empty());
    }

    #[test]
    fn the_scene_asks_for_the_effect_variants_it_may_push() {
        // The overlay fires any preset in `dev` builds; otherwise only `--post-effect` pushes.
        let mut scene = scene();
        let every = if cfg!(feature = "dev") { PostVariants::SCREEN_EFFECTS } else { PostVariants::NONE };
        assert_eq!(scene.effect_variants(), every);
        scene.push_effect("radial_blur", None).expect("a sample preset");
        let lens = PostVariants { lens_effects: true, ..PostVariants::NONE };
        assert_eq!(scene.effect_variants(), every | lens);
        scene.push_effect("inversion", Some(0.0)).expect("a sample preset");
        assert_eq!(scene.effect_variants(), PostVariants::SCREEN_EFFECTS);
    }

    #[test]
    fn the_main_view_needs_a_drawable_target() {
        assert_eq!(drawable_aspect(Some((1920, 1080))), Some(1920.0 / 1080.0));
        for size in [None, Some((0, 1080)), Some((1920, 0))] {
            assert_eq!(drawable_aspect(size), None, "{size:?}");
        }
    }

    #[test]
    fn packets_carry_the_view_and_only_instances_that_moved() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let character = InstanceId(scene.character.to_bits());
        let changed = |packet: &FramePacket| -> Vec<InstanceId> {
            packet.instances.changed.iter().map(|instance| instance.id).collect()
        };

        scene.step();
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_eq!(packet.views.len(), 1);
        // At the default hour, time of day draws the sandbox's look from before it existed, with
        // post's compositing layers on top.
        assert_eq!(packet.sun, SUN);
        assert_eq!(packet.sky, sapphire_render::SkyGradient::default());
        assert_eq!(packet.post.plain(), sapphire_render::PostParams::default());
        assert_ne!(packet.post, packet.post.plain(), "the default day's compositing layers");
        assert!(packet.lights.is_empty(), "no demo lights unless asked for");
        assert_eq!(changed(packet), [character], "added");
        let first = packet.instances.changed[0].transform;
        scene.end_frame();

        // The turntable spins, so it changes every frame, drawn or not.
        scene.step();
        let packet = scene.frame_packet(1.0, None);
        assert!(packet.views.is_empty(), "no target, no view");
        assert_eq!(changed(packet), [character]);
        assert_ne!(packet.instances.changed[0].transform, first);
        scene.end_frame();

        // Frozen by hitstop, it holds still and drops out of the packets.
        let world = scene.ecs.world_mut();
        world.get_mut::<EntityClock>(scene.character).expect("clock").0.apply_hitstop(2);
        scene.step();
        assert!(scene.frame_packet(1.0, Some((1920, 1080))).instances.is_empty());
        scene.end_frame();

        // A renderer that starts over gets it again.
        scene.resend_instances();
        assert_eq!(changed(scene.frame_packet(1.0, Some((1920, 1080)))), [character]);
        scene.end_frame();
        assert!(scene.frame_packet(1.0, Some((1920, 1080))).instances.is_empty());
    }
}
