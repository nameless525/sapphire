//! Transient textures, pooled by descriptor (spec §5.11).
//!
//! wgpu can't alias GPU memory between resources, so the frame graph shares whole textures
//! instead: it packs transients whose lifetimes don't overlap into one request, and the pool
//! hands each frame's requests the textures with the same descriptors from earlier frames.
//! Textures unused for [`EVICT_AFTER_FRAMES`] frames are released, so sizes left behind by a
//! resize or a quality change don't stay resident. Until then they count in
//! [`TransientStats::bytes`]: each step of dynamic resolution leaves a set of scene-sized textures
//! behind for that long. The pool has no byte budget yet: one waits for phones' memory to be
//! measured (review RA-O06).

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

impl TransientDesc {
    /// About how many bytes a texture of this descriptor holds: its blocks (texels, for formats
    /// that aren't compressed) × a block's bytes × its samples. An estimate of the payload, not
    /// of GPU memory: backends add alignment, padding and metadata, may compress, and lay out
    /// depth formats without a copy size as they choose (24-bit depth counts 4 bytes a texel,
    /// with stencil too, and 32-bit depth with stencil 8).
    pub fn estimated_bytes(&self) -> u64 {
        let (block_width, block_height) = self.format.block_dimensions();
        let blocks =
            u64::from(self.width.div_ceil(block_width)) * u64::from(self.height.div_ceil(block_height));
        blocks * block_bytes(self.format) * u64::from(self.sample_count)
    }
}

/// The bytes of one of `format`'s blocks: its copy size where it has one.
fn block_bytes(format: wgpu::TextureFormat) -> u64 {
    use wgpu::TextureFormat as F;
    if let Some(bytes) = format.block_copy_size(None) {
        return u64::from(bytes);
    }
    match format {
        F::Depth24Plus | F::Depth24PlusStencil8 => 4,
        F::Depth32FloatStencil8 => 8,
        // Planar formats: their planes' texels together (more than a subsampled plane holds);
        // anything else, the largest block.
        _ => {
            let planes =
                [wgpu::TextureAspect::Plane0, wgpu::TextureAspect::Plane1, wgpu::TextureAspect::Plane2];
            let bytes: u32 = planes.into_iter().filter_map(|plane| format.block_copy_size(Some(plane))).sum();
            if bytes > 0 { u64::from(bytes) } else { 16 }
        }
    }
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
    /// The resident textures' estimated bytes ([`TransientDesc::estimated_bytes`]), the unused
    /// ones waiting to be released included.
    pub bytes: u64,
    /// The most [`Self::bytes`] has been since the pool was made.
    pub peak_bytes: u64,
}

/// Textures and their default views, reused by descriptor.
#[derive(Debug)]
pub struct TransientPool {
    slots: SlotPool<TransientDesc, (wgpu::Texture, wgpu::TextureView)>,
}

impl Default for TransientPool {
    fn default() -> Self {
        Self { slots: SlotPool::new(TransientDesc::estimated_bytes) }
    }
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
    /// A key's estimated bytes.
    weigh: fn(&K) -> u64,
    stats: TransientStats,
}

#[derive(Debug)]
struct Slot<K, T> {
    key: K,
    value: T,
    in_use: bool,
    last_used: u64,
}

impl<K, T> SlotPool<K, T> {
    fn new(weigh: fn(&K) -> u64) -> Self {
        Self { slots: Vec::new(), frame: 0, weigh, stats: TransientStats::default() }
    }
}

impl<K: PartialEq + Copy, T> SlotPool<K, T> {
    fn begin_frame(&mut self) {
        self.frame += 1;
        let (frame, weigh) = (self.frame, self.weigh);
        let (before, mut released) = (self.slots.len(), 0);
        self.slots.retain(|slot| {
            let kept = frame - slot.last_used <= EVICT_AFTER_FRAMES;
            if !kept {
                released += weigh(&slot.key);
            }
            kept
        });
        self.stats.evicted += (before - self.slots.len()) as u64;
        self.stats.bytes -= released;
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
        self.stats.bytes += (self.weigh)(key);
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.stats.bytes);
        self.slots.len() - 1
    }

    fn get(&self, index: usize) -> &T {
        &self.slots[index].value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool whose key `k` weighs `100 k` bytes.
    fn pool() -> SlotPool<u32, char> {
        SlotPool::new(|&key| u64::from(key) * 100)
    }

    #[test]
    fn slots_are_reused_across_frames_by_matching_keys_only() {
        let mut pool = pool();
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
        let mut pool = pool();
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
        assert_eq!((pool.stats.bytes, pool.stats.peak_bytes), (100, 300), "its bytes went with it");
    }

    #[test]
    fn bytes_follow_what_is_resident_and_keep_their_peak() {
        let mut pool = pool();
        pool.begin_frame();
        pool.acquire(&1, || 'a');
        pool.acquire(&1, || 'b');
        pool.acquire(&4, || 'c');
        assert_eq!((pool.stats.bytes, pool.stats.peak_bytes), (600, 600));
        // A frame of the same requests reuses them: nothing new.
        pool.begin_frame();
        pool.acquire(&4, || 'x');
        pool.acquire(&1, || 'y');
        assert_eq!((pool.stats.bytes, pool.stats.peak_bytes, pool.stats.created), (600, 600, 3));
        // A resize: the new size's texture joins the old ones until they are released.
        pool.begin_frame();
        pool.acquire(&2, || 'd');
        assert_eq!((pool.stats.bytes, pool.stats.peak_bytes), (800, 800));
        for _ in 0..EVICT_AFTER_FRAMES {
            pool.begin_frame();
            pool.acquire(&2, || 'z');
        }
        assert_eq!((pool.stats.textures, pool.stats.bytes, pool.stats.peak_bytes), (1, 200, 800));
    }

    #[test]
    fn estimated_bytes_count_blocks_and_samples() {
        use wgpu::TextureFormat as F;
        let desc = |width, height, format, sample_count| TransientDesc {
            width,
            height,
            format,
            sample_count,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        assert_eq!(desc(640, 360, F::Rgba16Float, 1).estimated_bytes(), 640 * 360 * 8);
        assert_eq!(desc(640, 360, F::Depth32Float, 4).estimated_bytes(), 640 * 360 * 4 * 4);
        assert_eq!(desc(3840, 2160, F::Rg11b10Ufloat, 1).estimated_bytes(), 3840 * 2160 * 4);
        assert_eq!(desc(641, 361, F::R8Unorm, 1).estimated_bytes(), 641 * 361);
        // Depth formats without a copy size, as backends commonly lay them out.
        assert_eq!(desc(100, 10, F::Depth24Plus, 1).estimated_bytes(), 4000);
        assert_eq!(desc(100, 10, F::Depth24PlusStencil8, 1).estimated_bytes(), 4000);
        assert_eq!(desc(100, 10, F::Depth32FloatStencil8, 2).estimated_bytes(), 16_000);
        // Compressed formats in whole 4 × 4 blocks; a planar one by its planes' texels.
        assert_eq!(desc(10, 10, F::Bc7RgbaUnorm, 1).estimated_bytes(), 9 * 16);
        assert_eq!(desc(10, 10, F::NV12, 1).estimated_bytes(), 100 * 3);
    }
}
