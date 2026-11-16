//! What one unit's sandbox log says (M5 Z1): the bench's rows, counts, memory, trips and pipelines, as the
//! sandbox's bench logs them (`apps/sapphire_sandbox/src/fight_bench.rs`, its `counts` and `zone` modules, and
//! the plain bench's `bench` and `bench gpu` lines), and the trace's columns where the unit wrote one.

use std::collections::BTreeMap;

/// A timed row's spread, in µs, with where its worst frame fell and its budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timed {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub worst: f64,
    /// Metres along the walk at the worst frame, on a walk.
    pub walked: Option<f64>,
    pub budget: Option<f64>,
    /// A figure proposed where the spec has adopted none (M5 B4's Desktop-High and Mobile-High columns of §3.3):
    /// shown beside the row, never failed on.
    pub proposed: Option<f64>,
}

/// A count row over a phase's frames, with where its worst frame fell and its budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counted {
    pub p50: u64,
    pub p95: u64,
    pub worst: u64,
    /// The phase's sum, which logs from before it was logged (B4's) don't have.
    pub total: Option<u64>,
    /// Decimetres along the walk at the worst frame: a count compares exactly, so its place does too.
    pub walked_dm: Option<i64>,
    pub budget: Option<u64>,
}

/// A row as a phase logged it: read, or logged as not exercised (never zero).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Entry<T> {
    Read(T),
    NotExercised,
}

/// A phase's memory line: each field's start, most and end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Memory {
    /// By field (`process_mib`, `mesh_pool_mib`, `textures_mib`, ...): start, most, end.
    pub fields: BTreeMap<String, [f64; 3]>,
    pub texture_budget_mib: Option<f64>,
    pub process_most_mib: Option<f64>,
}

/// One trip's steps from its exit, as logged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trip {
    /// `covered`, `swapped`, `first_cells`, `clear`: `"<frames> frames <ms> ms"`, or absent.
    pub steps: BTreeMap<String, String>,
    pub frame_p99_us: Option<f64>,
}

/// A phase's pipeline line: what was built after the load, and the hour its first frame drew at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pipelines {
    pub first_use: u64,
    pub frame_thread: u64,
    pub frame_waits: u64,
    /// Pipelines built again after the load (a counting walk's and a fight's line both log it).
    pub rebuilt: u64,
    pub first_hour: Option<f64>,
}

/// A bin of a `texture waits` line, in frames: none completed (`-`), a bin, or the last bin (`32+`), which holds
/// every longer wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitFrames {
    None,
    Frames(u32),
    AtLeast(u32),
}

impl WaitFrames {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "-" => Some(Self::None),
            _ => match value.strip_suffix('+') {
                Some(bin) => bin.parse().ok().map(Self::AtLeast),
                None => value.parse().ok().map(Self::Frames),
            },
        }
    }

    /// Whether it's more than `frames`: the last bin is, past its own.
    pub fn over(self, frames: u32) -> bool {
        match self {
            Self::None => false,
            Self::Frames(bin) => bin > frames,
            Self::AtLeast(bin) => bin >= frames,
        }
    }
}

impl std::fmt::Display for WaitFrames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("-"),
            Self::Frames(bin) => write!(f, "{bin}"),
            Self::AtLeast(bin) => write!(f, "{bin}+"),
        }
    }
}

/// A phase's `texture waits` line (m5-plan exit test 2): every wait for a texture's level completed in it, both
/// kinds, and those still open at its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waits {
    pub completed: u32,
    pub p50: WaitFrames,
    pub p99: WaitFrames,
    pub worst: WaitFrames,
    /// The pool's share: waits a texture's level spent held by the pool.
    pub pool_held: u32,
    pub pool_p99: WaitFrames,
    pub open: u32,
    pub longest_open: u32,
    /// The longest a wait the pool held is still open at the phase's end.
    pub pool_longest_open: u32,
}

/// A phase's `texture wait reasons` line (M5 Z2), beside its waits: whether the pool was ever too small for the
/// textures' tails, the update the zone arrived at (0 if it never did), and its other fields, shown only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaitReasons {
    pub tails_over_pool: u64,
    pub arrived_at_update: u64,
    /// Every field the line logs that reads as a number, by name.
    pub fields: BTreeMap<String, f64>,
}

/// A counting walk's end (M5 Z2, exit test 2's floor): the zone taken down and its pack unmounted, as a trip
/// leaves one, and the mesh pool given back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneClosed {
    /// Whether its pack unmounted.
    pub unmounted: bool,
    /// Whether the pool came back to its floor, and in how many frames.
    pub at_floor: bool,
    pub frames: u32,
    /// The pool's capacity and floor, in bytes, before the close and at the end.
    pub capacity_before: u64,
    pub floor_before: u64,
    pub capacity: u64,
    pub floor: u64,
}

/// A phase and a row: `("cpu", "cpu frame")`.
pub type Key = (String, String);

/// Everything a unit's log says, by phase (`first`, `cpu`, `gpu`, or `drawn` for a counting bench). Rows keep the
/// order the bench logged them in, which is spec §3's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Log {
    pub timed: Vec<(Key, Entry<Timed>)>,
    pub counted: Vec<(Key, Entry<Counted>)>,
    pub memory: BTreeMap<String, Memory>,
    pub trips: BTreeMap<String, Vec<Trip>>,
    pub pipelines: BTreeMap<String, Pipelines>,
    /// Each phase's `texture waits` line, while streaming.
    pub waits: BTreeMap<String, Waits>,
    /// Each phase's `texture wait reasons` line, while streaming.
    pub wait_reasons: BTreeMap<String, WaitReasons>,
    /// A counting walk's `texture streaming cpu` line (M5 Z2): its fields that read as numbers, shown only.
    pub streaming_cpu: Option<BTreeMap<String, f64>>,
    /// A counting walk's `zone closed` line, at its end.
    pub zone_closed: Option<ZoneClosed>,
    /// The load's time, from the bench's first line.
    pub startup_ms: Option<f64>,
    /// A plain bench's `bench` and `bench gpu` fields, numbers only.
    pub fields: BTreeMap<String, f64>,
}

impl Log {
    /// The timed row `row` of `phase`, if the log has it.
    pub fn timed(&self, phase: &str, row: &str) -> Option<Entry<Timed>> {
        self.timed.iter().find(|((p, r), _)| p == phase && r == row).map(|(_, entry)| *entry)
    }

    /// The count row `row` of `phase`, if the log has it.
    pub fn counted(&self, phase: &str, row: &str) -> Option<Entry<Counted>> {
        self.counted.iter().find(|((p, r), _)| p == phase && r == row).map(|(_, entry)| *entry)
    }

    /// Reads a log's lines; lines it doesn't know are passed over.
    pub fn parse(text: &str) -> Self {
        let mut log = Self::default();
        for line in text.lines() {
            let fields = fields(line);
            let field = |name: &str| fields.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
            let float = |name: &str| field(name).and_then(|value| value.parse::<f64>().ok());
            let int = |name: &str| field(name).and_then(|value| value.parse::<u64>().ok());
            let key = || Some((field("phase")?.to_owned(), field("row")?.to_owned()));
            if has(line, "texture streaming cpu") {
                log.streaming_cpu = Some(
                    fields
                        .iter()
                        .filter_map(|(name, value)| Some(((*name).to_owned(), value.parse().ok()?)))
                        .collect(),
                );
            } else if has(line, "texture wait reasons") {
                let Some(phase) = field("phase") else { continue };
                log.wait_reasons.insert(
                    phase.to_owned(),
                    WaitReasons {
                        tails_over_pool: int("tails_over_pool").unwrap_or(0),
                        arrived_at_update: int("arrived_at_update").unwrap_or(0),
                        fields: fields
                            .iter()
                            .filter_map(|(name, value)| Some(((*name).to_owned(), value.parse().ok()?)))
                            .collect(),
                    },
                );
            } else if has(line, "fight row not exercised") || has(line, "count row not exercised") {
                let Some(key) = key() else { continue };
                if has(line, "fight row not exercised") {
                    log.timed.push((key, Entry::NotExercised));
                } else {
                    log.counted.push((key, Entry::NotExercised));
                }
            } else if has(line, "fight row") {
                let (Some(key), Some(p50), Some(p95), Some(p99), Some(worst)) =
                    (key(), float("p50_us"), float("p95_us"), float("p99_us"), float("worst_us"))
                else {
                    continue;
                };
                let budget = field("budget_us").and_then(some).and_then(|value| value.parse().ok());
                let proposed = field("proposed_us").and_then(some).and_then(|value| value.parse().ok());
                let walked = float("worst_walked_m");
                log.timed.push((key, Entry::Read(Timed { p50, p95, p99, worst, walked, budget, proposed })));
            } else if has(line, "count row") {
                let (Some(key), Some(p50), Some(p95), Some(worst)) =
                    (key(), int("p50"), int("p95"), int("worst"))
                else {
                    continue;
                };
                let total = int("total");
                let budget = field("budget").and_then(some).and_then(|value| value.parse().ok());
                let walked_dm = float("worst_walked_m").map(|metres| (metres * 10.0).round() as i64);
                log.counted.push((key, Entry::Read(Counted { p50, p95, worst, total, walked_dm, budget })));
            } else if has(line, "zone memory, start/most@tick/end:") {
                let Some(phase) = field("phase") else { continue };
                let mut memory = Memory {
                    texture_budget_mib: float("texture_budget_mib"),
                    process_most_mib: float("process_most_mib"),
                    ..Memory::default()
                };
                for (name, value) in &fields {
                    // `start/most@tick/end`.
                    let Some((start, rest)) = value.split_once('/') else { continue };
                    let Some((most, end)) = rest.split_once('/') else { continue };
                    let most = most.split('@').next().unwrap_or(most);
                    if let (Ok(start), Ok(most), Ok(end)) = (start.parse(), most.parse(), end.parse()) {
                        memory.fields.insert((*name).to_owned(), [start, most, end]);
                    }
                }
                log.memory.insert(phase.to_owned(), memory);
            } else if has(line, "zone trip, from its exit") {
                let Some(phase) = field("phase") else { continue };
                let mut trip = Trip { frame_p99_us: float("frame_p99_us"), ..Trip::default() };
                for step in ["covered", "swapped", "first_cells", "clear"] {
                    if let Some(value) = field(step).and_then(some) {
                        trip.steps.insert(step.to_owned(), value.to_owned());
                    }
                }
                log.trips.entry(phase.to_owned()).or_default().push(trip);
            } else if has(line, "zone closed") {
                log.zone_closed = Some(ZoneClosed {
                    unmounted: field("unmounted") == Some("unmounted"),
                    at_floor: field("at_floor") == Some("true"),
                    frames: field("frames").and_then(|value| value.parse().ok()).unwrap_or(0),
                    capacity_before: int("capacity_before").unwrap_or(0),
                    floor_before: int("floor_before").unwrap_or(0),
                    capacity: int("capacity").unwrap_or(0),
                    floor: int("floor").unwrap_or(0),
                });
            } else if has(line, "zone walk") {
                // A plain bench's walk (a counting walk's): its pipelines over the whole run, under the phase its
                // count rows took, as a fight's bench logs each phase's.
                log.pipelines.insert(
                    "drawn".to_owned(),
                    Pipelines {
                        first_use: int("first_use").unwrap_or(0),
                        frame_thread: int("frame_thread").unwrap_or(0),
                        frame_waits: int("frame_waits").unwrap_or(0),
                        rebuilt: int("rebuilt").unwrap_or(0),
                        first_hour: None,
                    },
                );
            } else if has(line, "texture waits") {
                let bin = |name: &str| field(name).and_then(WaitFrames::parse);
                let count = |name: &str| field(name).and_then(|value| value.parse::<u32>().ok());
                let (
                    Some(phase),
                    Some(completed),
                    Some(p50),
                    Some(p99),
                    Some(worst),
                    Some(open),
                    Some(longest_open),
                ) = (
                    field("phase"),
                    count("completed"),
                    bin("p50_frames"),
                    bin("p99_frames"),
                    bin("worst_frames"),
                    count("open"),
                    count("longest_open_frames"),
                )
                else {
                    continue;
                };
                log.waits.insert(
                    phase.to_owned(),
                    Waits {
                        completed,
                        p50,
                        p99,
                        worst,
                        pool_held: count("pool_held").unwrap_or(0),
                        pool_p99: bin("pool_held_p99_frames").unwrap_or(WaitFrames::None),
                        open,
                        longest_open,
                        pool_longest_open: count("pool_longest_open_frames").unwrap_or(0),
                    },
                );
            } else if has(line, "fight pipelines") {
                let Some(phase) = field("phase") else { continue };
                log.pipelines.insert(
                    phase.to_owned(),
                    Pipelines {
                        first_use: int("first_use").unwrap_or(0),
                        frame_thread: int("frame_thread").unwrap_or(0),
                        frame_waits: int("frame_waits").unwrap_or(0),
                        rebuilt: int("rebuilt").unwrap_or(0),
                        first_hour: float("first_hour"),
                    },
                );
            } else if has(line, "bench of a fight") || has(line, "bench") || has(line, "bench gpu") {
                if let Some(startup) = float("startup_ms") {
                    log.startup_ms = Some(startup);
                }
                if has(line, "bench") || has(line, "bench gpu") {
                    for (name, value) in &fields {
                        if let Ok(number) = value.parse::<f64>() {
                            log.fields.insert((*name).to_owned(), number);
                        }
                    }
                }
            }
        }
        log
    }
}

/// Whether `line`'s message is `message`: the text between the log's target and its first field.
fn has(line: &str, message: &str) -> bool {
    line.find(&format!(" {message} ")).is_some_and(|at| {
        let after = &line[at + message.len() + 2..];
        after.split(' ').next().is_some_and(|first| first.contains('='))
    }) || line.ends_with(&format!(" {message}"))
}

/// `Some(x)`'s `x`, or the value itself when it isn't written as an option; `None` for `None`.
fn some(value: &str) -> Option<&str> {
    match value {
        "None" => None,
        _ => Some(value.strip_prefix("Some(").and_then(|rest| rest.strip_suffix(')')).unwrap_or(value)),
    }
}

/// A log line's `key=value` fields, a quoted value without its quotes.
pub(crate) fn fields(line: &str) -> Vec<(&str, &str)> {
    let mut fields = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find('=') {
        let key = rest[..at].rsplit(' ').next().unwrap_or_default();
        let after = &rest[at + 1..];
        let (value, next) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.find('"') {
                Some(end) => (&quoted[..end], &quoted[end + 1..]),
                None => (quoted, ""),
            },
            None => {
                let end = after.find(' ').unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        if !key.is_empty() {
            fields.push((key, value));
        }
        rest = next;
    }
    fields
}

/// A trace's column `name` over the frames of `phase`, in order: what `--bench-trace` wrote.
pub fn trace_column(text: &str, phase: &str, name: &str) -> Option<Vec<f64>> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next()?.split(',').collect();
    let (phase_at, at) =
        (header.iter().position(|&h| h == "phase")?, header.iter().position(|&h| h == name)?);
    Some(
        lines
            .map(|line| line.split(',').collect::<Vec<_>>())
            .filter(|cells| cells.get(phase_at) == Some(&phase))
            .filter_map(|cells| cells.get(at)?.parse().ok())
            .collect(),
    )
}

/// The `share` percentile of `values` by nearest rank, as the bench's spreads take it.
pub fn percentile(values: &[f64], share: f64) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let last = sorted.len().checked_sub(1)?;
    Some(sorted[((share * last as f64).round() as usize).min(last)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const WALK: &str = include_str!("fixtures/tour.desktop-low.r1.log");

    /// A counting walk logs its pipelines on its `zone walk` line, not a phase line: read them there, under the
    /// phase its count rows took, so the report's pipelines table shows them rather than assuming none.
    #[test]
    fn a_counting_walks_pipelines_are_read_from_its_walk_line() {
        let line = "2026-11-15T10:00:00.000000Z  INFO sapphire::sandbox: zone walk frames=7660 \
                    streaming_us_p50=0.0 prewarmed=812 first_use=0 rebuilt=0 warmed_off_frame=812 frame_thread=0 \
                    frame_waits=3";
        let log = Log::parse(line);
        assert_eq!(log.pipelines.get("drawn").map(|built| built.frame_waits), Some(3), "{log:?}");
    }

    #[test]
    fn a_walks_log_reads_its_rows_counts_memory_and_pipelines() {
        let log = Log::parse(WALK);
        let Some(Entry::Read(cpu)) = log.timed("cpu", "cpu frame") else { panic!("{log:?}") };
        assert_eq!((cpu.p50, cpu.p95, cpu.worst, cpu.budget), (1182.0, 2264.0, 20330.0, Some(12000.0)));
        assert_eq!(cpu.walked, Some(30.1));
        assert_eq!(log.timed("cpu", "simulation"), Some(Entry::NotExercised));
        let Some(Entry::Read(calls)) = log.counted("cpu", "draw calls") else { panic!() };
        assert_eq!((calls.p95, calls.budget, calls.walked_dm), (7135, Some(2500), Some(6280)));
        assert_eq!(log.counted("cpu", "joints"), Some(Entry::NotExercised));
        assert_eq!(log.memory["first"].fields["process_mib"], [3619.2, 3819.9, 3759.7]);
        assert_eq!(log.memory["first"].texture_budget_mib, Some(2560.0));
        assert_eq!(log.pipelines["gpu"].first_hour, Some(19.7667));
        assert_eq!(log.startup_ms, Some(10337.3));
    }

    /// The completed waits' line (M5 Z2): a bin, the last bin's `32+` and an empty histogram's `-`.
    #[test]
    fn a_phases_texture_waits_are_read_with_their_bins() {
        let log = Log::parse(
            "2026-11-16T00:00:00.000000Z  INFO sapphire::sandbox: texture waits phase=\"drawn\" completed=40 \
             p50_frames=2 p99_frames=32+ worst_frames=32+ pool_held=3 pool_held_p99_frames=- \
             pool_held_worst_frames=- open=1 longest_open_frames=12 pool_open=0 pool_longest_open_frames=0",
        );
        let waits = log.waits["drawn"];
        assert_eq!(
            (waits.completed, waits.p50, waits.p99),
            (40, WaitFrames::Frames(2), WaitFrames::AtLeast(32))
        );
        assert_eq!((waits.pool_p99, waits.open, waits.longest_open), (WaitFrames::None, 1, 12));
        assert!(waits.p99.over(30) && !waits.p50.over(30) && !WaitFrames::None.over(0));
        assert!(!WaitFrames::Frames(30).over(30) && WaitFrames::Frames(31).over(30));
    }

    /// A counting walk's end (M5 Z2): the pool against its floor once the zone is closed.
    #[test]
    fn a_closed_zones_pool_is_read_against_its_floor() {
        let log = Log::parse(
            "2026-11-16T00:00:00.000000Z  INFO sapphire::sandbox: zone closed unmounted=\"unmounted\" frames=4 \
             at_floor=true capacity_before=50331648 floor_before=50331648 capacity=8388608 floor=8388608 \
             in_use=6000000 meshes_freed=244",
        );
        let closed = log.zone_closed.expect("the line");
        assert!(closed.unmounted && closed.at_floor);
        assert_eq!((closed.frames, closed.capacity, closed.floor), (4, 8_388_608, 8_388_608));
        assert_eq!((closed.capacity_before, closed.floor_before), (50_331_648, 50_331_648));
        assert!(Log::parse("zone walk frames=3").zone_closed.is_none());
    }

    #[test]
    fn a_trace_gives_a_columns_percentile() {
        let trace = "phase,frame,worst_texture_debt\nfirst,0,3\nfirst,1,40\ncpu,0,99\nfirst,2,5\n";
        let column = trace_column(trace, "first", "worst_texture_debt").unwrap();
        assert_eq!(column, [3.0, 40.0, 5.0]);
        assert_eq!(percentile(&column, 0.99), Some(40.0));
        assert_eq!(percentile(&[], 0.5), None);
        assert!(trace_column(trace, "first", "no_such").is_none());
    }
}
