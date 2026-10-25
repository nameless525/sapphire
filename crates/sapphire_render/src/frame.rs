//! The extract boundary (spec §4.6, rule R4): the plain data the renderer draws from.
//!
//! Each frame the app's extract systems fill one [`FramePacket`] from the simulation, and the
//! renderer reads only that. A packet holds `Copy` data and owned vectors, never references
//! into the simulation or gameplay types, so it can be handed to render workers while the next
//! frame simulates (§4.2).
//!
//! - **Views and scene constants** are rewritten whole every frame; they are small.
//! - **Instances are deltas.** The renderer keeps every instance in a persistent GPU scene, so a
//!   packet carries only the instances added, changed or removed since the previous one. The
//!   renderer must therefore see every packet, in order (see [`crate::Renderer::prepare`]).
//!
//! Packets live in a [`FramePacketRing`] of three, the in-flight depth of the pipelined frame:
//! one being extracted, one being rendered, one spare so neither side waits. A packet's vectors
//! keep their capacity when the ring reuses it, so a warm frame allocates nothing for its
//! packet. What the packet gains in later milestones is listed on [`FramePacket`].

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};

use crate::MeshHandle;

/// Packets in a [`FramePacketRing`]: one extracting, one rendering, one spare.
const RING_SIZE: usize = 3;

/// Everything the renderer needs to draw one frame.
///
/// # Later milestones
///
/// The packet grows with the systems that feed it (spec §4.6): skinned instances with their
/// palette ranges (R2, A5), morph weights, lights (R5), VFX spawns and emitter parameters,
/// post-process parameters including gameplay-triggered effects (R3, P3), the UI draw list, and
/// debug draws (dev builds). Shadow cascades (R4) and UI 3D insets add views after the main
/// camera, and the time of day (W1) resolves into the scene constants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FramePacket {
    /// Numbers packets in extraction order. Consecutive packets differ by one, which is how
    /// the renderer checks that it saw them all.
    pub frame: u64,
    /// What to draw from. The first view is the main camera; with none, the frame draws
    /// nothing (no target this frame), though its instance deltas still apply.
    pub views: Vec<ViewData>,
    pub sun: DirectionalLight,
    pub sky: SkyGradient,
    /// Presentation time in seconds, read from the owning clock (rule R2), for shader
    /// animation. The app wraps it to a fixed period (the sandbox uses an hour) so it keeps
    /// f32 precision in long sessions; shaders may only use it for effects that are periodic
    /// or tolerate the jump at the wrap.
    pub time_seconds: f32,
    /// Mesh instances added, changed or removed since the previous packet.
    pub instances: InstanceDeltas,
}

impl FramePacket {
    /// The main camera's view, if this frame draws one.
    pub fn main_view(&self) -> Option<&ViewData> {
        self.views.first()
    }

    /// Empties the packet for `frame`, keeping its vectors' capacity.
    fn reset(&mut self, frame: u64) {
        let Self { frame: number, views, sun, sky, time_seconds, instances } = self;
        *number = frame;
        views.clear();
        *sun = DirectionalLight::default();
        *sky = SkyGradient::default();
        *time_seconds = 0.0;
        instances.changed.clear();
        instances.removed.clear();
    }
}

/// The ring of packets extraction writes into, reused frame after frame.
///
/// [`Self::begin`] starts the next frame's packet in the oldest slot; extract systems then fill
/// [`Self::current_mut`], and the renderer reads [`Self::current`]. Before the first `begin`,
/// the current packet is an empty frame 0, so begun frames count from 1.
#[derive(Clone, Debug, Default)]
pub struct FramePacketRing {
    packets: [FramePacket; RING_SIZE],
    current: usize,
}

impl FramePacketRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts the next frame: empties the oldest packet, numbers it one past the current one
    /// and makes it current.
    pub fn begin(&mut self) -> &mut FramePacket {
        let frame = self.packets[self.current].frame + 1;
        self.current = (self.current + 1) % RING_SIZE;
        let packet = &mut self.packets[self.current];
        packet.reset(frame);
        packet
    }

    /// The packet begun last.
    pub fn current(&self) -> &FramePacket {
        &self.packets[self.current]
    }

    pub fn current_mut(&mut self) -> &mut FramePacket {
        &mut self.packets[self.current]
    }
}

/// One view: a camera the renderer draws from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewData {
    pub camera: CameraData,
}

/// Names one mesh instance across packets. The app picks it (the sandbox uses the entity's
/// bits); it must be unique among live instances and stay the same while the instance lives.
/// The renderer maps it to a slot in its instance buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(pub u64);

/// Changes to the set of mesh instances since the previous packet. The renderer applies
/// [`Self::removed`] first, then [`Self::changed`] in order, so an instance removed and added
/// again within one frame survives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InstanceDeltas {
    /// Instances added, or whose mesh or transform changed, with their current state.
    pub changed: Vec<MeshInstance>,
    /// Instances that stopped drawing. IDs the renderer doesn't know are ignored, so reporting
    /// one that was never drawn is harmless.
    pub removed: Vec<InstanceId>,
}

impl InstanceDeltas {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// The main camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraData {
    /// World to view space: right-handed, the camera looks down −Z.
    pub view: Mat4,
    /// View to clip space. Must be reverse-Z with an infinite far plane and Y-up NDC, as built
    /// by `glam::camera::rh::proj::directx::perspective_infinite_reverse` (WebGPU shares
    /// DirectX's clip space).
    pub proj: Mat4,
    /// World-space eye position.
    pub position: Vec3,
}

impl CameraData {
    /// A perspective camera at `eye` looking at `target`.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3, fov_y_radians: f32, aspect: f32, z_near: f32) -> Self {
        Self {
            view: view::look_at_mat4(eye, target, up),
            proj: directx::perspective_infinite_reverse(fov_y_radians, aspect, z_near),
            position: eye,
        }
    }
}

/// The key light. For characters this is the artist-curved virtual key light (spec §5.12).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalLight {
    /// Direction the light travels, from the sun into the scene. Need not be normalized.
    pub direction: Vec3,
    /// Linear RGB.
    pub color: Vec3,
    pub intensity: f32,
}

impl Default for DirectionalLight {
    /// White light of intensity 1, straight down.
    fn default() -> Self {
        Self { direction: Vec3::NEG_Y, color: Vec3::ONE, intensity: 1.0 }
    }
}

/// One drawn mesh, as an instance delta carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshInstance {
    pub id: InstanceId,
    pub mesh: MeshHandle,
    /// Object to world. Mirroring (negative determinant) is not supported yet: it flips
    /// winding, so the toon and outline passes would cull the wrong faces.
    pub transform: Mat4,
}

/// A three-stop sky gradient in linear RGB. It also drives the toon shader's ambient term.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyGradient {
    pub zenith: Vec3,
    pub horizon: Vec3,
    pub ground: Vec3,
}

impl Default for SkyGradient {
    /// A clear midday sky.
    fn default() -> Self {
        Self {
            zenith: Vec3::new(0.10, 0.30, 0.80),
            horizon: Vec3::new(0.62, 0.78, 0.95),
            ground: Vec3::new(0.36, 0.34, 0.42),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_is_reverse_z_with_infinite_far() {
        let camera = CameraData::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, 1.0, 1.5, 0.1);
        let depth = |z: f32| {
            let clip = camera.proj * camera.view * Vec3::new(0.0, 0.0, z).extend(1.0);
            clip.z / clip.w
        };
        assert!((depth(4.9) - 1.0).abs() < 1e-5, "near plane maps to 1");
        assert!(depth(0.0) > depth(-100.0), "nearer is greater");
        assert!(depth(-1.0e7) > 0.0 && depth(-1.0e7) < 1.0e-6, "far away approaches 0");
    }

    fn fill(packet: &mut FramePacket, instances: u64) {
        let camera = CameraData::look_at(Vec3::Z, Vec3::ZERO, Vec3::Y, 1.0, 1.0, 0.1);
        packet.views.push(ViewData { camera });
        packet.sun.intensity = 2.0;
        packet.time_seconds = 1.5;
        for id in 0..instances {
            packet.instances.changed.push(MeshInstance {
                id: InstanceId(id),
                mesh: MeshHandle::from_raw(0),
                transform: Mat4::IDENTITY,
            });
        }
        packet.instances.removed.push(InstanceId(99));
    }

    #[test]
    fn ring_numbers_frames_and_starts_each_packet_empty() {
        let mut ring = FramePacketRing::new();
        assert_eq!(ring.current(), &FramePacket::default(), "an empty frame 0 before the first begin");
        for frame in 1..=7 {
            let packet = ring.begin();
            assert_eq!(packet.frame, frame);
            assert_eq!(packet, &FramePacket { frame, ..FramePacket::default() }, "reset completely");
            fill(packet, 4);
            assert_eq!(ring.current().instances.changed.len(), 4);
        }
    }

    #[test]
    fn warm_packets_reuse_their_storage() {
        let mut ring = FramePacketRing::new();
        // Warm every slot once.
        for _ in 0..RING_SIZE {
            fill(ring.begin(), 8);
        }
        let storage = |packet: &FramePacket| {
            (
                packet.views.as_ptr(),
                packet.views.capacity(),
                packet.instances.changed.as_ptr(),
                packet.instances.changed.capacity(),
                packet.instances.removed.as_ptr(),
            )
        };
        let warm: Vec<_> = ring.packets.iter().map(storage).collect();
        for _ in 0..3 * RING_SIZE {
            fill(ring.begin(), 8);
        }
        let reused: Vec<_> = ring.packets.iter().map(storage).collect();
        assert_eq!(warm, reused, "no packet vector was reallocated");
    }
}
