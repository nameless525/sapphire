//! The persistent GPU scene (spec §5.12 "Scene and draws"): each mesh instance keeps one slot
//! of the instance buffer from the packet that adds it until the packet that removes it.
//!
//! - [`InstanceSlots`] maps stable [`InstanceId`]s to slots, reusing freed slots through a free
//!   list, and keeps a CPU mirror of the instance buffer. Applying a packet's deltas rewrites
//!   only the slots they touch and marks those for upload.
//! - [`DrawList`] turns the live slots into draw records and draw commands. Draws name their
//!   instance by slot, so a transform change leaves them as they are: the list is rebuilt only
//!   when instances are added or removed, change mesh, or meshes are uploaded.
//! - [`GpuScene`] owns both, plus the instance and draw-record buffers. `prepare` applies a
//!   packet and sizes the buffers; the frame's uploads pass then writes the dirty slots, in
//!   contiguous runs, and the draw records after a rebuild, through the staging belt.
//!
//! The commands are for the CPU-driven submission path, which is portable everywhere: each
//! draw binds the mesh's vertices and its draw record by buffer offset. The GPU-driven path
//! (later) writes equivalent multi-draw-indirect records on the GPU, using `base_vertex` and
//! `first_instance` instead (see the crate docs).
//!
//! Draws are grouped by the meshes' position format, whose pipelines and stream slab differ,
//! and come in slot order within a group. Each draw record carries its mesh's dequantization,
//! so the vertex shaders decode any mesh without another binding. Sorting opaque draws by
//! pipeline, material and mesh (§5.12) comes with a second opaque pipeline; because the list
//! is rebuilt only when the set changes, the sort will cost nothing on frames that only move
//! instances.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Range;

use bytemuck::Zeroable;
use glam::Mat4;
use sapphire_rhi::wgpu;

use crate::buffer::{self, GrowableBuffer};
use crate::gpu_types::{DrawData, InstanceData};
use crate::mesh_pool::MeshRecord;
use crate::pipelines::format_index;
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
        }
    }

    /// Applies packet `frame`'s instance deltas, rebuilds the draw list if the drawn set
    /// changed, and makes room in the buffers. Returns `true` when the instance buffer was
    /// replaced, so bind groups that reference it must be rebuilt.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: u64,
        deltas: &InstanceDeltas,
        meshes: &[MeshRecord],
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
        // The old buffer holds every slot uploaded so far: a GPU copy keeps them all without
        // sending them again.
        let uploaded = self.instances.buffer().size();
        let replaced =
            self.instances.reserve(device, queue, self.slots.len() as u64 * INSTANCE_BYTES, uploaded);
        if self.draws_stale {
            self.draw_list.build(meshes, &self.slots);
            // The records are uploaded whole after every rebuild, so nothing needs keeping.
            self.draws.reserve(device, queue, size_of_val(self.draw_list.draws.as_slice()) as u64, 0);
            self.draws_stale = false;
            self.draws_unsent = true;
            self.builds += 1;
        }
        replaced
    }

    /// Meshes were uploaded or moved: rebuild the draw list on the next `prepare`.
    pub(crate) fn meshes_changed(&mut self) {
        self.draws_stale = true;
    }

    pub(crate) fn instance_buffer(&self) -> &wgpu::Buffer {
        self.instances.buffer()
    }

    /// The instances by slot, as the last `prepare` left them.
    pub(crate) fn slots(&self) -> &InstanceSlots {
        &self.slots
    }

    /// Splits the scene between a frame's passes: the uploads pass writes what changed, and
    /// the draw passes read the draw list and its record buffer.
    pub(crate) fn split_for_record(&mut self) -> (SceneUpload<'_>, &DrawList, &GrowableBuffer) {
        let Self { slots, draw_list, instances, draws, draws_unsent, .. } = self;
        let upload = SceneUpload {
            slots,
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

    /// Draw commands per mesh pass.
    pub(crate) fn draw_count(&self) -> u32 {
        to_u32(self.draw_list.len())
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
}

/// The scene's share of a frame's uploads pass.
pub(crate) struct SceneUpload<'a> {
    slots: &'a mut InstanceSlots,
    instances: &'a wgpu::Buffer,
    draw_records: (&'a [DrawData], &'a wgpu::Buffer),
    draws_unsent: &'a mut bool,
}

impl SceneUpload<'_> {
    /// Writes the dirty slots and, after a rebuild, the draw records through `belt`.
    pub(crate) fn write(self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder) {
        let Self { slots, instances, draw_records: (records, record_buffer), draws_unsent } = self;
        slots.flush(|first, data| {
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
        for id in &deltas.removed {
            if let Some(slot) = self.ids.remove(id) {
                self.slots[slot as usize].id = None;
                self.free.push(slot);
                drawn_changed = true;
            }
        }
        for instance in &deltas.changed {
            let slot = match self.ids.get(&instance.id) {
                Some(&slot) => {
                    let mesh = &mut self.slots[slot as usize].mesh;
                    if *mesh != instance.mesh {
                        *mesh = instance.mesh;
                        drawn_changed = true;
                    }
                    slot
                }
                None => {
                    let slot = self.allocate(instance.id, instance.mesh);
                    self.ids.insert(instance.id, slot);
                    drawn_changed = true;
                    slot
                }
            };
            self.data[slot as usize] = InstanceData::new(instance.transform);
            let entry = &mut self.slots[slot as usize];
            if !entry.dirty {
                entry.dirty = true;
                self.dirty.push(slot);
            }
        }
        drawn_changed
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

    /// The slot of a live instance.
    pub(crate) fn slot(&self, id: InstanceId) -> Option<u32> {
        self.ids.get(&id).copied()
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

    /// Hands each run of consecutive dirty slots to `write` (first slot, data), then marks
    /// them clean.
    fn flush(&mut self, mut write: impl FnMut(u32, &[InstanceData])) {
        self.dirty.sort_unstable();
        for run in self.dirty.chunk_by(|&a, &b| b == a + 1) {
            let (first, last) = (run[0], run[run.len() - 1]);
            write(first, &self.data[first as usize..=last as usize]);
        }
        for &slot in &self.dirty {
            self.slots[slot as usize].dirty = false;
        }
        self.dirty.clear();
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
/// mesh-relative) and slot 0 at draw record `draw_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DrawCommand {
    pub indices: Range<u32>,
    pub first_vertex: u32,
    pub draw_id: u32,
}

/// The draws for every live instance: one per submesh of the mesh's LOD0. Its vectors are
/// reused across rebuilds.
#[derive(Debug, Default)]
pub(crate) struct DrawList {
    pub draws: Vec<DrawData>,
    /// Per [`PositionFormat::ALL`] entry, since each format has its own pipelines and slab.
    pub commands: [Vec<DrawCommand>; 2],
}

impl DrawList {
    pub(crate) fn build(&mut self, meshes: &[MeshRecord], slots: &InstanceSlots) {
        profiling::scope!("DrawList::build");
        self.draws.clear();
        for commands in &mut self.commands {
            commands.clear();
        }
        for (slot, mesh) in slots.live() {
            let record = meshes.get(mesh.index());
            debug_assert!(record.is_some(), "{mesh:?} was not uploaded to this renderer");
            let Some(record) = record else { continue };
            let commands = &mut self.commands[format_index(record.format)];
            for submesh in &record.lods[0] {
                let draw_id = to_u32(self.draws.len());
                self.draws.push(DrawData {
                    instance: slot,
                    material: submesh.material,
                    dequantize: record.dequantize,
                });
                commands.push(DrawCommand {
                    indices: submesh.first_index..submesh.first_index + submesh.index_count,
                    first_vertex: record.first_vertex,
                    draw_id,
                });
            }
        }
    }

    /// Draw commands per mesh pass.
    pub(crate) fn len(&self) -> usize {
        self.commands.iter().map(Vec::len).sum()
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("instance and draw counts fit the u32 GPU indices")
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::MeshInstance;
    use crate::gpu_types::Dequantize;
    use crate::mesh_pool::SubmeshDraw;

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
        slots.flush(|first, data| runs.push((first, data.len())));
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

        assert!(slots.apply(&deltas(&[instance(20, 3, 5.0)], &[])), "a mesh change rebuilds the draws");
        assert_eq!(slot_of(&slots, 20), Some(1));

        // Changed twice before an upload: listed once, with the latest data.
        slots.apply(&deltas(&[instance(10, 0, 7.0)], &[]));
        slots.apply(&deltas(&[instance(10, 0, 8.0)], &[]));
        assert_eq!(x(&slots, 0), 8.0);
        assert_eq!(flushed(&mut slots), [(0, 2)]);
    }

    #[test]
    fn removals_free_slots_for_reuse_and_ids_stay_stable() {
        let mut slots = InstanceSlots::default();
        let three = [instance(1, 0, 0.0), instance(2, 0, 0.0), instance(3, 0, 0.0)];
        slots.apply(&deltas(&three, &[]));
        flushed(&mut slots);

        assert!(slots.apply(&deltas(&[], &[1, 2])));
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
    fn removed_then_added_in_one_packet_survives_and_unknown_removals_are_ignored() {
        let mut slots = InstanceSlots::default();
        slots.apply(&deltas(&[instance(1, 0, 0.0)], &[]));
        assert!(!slots.apply(&deltas(&[], &[42, 42])), "unknown IDs change nothing");

        // Removals apply first, so the instance comes back (into the slot it just freed).
        assert!(slots.apply(&deltas(&[instance(1, 2, 3.0)], &[1])));
        assert_eq!(slot_of(&slots, 1), Some(0));
        assert_eq!(slots.live().collect::<Vec<_>>(), [(0, MeshHandle::from_raw(2))]);
        assert_eq!(x(&slots, 0), 3.0);
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
    fn draw_list_expands_live_slots_into_one_draw_per_lod0_submesh() {
        let quantized = Dequantize {
            position_offset: [-1.0; 3],
            position_scale: [2.0; 3],
            uv_transform: [0.0, 0.0, 1.0, 1.0],
        };
        let identity = Dequantize { position_offset: [0.0; 3], position_scale: [1.0; 3], ..quantized };
        let meshes = [
            MeshRecord {
                format: PositionFormat::Unorm16,
                first_vertex: 0,
                dequantize: quantized,
                lods: vec![
                    vec![SubmeshDraw { first_index: 0, index_count: 6, material: 0 }],
                    vec![SubmeshDraw { first_index: 6, index_count: 3, material: 0 }],
                ],
            },
            MeshRecord {
                format: PositionFormat::Unorm16,
                first_vertex: 24,
                dequantize: quantized,
                lods: vec![vec![
                    SubmeshDraw { first_index: 9, index_count: 3, material: 1 },
                    SubmeshDraw { first_index: 12, index_count: 12, material: 2 },
                ]],
            },
            MeshRecord {
                format: PositionFormat::Float32,
                first_vertex: 0,
                dequantize: identity,
                lods: vec![vec![SubmeshDraw { first_index: 24, index_count: 3, material: 3 }]],
            },
        ];
        let mut slots = InstanceSlots::default();
        let instances = [instance(1, 0, 0.0), instance(2, 1, 0.0), instance(3, 0, 0.0), instance(4, 2, 0.0)];
        slots.apply(&deltas(&instances, &[]));
        slots.apply(&deltas(&[], &[1]));
        let mut list = DrawList::default();
        list.build(&meshes, &slots);

        // Slot 0 is free; draws refer to instances by slot, carry their mesh's dequantization,
        // and draw LOD0.
        let draw = |instance, material, dequantize| DrawData { instance, material, dequantize };
        assert_eq!(
            list.draws,
            [draw(1, 1, quantized), draw(1, 2, quantized), draw(2, 0, quantized), draw(3, 3, identity)]
        );
        let [unorm16, float32] = &list.commands;
        assert_eq!(unorm16[1], DrawCommand { indices: 12..24, first_vertex: 24, draw_id: 1 });
        assert_eq!(unorm16[2], DrawCommand { indices: 0..6, first_vertex: 0, draw_id: 2 });
        assert_eq!(float32[..], [DrawCommand { indices: 24..27, first_vertex: 0, draw_id: 3 }]);
        assert_eq!(list.len(), 4);

        // Rebuilding reuses the vectors instead of appending.
        slots.apply(&deltas(&[], &[3]));
        list.build(&meshes, &slots);
        assert_eq!((list.draws.len(), list.len()), (3, 3));
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
