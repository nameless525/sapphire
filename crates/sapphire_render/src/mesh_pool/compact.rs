//! Giving the mesh pool's room back (M5 D9): see the parent's docs, "Giving room back".
//!
//! A part compacts whole within one frame's `prepare`, before anything reads a record: its live
//! ranges are packed in the order they lay, its buffers replaced by new ones at its floor, the ranges
//! copied across on the GPU, and every record that had a range there rewritten. So nothing a frame
//! draws can read a moved range: the frame being prepared builds its draw list, its skinning jobs
//! and everything after them from the rewritten records, and the frames already submitted read the
//! old buffers, which wait for the fence. Indices are mesh-relative, so moving a mesh's vertices
//! rewrites no index, and each part moves on its own.

use sapphire_rhi::wgpu;

use super::{
    DEFORMABLE, DeformSource, FOLIAGE, FOLIAGE_BYTES, INDEX_BYTES, INDICES, MeshPool, MeshRecord,
    MeshStreams, PARTS, PoolMoves, SHADING_BYTES, SURFACE_BYTES,
};
use crate::buffer::{GrowableBuffer, RangeAllocator};

/// The most bytes of new buffers one frame's compaction makes and copies into before it stops for the
/// frame: a trip's heavy work goes a bounded part a frame, as a load screen's frames are held to twice
/// the tier's interval. A part is never split, so a frame compacts one part at least.
const BYTES_PER_FRAME: u64 = 64 << 20;

impl MeshPool {
    /// Asks for the pool to give back what it holds past its floor, from the next [`Self::compact`] on,
    /// until it's there. Ask only under a wipe: a frame that compacts copies and allocates.
    pub(crate) fn ask_compaction(&mut self) {
        self.compacting = true;
    }

    /// A compaction was asked for, and the pool wasn't at its floor at the last [`Self::compact`].
    pub(crate) fn compacting(&self) -> bool {
        self.compacting
    }

    /// Whether every part's buffers are at their floor for what its current meshes hold.
    pub(crate) fn at_floor(&self) -> bool {
        (0..PARTS).all(|part| self.part_at_floor(part))
    }

    /// Bytes the pool's buffers would hold at their floor.
    pub(crate) fn floor_bytes(&self) -> u64 {
        (0..PARTS)
            .map(|part| {
                let live = self.live[part];
                self.buffers(part).iter().map(|(buffer, stride)| buffer.floor(live * stride)).sum::<u64>()
            })
            .sum()
    }

    /// Bytes of the buffers a compaction replaced that a frame may still be reading.
    pub(crate) fn retired_bytes(&self) -> u64 {
        self.retired_buffers.iter().map(wgpu::Buffer::size).sum()
    }

    /// The frame's part of a compaction asked for: compacts the parts above their floor, in order, until
    /// the frame has made [`BYTES_PER_FRAME`], and stops being asked once the pool is at its floor.
    /// `last_read` is the last frame that may read the buffers it replaces: the one before the frame
    /// being prepared. Call it in `prepare`, after what's freed has retired and before anything reads a
    /// record. Returns what it moved, if it moved anything: the draw list rebuilds, and bind groups of the
    /// deformable buffer.
    pub(crate) fn compact(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        last_read: u64,
    ) -> Option<PoolMoves> {
        if !self.compacting {
            return None;
        }
        let mut moves: Option<PoolMoves> = None;
        let mut encoder = None;
        let mut made = 0;
        for part in 0..PARTS {
            if made >= BYTES_PER_FRAME {
                break;
            }
            if self.part_at_floor(part) {
                continue;
            }
            let encoder = encoder.get_or_insert_with(|| {
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("mesh pool compaction"),
                })
            });
            made += self.compact_part(device, encoder, part, last_read);
            moves.get_or_insert_default().deformable |= part == DEFORMABLE;
        }
        if let Some(encoder) = encoder {
            // Uploads staged and queue writes made since the last submission land first, so the copies
            // take them.
            self.uploads.flush(queue);
            queue.submit([encoder.finish()]);
            tracing::debug!(
                target: crate::LOG_TARGET,
                made,
                capacity = self.capacity_bytes(),
                floor = self.floor_bytes(),
                "compacted the mesh pool"
            );
        }
        self.compacting = !self.at_floor();
        moves
    }

    /// Parts compacted since the pool was created.
    pub(crate) fn compactions(&self) -> u32 {
        self.layouts.iter().sum()
    }

    fn part_at_floor(&self, part: usize) -> bool {
        let live = self.live[part];
        self.buffers(part)
            .iter()
            .all(|(buffer, stride)| buffer.buffer().size() <= buffer.floor(live * stride))
    }

    /// Packs `part`'s live ranges into new buffers at its floor, copying through `encoder`, and retires
    /// the old buffers as read by frames up to `last_read`. Returns the new buffers' bytes.
    fn compact_part(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        part: usize,
        last_read: u64,
    ) -> u64 {
        let mut live: Vec<(usize, u32, u32)> = (self.records.iter().enumerate())
            .filter_map(|(index, record)| record.range(part).map(|(start, len)| (index, start, len)))
            .collect();
        live.sort_unstable_by_key(|&(_, start, _)| start);
        // Neighbours in the old layout stay neighbours, so each run of them is one copy: (from, to, len).
        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        let mut end = 0;
        for &(index, start, len) in &live {
            match runs.last_mut() {
                Some(run) if run.0 + run.2 == start => run.2 += len,
                _ => runs.push((start, end, len)),
            }
            self.records[index].move_to(part, end);
            end += len;
        }
        debug_assert_eq!(u64::from(end), self.live[part], "the live count is what the records hold");
        let mut made = 0;
        let mut old = Vec::with_capacity(3);
        for (buffer, stride) in self.buffers_mut(part) {
            let replaced = buffer.replace_at_floor(device, u64::from(end) * stride);
            for &(from, to, len) in &runs {
                let (from, to, len) =
                    (u64::from(from) * stride, u64::from(to) * stride, u64::from(len) * stride);
                encoder.copy_buffer_to_buffer(&replaced, from, buffer.buffer(), to, len);
            }
            made += buffer.buffer().size();
            old.push(replaced);
        }
        for buffer in old {
            self.retired_buffers.push(last_read, buffer);
        }
        let packed = RangeAllocator::packed(end);
        match part {
            DEFORMABLE => (self.deformable_words, self.deformable_used) = (packed, end),
            INDICES => (self.index_ranges, self.indices_used) = (packed, end),
            FOLIAGE => (self.foliage_ranges, self.foliage_used) = (packed, end),
            slab => {
                let slab = &mut self.slabs[slab];
                (slab.vertices, slab.used) = (packed, end);
            }
        }
        self.layouts[part] += 1;
        made
    }

    /// `part`'s buffers, each with its bytes per unit of the part's ranges.
    fn buffers(&self, part: usize) -> Vec<(&GrowableBuffer, u64)> {
        match part {
            DEFORMABLE => vec![(&self.deformable, 4)],
            INDICES => vec![(&self.indices, INDEX_BYTES)],
            FOLIAGE => vec![(&self.foliage, FOLIAGE_BYTES)],
            slab => {
                let slab = &self.slabs[slab];
                let stride = slab.position_stride();
                vec![
                    (&slab.positions, stride),
                    (&slab.shading, SHADING_BYTES),
                    (&slab.surface, SURFACE_BYTES),
                ]
            }
        }
    }

    fn buffers_mut(&mut self, part: usize) -> Vec<(&mut GrowableBuffer, u64)> {
        match part {
            DEFORMABLE => vec![(&mut self.deformable, 4)],
            INDICES => vec![(&mut self.indices, INDEX_BYTES)],
            FOLIAGE => vec![(&mut self.foliage, FOLIAGE_BYTES)],
            slab => {
                let slab = &mut self.slabs[slab];
                let stride = slab.position_stride();
                vec![
                    (&mut slab.positions, stride),
                    (&mut slab.shading, SHADING_BYTES),
                    (&mut slab.surface, SURFACE_BYTES),
                ]
            }
        }
    }
}

impl MeshRecord {
    /// Moves its range of `part` to start at `start`, rewriting everything that says where it is.
    fn move_to(&mut self, part: usize, start: u32) {
        match part {
            INDICES => {
                let from = self.placement.indices.0;
                for submesh in self.lods.iter_mut().flatten() {
                    submesh.first_index = submesh.first_index - from + start;
                }
                self.placement.indices.0 = start;
            }
            FOLIAGE => {
                self.foliage = Some(start);
                self.placement.foliage.0 = start;
            }
            _ => {
                let from = self.placement.vertices.0;
                match &mut self.streams {
                    MeshStreams::Slab { first_vertex, .. } => *first_vertex = start,
                    MeshStreams::Deformable(source) => source.rebase(from, start),
                }
                self.placement.vertices.0 = start;
            }
        }
    }
}

impl DeformSource {
    /// Moves word offsets into a region placed at word `from` to the region placed at `to`.
    fn rebase(&mut self, from: u32, to: u32) {
        let shift = |word: &mut u32| *word = *word - from + to;
        shift(&mut self.positions);
        shift(&mut self.shading);
        shift(&mut self.surface);
        if let Some(skin) = &mut self.skin {
            shift(&mut skin.influences);
        }
        if let Some(morphs) = &mut self.morphs {
            shift(&mut morphs.offsets);
            shift(&mut morphs.deltas);
        }
    }
}

#[cfg(test)]
mod tests {
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::bounds::Sphere;
    use crate::gpu_types::Dequantize;
    use crate::materials::MaterialHandle;
    use crate::mesh_pool::{MorphSource, Placement, SkinSource, SubmeshDraw, SubmeshMaterial};

    fn record(streams: MeshStreams, placement: Placement) -> MeshRecord {
        let material = SubmeshMaterial { handle: MaterialHandle::from_raw(0), slot: 0 };
        let first = placement.indices.0;
        MeshRecord {
            streams,
            dequantize: Dequantize::default(),
            lods: vec![
                vec![
                    SubmeshDraw { first_index: first, index_count: 3, material },
                    SubmeshDraw { first_index: first + 3, index_count: 3, material },
                ],
                vec![SubmeshDraw { first_index: first + 6, index_count: 3, material }],
            ],
            lod_vertices: vec![4, 3],
            placement,
            slots: Vec::new(),
            bounds: Sphere::around([[0.0; 3], [1.0; 3]]),
            palette_reach: Vec::new(),
            morph_reach: Vec::new(),
            foliage: (placement.foliage.1 > 0).then_some(placement.foliage.0),
        }
    }

    #[test]
    fn a_moved_record_says_where_it_is_everywhere_it_does() {
        let placement = Placement { vertices: (40, 4), indices: (90, 9), foliage: (40, 4) };
        let mut plant =
            record(MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 40 }, placement);
        for part in [0, INDICES, FOLIAGE] {
            plant.move_to(part, 7);
        }
        assert_eq!(plant.streams, MeshStreams::Slab { format: PositionFormat::Unorm16, first_vertex: 7 });
        assert_eq!(plant.placement, Placement { vertices: (7, 4), indices: (7, 9), foliage: (7, 4) });
        assert_eq!(plant.foliage, Some(7));
        let firsts: Vec<u32> = plant.lods.iter().flatten().map(|submesh| submesh.first_index).collect();
        assert_eq!(firsts, [7, 10, 13], "every LOD's submeshes, as they lay");
        assert_eq!(plant.range(1), None, "a unorm16 mesh has nothing in the f32 slab");
        assert_eq!(plant.range(DEFORMABLE), None);

        let source = DeformSource {
            vertex_count: 4,
            positions: 100,
            shading: 112,
            surface: 120,
            skin: Some(SkinSource { influences: 128, wide: false, palette_len: 2 }),
            morphs: Some(MorphSource { offsets: 136, deltas: 141, targets: 1 }),
            ..DeformSource::default()
        };
        let placement = Placement { vertices: (100, 45), indices: (0, 9), foliage: (0, 0) };
        let mut skinned = record(MeshStreams::Deformable(source), placement);
        skinned.move_to(DEFORMABLE, 20);
        let moved = *skinned.deform().expect("deformable");
        assert_eq!((moved.positions, moved.shading, moved.surface), (20, 32, 40));
        assert_eq!(moved.skin.map(|skin| skin.influences), Some(48));
        assert_eq!(moved.morphs.map(|morphs| (morphs.offsets, morphs.deltas)), Some((56, 61)));
        assert_eq!(skinned.placement.vertices, (20, 45));
        assert_eq!(skinned.range(DEFORMABLE), Some((20, 45)));
        assert_eq!(skinned.range(0), None);
    }
}
