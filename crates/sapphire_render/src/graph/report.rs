//! [`GraphReport`]: the last executed frame graph, pass by pass, for inspection.

use sapphire_rhi::TransientDesc;

use super::plan::{Plan, Shape};

/// What the last executed frame graph looked like: every pass and texture, which passes ran and
/// in which command buffer, and which pooled texture each transient used. Filled while
/// [`GraphExecutor::set_report`](super::GraphExecutor::set_report) is on; its lists keep their
/// capacity, so a steady graph fills it without allocating.
#[derive(Clone, Debug, Default)]
pub struct GraphReport {
    /// Every pass, in the order it was added.
    pub passes: Vec<PassReport>,
    /// Every texture, in the order it was added.
    pub textures: Vec<TextureReport>,
    accesses: Vec<AccessReport>,
}

/// One pass of a [`GraphReport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassReport {
    pub name: &'static str,
    /// Its position among the passes that ran, or `None` if it was culled.
    pub order: Option<u32>,
    /// Declared [`side_effect`](super::PassBuilder::side_effect), so it runs regardless.
    pub side_effect: bool,
    /// The command buffer it was recorded into, in submission order.
    pub command_buffer: Option<u32>,
    accesses: (u32, u32),
}

/// One texture of a [`GraphReport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureReport {
    pub name: &'static str,
    pub size: (u32, u32),
    /// The descriptor of a transient; `None` for an imported texture.
    pub transient: Option<TransientDesc>,
    /// The pooled texture a transient used this frame; transients that share one never lived at
    /// the same time. `None` for imported textures and for transients only culled passes used.
    pub slot: Option<u32>,
    /// The first and last running pass (by [`PassReport::order`]) that used it.
    pub lifetime: Option<(u32, u32)>,
}

/// A pass's use of a texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccessReport {
    /// Index into [`GraphReport::textures`].
    pub texture: u32,
    /// A write replaces the contents; a read uses them.
    pub write: bool,
}

impl GraphReport {
    /// What `pass` reads and writes, as declared.
    pub fn accesses(&self, pass: &PassReport) -> &[AccessReport] {
        &self.accesses[pass.accesses.0 as usize..pass.accesses.1 as usize]
    }

    /// Passes that ran.
    pub fn running(&self) -> impl Iterator<Item = &PassReport> {
        self.passes.iter().filter(|pass| pass.order.is_some())
    }

    /// Rewrites the report from a compiled graph whose running passes were recorded into command
    /// buffers of `chunk_len` passes each, in order.
    pub(super) fn fill(&mut self, shape: &Shape, plan: &Plan, chunk_len: usize) {
        self.passes.clear();
        self.textures.clear();
        self.accesses.clear();
        self.textures.extend(shape.textures.iter().enumerate().map(|(index, texture)| TextureReport {
            name: texture.name,
            size: texture.size,
            transient: texture.transient,
            slot: plan.slots[index].map(|slot| slot as u32),
            lifetime: None,
        }));
        let mut order = plan.passes.iter().copied().enumerate().peekable();
        for (index, pass) in shape.passes.iter().enumerate() {
            let running = order.next_if(|&(_, running)| running == index).map(|(order, _)| order as u32);
            let start = self.accesses.len() as u32;
            for access in shape.pass_accesses(index) {
                let texture = access.texture.index();
                self.accesses.push(AccessReport { texture: texture as u32, write: access.write });
                if let Some(order) = running {
                    let lifetime = &mut self.textures[texture].lifetime;
                    *lifetime = Some(lifetime.map_or((order, order), |(first, _)| (first, order)));
                }
            }
            self.passes.push(PassReport {
                name: pass.name,
                order: running,
                side_effect: pass.side_effect,
                command_buffer: running.map(|order| order / chunk_len.max(1) as u32),
                accesses: (start, self.accesses.len() as u32),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use sapphire_rhi::wgpu;

    use super::*;

    fn desc() -> TransientDesc {
        TransientDesc {
            width: 8,
            height: 8,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sample_count: 1,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        }
    }

    #[test]
    fn reports_culled_passes_slots_lifetimes_and_command_buffers() {
        let mut shape = Shape::default();
        let target = shape.add_texture("target", (8, 8), None);
        let a = shape.add_texture("a", (8, 8), Some(desc()));
        let unused = shape.add_texture("unused", (8, 8), Some(desc()));
        let b = shape.add_texture("b", (8, 8), Some(desc()));
        let pass = |shape: &mut Shape, name, reads: &[_], writes: &[_]| {
            let pass = shape.add_pass(name);
            for &texture in reads {
                shape.add_access(pass, texture, false);
            }
            for &texture in writes {
                shape.add_access(pass, texture, true);
            }
        };
        pass(&mut shape, "write a", &[], &[a]);
        pass(&mut shape, "write unused", &[], &[unused]);
        pass(&mut shape, "a to b", &[a], &[b]);
        pass(&mut shape, "b to target", &[b], &[target]);
        let mut plan = Plan::default();
        plan.compile(&shape).expect("valid");

        let mut report = GraphReport::default();
        report.fill(&shape, &plan, 2);
        let summary: Vec<_> = report.passes.iter().map(|p| (p.name, p.order, p.command_buffer)).collect();
        assert_eq!(
            summary,
            [
                ("write a", Some(0), Some(0)),
                ("write unused", None, None),
                ("a to b", Some(1), Some(0)),
                ("b to target", Some(2), Some(1)),
            ]
        );
        assert_eq!(report.running().count(), 3);
        let accesses = report.accesses(&report.passes[2]);
        assert_eq!(
            accesses,
            [AccessReport { texture: 1, write: false }, AccessReport { texture: 3, write: true }]
        );

        let texture = |name: &str| *report.textures.iter().find(|t| t.name == name).expect("texture");
        assert_eq!(texture("target").slot, None, "imported");
        assert_eq!(texture("target").lifetime, Some((2, 2)));
        assert_eq!(texture("a").lifetime, Some((0, 1)));
        assert_eq!(texture("b").lifetime, Some((1, 2)));
        assert_ne!(texture("a").slot, texture("b").slot, "a and b overlap in `a to b`");
        assert_eq!((texture("unused").slot, texture("unused").lifetime), (None, None));

        // Refilling reuses the lists.
        let capacity = report.passes.capacity();
        report.fill(&shape, &plan, 8);
        assert_eq!(report.passes.capacity(), capacity);
        assert!(report.running().all(|pass| pass.command_buffer == Some(0)));
    }
}
