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
//!   ([`Scene::spawn_look_dev`], [`crate::lookdev`]).
//! - **Systems:** time of day and the screen effects tick in `FixedFirst`, after the clocks;
//!   turntables spin, and
//!   stalks sway, in `FixedCombat`. `Extract` poses them,
//!   then writes the instances that changed, the lights and the poses that changed into the
//!   frame's packet (see [`crate::extract`]). The stalk's systems are added only with `--skinned`,
//!   since each system in `Extract` costs an allocation per frame (below).
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
//!   or the proximity fade by the camera's distance to their bodies (`--proximity-fade`). Both
//!   run there rather than as `Extract` systems, which would cost an allocation a frame each.
//!
//! `Presentation` has no systems yet. Posing is presentation work, but a multi-threaded
//! schedule that has systems costs an allocation per run plus one per system, and wakes a
//! worker; an empty one costs nothing. In `Presentation` the pose cost 2 allocations and about
//! 8 µs more per frame (`--bench`, M5), so it runs at the head of `Extract`, which runs anyway.
//! It moves to `Presentation` once animation gives that schedule real work (M2).
//!
//! Queued input is applied by [`Scene::apply`] just before a frame's fixed steps, outside them:
//! it logs, and logging allocates, which the fixed step must not (rule R6).

use sapphire_assets::mesh::ArchivedMesh;
use sapphire_ecs::bevy_ecs::system::SystemState;
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat4, Quat, Vec3};
use sapphire_render::{
    CharacterBody, CharacterFace, CharacterInstance, Fog, FramePacket, HeadBasis, LightKind, LocalLight,
    MaterialHandle, MeshHandle, PostVariants, SkyLayers, ViewData, Wind,
};
use sapphire_time::{ScaleHandle, ScaleSource};
use sapphire_world::fade::ProximityFade;
use sapphire_world::light_rig::{CharacterLightRig, LightRigId, LightRigLibrary};
use sapphire_world::screen_effects::{EffectLibrary, EffectStack, tick_screen_effects};
use sapphire_world::tod::{TimeOfDay, TodTimeline, tick_time_of_day};

use crate::LOG_TARGET;
use crate::environment;
use crate::extract::{self, RenderExtract, RenderLight, RenderMesh, RenderPose, RenderTransform};
use crate::faces::{self, FaceLayout, FaceRig};
use crate::foliage::{self, FoliageMeshes, FoliageView, Plant};
use crate::input::SimActions;
use crate::layers::{self, SAMPLES};
use crate::lookdev::{self, Shot, TownMeshes};
use crate::orbit::OrbitCamera;
use crate::outlines::{self, ROW};
use crate::rims::{self, Panel, RimKey, RimLayout, RimRigMeshes};
use crate::skinned::{self, Sway};
use crate::stage::{self, Piece, StageLayout, StageMeshes, Subject};
use crate::swatches::{self, SWATCHES};
use crate::turntable::{self, Turntable};

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
    (Entity, &'static CharacterLightRig, &'static RenderTransform, Option<&'static RimKey>),
    With<RenderMesh>,
>;

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

/// Where the character's light rig finds its head and body, in the character's own space.
#[derive(Clone, Copy, Debug, PartialEq)]
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
}

impl Figure {
    /// The sample mannequin (`sapphire_pipeline`'s samples): its head sphere 1.47 m above its
    /// 0.2 m pedestal, its torso's radius, its head turning with it.
    pub const MANNEQUIN: Self = Self {
        head: Vec3::new(0.0, 1.67, 0.0),
        feet: Vec3::new(0.0, 0.2, 0.0),
        body_radius: 0.24,
        face: Quat::IDENTITY,
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
    /// The look-dev town, with `--look-dev`, and its plants.
    town: Option<Entity>,
    town_plants: Vec<(Entity, lookdev::Plant)>,
    /// Where the character's light rig finds its head and body.
    figure: Figure,
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
    /// Time of day's fog, unless `--fog off` (M1 W3).
    fog: bool,
    /// Post without its compositing layers (`--plain-post`).
    plain_post: bool,
    /// The player's sharpening instead of the cvar's (`--sharpening`).
    sharpening: Option<f32>,
    /// How the characters fade (`--fade`, `--proximity-fade`).
    fade: Fade,
    /// The post variants of the presets [`Self::push_effect`] pushed (`--post-effect`).
    pushed_effects: PostVariants,
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
        let time_of_day = TimeOfDay::new(TodTimeline::default_day(), day_clock, DEFAULT_TIME_OF_DAY)
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
        Self {
            ecs,
            character,
            stalk,
            swatches: None,
            layer_samples: None,
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
            figure,
            poses: skinned,
            bounds,
            characters,
            heads,
            screen_effects,
            lamps,
            sky_material: None,
            cloud_cover: None,
            plain_sky: false,
            fog: true,
            plain_post: false,
            sharpening: None,
            fade: Fade::Off,
            pushed_effects: PostVariants::NONE,
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
    /// layers they were pushed with.
    #[cfg(feature = "dev")]
    pub fn set_effect_library(&mut self, library: EffectLibrary) {
        self.ecs.world_mut().insert_resource(library);
    }

    /// Replaces the character light rig presets (hot reload, `dev` builds). Characters keep their
    /// preset IDs; one past the new presets' end is lit with their default.
    #[cfg(feature = "dev")]
    pub fn set_light_rigs(&mut self, rigs: LightRigLibrary) {
        self.ecs.world_mut().insert_resource(rigs);
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

    /// Makes `Extract` write poses from now on, once: scenes without any pose don't run it.
    fn extract_poses(&mut self) {
        if !self.poses {
            self.ecs.add_systems(Extract, extract::extract_poses.after(extract::extract_lights));
            self.poses = true;
        }
    }

    /// Builds the look-dev stage (`--look-dev`, [`crate::lookdev`]): the town and its plants
    /// around the character, its fog cards and lamps, the character held in `look.pose` (its
    /// turntable stopped, its light rig finding its head and body by `look.figure`) and the
    /// `shot`'s camera. Time of day runs on the stage's own day ([`lookdev::day`]) and the wind
    /// blows the stage's breeze. The town, its plants and its cards never move, so they are
    /// extracted once.
    pub fn spawn_look_dev(&mut self, shot: Shot, look: LookDev) {
        self.figure = look.figure;
        self.extract_poses();
        self.time_of_day_mut().set_timeline(lookdev::day()).expect("the look-dev day sets every group");
        self.wind = lookdev::WIND;
        let world = self.ecs.world_mut();
        world.insert_resource(shot.camera());
        let mut character = world.entity_mut(self.character);
        character.insert(look.pose);
        if let Some(mut turntable) = character.get_mut::<Turntable>() {
            turntable.speed = 0.0;
        }
        if let Some(mut rig) = character.get_mut::<CharacterLightRig>() {
            rig.head = look.figure.head;
        }
        self.town = Some(world.spawn((RenderMesh(look.town.town), RenderTransform(Mat4::IDENTITY))).id());
        self.town_plants = lookdev::plantings()
            .map(|(plant, place)| {
                (world.spawn((RenderMesh(look.town.plant(plant)), RenderTransform(place))).id(), plant)
            })
            .collect();
        for light in lookdev::lamps() {
            world.spawn((Lamp { intensity: light.intensity }, RenderLight(light)));
        }
        if let Some(meshes) = look.fog_cards {
            self.fog_cards = environment::place_cards(&lookdev::CARDS)
                .map(|(mesh, place)| {
                    (world.spawn((RenderMesh(meshes[mesh]), RenderTransform(place))).id(), mesh)
                })
                .collect();
        }
    }

    /// Sets the meshes the look-dev town and its plants draw again: after GPU device loss.
    pub fn set_town_mesh(&mut self, meshes: TownMeshes) {
        if let Some(town) = self.town {
            self.ecs.world_mut().entity_mut(town).insert(RenderMesh(meshes.town));
        }
        for &(entity, plant) in &self.town_plants {
            self.ecs.world_mut().entity_mut(entity).insert(RenderMesh(meshes.plant(plant)));
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

    /// One fixed simulation step. Must not allocate (rule R6).
    pub fn step(&mut self) {
        self.ecs.fixed_step();
    }

    /// Runs presentation and extraction, then returns this frame's packet, `alpha` of the way
    /// from the previous fixed step to the latest, with the main view drawing into a target of
    /// `target_size`. Call it every frame: on frames that draw nothing, pass `None`; their
    /// packets have no view but still carry instance deltas, which the renderer must see.
    pub fn frame_packet(&mut self, alpha: f32, target_size: Option<(u32, u32)>) -> &FramePacket {
        self.ecs.presentation(alpha);
        let (sky_material, cloud_cover, plain_sky) = (self.sky_material, self.cloud_cover, self.plain_sky);
        let fog = self.fog;
        let (plain_post, sharpening, wind) = (self.plain_post, self.sharpening, self.wind);
        let probe_hours = &self.probe_hours;
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.begin();
            let camera = world.resource::<OrbitCamera>();
            if let Some(aspect) = drawable_aspect(target_size) {
                packet.views.push(ViewData::new(camera.camera_data(aspect)));
            }
            let probe = camera.eye();
            let clock = &world.resource::<WorldClock>().0;
            // Interpolated like everything else drawn this frame.
            packet.time_seconds =
                clock.now().as_secs_f64() + f64::from(alpha) * f64::from(clock.last_delta().as_secs_f32());
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
        let figure = self.figure;
        let (characters, heads, face_layout, fade) =
            (&mut self.characters, &mut self.heads, self.face_layout, self.fade);
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.packet_mut();
            // The rigs are resolved for the camera; without one nothing draws.
            let Some(camera) = packet.main_view().map(|view| view.camera) else { return };
            let frame = world.resource::<TimeOfDay>().frame();
            let rigs = world.resource::<LightRigLibrary>();
            for (entity, character, transform, key) in characters.iter(world) {
                let head = transform.0.transform_point3(character.head);
                let mut lighting = frame.character_lighting(rigs.rig(character.rig), &camera, head);
                // A rim rig figure's key light turns to its angles around it.
                if let Some(key) = key {
                    lighting.key.direction = -key.toward(camera.position, head);
                }
                // The head turns with the model, and by the figure's held pose.
                let pose = HeadBasis {
                    forward: transform.0.transform_vector3(figure.face * Vec3::Z),
                    up: transform.0.transform_vector3(figure.face * Vec3::Y),
                };
                let face = CharacterFace { head: pose, ..CharacterFace::default() };
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
        self.ecs.world().resource::<RenderExtract>().packet()
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

    pub fn camera_mut(&mut self) -> Mut<'_, OrbitCamera> {
        self.ecs.world_mut().resource_mut()
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
    pub fn replace_day(&mut self, origin: &str, timeline: TodTimeline) -> Result<Day, String> {
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
mod tests {
    use sapphire_render::InstanceId;
    use sapphire_time::TICK_HZ;

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

    #[test]
    fn the_look_dev_stage_holds_the_pose_and_lights_its_lamps_by_night() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        let lifted = sapphire_math::Affine3A::from_translation(Vec3::Y);
        let pose = RenderPose { palette: vec![lifted; 2], morph_weights: vec![0.5] };
        let figure = Figure {
            head: Vec3::new(0.0, 1.45, 0.0),
            feet: Vec3::ZERO,
            body_radius: 0.16,
            face: Quat::IDENTITY,
        };
        let town = TownMeshes { town: MeshHandle::from_raw(1), plants: [MeshHandle::from_raw(2); 8] };
        let look = LookDev { town, fog_cards: None, pose: pose.clone(), figure };
        scene.spawn_look_dev(Shot::Hero, look);
        assert_eq!(scene.time_of_day().timeline(), &lookdev::day(), "the stage runs on its own day");
        assert_eq!(scene.town_plants.len(), lookdev::PLANTINGS.len());
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

    #[test]
    fn a_timeline_replaces_the_day_only_from_the_days_own_file() {
        let look = || {
            let town = TownMeshes { town: MeshHandle::from_raw(1), plants: [MeshHandle::from_raw(2); 8] };
            let pose = RenderPose { palette: Vec::new(), morph_weights: Vec::new() };
            let figure = Figure { head: Vec3::Y, feet: Vec3::ZERO, body_radius: 0.16, face: Quat::IDENTITY };
            LookDev { town, fog_cards: None, pose, figure }
        };
        // Two complete timelines that differ, standing for an edit of either file.
        let (default_day, stage_day) = (TodTimeline::default_day(), lookdev::day());
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
    }

    #[test]
    fn time_of_day_runs_on_the_world_clock() {
        let mut scene = scene();
        scene.set_day(18.0, Some(24.0));
        for _ in 0..TICK_HZ {
            scene.step();
        }
        let hours = |scene: &Scene| scene.time_of_day().clock().hours(0.0);
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
        let day = TodTimeline::default_day();
        let keys = day.keys().iter().map(|key| {
            let scale = if (6.5..=18.75).contains(&key.time) { 0.1 } else { 1.0 };
            sapphire_world::tod::TodKey { emission: Some(scale), ..key.clone() }
        });
        let keyed = TodTimeline::new(keys.collect()).expect("valid");
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
