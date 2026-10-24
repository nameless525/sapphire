//! The sandbox scene: the character on its turntable, the world clock, and the camera that
//! frames it. Shared by the windowed app and the offscreen capture, so a capture shows exactly
//! what the app draws.

use sapphire_assets::mesh::MeshAsset;
use sapphire_math::{Mat4, Vec3};
use sapphire_platform::{AppInitError, Platform};
use sapphire_render::{DirectionalLight, FrameView, MeshHandle, MeshInstance, SkyGradient};
use sapphire_time::{ScaleHandle, ScaleSource, ScaledClock};

use crate::LOG_TARGET;
use crate::input::SimActions;
use crate::orbit::OrbitCamera;
use crate::turntable::Turntable;

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

pub(crate) struct Scene {
    pub character: MeshHandle,
    pub camera: OrbitCamera,
    /// The world clock. Entity clocks are issued by it, and lifecycle pauses push onto it.
    pub world: ScaledClock,
    turntable: Turntable,
    slow_motion: Option<ScaleHandle>,
    /// Reused every frame so building the frame view doesn't allocate.
    instances: Vec<MeshInstance>,
}

impl Scene {
    /// A scene around `character`, framed by its object-space `bounds`.
    pub fn new(character: MeshHandle, bounds: [[f32; 3]; 2]) -> Self {
        let mut world = ScaledClock::new();
        let turntable = Turntable::new(&mut world);
        Self {
            character,
            camera: OrbitCamera::framing(bounds),
            world,
            turntable,
            slow_motion: None,
            instances: Vec::with_capacity(1),
        }
    }

    /// Applies input queued since the last fixed step. Call before the frame's steps.
    pub fn apply(&mut self, actions: SimActions) {
        if actions.toggle_slow_motion {
            match self.slow_motion.take() {
                Some(handle) => {
                    self.world.pop_scale(handle);
                }
                None => {
                    self.slow_motion = self.world.push_scale(ScaleSource::PerfectDodge, SLOW_MOTION).ok();
                }
            }
            tracing::info!(target: LOG_TARGET, world_scale = self.world.effective_scale(), "slow motion toggled");
        }
        if actions.hitstop {
            self.turntable.hitstop(HITSTOP_TICKS);
            tracing::debug!(target: LOG_TARGET, ticks = HITSTOP_TICKS, "hitstop");
        }
    }

    /// One fixed simulation step. Must not allocate (rule R6).
    pub fn step(&mut self) {
        self.world.tick();
        self.turntable.step(&self.world);
    }

    /// The renderer's view of the scene, `alpha` of the way from the previous fixed step to
    /// the latest, for a target with the given aspect ratio.
    pub fn frame_view(&mut self, alpha: f32, aspect: f32) -> FrameView<'_> {
        self.instances.clear();
        self.instances.push(MeshInstance {
            mesh: self.character,
            transform: Mat4::from_rotation_y(self.turntable.angle(alpha)),
        });
        // Interpolated like everything else drawn this frame.
        let time = self.world.now().as_secs_f64()
            + f64::from(alpha) * f64::from(self.world.last_delta().as_secs_f32());
        FrameView {
            camera: self.camera.camera_data(aspect),
            sun: SUN,
            instances: &self.instances,
            sky: SkyGradient::default(),
            time_seconds: (time % SHADER_TIME_WRAP_SECONDS) as f32,
        }
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
