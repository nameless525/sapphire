//! Bind groups of texture views for post's and temporal AA's passes, rebuilt only when a
//! view changes: the pool hands a pass a different texture only after a resize or a settings
//! change, so steady frames create none.

use sapphire_rhi::wgpu;

/// A bind group of up to `N` texture views at bindings 0, 1, …, and the views it was made from.
#[derive(Debug)]
pub(crate) struct CachedBindGroup<const N: usize> {
    views: [Option<wgpu::TextureView>; N],
    bind_group: Option<wgpu::BindGroup>,
}

impl<const N: usize> Default for CachedBindGroup<N> {
    fn default() -> Self {
        Self { views: std::array::from_fn(|_| None), bind_group: None }
    }
}

impl<const N: usize> CachedBindGroup<N> {
    /// A bind group of `views` at bindings 0, 1, …, reused while they stay the same. `views`
    /// holds at least one and at most `N` views, in the order `layout` binds them.
    pub(crate) fn get(
        &mut self,
        device: &wgpu::Device,
        label: Option<&str>,
        layout: &wgpu::BindGroupLayout,
        views: &[&wgpu::TextureView],
    ) -> &wgpu::BindGroup {
        debug_assert!((1..=N).contains(&views.len()), "{} views for a cache of {N}", views.len());
        let current = self.views.iter().zip(views).all(|(cached, view)| cached.as_ref() == Some(*view));
        if !current || self.bind_group.is_none() {
            let entries: [wgpu::BindGroupEntry<'_>; N] =
                std::array::from_fn(|binding| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: wgpu::BindingResource::TextureView(views[binding.min(views.len() - 1)]),
                });
            self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label,
                layout,
                entries: &entries[..views.len()],
            }));
            for (cached, view) in self.views.iter_mut().zip(views) {
                *cached = Some((*view).clone());
            }
        }
        self.bind_group.as_ref().expect("created above")
    }
}
