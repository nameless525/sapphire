//! What a bench of a walk through a zone adds to a fight's (M3 Z1): memory across the walk, by the engine's
//! own accounting and the process's as its OS counts it, and each zone trip's time, step by step.
//!
//! - **Memory** is read after every frame: what the asset server holds, of it meshes and textures against their
//!   budgets, the renderer's mesh pool and textures, its meshes and materials, the scene's entities, the heap,
//!   and the process ([`sapphire_platform::process_memory`]). Each phase logs it at its first frame, at its
//!   last, and at its most, with the tick each most fell on.
//! - **A trip** (`zone-trip`'s, or a game flow's) is timed from the frame its exit was taken through the
//!   frame the wipe covered, the swap (its zone ready and the simulation in it), its first cells placed, and
//!   the wipe gone: in frames, and in the bench's ms, each frame's CPU and its wait for the GPU. What the load
//!   itself took is the stage's own report, `arrived in a zone`'s `load`. The frames inside a trip have their
//!   own p50 and p99, and a hub's prefetch its cost a frame, as `zone prefetch`'s row has it.

use std::time::Duration;

use sapphire_assets::AssetKind;

use super::{Phase, Traced, spread};
use crate::LOG_TARGET;
use crate::offscreen::Stage;
use game_combat::fight::trip::TripPhase;

/// What the engine held after a frame, in bytes but for the counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Held {
    /// All the asset server holds, cooked.
    resident: u64,
    resident_meshes: u64,
    resident_textures: u64,
    mesh_pool: u64,
    textures: u64,
    meshes: u32,
    materials: u32,
    entities: u32,
    heap: u64,
    /// The process, as its OS counts it: 0 where it isn't read.
    process: u64,
}

impl Held {
    pub(super) fn of(stage: &Stage, heap: usize) -> Self {
        let assets = stage.content.stats();
        let render = stage.renderer.stats();
        Self {
            resident: assets.resident.iter().sum(),
            resident_meshes: assets.resident[AssetKind::Mesh.index()],
            // Tails and the levels above them alike (M5 M4).
            resident_textures: assets.resident[AssetKind::Texture.index()]
                + assets.resident[AssetKind::TextureLevel.index()],
            mesh_pool: render.mesh_pool_bytes,
            textures: stage.content.texture_bytes(),
            meshes: render.meshes,
            materials: render.materials,
            entities: stage.scene.world().entities().len(),
            heap: heap as u64,
            process: sapphire_platform::process_memory().map_or(0, |memory| memory.now),
        }
    }

    /// `--bench-trace`'s columns from `resident`: bytes, and counts.
    pub(super) fn trace_into(&self, line: &mut String) {
        use std::fmt::Write as _;
        let _ = write!(
            line,
            "{},{},{},{},{}",
            self.resident, self.mesh_pool, self.textures, self.entities, self.process
        );
    }

    /// Each field's reading, by name, with bytes in MiB.
    fn fields(&self) -> [(&'static str, f64); 10] {
        let mib = |bytes: u64| bytes as f64 / 1_048_576.0;
        [
            ("resident_mib", mib(self.resident)),
            ("resident_meshes_mib", mib(self.resident_meshes)),
            ("resident_textures_mib", mib(self.resident_textures)),
            ("mesh_pool_mib", mib(self.mesh_pool)),
            ("textures_mib", mib(self.textures)),
            ("meshes", f64::from(self.meshes)),
            ("materials", f64::from(self.materials)),
            ("entities", f64::from(self.entities)),
            ("heap_mib", mib(self.heap)),
            ("process_mib", mib(self.process)),
        ]
    }
}

/// Logs a phase's memory, at its first frame, its last and its most, and the trips in it.
pub(super) fn report(stage: &Stage, trace: &[Traced], phase: Phase) {
    let frames: Vec<&Traced> = trace.iter().filter(|frame| frame.phase == phase).collect();
    let (Some(first), Some(last)) = (frames.first(), frames.last()) else { return };
    let budgets = stage.content.stats().budgets;
    let mib = |bytes: u64| bytes as f64 / 1_048_576.0;
    let mut line = String::new();
    for (index, ((name, start), (_, end))) in
        first.memory.fields().into_iter().zip(last.memory.fields()).enumerate()
    {
        let (most, tick) = frames
            .iter()
            .map(|frame| (frame.memory.fields()[index].1, frame.tick))
            .fold((f64::MIN, 0), |best, next| if next.0 > best.0 { next } else { best });
        line.push_str(&format!(" {name}={start:.1}/{most:.1}@{tick}/{end:.1}"));
    }
    let process = sapphire_platform::process_memory();
    tracing::info!(
        target: LOG_TARGET,
        phase = phase.name(),
        frames = frames.len(),
        start_tick = first.tick,
        end_tick = last.tick,
        mesh_budget_mib = format_args!("{:.1}", mib(budgets.resident(AssetKind::Mesh))),
        texture_budget_mib = format_args!("{:.1}", mib(budgets.resident(AssetKind::Texture))),
        process_most_mib = process.map(|memory| format!("{:.1}", mib(memory.most))),
        "zone memory, start/most@tick/end:{line}",
    );
    trips(&frames, phase);
}

/// How far along a trip is, in order.
const fn rank(trip: Option<TripPhase>) -> u8 {
    match trip {
        None => 4,
        Some(TripPhase::Covering) => 0,
        Some(TripPhase::Swapping) => 1,
        Some(TripPhase::Arriving) => 2,
        Some(TripPhase::Uncovering) => 3,
    }
}

/// When a trip reached each of its steps: frames since its exit was taken, counting that frame, and the
/// bench's time over them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Reached {
    frames: u32,
    ms: f64,
}

/// One trip, from the frame its exit was taken.
#[derive(Clone, Debug, Default, PartialEq)]
struct TripTimes {
    tick: u64,
    covered: Option<Reached>,
    swapped: Option<Reached>,
    first_cells: Option<Reached>,
    clear: Option<Reached>,
    /// The frames from the exit to the wipe gone, each one's CPU and wait.
    frames: Vec<Duration>,
    cpu: Vec<Duration>,
}

/// The trips in a phase's frames, in order. A trip still under way at the phase's end has no `clear`.
fn trips_in(frames: &[&Traced]) -> Vec<TripTimes> {
    let mut trips = Vec::new();
    let mut under_way: Option<TripTimes> = None;
    for frame in frames {
        let length = frame.cpu + frame.wait;
        let trip = match (&mut under_way, frame.trip) {
            (Some(trip), _) => trip,
            (None, Some(_)) => under_way.insert(TripTimes { tick: frame.tick, ..TripTimes::default() }),
            (None, None) => continue,
        };
        trip.frames.push(length);
        trip.cpu.push(frame.cpu);
        let reached = Reached {
            frames: trip.frames.len() as u32,
            ms: trip.frames.iter().sum::<Duration>().as_secs_f64() * 1e3,
        };
        let rank = rank(frame.trip);
        for (step, slot) in
            [(1, &mut trip.covered), (2, &mut trip.swapped), (3, &mut trip.first_cells), (4, &mut trip.clear)]
        {
            if rank >= step && slot.is_none() {
                *slot = Some(reached);
            }
        }
        if frame.trip.is_none() {
            trips.extend(under_way.take());
        }
    }
    trips.extend(under_way);
    trips
}

/// Logs each trip in a phase's frames, and what prefetching cost a frame across them.
fn trips(frames: &[&Traced], phase: Phase) {
    let trips = trips_in(frames);
    if trips.is_empty() {
        return;
    }
    let micros = |duration: Duration| duration.as_secs_f64() * 1e6;
    let at = |reached: Option<Reached>| {
        reached.map(|reached| format!("{} frames {:.1} ms", reached.frames, reached.ms))
    };
    for (index, trip) in trips.iter().enumerate() {
        let total = spread(trip.frames.iter().map(|&length| (length, trip.tick)));
        let cpu = spread(trip.cpu.iter().map(|&length| (length, trip.tick)));
        tracing::info!(
            target: LOG_TARGET,
            phase = phase.name(),
            trip = index,
            exit_tick = trip.tick,
            covered = at(trip.covered),
            swapped = at(trip.swapped),
            first_cells = at(trip.first_cells),
            clear = at(trip.clear),
            frame_p50_us = total.map(|spread| format!("{:.1}", micros(spread.p50))),
            frame_p99_us = total.map(|spread| format!("{:.1}", micros(spread.p99))),
            frame_worst_us = total.map(|spread| format!("{:.1}", micros(spread.worst))),
            cpu_p50_us = cpu.map(|spread| format!("{:.1}", micros(spread.p50))),
            cpu_p99_us = cpu.map(|spread| format!("{:.1}", micros(spread.p99))),
            "zone trip, from its exit",
        );
    }
    let prefetching = || frames.iter().filter(|frame| !frame.prefetch.is_zero());
    let prefetch = spread(prefetching().map(|frame| (frame.prefetch, frame.tick)));
    tracing::info!(
        target: LOG_TARGET,
        phase = phase.name(),
        trips = trips.len(),
        prefetch_frames = prefetching().count(),
        prefetch_p50_us = prefetch.map(|spread| format!("{:.1}", micros(spread.p50))),
        prefetch_p99_us = prefetch.map(|spread| format!("{:.1}", micros(spread.p99))),
        prefetch_worst_us = prefetch.map(|spread| format!("{:.1}", micros(spread.worst))),
        prefetch_worst_tick = prefetch.map(|spread| spread.worst_tick),
        "zone trips",
    );
}

#[cfg(test)]
mod tests {
    use super::super::tests::traced;
    use super::*;

    fn frame(tick: u64, trip: Option<TripPhase>, ms: u64) -> Traced {
        Traced { trip, wait: Duration::from_millis(ms), ..traced(tick, 0) }
    }

    /// A trip's steps are timed from the frame its exit was taken, each from the first frame that reached it,
    /// and one that skips a step, as one with no wipe skips covering, reaches it with the next.
    #[test]
    fn a_trip_is_timed_step_by_step_from_its_exit() {
        let trace = [
            frame(1, None, 9),
            frame(2, Some(TripPhase::Covering), 1),
            frame(2, Some(TripPhase::Covering), 2),
            frame(2, Some(TripPhase::Arriving), 3),
            frame(3, Some(TripPhase::Uncovering), 4),
            frame(4, None, 5),
            frame(5, None, 9),
            frame(6, Some(TripPhase::Swapping), 1),
        ];
        let frames: Vec<&Traced> = trace.iter().collect();
        let trips = trips_in(&frames);
        assert_eq!(trips.len(), 2, "{trips:?}");
        let first = &trips[0];
        assert_eq!(first.tick, 2);
        assert_eq!(first.covered, Some(Reached { frames: 3, ms: 6.0 }), "covered as it swapped");
        assert_eq!(first.swapped, Some(Reached { frames: 3, ms: 6.0 }));
        assert_eq!(first.first_cells, Some(Reached { frames: 4, ms: 10.0 }));
        assert_eq!(first.clear, Some(Reached { frames: 5, ms: 15.0 }));
        assert_eq!(first.frames.len(), 5);
        let second = &trips[1];
        assert_eq!((second.tick, second.covered.map(|at| at.frames), second.clear), (6, Some(1), None));
    }
}
