//! Bind groups of texture views for post's and temporal AA's passes, rebuilt only when a
//! view changes: the pool hands a pass a different texture only after a resize or a settings
//! change, so steady frames create none. Each cache keeps the last two sets of views, since
//! temporal AA's history alternates between two textures, frame by frame, as post's input.
//!
//! A view keeps its texture alive, so a cache must not keep a size left behind, or the transient
//! pool's eviction would free nothing. A set that can come round again shares its sizes with the
//! one in use; any other goes at the miss that shows it.

use sapphire_rhi::wgpu;

/// Bind groups of up to `N` texture views at bindings 0, 1, …: the two most recently used, with
/// the views each was made from.
#[derive(Debug)]
pub(crate) struct CachedBindGroup<const N: usize> {
    entries: [Entry<N>; 2],
    /// The entry used last; the other is replaced on a miss.
    last: usize,
}

#[derive(Debug)]
struct Entry<const N: usize> {
    views: [Option<wgpu::TextureView>; N],
    bind_group: Option<wgpu::BindGroup>,
}

impl<const N: usize> Default for Entry<N> {
    fn default() -> Self {
        Self { views: std::array::from_fn(|_| None), bind_group: None }
    }
}

impl<const N: usize> Entry<N> {
    fn holds(&self, views: &[&wgpu::TextureView]) -> bool {
        self.bind_group.is_some()
            && self.views.iter().zip(views).all(|(cached, view)| cached.as_ref() == Some(*view))
    }

    /// Whether each view it holds is of the same size as the one `views` has in its place: any
    /// of them can be the one that changes, as scene color does under a target-sized input.
    fn same_sizes(&self, views: &[&wgpu::TextureView]) -> bool {
        self.views.iter().zip(views).all(|(cached, view)| {
            cached.as_ref().is_none_or(|cached| cached.texture().size() == view.texture().size())
        })
    }
}

impl<const N: usize> Default for CachedBindGroup<N> {
    fn default() -> Self {
        Self { entries: Default::default(), last: 0 }
    }
}

impl<const N: usize> CachedBindGroup<N> {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.iter().all(|entry| entry.bind_group.is_none())
    }

    /// A bind group of `views` at bindings 0, 1, …, reused while they stay the same. `views`
    /// holds at least one and at most `N` views, in the order `layout` binds them; each set
    /// keeps its own layout, so a cache serves one pass position.
    pub(crate) fn get(
        &mut self,
        device: &wgpu::Device,
        label: Option<&str>,
        layout: &wgpu::BindGroupLayout,
        views: &[&wgpu::TextureView],
    ) -> &wgpu::BindGroup {
        debug_assert!((1..=N).contains(&views.len()), "{} views for a cache of {N}", views.len());
        let hit =
            (0..2).map(|offset| (self.last + offset) % 2).find(|&index| self.entries[index].holds(views));
        let index = hit.unwrap_or_else(|| {
            let kept = &mut self.entries[self.last];
            if !kept.same_sizes(views) {
                *kept = Entry::default();
            }
            let index = 1 - self.last;
            let entries: [wgpu::BindGroupEntry<'_>; N] =
                std::array::from_fn(|binding| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: wgpu::BindingResource::TextureView(views[binding.min(views.len() - 1)]),
                });
            let entry = &mut self.entries[index];
            entry.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label,
                layout,
                entries: &entries[..views.len()],
            }));
            for (cached, view) in entry.views.iter_mut().zip(views) {
                *cached = Some((*view).clone());
            }
            index
        });
        self.last = index;
        self.entries[index].bind_group.as_ref().expect("created above")
    }
}

#[cfg(test)]
mod tests {
    use sapphire_rhi::{GpuContext, GpuContextDesc, GpuInstance, GpuInstanceDesc};

    use super::*;

    fn view(gpu: &GpuContext, side: u32) -> wgpu::TextureView {
        gpu.device()
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// The views `cache`'s entries hold, in no particular order.
    fn held<const N: usize>(cache: &CachedBindGroup<N>) -> Vec<wgpu::TextureView> {
        cache.entries.iter().flat_map(|entry| entry.views.iter().flatten().cloned()).collect()
    }

    /// Review RC-3: the entry a miss keeps holds strong views. It is worth them for the other half
    /// of a history pair, which comes round again; not for a size left behind, which the pool's
    /// eviction would then not release.
    #[test]
    #[ignore = "needs a GPU"]
    fn a_new_size_lets_the_old_views_go_and_alternation_keeps_its_groups() {
        let instance = GpuInstance::new(&GpuInstanceDesc::default()).expect("instance");
        let gpu = GpuContext::new(&instance, &GpuContextDesc::default(), None).expect("device");
        let device = gpu.device();
        let binding = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[binding(0), binding(1)],
        });
        let mut cache = CachedBindGroup::<2>::default();
        let get = |cache: &mut CachedBindGroup<2>, views: [&wgpu::TextureView; 2]| {
            cache.get(device, None, &layout, &views).clone()
        };

        // A history pair at one size alternates in the first slot, each with a group of its own
        // for as long as it lasts: nothing is made again.
        let (small_a, small_b, small_mask) = (view(&gpu, 8), view(&gpu, 8), view(&gpu, 8));
        let first = get(&mut cache, [&small_a, &small_mask]);
        let second = get(&mut cache, [&small_b, &small_mask]);
        assert_ne!(first, second);
        for _ in 0..4 {
            assert_eq!(get(&mut cache, [&small_a, &small_mask]), first);
            assert_eq!(get(&mut cache, [&small_b, &small_mask]), second);
        }

        // The size holds at a new one, with one set of views (no alternation to replace the entry
        // kept): the last size's are gone at the first miss.
        let (large_a, large_mask) = (view(&gpu, 16), view(&gpu, 16));
        let large = get(&mut cache, [&large_a, &large_mask]);
        assert_eq!(held(&cache), [large_a.clone(), large_mask.clone()]);
        assert_eq!(get(&mut cache, [&large_a, &large_mask]), large, "and the new group is reused");

        // Any view of another size counts: upscaling steps scene color, the composite's mask, under
        // an input that keeps its size.
        let stepped = view(&gpu, 12);
        get(&mut cache, [&large_a, &stepped]);
        assert_eq!(held(&cache), [large_a.clone(), stepped.clone()]);

        // A new pair at that size keeps the entry it may alternate with.
        let large_b = view(&gpu, 16);
        get(&mut cache, [&large_b, &stepped]);
        assert_eq!(held(&cache).len(), 4, "both halves stay");
    }
}
