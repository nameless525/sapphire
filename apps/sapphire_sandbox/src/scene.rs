//! The sandbox scene, as ECS data: the character on its turntable, the world clock, and the
//! camera that frames it. Shared by the windowed app and the offscreen modes, so a capture shows
//! exactly what the app draws.
//!
//! - **Resources:** `WorldClock` (from `sapphire_ecs`), the [`OrbitCamera`], the slow-motion
//!   handle, and the [`RenderInstances`] extraction fills.
//! - **The character entity:** a [`Turntable`], its `EntityClock` and the [`RenderMesh`] it
//!   draws.
//! - **Systems:** turntables spin in `FixedCombat`; `Extract` copies the interpolated instances
//!   out for the renderer.
//!
//! Queued input is applied by [`Scene::apply`] just before a frame's fixed steps, outside them:
//! it logs, and logging allocates, which the fixed step must not (rule R6).

use sapphire_assets::mesh::MeshAsset;
use sapphire_ecs::prelude::*;
use sapphire_math::{Mat4, Vec3};
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::{DirectionalLight, FrameView, MeshHandle, MeshInstance, SkyGradient};
use sapphire_time::{ScaleHandle, ScaleSource};

use crate::LOG_TARGET;
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

/// The mesh an entity draws.
#[derive(Component, Debug)]
pub(crate) struct RenderMesh(pub MeshHandle);

/// World slow motion, while it is toggled on.
#[derive(Resource, Debug, Default)]
struct SlowMotion(Option<ScaleHandle>);

/// What `Extract` copied out for the renderer, reused every frame so extraction doesn't
/// allocate. `FramePacket` replaces it in M1 F3.
#[derive(Resource, Debug, Default)]
struct RenderInstances(Vec<MeshInstance>);

pub(crate) struct Scene {
    ecs: Ecs,
    character: Entity,
}

impl Scene {
    /// A scene around a character with the object-space `bounds`. The character draws once
    /// it has a mesh ([`Self::set_character_mesh`]).
    pub fn new(bounds: [[f32; 3]; 2]) -> Self {
        let mut ecs = Ecs::new();
        let world = ecs.world_mut();
        world.insert_resource(OrbitCamera::framing(bounds));
        world.insert_resource(SlowMotion::default());
        world.insert_resource(RenderInstances(Vec::with_capacity(1)));
        let clock = world.resource_mut::<WorldClock>().new_entity_clock();
        let character = world.spawn((Turntable::default(), clock)).id();
        ecs.add_systems(FixedCombat, turntable::spin).add_systems(Extract, extract_instances);
        ecs.startup();
        Self { ecs, character }
    }

    /// [`Self::new`] with the character drawing `mesh`.
    pub fn with_character(mesh: MeshHandle, bounds: [[f32; 3]; 2]) -> Self {
        let mut scene = Self::new(bounds);
        scene.set_character_mesh(mesh);
        scene
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

    /// Runs presentation and extraction, then returns the renderer's view of the scene,
    /// `alpha` of the way from the previous fixed step to the latest, for a target with the
    /// given aspect ratio.
    pub fn frame_view(&mut self, alpha: f32, aspect: f32) -> FrameView<'_> {
        self.ecs.presentation(alpha);
        self.ecs.extract();
        let world = self.ecs.world();
        let clock = &world.resource::<WorldClock>().0;
        // Interpolated like everything else drawn this frame.
        let time = clock.now().as_secs_f64() + f64::from(alpha) * f64::from(clock.last_delta().as_secs_f32());
        FrameView {
            camera: world.resource::<OrbitCamera>().camera_data(aspect),
            sun: SUN,
            instances: &world.resource::<RenderInstances>().0,
            sky: SkyGradient::default(),
            time_seconds: (time % SHADER_TIME_WRAP_SECONDS) as f32,
        }
    }

    /// Ends the frame (the ECS `Cleanup` schedule). Call once per frame, after rendering.
    pub fn end_frame(&mut self) {
        self.ecs.cleanup();
    }

    pub fn camera_mut(&mut self) -> Mut<'_, OrbitCamera> {
        self.ecs.world_mut().resource_mut()
    }

    /// The world clock, for lifecycle pauses.
    pub fn world_clock_mut(&mut self) -> Mut<'_, WorldClock> {
        self.ecs.world_mut().resource_mut()
    }

    /// Sets the mesh the character draws: at startup, and again after GPU device loss.
    pub fn set_character_mesh(&mut self, mesh: MeshHandle) {
        self.ecs.world_mut().entity_mut(self.character).insert(RenderMesh(mesh));
    }
}

/// Copies every drawn entity out, interpolated, for the renderer. Runs in `Extract`.
fn extract_instances(
    interpolation: Res<'_, Interpolation>,
    drawn: Query<'_, '_, (&Turntable, &RenderMesh)>,
    mut instances: ResMut<'_, RenderInstances>,
) {
    instances.0.clear();
    for (table, mesh) in &drawn {
        instances.0.push(MeshInstance {
            mesh: mesh.0,
            transform: Mat4::from_rotation_y(table.angle(interpolation.alpha)),
        });
    }
}

/// Reads and validates a cooked mesh from the app's content.
pub(crate) fn load_mesh(platform: &Platform, path: &str) -> Result<MeshAsset, AppInitError> {
    let bytes = platform.content().read(path).map_err(|error| {
        AppInitError::msg(format!(
            "{error} (content root {:?}; cook it with `cargo xtask content`)",
            platform.content().root()
        ))
    })?;
    let mesh = MeshAsset::from_bytes(&bytes)?;
    tracing::info!(
        target: LOG_TARGET,
        path,
        vertices = mesh.vertices.len(),
        triangles = mesh.indices.len() / 3,
        "mesh loaded"
    );
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use sapphire_time::TICK_HZ;

    use super::*;

    fn scene() -> Scene {
        Scene::new([[-0.5, 0.0, -0.5], [0.5, 2.0, 0.5]])
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
        let mut allocations = 0;
        for _ in 0..60 {
            let ((), steps) = sapphire_core::alloc::count_in_scope(|| {
                scene.step();
                scene.step();
            });
            allocations += steps.allocations;
            let _ = scene.frame_view(0.5, 1.0);
            scene.end_frame();
        }
        assert_eq!(allocations, 0, "rule R6");
    }
}
