//! Instanced runs (M5 D2, spec §5.12 "Scene and draws", m5-plan §3 "Instancing without first
//! instance"): neighbours in a view's list that draw the same thing, the same mesh, level, submesh
//! and material through the same pipeline, draw as one call of instances `0..n`.
//!
//! - **Where runs form:** in the groups of static depth-writing surfaces (a position format, a
//!   surface variant that writes depth, the surfaces set), from neighbours in the list's order,
//!   which a run keeps. A run is of one level of detail (M5 D4), whose index ranges it draws: two
//!   draws of a mesh at different levels never join. The shadow views sort those groups by mesh,
//!   then level, then the submesh's place (the `lists` module's `DrawKey`), so each mesh's submesh
//!   at a level is one run there; the prepasses keep the draw list's order, where a run forms only
//!   where equal draws happen to be neighbours. Groups
//!   whose order shows (transparent surfaces, eyes' layers, eyes through hair, outline hulls) draw a
//!   call a draw, as do skinned and morphed instances, which bind their own outputs, and fog cards,
//!   which sort themselves.
//! - **Which views** ([`DrawRuns`], `RenderSettings::draw_runs`): by default the shadow views
//!   alone (each cascade, each character map), where a run writes every byte its draws wrote one by
//!   one and, as they sort by mesh, the runs and calls don't depend on the order instances were
//!   placed in. The world's two prepasses write every byte alike too (every Metal golden, and the
//!   readback test on every tier), but they keep the draw list's order, instance slot order, which
//!   follows the order streamed cells arrive in: their runs, and so the frame's calls, would move
//!   with it, and in that order they hold almost no equal neighbours, so they run only by
//!   `DrawRuns::ShadowsAndPrepasses`. The main view and the insets draw a call a draw: on Metal, one
//!   instanced call of the main pass's
//!   pipelines shades a few pixels differently from the same records drawn as one call each (five
//!   desktop goldens with temporal AA, up to 8 code values on under 0.04% of pixels). It isn't a tie
//!   in depth resolved the other way: the same instances drawn as separate calls in reverse order
//!   move nothing, and an exact tie between two instances of one call shows the first, as separate
//!   calls do (`a_tie_between_a_runs_instances_shows_one_of_them`). `DrawRuns::EveryView` runs them
//!   too, for the bench and for the test that shows it.
//! - **How a run's records reach the shader:** a vertex stage reads its draw's record as
//!   instance-step vertex data (slot 0), with no first instance (the iOS Simulator has none, ADR
//!   0002), so a run's records must lie side by side, the first where the slot is bound. The draw
//!   list's records lie in slot order, an instance's submeshes together, so a run's seldom do. Each
//!   frame, as each list forms its runs, a run of more than one draw lists its records' indices in
//!   the frame's run records, which the uploads pass copies into their own buffer
//!   ([`RunRecords`]) through the staging belt; the run binds that buffer at its first. A run of one
//!   binds its record where it lies, as every draw did before. Where the main view formed runs, the
//!   scene prepass takes them wherever its group lists what the main view's does (always, as they
//!   share one view's tests), and writes no records of its own. Nothing changes in a shader or a
//!   bind group, and the path is the same on every backend.
//! - **Off** (`DrawRuns::Off`), no list forms runs and every draw is a call of one instance, as
//!   before: for the test that draws scenes both ways and compares every byte, and for the bench.
//!
//! Lists keep their runs' capacity, as they keep their draws', so a warm frame allocates nothing.

use sapphire_assets::mesh::PositionFormat;
use sapphire_rhi::wgpu;

use super::lists::Entry;
use crate::buffer::GrowableBuffer;
use crate::gpu_types::DrawData;
use crate::pipelines::{SurfaceVariant, format_index};
use crate::scene::{DrawCommand, DrawList};

const FORMATS: usize = PositionFormat::ALL.len();
/// The variants whose static surfaces can run: the depth-writing ones, first in
/// [`SurfaceVariant::ALL`].
const RUNNING: usize = SurfaceVariant::DEPTH_WRITING.len();
const RECORD_BYTES: u64 = size_of::<DrawData>() as u64;

/// Which views' lists form runs (M5 D2, `RenderSettings::draw_runs`; the sandbox's `--draw-runs
/// off|shadows|prepasses|every`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawRuns {
    /// None: every draw is a call of one instance. For the test that compares, and the bench.
    Off,
    /// The shadow views alone (each cascade, each character map), every tier's: they sort by mesh,
    /// so their runs and calls are the same whatever order instances were placed in, and every
    /// call the district saves is theirs.
    #[default]
    Shadows,
    /// The shadow views and the world's scene and character prepasses. Not the default: the
    /// prepasses keep the draw list's order, instance slot order, which follows the order streamed
    /// cells arrive in, so their runs and the frame's calls would move with it (and the nightly
    /// scale lane holds those rows exactly); in that order they hold almost no equal neighbours.
    ShadowsAndPrepasses,
    /// Every view, the main view and the insets too. Not the default: on Metal an instanced call of
    /// the main pass's pipelines moves a few pixels by a code value or two, cause under study (the
    /// module docs).
    EveryView,
}

/// What a view's list draws, for which [`DrawRuns`] forms its runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunView {
    /// A cascade or a character map.
    Shadow,
    /// The world's scene or character prepass.
    Prepass,
    /// The main view or an inset, whose colour is what the frame shows.
    Shown,
}

impl DrawRuns {
    /// Whether `view`'s list forms runs.
    pub(crate) fn form_in(self, view: RunView) -> bool {
        match self {
            Self::Off => false,
            Self::Shadows => view == RunView::Shadow,
            Self::ShadowsAndPrepasses => view != RunView::Shown,
            Self::EveryView => true,
        }
    }
}

/// One call of a view's list: `count` neighbours from `first`, in their group, equal in what they
/// draw ([`joins`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    /// Its first draw's place in its group of the view's list.
    pub first: u32,
    /// Its draws: the call's instances.
    pub count: u32,
    /// Where its first instance's record lies: a run of one's own record in the draw records, a
    /// longer run's first in the frame's run records.
    pub record: u32,
}

/// Whether `next` draws what `run` does, so it can join its run: the same mesh (so the same
/// streams, at the same first vertex), the same level of detail and submesh (so the same index
/// range, M5 D4) and the same material. They share a group already, so a pipeline.
pub(crate) fn joins((run, run_level): (&DrawCommand, u8), (next, next_level): (&DrawCommand, u8)) -> bool {
    run.mesh == next.mesh
        && run_level == next_level
        && run.indices == next.indices
        && run.material == next.material
}

/// A view list's runs, per group of static depth-writing surfaces.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Runs {
    runs: Vec<Run>,
    /// Per format and running variant, its runs' range in `runs`.
    ranges: [[[u32; 2]; RUNNING]; FORMATS],
    /// Whether they were formed since they were last cleared: a twin's runs are taken only then.
    formed: bool,
}

impl Default for Runs {
    fn default() -> Self {
        Self { runs: Vec::new(), ranges: [[[0; 2]; RUNNING]; FORMATS], formed: false }
    }
}

impl Runs {
    /// None, keeping the capacity: every draw is a call of its own.
    pub(crate) fn clear(&mut self) {
        self.runs.clear();
        self.ranges = [[[0; 2]; RUNNING]; FORMATS];
        self.formed = false;
    }

    /// The runs of `variant`'s static surfaces in `format`, in order: none if it formed none (a
    /// group that doesn't run, or the runs are off), when its draws are drawn one by one.
    pub(crate) fn of(&self, format: PositionFormat, variant: SurfaceVariant) -> &[Run] {
        match self.ranges[format_index(format)].get(variant.index()) {
            Some(&[start, end]) => &self.runs[start as usize..end as usize],
            None => &[],
        }
    }

    /// Forms the runs of each running group, whose listed draws `group` gives (positions in the
    /// draw list's group), from `list`'s commands. A run of more than one lists its records in
    /// `records`. A group that `twin` (its runs, and its groups' listed draws) lists alike takes
    /// `twin`'s runs, whose records are listed already, if `twin` formed them since it was cleared.
    pub(crate) fn form<'a, G, T>(
        &mut self,
        group: G,
        twin: Option<(&Self, T)>,
        list: &DrawList,
        records: &mut Vec<u32>,
    ) where
        G: Fn(PositionFormat, SurfaceVariant) -> &'a [Entry],
        T: Fn(PositionFormat, SurfaceVariant) -> &'a [Entry],
    {
        self.clear();
        let twin = twin.filter(|(twin, _)| twin.formed);
        let Self { runs, ranges, formed } = self;
        *formed = true;
        for format in PositionFormat::ALL {
            for &variant in &SurfaceVariant::DEPTH_WRITING {
                let listed = group(format, variant);
                let start = to_u32(runs.len());
                match &twin {
                    Some((twin, twin_group)) if twin_group(format, variant) == listed => {
                        runs.extend_from_slice(twin.of(format, variant));
                    }
                    _ => {
                        let commands = &list.commands[format_index(format)][variant.index()];
                        form_group(listed, commands, runs, records);
                    }
                }
                ranges[format_index(format)][variant.index()] = [start, to_u32(runs.len())];
            }
        }
    }

    /// Every run, in order: for the frame's stats.
    pub(crate) fn all(&self) -> &[Run] {
        &self.runs
    }
}

/// Forms the runs of one group's listed draws, `listed` (positions in `commands`, and levels).
fn form_group(listed: &[Entry], commands: &[DrawCommand], runs: &mut Vec<Run>, records: &mut Vec<u32>) {
    let drawn = |entry: &Entry| (&commands[entry.at as usize], entry.level);
    let mut at = 0;
    while at < listed.len() {
        let first = drawn(&listed[at]);
        let count = 1 + listed[at + 1..].iter().take_while(|next| joins(first, drawn(next))).count();
        let record = if count == 1 {
            first.0.draw_id
        } else {
            let start = to_u32(records.len());
            records.extend(listed[at..at + count].iter().map(|entry| commands[entry.at as usize].draw_id));
            start
        };
        runs.push(Run { first: to_u32(at), count: to_u32(count), record });
        at += count;
    }
}

/// What the frame's lists made of their draws (M5 D2, `RenderStats::runs`): of every run they
/// formed, the scene prepass's too whether or not it draws, those of more than one draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunStats {
    /// Runs of more than one draw: calls that draw several instances.
    pub runs: u32,
    /// The instances those draw.
    pub instances: u32,
    /// The longest run's instances.
    pub longest: u32,
    /// The median run's instances, of the runs of more than one (the lower middle of an even count).
    pub median: u32,
    /// Bytes of records the frame wrote for them (each a `DrawData` of 48 bytes): the run records,
    /// which the scene prepass shares with the main view.
    pub record_bytes: u64,
}

impl RunStats {
    /// The stats of `runs`, every run the frame formed, with `records` listed for them; `lengths` is
    /// scratch, kept so a warm frame allocates nothing.
    pub(crate) fn of<'a>(
        runs: impl Iterator<Item = &'a Run>,
        records: usize,
        lengths: &mut Vec<u32>,
    ) -> Self {
        lengths.clear();
        lengths.extend(runs.filter(|run| run.count > 1).map(|run| run.count));
        let instances = lengths.iter().sum();
        let longest = lengths.iter().copied().max().unwrap_or(0);
        let median = if lengths.is_empty() {
            0
        } else {
            let middle = (lengths.len() - 1) / 2;
            *lengths.select_nth_unstable(middle).1
        };
        Self {
            runs: to_u32(lengths.len()),
            instances,
            longest,
            median,
            record_bytes: records as u64 * RECORD_BYTES,
        }
    }
}

/// The frame's run records on the GPU: for each run of more than one draw, its draws' records side
/// by side, in the order the lists formed them, read as instance-step vertex data as the draw
/// records are. Rewritten every frame; it only grows.
#[derive(Debug)]
pub(crate) struct RunRecords {
    buffer: GrowableBuffer,
}

impl RunRecords {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let limit = device.limits().max_buffer_size;
        let buffer = GrowableBuffer::new(
            device,
            "run records",
            wgpu::BufferUsages::VERTEX,
            1024 * RECORD_BYTES,
            limit,
        );
        Self { buffer }
    }

    /// Whether `records` records fit the buffer's limit (review RA-004): if not, the frame draws a call a
    /// draw, forming no runs.
    pub(crate) fn fits(&self, records: usize) -> bool {
        self.buffer.fits(records as u64 * RECORD_BYTES)
    }

    /// Makes room for `records` records, which must [fit](Self::fits). Nothing is kept: the frame writes
    /// them all.
    pub(crate) fn reserve(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, records: usize) {
        self.buffer.reserve(device, queue, records as u64 * RECORD_BYTES, 0);
    }

    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        self.buffer.buffer()
    }

    /// Writes the records `records` names, from the draw list's `draws`, through `belt`, straight into
    /// the belt's mapped memory.
    pub(crate) fn write(
        &self,
        belt: &mut wgpu::util::StagingBelt,
        encoder: &mut wgpu::CommandEncoder,
        records: &[u32],
        draws: &[DrawData],
    ) {
        let Some(size) = wgpu::BufferSize::new(records.len() as u64 * RECORD_BYTES) else { return };
        let mut view = belt.write_buffer(encoder, self.buffer.buffer(), 0, size);
        // Mapped memory is written, never read: record by record, through the view's slices.
        let bytes = RECORD_BYTES as usize;
        for (at, &record) in (0..).step_by(bytes).zip(records) {
            view.slice(at..at + bytes).copy_from_slice(bytemuck::bytes_of(&draws[record as usize]));
        }
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("draw counts fit u32")
}
