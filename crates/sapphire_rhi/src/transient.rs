//! Transient textures, pooled by descriptor (spec §5.11).
//!
//! wgpu can't alias GPU memory between resources, so the frame graph shares whole textures
//! instead: it packs transients whose lifetimes don't overlap into one request, and the pool
//! hands each frame's requests the textures with the same descriptors from earlier frames.
//! Textures unused for [`EVICT_AFTER_FRAMES`] frames are released, so sizes left behind by a
//! resize or a quality change don't stay resident.

use crate::GpuContext;

/// Frames a pooled texture may sit unused before it is released.
pub const EVICT_AFTER_FRAMES: u64 = 60;

/// What a transient texture is; textures are shared only between identical descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransientDesc {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub sample_count: u32,
    pub usage: wgpu::TextureUsages,
}

/// A texture handed out by [`TransientPool::acquire`] for the current frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TransientTexture(usize);

/// Pool counters (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransientStats {
    /// Textures resident in the pool.
    pub textures: usize,
    /// Textures ever created; it settles once sizes and settings stop changing.
    pub created: u64,
    /// Textures released after going unused.
    pub evicted: u64,
}

/// Textures and their default views, reused by descriptor.
#[derive(Debug, Default)]
pub struct TransientPool {
    slots: SlotPool<TransientDesc, (wgpu::Texture, wgpu::TextureView)>,
}

impl TransientPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a frame: every texture is free again, and long-unused ones are released.
    pub fn begin_frame(&mut self) {
        self.slots.begin_frame();
    }

    /// A texture matching `desc` for the rest of the frame: one not yet acquired this frame,
    /// or a new one.
    pub fn acquire(&mut self, gpu: &GpuContext, desc: &TransientDesc) -> TransientTexture {
        TransientTexture(self.slots.acquire(desc, || create(gpu, desc)))
    }

    pub fn texture(&self, texture: TransientTexture) -> &wgpu::Texture {
        &self.slots.get(texture.0).0
    }

    pub fn view(&self, texture: TransientTexture) -> &wgpu::TextureView {
        &self.slots.get(texture.0).1
    }

    pub fn stats(&self) -> TransientStats {
        self.slots.stats
    }
}

fn create(gpu: &GpuContext, desc: &TransientDesc) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: gpu.label("transient"),
        size: wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: desc.sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: desc.format,
        usage: desc.usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// The pool's bookkeeping, apart from the GPU objects it holds.
#[derive(Debug)]
struct SlotPool<K, T> {
    slots: Vec<Slot<K, T>>,
    frame: u64,
    stats: TransientStats,
}

#[derive(Debug)]
struct Slot<K, T> {
    key: K,
    value: T,
    in_use: bool,
    last_used: u64,
}

impl<K, T> Default for SlotPool<K, T> {
    fn default() -> Self {
        Self { slots: Vec::new(), frame: 0, stats: TransientStats::default() }
    }
}

impl<K: PartialEq + Copy, T> SlotPool<K, T> {
    fn begin_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        let before = self.slots.len();
        self.slots.retain(|slot| frame - slot.last_used <= EVICT_AFTER_FRAMES);
        self.stats.evicted += (before - self.slots.len()) as u64;
        for slot in &mut self.slots {
            slot.in_use = false;
        }
        self.stats.textures = self.slots.len();
    }

    fn acquire(&mut self, key: &K, create: impl FnOnce() -> T) -> usize {
        let frame = self.frame;
        if let Some((index, slot)) =
            self.slots.iter_mut().enumerate().find(|(_, slot)| !slot.in_use && slot.key == *key)
        {
            slot.in_use = true;
            slot.last_used = frame;
            return index;
        }
        self.slots.push(Slot { key: *key, value: create(), in_use: true, last_used: frame });
        self.stats.created += 1;
        self.stats.textures = self.slots.len();
        self.slots.len() - 1
    }

    fn get(&self, index: usize) -> &T {
        &self.slots[index].value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_reused_across_frames_by_matching_keys_only() {
        let mut pool: SlotPool<u32, char> = SlotPool::default();
        pool.begin_frame();
        let a = pool.acquire(&1, || 'a');
        let b = pool.acquire(&1, || 'b');
        assert_ne!(a, b, "both in use at once");
        pool.begin_frame();
        assert_eq!(pool.acquire(&2, || 'c'), 2, "a different key never shares");
        assert_eq!(pool.acquire(&1, || 'd'), a, "a free slot is reused");
        assert_eq!(*pool.get(a), 'a');
        assert_eq!(pool.stats.created, 3);
    }

    #[test]
    fn frames_free_everything_and_evict_idle_slots() {
        let mut pool: SlotPool<u32, char> = SlotPool::default();
        pool.begin_frame();
        let a = pool.acquire(&1, || 'a');
        pool.acquire(&2, || 'b');
        pool.begin_frame();
        assert_eq!(pool.acquire(&1, || 'x'), a, "free again in the next frame");
        for _ in 0..EVICT_AFTER_FRAMES {
            pool.begin_frame();
            pool.acquire(&1, || 'y');
        }
        assert_eq!(pool.stats.textures, 1, "key 2 went unused and was evicted");
        assert_eq!((pool.stats.created, pool.stats.evicted), (2, 1));
    }
}
