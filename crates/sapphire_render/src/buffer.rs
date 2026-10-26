//! GPU buffer helpers: a buffer that grows geometrically, writes staged through the frame's
//! staging belt, and a range allocator for sharing one buffer between owners that come and go.

use std::ops::Range;

use sapphire_rhi::wgpu;

/// A buffer that is replaced by a larger one when it runs out of room. Whoever binds it must
/// rebuild bind groups when [`GrowableBuffer::reserve`] reports a replacement.
#[derive(Debug)]
pub(crate) struct GrowableBuffer {
    buffer: wgpu::Buffer,
    label: &'static str,
    usage: wgpu::BufferUsages,
    max_size: u64,
    /// Times the buffer was replaced by a larger one (rule R9 stats).
    growths: u32,
}

impl GrowableBuffer {
    /// `max_size` is the largest buffer the device allows for this use (buffer size, or
    /// binding size for storage buffers).
    pub(crate) fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        capacity: u64,
        max_size: u64,
    ) -> Self {
        // COPY_DST for queue writes, COPY_SRC to carry contents over when growing.
        let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let buffer = create(device, label, usage, capacity.min(max_size));
        Self { buffer, label, usage, max_size, growths: 0 }
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    pub(crate) fn max_size(&self) -> u64 {
        self.max_size
    }

    pub(crate) fn growths(&self) -> u32 {
        self.growths
    }

    /// Ensures room for `needed` bytes (at most [`Self::max_size`]), copying the first `keep`
    /// bytes into the new buffer if it has to grow. Returns `true` when the buffer was replaced.
    pub(crate) fn reserve(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        needed: u64,
        keep: u64,
    ) -> bool {
        debug_assert!(
            needed <= self.max_size,
            "{} needs {needed} bytes, limit {}",
            self.label,
            self.max_size
        );
        if needed <= self.buffer.size() {
            return false;
        }
        let capacity = needed.max(self.buffer.size() * 2).next_power_of_two().min(self.max_size);
        let grown = create(device, self.label, self.usage, capacity);
        if keep > 0 {
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(self.label) });
            encoder.copy_buffer_to_buffer(&self.buffer, 0, &grown, 0, keep);
            queue.submit([encoder.finish()]);
        }
        tracing::debug!(target: crate::LOG_TARGET, label = self.label, capacity, "grew GPU buffer");
        self.buffer = grown;
        self.growths += 1;
        true
    }
}

/// Copies `bytes` to `target` at `offset` through the staging belt, as part of `encoder`.
/// Empty writes are skipped. `offset` and the length must be multiples of 4.
pub(crate) fn stage(
    belt: &mut wgpu::util::StagingBelt,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::Buffer,
    offset: u64,
    bytes: &[u8],
) {
    if let Some(size) = wgpu::BufferSize::new(bytes.len() as u64) {
        belt.write_buffer(encoder, target, offset, size).copy_from_slice(bytes);
    }
}

/// Hands out ranges of a buffer, in whatever unit its owner counts in, first fit, and takes them
/// back, merging free neighbors. The buffer itself is the owner's: [`Self::end`] says how much of
/// it is in use.
#[derive(Debug, Default)]
pub(crate) struct RangeAllocator {
    /// Free ranges below `end`, sorted, never adjacent to each other or to `end`.
    free: Vec<Range<u32>>,
    end: u32,
}

impl RangeAllocator {
    /// The start of a new range of `len` units: the first free range that fits, or the end. An
    /// empty range takes no room: it starts at 0.
    pub(crate) fn allocate(&mut self, len: u32) -> u32 {
        self.try_allocate(len).expect("allocations fit u32 units")
    }

    /// [`Self::allocate`], or `None` if the range would end past `u32::MAX` units.
    pub(crate) fn try_allocate(&mut self, len: u32) -> Option<u32> {
        if len == 0 {
            return Some(0);
        }
        if let Some(index) = self.free.iter().position(|range| range.len() as u32 >= len) {
            let range = &mut self.free[index];
            let start = range.start;
            range.start += len;
            if range.start == range.end {
                self.free.remove(index);
            }
            return Some(start);
        }
        let start = self.end;
        self.end = start.checked_add(len)?;
        Some(start)
    }

    /// Returns `start..start + len`, which [`Self::allocate`] handed out.
    pub(crate) fn free(&mut self, start: u32, len: u32) {
        if len == 0 {
            return;
        }
        let end = start + len;
        debug_assert!(end <= self.end, "{start}..{end} was never allocated");
        let index = self.free.partition_point(|range| range.start < start);
        debug_assert!(
            self.free.get(index).is_none_or(|next| end <= next.start)
                && index.checked_sub(1).is_none_or(|before| self.free[before].end <= start),
            "{start}..{end} is already free"
        );
        let mut range = start..end;
        if let Some(next) = self.free.get(index).filter(|next| next.start == end) {
            range.end = next.end;
            self.free.remove(index);
        }
        if let Some(before) = index.checked_sub(1).filter(|&before| self.free[before].end == start) {
            range.start = self.free[before].start;
            self.free.remove(before);
        }
        let index = self.free.partition_point(|free| free.start < range.start);
        if range.end == self.end {
            // Free space at the end returns to the end, so a later allocation can grow there.
            self.end = range.start;
        } else {
            self.free.insert(index, range);
        }
    }

    /// One past the last unit in use: what the buffer must hold.
    pub(crate) fn end(&self) -> u32 {
        self.end
    }
}

fn create(device: &wgpu::Device, label: &str, usage: wgpu::BufferUsages, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        // Bindings can't be empty, and copies need 4-byte multiples.
        size: capacity.max(16).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
        usage,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_reused_first_fit_and_merge_when_freed() {
        let mut ranges = RangeAllocator::default();
        let [a, b, c, d] = [10, 20, 30, 5].map(|len| ranges.allocate(len));
        assert_eq!(([a, b, c, d], ranges.end()), ([0, 10, 30, 60], 65));

        ranges.free(b, 20);
        assert_eq!(ranges.allocate(25), 65, "too big for the hole");
        assert_eq!(ranges.allocate(15), 10, "first fit");
        assert_eq!(ranges.allocate(5), 25, "the rest of the hole");
        assert_eq!(ranges.free, []);

        // Freeing neighbors merges them into one range.
        ranges.free(a, 10);
        ranges.free(25, 5);
        ranges.free(10, 15);
        assert_eq!((ranges.free.len(), ranges.free.first()), (1, Some(&(0..30))));
        assert_eq!(ranges.allocate(30), 0);
    }

    #[test]
    fn freeing_the_last_ranges_shrinks_the_end() {
        let mut ranges = RangeAllocator::default();
        let [a, b, c] = [4, 4, 4].map(|len| ranges.allocate(len));
        ranges.free(b, 4);
        ranges.free(c, 4);
        assert_eq!((ranges.end(), ranges.free.len()), (4, 0), "b merged into c, and both into the end");
        ranges.free(a, 4);
        assert_eq!(ranges.end(), 0);
        assert_eq!(ranges.allocate(7), 0);
    }

    #[test]
    fn empty_ranges_take_no_room() {
        let mut ranges = RangeAllocator::default();
        let a = ranges.allocate(4);
        assert_eq!((ranges.allocate(0), ranges.end()), (0, 4));
        ranges.free(0, 0);
        ranges.free(a, 4);
        assert_eq!((ranges.end(), ranges.free.len()), (0, 0));
    }
}
