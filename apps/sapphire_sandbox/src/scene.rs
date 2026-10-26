//! The sandbox scene, as ECS data: the character on its turntable, the world clock, and the
//! camera that frames it. Shared by the windowed app and the offscreen modes, so a capture shows
//! exactly what the app draws.
//!
//! - **Resources:** `WorldClock` (from `sapphire_ecs`), the [`OrbitCamera`], the slow-motion
//!   handle, [`TimeOfDay`] (the default day, on a clock of its own issued by the world clock),
//!   the character light rig presets ([`LightRigLibrary`], the shipped ones), and the
//!   [`RenderExtract`] packet ring that extraction fills.
//! - **The character entity:** a [`Turntable`], its `EntityClock`, the [`RenderMesh`] and
//!   [`RenderTransform`] it draws with, and its [`CharacterLightRig`]:
//!   the `gameplay` preset unless `--character-rig` names another.
//! - **Demo lights** (`--lights`): three colored local lights around the character, entities
//!   with a [`RenderLight`] each ([`Scene::spawn_demo_lights`]).
//! - **The skinned stalk** (`--skinned`): a procedural skinned and morphed mesh beside the
//!   character, with its own `EntityClock`, a [`Sway`](skinned::Sway) and a
//!   [`RenderPose`](extract::RenderPose) (see [`crate::skinned`]).
//! - **The shader swatches** (`--swatches`): three spheres in a column on the character's other
//!   side, static, each with a [`RenderMesh`] and its placement ([`Scene::spawn_swatches`],
//!   [`crate::swatches`]).
//! - **The face rig** (`--faces`): rows of the face test head well beside the character, static,
//!   each with a [`FaceRig`] beside its [`RenderMesh`] and placement, which the camera frames
//!   instead of the character ([`Scene::spawn_face_rig`], [`crate::faces`]).
//! - **Systems:** time of day ticks in `FixedFirst`, after the clocks; turntables spin, and
//!   stalks sway, in `FixedCombat`. `Extract` poses them,
//!   then writes the instances that changed, the lights and the poses that changed into the
//!   frame's packet (see [`crate::extract`]). The stalk's systems are added only with `--skinned`,
//!   since each system in `Extract` costs an allocation per frame (below).
//! - **Each frame's packet** starts with the main view and the scene constants, which
//!   [`Scene::frame_packet`] copies whole: they are a few resources, small and new every frame.
//!   Time of day resolves the scene constants, with the camera's position as its zone volumes'
//!   probe, and writes them; the player's filter strength (`post.filter_strength`) goes on top,
//!   then the zone's sky material, and `--cloud-cover`'s coverage or `--plain-sky`'s bare
//!   gradient, if set. After `Extract` has posed and extracted the instances, it resolves each
//!   character's light rig for the frame's camera into the packet's characters (M1 C2), with its
//!   face (M1 C3): the turntable's head turning with it, and each face rig head lit from its own
//!   angles. Both run there rather than as `Extract` systems, which would cost an allocation a
//!   frame each.
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
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_render::{
    CharacterFace, CharacterInstance, FramePacket, HeadBasis, LightKind, LocalLight, MaterialHandle,
    MeshHandle, SkyLayers, ViewData,
};
use sapphire_time::{ScaleHandle, ScaleSource};
use sapphire_world::light_rig::{CharacterLightRig, LightRigId, LightRigLibrary};
use sapphire_world::tod::{TimeOfDay, TodTimeline, tick_time_of_day};

use crate::LOG_TARGET;
use crate::extract::{self, RenderExtract, RenderLight, RenderMesh, RenderTransform};
use crate::faces::{self, FaceRig, HEADS};
use crate::input::SimActions;
use crate::orbit::OrbitCamera;
use crate::skinned::{self, Sway};
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
/// The sample mannequin's head, in its own space: the center of its head sphere, 1.47 m above
/// its 0.2 m pedestal (`sapphire_pipeline`'s samples): face and hair GI turn toward a sphere
/// around it, and the light rig's limits are judged around it.
const CHARACTER_HEAD: Vec3 = Vec3::new(0.0, 1.67, 0.0);

/// The characters the light rig lights: instances drawn with a [`CharacterLightRig`].
type Characters =
    QueryState<(Entity, &'static CharacterLightRig, &'static RenderTransform), With<RenderMesh>>;

/// The face rig's heads, each a character lit from its own angles (see [`crate::faces`]).
type FaceHeads = QueryState<(Entity, &'static FaceRig, &'static RenderTransform), With<RenderMesh>>;

/// World slow motion, while it is toggled on.
#[derive(Resource, Debug, Default)]
struct SlowMotion(Option<ScaleHandle>);

pub(crate) struct Scene {
    ecs: Ecs,
    character: Entity,
    /// The skinned stalk, with `--skinned`.
    stalk: Option<Entity>,
    /// The shader swatches, with `--swatches`.
    swatches: Option<[Entity; SWATCHES]>,
    /// The face rig's heads, with `--faces`.
    faces: Option<[Entity; HEADS]>,
    /// The character's object-space bounds, which demo lights, swatches and the face rig are
    /// placed around.
    bounds: [[f32; 3]; 2],
    /// Made once: a query made each frame would allocate.
    characters: Characters,
    heads: FaceHeads,
    /// The zone's sky material on the current renderer, once loaded.
    sky_material: Option<MaterialHandle>,
    /// Both cloud layers' coverage instead of time of day's (`--cloud-cover`).
    cloud_cover: Option<f32>,
    /// The sky's gradient alone (`--plain-sky`).
    plain_sky: bool,
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
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        let rig = CharacterLightRig { rig: LightRigId::DEFAULT, head: CHARACTER_HEAD };
        let character = world.spawn((Turntable::default(), clock, rig)).id();
        let stalk = skinned.then(|| {
            let clock = world.resource_mut::<WorldClock>().new_entity_clock();
            let place = RenderTransform(skinned::placement(bounds));
            world.spawn((Sway::default(), clock, place, skinned::rest_pose())).id()
        });
        ecs.add_systems(FixedFirst, tick_time_of_day.after(SimSet::Clocks))
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
        Self {
            ecs,
            character,
            stalk,
            swatches: None,
            faces: None,
            bounds,
            characters,
            heads,
            sky_material: None,
            cloud_cover: None,
            plain_sky: false,
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

    /// Places the face look-dev rig (`--faces`) well beside the character, its heads drawing
    /// `mesh`, and frames it with the camera instead of the character. The heads never move, so
    /// they are extracted once.
    pub fn spawn_face_rig(&mut self, mesh: MeshHandle) {
        let camera = faces::camera(self.bounds);
        let heads = faces::heads(self.bounds, &camera);
        let world = self.ecs.world_mut();
        world.insert_resource(camera);
        self.faces =
            Some(heads.map(|(rig, place)| world.spawn((rig, RenderMesh(mesh), RenderTransform(place))).id()));
    }

    /// Sets the mesh the face rig's heads draw again: after GPU device loss.
    pub fn set_face_mesh(&mut self, mesh: MeshHandle) {
        debug_assert!(self.faces.is_some(), "no face rig was spawned");
        for head in self.faces.into_iter().flatten() {
            self.ecs.world_mut().entity_mut(head).insert(RenderMesh(mesh));
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
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.begin();
            let camera = world.resource::<OrbitCamera>();
            if let Some(aspect) = drawable_aspect(target_size) {
                packet.views.push(ViewData { camera: camera.camera_data(aspect) });
            }
            let probe = camera.eye();
            let clock = &world.resource::<WorldClock>().0;
            // Interpolated like everything else drawn this frame.
            packet.time_seconds =
                clock.now().as_secs_f64() + f64::from(alpha) * f64::from(clock.last_delta().as_secs_f32());
            world.resource_mut::<TimeOfDay>().resolve(probe, alpha).write(packet);
            packet.post.filter_strength = crate::cvars::filter_strength();
            packet.sky_material = sky_material;
            if let Some(cover) = cloud_cover {
                packet.sky_layers.clouds.coverage = cover;
                packet.sky_layers.clouds.cirrus = cover;
            }
            if plain_sky {
                packet.sky_layers = SkyLayers::default();
            }
        });
        self.ecs.extract();
        let (characters, heads) = (&mut self.characters, &mut self.heads);
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.packet_mut();
            // The rigs are resolved for the camera; without one nothing draws.
            let Some(camera) = packet.main_view().map(|view| view.camera) else { return };
            let frame = world.resource::<TimeOfDay>().frame();
            let rigs = world.resource::<LightRigLibrary>();
            for (entity, character, transform) in characters.iter(world) {
                let head = transform.0.transform_point3(character.head);
                let lighting = frame.character_lighting(rigs.rig(character.rig), &camera, head);
                // The model's head turns with it: no head joint of its own yet.
                let pose = HeadBasis {
                    forward: transform.0.transform_vector3(Vec3::Z),
                    up: transform.0.transform_vector3(Vec3::Y),
                };
                let face = CharacterFace { head: pose, ..CharacterFace::default() };
                packet.characters.push(CharacterInstance {
                    id: extract::instance_id(entity),
                    lighting,
                    face,
                });
            }
            // The face rig's heads, in the default preset turned to each head's angles.
            let preset = rigs.rig(LightRigId::DEFAULT);
            for (entity, rig, transform) in heads.iter(world) {
                let center = transform.0.transform_point3(Vec3::ZERO);
                let lighting = frame.character_lighting(preset, &camera, center);
                let (lighting, face) = faces::character(rig, transform.0, lighting);
                packet.characters.push(CharacterInstance {
                    id: extract::instance_id(entity),
                    lighting,
                    face,
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
        scene.spawn_face_rig(MeshHandle::from_raw(2));
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
        assert_eq!(lit.lighting.gi.head, Some(CHARACTER_HEAD), "on the turntable's axis");
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
    fn the_face_rig_lists_each_head_lit_from_its_own_angles() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS, false);
        scene.spawn_face_rig(MeshHandle::from_raw(1));
        let heads = scene.faces.expect("spawned").map(|head| InstanceId(head.to_bits()));
        let (turntable, turntable_id) = (scene.character, InstanceId(scene.character.to_bits()));
        for frame in 0..2 {
            scene.step();
            let packet = scene.frame_packet(1.0, Some((1920, 1080)));
            assert_eq!(packet.characters.len(), HEADS + 1, "the heads and the turntable");
            let added =
                packet.instances.changed.iter().filter(|instance| heads.contains(&instance.id)).count();
            assert_eq!(added, if frame == 0 { HEADS } else { 0 }, "the heads are extracted once");
            let rigs = faces::ROWS.iter().flat_map(|row| row.iter());
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
        // At the default hour, time of day draws the sandbox's look from before it existed.
        assert_eq!(packet.sun, SUN);
        assert_eq!(packet.sky, sapphire_render::SkyGradient::default());
        assert_eq!(packet.post, sapphire_render::PostParams::default());
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
