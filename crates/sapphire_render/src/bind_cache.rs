//! Bind groups of texture views for post's and temporal AA's passes, rebuilt only when a
//! view changes: the pool hands a pass a different texture only after a resize or a settings
//! change, so steady frames create none. Each cache keeps the last two sets of views, since
//! temporal AA's history alternates between two textures, frame by frame, as post's input.

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
}

impl<const N: usize> Default for CachedBindGroup<N> {
    fn default() -> Self {
        Self { entries: Default::default(), last: 0 }
    }
}

impl<const N: usize> CachedBindGroup<N> {
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
