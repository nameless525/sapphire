//! Reflection probes' runtime (M5 L3): a zone's baked local cubes (`.sreflect`, M5 L2) on the GPU, composed
//! by the hour and read by the environment and by characters (`reflections.wgsl`).
//!
//! # What lives where
//!
//! [`ProbeSets`] holds every uploaded set: each probe's every baked key, one `Rgba16Float` 2D array a probe
//! (six layers a key, at the cook's size), uploaded once from the bytes the asset server hands over and
//! then drops (`AssetKind::keeps_bytes` is false for the kind). One set at a time is shown
//! ([`ActiveProbes`]): its composed texture, six layers a probe at the tier's size with the roughness
//! chain down to 8 texels, read as a cube array or as faces ([`ProbeRead`]); the environment's cell
//! table; the probes' boxes as the environment and the characters' picker read them; and the bind
//! groups that compose each probe.
//!
//! Every key stays on the GPU, rather than the two in play: the asset keeps no bytes, so only a streamer
//! (out of M5's scope) could bring a key back as the hour moves, and 42 probes of ten keys come to 315 MiB on
//! the desktop tiers and 79 MiB on the phones', inside L2's pools of 384 and 96 (spec §5.12, "Reflections",
//! has the arithmetic). Mobile-Low holds each key at the 32 it composes at, averaged on the CPU as the
//! composition would average it, 20 MiB; a set it uploaded composes no larger on a larger tier until it's
//! uploaded again ([`ProbeSet::new`]).
//!
//! # Composing
//!
//! A probe's cube is its keys blended by the frame's weights ([`FramePacket::reflection_keys`], the same
//! blend as the probe grid's), plus the live sky toward each texel times the texel's baked sky share, at the
//! ambient's scale (`cs_compose`), then prefiltered level by level (`cs_prefilter`, the reflection cube's).
//! A probe is out of date when the keys' weights change, when the authored sky jumps, and while the sky
//! cube redraws its faces after a jump ([`ComposeSchedule`]); one is composed a frame, the oldest first and
//! the nearest the camera among equals, so after a jump the nearest goes first and a slowly moving hour
//! turns through them all. Nothing is dispatched while every probe is current. The frame a set is shown
//! composes all of it, so no probe is ever read before it's whole, and a capture does the same
//! ([`ComposeSchedule::finish`]). Mobile-Low composes each probe once at the tier's 32 from its nearest key
//! and again only when the hour or the sky jumps ([`ProbeMode::CharactersOnce`]).
//!
//! [`FramePacket::reflection_keys`]: crate::FramePacket::reflection_keys

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use sapphire_assets::reflections::ArchivedReflectionProbes;
use sapphire_rhi::wgpu;

use super::{KEY_JUMP, ProbeKeys, SKY_JUMP, STORAGE_FORMAT, keys_change};
use crate::RenderError;
use crate::environment::EnvironmentSpecular;
use crate::shaders::ProbeRead;

/// The most probes a set holds (`sapphire_assets::reflections::MAX_PROBES`): one texture of the device
/// floor's 256 layers holds 42 cubes.
pub(crate) const MAX_PROBES: usize = sapphire_assets::reflections::MAX_PROBES;

/// A character entry with no probe (WGSL `NO_CHARACTER_PROBE`): it reflects the sky's cube alone.
pub(crate) const NO_PROBE: u32 = u32::MAX;

/// A cell with no probe (WGSL `NO_CELL_PROBE`).
pub(crate) const NO_CELL_PROBE: u8 = 255;
/// The cell table's texels: a cell's two probes.
pub(crate) const CELL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Uint;

/// The cell table's side at its finest, metres; it doubles until the zone's probes fit [`MAX_CELLS`].
const CELL_SIDE: f32 = 1.0;
/// The most cells the table holds: 512 KiB of `Rg8Uint`.
const MAX_CELLS: usize = 1 << 18;

/// How far past its box's faces a probe's cube reaches, fading out: a quarter of the box's least side, at
/// most this.
const MAX_FADE: f32 = 0.5;

/// The composed cubes' last level, texels a side, as the reflection cube's.
const MIN_COMPOSED: u32 = 8;

/// The sky cube's faces, which redraw one at a time after the sky jumps.
const SKY_FACES: u32 = 6;

/// How a tier reads probes, from its environment specular (`tiers.ron` needs nothing new).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeMode {
    /// Characters only, from cubes composed once (Mobile-Low, which has no environment specular).
    CharactersOnce,
    /// The environment along the plain reflected ray, and characters (Mobile-High).
    Plain,
    /// The environment with box parallax, and characters (the desktop tiers).
    Parallax,
}

impl ProbeMode {
    pub(crate) fn of(specular: EnvironmentSpecular) -> Self {
        match specular {
            EnvironmentSpecular::Off => Self::CharactersOnce,
            EnvironmentSpecular::Approximate => Self::Plain,
            EnvironmentSpecular::Full => Self::Parallax,
        }
    }

    /// The environment's `REFLECTION_PROBES` constant, or none where it reads no probes.
    pub(crate) fn constant(self) -> Option<f64> {
        match self {
            Self::CharactersOnce => None,
            Self::Plain => Some(1.0),
            Self::Parallax => Some(2.0),
        }
    }
}

/// A set of reflection probes uploaded to the renderer ([`Renderer::upload_reflection_probes`]).
///
/// [`Renderer::upload_reflection_probes`]: crate::Renderer::upload_reflection_probes
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReflectionProbesHandle(u32);

/// A probe's box and where it was baked from, world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProbeBox {
    pub position: Vec3,
    pub min: Vec3,
    pub max: Vec3,
}

impl ProbeBox {
    /// How far past the box's faces its cube reaches, fading out.
    pub(crate) fn fade(&self) -> f32 {
        (0.25 * (self.max - self.min).min_element()).min(MAX_FADE)
    }

    /// Whether the probe reaches into the cell from `low` to `low + side`: its box grown by [`Self::fade`]
    /// overlaps it.
    fn reaches_cell(&self, low: Vec3, side: f32) -> bool {
        let fade = Vec3::splat(self.fade());
        (low + side).cmpgt(self.min - fade).all() && low.cmplt(self.max + fade).all()
    }

    /// The largest weight it has anywhere in the cell from `low` to `low + side` ([`Self::weight`] at the
    /// cell's point nearest the box): 1 where the box meets the cell.
    fn weight_in_cell(&self, low: Vec3, side: f32) -> f32 {
        let outside = (self.min - (low + side)).max(low - self.max).max(Vec3::ZERO).length();
        (1.0 - outside / self.fade()).clamp(0.0, 1.0)
    }

    /// How much of its cube a point takes: 1 inside the box and on its faces, since a box is the room its
    /// probe stands in and its walls lie on them, falling to 0 over [`Self::fade`] past them (WGSL
    /// `probe_weight`).
    pub(crate) fn weight(&self, point: Vec3) -> f32 {
        let outside = (self.min - point).max(point - self.max).max(Vec3::ZERO).length();
        (1.0 - outside / self.fade()).clamp(0.0, 1.0)
    }
}

/// The environment's table of which probes each cell of the zone reads: cells of [`CELL_SIDE`] (doubled
/// until they fit [`MAX_CELLS`]) over the union of the probes' reaches (each box grown by its fade), each
/// naming the two probes that reach into it with the largest weight anywhere in it, the one baked nearer
/// its centre first where they tie, or [`NO_CELL_PROBE`]. A pixel takes the one of its cell's two whose
/// weight at it is larger, the first where they tie (`probe_at`, WGSL `environment_reflection`). A cell
/// any of a probe reaches names one, so a pixel's weight, which the shader takes from the box itself,
/// falls to 0 before the table's cells can stop naming its probe. A pixel inside a box reads it whole
/// wherever no more than two boxes meet its cell: the box meets the cell, so it is one of the two, and
/// its weight there is 1 (review F3, 2026-11-15: one probe a cell, the one baked nearest, gave a strip
/// inside a box by a wall it shares the neighbour's fade).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CellTable {
    pub origin: Vec3,
    pub side: f32,
    pub dims: [u32; 3],
    /// x fastest, then y, then z: per cell, its first probe and its second.
    pub cells: Vec<[u8; 2]>,
}

impl CellTable {
    pub(crate) fn build(boxes: &[ProbeBox]) -> Self {
        debug_assert!(!boxes.is_empty() && boxes.len() <= MAX_PROBES);
        let min = boxes.iter().fold(Vec3::splat(f32::MAX), |min, probe| min.min(probe.min - probe.fade()));
        let max = boxes.iter().fold(Vec3::splat(f32::MIN), |max, probe| max.max(probe.max + probe.fade()));
        let mut side = CELL_SIDE;
        let dims = loop {
            let dims = ((max - min) / side).ceil().max(Vec3::ONE).as_uvec3();
            if (dims.x as usize) * (dims.y as usize) * (dims.z as usize) <= MAX_CELLS {
                break dims;
            }
            side *= 2.0;
        };
        let mut cells = Vec::with_capacity((dims.x * dims.y * dims.z) as usize);
        let mut reaching = Vec::with_capacity(boxes.len());
        for z in 0..dims.z {
            for y in 0..dims.y {
                for x in 0..dims.x {
                    let low = min + Vec3::new(x as f32, y as f32, z as f32) * side;
                    let centre = low + 0.5 * side;
                    // The probes reaching in, by their largest weight in the cell, then by how near
                    // their baked position is to its centre, then by index.
                    reaching.clear();
                    for (index, probe) in boxes.iter().enumerate() {
                        if probe.reaches_cell(low, side) {
                            let weight = probe.weight_in_cell(low, side);
                            reaching.push((weight, probe.position.distance_squared(centre), index));
                        }
                    }
                    reaching.sort_unstable_by(|a, b| {
                        b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2))
                    });
                    let named = |rank: usize| reaching.get(rank).map_or(NO_CELL_PROBE, |probe| probe.2 as u8);
                    cells.push([named(0), named(1)]);
                }
            }
        }
        Self { origin: min, side, dims: dims.to_array(), cells }
    }

    /// The probe a point at `point` reads, of `boxes` the table was built from, as the shader finds it
    /// (`environment_reflection`): of its cell's two, the one whose weight at the point is larger, the
    /// first where they tie.
    #[cfg(test)]
    fn probe_at(&self, boxes: &[ProbeBox], point: Vec3) -> Option<usize> {
        let cell = ((point - self.origin) / self.side).floor();
        let dims = Vec3::new(self.dims[0] as f32, self.dims[1] as f32, self.dims[2] as f32);
        if cell.cmplt(Vec3::ZERO).any() || cell.cmpge(dims).any() {
            return None;
        }
        let [x, y, z] = cell.as_uvec3().to_array();
        let [first, second] = self.cells[(x + self.dims[0] * (y + self.dims[1] * z)) as usize];
        let weight = |probe: u8| boxes[usize::from(probe)].weight(point);
        let probe = if second != NO_CELL_PROBE && weight(second) > weight(first) { second } else { first };
        (probe != NO_CELL_PROBE).then_some(usize::from(probe))
    }
}

/// Picks a character's probe on the CPU, from where its body's centre stands: the probe that reaches it, the
/// one baked nearest where several do, fading out past the box's faces as the environment's does.
#[derive(Clone, Debug, Default)]
pub(crate) struct ProbePicker {
    boxes: Vec<ProbeBox>,
}

impl ProbePicker {
    /// The probe a character centred at `centre` reflects, and how much of it: ([`NO_PROBE`], 0) beyond
    /// every probe's reach.
    pub(crate) fn pick(&self, centre: Vec3) -> (u32, f32) {
        let nearest = self
            .boxes
            .iter()
            .enumerate()
            .map(|(index, probe)| (index, probe, probe.weight(centre)))
            .filter(|&(_, _, weight)| weight > 0.0)
            .min_by(|(_, a, _), (_, b, _)| {
                a.position.distance_squared(centre).total_cmp(&b.position.distance_squared(centre))
            });
        nearest.map_or((NO_PROBE, 0.0), |(index, _, weight)| (index as u32, weight))
    }
}

/// One probe as the environment reads it (WGSL `ReflectionProbe`). 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuProbe {
    /// xyz: the box's least corner; w: 0.
    pub box_min: [f32; 4],
    /// xyz: its greatest corner; w: how far past the box its cube reaches, fading out, metres.
    pub box_max: [f32; 4],
    /// xyz: where its cubes were baked from; w: 0.
    pub position: [f32; 4],
}

/// The environment's probes (WGSL `ReflectionUniforms`). 2,048 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct ProbeUniforms {
    /// xyz: the cell table's least corner; w: a cell's side.
    pub cells_origin: [f32; 4],
    /// xyz: cells along each axis, 0 without probes; w: probes.
    pub cells: [u32; 4],
    pub probes: [GpuProbe; MAX_PROBES],
}

/// A frame's composition (WGSL `ComposeUniforms`). 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct ComposeUniforms {
    /// The layer each blended key's +X face starts at (key × 6).
    pub keys: [u32; 4],
    pub weights: [f32; 4],
    /// x: the composed size; y: baked texels a composed texel averages along a side; zw: 0.
    pub size: [u32; 4],
}

const _: () = {
    assert!(size_of::<GpuProbe>() == 48);
    assert!(size_of::<ProbeUniforms>() == 2048);
    assert!(size_of::<ComposeUniforms>() == 48);
};

/// When a set's probes are composed: which are out of date, and which one this frame composes.
///
/// `epoch` counts changes to what composition reads; a probe composed at an earlier one is out of date.
/// Plain data, so a lost frame puts back the copy taken before it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ComposeSchedule {
    count: usize,
    epoch: u64,
    /// Per probe, the epoch it was last composed at: 0 never.
    composed: [u64; MAX_PROBES],
    /// The keys and authored sky the last frame composed against.
    keys: ProbeKeys,
    sky: Vec3,
    /// Sky cube faces still to redraw since the sky jumped, while every frame is a change.
    settling: u32,
    /// Mobile-Low's: the hour's slow moves change nothing; only jumps do.
    frozen: bool,
    /// Compose every probe out of date in the next frame: a set just shown, or a capture's.
    finish: bool,
}

impl ComposeSchedule {
    /// A set of `count` probes just shown: the next frame composes them all.
    pub(crate) fn new(count: usize, frozen: bool) -> Self {
        Self {
            count: count.min(MAX_PROBES),
            epoch: 1,
            composed: [0; MAX_PROBES],
            keys: ProbeKeys::default(),
            sky: Vec3::ZERO,
            settling: 0,
            frozen,
            finish: true,
        }
    }

    /// Composes every probe out of date in the next frame (a capture's, which no rotation may decide).
    pub(crate) fn finish(&mut self) {
        self.finish = true;
    }

    /// Whether any probe is out of date.
    #[cfg(test)]
    fn any_due(&self) -> bool {
        self.composed[..self.count].iter().any(|&composed| composed < self.epoch)
    }

    /// The probes a frame composes, as bits (probe `i` at bit `i`): the frame blends `keys` under the
    /// authored ambient sky `sky`, its sky cube drew `faces` faces, and its camera is at `eye`; `boxes` are
    /// the probes'.
    pub(crate) fn frame(
        &mut self,
        keys: ProbeKeys,
        sky: Vec3,
        faces: u32,
        eye: Option<Vec3>,
        boxes: &[ProbeBox],
    ) -> u64 {
        let first = self.composed[..self.count].iter().all(|&composed| composed == 0);
        let sky_jump =
            !first && (sky - self.sky).abs().max_element() > SKY_JUMP * sky.max(self.sky).max_element();
        let jump = sky_jump || (!first && keys_change(&self.keys, &keys) > KEY_JUMP);
        if jump {
            // The nearest first: every probe is as old as every other.
            self.composed = [0; MAX_PROBES];
            self.settling = self.settling.max(u32::from(sky_jump) * SKY_FACES);
        }
        if jump || self.settling > 0 || (!self.frozen && keys != self.keys) {
            self.epoch += 1;
        }
        self.settling = self.settling.saturating_sub(faces);
        (self.keys, self.sky) = (keys, sky);
        let epoch = self.epoch;
        if std::mem::take(&mut self.finish) {
            let mut bits = 0;
            for (probe, composed) in self.composed[..self.count].iter_mut().enumerate() {
                if *composed < epoch {
                    *composed = epoch;
                    bits |= 1 << probe;
                }
            }
            return bits;
        }
        let distance = |probe: usize| eye.map_or(0.0, |eye| eye.distance_squared(boxes[probe].position));
        let composed = &self.composed;
        let next = (0..self.count.min(boxes.len()))
            .filter(|&probe| composed[probe] < epoch)
            .min_by(|&a, &b| composed[a].cmp(&composed[b]).then(distance(a).total_cmp(&distance(b))));
        next.map_or(0, |probe| {
            self.composed[probe] = self.epoch;
            1 << probe
        })
    }
}

/// The keys a frame composes from: `keys` as they are, or for Mobile-Low (`frozen`) the heaviest alone;
/// the frame's first key alone when time of day gave none.
pub(crate) fn composed_keys(keys: ProbeKeys, frozen: bool) -> ProbeKeys {
    let weighed = keys.is_valid() && keys.weights.iter().sum::<f32>() > 0.0;
    let keys = if weighed { keys } else { ProbeKeys::single(0) };
    if !frozen {
        return keys;
    }
    let heaviest =
        (0..keys.keys.len()).max_by(|&a, &b| keys.weights[a].total_cmp(&keys.weights[b])).unwrap_or(0);
    ProbeKeys::single(keys.keys[heaviest])
}

/// An uploaded set: every key of every probe, and what its probes are.
#[derive(Debug)]
pub(crate) struct ProbeSet {
    /// The size its keys are held at, texels a side: the cook's, or on Mobile-Low the size it composes at
    /// ([`ProbeSet::new`]'s `hold`).
    pub size: u32,
    pub keys: u32,
    pub boxes: Vec<ProbeBox>,
    /// Per probe, every key: six layers a key.
    pub views: Vec<wgpu::TextureView>,
    pub bytes: u64,
}

impl ProbeSet {
    /// Uploads `probes`, checked by `ReflectionProbes::read`, each key held at `hold` texels a side where
    /// that's under the cook's: Mobile-Low's, whose composition would average the cook's texels down to it
    /// anyway (`ArchivedReflectionProbes::cube_at` averages them as it would). A set held smaller composes at
    /// that size on any tier it's handed to ([`composed_size`]). Fails if the device can't hold a probe's
    /// keys in one texture.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        probes: &ArchivedReflectionProbes,
        hold: u32,
    ) -> Result<Self, RenderError> {
        let size = composed_size(probes.size(), hold);
        let keys = probes.key_hours().len() as u32;
        let count = probes.probes.len();
        let limits = device.limits();
        let too_large =
            RenderError::ReflectionProbesTooLarge { probes: count as u32, keys, size: probes.size() };
        if count == 0
            || count > MAX_PROBES
            || 6 * keys > limits.max_texture_array_layers
            || size > limits.max_texture_dimension_2d
        {
            return Err(too_large);
        }
        let boxes = probes
            .probes
            .iter()
            .map(|probe| ProbeBox {
                position: Vec3::from_array(probe.position.map(|v| v.to_native())),
                min: Vec3::from_array(probe.parallax_min.map(|v| v.to_native())),
                max: Vec3::from_array(probe.parallax_max.map(|v| v.to_native())),
            })
            .collect();
        let layers = 6 * keys;
        let mut texels: Vec<u16> = Vec::with_capacity(
            sapphire_assets::reflections::ReflectionProbes::cube_len(size) * keys as usize,
        );
        let views = (0..count)
            .map(|probe| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("reflection probe keys"),
                    size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: STORAGE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                texels.clear();
                for key in 0..keys as usize {
                    probes.cube_at(probe, key, size, &mut texels);
                }
                queue.write_texture(
                    texture.as_image_copy(),
                    bytemuck::cast_slice(&texels),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(size * 8),
                        rows_per_image: Some(size),
                    },
                    texture.size(),
                );
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("reflection probe keys"),
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
            })
            .collect();
        let bytes = count as u64 * u64::from(layers) * u64::from(size) * u64::from(size) * 8;
        Ok(Self { size, keys, boxes, views, bytes })
    }
}

/// Every uploaded set, by handle.
#[derive(Debug, Default)]
pub(crate) struct ProbeSets {
    slots: Vec<Option<ProbeSet>>,
}

impl ProbeSets {
    pub(crate) fn insert(&mut self, set: ProbeSet) -> ReflectionProbesHandle {
        let index = self.slots.iter().position(Option::is_none).unwrap_or_else(|| {
            self.slots.push(None);
            self.slots.len() - 1
        });
        self.slots[index] = Some(set);
        ReflectionProbesHandle(index as u32)
    }

    pub(crate) fn get(&self, handle: ReflectionProbesHandle) -> Option<&ProbeSet> {
        self.slots.get(handle.0 as usize)?.as_ref()
    }

    pub(crate) fn remove(&mut self, handle: ReflectionProbesHandle) -> Option<ProbeSet> {
        self.slots.get_mut(handle.0 as usize)?.take()
    }

    /// Sets held.
    pub(crate) fn len(&self) -> u32 {
        self.slots.iter().flatten().count() as u32
    }

    /// Their keys' bytes.
    pub(crate) fn bytes(&self) -> u64 {
        self.slots.iter().flatten().map(|set| set.bytes).sum()
    }
}

/// The size a set baked at `baked` composes at on a tier whose reflection cube is `tier` a side: the
/// tier's, or the bake's where it's smaller, never under 8.
pub(crate) fn composed_size(baked: u32, tier: u32) -> u32 {
    tier.min(baked).max(MIN_COMPOSED)
}

/// Levels of a composed cube of `size`: down to 8 texels.
pub(crate) fn composed_mips(size: u32) -> u32 {
    (size / MIN_COMPOSED).max(1).ilog2() + 1
}

/// Bytes of `probes` composed cubes of `size` with their levels.
pub(crate) fn composed_bytes(probes: u32, size: u32) -> u64 {
    (0..composed_mips(size)).map(|mip| 6 * u64::from(probes) * u64::from(size >> mip).pow(2) * 8).sum()
}

/// One probe's bind groups: its composition's, then its prefilter's per level past the first.
#[derive(Debug)]
pub(crate) struct ProbeGroups {
    pub compose: wgpu::BindGroup,
    pub prefilter: Vec<wgpu::BindGroup>,
}

/// The set the renderer shows.
#[derive(Debug)]
pub(crate) struct ActiveProbes {
    pub handle: ReflectionProbesHandle,
    pub size: u32,
    pub mips: u32,
    /// Baked texels a composed texel averages along a side.
    pub step: u32,
    pub keys: u32,
    /// Group 0's binding 17: every composed cube.
    pub view: wgpu::TextureView,
    /// Group 0's binding 18.
    pub cells: wgpu::TextureView,
    pub groups: Vec<ProbeGroups>,
    pub boxes: Vec<ProbeBox>,
    pub picker: ProbePicker,
    pub schedule: ComposeSchedule,
    pub bytes: u64,
    /// The prefilter's per-level uniforms, which its groups bind.
    _prefilter_uniforms: wgpu::Buffer,
}

/// What showing a set needs of the ambient pass.
pub(crate) struct ActiveBindings<'a> {
    pub compose: &'a wgpu::BindGroupLayout,
    pub prefilter: &'a wgpu::BindGroupLayout,
    pub sky_light: &'a wgpu::Buffer,
    pub sky_cube: &'a wgpu::TextureView,
    /// The ambient uniforms' buffer and where the composition's region starts in it.
    pub uniforms: (&'a wgpu::Buffer, u64),
}

impl ActiveProbes {
    /// Shows `set` (`handle`) at a tier's reflection size `tier` read as `read`, Mobile-Low's way where
    /// `frozen`: makes its composed texture, cell table and groups, and writes the environment's uniforms
    /// into `probe_uniforms`. Its first frame composes every probe.
    #[expect(clippy::too_many_arguments, reason = "the set, the tier's choices and the pass's bindings")]
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        handle: ReflectionProbesHandle,
        set: &ProbeSet,
        tier: u32,
        read: ProbeRead,
        frozen: bool,
        bindings: &ActiveBindings<'_>,
        probe_uniforms: &wgpu::Buffer,
    ) -> Self {
        let count = set.boxes.len() as u32;
        let size = composed_size(set.size, tier);
        let mips = composed_mips(size);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflection probes"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 * count },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: STORAGE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("reflection probes"),
            dimension: Some(read.dimension()),
            ..Default::default()
        });
        let probe_view = |probe: u32, mip: u32, dimension| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("reflection probe"),
                dimension: Some(dimension),
                base_mip_level: mip,
                mip_level_count: Some(1),
                base_array_layer: 6 * probe,
                array_layer_count: Some(6),
                ..Default::default()
            })
        };
        // Each level's prefilter: the reflection cube's lobes over this chain, its faces from the first.
        let prefilter_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reflection probe prefilter"),
            size: super::REGION_ALIGNMENT * u64::from(mips),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for (mip, uniforms) in super::prefilter_values(size, mips, 0).iter().enumerate().take(mips as usize) {
            let uniforms = super::PrefilterUniforms {
                mip: [uniforms.mip[0], uniforms.mip[1], uniforms.mip[2], 0.0],
                ..*uniforms
            };
            queue.write_buffer(
                &prefilter_uniforms,
                super::REGION_ALIGNMENT * mip as u64,
                bytemuck::bytes_of(&uniforms),
            );
        }
        let (uniform_buffer, compose_region) = bindings.uniforms;
        let groups = (0..count)
            .map(|probe| {
                let target = probe_view(probe, 0, wgpu::TextureViewDimension::D2Array);
                let compose = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("reflection probe compose"),
                    layout: bindings.compose,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 2, resource: bindings.sky_light.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(bindings.sky_cube),
                        },
                        wgpu::BindGroupEntry {
                            binding: 7,
                            resource: wgpu::BindingResource::TextureView(&target),
                        },
                        wgpu::BindGroupEntry {
                            binding: 9,
                            resource: wgpu::BindingResource::TextureView(&set.views[probe as usize]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 10,
                            resource: super::region(
                                uniform_buffer,
                                compose_region,
                                size_of::<ComposeUniforms>(),
                            ),
                        },
                    ],
                });
                let prefilter = (1..mips)
                    .map(|mip| {
                        let source = probe_view(probe, mip - 1, wgpu::TextureViewDimension::Cube);
                        let target = probe_view(probe, mip, wgpu::TextureViewDimension::D2Array);
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("reflection probe prefilter"),
                            layout: bindings.prefilter,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: bindings.sky_light.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 4,
                                    resource: wgpu::BindingResource::TextureView(&source),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 6,
                                    resource: super::region(
                                        &prefilter_uniforms,
                                        super::REGION_ALIGNMENT * u64::from(mip),
                                        size_of::<super::PrefilterUniforms>(),
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 7,
                                    resource: wgpu::BindingResource::TextureView(&target),
                                },
                            ],
                        })
                    })
                    .collect();
                ProbeGroups { compose, prefilter }
            })
            .collect();
        let table = CellTable::build(&set.boxes);
        let cells = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reflection probe cells"),
            size: wgpu::Extent3d {
                width: table.dims[0],
                height: table.dims[1],
                depth_or_array_layers: table.dims[2],
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: CELL_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            cells.as_image_copy(),
            table.cells.as_flattened(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(2 * table.dims[0]),
                rows_per_image: Some(table.dims[1]),
            },
            cells.size(),
        );
        let mut uniforms = ProbeUniforms::zeroed();
        uniforms.cells_origin = table.origin.extend(table.side).to_array();
        uniforms.cells = [table.dims[0], table.dims[1], table.dims[2], count];
        for (gpu, probe) in uniforms.probes.iter_mut().zip(&set.boxes) {
            *gpu = GpuProbe {
                box_min: probe.min.extend(0.0).to_array(),
                box_max: probe.max.extend(probe.fade()).to_array(),
                position: probe.position.extend(0.0).to_array(),
            };
        }
        queue.write_buffer(probe_uniforms, 0, bytemuck::bytes_of(&uniforms));
        Self {
            handle,
            size,
            mips,
            step: (set.size / size).max(1),
            keys: set.keys,
            view,
            cells: cells.create_view(&wgpu::TextureViewDescriptor::default()),
            groups,
            boxes: set.boxes.clone(),
            picker: ProbePicker { boxes: set.boxes.clone() },
            schedule: ComposeSchedule::new(count as usize, frozen),
            bytes: composed_bytes(count, size),
            _prefilter_uniforms: prefilter_uniforms,
        }
    }

    /// The composition's uniforms for a frame blending `keys`.
    pub(crate) fn compose_uniforms(&self, keys: ProbeKeys) -> ComposeUniforms {
        let mut uniforms =
            ComposeUniforms { size: [self.size, self.step, 0, 0], ..ComposeUniforms::default() };
        for (slot, (&key, &weight)) in keys.keys.iter().zip(&keys.weights).enumerate() {
            if key < self.keys && weight > 0.0 {
                uniforms.keys[slot] = 6 * key;
                uniforms.weights[slot] = weight;
            }
        }
        uniforms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(position: Vec3, min: Vec3, max: Vec3) -> ProbeBox {
        ProbeBox { position, min, max }
    }

    #[test]
    fn cells_name_the_probe_reaching_them_and_the_nearest_where_several_do() {
        // A hall from x = 0 to 6 and a yard from 4 to 12 that overlaps it by 2 m, both 3 m high and 2 deep:
        // each reaches 0.5 m past its box (a quarter of 2 m), so the table starts at -0.5 in 1 m cells.
        let hall = probe(Vec3::new(1.0, 1.5, 1.0), Vec3::ZERO, Vec3::new(6.0, 3.0, 2.0));
        let yard = probe(Vec3::new(10.0, 1.5, 1.0), Vec3::new(4.0, 0.0, 0.0), Vec3::new(12.0, 3.0, 2.0));
        let boxes = [hall, yard];
        let table = CellTable::build(&boxes);
        assert_eq!((table.origin, table.side, table.dims), (Vec3::splat(-0.5), 1.0, [13, 4, 3]));
        assert_eq!(table.probe_at(&boxes, Vec3::new(0.5, 0.5, 0.5)), Some(0));
        assert_eq!(table.probe_at(&boxes, Vec3::new(11.5, 2.5, 1.5)), Some(1));
        // A wall on the box's face, and the cell past it that the fade reaches into.
        assert_eq!(table.probe_at(&boxes, Vec3::new(-0.2, 1.0, 1.0)), Some(0));
        assert_eq!(table.probe_at(&boxes, Vec3::new(12.2, 1.0, 1.0)), Some(1));
        // Where both reach, the probe baked nearer the cell's centre: the cell from 4.5 to 5.5 is centred
        // 4 m from the hall's and 5 from the yard's.
        assert_eq!(table.probe_at(&boxes, Vec3::new(4.6, 1.5, 1.5)), Some(0));
        let boxes =
            [hall, probe(Vec3::new(7.0, 1.5, 1.0), Vec3::new(4.0, 0.0, 0.0), Vec3::new(12.0, 3.0, 2.0))];
        let table = CellTable::build(&boxes);
        assert_eq!(table.probe_at(&boxes, Vec3::new(4.6, 1.5, 1.5)), Some(1), "now nearer the yard's");
        // Outside the table, and a cell of the table no probe reaches.
        assert_eq!(table.probe_at(&boxes, Vec3::new(-1.0, 1.0, 1.0)), None);
        assert_eq!(table.probe_at(&boxes, Vec3::new(13.0, 1.0, 1.0)), None);
        let boxes =
            [hall, probe(Vec3::new(10.0, 1.5, 1.0), Vec3::new(8.0, 0.0, 0.0), Vec3::new(12.0, 3.0, 2.0))];
        let apart = CellTable::build(&boxes);
        assert_eq!(
            apart.probe_at(&boxes, Vec3::new(7.0, 1.5, 1.5)),
            None,
            "between the reaches, 6.5 and 7.5"
        );
    }

    #[test]
    fn cells_grow_until_a_large_zone_fits() {
        let huge = probe(Vec3::splat(48.0), Vec3::ZERO, Vec3::new(1000.0, 96.0, 1000.0));
        let table = CellTable::build(&[huge]);
        let cells = table.dims.iter().map(|&d| d as usize).product::<usize>();
        assert!(cells <= MAX_CELLS && table.side == 8.0, "{table:?}");
        assert_eq!(table.cells.len(), cells);
        assert!(table.cells.iter().all(|&cell| cell == [0, NO_CELL_PROBE]));
    }

    /// Inside one box, by a wall it shares with a neighbour whose cube reaches past it, a point reads
    /// its own box's probe whole, never the neighbour's fade: the neighbour's probe baked nearer the
    /// cell's centre used to take the cell, so a strip of the hall's floor faded toward the yard's cube.
    #[test]
    fn a_point_inside_one_box_never_reads_a_neighbours_fade() {
        // A hall from x = 0 to 10 baked at 2, and a yard beside it from 10 to 14 baked by the shared
        // wall: the yard reaches 0.5 m into the hall.
        let hall = probe(Vec3::new(2.0, 1.5, 2.0), Vec3::ZERO, Vec3::new(10.0, 3.0, 4.0));
        let yard = probe(Vec3::new(10.5, 1.5, 2.0), Vec3::new(10.0, 0.0, 0.0), Vec3::new(14.0, 3.0, 4.0));
        let boxes = [hall, yard];
        let table = CellTable::build(&boxes);
        for step in 0..=200 {
            let x = 8.0 + 0.01 * step as f32;
            for point in [Vec3::new(x, 0.0, 2.0), Vec3::new(x, 1.3, 0.7)] {
                let read = table.probe_at(&boxes, point).map(|probe| boxes[probe].weight(point));
                assert_eq!(read, Some(1.0), "at {point}: the hall whole");
            }
            let point = Vec3::new(20.0 - x, 0.0, 2.0);
            let read = table.probe_at(&boxes, point).map(|probe| boxes[probe].weight(point));
            assert_eq!(read, Some(1.0), "at {point}: the yard whole");
        }
    }

    #[test]
    fn a_character_picks_the_box_it_stands_in_fading_out_past_its_faces() {
        let hall = probe(Vec3::new(1.0, 1.5, 1.0), Vec3::ZERO, Vec3::new(6.0, 3.0, 4.0));
        let picker = ProbePicker { boxes: vec![hall] };
        assert_eq!(picker.pick(Vec3::new(3.0, 1.0, 2.0)), (0, 1.0));
        assert_eq!(picker.pick(Vec3::new(6.0, 1.0, 2.0)), (0, 1.0), "on the box's face, as a wall is");
        assert_eq!(picker.pick(Vec3::new(9.0, 1.0, 2.0)), (NO_PROBE, 0.0));
        // The fade is a quarter of the least side, at most half a metre: 0.5 m here, past the face.
        let (probe, weight) = picker.pick(Vec3::new(6.25, 1.0, 2.0));
        assert_eq!(probe, 0);
        assert!((weight - 0.5).abs() < 1e-5, "{weight}");
        // Past an edge the distance is to the edge: 0.3 and 0.4 m out are 0.5 m from it.
        let (_, weight) = picker.pick(Vec3::new(6.3, 1.0, 4.4));
        assert!(weight.abs() < 1e-5, "{weight}");
        assert_eq!(picker.pick(Vec3::new(6.6, 1.0, 2.0)), (NO_PROBE, 0.0));
    }

    #[test]
    fn a_surface_on_a_box_face_takes_its_probe_whole() {
        // The shelter's floor and walls lie on its probe's box (spec §5.12): they reflect the probe, not the
        // sky.
        let shelter =
            probe(Vec3::new(-24.0, 1.6, 30.0), Vec3::new(-27.0, 0.0, 27.7), Vec3::new(-21.0, 3.5, 32.0));
        for point in [Vec3::new(-24.0, 0.0, 30.0), Vec3::new(-27.0, 1.0, 30.0), Vec3::new(-24.0, 3.5, 32.0)] {
            assert_eq!(shelter.weight(point), 1.0, "{point}");
        }
        let table = CellTable::build(&[shelter]);
        assert_eq!(table.probe_at(&[shelter], Vec3::new(-24.0, 0.0, 30.0)), Some(0));
    }

    fn keys(a: u32, b: u32, t: f32) -> ProbeKeys {
        ProbeKeys { keys: [a, b, 0, 0], weights: [1.0 - t, t, 0.0, 0.0] }
    }

    #[test]
    fn a_shown_set_composes_whole_then_one_probe_a_frame_nearest_first_and_nothing_while_current() {
        let boxes: Vec<ProbeBox> = (0..4)
            .map(|i| {
                let x = 10.0 * i as f32;
                probe(Vec3::new(x, 1.0, 0.0), Vec3::new(x - 4.0, 0.0, -4.0), Vec3::new(x + 4.0, 3.0, 4.0))
            })
            .collect();
        let eye = Some(Vec3::new(31.0, 1.0, 0.0));
        let sky = Vec3::ONE;
        let mut schedule = ComposeSchedule::new(4, false);
        assert_eq!(
            schedule.frame(keys(0, 1, 0.5), sky, 6, eye, &boxes),
            0b1111,
            "the whole set the first frame"
        );
        assert!(!schedule.any_due());
        assert_eq!(schedule.frame(keys(0, 1, 0.5), sky, 0, eye, &boxes), 0, "nothing while nothing changes");
        // The hour moves a little: one a frame, nearest the camera first, every probe once.
        let mut order = Vec::new();
        for frame in 0..4 {
            let t = 0.5 + 0.001 * (frame + 1) as f32;
            order.push(schedule.frame(keys(0, 1, t), sky, 0, eye, &boxes));
        }
        assert_eq!(order, [0b1000, 0b0100, 0b0010, 0b0001], "oldest first, then nearest");
        let t = 0.504;
        for _ in 0..4 {
            schedule.frame(keys(0, 1, t), sky, 0, eye, &boxes);
        }
        assert_eq!(
            schedule.frame(keys(0, 1, t), sky, 0, eye, &boxes),
            0,
            "settled: a probe a frame, one turn"
        );
        // A capture finishes every probe out of date at once.
        schedule.frame(keys(0, 1, 0.6), sky, 0, eye, &boxes);
        schedule.finish();
        assert_eq!(schedule.frame(keys(0, 1, 0.6), sky, 0, eye, &boxes), 0b0111, "the three still due");
    }

    #[test]
    fn a_jump_starts_again_from_the_nearest_and_waits_for_the_sky_cube() {
        let boxes: Vec<ProbeBox> = (0..3)
            .map(|i| {
                probe(Vec3::new(i as f32, 1.0, 0.0), Vec3::new(-4.0, 0.0, -4.0), Vec3::new(8.0, 3.0, 4.0))
            })
            .collect();
        let mut schedule = ComposeSchedule::new(3, false);
        let eye = Some(Vec3::new(1.2, 1.0, 0.0));
        schedule.frame(keys(0, 1, 0.0), Vec3::ONE, 6, eye, &boxes);
        // Half way along a probe's composition, the hour jumps (a trip, a cutscene's cut): the nearest
        // first.
        schedule.frame(keys(0, 1, 0.001), Vec3::ONE, 1, eye, &boxes);
        assert_eq!(schedule.frame(keys(2, 3, 0.0), Vec3::ONE, 1, eye, &boxes), 0b010);
        // The authored sky jumps: while the sky cube redraws its six faces every frame is a change.
        assert_eq!(schedule.frame(keys(2, 3, 0.0), Vec3::splat(4.0), 1, eye, &boxes), 0b010);
        for _ in 0..5 {
            assert_ne!(schedule.frame(keys(2, 3, 0.0), Vec3::splat(4.0), 1, eye, &boxes), 0);
        }
        // The frame the last face drew composed one under the whole sky; then the other two, and nothing.
        let rest: Vec<u64> =
            (0..3).map(|_| schedule.frame(keys(2, 3, 0.0), Vec3::splat(4.0), 1, eye, &boxes)).collect();
        assert_eq!(rest, [0b010, 0b100, 0]);
    }

    #[test]
    fn mobile_low_composes_once_from_the_nearest_key() {
        let boxes = [probe(Vec3::ZERO, Vec3::splat(-1.0), Vec3::ONE)];
        let mut schedule = ComposeSchedule::new(1, true);
        assert_eq!(schedule.frame(keys(0, 1, 0.3), Vec3::ONE, 6, None, &boxes), 1);
        for frame in 0..10 {
            assert_eq!(schedule.frame(keys(0, 1, 0.3 + 0.01 * frame as f32), Vec3::ONE, 1, None, &boxes), 0);
        }
        assert_eq!(composed_keys(keys(4, 5, 0.3), true), ProbeKeys::single(4));
        assert_eq!(composed_keys(keys(4, 5, 0.7), true), ProbeKeys::single(5));
        assert_eq!(composed_keys(keys(4, 5, 0.7), false), keys(4, 5, 0.7));
        assert_eq!(composed_keys(ProbeKeys::default(), false), ProbeKeys::single(0), "none given: the first");
    }

    #[test]
    fn composed_cubes_end_at_eight_texels() {
        assert_eq!(composed_size(128, 128), 128);
        assert_eq!(composed_size(64, 128), 64, "a phone's cook on a desktop tier");
        assert_eq!(composed_size(64, 32), 32, "Mobile-Low");
        assert_eq!(
            (composed_mips(128), composed_mips(64), composed_mips(32), composed_mips(8)),
            (5, 4, 3, 1)
        );
        // The numbers the spec gives: 1,023, 255 and 63 KiB a probe.
        assert_eq!(composed_bytes(1, 128), 1_047_552);
        assert_eq!(composed_bytes(1, 64), 261_120);
        assert_eq!(composed_bytes(1, 32), 64_512);
    }
}
