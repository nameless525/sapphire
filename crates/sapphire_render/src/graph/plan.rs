//! Compiling a frame graph: the GPU-free half, so it is unit-tested without a device.

use std::ops::Range;

use sapphire_rhi::TransientDesc;

use super::{GraphError, TextureId};

/// What the graph declares, without the views and record functions.
#[derive(Debug, Default)]
pub(super) struct Shape {
    pub textures: Vec<TextureNode>,
    pub passes: Vec<PassNode>,
    /// Every pass's accesses, one contiguous range per pass.
    pub accesses: Vec<Access>,
}

#[derive(Debug)]
pub(super) struct TextureNode {
    pub name: &'static str,
    pub size: (u32, u32),
    /// `None` for imported textures.
    pub transient: Option<TransientDesc>,
}

#[derive(Debug)]
pub(super) struct PassNode {
    pub name: &'static str,
    pub accesses: Range<usize>,
    pub side_effect: bool,
    /// What recording it costs against a pass that draws nothing ([`super::PassBuilder::weight`]).
    pub weight: u32,
}

/// How many of the next passes, of `weights`, go into the next of `runs` runs in order: about a
/// fair share of the `left` weight still to place, at least one pass. A pass goes in while at
/// least half of it fits, so a heavy pass doesn't start a run early for a sliver of the share.
pub(super) fn run_length(weights: impl Iterator<Item = u32>, left: u64, runs: usize) -> usize {
    let runs = runs.max(1) as u128;
    let mut taken = 0_u64;
    let mut length = 0;
    for weight in weights {
        let weight = u64::from(weight);
        if length > 0 && (2 * u128::from(taken) + u128::from(weight)) * runs > 2 * u128::from(left) {
            break;
        }
        taken += weight;
        length += 1;
    }
    length
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Access {
    pub texture: TextureId,
    pub write: bool,
}

impl Shape {
    pub fn with_capacity(textures: usize, passes: usize, accesses: usize) -> Self {
        Self {
            textures: Vec::with_capacity(textures),
            passes: Vec::with_capacity(passes),
            accesses: Vec::with_capacity(accesses),
        }
    }

    pub fn add_texture(
        &mut self,
        name: &'static str,
        size: (u32, u32),
        transient: Option<TransientDesc>,
    ) -> TextureId {
        let id = TextureId(self.textures.len() as u32);
        self.textures.push(TextureNode { name, size, transient });
        id
    }

    pub fn add_pass(&mut self, name: &'static str) -> usize {
        let start = self.accesses.len();
        self.passes.push(PassNode { name, accesses: start..start, side_effect: false, weight: 1 });
        self.passes.len() - 1
    }

    /// Adds an access to `pass`, which must be the last pass added.
    pub fn add_access(&mut self, pass: usize, texture: TextureId, write: bool) {
        assert!(texture.index() < self.textures.len(), "{texture:?} is not from this graph");
        debug_assert_eq!(pass, self.passes.len() - 1, "accesses are added to the newest pass");
        self.accesses.push(Access { texture, write });
        self.passes[pass].accesses.end = self.accesses.len();
    }

    pub fn pass_accesses(&self, pass: usize) -> &[Access] {
        &self.accesses[self.passes[pass].accesses.clone()]
    }
}

/// A compiled graph: which passes run, and which pooled texture each transient uses.
///
/// Kept by the executor and recompiled every frame, reusing its memory.
#[derive(Debug, Default)]
pub(super) struct Plan {
    /// The passes to run, in order.
    pub passes: Vec<usize>,
    /// Per texture: the slot of a transient that a running pass uses.
    pub slots: Vec<Option<usize>>,
    /// Per slot: the pooled texture to acquire for it.
    pub slot_descs: Vec<TransientDesc>,
    // Scratch.
    flags: Vec<bool>,
    last_use: Vec<usize>,
    free: Vec<bool>,
}

impl Plan {
    pub fn compile(&mut self, shape: &Shape) -> Result<(), GraphError> {
        self.cull(shape);
        self.check_reads(shape)?;
        self.assign_slots(shape);
        Ok(())
    }

    /// Walks the passes backwards, tracking which textures' current contents are still
    /// needed: imported ones outlive the frame, and a running pass needs what it reads. A pass
    /// runs if it has a side effect or writes a needed texture. Its writes replace those
    /// contents, so earlier writers are needed only for what it reads.
    fn cull(&mut self, shape: &Shape) {
        let needed = &mut self.flags;
        needed.clear();
        needed.extend(shape.textures.iter().map(|texture| texture.transient.is_none()));
        self.passes.clear();
        for (index, pass) in shape.passes.iter().enumerate().rev() {
            let accesses = shape.pass_accesses(index);
            if !pass.side_effect
                && !accesses.iter().any(|access| access.write && needed[access.texture.index()])
            {
                continue;
            }
            self.passes.push(index);
            for access in accesses.iter().filter(|access| access.write) {
                needed[access.texture.index()] = false;
            }
            for access in accesses.iter().filter(|access| !access.write) {
                needed[access.texture.index()] = true;
            }
        }
        self.passes.reverse();
    }

    /// Every read must follow a write in an earlier pass; imported textures arrive written.
    fn check_reads(&mut self, shape: &Shape) -> Result<(), GraphError> {
        let written = &mut self.flags;
        written.clear();
        written.extend(shape.textures.iter().map(|texture| texture.transient.is_none()));
        for &index in &self.passes {
            let accesses = shape.pass_accesses(index);
            if let Some(read) =
                accesses.iter().find(|access| !access.write && !written[access.texture.index()])
            {
                return Err(GraphError::ReadBeforeWrite {
                    pass: shape.passes[index].name,
                    texture: shape.textures[read.texture.index()].name,
                });
            }
            for access in accesses.iter().filter(|access| access.write) {
                written[access.texture.index()] = true;
            }
        }
        Ok(())
    }

    /// A transient lives from the first running pass that uses it to the last. Walking the
    /// passes in order, each takes a free slot with its descriptor when its life starts, or a
    /// new one, and frees it after its last pass, so transients that never overlap share.
    fn assign_slots(&mut self, shape: &Shape) {
        self.last_use.clear();
        self.last_use.resize(shape.textures.len(), 0);
        for (order, &index) in self.passes.iter().enumerate() {
            for access in shape.pass_accesses(index) {
                self.last_use[access.texture.index()] = order;
            }
        }
        self.slots.clear();
        self.slots.resize(shape.textures.len(), None);
        self.slot_descs.clear();
        self.free.clear();
        for (order, &index) in self.passes.iter().enumerate() {
            let accesses = shape.pass_accesses(index);
            for access in accesses {
                let texture = access.texture.index();
                let Some(desc) = shape.textures[texture].transient else { continue };
                if self.slots[texture].is_some() {
                    continue;
                }
                let free =
                    (0..self.slot_descs.len()).find(|&slot| self.free[slot] && self.slot_descs[slot] == desc);
                let slot = free.unwrap_or_else(|| {
                    self.slot_descs.push(desc);
                    self.free.push(false);
                    self.slot_descs.len() - 1
                });
                self.free[slot] = false;
                self.slots[texture] = Some(slot);
            }
            for access in accesses {
                let texture = access.texture.index();
                if let Some(slot) = self.slots[texture]
                    && self.last_use[texture] == order
                {
                    self.free[slot] = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sapphire_rhi::wgpu;

    /// `weights` split into `runs` runs: each run's weights.
    fn split(weights: &[u32], runs: usize) -> Vec<Vec<u32>> {
        let mut left: u64 = weights.iter().map(|&weight| u64::from(weight)).sum();
        let mut rest = weights;
        let mut out = Vec::new();
        for run in 0..runs {
            let length = run_length(rest.iter().copied(), left, runs - run);
            let (chunk, tail) = rest.split_at(length);
            left -= chunk.iter().map(|&weight| u64::from(weight)).sum::<u64>();
            rest = tail;
            if !chunk.is_empty() {
                out.push(chunk.to_vec());
            }
        }
        assert!(rest.is_empty(), "every pass is in a run: {rest:?} left");
        out
    }

    #[test]
    fn runs_share_the_weight_in_order_and_hold_every_pass() {
        assert_eq!(split(&[1, 1, 1, 1], 2), [vec![1, 1], vec![1, 1]], "equal passes split evenly");
        assert_eq!(split(&[1, 1, 1], 2), [vec![1, 1], vec![1]]);
        assert_eq!(
            split(&[1, 1, 1, 8, 1, 1], 2),
            [vec![1, 1, 1], vec![8, 1, 1]],
            "a heavy pass goes with less"
        );
        assert_eq!(split(&[8, 8, 1, 1], 2), [vec![8], vec![8, 1, 1]], "a heavy pass fills a run");
        assert_eq!(split(&[8, 8, 8], 8), [vec![8], vec![8], vec![8]], "fewer passes than runs");
        assert_eq!(split(&[1, 2, 3], 1), [vec![1, 2, 3]], "one run takes everything");
        assert_eq!(split(&[], 3), Vec::<Vec<u32>>::new());
        assert_eq!(split(&[1 << 31; 4], 2), [vec![1 << 31; 2], vec![1 << 31; 2]]);
        assert_eq!(split(&[u32::MAX; 6], 3), vec![vec![u32::MAX; 2]; 3]);
    }

    /// A fight's frame on Desktop-High, in its passes' weights: six encoders, the heaviest with no
    /// more than a little over its share.
    #[test]
    fn a_fights_frame_splits_into_about_equal_shares() {
        let frame = [4, 1, 1, 1, 4, 4, 4, 4, 2, 1, 2, 8, 1, 8, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        let runs = split(&frame, 6);
        let total: u32 = frame.iter().sum();
        assert_eq!(runs.len(), 6, "{runs:?}");
        let heaviest = runs.iter().map(|run| run.iter().sum::<u32>()).max().expect("runs");
        assert!(heaviest * 6 <= total * 3 / 2, "{heaviest} of {total}: {runs:?}");
    }

    use super::*;

    fn desc(width: u32) -> TransientDesc {
        TransientDesc {
            width,
            height: 8,
            format: wgpu::TextureFormat::Rgba8Unorm,
            sample_count: 1,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        }
    }

    #[derive(Default)]
    struct Graph {
        shape: Shape,
        plan: Plan,
    }

    impl Graph {
        fn import(&mut self, name: &'static str) -> TextureId {
            self.shape.add_texture(name, (8, 8), None)
        }

        fn transient(&mut self, name: &'static str, width: u32) -> TextureId {
            self.shape.add_texture(name, (width, 8), Some(desc(width)))
        }

        fn pass(&mut self, name: &'static str, reads: &[TextureId], writes: &[TextureId]) -> usize {
            let pass = self.shape.add_pass(name);
            for &texture in reads {
                self.shape.add_access(pass, texture, false);
            }
            for &texture in writes {
                self.shape.add_access(pass, texture, true);
            }
            pass
        }

        fn compile(&mut self) -> Result<Vec<&'static str>, GraphError> {
            self.plan.compile(&self.shape)?;
            Ok(self.plan.passes.iter().map(|&pass| self.shape.passes[pass].name).collect())
        }

        fn slot(&self, texture: TextureId) -> Option<usize> {
            self.plan.slots[texture.index()]
        }
    }

    #[test]
    fn passes_run_for_imported_writes_reads_and_side_effects_only() {
        let mut graph = Graph::default();
        let target = graph.import("target");
        let (used, unused, debug) =
            (graph.transient("used", 8), graph.transient("unused", 8), graph.transient("debug", 8));
        graph.pass("write used", &[], &[used]);
        graph.pass("write unused", &[], &[unused]);
        graph.pass("feeds debug only", &[used], &[debug]);
        graph.pass("composite", &[used], &[target]);
        let readback = graph.pass("readback", &[target], &[]);
        graph.shape.passes[readback].side_effect = true;
        assert_eq!(graph.compile(), Ok(vec!["write used", "composite", "readback"]));
        assert_eq!(graph.slot(unused), None, "only culled passes use it");
        assert_eq!(graph.slot(debug), None);
    }

    #[test]
    fn an_overwrite_culls_earlier_writers_unless_the_overwriter_reads() {
        let mut graph = Graph::default();
        let target = graph.import("target");
        graph.pass("first", &[], &[target]);
        graph.pass("second", &[], &[target]);
        assert_eq!(graph.compile(), Ok(vec!["second"]));

        let mut graph = Graph::default();
        let target = graph.import("target");
        graph.pass("first", &[], &[target]);
        graph.pass("blend over", &[target], &[target]);
        assert_eq!(graph.compile(), Ok(vec!["first", "blend over"]));
    }

    #[test]
    fn reading_a_transient_before_it_is_written_is_an_error() {
        let mut graph = Graph::default();
        let target = graph.import("target");
        let history = graph.transient("history", 8);
        graph.pass("resolve", &[history], &[target]);
        graph.pass("store history", &[target], &[history]);
        assert_eq!(graph.compile(), Err(GraphError::ReadBeforeWrite { pass: "resolve", texture: "history" }));

        // Reading and writing in one pass is no exception: the read sees the old contents.
        let mut graph = Graph::default();
        let target = graph.import("target");
        let accumulate = graph.transient("accumulate", 8);
        graph.pass("accumulate", &[accumulate], &[accumulate, target]);
        assert!(graph.compile().is_err());
    }

    #[test]
    fn transients_share_slots_only_with_disjoint_lives_and_equal_descriptors() {
        let mut graph = Graph::default();
        let target = graph.import("target");
        let a = graph.transient("a", 8);
        let b = graph.transient("b", 8);
        let c = graph.transient("c", 8);
        let wide = graph.transient("wide", 16);
        let d = graph.transient("d", 8);
        graph.pass("a", &[], &[a]);
        graph.pass("a to b", &[a], &[b]);
        graph.pass("b to c", &[b], &[c]);
        graph.pass("c to wide", &[c], &[wide]);
        graph.pass("wide to d", &[wide], &[d]);
        graph.pass("d to target", &[d], &[target]);
        assert_eq!(graph.compile().map(|passes| passes.len()), Ok(6));
        // `a` ends at "a to b", where `b` starts, so they overlap; `c` starts after `a` ends.
        assert_ne!(graph.slot(a), graph.slot(b));
        assert_eq!(graph.slot(a), graph.slot(c));
        assert_eq!(graph.slot(d), graph.slot(a), "the first free slot is taken");
        assert_eq!(graph.slot(target), None, "imported textures aren't pooled");
        assert_eq!(graph.plan.slot_descs, [desc(8), desc(8), desc(16)]);
    }

    #[test]
    fn recompiling_reuses_the_plan() {
        let mut graph = Graph::default();
        let target = graph.import("target");
        let a = graph.transient("a", 8);
        graph.pass("a", &[], &[a]);
        graph.pass("a to target", &[a], &[target]);
        let first = graph.compile();
        assert_eq!(graph.compile(), first);
        assert_eq!(graph.plan.slot_descs.len(), 1);
    }
}
