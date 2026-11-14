//! What a frame submits, pass by pass (M5 B4): the draw calls, the instances and triangles they draw,
//! the mesh draws each pass looked at and what left the rest out, and the levels of detail drawn.
//! [`RenderStats::passes`](crate::RenderStats::passes) publishes them (rule R9), for the bench's rows,
//! the debug overlay and a playtest's report.
//!
//! - **Counted where the calls are made,** as each pass records: a mesh draw's group of draws
//!   (`MeshDraws`, the `draw` module) keeps a plain [`PassCounts`] on its stack and adds it to its pass's
//!   counters once, at the group's end; a pass of a few calls (the sky, the effects' sprites and
//!   effect meshes, post, the interface) adds what it drew once. Passes record on several threads at once, so each pass's
//!   counters are relaxed atomics only it adds to; a frame's adds are a few dozen. Nothing allocates,
//!   hashes or reads a clock, so the counters are on in every build, shipping ones too.
//! - **A frame's, read after its graph ran.** [`Renderer::record`](crate::Renderer::record) zeroes them,
//!   and the passes add as the graph executes; a pass the graph culled, or a frame that never ran, leaves
//!   its counts at zero. Read between a frame's record and its execution, they're zero.
//! - **"Culled" says by what:** a mesh draw a pass looked at and didn't draw counts against the first of
//!   its tests that left it out ([`PassCounts::culled_by_frustum`], [`PassCounts::culled_by_pass`],
//!   [`PassCounts::culled_by_cutoff`]). Draws of another view (an inset's, the world's) and of another
//!   set (an outline pass's surfaces without a hull) aren't looked at.
//! - **Levels of detail:** every mesh draw counts in [`PassCounts::levels`] by the level it drew. Every
//!   draw is LOD0 until levels are selected (m5-plan D4, D5); the buckets are fixed so that work fills
//!   them as they are.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::inset::MAX_INSETS;
use crate::shadows::MAX_CASCADES;

/// The levels of detail [`PassCounts::levels`] tells apart: LOD0 and the cook's three coarser levels
/// (spec §5.27). A deeper level counts in the last.
pub const MESH_LEVELS: usize = 4;

/// What one pass submitted in a frame, or the frame's sum ([`FrameCounts::total`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassCounts {
    /// Draw calls submitted: mesh draws, outline hulls, the sky, the effects' quads, fullscreen
    /// triangles and the interface's batches.
    pub draws: u32,
    /// Instances those calls drew: one a mesh draw, each sprite or batch of quads its own.
    pub instances: u32,
    /// Triangles those calls drew, every instance's: a mesh draw's index range over three, a hull's the
    /// same as its surface's.
    pub triangles: u64,
    /// Of [`Self::draws`], outline hulls.
    pub outline_draws: u32,
    /// Mesh draws the pass looked at: those of its set (surfaces, hulls, eyes through hair) standing in
    /// its view, drawn or culled.
    pub considered: u32,
    /// Of those, the ones the main view's frustum left out (the `view_cull` module): the main pass, the
    /// scene prepass and the debug views' replays.
    pub culled_by_frustum: u32,
    /// Of those, the ones the pass's own pick left out: a cascade's caster test, a character map's
    /// character, the character prepass's listed characters. The casters a receiver reads (M5 L12's
    /// planner) are chosen in shading and leave no draw out, so they don't count here; a masked caster
    /// (an alpha-tested character variant) is a mesh draw of its pass like any other.
    pub culled_by_pass: u32,
    /// Of those, outline hulls past their material's distance cutoff
    /// ([`RenderStats::outlines_culled`](crate::RenderStats::outlines_culled) counts them by instance).
    pub culled_by_cutoff: u32,
    /// Mesh draws by the level of detail they drew, LOD0 first. They sum to the pass's mesh draws.
    pub levels: [u32; MESH_LEVELS],
}

impl PassCounts {
    /// One draw call of `instances` instances of `triangles` triangles each, that isn't a scene mesh's
    /// (an effect mesh is one of these: it has no level).
    pub(crate) fn call(instances: u32, triangles: u64) -> Self {
        Self { draws: 1, instances, triangles: u64::from(instances) * triangles, ..Self::default() }
    }

    /// One fullscreen triangle.
    pub(crate) fn fullscreen() -> Self {
        Self::call(1, 1)
    }

    /// A mesh draw of one instance of `indices` indices at `level`, an outline hull or not.
    pub(crate) fn mesh_draw(&mut self, indices: u32, level: u8, outline: bool) {
        self.draws += 1;
        self.instances += 1;
        self.triangles += u64::from(indices / 3);
        self.outline_draws += u32::from(outline);
        self.levels[usize::from(level).min(MESH_LEVELS - 1)] += 1;
    }

    /// Mesh draws counted in [`Self::levels`].
    pub fn mesh_draws(&self) -> u32 {
        self.levels.iter().sum()
    }

    /// Mesh draws looked at and left out, by any test.
    pub fn culled(&self) -> u32 {
        self.culled_by_frustum + self.culled_by_pass + self.culled_by_cutoff
    }

    /// `self` and `other` together.
    #[must_use]
    pub fn plus(&self, other: &Self) -> Self {
        let mut levels = self.levels;
        for (level, other) in levels.iter_mut().zip(other.levels) {
            *level += other;
        }
        Self {
            draws: self.draws + other.draws,
            instances: self.instances + other.instances,
            triangles: self.triangles + other.triangles,
            outline_draws: self.outline_draws + other.outline_draws,
            considered: self.considered + other.considered,
            culled_by_frustum: self.culled_by_frustum + other.culled_by_frustum,
            culled_by_pass: self.culled_by_pass + other.culled_by_pass,
            culled_by_cutoff: self.culled_by_cutoff + other.culled_by_cutoff,
            levels,
        }
    }
}

/// What each pass of a frame submitted (see the [module docs](self)). Passes that draw nothing, the
/// uploads, skinning, light clusters, the sky cube's face, the ambient pass and the face maps' compute
/// pass, have no entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameCounts {
    /// The character prepass: the listed characters' surfaces.
    pub character_prepass: PassCounts,
    /// The scene prepass, when a later pass reads it: surfaces and outline hulls.
    pub scene_prepass: PassCounts,
    /// The main pass: surfaces, outline hulls, eyes through hair, eye layers, fog cards, the sky and the
    /// effects (sprites and effect meshes) where no temporal AA follows, as on the phone tiers.
    pub main: PassCounts,
    /// Each shadow cascade drawn this frame, by its index.
    pub cascades: [PassCounts; MAX_CASCADES],
    /// The character shadow maps, all of the atlas's tiles.
    pub character_maps: PassCounts,
    /// Each inset drawn this frame, its character prepass and main pass together, by its index.
    pub insets: [PassCounts; MAX_INSETS],
    /// Temporal AA's passes and the effects drawn after its resolve (sprites and effect meshes).
    pub temporal: PassCounts,
    /// Post's passes: the bloom chain, the fused composite, and the colour-vision view's pass.
    pub post: PassCounts,
    /// The on-screen interface.
    pub ui: PassCounts,
    /// The debug views' replays and passes, and the debug lines.
    pub debug: PassCounts,
}

impl FrameCounts {
    /// Every pass, with the name a bench's row gives it.
    pub fn passes(&self) -> [(&'static str, &PassCounts); 8 + MAX_CASCADES + MAX_INSETS] {
        let [c0, c1, c2, c3] = &self.cascades;
        let [i0, i1] = &self.insets;
        [
            ("character prepass", &self.character_prepass),
            ("scene prepass", &self.scene_prepass),
            ("main", &self.main),
            ("cascade 0", c0),
            ("cascade 1", c1),
            ("cascade 2", c2),
            ("cascade 3", c3),
            ("character maps", &self.character_maps),
            ("inset 0", i0),
            ("inset 1", i1),
            ("temporal", &self.temporal),
            ("post", &self.post),
            ("ui", &self.ui),
            ("debug", &self.debug),
        ]
    }

    /// The whole frame: every pass's counts summed.
    pub fn total(&self) -> PassCounts {
        self.passes().iter().fold(PassCounts::default(), |total, (_, pass)| total.plus(pass))
    }

    /// The shadow passes': every cascade's and the character maps'.
    pub fn shadows(&self) -> PassCounts {
        self.cascades.iter().fold(self.character_maps, |total, pass| total.plus(pass))
    }
}

/// A pass [`PassCounters`] keeps counts for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Counted {
    CharacterPrepass,
    ScenePrepass,
    Main,
    /// A shadow cascade, below [`MAX_CASCADES`].
    Cascade(usize),
    CharacterMaps,
    /// An inset, below [`MAX_INSETS`].
    Inset(usize),
    Temporal,
    Post,
    Ui,
    Debug,
}

impl Counted {
    const SLOTS: usize = 8 + MAX_CASCADES + MAX_INSETS;

    fn slot(self) -> usize {
        match self {
            Self::CharacterPrepass => 0,
            Self::ScenePrepass => 1,
            Self::Main => 2,
            Self::Cascade(index) => 3 + index.min(MAX_CASCADES - 1),
            Self::CharacterMaps => 3 + MAX_CASCADES,
            Self::Inset(index) => 4 + MAX_CASCADES + index.min(MAX_INSETS - 1),
            Self::Temporal => 4 + MAX_CASCADES + MAX_INSETS,
            Self::Post => 5 + MAX_CASCADES + MAX_INSETS,
            Self::Ui => 6 + MAX_CASCADES + MAX_INSETS,
            Self::Debug => 7 + MAX_CASCADES + MAX_INSETS,
        }
    }
}

/// One pass's counters, which only its recording adds to.
#[derive(Debug, Default)]
pub(crate) struct Counter {
    draws: AtomicU32,
    instances: AtomicU32,
    triangles: AtomicU64,
    outline_draws: AtomicU32,
    considered: AtomicU32,
    culled_by_frustum: AtomicU32,
    culled_by_pass: AtomicU32,
    culled_by_cutoff: AtomicU32,
    levels: [AtomicU32; MESH_LEVELS],
}

impl Counter {
    /// Adds `counts`, what a group of the pass's calls submitted.
    pub(crate) fn add(&self, counts: &PassCounts) {
        let add = |counter: &AtomicU32, value: u32| {
            if value != 0 {
                counter.fetch_add(value, Ordering::Relaxed);
            }
        };
        add(&self.draws, counts.draws);
        add(&self.instances, counts.instances);
        if counts.triangles != 0 {
            self.triangles.fetch_add(counts.triangles, Ordering::Relaxed);
        }
        add(&self.outline_draws, counts.outline_draws);
        add(&self.considered, counts.considered);
        add(&self.culled_by_frustum, counts.culled_by_frustum);
        add(&self.culled_by_pass, counts.culled_by_pass);
        add(&self.culled_by_cutoff, counts.culled_by_cutoff);
        for (counter, &value) in self.levels.iter().zip(&counts.levels) {
            add(counter, value);
        }
    }

    fn read(&self) -> PassCounts {
        let read = |counter: &AtomicU32| counter.load(Ordering::Relaxed);
        PassCounts {
            draws: read(&self.draws),
            instances: read(&self.instances),
            triangles: self.triangles.load(Ordering::Relaxed),
            outline_draws: read(&self.outline_draws),
            considered: read(&self.considered),
            culled_by_frustum: read(&self.culled_by_frustum),
            culled_by_pass: read(&self.culled_by_pass),
            culled_by_cutoff: read(&self.culled_by_cutoff),
            levels: self.levels.each_ref().map(read),
        }
    }

    fn reset(&self) {
        let zero = |counter: &AtomicU32| counter.store(0, Ordering::Relaxed);
        for counter in [
            &self.draws,
            &self.instances,
            &self.outline_draws,
            &self.considered,
            &self.culled_by_frustum,
            &self.culled_by_pass,
            &self.culled_by_cutoff,
        ] {
            zero(counter);
        }
        self.triangles.store(0, Ordering::Relaxed);
        self.levels.iter().for_each(zero);
    }
}

/// Every counted pass's counters: the renderer's, zeroed at each record.
#[derive(Debug, Default)]
pub(crate) struct PassCounters {
    slots: [Counter; Counted::SLOTS],
}

impl PassCounters {
    pub(crate) fn get(&self, pass: Counted) -> &Counter {
        &self.slots[pass.slot()]
    }

    /// Zeroes every pass's counts, for a frame about to record.
    pub(crate) fn reset(&self) {
        self.slots.iter().for_each(Counter::reset);
    }

    /// What every pass has added since the last [`Self::reset`].
    pub(crate) fn read(&self) -> FrameCounts {
        let read = |pass| self.get(pass).read();
        FrameCounts {
            character_prepass: read(Counted::CharacterPrepass),
            scene_prepass: read(Counted::ScenePrepass),
            main: read(Counted::Main),
            cascades: std::array::from_fn(|index| read(Counted::Cascade(index))),
            character_maps: read(Counted::CharacterMaps),
            insets: std::array::from_fn(|index| read(Counted::Inset(index))),
            temporal: read(Counted::Temporal),
            post: read(Counted::Post),
            ui: read(Counted::Ui),
            debug: read(Counted::Debug),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pass has a slot of its own, and every slot is a pass's.
    #[test]
    fn every_pass_counts_apart() {
        let counters = PassCounters::default();
        let passes = [
            Counted::CharacterPrepass,
            Counted::ScenePrepass,
            Counted::Main,
            Counted::Cascade(0),
            Counted::Cascade(1),
            Counted::Cascade(2),
            Counted::Cascade(3),
            Counted::CharacterMaps,
            Counted::Inset(0),
            Counted::Inset(1),
            Counted::Temporal,
            Counted::Post,
            Counted::Ui,
            Counted::Debug,
        ];
        assert_eq!(passes.len(), Counted::SLOTS);
        for (index, pass) in passes.into_iter().enumerate() {
            counters.get(pass).add(&PassCounts::call(1, index as u64 + 1));
        }
        let read = counters.read();
        for (index, (name, pass)) in read.passes().into_iter().enumerate() {
            assert_eq!(pass.triangles, index as u64 + 1, "{name}");
            assert_eq!(pass.draws, 1, "{name}");
        }
        assert_eq!(read.total().draws, Counted::SLOTS as u32);
        counters.reset();
        assert_eq!(counters.read(), FrameCounts::default());
    }

    /// A mesh draw counts its triangles, its level (a level past the buckets in the last) and whether
    /// it's a hull; the levels sum to the mesh draws.
    #[test]
    fn mesh_draws_count_by_level() {
        let mut counts = PassCounts::default();
        counts.mesh_draw(36, 0, false);
        counts.mesh_draw(36, 0, true);
        counts.mesh_draw(12, 2, false);
        counts.mesh_draw(3, 9, false);
        assert_eq!((counts.draws, counts.instances, counts.triangles, counts.outline_draws), (4, 4, 29, 1));
        assert_eq!(counts.levels, [2, 0, 1, 1]);
        assert_eq!(counts.mesh_draws(), counts.draws);
        let sky = PassCounts::fullscreen();
        let both = counts.plus(&sky);
        assert_eq!((both.draws, both.triangles, both.mesh_draws()), (5, 30, 4));
        assert_eq!(PassCounts::call(5, 2).triangles, 10);
    }

    /// Counting a frame allocates nothing: adding a pass's counts, reading them and starting over.
    #[test]
    fn counting_allocates_nothing() {
        let counters = PassCounters::default();
        let mut counts = PassCounts::default();
        let ((), allocated) = sapphire_core::alloc::count_in_scope(|| {
            counts.mesh_draw(36, 0, true);
            counters.get(Counted::Main).add(&counts);
            counters.get(Counted::Cascade(1)).add(&PassCounts::fullscreen());
            assert_eq!(counters.read().total().draws, 2);
            counters.reset();
        });
        assert_eq!(allocated.allocations, 0, "{allocated:?}");
    }
}
