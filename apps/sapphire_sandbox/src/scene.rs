//! The sandbox scene, as ECS data: the character on its turntable, the world clock, and the
//! camera that frames it. Shared by the windowed app and the offscreen modes, so a capture shows
//! exactly what the app draws.
//!
//! - **Resources:** `WorldClock` (from `sapphire_ecs`), the [`OrbitCamera`], the slow-motion
//!   handle, and the [`RenderExtract`] packet ring that extraction fills.
//! - **The character entity:** a [`Turntable`], its `EntityClock`, and the [`RenderMesh`] and
//!   [`RenderTransform`](extract::RenderTransform) it draws with.
//! - **Demo lights** (`--lights`): three colored local lights around the character, entities
//!   with a [`RenderLight`] each ([`Scene::spawn_demo_lights`]).
//! - **Systems:** turntables spin in `FixedCombat`. `Extract` poses them, then writes the
//!   instances that changed and the lights into the frame's packet (see [`crate::extract`]).
//! - **Each frame's packet** starts with the main view and the scene constants, which
//!   [`Scene::frame_packet`] copies whole: they are a few resources, small and new every frame.
//!
//! `Presentation` has no systems yet. Posing is presentation work, but a multi-threaded
//! schedule that has systems costs an allocation per run plus one per system, and wakes a
//! worker; an empty one costs nothing. In `Presentation` the pose cost 2 allocations and about
//! 8 µs more per frame (`--bench`, M5), so it runs at the head of `Extract`, which runs anyway.
//! It moves to `Presentation` once animation gives that schedule real work (M2).
//!
//! Queued input is applied by [`Scene::apply`] just before a frame's fixed steps, outside them:
//! it logs, and logging allocates, which the fixed step must not (rule R6).

use sapphire_assets::mesh::{AlignedVec, ArchivedMesh, Mesh};
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::{
    DirectionalLight, FramePacket, LightKind, LocalLight, MeshHandle, SkyGradient, ViewData,
};
use sapphire_time::{ScaleHandle, ScaleSource};

use crate::LOG_TARGET;
use crate::extract::{self, RenderExtract, RenderLight, RenderMesh};
use crate::input::SimActions;
use crate::orbit::OrbitCamera;
use crate::turntable::{self, Turntable};

/// The cooked sample character, produced by `cargo xtask content`.
pub(crate) const CHARACTER_MESH: &str = "samples/mannequin.smesh";
/// World slow motion while toggled, in per-mille (perfect-dodge style, spec §4.5).
const SLOW_MOTION: u32 = 200;
/// Fixed steps the hitstop action freezes the turntable for.
const HITSTOP_TICKS: u16 = 8;
/// Shader time wraps so it keeps f32 precision over long sessions.
const SHADER_TIME_WRAP_SECONDS: f64 = 3600.0;

const SUN: DirectionalLight = DirectionalLight {
    direction: Vec3::new(0.55, -0.7, -0.45),
    color: Vec3::new(1.0, 0.97, 0.92),
    intensity: 1.0,
};

/// World slow motion, while it is toggled on.
#[derive(Resource, Debug, Default)]
struct SlowMotion(Option<ScaleHandle>);

pub(crate) struct Scene {
    ecs: Ecs,
    character: Entity,
    /// The character's object-space bounds, which demo lights are placed around.
    bounds: [[f32; 3]; 2],
}

impl Scene {
    /// A scene around a character with the object-space `bounds`. The character draws once
    /// it has a mesh ([`Self::set_character_mesh`]).
    pub fn new(bounds: [[f32; 3]; 2]) -> Self {
        let mut ecs = Ecs::new();
        let world = ecs.world_mut();
        world.insert_resource(OrbitCamera::framing(bounds));
        world.insert_resource(SlowMotion::default());
        world.insert_resource(RenderExtract::default());
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        let character = world.spawn((Turntable::default(), clock)).id();
        ecs.add_systems(FixedCombat, turntable::spin).add_systems(
            Extract,
            (turntable::pose, extract::extract_instances, extract::extract_lights).chain(),
        );
        ecs.startup();
        Self { ecs, character, bounds }
    }

    /// [`Self::new`] with the character drawing `mesh`.
    pub fn with_character(mesh: MeshHandle, bounds: [[f32; 3]; 2]) -> Self {
        let mut scene = Self::new(bounds);
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
        self.ecs.world_mut().resource_scope(|world, mut extract: Mut<'_, RenderExtract>| {
            let packet = extract.begin();
            if let Some(aspect) = drawable_aspect(target_size) {
                packet.views.push(ViewData { camera: world.resource::<OrbitCamera>().camera_data(aspect) });
            }
            let clock = &world.resource::<WorldClock>().0;
            // Interpolated like everything else drawn this frame.
            let time =
                clock.now().as_secs_f64() + f64::from(alpha) * f64::from(clock.last_delta().as_secs_f32());
            packet.sun = SUN;
            packet.sky = SkyGradient::default();
            packet.time_seconds = (time % SHADER_TIME_WRAP_SECONDS) as f32;
            // The look-dev cvars on the default look, until time of day resolves it (W1).
            packet.post = crate::cvars::post_params();
        });
        self.ecs.extract();
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

    /// Sets the mesh the character draws: at startup, and again after GPU device loss.
    pub fn set_character_mesh(&mut self, mesh: MeshHandle) {
        self.ecs.world_mut().entity_mut(self.character).insert(RenderMesh(mesh));
    }
}

/// Width over height of a target the main view can draw into: none without a target, or for
/// an empty one (a minimized window).
fn drawable_aspect(target_size: Option<(u32, u32)>) -> Option<f32> {
    let (width, height) = target_size.filter(|&(width, height)| width > 0 && height > 0)?;
    Some(width as f32 / height as f32)
}

/// Reads a cooked mesh from the app's content into aligned memory, checks it, and hands it,
/// read in place, to `use_mesh` (to upload it and take its bounds). The bytes live for the call
/// only: the renderer copies what it keeps.
pub(crate) fn load_mesh<R, E>(
    platform: &Platform,
    path: &str,
    use_mesh: impl FnOnce(&ArchivedMesh) -> Result<R, E>,
) -> Result<R, AppInitError>
where
    AppInitError: From<E>,
{
    let content = platform.content();
    let missing = |error: &dyn std::fmt::Display| {
        AppInitError::msg(format!(
            "{path}: {error} (content root {:?}; cook it with `cargo xtask content`)",
            content.root()
        ))
    };
    let mut file = content.open(path).map_err(|error| missing(&error))?;
    let mut bytes: AlignedVec = AlignedVec::new();
    bytes.extend_from_reader(&mut file).map_err(|error| missing(&error))?;
    let mesh = Mesh::read(&bytes)?;
    let lod0_triangles: u32 = mesh.lods[0].submeshes.iter().map(|s| s.index_count.to_native() / 3).sum();
    tracing::info!(
        target: LOG_TARGET,
        path,
        vertices = mesh.streams.len(),
        triangles = lod0_triangles,
        lods = mesh.lods.len(),
        skinned = mesh.skin.is_some(),
        "mesh loaded"
    );
    Ok(use_mesh(mesh)?)
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

    fn scene() -> Scene {
        Scene::new(BOUNDS)
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
        let mut scene = scene();
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
    fn demo_lights_surround_the_character() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS);
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
    fn the_main_view_needs_a_drawable_target() {
        assert_eq!(drawable_aspect(Some((1920, 1080))), Some(1920.0 / 1080.0));
        for size in [None, Some((0, 1080)), Some((1920, 0))] {
            assert_eq!(drawable_aspect(size), None, "{size:?}");
        }
    }

    #[test]
    fn packets_carry_the_view_and_only_instances_that_moved() {
        let mut scene = Scene::with_character(MeshHandle::from_raw(0), BOUNDS);
        let character = InstanceId(scene.character.to_bits());
        let changed = |packet: &FramePacket| -> Vec<InstanceId> {
            packet.instances.changed.iter().map(|instance| instance.id).collect()
        };

        scene.step();
        let packet = scene.frame_packet(1.0, Some((1920, 1080)));
        assert_eq!(packet.views.len(), 1);
        assert_eq!(packet.sun, SUN);
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
