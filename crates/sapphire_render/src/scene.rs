//! The persistent GPU scene (spec §5.12 "Scene and draws"): each mesh instance keeps one slot
//! of the instance buffer from the packet that adds it until the packet that removes it.
//!
//! - [`InstanceSlots`] maps stable [`InstanceId`]s to slots, reusing freed slots through a free
//!   list, and keeps a CPU mirror of the instance buffer. Applying a packet's deltas rewrites
//!   only the slots they touch and marks those for upload. It also lists the slots whose mesh
//!   changed or that were freed, for the per-instance skinning state that follows them.
//! - Each slot keeps the transform it was last drawn with, for velocity (M1 R2): a moved
//!   instance is uploaded with both, and once more when it comes to rest.
//! - [`DrawList`] turns the live slots into draw records and draw commands. Draws name their
//!   instance by slot, so a transform change leaves them as they are: the list is rebuilt only
//!   when instances are added or removed, change mesh, meshes are uploaded, a material
//!   changes permutation, or an instance starts or stops fading (M1 C7: a fading instance's
//!   opaque anime character draws go through the alpha-tested variants, which can discard; see
//!   the `characters` module).
//! - [`GpuScene`] owns both, plus the instance and draw-record buffers. `prepare` applies a
//!   packet and sizes the instance buffer, and `build_draws`, once the frame's characters have
//!   given each slot its fade, rebuilds the draw list if it is stale; the frame's uploads pass
//!   then writes the dirty slots, in contiguous runs, and the draw records after a rebuild,
//!   through the staging belt.
//! - Both buffers stay within their device limits (review RA-004): slots past the instance
//!   buffer's are neither uploaded nor drawn, and the draw list takes whole instances, in slot
//!   order, while their draws fit the records' ([`DrawLimits`]).
//!
//! The commands are for the CPU-driven submission path, which is portable everywhere: each
//! draw binds the mesh's vertices and its draw record by buffer offset. The GPU-driven path
//! (later) writes equivalent multi-draw-indirect records on the GPU, using `base_vertex` and
//! `first_instance` instead (see the crate docs).
//!
//! Draws of static meshes are grouped by position format, whose pipelines and stream slab
//! differ, and within a format by surface variant ([`SurfaceVariant`]: the material's family and
//! its static permutation), each its own pipeline, so draws sort by pipeline (§5.12) whatever
//! order their instances and submeshes come in, and families mix freely within a mesh; within a
//! group they come in slot order, except that an eye's layers (M1 C6) come in their layers'
//! order, each over the ones before it. Draws of skinned or morphed meshes form groups of their own,
//! per variant: each instance draws from its own skinning outputs (f32 positions), so they bind
//! by instance slot rather than by mesh. Each draw record carries its mesh's dequantization, so
//! the vertex shaders decode any mesh without another binding. Sorting by material and mesh
//! within a group (§5.12) will cost nothing on frames that only move instances, since the list
//! is rebuilt only when the set changes; sorting transparent draws back to front depends on
//! where things are, so it will run every frame that has them.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Range;

use bytemuck::Zeroable;
use glam::Mat4;
use sapphire_assets::material::{MaterialFamily, MaterialFlags};
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer};
use crate::gpu_types::{DrawData, InstanceData};
use crate::materials::MaterialHandle;
use crate::mesh_pool::{MeshRecord, MeshStreams};
use crate::pipelines::{Coverage, Shading, SurfaceVariant, format_index, mesh_draw};
use crate::{InstanceDeltas, InstanceId, MeshHandle};

const INSTANCE_BYTES: u64 = size_of::<InstanceData>() as u64;
const DRAW_BYTES: u64 = size_of::<DrawData>() as u64;

/// The instance and draw-record buffers and the CPU state they mirror.
#[derive(Debug)]
pub(crate) struct GpuScene {
    slots: InstanceSlots,
    draw_list: DrawList,
    instances: GrowableBuffer,
    /// Draw records, read as instance-step vertex data (vertex slot 1).
    draws: GrowableBuffer,
    /// Something the draw list depends on changed since it was built.
    draws_stale: bool,
    /// The draw list was rebuilt since its records were last uploaded.
    draws_unsent: bool,
    /// The last packet applied, to check none is skipped.
    last_frame: Option<u64>,
    /// Instances the last packet added or changed.
    changed: u32,
    /// Draw list rebuilds since creation.
    builds: u32,
    /// Live instances in slots past the instance buffer's limit, which don't draw.
    instances_over_limit: u32,
    /// An instance or a draw past its buffer's limit was logged: each is logged once.
    warned: [bool; 2],
}

impl GpuScene {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limits = device.limits();
        Self {
            slots: InstanceSlots::default(),
            draw_list: DrawList::default(),
            instances: GrowableBuffer::new(
                device,
                "instances",
                wgpu::BufferUsages::STORAGE,
                256 * INSTANCE_BYTES,
                limits.max_storage_buffer_binding_size.min(limits.max_buffer_size),
            ),
            draws: GrowableBuffer::new(
                device,
                "draw records",
                wgpu::BufferUsages::VERTEX,
                1024 * DRAW_BYTES,
                limits.max_buffer_size,
            ),
            draws_stale: false,
            draws_unsent: false,
            last_frame: None,
            changed: 0,
            builds: 0,
            instances_over_limit: 0,
            warned: [false; 2],
        }
    }

    /// The slots the instance buffer can hold: instances in later slots don't draw.
    fn slot_limit(&self) -> u32 {
        u32::try_from(self.instances.max_items(INSTANCE_BYTES)).unwrap_or(u32::MAX)
    }

    /// Applies packet `frame`'s instance deltas, marking the draw list stale if the drawn set
    /// changed, and makes room in the instance buffer. Returns `true` when the instance buffer
    /// was replaced, so bind groups that reference it must be rebuilt. [`Self::build_draws`]
    /// follows, once the frame's characters have given their instances their fades.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: u64,
        deltas: &InstanceDeltas,
    ) -> bool {
        profiling::scope!("GpuScene::prepare");
        debug_assert!(
            self.last_frame.is_none_or(|last| frame == last + 1),
            "packet {frame} after packet {:?}: every packet must be prepared, in order, or its deltas are lost",
            self.last_frame
        );
        self.last_frame = Some(frame);
        self.changed = to_u32(deltas.changed.len());
        if self.slots.apply(deltas) {
            self.draws_stale = true;
        }
        // Slots past the buffer's limit (a storage binding: 699 050 instances on the device floor)
        // are neither uploaded nor drawn (review RA-004), so the buffer never needs more.
        let limit = self.slot_limit();
        self.instances_over_limit = to_u32(self.slots.live_past(limit));
        if self.instances_over_limit > 0 && !std::mem::replace(&mut self.warned[0], true) {
            tracing::warn!(
                target: crate::LOG_TARGET,
                instances = self.instances_over_limit,
                limit,
                "the instance buffer is full: instances past its limit don't draw"
            );
        }
        let held = (self.slots.len() as u64).min(u64::from(limit));
        // The old buffer holds every slot uploaded so far: a GPU copy keeps them all without
        // sending them again.
        let uploaded = self.instances.buffer().size();
        self.instances.reserve(device, queue, held * INSTANCE_BYTES, uploaded)
    }

    /// Rebuilds the draw list if the drawn set changed since it was built, and makes room for
    /// its records. `material` gives a material's family, flags and eye layer
    /// (`DrawList::build`'s `material_kind` and `layer`). Returns whether it was rebuilt.
    pub(crate) fn build_draws(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        meshes: &[MeshRecord],
        material: impl Fn(MaterialHandle) -> (MaterialFamily, MaterialFlags, u32),
    ) -> bool {
        if !self.draws_stale {
            return false;
        }
        let material_kind = |handle| {
            let (family, flags, _) = material(handle);
            (family, flags)
        };
        let max_draws = usize::try_from(self.draws.max_items(DRAW_BYTES)).unwrap_or(usize::MAX);
        let limits = DrawLimits { slots: self.slot_limit(), draws: max_draws };
        self.draw_list.build(meshes, material_kind, |handle| material(handle).2, &self.slots, limits);
        if self.draw_list.over_limit > 0 && !std::mem::replace(&mut self.warned[1], true) {
            tracing::warn!(
                target: crate::LOG_TARGET,
                draws = self.draw_list.over_limit,
                limit = max_draws,
                "the draw records are full: instances whose draws pass their limit don't draw"
            );
        }
        // The records are uploaded whole after every rebuild, so nothing needs keeping. They fit:
        // the list holds no more than the limit.
        self.draws.reserve(device, queue, size_of_val(self.draw_list.draws.as_slice()) as u64, 0);
        self.draws_stale = false;
        self.draws_unsent = true;
        self.builds += 1;
        true
    }

    /// Meshes were uploaded or moved, a material changed its surface variant, or an instance
    /// started or stopped fading: rebuild the draw list on the next `build_draws`.
    pub(crate) fn meshes_changed(&mut self) {
        self.draws_stale = true;
    }

    /// Mesh `mesh` was replaced in place (hot reload, live link): the next `prepare` lists its
    /// instances as given a mesh again, so what follows meshes (skinning outputs) starts over, and
    /// rebuilds the draw list.
    pub(crate) fn mesh_replaced(&mut self, mesh: MeshHandle) {
        self.slots.remesh(mesh);
        self.draws_stale = true;
    }

    pub(crate) fn instance_buffer(&self) -> &wgpu::Buffer {
        self.instances.buffer()
    }

    /// The instances by slot, as the last `prepare` left them.
    pub(crate) fn slots(&self) -> &InstanceSlots {
        &self.slots
    }

    /// The instances by slot, to point them at their character entries after `prepare`.
    pub(crate) fn slots_mut(&mut self) -> &mut InstanceSlots {
        &mut self.slots
    }

    /// Splits the scene between a frame's passes: the uploads pass writes what changed, and
    /// the draw passes read the draw list and its record buffer.
    pub(crate) fn split_for_record(&mut self) -> (SceneUpload<'_>, &DrawList, &GrowableBuffer) {
        let limit = self.slot_limit();
        let Self { slots, draw_list, instances, draws, draws_unsent, .. } = self;
        let upload = SceneUpload {
            slots,
            limit,
            instances: instances.buffer(),
            draw_records: (&draw_list.draws, draws.buffer()),
            draws_unsent,
        };
        (upload, draw_list, draws)
    }

    /// Instances drawn.
    pub(crate) fn live_instances(&self) -> u32 {
        to_u32(self.slots.live_count())
    }

    /// Live instances in slots past the instance buffer's limit after the last `prepare`: they
    /// don't draw.
    pub(crate) fn instances_over_limit(&self) -> u32 {
        self.instances_over_limit
    }

    /// Surface draws, per [`SurfaceVariant::ALL`] entry.
    pub(crate) fn draw_counts(&self) -> [u32; SurfaceVariant::ALL.len()] {
        SurfaceVariant::ALL.map(|variant| to_u32(self.draw_list.len(variant)))
    }

    /// The draws of the live instances, as the last `prepare` left them.
    pub(crate) fn draw_list(&self) -> &DrawList {
        &self.draw_list
    }

    /// Instances the last packet added or changed.
    pub(crate) fn changed_instances(&self) -> u32 {
        self.changed
    }

    pub(crate) fn draw_list_builds(&self) -> u32 {
        self.builds
    }

    /// Times either buffer grew.
    pub(crate) fn growths(&self) -> u32 {
        self.instances.growths() + self.draws.growths()
    }

    /// New buffers that hold at most `instances` instances and `draws` draw records, for tests
    /// of full ones. Before the first instance is added.
    #[cfg(test)]
    pub(crate) fn limit(&mut self, device: &wgpu::Device, instances: u64, draws: u64) {
        assert_eq!(self.slots.len(), 0, "limit the scene before adding instances");
        self.instances = self.instances.limited(device, 256 * INSTANCE_BYTES, instances * INSTANCE_BYTES);
        self.draws = self.draws.limited(device, 1024 * DRAW_BYTES, draws * DRAW_BYTES);
    }
}

/// The scene's share of a frame's uploads pass.
pub(crate) struct SceneUpload<'a> {
    slots: &'a mut InstanceSlots,
    /// The slots the instance buffer holds.
    limit: u32,
    instances: &'a wgpu::Buffer,
    draw_records: (&'a [DrawData], &'a wgpu::Buffer),
    draws_unsent: &'a mut bool,
}

impl SceneUpload<'_> {
    /// Writes the dirty slots the instance buffer holds and, after a rebuild, the draw records
    /// through `belt`.
    pub(crate) fn write(self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        let Self { slots, limit, instances, draw_records: (records, record_buffer), draws_unsent } = self;
        slots.flush(limit, |first, data| {
            buffer::stage(
                belt,
                encoder,
                instances,
                u64::from(first) * INSTANCE_BYTES,
                bytemuck::cast_slice(data),
            );
        });
        if std::mem::take(draws_unsent) {
            buffer::stage(belt, encoder, record_buffer, 0, bytemuck::cast_slice(records));
        }
    }
}

/// Instances in slots keyed by stable ID, with a free list, and the CPU mirror of their GPU
/// data.
#[derive(Debug, Default)]
pub(crate) struct InstanceSlots {
    /// Per slot: what the instance buffer holds there (stale for free slots).
    data: Vec<InstanceData>,
    /// Per slot.
    slots: Vec<Slot>,
    /// Slots no instance uses, reused most recently freed first.
    free: Vec<u32>,
    ids: HashMap<InstanceId, u32, BuildHasherDefault<IdHasher>>,
    /// Slots written since the last flush, each listed once.
    dirty: Vec<u32>,
    /// Slots the last `apply` freed, then slots it gave a mesh: new instances, mesh changes and
    /// instances of meshes replaced in place. A slot freed and reused within one packet is in
    /// both.
    freed: Vec<u32>,
    meshed: Vec<u32>,
    /// Meshes replaced in place since the last `apply`.
    remeshed: Vec<MeshHandle>,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    /// `None` while the slot is free.
    id: Option<InstanceId>,
    mesh: MeshHandle,
    /// Listed in `InstanceSlots::dirty`.
    dirty: bool,
}

impl InstanceSlots {
    /// Applies removals, then additions and changes. Returns `true` when the set of drawn
    /// meshes changed (an instance was added or removed, or changed mesh), which invalidates
    /// the draw list. A transform change only marks its slot for upload.
    pub(crate) fn apply(&mut self, deltas: &InstanceDeltas) -> bool {
        let mut drawn_changed = false;
        self.freed.clear();
        self.meshed.clear();
        for id in &deltas.removed {
            if let Some(slot) = self.ids.remove(id) {
                self.slots[slot as usize].id = None;
                self.free.push(slot);
                self.freed.push(slot);
                drawn_changed = true;
            }
        }
        if !self.remeshed.is_empty() {
            for (index, slot) in self.slots.iter().enumerate() {
                if slot.id.is_some() && self.remeshed.contains(&slot.mesh) {
                    self.meshed.push(to_u32(index));
                }
            }
            self.remeshed.clear();
            drawn_changed = true;
        }
        for instance in &deltas.changed {
            let slot = match self.ids.get(&instance.id) {
                Some(&slot) => {
                    let mesh = &mut self.slots[slot as usize].mesh;
                    if *mesh != instance.mesh {
                        *mesh = instance.mesh;
                        self.meshed.push(slot);
                        drawn_changed = true;
                    }
                    self.data[slot as usize].set_transform(instance.transform);
                    slot
                }
                None => {
                    let slot = self.allocate(instance.id, instance.mesh);
                    self.ids.insert(instance.id, slot);
                    self.meshed.push(slot);
                    drawn_changed = true;
                    // A new instance has no motion to draw.
                    self.data[slot as usize] = InstanceData::new(instance.transform);
                    slot
                }
            };
            let entry = &mut self.slots[slot as usize];
            if !entry.dirty {
                entry.dirty = true;
                self.dirty.push(slot);
            }
        }
        drawn_changed
    }

    /// Points the live instance in `slot` at entry `index` of the frame's character buffer (0,
    /// the world's key light, for none), marking it for upload if that changed. Free slots are
    /// left alone.
    pub(crate) fn set_character(&mut self, slot: u32, index: u32) {
        let Some(entry) = self.slots.get_mut(slot as usize).filter(|entry| entry.id.is_some()) else {
            return;
        };
        let data = &mut self.data[slot as usize];
        if data.character != index {
            data.character = index;
            if !entry.dirty {
                entry.dirty = true;
                self.dirty.push(slot);
            }
        }
    }

    /// The character entry the instance in `slot` reads.
    #[cfg(test)]
    pub(crate) fn character(&self, slot: u32) -> u32 {
        self.data[slot as usize].character
    }

    /// Gives the live instance in `slot` its character's fade (M1 C7; 0 drawn whole), marking it
    /// for upload if that changed. Returns whether it started or stopped fading, which moves its
    /// opaque draws between variants. Free slots are left alone.
    pub(crate) fn set_fade(&mut self, slot: u32, fade: f32) -> bool {
        let Some(entry) = self.slots.get_mut(slot as usize).filter(|entry| entry.id.is_some()) else {
            return false;
        };
        let data = &mut self.data[slot as usize];
        if data.fade == fade {
            return false;
        }
        let flipped = (data.fade > 0.0) != (fade > 0.0);
        data.fade = fade;
        if !entry.dirty {
            entry.dirty = true;
            self.dirty.push(slot);
        }
        flipped
    }

    /// Whether the instance in `slot` fades (M1 C7).
    pub(crate) fn faded(&self, slot: u32) -> bool {
        self.data.get(slot as usize).is_some_and(|data| data.fade > 0.0)
    }

    /// Lists every live instance of `mesh` as given a mesh by the next [`Self::apply`].
    pub(crate) fn remesh(&mut self, mesh: MeshHandle) {
        if !self.remeshed.contains(&mesh) {
            self.remeshed.push(mesh);
        }
    }

    fn allocate(&mut self, id: InstanceId, mesh: MeshHandle) -> u32 {
        let slot = self.free.pop().unwrap_or_else(|| {
            self.slots.push(Slot { id: None, mesh, dirty: false });
            self.data.push(InstanceData::zeroed());
            to_u32(self.slots.len() - 1)
        });
        let entry = &mut self.slots[slot as usize];
        entry.id = Some(id);
        entry.mesh = mesh;
        slot
    }

    /// Slots in use or free: the instance buffer must hold this many.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn live_count(&self) -> usize {
        self.ids.len()
    }

    /// Live instances in slot `limit` or later.
    pub(crate) fn live_past(&self, limit: u32) -> usize {
        self.slots
            .get(limit as usize..)
            .map_or(0, |past| past.iter().filter(|slot| slot.id.is_some()).count())
    }

    /// The slot of a live instance.
    pub(crate) fn slot(&self, id: InstanceId) -> Option<u32> {
        self.ids.get(&id).copied()
    }

    /// The mesh of the live instance in `slot`.
    pub(crate) fn mesh(&self, slot: u32) -> Option<MeshHandle> {
        self.slots.get(slot as usize).filter(|entry| entry.id.is_some()).map(|entry| entry.mesh)
    }

    /// Slots the last [`Self::apply`] freed.
    pub(crate) fn freed(&self) -> &[u32] {
        &self.freed
    }

    /// Slots the last [`Self::apply`] gave a mesh, after [`Self::freed`]: added instances and
    /// changed meshes.
    pub(crate) fn meshed(&self) -> &[u32] {
        &self.meshed
    }

    /// The transform of the live instance in `slot`.
    pub(crate) fn transform(&self, slot: u32) -> Option<Mat4> {
        self.slots
            .get(slot as usize)
            .filter(|entry| entry.id.is_some())
            .map(|_| Mat4::from_cols_array_2d(&self.data[slot as usize].world_from_object))
    }

    /// Live slots with their meshes and transforms, in slot order.
    pub(crate) fn live_transforms(&self) -> impl Iterator<Item = (u32, MeshHandle, Mat4)> + '_ {
        self.live().map(|(slot, mesh)| {
            (slot, mesh, Mat4::from_cols_array_2d(&self.data[slot as usize].world_from_object))
        })
    }

    /// Live slots and their meshes, in slot order.
    pub(crate) fn live(&self) -> impl Iterator<Item = (u32, MeshHandle)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.id.is_some())
            .map(|(index, slot)| (to_u32(index), slot.mesh))
    }

    /// Hands each run of consecutive dirty slots below `limit` to `write` (first slot, data), for
    /// a frame that draws them; slots past it are past the instance buffer's, and don't draw. The
    /// slots then settle: one that moved stays dirty, so the next drawn frame uploads it at rest
    /// (its previous transform equal to its current one) unless it moves again; the others are
    /// marked clean.
    fn flush(&mut self, limit: u32, mut write: impl FnMut(u32, &[InstanceData])) {
        self.dirty.sort_unstable();
        let held = self.dirty.partition_point(|&slot| slot < limit);
        for run in self.dirty[..held].chunk_by(|&a, &b| b == a + 1) {
            let (first, last) = (run[0], run[run.len() - 1]);
            write(first, &self.data[first as usize..=last as usize]);
        }
        let Self { data, slots, dirty, .. } = self;
        dirty.retain(|&slot| {
            let moved = data[slot as usize].settle();
            slots[slot as usize].dirty = moved;
            moved
        });
    }
}

/// Hashes [`InstanceId`]s with one multiply by 2⁶⁴/φ (Fibonacci hashing). The IDs are unique
/// integers, typically entity bits with the index in the low half, so they need mixing into
/// the high bits rather than resistance to chosen collisions; SipHash would cost several times
/// more for every delta.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(self.0.rotate_left(8) ^ u64::from(byte));
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// One `draw_indexed` call on the CPU-driven path, with `base_vertex` 0 and instances `0..1`:
/// the mesh's streams (vertex slots 1–3) are bound at `first_vertex` of their slab (indices are
/// mesh-relative), slot 0 at draw record `draw_id`, and group 3 to `material`'s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DrawCommand {
    pub indices: Range<u32>,
    pub first_vertex: u32,
    /// The instance's slot: passes that draw some instances and not others (shadows) pick by it.
    pub slot: u32,
    pub draw_id: u32,
    pub material: MaterialHandle,
    /// It draws an outline hull too
    /// ([`Permutation::outlined`](crate::pipelines::Permutation::outlined)).
    pub outline: bool,
    /// Its mesh's first element in the pool's foliage buffer (M1 W6), which foliage passes bind
    /// as slot 5; 0 for meshes without a foliage stream, which no foliage pass draws.
    pub foliage: u32,
}

/// A draw of a skinned or morphed instance on the CPU-driven path: positions and shading
/// (slots 1 and 2) come from the skinning outputs of the instance in `slot`, the surface stream
/// (slot 3) from its mesh's sources at word `surface` of the deformable buffer, and group 3 from
/// `material`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeformedDraw {
    pub indices: Range<u32>,
    pub slot: u32,
    pub surface: u32,
    pub draw_id: u32,
    pub material: MaterialHandle,
    /// It draws an outline hull too.
    pub outline: bool,
}

/// The draws for every live instance: one per submesh of the mesh's LOD0. Its vectors are
/// reused across rebuilds.
#[derive(Debug)]
pub(crate) struct DrawList {
    pub draws: Vec<DrawData>,
    /// Static meshes, per [`PositionFormat::ALL`](sapphire_assets::mesh::PositionFormat::ALL)
    /// entry, since each format has its own pipelines and slab, then per [`SurfaceVariant::ALL`]
    /// entry.
    pub commands: [[Vec<DrawCommand>; SurfaceVariant::ALL.len()]; 2],
    /// Skinned and morphed meshes, drawn with the f32 pipelines, per [`SurfaceVariant::ALL`]
    /// entry.
    pub deformed: [Vec<DeformedDraw>; SurfaceVariant::ALL.len()],
    /// Instances with at least one foliage draw (M1 W6), for the stats.
    pub foliage_instances: u32,
    /// Anime character draws of fading instances (M1 C7), static and deformed, for the stats:
    /// their opaque ones are drawn through the alpha-tested variants, which the prepasses draw
    /// through their masked pipelines.
    pub faded: u32,
    /// Draws left out because they would pass the draw records' limit ([`DrawLimits`]).
    pub over_limit: u32,
}

impl Default for DrawList {
    fn default() -> Self {
        Self {
            draws: Vec::new(),
            commands: [(); 2].map(|_| std::array::from_fn(|_| Vec::new())),
            deformed: std::array::from_fn(|_| Vec::new()),
            foliage_instances: 0,
            faded: 0,
            over_limit: 0,
        }
    }
}

/// What a draw list may hold, so its buffers stay within their limits (review RA-004): the
/// instances in slots below `slots` (the instance buffer's), and at most `draws` draws (the draw
/// records'). Instances are taken whole, in slot order, until one doesn't fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DrawLimits {
    pub slots: u32,
    pub draws: usize,
}

impl DrawLimits {
    /// No limit.
    #[cfg(test)]
    pub(crate) const NONE: Self = Self { slots: u32::MAX, draws: usize::MAX };
}

impl DrawList {
    /// Rebuilds the list from the live `slots`, drawing each submesh with the surface variant
    /// of its material's family and flags (`material_kind`) on its mesh (`pipelines::mesh_draw`),
    /// or for an opaque anime character surface of a fading instance, its alpha-tested variant.
    /// Eye layers then go in their `layer`'s order (M1 C6), and in slot order within a layer.
    /// Instances past `limits` are left out.
    pub(crate) fn build(
        &mut self,
        meshes: &[MeshRecord],
        material_kind: impl Fn(MaterialHandle) -> (MaterialFamily, MaterialFlags),
        layer: impl Fn(MaterialHandle) -> u32,
        slots: &InstanceSlots,
        limits: DrawLimits,
    ) {
        profiling::scope!("DrawList::build");
        self.draws.clear();
        for commands in self.commands.iter_mut().flatten() {
            commands.clear();
        }
        for deformed in &mut self.deformed {
            deformed.clear();
        }
        self.foliage_instances = 0;
        self.faded = 0;
        self.over_limit = 0;
        let mut full = false;
        // In slot order, so every slot after one past the limit is past it too.
        for (slot, mesh) in slots.live().take_while(|&(slot, _)| slot < limits.slots) {
            let record = meshes.get(mesh.index());
            debug_assert!(record.is_some(), "{mesh:?} was not uploaded to this renderer");
            let Some(record) = record else { continue };
            // Whole instances: once one doesn't fit, it and every later one are left out, so a
            // smaller one never slips in after a bigger one that didn't.
            full = full || limits.draws - self.draws.len() < record.lods[0].len();
            if full {
                self.over_limit += to_u32(record.lods[0].len());
                continue;
            }
            let (skinned, morphed) = record
                .deform()
                .map_or((false, false), |source| (source.skin.is_some(), source.morphs.is_some()));
            let mut foliage = false;
            let faded = slots.faded(slot);
            for submesh in &record.lods[0] {
                let handle = submesh.material.handle;
                let (family, flags) = material_kind(handle);
                let drawn = mesh_draw(family, flags, skinned, morphed);
                debug_assert!(drawn.is_some(), "uploads refuse {family} materials on meshes");
                let Some((mut variant, outline)) = drawn else { continue };
                foliage |= variant.shading == Shading::Foliage;
                if faded && matches!(variant.shading, Shading::Character(_)) {
                    if variant.coverage == Coverage::Opaque {
                        variant = variant.with_coverage(Coverage::AlphaTest);
                    }
                    self.faded += 1;
                }
                let (variant, material) = (variant.index(), handle);
                let draw_id = to_u32(self.draws.len());
                self.draws.push(DrawData {
                    instance: slot,
                    material: submesh.material.slot,
                    dequantize: record.dequantize,
                });
                let indices = submesh.first_index..submesh.first_index + submesh.index_count;
                match &record.streams {
                    MeshStreams::Slab { format, first_vertex } => {
                        let first_vertex = *first_vertex;
                        self.commands[format_index(*format)][variant].push(DrawCommand {
                            indices,
                            first_vertex,
                            slot,
                            draw_id,
                            material,
                            outline,
                            foliage: record.foliage.unwrap_or(0),
                        });
                    }
                    MeshStreams::Deformable(source) => {
                        let surface = source.surface;
                        self.deformed[variant].push(DeformedDraw {
                            indices,
                            slot,
                            surface,
                            draw_id,
                            material,
                            outline,
                        });
                    }
                }
            }
            self.foliage_instances += u32::from(foliage);
        }
        // An eye's shadow, iris and highlight layers each blend over the ones before them,
        // whatever order a mesh lists them in. Draw IDs keep slot order within a layer; the
        // sort is in place, so a rebuild allocates nothing new.
        let layers = SurfaceVariant::EYE_LAYERS.index();
        for commands in &mut self.commands {
            commands[layers].sort_unstable_by_key(|command| (layer(command.material), command.draw_id));
        }
        self.deformed[layers].sort_unstable_by_key(|draw| (layer(draw.material), draw.draw_id));
    }

    /// Surface draws of `variant`.
    pub(crate) fn len(&self, variant: SurfaceVariant) -> usize {
        let index = variant.index();
        self.commands.iter().map(|commands| commands[index].len()).sum::<usize>() + self.deformed[index].len()
    }

    /// Outline hulls.
    pub(crate) fn outlines(&self) -> usize {
        let commands = self.commands.iter().flatten().flatten().filter(|command| command.outline).count();
        commands + self.deformed.iter().flatten().filter(|draw| draw.outline).count()
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("instance and draw counts fit the u32 GPU indices")
}

#[cfg(test)]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::MeshInstance;
    use crate::bounds::Sphere;
    use crate::gpu_types::Dequantize;
    use crate::mesh_pool::{DeformSource, Placement, SubmeshDraw, SubmeshMaterial};
    use crate::pipelines::{Coverage, Part};

    /// Material `slot`, uploaded as handle `slot + 10`.
    fn material(slot: u32) -> SubmeshMaterial {
        SubmeshMaterial { handle: MaterialHandle(slot + 10), slot }
    }

    fn instance(id: u64, mesh: u32, x: f32) -> MeshInstance {
        MeshInstance {
            id: InstanceId(id),
            mesh: MeshHandle::from_raw(mesh),
            transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
        }
    }

    fn deltas(changed: &[MeshInstance], removed: &[u64]) -> InstanceDeltas {
        InstanceDeltas {
            changed: changed.to_vec(),
            removed: removed.iter().copied().map(InstanceId).collect(),
        }
    }

    fn slot_of(slots: &InstanceSlots, id: u64) -> Option<u32> {
        slots.ids.get(&InstanceId(id)).copied()
    }

    /// The slot's mirrored x translation.
    fn x(slots: &InstanceSlots, slot: u32) -> f32 {
        slots.data[slot as usize].world_from_object[3][0]
    }

    /// Flushes and returns the runs written: (first slot, slot count).
    fn flushed(slots: &mut InstanceSlots) -> Vec<(u32, usize)> {
        let mut runs = Vec::new();
        slots.flush(u32::MAX, |first, data| runs.push((first, data.len())));
        runs
    }

    #[test]
    fn adds_get_slots_in_order_and_are_uploaded() {
        let mut slots = InstanceSlots::default();
        assert!(slots.apply(&deltas(&[instance(10, 0, 1.0), instance(20, 1, 2.0)], &[])));
        assert_eq!((slot_of(&slots, 10), slot_of(&slots, 20)), (Some(0), Some(1)));
        assert_eq!((slots.len(), slots.live_count()), (2, 2));
        assert_eq!(x(&slots, 1), 2.0);
        assert_eq!(flushed(&mut slots), [(0, 2)]);
        assert_eq!(flushed(&mut slots), [], "flushing marks the slots clean");
    }

    #[test]
    fn transform_changes_keep_the_slot_and_the_draw_list() {
        let mut slots = InstanceSlots::default();
        slots.apply(&deltas(&[instance(10, 0, 1.0), instance(20, 0, 2.0)], &[]));
        flushed(&mut slots);

        assert!(!slots.apply(&deltas(&[instance(20, 0, 5.0)], &[])), "a move keeps every draw");
        assert_eq!(slot_of(&slots, 20), Some(1));
        assert_eq!(x(&slots, 1), 5.0);
        assert_eq!(flushed(&mut slots), [(1, 1)], "only the moved slot is uploaded");
        assert_eq!(flushed(&mut slots), [(1, 1)], "and again once it comes to rest");
        assert_eq!(flushed(&mut slots), []);

        assert!(slots.apply(&deltas(&[instance(20, 3, 5.0)], &[])), "a mesh change rebuilds the draws");
        assert_eq!(slot_of(&slots, 20), Some(1));
        assert_eq!((slots.freed(), slots.meshed()), (&[][..], &[1][..]));
        assert_eq!(slots.mesh(1), Some(MeshHandle::from_raw(3)));

        // Changed twice before an upload: listed once, with the latest data.
        slots.apply(&deltas(&[instance(10, 0, 7.0)], &[]));
        slots.apply(&deltas(&[instance(10, 0, 8.0)], &[]));
        assert_eq!(x(&slots, 0), 8.0);
        assert_eq!(flushed(&mut slots), [(0, 2)]);
    }

    #[test]
    fn moved_instances_are_drawn_from_where_the_last_drawn_frame_left_them() {
        let previous_x =
            |slots: &InstanceSlots, slot: u32| slots.data[slot as usize].previous_world_from_object[3][0];
        let mut slots = InstanceSlots::default();
        slots.apply(&deltas(&[instance(1, 0, 1.0)], &[]));
        assert_eq!(previous_x(&slots, 0), 1.0, "a new instance starts at rest");
        flushed(&mut slots);

        // Two packets prepared before a frame is drawn: the motion spans both.
        slots.apply(&deltas(&[instance(1, 0, 2.0)], &[]));
        slots.apply(&deltas(&[instance(1, 0, 3.0)], &[]));
        assert_eq!((previous_x(&slots, 0), x(&slots, 0)), (1.0, 3.0));
        let mut written = Vec::new();
        slots.flush(u32::MAX, |_, data| {
            written
                .extend(data.iter().map(|d| (d.previous_world_from_object[3][0], d.world_from_object[3][0])))
        });
        assert_eq!(written, [(1.0, 3.0)]);

        // Held still: drawn once more at rest, then no more uploads.
        written.clear();
        slots.flush(u32::MAX, |_, data| {
            written
                .extend(data.iter().map(|d| (d.previous_world_from_object[3][0], d.world_from_object[3][0])))
        });
        assert_eq!(written, [(3.0, 3.0)]);
        assert_eq!(flushed(&mut slots), []);

        // Moving again right after a move continues from the last drawn frame.
        slots.apply(&deltas(&[instance(1, 0, 4.0)], &[]));
        flushed(&mut slots);
        slots.apply(&deltas(&[instance(1, 0, 6.0)], &[]));
        assert_eq!(previous_x(&slots, 0), 4.0);
    }

    #[test]
    fn removals_free_slots_for_reuse_and_ids_stay_stable() {
        let mut slots = InstanceSlots::default();
        let three = [instance(1, 0, 0.0), instance(2, 0, 0.0), instance(3, 0, 0.0)];
        slots.apply(&deltas(&three, &[]));
        flushed(&mut slots);

        assert!(slots.apply(&deltas(&[], &[1, 2])));
        assert_eq!((slots.freed(), slots.meshed()), (&[0, 1][..], &[][..]));
        assert_eq!((slots.mesh(0), slots.mesh(2)), (None, Some(MeshHandle::from_raw(0))));
        assert_eq!((slots.len(), slots.live_count()), (3, 1));
        assert_eq!(slot_of(&slots, 3), Some(2), "survivors keep their slot");
        assert_eq!(slots.live().collect::<Vec<_>>(), [(2, MeshHandle::from_raw(0))]);
        assert_eq!(flushed(&mut slots), [], "a removal uploads nothing");

        // New instances fill the freed slots, most recently freed first, before growing.
        slots.apply(&deltas(&[instance(4, 0, 0.0), instance(5, 0, 0.0), instance(6, 0, 0.0)], &[]));
        assert_eq!([4, 5, 6].map(|id| slot_of(&slots, id)), [Some(1), Some(0), Some(3)]);
        assert_eq!(slot_of(&slots, 3), Some(2));
        assert_eq!(slots.len(), 4);
    }

    #[test]
    fn a_mesh_replaced_in_place_lists_its_live_instances_once() {
        let mut slots = InstanceSlots::default();
        let instances = [instance(1, 0, 0.0), instance(2, 1, 0.0), instance(3, 0, 0.0)];
        slots.apply(&deltas(&instances, &[]));
        slots.apply(&deltas(&[], &[3]));
        slots.remesh(MeshHandle::from_raw(0));
        slots.remesh(MeshHandle::from_raw(0));
        // The next packet lists mesh 0's live instances as given a mesh, which rebuilds the draw
        // list and starts their skinning outputs over; a removed one isn't.
        assert!(slots.apply(&deltas(&[], &[])));
        assert_eq!(slots.meshed(), [0]);
        assert!(!slots.apply(&deltas(&[], &[])), "only once");
        assert_eq!(slots.meshed(), [] as [u32; 0]);
    }

    #[test]
    fn removed_then_added_in_one_packet_survives_and_unknown_removals_are_ignored() {
        let mut slots = InstanceSlots::default();
        slots.apply(&deltas(&[instance(1, 0, 0.0)], &[]));
        assert!(!slots.apply(&deltas(&[], &[42, 42])), "unknown IDs change nothing");

        // Removals apply first, so the instance comes back (into the slot it just freed).
        assert!(slots.apply(&deltas(&[instance(1, 2, 3.0)], &[1])));
        assert_eq!((slots.freed(), slots.meshed()), (&[0][..], &[0][..]));
        assert_eq!(slot_of(&slots, 1), Some(0));
        assert_eq!(slots.live().collect::<Vec<_>>(), [(0, MeshHandle::from_raw(2))]);
        assert_eq!(x(&slots, 0), 3.0);
    }

    #[test]
    fn character_entries_upload_only_when_they_change() {
        let character = |slots: &InstanceSlots, slot: u32| slots.data[slot as usize].character;
        let mut slots = InstanceSlots::default();
        slots.apply(&deltas(&[instance(1, 0, 0.0), instance(2, 0, 0.0)], &[]));
        flushed(&mut slots);
        assert_eq!(character(&slots, 1), 0, "new instances read the world's key light");

        slots.set_character(1, 3);
        slots.set_character(1, 3);
        assert_eq!(character(&slots, 1), 3);
        assert_eq!(flushed(&mut slots), [(1, 1)], "uploaded once");
        slots.set_character(1, 3);
        assert_eq!(flushed(&mut slots), [], "unchanged: nothing to upload");

        // A freed slot is left alone; reused, it starts over at the world's key light.
        slots.apply(&deltas(&[], &[2]));
        slots.set_character(1, 5);
        assert_eq!((character(&slots, 1), flushed(&mut slots)), (3, vec![]));
        slots.apply(&deltas(&[instance(3, 0, 0.0)], &[]));
        assert_eq!((slot_of(&slots, 3), character(&slots, 1)), (Some(1), 0));
    }

    #[test]
    fn flush_coalesces_consecutive_slots() {
        let mut slots = InstanceSlots::default();
        let ids: Vec<_> = (0..8).map(|id| instance(id, 0, 0.0)).collect();
        slots.apply(&deltas(&ids, &[]));
        flushed(&mut slots);
        let changed: Vec<_> = [5, 1, 2, 3, 7].into_iter().map(|id| instance(id, 0, 1.0)).collect();
        slots.apply(&deltas(&changed, &[]));
        assert_eq!(flushed(&mut slots), [(1, 3), (5, 1), (7, 1)]);
    }

    #[test]
    fn slots_past_the_limit_are_never_written() {
        // Review RA-004: the instance buffer holds 5 slots.
        let mut slots = InstanceSlots::default();
        let flushed = |slots: &mut InstanceSlots| {
            let mut runs = Vec::new();
            slots.flush(5, |first, data| runs.push((first, data.len())));
            runs
        };
        let ids: Vec<_> = (0..8).map(|id| instance(id, 0, 0.0)).collect();
        slots.apply(&deltas(&ids, &[]));
        assert_eq!((slots.live_past(4), slots.live_past(5), slots.live_past(8)), (4, 3, 0));
        assert_eq!(flushed(&mut slots), [(0, 5)]);
        let moved: Vec<_> = [6, 2, 4, 5].into_iter().map(|id| instance(id, 0, 1.0)).collect();
        slots.apply(&deltas(&moved, &[]));
        assert_eq!(flushed(&mut slots), [(2, 1), (4, 1)], "and the moved ones past it settle unwritten");
        assert_eq!(flushed(&mut slots), [(2, 1), (4, 1)]);
        assert_eq!(flushed(&mut slots), []);
        // Freed slots are reused most recently freed first: one past the limit, whose new
        // instance doesn't draw, then one within it, whose new instance does.
        slots.apply(&deltas(&[], &[1, 6]));
        assert_eq!(slots.live_past(5), 2);
        slots.apply(&deltas(&[instance(9, 0, 0.0)], &[]));
        assert_eq!((slot_of(&slots, 9), slots.live_past(5)), (Some(6), 3), "the last freed goes first");
        slots.apply(&deltas(&[instance(10, 0, 0.0)], &[]));
        assert_eq!(slot_of(&slots, 10), Some(1));
        assert_eq!(flushed(&mut slots), [(1, 1)]);
    }

    #[test]
    fn draw_lists_take_whole_instances_within_their_limits() {
        let identity = Dequantize { position_scale: [1.0; 3], ..Dequantize::default() };
        let submesh = |first_index| SubmeshDraw { first_index, index_count: 3, material: material(0) };
        let mesh = |lod: Vec<SubmeshDraw>| MeshRecord {
            streams: MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 0 },
            dequantize: identity,
            lods: vec![lod],
            lod_vertices: vec![3],
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
            foliage: None,
        };
        // One draw, then two.
        let meshes = [mesh(vec![submesh(0)]), mesh(vec![submesh(0), submesh(3)])];
        let kind = |_| (MaterialFamily::Environment, MaterialFlags::empty());
        let mut slots = InstanceSlots::default();
        // Slots 0 to 3: one, two, one and two draws.
        let ids = [instance(0, 0, 0.0), instance(1, 1, 0.0), instance(2, 0, 0.0), instance(3, 1, 0.0)];
        slots.apply(&deltas(&ids, &[]));
        let mut list = DrawList::default();
        let drawn = |list: &DrawList| -> Vec<u32> { list.draws.iter().map(|draw| draw.instance).collect() };
        // Draw limits just above, at and just below what the instances need, and below what the
        // second needs: it is left out whole, and the third, which would fit, with it.
        for (draws, expected, over) in [
            (7, vec![0, 1, 1, 2, 3, 3], 0),
            (6, vec![0, 1, 1, 2, 3, 3], 0),
            (5, vec![0, 1, 1, 2], 2),
            (2, vec![0], 5),
        ] {
            list.build(&meshes, kind, |_| 0, &slots, DrawLimits { slots: u32::MAX, draws });
            assert_eq!((drawn(&list), list.over_limit), (expected, over), "room for {draws} draws");
        }
        // Slot limits: the instances in later slots aren't listed (their count is the scene's).
        for (limit, expected) in
            [(5, vec![0, 1, 1, 2, 3, 3]), (4, vec![0, 1, 1, 2, 3, 3]), (3, vec![0, 1, 1, 2])]
        {
            list.build(&meshes, kind, |_| 0, &slots, DrawLimits { slots: limit, draws: usize::MAX });
            assert_eq!((drawn(&list), list.over_limit), (expected, 0), "room for {limit} slots");
        }
    }

    #[test]
    fn draw_list_expands_live_slots_into_one_draw_per_lod0_submesh() {
        let quantized = Dequantize {
            position_offset: [-1.0; 3],
            position_scale: [2.0; 3],
            uv_transform: [0.0, 0.0, 1.0, 1.0],
        };
        let identity = Dequantize { position_offset: [0.0; 3], position_scale: [1.0; 3], ..quantized };
        let slab = |format, first_vertex| MeshStreams::Slab { format, first_vertex };
        // The draw list reads only the streams, dequantization and LODs.
        let record = || MeshRecord {
            streams: slab(PositionFormat::Unorm16, 0),
            dequantize: quantized,
            lods: Vec::new(),
            lod_vertices: Vec::new(),
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
            foliage: None,
        };
        let meshes = [
            MeshRecord {
                streams: slab(PositionFormat::Unorm16, 0),
                dequantize: quantized,
                lods: vec![
                    vec![SubmeshDraw { first_index: 0, index_count: 6, material: material(0) }],
                    vec![SubmeshDraw { first_index: 6, index_count: 3, material: material(0) }],
                ],
                lod_vertices: vec![24, 12],
                ..record()
            },
            MeshRecord {
                streams: slab(PositionFormat::Unorm16, 24),
                dequantize: quantized,
                lods: vec![vec![
                    SubmeshDraw { first_index: 9, index_count: 3, material: material(1) },
                    SubmeshDraw { first_index: 12, index_count: 12, material: material(2) },
                ]],
                lod_vertices: vec![8],
                ..record()
            },
            MeshRecord {
                streams: slab(PositionFormat::Float32, 0),
                dequantize: identity,
                lods: vec![vec![SubmeshDraw { first_index: 24, index_count: 3, material: material(3) }]],
                lod_vertices: vec![3],
                ..record()
            },
            MeshRecord {
                streams: MeshStreams::Deformable(DeformSource { surface: 90, ..DeformSource::default() }),
                dequantize: identity,
                lods: vec![vec![SubmeshDraw { first_index: 27, index_count: 6, material: material(4) }]],
                lod_vertices: vec![4],
                ..record()
            },
        ];
        let mut slots = InstanceSlots::default();
        let instances = [
            instance(1, 0, 0.0),
            instance(2, 1, 0.0),
            instance(3, 0, 0.0),
            instance(4, 2, 0.0),
            instance(5, 3, 0.0),
        ];
        slots.apply(&deltas(&instances, &[]));
        slots.apply(&deltas(&[], &[1]));
        // Handle 12 (slot 2) is alpha-tested, 13 transparent with an outline it won't draw, 14
        // has no outline, and 11 is a face; all are anime character materials.
        let flags = |handle: MaterialHandle| {
            let flags = match handle.0 {
                11 => MaterialFlags::FACE | MaterialFlags::OUTLINE,
                12 => MaterialFlags::ALPHA_TEST | MaterialFlags::OUTLINE,
                13 => MaterialFlags::TRANSPARENT | MaterialFlags::OUTLINE,
                14 => MaterialFlags::empty(),
                _ => MaterialFlags::OUTLINE,
            };
            (MaterialFamily::AnimeCharacter, flags)
        };
        let mut list = DrawList::default();
        list.build(&meshes, flags, |_| 0, &slots, DrawLimits::NONE);

        // Slot 0 is free; draws refer to instances by slot, carry their mesh's dequantization,
        // and draw LOD0.
        let draw = |instance, material, dequantize| DrawData { instance, material, dequantize };
        assert_eq!(
            list.draws,
            [
                draw(1, 1, quantized),
                draw(1, 2, quantized),
                draw(2, 0, quantized),
                draw(3, 3, identity),
                draw(4, 4, identity)
            ]
        );
        // Grouped by format, then by surface variant, in slot order within each group.
        let character = |coverage, part| SurfaceVariant::character(coverage, part).index();
        let (opaque, face) =
            (character(Coverage::Opaque, Part::Body), character(Coverage::Opaque, Part::Face));
        let alpha_test = character(Coverage::AlphaTest, Part::Body);
        let transparent = character(Coverage::Transparent, Part::Body);
        let [unorm16_variants, float32_variants] = &list.commands;
        let (unorm16, unorm16_face) = (&unorm16_variants[opaque], &unorm16_variants[face]);
        let (unorm16_alpha_test, unorm16_transparent) =
            (&unorm16_variants[alpha_test], &unorm16_variants[transparent]);
        let (float32, float32_transparent) = (&float32_variants[opaque], &float32_variants[transparent]);
        // Each names its instance's slot and its material (by material slot).
        let command = |indices, first_vertex, instance, draw_id, slot: u32, outline| DrawCommand {
            indices,
            first_vertex,
            slot: instance,
            draw_id,
            material: MaterialHandle(slot + 10),
            outline,
            foliage: 0,
        };
        assert_eq!(unorm16[..], [command(0..6, 0, 2, 2, 0, true)]);
        assert_eq!(unorm16_face[..], [command(9..12, 24, 1, 0, 1, true)], "a face of its own");
        assert_eq!(unorm16_alpha_test[..], [command(12..24, 24, 1, 1, 2, true)]);
        assert!(unorm16_transparent.is_empty());
        assert!(float32.is_empty());
        assert_eq!(float32_transparent[..], [command(24..27, 0, 3, 3, 3, false)], "transparent: no outline");
        let [deformed_opaque, ..] = &list.deformed;
        let material = MaterialHandle(14);
        assert_eq!(
            deformed_opaque[..],
            [DeformedDraw { indices: 27..33, slot: 4, surface: 90, draw_id: 4, material, outline: false }]
        );
        // Draws per variant: `[opaque, face, alpha-tested, transparent]` bodies and faces.
        let lens = |[o, f, a, t]: [usize; 4]| {
            let mut lens = [0; SurfaceVariant::ALL.len()];
            (lens[opaque], lens[face], lens[alpha_test], lens[transparent]) = (o, f, a, t);
            lens
        };
        let found = SurfaceVariant::ALL.map(|variant| list.len(variant));
        assert_eq!((found, list.outlines(), list.foliage_instances), (lens([2, 1, 1, 1]), 3, 0));

        // Rebuilding reuses the vectors instead of appending.
        slots.apply(&deltas(&[], &[3, 5]));
        list.build(&meshes, flags, |_| 0, &slots, DrawLimits::NONE);
        assert_eq!(list.draws.len(), 3);
        assert_eq!(SurfaceVariant::ALL.map(|variant| list.len(variant)), lens([0, 1, 1, 1]));
    }

    #[test]
    fn families_mix_in_a_mesh_and_draws_group_by_pipeline() {
        let identity = Dequantize { position_scale: [1.0; 3], ..Dequantize::default() };
        let submesh =
            |first_index, slot| SubmeshDraw { first_index, index_count: 3, material: material(slot) };
        // One mesh whose submeshes alternate between the families (a fog card among them), and a
        // second, skinned one.
        let meshes = [
            MeshRecord {
                streams: MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 8 },
                dequantize: identity,
                lods: vec![vec![
                    submesh(0, 0),
                    submesh(3, 1),
                    submesh(6, 2),
                    submesh(9, 3),
                    submesh(12, 4),
                    submesh(21, 5),
                ]],
                lod_vertices: vec![8],
                placement: Placement::default(),
                slots: Vec::new(),
                bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
                foliage: None,
            },
            MeshRecord {
                streams: MeshStreams::Deformable(DeformSource::default()),
                dequantize: identity,
                lods: vec![vec![submesh(15, 1), submesh(18, 0)]],
                lod_vertices: vec![4],
                placement: Placement::default(),
                slots: Vec::new(),
                bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
                foliage: None,
            },
        ];
        // Handles 10 and 12 are anime character materials (12 a face), 11, 13 and 14 environment
        // ones: opaque, alpha-tested, and transparent asking for an outline it never draws; 15 is
        // a fog card.
        let kind = |handle: MaterialHandle| match handle.0 {
            10 => (MaterialFamily::AnimeCharacter, MaterialFlags::OUTLINE),
            12 => (MaterialFamily::AnimeCharacter, MaterialFlags::FACE | MaterialFlags::OUTLINE),
            11 => (MaterialFamily::Environment, MaterialFlags::empty()),
            13 => (MaterialFamily::Environment, MaterialFlags::ALPHA_TEST),
            15 => (MaterialFamily::FogCard, MaterialFlags::TRANSPARENT),
            _ => (MaterialFamily::Environment, MaterialFlags::TRANSPARENT | MaterialFlags::OUTLINE),
        };
        let mut slots = InstanceSlots::default();
        slots.apply(&deltas(&[instance(1, 0, 0.0), instance(2, 1, 0.0), instance(3, 0, 1.0)], &[]));
        let mut list = DrawList::default();
        list.build(&meshes, kind, |_| 0, &slots, DrawLimits::NONE);

        // Each variant is a pipeline: its draws gather from every instance and submesh, in slot
        // order, whatever order the submeshes alternate in.
        let static_draws = |variant: SurfaceVariant| -> Vec<(u32, MaterialHandle, bool)> {
            list.commands[format_index(PositionFormat::Unorm16)][variant.index()]
                .iter()
                .map(|command| (command.indices.start, command.material, command.outline))
                .collect()
        };
        let (character, environment) = (MaterialHandle(10), MaterialHandle(11));
        assert_eq!(
            static_draws(SurfaceVariant::character(Coverage::Opaque, Part::Body)),
            [(0, character, true), (0, character, true)]
        );
        assert_eq!(
            static_draws(SurfaceVariant::character(Coverage::Opaque, Part::Face)),
            [(6, MaterialHandle(12), true), (6, MaterialHandle(12), true)]
        );
        assert_eq!(
            static_draws(SurfaceVariant::environment(Coverage::Opaque)),
            [(3, environment, false), (3, environment, false)],
            "the environment draws no hull"
        );
        assert_eq!(
            static_draws(SurfaceVariant::environment(Coverage::AlphaTest)),
            [(9, MaterialHandle(13), false), (9, MaterialHandle(13), false)]
        );
        assert_eq!(
            static_draws(SurfaceVariant::environment(Coverage::Transparent)),
            [(12, MaterialHandle(14), false), (12, MaterialHandle(14), false)],
            "transparent and environment: no outline either way"
        );
        assert_eq!(
            static_draws(SurfaceVariant::FOG_CARD),
            [(21, MaterialHandle(15), false), (21, MaterialHandle(15), false)],
            "fog cards: transparent, without a hull"
        );
        // The skinned instance's two families land in their variants' deformed groups.
        let deformed = |variant: SurfaceVariant| -> Vec<u32> {
            list.deformed[variant.index()].iter().map(|draw| draw.indices.start).collect()
        };
        assert_eq!(deformed(SurfaceVariant::environment(Coverage::Opaque)), [15]);
        assert_eq!(deformed(SurfaceVariant::OPAQUE), [18]);
        assert_eq!(list.draws.len(), 14, "every submesh of every instance");
        assert_eq!(list.outlines(), 5, "the anime character draws' hulls alone");
        let instances: Vec<u32> = list.draws.iter().map(|draw| draw.instance).collect();
        assert_eq!(instances, [0, 0, 0, 0, 0, 0, 1, 1, 2, 2, 2, 2, 2, 2], "records follow the slots");
    }

    #[test]
    fn foliage_draws_carry_their_stream_and_count_their_instances() {
        use crate::pipelines::Coverage;
        let identity = Dequantize { position_scale: [1.0; 3], ..Dequantize::default() };
        let submesh =
            |first_index, slot| SubmeshDraw { first_index, index_count: 3, material: material(slot) };
        let mesh = |first_vertex, foliage, lod: Vec<SubmeshDraw>| MeshRecord {
            streams: MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex },
            dequantize: identity,
            lods: vec![lod],
            lod_vertices: vec![4],
            placement: Placement::default(),
            slots: Vec::new(),
            bounds: Sphere { center: Vec3::ZERO, radius: 1.0 },
            foliage,
        };
        // A tree (opaque wood and alpha-tested leaves), a patch of grass, and a rock.
        let meshes = [
            mesh(0, Some(0), vec![submesh(0, 0), submesh(3, 1)]),
            mesh(4, Some(4), vec![submesh(6, 1)]),
            mesh(8, None, vec![submesh(9, 2)]),
        ];
        // Handle 10 is opaque foliage, 11 alpha-tested foliage, 12 an environment material.
        let kind = |handle: MaterialHandle| match handle.0 {
            10 => (MaterialFamily::Foliage, MaterialFlags::empty()),
            11 => (MaterialFamily::Foliage, MaterialFlags::ALPHA_TEST),
            _ => (MaterialFamily::Environment, MaterialFlags::empty()),
        };
        let mut slots = InstanceSlots::default();
        let trees = [instance(1, 0, 0.0), instance(2, 0, 5.0)];
        slots.apply(&deltas(&[trees[0], trees[1], instance(3, 1, 0.0), instance(4, 2, 0.0)], &[]));
        let mut list = DrawList::default();
        list.build(&meshes, kind, |_| 0, &slots, DrawLimits::NONE);
        let draws = |variant: SurfaceVariant| -> Vec<(u32, u32, u32)> {
            list.commands[format_index(PositionFormat::Unorm16)][variant.index()]
                .iter()
                .map(|command| (command.slot, command.first_vertex, command.foliage))
                .collect()
        };
        assert_eq!(draws(SurfaceVariant::foliage(Coverage::Opaque)), [(0, 0, 0), (1, 0, 0)]);
        assert_eq!(draws(SurfaceVariant::foliage(Coverage::AlphaTest)), [(0, 0, 0), (1, 0, 0), (2, 4, 4)]);
        assert_eq!(draws(SurfaceVariant::environment(Coverage::Opaque)), [(3, 8, 0)]);
        assert_eq!(list.foliage_instances, 3, "two trees and the grass");
        assert_eq!(list.outlines(), 0, "foliage draws no hull");
        slots.apply(&deltas(&[], &[1]));
        list.build(&meshes, kind, |_| 0, &slots, DrawLimits::NONE);
        assert_eq!(list.foliage_instances, 2);
    }

    #[test]
    fn id_hasher_spreads_entity_like_ids() {
        use std::hash::BuildHasher;
        let build = BuildHasherDefault::<IdHasher>::default();
        // Consecutive indices at one generation must differ in the bits hashbrown buckets by
        // (low) and tags with (top 7).
        let hashes: Vec<u64> = (0..64u64).map(|index| build.hash_one(InstanceId(1 << 32 | index))).collect();
        let low: std::collections::BTreeSet<_> = hashes.iter().map(|h| h & 63).collect();
        let top: std::collections::BTreeSet<_> = hashes.iter().map(|h| h >> 57).collect();
        assert_eq!(low.len(), 64);
        assert!(top.len() > 32, "top bits are mixed: {} distinct", top.len());
    }
}
