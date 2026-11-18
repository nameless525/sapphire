//! The wind field on the GPU (M6 Y1; the field and its sampler are `sapphire_look::wind`'s): the
//! frame's [`WindField`](crate::WindField), resolved and written each recorded frame into one
//! 832-byte uniform buffer ([`WindBuffer`]) beside the last drawn frame's, for velocity, which every
//! GPU reader of the air binds in a group of its own (foliage's scene group, binding 2) and samples
//! through `shaders/wind.wgsl`.
//!
//! A tier says how many of the field's volumes its readers take ([`WindSettings`], `tiers.ron`): the
//! GPU's loop over at most [`WindSettings::volumes`], a pipeline constant (8, 8, 4 and 0, so Mobile-Low's
//! loop compiles out and its readers see the global wind and turbulence alone), and the CPU's readers
//! take at most [`WindSettings::cpu_volumes`] (8, 8, 4 and 4), which whoever fills the field keeps
//! with [`WindField::choose`](crate::WindField::choose). Turbulence and the volumes add exactly nothing
//! at turbulence 0 with no volumes, so a frame of the global wind alone draws as foliage drew before
//! the field. Nothing here enters the simulation.

use bevy_reflect::Reflect;
use bytemuck::{Pod, Zeroable};
use sapphire_look::wind::{MAX_WIND_VOLUMES, ResolvedWind, WindState, WindVolumeState};
use sapphire_rhi::wgpu;
use serde::{Deserialize, Serialize};

use crate::FramePacket;

/// The name of the wind's pipeline constant: the most volumes a GPU reader loops over.
pub const VOLUMES_CONSTANT: &str = "WIND_VOLUMES";

/// How many of the wind field's volumes a tier's readers take (M6 Y1, `tiers.ron`; see the
/// [module docs](self)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[type_path = "sapphire::render"]
#[serde(default)]
pub struct WindSettings {
    /// The most volumes the GPU's readers loop over, 0..=[`MAX_WIND_VOLUMES`]: the first of the
    /// field's, its strongest. A pipeline constant, so only a new renderer changes it.
    pub volumes: u32,
    /// The most volumes the CPU's readers (springs, effects, audio) take, 0..=[`MAX_WIND_VOLUMES`]:
    /// what whoever fills the field keeps of its candidates.
    pub cpu_volumes: u32,
}

impl Default for WindSettings {
    /// The desktop tiers': every volume, on the GPU and the CPU.
    fn default() -> Self {
        Self { volumes: MAX_WIND_VOLUMES as u32, cpu_volumes: MAX_WIND_VOLUMES as u32 }
    }
}

impl WindSettings {
    /// Checks every knob is in its range, returning the field and what is wrong.
    pub fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if self.volumes as usize > MAX_WIND_VOLUMES {
            return Err(("wind.volumes", "must be within 0..=8"));
        }
        if self.cpu_volumes as usize > MAX_WIND_VOLUMES {
            return Err(("wind.cpu_volumes", "must be within 0..=8"));
        }
        Ok(())
    }

    /// The pipeline constant every GPU reader of the wind takes.
    pub(crate) fn constant(&self) -> (&'static str, f64) {
        (VOLUMES_CONSTANT, f64::from(self.volumes.min(MAX_WIND_VOLUMES as u32)))
    }
}

/// The wind as the GPU reads it (WGSL `WindUniforms`, 832 bytes): this frame's global wind and the
/// last drawn frame's, for velocity, then each one's volumes, this frame's first. The two global
/// states come first, where foliage's wind always had them.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct WindBuffer {
    pub current: WindState,
    pub previous: WindState,
    pub volumes: [WindVolumeState; 2 * MAX_WIND_VOLUMES],
}

const _: () = assert!(size_of::<WindBuffer>() == 832, "m6-plan §3's one wind buffer");

impl WindBuffer {
    /// `current` and `previous` as the shaders read them. Allocates nothing.
    #[must_use]
    pub fn new(current: &ResolvedWind, previous: &ResolvedWind) -> Self {
        let mut volumes = [WindVolumeState::default(); 2 * MAX_WIND_VOLUMES];
        let (now, before) = volumes.split_at_mut(MAX_WIND_VOLUMES);
        now.copy_from_slice(&current.volumes);
        before.copy_from_slice(&previous.volumes);
        Self { current: current.state, previous: previous.state, volumes }
    }
}

/// The wind's GPU side: its uniform buffer, and the prepared and drawn frames' wind.
#[derive(Debug)]
pub(crate) struct GpuWind {
    buffer: wgpu::Buffer,
    history: WindHistory,
}

impl GpuWind {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wind"),
            size: size_of::<WindBuffer>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer, history: WindHistory::default() }
    }

    /// What its readers bind: foliage's scene group's binding 2.
    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Takes the packet's wind at its time.
    pub(crate) fn prepare(&mut self, packet: &FramePacket) {
        self.history.current = packet.wind.resolve(packet.time_seconds);
    }

    /// The recorded frame's buffer ([`WindHistory::record`]).
    pub(crate) fn record(&mut self, cut: bool) -> WindBuffer {
        self.history.record(cut)
    }

    /// The recorded frame never ran ([`WindHistory::frame_lost`]).
    pub(crate) fn frame_lost(&mut self) {
        self.history.frame_lost();
    }
}

/// The prepared frame's wind and the last recorded frame's, which velocity compares with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct WindHistory {
    current: ResolvedWind,
    drawn: Option<ResolvedWind>,
    /// [`Self::drawn`] before the last recorded frame.
    before: Option<ResolvedWind>,
}

impl WindHistory {
    /// The recorded frame's buffer: the prepared wind, and the last recorded frame's for velocity;
    /// the same after a `cut` (or with none before), when velocity counts the camera as unmoved too.
    fn record(&mut self, cut: bool) -> WindBuffer {
        self.before = self.drawn.replace(self.current);
        let previous = match &self.before {
            Some(previous) if !cut => previous,
            _ => &self.current,
        };
        WindBuffer::new(&self.current, previous)
    }

    /// The recorded frame never ran, so velocity goes on comparing with the last that did.
    fn frame_lost(&mut self) {
        self.drawn = self.before;
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sapphire_look::{Wind, WindField, WindVolume};

    use super::*;

    fn breeze() -> Wind {
        Wind { direction: Vec3::new(3.0, 5.0, 4.0), strength: 1.0, gustiness: 0.3 }
    }

    /// A field with every kind of volume, more than a phone's GPU takes.
    fn stormy() -> WindField {
        let mut field = WindField::new(breeze());
        field.turbulence = 0.4;
        for step in 0..6 {
            let at = Vec3::new(step as f32 * 3.0, 0.0, -2.0);
            field.push(match step % 3 {
                0 => WindVolume::directional(at, 5.0, Vec3::new(0.0, 0.5, 2.0)),
                1 => WindVolume::vortex(at, 4.0, Vec3::Y, 3.0, 0.5),
                _ => WindVolume::burst(at, 6.0, 4.0).with_falloff(0.8),
            });
        }
        field
    }

    #[test]
    fn velocity_compares_with_the_last_wind_that_ran() {
        let at = |seconds| WindField::new(breeze()).resolve(seconds);
        let mut history = WindHistory::default();
        let record = |history: &mut WindHistory, seconds, cut| {
            history.current = at(seconds);
            history.record(cut)
        };
        assert_eq!(record(&mut history, 1.0, false).previous, at(1.0).state, "none before");
        record(&mut history, 2.0, false);
        history.frame_lost();
        assert_eq!(record(&mut history, 3.0, false).previous, at(1.0).state, "the lost frame's is gone");
        history.frame_lost();
        history = WindHistory { drawn: None, ..history };
        assert_eq!(record(&mut history, 4.0, false).previous, at(4.0).state);
        record(&mut history, 5.0, true);
        history.frame_lost();
        assert_eq!(record(&mut history, 6.0, false).previous, at(4.0).state, "a lost cut changes nothing");
    }

    #[test]
    fn the_buffer_holds_both_frames_volumes_in_their_halves() {
        let now = stormy().resolve(10.0);
        let mut before = stormy();
        before.truncate(2);
        let before = before.resolve(9.5);
        let buffer = WindBuffer::new(&now, &before);
        assert_eq!((buffer.current, buffer.previous), (now.state, before.state));
        assert_eq!(&buffer.volumes[..MAX_WIND_VOLUMES], &now.volumes);
        assert_eq!(&buffer.volumes[MAX_WIND_VOLUMES..], &before.volumes);
        assert_eq!((buffer.current.volumes(), buffer.previous.volumes()), (6, 2));
        // A field of the global wind alone fills the global states as foliage's wind did before the
        // field, and every volume with zeros.
        let plain = WindField::new(breeze()).resolve(10.0);
        let buffer = WindBuffer::new(&plain, &plain);
        assert_eq!(buffer.current, WindState::new(&breeze(), 10.0));
        assert!(bytemuck::bytes_of(&buffer.volumes).iter().all(|&byte| byte == 0));
    }

    #[test]
    fn settings_default_to_the_desktop_s_and_validate() {
        let settings = WindSettings::default();
        assert_eq!((settings.volumes, settings.cpu_volumes), (8, 8));
        assert_eq!(settings.constant(), ("WIND_VOLUMES", 8.0));
        assert!(settings.validate().is_ok());
        assert_eq!(
            WindSettings { volumes: 9, ..settings }.validate(),
            Err(("wind.volumes", "must be within 0..=8"))
        );
        assert_eq!(
            WindSettings { cpu_volumes: 9, ..settings }.validate(),
            Err(("wind.cpu_volumes", "must be within 0..=8"))
        );
    }

    /// Sampling the field on the CPU and filling the frame's buffer allocate nothing: what a reader
    /// does every frame (springs and effects sample it per chain or per particle).
    #[test]
    fn sampling_and_filling_allocate_nothing() {
        let field = stormy();
        let candidates = [stormy().volumes(), stormy().volumes()].concat();
        let mut history = WindHistory::default();
        let mut chosen = WindField::new(breeze());
        let (sum, allocated) = sapphire_core::alloc::count_in_scope(|| {
            let mut sum = Vec3::ZERO;
            for step in 0..240 {
                let seconds = f64::from(step) / 60.0;
                chosen.choose(&candidates, Vec3::new(step as f32 * 0.1, 0.0, 0.0), 4);
                for point in 0..32 {
                    let at = Vec3::new(point as f32 * 0.7, 1.0, -(point as f32) * 0.3);
                    sum += field.sample(at, seconds) + chosen.sample(at, seconds);
                }
                history.current = field.resolve(seconds);
                let buffer = history.record(step % 60 == 0);
                sum.x += buffer.current.time[0];
            }
            sum
        });
        assert!(sum.is_finite());
        assert_eq!(allocated.allocations, 0, "{allocated:?}");
    }
}
