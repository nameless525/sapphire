//! The batch's report (M5 Z1): a commit's logs turned into the Markdown spec §3.8 takes, each row beside its
//! budget and beside the baseline's, with what is over said plainly.
//!
//! - **Times** are one round's own (M6 B1): a row reads the round a verdict reads, the least p95 of the valid
//!   rounds, named, with that round's p50 and worst frame and the metres walked where it fell. A GPU row leaves out
//!   the rounds the display's 60 Hz lock voids, so every GPU figure a table reads is one a verdict could rest on;
//!   a CPU row reads every round. Beside it, where they differ, each statistic's least over every round, void ones
//!   too (B4's reading, which can print a frame no round produced), is shown as a diagnostic, never judged. Each
//!   timed run's rounds table prints a row a round: its lock, the lock's floor for the round's slow frames, the
//!   chance of reading it with no lock, and whether it is void. A unit read above the manifest's load bound is
//!   marked, never dropped.
//! - **Work, waits and frames left out** (M6 B3): beside the CPU rows, each span's wall time (the CPU frame, its
//!   record, its wait for the GPU) beside the frame thread's CPU time over it, from one round, and that round's
//!   frames by kind of core; then every round's frames that no row reads (restarts, untimed GPU frames, a plain
//!   bench's warm-up) as rows of their own. A log from before them prints neither.
//! - **Counts** of a counting run are exact: every round must read the same. A count that differs is shown with
//!   each round's, each marked against its budget, and on a run of counts alone that isn't streaming it fails
//!   the report, as the scale lane would; a timed walk's may differ, and a round over its budget stays marked.
//! - **Nothing is zero that wasn't read:** a row a run should have and no round logged says `missing`, and a row
//!   the bench logged as not exercised says so.
//! - **Budgets** are the bench's own (spec §3.1 to §3.4, logged beside each row), so nothing is compared with a
//!   budget it wasn't logged against; a Mac is none of §3.1's devices, which the report says once. §3.1's memory
//!   figure and exit test 6's crowd are the manifest's tiers'.
//! - **Verdicts** (the `verdict` module, M5 Z2): one line for each exit test and tier, by the exit run's rules,
//!   and the report fails on any that fails. On Metal they read the bench's `fight row no upload` and
//!   `fight gpu reading` lines beside its rows: the depth-only row, the frames with no upload in flight, the
//!   uploads' cost and the display's lock.

mod verdict;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::log::{Counted, Entry, Log, Timed, percentile, trace_column};
use super::manifest::{Device, Read, Run, ScaleBench, Tier, Unit};
use super::status::Status;

/// Count rows the report's counts table shows, after every row with a budget (spec §3.4's): draws, lights,
/// triangles, skinned vertices, materials.
const COUNTS: [&str; 13] = [
    "main draws",
    "draw calls",
    "draw instances",
    "triangles",
    "main triangles",
    "shadow triangles",
    "skinned instances",
    "skinned vertices",
    "lights",
    "lights dropped",
    "materials bound",
    "lod1+ draws",
    "level switches",
];

/// The streamer's and admission's rows: exit test 2.
const STREAMING: [&str; 13] = [
    "texture pool bytes",
    "bytes over budget in use",
    "bytes admitted",
    "bytes decoded waiting",
    "reads held",
    "textures in debt",
    "worst texture debt frames",
    "texture levels raised",
    "texture levels lowered",
    "streamed texture bytes",
    "streamed texture pool",
    "texture bytes wanted",
    "texture bytes raised",
];

/// The characters' rows: exit test 5's timed rows and what the frames posed and skinned.
const CAST_TIMED: [&str; 4] = ["animation and ik", "secondary motion", "gpu skinning", "gpu main"];
const CAST_COUNTED: [&str; 8] = [
    "characters animated",
    "poses evaluated",
    "skinned instances",
    "skinned vertices",
    "joints",
    "character prepass draws",
    "outline draws",
    "ambient characters in view",
];

/// The spans a fight's bench reads the thread's CPU time over (M6 B3): each wall row and its `on core` row, shown
/// side by side in a table of their own.
const SPANS: [(&str, &str); 3] =
    [("cpu frame", "cpu frame on core"), ("record", "record on core"), ("gpu wait", "gpu wait on core")];

/// The memory line's fields the report shows, in MiB, and the names it gives them.
const MEMORY: [(&str, &str); 6] = [
    ("process_mib", "Process"),
    ("textures_mib", "Textures"),
    ("resident_textures_mib", "Textures resident in the asset server"),
    ("mesh_pool_mib", "Mesh pool"),
    ("resident_mib", "Asset server, all"),
    ("heap_mib", "Heap"),
];

/// What the report reads: the manifest, the commit's folder of logs and statuses, and the baseline's folder,
/// laid out the same (the baseline's units named as the runs they stand beside).
pub struct Inputs<'a> {
    pub manifest: &'a ScaleBench,
    pub logs: &'a Path,
    pub baseline: Option<&'a Path>,
    /// What the folder's commit is called, for the title.
    pub commit: &'a str,
    pub smoke: bool,
}

/// One round's log of a unit, with the load it started at.
struct Round {
    round: u32,
    log: Log,
    load: Option<f64>,
    trace: Option<String>,
}

impl Round {
    /// The display's lock the round's GPU phase read, where it read one.
    fn lock(&self) -> Option<Lock> {
        let reading = self.log.gpu_reading.as_ref()?;
        Some(Lock { lock: reading.lock?, slow_frames: reading.slow_frames })
    }

    /// Whether the display's lock voids the round's GPU rows (the exit run's rule, M5 Z2).
    fn void(&self) -> bool {
        self.lock().is_some_and(Lock::void)
    }
}

/// A round's reading of the display's 60 Hz lock (M5 Z2): the mean resultant length of its slow frames' phases
/// against a 60 Hz period, 0 spread evenly and 1 all at one phase. Beside it (M6 B1, m6-plan §3 "Void rounds"),
/// what chance alone reads from that many frames: its floor and the chance of reading it with no lock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Lock {
    lock: f64,
    /// The frames the lock is read over; 0 in a log from before the bench printed them.
    slow_frames: u64,
}

impl Lock {
    /// The voiding rule, unchanged until B0's rounds show the significance reading agrees with it.
    fn void(self) -> bool {
        self.lock >= verdict::LOCK_VOID
    }

    /// What N slow frames spread evenly by chance read, about 1/√N (the root mean square of the mean resultant
    /// length of N uniform phases), so a short run's honest lock reads higher than a long one's.
    fn floor(self) -> Option<f64> {
        (self.slow_frames > 0).then(|| 1.0 / (self.slow_frames as f64).sqrt())
    }

    /// The chance that N phases spread evenly read a lock at least this high at one period: Rayleigh's test,
    /// with Zar's correction for small N (exact enough from the bench's least of 20 frames). The bench takes the
    /// highest lock of the periods it tries within 0.03% of 1/60 s, so the chance at any of them is somewhat
    /// higher; the run's span, which says how many of those periods are independent, isn't logged.
    fn chance(self) -> Option<f64> {
        if self.slow_frames == 0 {
            return None;
        }
        let n = self.slow_frames as f64;
        let resultant = n * self.lock.clamp(0.0, 1.0);
        let exponent = (1.0 + 4.0 * n + 4.0 * (n * n - resultant * resultant)).sqrt() - (1.0 + 2.0 * n);
        Some(exponent.exp().min(1.0))
    }

    /// "0.205 (floor 0.081 for 154 slow frames, chance 0.001 with no lock)".
    fn describe(self) -> String {
        match (self.floor(), self.chance()) {
            (Some(floor), Some(chance)) => format!(
                "{:.3} (floor {floor:.3} for {} slow frames, chance {} with no lock)",
                self.lock,
                self.slow_frames,
                chance_text(chance)
            ),
            _ => format!("{:.3}", self.lock),
        }
    }
}

/// A chance to the places it's read at: "0.26", "0.004", "< 0.001".
fn chance_text(chance: f64) -> String {
    if chance < 0.001 {
        "< 0.001".to_owned()
    } else if chance < 0.01 {
        format!("{chance:.3}")
    } else {
        format!("{chance:.2}")
    }
}

/// A run's rounds on a tier, one variant: what the tables read from, in round order.
struct Rounds {
    rounds: Vec<Round>,
}

/// A cell of a timed row, read over the rounds.
enum TimedCell {
    /// One round's own figures, and the round: the one a verdict reads.
    Round(u32, Timed),
    NotExercised,
    Missing,
    /// Every round that read the row is void by the display's lock: no figure a verdict could rest on.
    Void,
}

/// A cell of a count row, combined over rounds.
enum CountCell {
    Read(Counted),
    /// Rounds that read differently, each round's.
    Differs(Vec<(u32, Counted)>),
    NotExercised,
    Missing,
}

impl Rounds {
    /// The row as a verdict reads it: the round with the least p95, every round's on the CPU and the valid
    /// rounds' on the GPU, with that round's own p50 and worst frame, so the figures are a frame a round produced.
    fn judged(&self, phase: &str, row: &str) -> TimedCell {
        let (mut logged, mut void, mut least) = (false, false, None::<(u32, Timed)>);
        for round in &self.rounds {
            let Some(entry) = round.log.timed(phase, row) else { continue };
            logged = true;
            let Entry::Read(timed) = entry else { continue };
            if phase == "gpu" && round.void() {
                void = true;
            } else if least.is_none_or(|(_, least)| timed.p95 < least.p95) {
                least = Some((round.round, timed));
            }
        }
        match least {
            Some((round, timed)) => TimedCell::Round(round, timed),
            None if void => TimedCell::Void,
            None if logged => TimedCell::NotExercised,
            None => TimedCell::Missing,
        }
    }

    /// The diagnostic: each statistic's least over every round, void ones included and each on its own (B4's
    /// reading), with the metres of the least worst frame; `None` where no round read the row. No verdict reads it.
    fn least(&self, phase: &str, row: &str) -> Option<Timed> {
        let read: Vec<Timed> = self
            .rounds
            .iter()
            .filter_map(|round| match round.log.timed(phase, row)? {
                Entry::Read(timed) => Some(timed),
                Entry::NotExercised => None,
            })
            .collect();
        let first = read.first()?;
        let least = |pick: fn(&Timed) -> f64| read.iter().map(pick).fold(f64::INFINITY, f64::min);
        let worst = read.iter().min_by(|a, b| a.worst.total_cmp(&b.worst)).expect("a round");
        Some(Timed {
            p50: least(|t| t.p50),
            p95: least(|t| t.p95),
            p99: least(|t| t.p99),
            worst: worst.worst,
            walked: worst.walked,
            budget: first.budget,
            proposed: first.proposed,
        })
    }

    fn counted(&self, phase: &str, row: &str) -> CountCell {
        let entries: Vec<(u32, Entry<Counted>)> = self
            .rounds
            .iter()
            .filter_map(|round| Some((round.round, round.log.counted(phase, row)?)))
            .collect();
        let read: Vec<(u32, Counted)> = entries
            .iter()
            .filter_map(|(round, entry)| match entry {
                Entry::Read(counted) => Some((*round, *counted)),
                Entry::NotExercised => None,
            })
            .collect();
        match (read.first(), entries.is_empty()) {
            (None, true) => CountCell::Missing,
            (None, false) => CountCell::NotExercised,
            (Some(_), _) if read.len() != entries.len() => CountCell::Differs(read),
            (Some(&(_, first)), _) if read.iter().all(|(_, counted)| *counted == first) => {
                CountCell::Read(first)
            }
            (Some(_), _) => CountCell::Differs(read),
        }
    }

    fn highest_load(&self) -> Option<f64> {
        self.rounds
            .iter()
            .filter_map(|round| round.load)
            .fold(None, |most, load| Some(most.map_or(load, |m: f64| m.max(load))))
    }

    /// The least of the rounds' readings of a plain bench's field.
    fn field(&self, name: &str) -> Option<f64> {
        self.rounds.iter().filter_map(|round| round.log.fields.get(name).copied()).reduce(f64::min)
    }
}

/// The report, and what in it fails: counts that should be exact and moved between rounds, phases of a running
/// day that started at different hours, and units that failed.
pub fn report(inputs: &Inputs<'_>) -> (String, Vec<String>) {
    let manifest = inputs.manifest;
    let mut out = String::new();
    let mut failures = Vec::new();
    let units: Vec<Unit> = manifest.every_unit(None);
    let statuses: BTreeMap<&str, Option<Status>> =
        units.iter().map(|unit| (unit.name.as_str(), Status::read(inputs.logs, &unit.name))).collect();
    let done = statuses.values().filter(|status| status.as_ref().is_some_and(Status::complete)).count();

    let _ = writeln!(out, "# Benches at scale: {}\n", inputs.commit);
    let _ = writeln!(
        out,
        "The `{}` district{}, {} round(s) of each run. A time is one round's own, named: the round a verdict \
         reads, the least p95 of every round on the CPU and of the valid rounds on the GPU, where a round the \
         display's lock voids is left out; where each statistic's least over every round, void ones too, \
         differs from it, that is shown beside as a diagnostic, judged on nothing, since no round need have read \
         it. Each timed run's rounds table gives every round's lock beside its floor and its chance with no \
         lock. A counting run reads the same counts in every round; a timed walk's counts follow what streaming \
         landed when, so they can differ between rounds, each round's shown and a round over its budget marked. \
         {done} of {} units complete. **This is a Mac:** none of spec §3.1's devices; every figure \
         is this machine at a tier's settings, size and cook, and each phone figure is still owed to a phone. On \
         Metal passes overlap, so of the GPU rows only the frame is exact; a pass's figure is what it added, and \
         the verdicts judge the prepasses and shadows as one depth-only row and a pass on the frames with no \
         upload in flight, and void a round with the display's 60 Hz lock (the exit run's rules). A unit \
         started above a load of {} is marked where it is read. Times in ms: p50 / p95 / worst @ metres along the \
         walk, the budget in brackets; **bold is over**, and the last column says by how much. §3.4's draw \
         budget is on the main view's draw calls after culling (a run of equal draws is one call), as m5-plan §3 \
         decided: `main draws`; `draw calls` is every call the frame submitted, every pass's, beside it with no \
         budget.\n",
        manifest.preset,
        if inputs.smoke {
            " (a smoke run: 200 frames at 320×180, read for its shape, never its numbers)"
        } else {
            ""
        },
        manifest.rounds,
        units.len(),
        manifest.load_bound
    );
    if inputs.baseline.is_some() {
        let _ = writeln!(
            out,
            "**The baseline drew the phones at their scenes' size:** B4 walked Mobile-High at 1600x740 and \
             Mobile-Low at 1280x590, with nothing upscaled, where this batch draws each phone tier onto its panel \
             (the manifest's size), its scene capped and upscaled. A phone row's baseline column isn't the same \
             frame: it leaves out the upscale and the interface at the panel's size, and its levels and size \
             culling read fewer rows.\n"
        );
    }
    // Units with no status yet, by run (a batch part-way has hundreds); a unit that ran and failed by name.
    let mut not_run: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for unit in &units {
        let entry = not_run.entry(manifest.runs[unit.run].name.as_str()).or_default();
        entry.1 += 1;
        if statuses[unit.name.as_str()].is_none() {
            entry.0 += 1;
        }
    }
    let not_run: Vec<String> = not_run
        .into_iter()
        .filter(|(_, (missing, _))| *missing > 0)
        .map(|(run, (missing, all))| format!("{run} ({missing} of {all})"))
        .collect();
    if !not_run.is_empty() {
        let _ = writeln!(out, "**Not run yet**, units by run: {}.\n", not_run.join(", "));
    }
    let failed: Vec<String> = statuses
        .iter()
        .filter_map(|(name, status)| Some((name, status.as_ref()?)))
        .filter(|(_, status)| !status.complete())
        .map(|(name, status)| format!("{name}: {}", status.describe()))
        .collect();
    if !failed.is_empty() {
        let _ = writeln!(out, "**Failed:**\n");
        for failure in &failed {
            let _ = writeln!(out, "- {failure}");
        }
        let _ = writeln!(out);
        failures.extend(failed);
    }
    verdict::section(&mut out, &mut failures, inputs, &units, &statuses);
    if !manifest.gaps.is_empty() {
        let _ = writeln!(out, "**What this batch can't measure yet:**\n");
        for gap in &manifest.gaps {
            let _ = writeln!(out, "- {gap}");
        }
        let _ = writeln!(out);
    }

    for (tier_index, tier) in manifest.tiers.iter().enumerate() {
        let _ = writeln!(out, "## {}\n", title(&tier.name));
        for (run_index, run) in manifest.runs.iter().enumerate() {
            if !run.on(&tier.name) || !run.variants.is_empty() || !run.devices.is_empty() {
                continue;
            }
            let rounds = gather(inputs, &units, &statuses, run_index, tier_index, None, None);
            let beside = run.beside.as_deref().and_then(|name| baseline(inputs, manifest, name, tier));
            section(&mut out, &mut failures, manifest, run, tier, &rounds, beside.as_ref());
        }
    }

    let pairs: Vec<(usize, &Run)> =
        manifest.runs.iter().enumerate().filter(|(_, run)| !run.variants.is_empty()).collect();
    if !pairs.is_empty() {
        let _ = writeln!(out, "## Pairs\n");
        let _ = writeln!(
            out,
            "Each pair's variants alternated inside one sitting a round, the order turned each round. A time is the \
             round a verdict reads (p50 / p95, the round named), the GPU's from the valid rounds; a plain bench's \
             field, which reads no lock, the least any round read; counts p50 / worst. Each variant's rounds and \
             their locks follow its table.\n"
        );
    }
    for (run_index, run) in pairs {
        let _ = writeln!(out, "### {}: {}\n", run.name, run.about);
        if let Some(size) = &run.size {
            let _ = writeln!(out, "At {size}, its package's own size, not the tier's.\n");
        }
        let names: Vec<&str> = run.variants.iter().map(|variant| variant.name.as_str()).collect();
        let _ = writeln!(out, "| Tier | Row | {} |", names.join(" | "));
        let _ = writeln!(out, "|---|---|{}", "---|".repeat(names.len()));
        // The loads after the table, which a line between its rows would break.
        let mut notes = String::new();
        for (tier_index, tier) in manifest.tiers.iter().enumerate() {
            if !run.on(&tier.name) {
                continue;
            }
            let variants: Vec<Rounds> = (0..run.variants.len())
                .map(|variant| gather(inputs, &units, &statuses, run_index, tier_index, Some(variant), None))
                .collect();
            for row in &run.rows {
                let cells: Vec<String> = variants.iter().map(|rounds| pair_cell(rounds, row)).collect();
                let _ = writeln!(out, "| {} | {row} | {} |", title(&tier.name), cells.join(" | "));
            }
            for (variant, rounds) in run.variants.iter().zip(&variants) {
                let what = format!("{} {}", title(&tier.name), variant.name);
                load_note(&mut notes, manifest, rounds, &what);
                locks_note(&mut notes, rounds, &what);
            }
        }
        let _ = writeln!(out);
        out.push_str(&notes);
    }

    // The devices' walks (M5 Z2), each tier's runs in a section of its own, as this Mac's are.
    for (device_index, device) in manifest.devices.iter().enumerate() {
        if !manifest.runs.iter().any(|run| run.devices.contains(&device.name)) {
            continue;
        }
        for (tier_index, tier) in manifest.tiers.iter().enumerate() {
            let walks: Vec<(usize, &Run)> = manifest
                .runs
                .iter()
                .enumerate()
                .filter(|(_, run)| run.devices.contains(&device.name) && run.on(&tier.name))
                .collect();
            if walks.is_empty() {
                continue;
            }
            let _ = writeln!(out, "## {}, {}\n", capital(&device.about), title(&tier.name));
            let _ = writeln!(out, "{}\n", device_note(device));
            for (run_index, run) in walks {
                let rounds =
                    gather(inputs, &units, &statuses, run_index, tier_index, None, Some(device_index));
                section(&mut out, &mut failures, manifest, run, tier, &rounds, None);
            }
        }
    }
    (out, failures)
}

/// What a device's walk is and isn't: what its section and its verdict lines say.
fn device_note(device: &Device) -> String {
    format!(
        "`cargo xtask bench-scale device --platform {}`: the `{}` district's phone tree in the bundle beside the \
         shipped mobile cook, walked by counts. **Counts only:** no time read on {} is judged, and none is a \
         phone's.",
        device.name, device.preset, device.about
    )
}

/// Each verdict of the units that walked on device `device` (by its index) in `logs`, a line each, and a line
/// for each that failed (M5 Z2): what `bench-scale device` prints when its walks are done.
pub fn device_verdicts(manifest: &ScaleBench, logs: &Path, device: usize) -> (Vec<String>, Vec<String>) {
    let inputs = Inputs { manifest, logs, baseline: None, commit: "", smoke: false };
    let units = manifest.on_device(device, &[], None);
    let statuses: BTreeMap<&str, Option<Status>> =
        units.iter().map(|unit| (unit.name.as_str(), Status::read(logs, &unit.name))).collect();
    let mut lines = vec![device_note(&manifest.devices[device])];
    let mut failures = Vec::new();
    for row in verdict::device_rows(&inputs, &units, &statuses, device) {
        lines.push(format!("exit test {} on {}: {}{}", row.test, row.tier, row.verdict, row.why_suffix()));
        if row.failed {
            failures.push(format!("exit test {} on {}: {}", row.test, row.tier, row.why.join("; ")));
        }
    }
    (lines, failures)
}

/// A run's section on one tier: a table for each thing it's read for.
fn section(
    out: &mut String,
    failures: &mut Vec<String>,
    manifest: &ScaleBench,
    run: &Run,
    tier: &Tier,
    rounds: &Rounds,
    beside: Option<&Rounds>,
) {
    let _ = writeln!(out, "### {}: {}\n", run.name, run.about);
    if let Some(stands_in) = &run.stands_in {
        let tests: Vec<String> = run.serves.iter().map(u32::to_string).collect();
        let _ = writeln!(
            out,
            "**A stand-in, not measured as written:** {stands_in}. Exit test{} {} {} these rows as not measured, \
             never as a pass.\n",
            if tests.len() > 1 { "s" } else { "" },
            tests.join(" and "),
            if tests.len() > 1 { "read" } else { "reads" }
        );
    }
    if rounds.rounds.is_empty() {
        let _ = writeln!(out, "Not run.\n");
        return;
    }
    load_note(out, manifest, rounds, "");
    if run.timed {
        rounds_table(out, rounds);
    }
    let beside_name = run.beside.as_deref().unwrap_or("baseline");
    for read in &run.read {
        match read {
            Read::Cpu => {
                // The spans' own rows read in their table; the CPU frame stays in this one too, as the verdicts' row.
                timed_table(out, rounds, beside, beside_name, "cpu", |row| {
                    !row.starts_with("gpu ") && !row.ends_with(" on core") && row != "record"
                });
                spans_table(out, rounds);
                excluded_table(out, rounds);
            }
            Read::Gpu => {
                timed_table(out, rounds, beside, beside_name, "gpu", |row| row.starts_with("gpu "));
                copies_note(out, rounds);
            }
            Read::Counts => {
                let budgeted = budgeted_counts(rounds);
                let rows: Vec<&str> = budgeted
                    .iter()
                    .map(String::as_str)
                    .chain(COUNTS.into_iter().filter(|row| !budgeted.iter().any(|b| b == row)))
                    .collect();
                count_table(out, failures, run, tier, rounds, beside, beside_name, &rows);
            }
            Read::Streaming => {
                streaming_table(out, failures, run, tier, rounds, beside, beside_name);
                streaming_notes(out, rounds);
            }
            Read::Cast => {
                let _ = writeln!(out, "Characters (exit test 5):\n");
                let _ = writeln!(out, "| Row | Read | Over |");
                let _ = writeln!(out, "|---|---|---|");
                for row in CAST_TIMED {
                    let phase = if row.starts_with("gpu ") { "gpu" } else { "cpu" };
                    let (cell, over) = timed_text(&rounds.judged(phase, row));
                    let _ = writeln!(out, "| {} | {cell} | {over} |", title(row));
                }
                count_rows(out, failures, run, tier, rounds, None, &CAST_COUNTED, false);
                let _ = writeln!(out);
            }
            Read::Memory => memory_table(out, tier, rounds, beside, beside_name),
            Read::Trips => trips(out, rounds),
            Read::Pipelines => pipelines(out, failures, run, tier, rounds),
        }
    }
    if run.runs_the_day() {
        day_check(out, failures, run, tier, rounds);
    }
}

fn load_note(out: &mut String, manifest: &ScaleBench, rounds: &Rounds, what: &str) {
    if let Some(load) = rounds.highest_load().filter(|&load| load > f64::from(manifest.load_bound)) {
        let what = if what.is_empty() { String::new() } else { format!("{what}: ") };
        let _ = writeln!(
            out,
            "{what}**read at a load of {load:.1},** above the bound of {}: its times say little.\n",
            manifest.load_bound
        );
    }
}

/// A pair variant's rounds on one line (M6 B1), each with its lock beside its floor and its chance with no lock,
/// a void one marked; nothing where no round logged a GPU reading.
fn locks_note(out: &mut String, rounds: &Rounds, what: &str) {
    if !rounds.rounds.iter().any(|round| round.log.gpu_reading.is_some()) {
        return;
    }
    let each: Vec<String> = rounds
        .rounds
        .iter()
        .map(|round| {
            let void = if round.void() { " **void**" } else { "" };
            match (round.lock(), &round.log.gpu_reading) {
                (Some(lock), _) => format!("r{}{void} lock {}", round.round, lock.describe()),
                (None, Some(reading)) => {
                    format!("r{} lock not read ({} slow frames, too few)", round.round, reading.slow_frames)
                }
                (None, None) => format!("r{} lock not logged", round.round),
            }
        })
        .collect();
    let _ = writeln!(out, "{what}'s rounds: {}.\n", each.join("; "));
}

/// Count rows that came with a budget, in the order the bench logged them, from the phase the table reads (a
/// counting walk's `drawn` as a measured walk's `cpu`).
fn budgeted_counts(rounds: &Rounds) -> Vec<String> {
    let Some(first) = rounds.rounds.first() else { return Vec::new() };
    let read = phase_of_counts(rounds);
    first
        .log
        .counted
        .iter()
        .filter(|((phase, _), entry)| {
            phase == read && matches!(entry, Entry::Read(counted) if counted.budget.is_some())
        })
        .map(|((_, row), _)| row.clone())
        .collect()
}

fn timed_table(
    out: &mut String,
    rounds: &Rounds,
    beside: Option<&Rounds>,
    beside_name: &str,
    phase: &str,
    keep: impl Fn(&str) -> bool,
) {
    let Some(first) = rounds.rounds.first() else { return };
    let rows: Vec<&str> = first
        .log
        .timed
        .iter()
        .filter(|((p, row), _)| p == phase && keep(row))
        .map(|((_, row), _)| row.as_str())
        .collect();
    let _ = writeln!(out, "{} rows ({} phase):\n", if phase == "gpu" { "GPU" } else { "CPU" }, phase);
    if rows.is_empty() {
        let _ = writeln!(out, "missing: the log has no {phase} rows.\n");
        return;
    }
    let _ = writeln!(out, "| Row | Read | {beside_name} | Over | The least of every round (diagnostic) |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for row in rows {
        let judged = rounds.judged(phase, row);
        let (cell, over) = timed_text(&judged);
        let base =
            beside.map_or_else(|| "none".to_owned(), |beside| timed_text(&beside.judged(phase, row)).0);
        // Shown only where it reads differently from the judged round, to the places printed.
        let diagnostic = rounds.least(phase, row).map_or_else(String::new, |least| {
            let text = figures(&least, false);
            match &judged {
                TimedCell::Round(_, timed) if figures(timed, false) == text => String::new(),
                _ => text,
            }
        });
        let _ = writeln!(out, "| {} | {cell} | {base} | {over} | {diagnostic} |", title(row));
    }
    let _ = writeln!(out);
}

/// Work and waits (M6 B3): each span's wall time beside the thread's CPU time over it, read from the round its wall
/// row is judged on, so both are that round's frames; then that round's CPU frames by the kind of core they ended
/// on, each with its time on a core. Nothing where no round read the thread's CPU time (a log from before it).
fn spans_table(out: &mut String, rounds: &Rounds) {
    if !rounds.rounds.iter().any(|round| round.log.timed("cpu", SPANS[0].1).is_some()) {
        return;
    }
    let _ = writeln!(
        out,
        "Work and waits (cpu phase): each span's wall time beside the frame thread's CPU time over it, p50 / p95 / \
         worst, from the round its wall row is judged on. What the wall has over the CPU, the thread spent off its \
         core: waiting on the GPU, the encoders' workers or the driver, or set aside by the machine's other work. \
         The bench draws offscreen, so its GPU wait stands where a window's acquire would block on a GPU behind \
         it.\n"
    );
    let _ = writeln!(out, "| Span | Wall | On core | Off core at p50 |");
    let _ = writeln!(out, "|---|---|---|---|");
    let mut judged_frame = None;
    for (wall, on_core) in SPANS {
        let (cell, core, off) = match rounds.judged("cpu", wall) {
            TimedCell::Round(round, timed) => {
                if wall == SPANS[0].0 {
                    judged_frame = Some(round);
                }
                let log = &rounds.rounds.iter().find(|r| r.round == round).expect("the judged round").log;
                match log.timed("cpu", on_core) {
                    Some(Entry::Read(core)) => (
                        format!("{} (r{round})", figures(&timed, false)),
                        figures(&core, false),
                        ms((timed.p50 - core.p50).max(0.0)),
                    ),
                    Some(Entry::NotExercised) | None => (
                        format!("{} (r{round})", figures(&timed, false)),
                        "not read".to_owned(),
                        String::new(),
                    ),
                }
            }
            other => (timed_text(&other).0, String::new(), String::new()),
        };
        let _ = writeln!(out, "| {} | {cell} | {core} | {off} |", title(wall));
    }
    let _ = writeln!(out);
    let Some(round) = judged_frame.and_then(|round| rounds.rounds.iter().find(|r| r.round == round)) else {
        return;
    };
    let cores: Vec<_> = round.log.by_core.iter().filter(|((phase, _), _)| phase == "cpu").collect();
    if cores.is_empty() {
        return;
    }
    let _ =
        writeln!(out, "Its CPU frames by the kind of core they ended on (r{}), p50 / p95:\n", round.round);
    let _ = writeln!(out, "| Core | Frames | Share | CPU frame | On core |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for ((_, core), by) in cores {
        let on_core = match (by.on_core_p50, by.on_core_p95) {
            (Some(p50), Some(p95)) => format!("{} / {}", ms(p50), ms(p95)),
            _ => "not read".to_owned(),
        };
        let _ = writeln!(
            out,
            "| {} | {} | {:.1}% | {} / {} | {on_core} |",
            capital(core),
            thousands(by.samples),
            by.share * 100.0,
            ms(by.cpu_p50),
            ms(by.cpu_p95)
        );
    }
    let _ = writeln!(out);
}

/// The frames each round's phases left out of every row (M6 B3), by reason: a fight's restarts, the GPU phase's
/// frames whose timings never came back and a plain bench's warm-up, each a row with its count, its CPU frame and
/// what it allocated. Nothing where no round logged them (a log from before they were).
fn excluded_table(out: &mut String, rounds: &Rounds) {
    if !rounds.rounds.iter().any(|round| !round.log.excluded.is_empty()) {
        return;
    }
    let _ = writeln!(
        out,
        "Frames left out of the rows: a frame that starts a fight over (or a walk's tour), in the GPU phase one \
         whose timings never came back, and a plain bench's warm-up, each phase's by reason, every round. Times in \
         ms, p50 / worst, a fight's worst frame's tick and its time on the thread's core beside it; a restart's own time within its frame, which \
         exit test 4 proposes holding under 1 ms a frame (shown, judged on nothing); allocations of the whole \
         frames, all of them and the most in one.\n"
    );
    let _ = writeln!(
        out,
        "| Round | Phase | Left out for | Frames | CPU frame | On core, worst | Restart | Allocations |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|");
    for round in &rounds.rounds {
        for ((phase, reason), excluded) in &round.log.excluded {
            if excluded.frames == 0 {
                let _ = writeln!(out, "| r{} | {phase} | {reason} | none | | | | |", round.round);
                continue;
            }
            let restart = if reason == "restart" {
                format!("{} / {}", ms(excluded.restart_p50), ms(excluded.restart_worst))
            } else {
                String::new()
            };
            let _ = writeln!(
                out,
                "| r{} | {phase} | {reason} | {} | {} / {}{} | {} | {restart} | {} / {} |",
                round.round,
                thousands(excluded.frames),
                ms(excluded.cpu_p50),
                ms(excluded.cpu_worst),
                excluded.worst_tick.map_or_else(String::new, |tick| format!(" @ tick {tick}")),
                excluded.on_core_worst.map_or_else(|| "not read".to_owned(), ms),
                thousands(excluded.allocations),
                thousands(excluded.allocations_worst)
            );
        }
    }
    let _ = writeln!(out);
}

/// Each round of a timed run, a row each (M6 B1): the display's lock beside its floor for the round's slow
/// frames and the chance of reading it with no lock, whether the exit run's rule voids the round, and its GPU
/// frame where it doesn't.
fn rounds_table(out: &mut String, rounds: &Rounds) {
    let _ = writeln!(
        out,
        "Rounds: the display's 60 Hz lock voids a round at {:.2} (the exit run's rule), and a void round's GPU \
         rows are judged on nothing and read only in the diagnostic. Beside each lock, its floor, about 1/√N for \
         the N slow frames it is read over, which chance alone reads, and the chance N frames spread evenly read \
         a lock this high at one period (Rayleigh's test; the bench takes the highest of the periods it tries, so \
         the chance at any of them is somewhat higher): a reading beside the rule, not the rule (m6-plan §3, \
         \"Void rounds\").\n",
        verdict::LOCK_VOID
    );
    let _ = writeln!(out, "| Round | Lock | Floor | Chance with no lock | Gpu frame |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for round in &rounds.rounds {
        let reading = round.log.gpu_reading.as_ref();
        let (lock, floor, chance) = match (round.lock(), reading) {
            (Some(lock), _) => (
                format!("{:.3}", lock.lock),
                lock.floor().map_or_else(String::new, |floor| {
                    format!("{floor:.3} ({} slow frames)", thousands(lock.slow_frames))
                }),
                lock.chance().map_or_else(String::new, chance_text),
            ),
            (None, Some(reading)) => (
                format!("not read ({} slow frames, too few)", thousands(reading.slow_frames)),
                String::new(),
                String::new(),
            ),
            (None, None) => ("not logged".to_owned(), String::new(), String::new()),
        };
        let frame = match round.log.timed("gpu", "gpu frame") {
            _ if round.void() => "void".to_owned(),
            Some(Entry::Read(timed)) => figures(&timed, true),
            Some(Entry::NotExercised) => "not exercised".to_owned(),
            None => "missing".to_owned(),
        };
        let name =
            if round.void() { format!("r{} **void**", round.round) } else { format!("r{}", round.round) };
        let _ = writeln!(out, "| {name} | {lock} | {floor} | {chance} | {frame} |");
    }
    let _ = writeln!(out);
}

/// A timed cell's text, the round named, and its over-budget note.
fn timed_text(cell: &TimedCell) -> (String, String) {
    match cell {
        TimedCell::Missing => ("missing".to_owned(), String::new()),
        TimedCell::NotExercised => ("not exercised".to_owned(), String::new()),
        TimedCell::Void => ("every round void".to_owned(), String::new()),
        TimedCell::Round(round, timed) => {
            let over = timed.budget.map_or_else(String::new, |budget| {
                over_note(&[("p50", timed.p50), ("p95", timed.p95), ("worst", timed.worst)], budget)
            });
            (format!("{} (r{round})", figures(timed, true)), over)
        }
    }
}

/// "p50 / p95 / worst @ metres \[budget\]", each over the budget bold where `marked`: the judged round's are, the
/// diagnostic's aren't, since nothing judges them.
fn figures(timed: &Timed, marked: bool) -> String {
    let mark = |value: f64| {
        let text = ms(value);
        if marked && timed.budget.is_some_and(|budget| value > budget) { format!("**{text}**") } else { text }
    };
    let at = timed.walked.map_or_else(String::new, |metres| format!(" @ {metres:.1}"));
    let budget = timed.budget.map_or_else(String::new, |budget| format!(" [{}]", trim(budget / 1000.0)));
    format!("{} / {} / {}{at}{budget}", mark(timed.p50), mark(timed.p95), mark(timed.worst))
}

/// "p95 ×1.24, worst ×7.19" for whichever readings pass `budget`.
fn over_note(readings: &[(&str, f64)], budget: f64) -> String {
    if budget <= 0.0 {
        return String::new();
    }
    readings
        .iter()
        .filter(|(_, value)| *value > budget)
        .map(|(name, value)| format!("{name} ×{:.2}", value / budget))
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(clippy::too_many_arguments, reason = "one table's inputs, each named")]
fn count_table(
    out: &mut String,
    failures: &mut Vec<String>,
    run: &Run,
    tier: &Tier,
    rounds: &Rounds,
    beside: Option<&Rounds>,
    beside_name: &str,
    rows: &[&str],
) {
    let _ = writeln!(out, "Counts ({} phase), p50 / p95 / worst @ metres:\n", phase_of_counts(rounds));
    count_rows(out, failures, run, tier, rounds, beside.map(|beside| (beside, beside_name)), rows, true);
    let _ = writeln!(out);
}

#[allow(clippy::too_many_arguments, reason = "one table's inputs, each named")]
fn count_rows(
    out: &mut String,
    failures: &mut Vec<String>,
    run: &Run,
    tier: &Tier,
    rounds: &Rounds,
    beside: Option<(&Rounds, &str)>,
    rows: &[&str],
    header: bool,
) {
    let phase = phase_of_counts(rounds);
    if header {
        let base = beside.map_or("baseline", |(_, name)| name);
        let _ = writeln!(out, "| Row | Read | {base} | Over |");
        let _ = writeln!(out, "|---|---|---|---|");
    }
    for row in rows {
        let cell = rounds.counted(phase, row);
        if let CountCell::Differs(readings) = &cell
            && run.exact()
            && !crate::counts::REPORTED.iter().any(|(reported, _)| reported == row)
        {
            failures.push(format!(
                "{} on {}: the count \"{row}\" differs between rounds ({})",
                run.name,
                tier.name,
                readings
                    .iter()
                    .map(|(round, c)| format!("r{round} {}/{}/{}", c.p50, c.p95, c.worst))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let (text, over) = count_text(&cell);
        if header {
            let base = beside.map_or_else(
                || "none".to_owned(),
                |(beside, _)| count_text(&beside.counted(phase_of_counts(beside), row)).0,
            );
            let _ = writeln!(out, "| {} | {text} | {base} | {over} |", title(row));
        } else {
            let _ = writeln!(out, "| {} | {text} | {over} |", title(row));
        }
    }
}

/// The phase a unit's counts come from: `cpu` for a measured bench, `drawn` for a counting one.
fn phase_of_counts(rounds: &Rounds) -> &'static str {
    let counting = rounds
        .rounds
        .first()
        .is_some_and(|round| round.log.counted.iter().any(|((phase, _), _)| phase == "drawn"));
    if counting { "drawn" } else { "cpu" }
}

fn count_text(cell: &CountCell) -> (String, String) {
    match cell {
        CountCell::Missing => ("missing".to_owned(), String::new()),
        CountCell::NotExercised => ("not exercised".to_owned(), String::new()),
        CountCell::Differs(readings) => {
            // Each round's figures, marked against the budget as one round's would be, and how far the round
            // that went furthest over went: a row over in any round stays over.
            let budget = readings.iter().find_map(|(_, c)| c.budget);
            let mark = |value: u64| {
                let text = thousands(value);
                if budget.is_some_and(|budget| value > budget) { format!("**{text}**") } else { text }
            };
            let over = budget.map_or_else(String::new, |budget| {
                readings
                    .iter()
                    .max_by_key(|(_, c)| c.worst)
                    .map(|(round, c)| {
                        let note = over_note(
                            &[("p50", c.p50 as f64), ("p95", c.p95 as f64), ("worst", c.worst as f64)],
                            budget as f64,
                        );
                        if note.is_empty() { note } else { format!("{note} (r{round})") }
                    })
                    .unwrap_or_default()
            });
            (
                format!(
                    "differs between rounds: {}{}",
                    readings
                        .iter()
                        .map(|(round, c)| format!(
                            "r{round} {} / {} / {}",
                            mark(c.p50),
                            mark(c.p95),
                            mark(c.worst)
                        ))
                        .collect::<Vec<_>>()
                        .join("; "),
                    budget.map_or_else(String::new, |budget| format!(" [{}]", thousands(budget)))
                ),
                over,
            )
        }
        CountCell::Read(counted) => {
            let mark = |value: u64| {
                let text = thousands(value);
                if counted.budget.is_some_and(|budget| value > budget) { format!("**{text}**") } else { text }
            };
            let at = counted.walked_dm.map_or_else(String::new, |dm| format!(" @ {:.1}", dm as f64 / 10.0));
            let budget =
                counted.budget.map_or_else(String::new, |budget| format!(" [{}]", thousands(budget)));
            let over = counted.budget.map_or_else(String::new, |budget| {
                over_note(
                    &[
                        ("p50", counted.p50 as f64),
                        ("p95", counted.p95 as f64),
                        ("worst", counted.worst as f64),
                    ],
                    budget as f64,
                )
            });
            (
                format!(
                    "{} / {} / {}{at}{budget}",
                    mark(counted.p50),
                    mark(counted.p95),
                    mark(counted.worst)
                ),
                over,
            )
        }
    }
}

/// The process's most through either lap in any round, in MiB: what exit test 2 holds to §3.1.
fn process_most(rounds: &Rounds) -> Option<f64> {
    rounds
        .rounds
        .iter()
        .flat_map(|round| {
            LAPS.iter().filter_map(|(phase, _)| round.log.memory.get(*phase)?.fields.get("process_mib"))
        })
        .map(|values| values[1])
        .reduce(f64::max)
}

fn memory_table(out: &mut String, tier: &Tier, rounds: &Rounds, beside: Option<&Rounds>, beside_name: &str) {
    let _ = writeln!(
        out,
        "Memory through the cold lap (`first`) and the warm lap (`cpu`), MiB, start / most / end, the most of the \
         rounds, the process's most against spec §3.1. On unified memory the process's footprint holds what the GPU \
         allocates too, so it stands for RAM and VRAM together against §3.1's RAM figure; a discrete GPU's VRAM (6 \
         and 3 GB on the desktops) is owed to one:\n"
    );
    let _ = writeln!(out, "| | Cold lap | Warm lap | {beside_name} (cold) | §3.1 |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    let memory = |rounds: &Rounds, phase: &str, field: &str| -> String {
        let read: Vec<[f64; 3]> = rounds
            .rounds
            .iter()
            .filter_map(|round| round.log.memory.get(phase)?.fields.get(field).copied())
            .collect();
        if read.is_empty() {
            return "missing".to_owned();
        }
        let most = |index: usize| read.iter().map(|values| values[index]).fold(f64::MIN, f64::max);
        let budget = tier.memory_mib.filter(|_| field == "process_mib").map(f64::from);
        let peak = most(1);
        let peak = if budget.is_some_and(|budget| peak > budget) {
            format!("**{peak:.1}**")
        } else {
            format!("{peak:.1}")
        };
        format!("{:.1} / {peak} / {:.1}", most(0), most(2))
    };
    for (field, name) in MEMORY {
        let base = beside.map_or_else(|| "none".to_owned(), |beside| memory(beside, "first", field));
        let budget = match (field, tier.memory_mib) {
            ("process_mib", Some(budget)) => thousands(u64::from(budget)),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {base} | {budget} |",
            memory(rounds, "first", field),
            memory(rounds, "cpu", field)
        );
    }
    let budget = rounds.rounds.iter().find_map(|round| round.log.memory.get("first")?.texture_budget_mib);
    if let Some(budget) = budget {
        let _ = writeln!(out, "| Texture pool's budget | {budget:.1} | | | |");
    }
    let startup = |rounds: &Rounds| -> String {
        rounds
            .rounds
            .iter()
            .filter_map(|round| round.log.startup_ms)
            .reduce(f64::min)
            .map_or_else(|| "missing".to_owned(), |ms| format!("{:.2} s", ms / 1000.0))
    };
    let base = beside.map_or_else(|| "none".to_owned(), startup);
    let _ = writeln!(out, "| Load, warm (the least of the rounds) | {} | | {base} | |\n", startup(rounds));
}

/// The laps a timed walk's streaming and trips are read over (M5 Z3): the cold one, where every surface comes into
/// view from its tail and every pack is read for the first time, and the warm one after it, which finds resident
/// what the first raised. Exit test 2 reads the worse.
const LAPS: [(&str, &str); 2] = [("first", "cold lap"), ("cpu", "warm lap")];

/// Exit test 2's rows: a counting walk's one phase, or a timed walk's two laps side by side, the worse lap's
/// figure over its budget named.
#[allow(clippy::too_many_arguments, reason = "one table's inputs, each named")]
/// Whether a run is paced at the tier's rate on this tier, and so whether exit test 2's 30 frames judge its waits
/// (M5 Z2, the orchestrator's ruling).
fn pace_note(run: &Run, tier: &Tier) -> String {
    match run.pace(&tier.name) {
        Some(rate) => format!(
            "**Paced** at {rate} frames a second (`--bench-pace`), a game's frames: exit test 2's 30 frames judge its \
             waits."
        ),
        None => {
            "**Not paced:** its frames run as fast as this Mac draws them, so 30 of them aren't 30 at the \
                 tier's rate; exit test 2 shows its waits beside the verdicts and judges them on nothing."
                .to_owned()
        }
    }
}

fn streaming_table(
    out: &mut String,
    failures: &mut Vec<String>,
    run: &Run,
    tier: &Tier,
    rounds: &Rounds,
    beside: Option<&Rounds>,
    beside_name: &str,
) {
    let _ = writeln!(out, "{}\n", pace_note(run, tier));
    if phase_of_counts(rounds) == "drawn" {
        count_table(out, failures, run, tier, rounds, beside, beside_name, &STREAMING);
        return;
    }
    let _ = writeln!(
        out,
        "Streaming on both laps, p50 / p95 / worst @ metres: the cold lap (`first`), where every surface comes \
         into view from its tail, and the warm lap (`cpu`), after the first raised what it passed. Exit test 2 \
         reads the worse.\n"
    );
    let _ = writeln!(out, "| Row | Cold lap | Warm lap | {beside_name} (warm) | Over (the worse lap) |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for row in STREAMING {
        let cells = LAPS.map(|(phase, _)| rounds.counted(phase, row));
        if run.exact() {
            for cell in &cells {
                if let CountCell::Differs(_) = cell {
                    failures.push(format!(
                        "{} on {}: the count \"{row}\" differs between rounds ({})",
                        run.name,
                        tier.name,
                        count_text(cell).0
                    ));
                }
            }
        }
        let [(cold, cold_over), (warm, warm_over)] = [count_text(&cells[0]), count_text(&cells[1])];
        let (over, lap) = if cell_worst(&cells[0]) >= cell_worst(&cells[1]) {
            (cold_over, "cold")
        } else {
            (warm_over, "warm")
        };
        let over = if over.is_empty() { over } else { format!("{over} ({lap})") };
        let base = beside.map_or_else(
            || "none".to_owned(),
            |beside| count_text(&beside.counted(phase_of_counts(beside), row)).0,
        );
        let _ = writeln!(out, "| {} | {cold} | {warm} | {base} | {over} |", title(row));
    }
    let _ = writeln!(out);
}

/// A count cell's worst frame, the most of any round's: what makes one lap the worse.
fn cell_worst(cell: &CountCell) -> u64 {
    match cell {
        CountCell::Read(counted) => counted.worst,
        CountCell::Differs(readings) => readings.iter().map(|(_, counted)| counted.worst).max().unwrap_or(0),
        CountCell::NotExercised | CountCell::Missing => 0,
    }
}

/// The GPU frame's timestamps are its passes', so the streamer's copies, submitted before the frame's first
/// pass, aren't in it. What a copy costs shows, if anywhere, in the wait past the GPU frame: this prints it on the
/// `gpu` lap's frames that raised a texture level against those that raised none, the rounds pooled, from the
/// trace (none where the manifest takes none).
fn copies_note(out: &mut String, rounds: &Rounds) {
    let (mut raised, mut none) = (Vec::new(), Vec::new());
    for trace in rounds.rounds.iter().filter_map(|round| round.trace.as_deref()) {
        let (Some(wait), Some(span), Some(levels)) = (
            trace_column(trace, "gpu", "wait"),
            trace_column(trace, "gpu", "gpu_span"),
            trace_column(trace, "gpu", "levels_raised"),
        ) else {
            continue;
        };
        if wait.len() != span.len() || wait.len() != levels.len() {
            continue;
        }
        for ((wait, span), levels) in wait.iter().zip(&span).zip(&levels) {
            if *levels > 0.0 { &mut raised } else { &mut none }.push(wait - span);
        }
    }
    let spread = |gaps: &[f64]| match (percentile(gaps, 0.5), percentile(gaps, 0.95)) {
        (Some(p50), Some(p95)) => format!("p50 {p50:.0} / p95 {p95:.0} µs ({} frames)", gaps.len()),
        _ => "no frames".to_owned(),
    };
    let _ = writeln!(
        out,
        "- **The GPU frame leaves out the streamer's copies:** they are submitted before the frame's first pass, \
         outside its timestamps. The wait past the GPU frame (the `gpu` lap, the rounds pooled), where a copy's \
         cost would show: on frames that raised a level: {}; on frames that raised none: {}.\n",
        if raised.is_empty() && none.is_empty() { "no trace".to_owned() } else { spread(&raised) },
        if raised.is_empty() && none.is_empty() { "no trace".to_owned() } else { spread(&none) },
    );
}

fn streaming_notes(out: &mut String, rounds: &Rounds) {
    // Each lap's column: the worst frame of any round, which bounds every surface's wait, and the worst of the
    // rounds' 99th percentiles of frames, which bounds nothing and stands beside it, named as that. Then the
    // worse lap, by its worst.
    let lap = |phase: &str| {
        rounds
            .rounds
            .iter()
            .filter_map(|round| {
                let column = trace_column(round.trace.as_deref()?, phase, "worst_texture_debt")?;
                let worst = column.iter().copied().reduce(f64::max)?;
                Some((worst, percentile(&column, 0.99)?))
            })
            .reduce(|(worst, p99), (other, other_p99)| (worst.max(other), p99.max(other_p99)))
    };
    let bound = LAPS
        .iter()
        .filter_map(|&(phase, name)| Some((lap(phase)?, name)))
        .reduce(|worse, other| if other.0.0 > worse.0.0 { other } else { worse });
    let _ = writeln!(
        out,
        "- **A surface newly in view at its level within 30 frames (exit test 2):** {}. A conservative bound, not \
         the row itself: the trace's column is each frame's most frames any texture has been in debt, so its worst \
         frame bounds every surface's wait and the exit test's 99th percentile of surfaces with it; a percentile \
         of frames bounds no surface's wait. A finer counter is built only if the bound fails.",
        match bound {
            Some(((worst, p99), lap)) => format!(
                "{} (the column's worst frame in any round, on the {lap}, the worse); the frames' 99th \
                 percentile {p99:.0}, the worst of the rounds'",
                if worst > 30.0 {
                    format!("**{worst:.0} frames**, over 30")
                } else {
                    format!("{worst:.0} frames")
                }
            ),
            None => "no trace (the run takes none): the worst texture debt frames row's worst bounds it"
                .to_owned(),
        }
    );
    let _ = writeln!(
        out,
        "- **Nothing loaded on the frame's thread:** the `frame thread loads` row (budget 0, `not exercised` at 0), \
         and the waits for a texture's level each phase's `texture waits` line holds; the verdicts judge both."
    );
    let _ = writeln!(
        out,
        "- **Arrivals held to their bound** (exit test 2 fails on any above 0): {}.\n",
        held_arrivals(rounds)
    );
}

/// Each round's and phase's `arrival_held_to_bound` (M5 Z2), bold above 0; "not printed" where its reasons line
/// is from before the field, which is not 0.
fn held_arrivals(rounds: &Rounds) -> String {
    let read: Vec<String> = rounds
        .rounds
        .iter()
        .flat_map(|round| {
            round.log.wait_reasons.iter().map(move |(phase, reasons)| match reasons.arrival_held_to_bound {
                Some(0) => format!("r{} {phase} 0", round.round),
                Some(held) => format!("r{} {phase} **{held}**", round.round),
                None => format!("r{} {phase} not printed", round.round),
            })
        })
        .collect();
    if read.is_empty() { "no texture wait reasons line".to_owned() } else { read.join(", ") }
}

fn trips(out: &mut String, rounds: &Rounds) {
    let _ = writeln!(
        out,
        "Trips on both laps, frames and ms from the exit: the cold lap reads its packs for the first time, the warm \
         lap again.\n"
    );
    let mut any = false;
    for (phase, lap) in LAPS {
        for round in &rounds.rounds {
            for (index, trip) in round.log.trips.get(phase).into_iter().flatten().enumerate() {
                any = true;
                let steps: Vec<String> = ["covered", "swapped", "first_cells", "clear"]
                    .iter()
                    .map(|step| format!("{step} {}", trip.steps.get(*step).map_or("missing", String::as_str)))
                    .collect();
                let p99 = trip.frame_p99_us.map_or_else(|| "missing".to_owned(), ms);
                let _ = writeln!(
                    out,
                    "- {lap} r{} trip {index}: {}; frame p99 {p99}",
                    round.round,
                    steps.join(", ")
                );
            }
            let floor = round.trace.as_deref().and_then(|trace| {
                let capacity = trace_column(trace, phase, "mesh_pool_capacity")?;
                let floor = trace_column(trace, phase, "mesh_pool_floor")?;
                Some((*capacity.last()?, *floor.last()?))
            });
            match floor {
                Some((capacity, floor)) => {
                    let back = if capacity <= floor { "back at its floor" } else { "**above its floor**" };
                    let _ = writeln!(
                        out,
                        "- {lap} r{}: at the lap's last frame the mesh pool is {back} ({} B against {} B)",
                        round.round,
                        thousands(capacity as u64),
                        thousands(floor as u64)
                    );
                }
                None => {
                    let _ =
                        writeln!(out, "- {lap} r{}: the mesh pool's floor: missing (no trace)", round.round);
                }
            }
        }
    }
    if !any {
        let _ = writeln!(out, "- no trip on either lap: missing");
    }
    let _ = writeln!(out);
}

fn pipelines(out: &mut String, failures: &mut Vec<String>, run: &Run, tier: &Tier, rounds: &Rounds) {
    let mut lines = Vec::new();
    for round in &rounds.rounds {
        for (phase, built) in &round.log.pipelines {
            lines.push(format!(
                "r{} {phase}: first use {}, on the frame's thread {}, frames waiting {}",
                round.round, built.first_use, built.frame_thread, built.frame_waits
            ));
            if built.first_use + built.frame_thread + built.frame_waits > 0 {
                failures.push(format!(
                    "{} on {}: r{} {phase} built a pipeline after its load",
                    run.name, tier.name, round.round
                ));
            }
        }
    }
    let _ = writeln!(out, "Pipelines built after the load (exit test 7, by counts):\n");
    if lines.is_empty() {
        // A fight's bench logs a phase line and a counting walk its `zone walk` line; neither is missing.
        let _ = writeln!(out, "- missing: no round logged its pipelines\n");
    } else {
        for line in lines {
            let _ = writeln!(out, "- {line}");
        }
        let _ = writeln!(out);
    }
}

fn day_check(out: &mut String, failures: &mut Vec<String>, run: &Run, tier: &Tier, rounds: &Rounds) {
    let _ = writeln!(out, "The running day (each phase must start the tour at one hour):\n");
    for round in &rounds.rounds {
        let hours: Vec<(String, f64)> = round
            .log
            .pipelines
            .iter()
            .filter_map(|(phase, built)| Some((phase.clone(), built.first_hour?)))
            .collect();
        let text: Vec<String> = hours.iter().map(|(phase, hour)| format!("{phase} {hour:.4}")).collect();
        let same = hours.windows(2).all(|pair| (pair[0].1 - pair[1].1).abs() < 1e-3);
        let _ = writeln!(
            out,
            "- r{}: each phase's first frame at hour {}{}",
            round.round,
            text.join(", "),
            if same { "" } else { ": **not one hour**" }
        );
        if !same || hours.is_empty() {
            failures.push(format!(
                "{} on {}: r{} the day's phases started at different hours ({})",
                run.name,
                tier.name,
                round.round,
                text.join(", ")
            ));
        }
    }
    let _ = writeln!(out);
}

/// A pair's cell: the row as a timed row (gpu, then cpu phase), a count row, or a plain bench's field.
fn pair_cell(rounds: &Rounds, row: &str) -> String {
    if rounds.rounds.is_empty() {
        return "not run".to_owned();
    }
    for phase in ["gpu", "cpu"] {
        match rounds.judged(phase, row) {
            TimedCell::Round(round, timed) => {
                return format!("{} / {} (r{round})", ms(timed.p50), ms(timed.p95));
            }
            TimedCell::NotExercised => return "not exercised".to_owned(),
            TimedCell::Void => return "every round void".to_owned(),
            TimedCell::Missing => {}
        }
    }
    match rounds.counted(phase_of_counts(rounds), row) {
        CountCell::Read(counted) => {
            return format!("{} / {}", thousands(counted.p50), thousands(counted.worst));
        }
        cell @ (CountCell::Differs(_) | CountCell::NotExercised) => return count_text(&cell).0,
        CountCell::Missing => {}
    }
    match rounds.field(row) {
        Some(value) if row.contains("_us") => ms(value),
        Some(value) => trim(value),
        None => "missing".to_owned(),
    }
}

/// A run's rounds on a tier, one variant, on this Mac or one device: each round whose unit completed.
fn gather(
    inputs: &Inputs<'_>,
    units: &[Unit],
    statuses: &BTreeMap<&str, Option<Status>>,
    run: usize,
    tier: usize,
    variant: Option<usize>,
    device: Option<usize>,
) -> Rounds {
    let mut rounds = Vec::new();
    for unit in units.iter().filter(|unit| {
        unit.run == run && unit.tier == tier && unit.variant == variant && unit.device == device
    }) {
        let Some(Some(status)) = statuses.get(unit.name.as_str()) else { continue };
        if !status.complete() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(inputs.logs.join(format!("{}.log", unit.name))) else {
            continue;
        };
        let trace = std::fs::read_to_string(inputs.logs.join(format!("{}.csv", unit.name))).ok();
        rounds.push(Round { round: unit.round, log: Log::parse(&text), load: status.load, trace });
    }
    rounds.sort_by_key(|round| round.round);
    Rounds { rounds }
}

/// The baseline's rounds of the run `name` on `tier`: its folder's `<name>.<tier>.r<n>.log`, every round there.
fn baseline(inputs: &Inputs<'_>, manifest: &ScaleBench, name: &str, tier: &Tier) -> Option<Rounds> {
    let folder = inputs.baseline?;
    let rounds: Vec<Round> = (1..=manifest.rounds.max(4))
        .filter_map(|round| {
            let text =
                std::fs::read_to_string(folder.join(format!("{name}.{}.r{round}.log", tier.name))).ok()?;
            Some(Round { round, log: Log::parse(&text), load: None, trace: None })
        })
        .collect();
    (!rounds.is_empty()).then_some(Rounds { rounds })
}

/// µs as ms, to B4's places: three under 10, two under 100, one past.
fn ms(micros: f64) -> String {
    let ms = micros / 1000.0;
    if ms < 10.0 {
        format!("{ms:.3}")
    } else if ms < 100.0 {
        format!("{ms:.2}")
    } else {
        format!("{ms:.1}")
    }
}

/// A number without trailing zeros: 4.9, 12, 0.5.
fn trim(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// "desktop-high" as "Desktop-High", "cpu frame" as "Cpu frame".
fn title(name: &str) -> String {
    if name.contains('-') && !name.contains(' ') {
        return name.split('-').map(capital).collect::<Vec<_>>().join("-");
    }
    capital(name)
}

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/scale/fixtures")
    }

    fn manifest() -> ScaleBench {
        ScaleBench::parse(&std::fs::read_to_string(fixtures().join("scale.ron")).unwrap()).unwrap()
    }

    fn generate(baseline: bool) -> (String, Vec<String>) {
        let manifest = manifest();
        let (logs, base) = (fixtures(), fixtures().join("baseline"));
        report(&Inputs {
            manifest: &manifest,
            logs: &logs,
            baseline: baseline.then_some(base.as_path()),
            commit: "fixture",
            smoke: false,
        })
    }

    /// A run's rounds from each round's log text, the first round 1.
    fn rounds(logs: &[&str]) -> Rounds {
        Rounds {
            rounds: (1..)
                .zip(logs)
                .map(|(round, text)| Round { round, log: Log::parse(text), load: None, trace: None })
                .collect(),
        }
    }

    /// A count row's log line in `phase`, as the sandbox writes it.
    fn count_line(phase: &str, row: &str, [p50, p95, worst]: [u64; 3], budget: Option<u64>) -> String {
        let budget = budget.map_or_else(|| "None".to_owned(), |budget| format!("Some({budget})"));
        format!(
            "2026-11-15T10:00:00.000000Z  INFO sapphire::sandbox: count row phase=\"{phase}\" row=\"{row}\" \
             frames=100 p50={p50} p95={p95} worst={worst} total=1000 worst_tick=0 worst_walked_m=\"1.0\" \
             budget={budget} over_p50=false over_worst=false"
        )
    }

    /// §3.4's draw budget is on the main view's calls after culling (m5-plan §3), and a counting walk's table
    /// leads with it as a measured walk's does; the frame's calls stand beside it with none.
    #[test]
    fn the_draw_budget_is_the_main_views_calls() {
        let (text, _) = generate(false);
        assert!(
            text.contains("the main view's draw calls after culling (a run of equal draws is one call)"),
            "the header says which row: {text}"
        );
        // A phone's counting walk, by the paused batch's main pass on Mobile-High.
        let log = [
            count_line("drawn", "draw calls", [2_482, 4_229, 5_400], None),
            count_line("drawn", "main draws", [1_090, 1_965, 2_299], Some(700)),
        ]
        .join("\n");
        assert_eq!(budgeted_counts(&rounds(&[&log])), ["main draws"]);
    }

    /// Exit test 2's rows, the 30-frame line and the trips read a timed walk's cold lap as well as its warm one:
    /// the cold lap, where every surface comes into view from its tail, was dropped for the warm lap after it.
    #[test]
    fn a_timed_walks_streaming_and_trips_read_both_laps() {
        let manifest = manifest();
        let (run, tier) = (&manifest.runs[0], &manifest.tiers[0]);
        assert!(run.read.contains(&Read::Streaming), "the fixture's tour reads streaming");
        let log = [
            count_line("first", "worst texture debt frames", [3, 20, 45], None),
            count_line("cpu", "worst texture debt frames", [0, 1, 2], None),
            "2026-11-15T10:00:00.000000Z  INFO sapphire::sandbox: zone trip, from its exit phase=\"first\" \
             covered=\"3 frames 40.0 ms\" frame_p99_us=900.0"
                .to_owned(),
        ]
        .join("\n");
        let mut walk = rounds(&[&log]);
        walk.rounds[0].trace =
            Some("phase,frame,worst_texture_debt\nfirst,0,50\nfirst,1,50\ncpu,0,1\ncpu,1,1\n".into());
        let mut out = String::new();
        section(&mut out, &mut Vec::new(), &manifest, run, tier, &walk, None);
        let line = out.lines().find(|line| line.starts_with("| Worst texture debt frames |")).expect(&out);
        assert!(line.contains("3 / 20 / 45") && line.contains("0 / 1 / 2"), "both laps: {line}");
        assert!(out.contains("**50 frames**") && out.contains("cold lap"), "the 30-frame line's lap: {out}");
        let mut trips_text = String::new();
        trips(&mut trips_text, &walk);
        assert!(trips_text.contains("cold lap r1 trip 0: covered 3 frames 40.0 ms"), "{trips_text}");
    }

    /// The trace's column is each frame's longest debt, not a surface's wait: its worst bounds every surface's
    /// wait, and its 99th percentile of frames bounds nothing. The bound is the worst; the percentile stands
    /// beside it, named as the frames'.
    #[test]
    fn the_30_frame_line_bounds_by_the_columns_worst() {
        let manifest = manifest();
        let (run, tier) = (&manifest.runs[0], &manifest.tiers[0]);
        let mut trace = "phase,frame,worst_texture_debt\n".to_owned();
        for frame in 0..199 {
            let _ = writeln!(trace, "cpu,{frame},5");
        }
        trace.push_str("cpu,199,60\nfirst,0,4\n");
        let mut walk = rounds(&[&count_line("cpu", "worst texture debt frames", [0, 1, 2], None)]);
        walk.rounds[0].trace = Some(trace);
        let mut out = String::new();
        section(&mut out, &mut Vec::new(), &manifest, run, tier, &walk, None);
        let line = out.lines().find(|line| line.contains("within 30 frames (exit test 2")).expect(&out);
        assert!(line.contains("**60 frames**, over 30"), "the column's worst is the bound: {line}");
        assert!(line.contains("the frames' 99th percentile 5"), "the percentile beside it, named: {line}");
        assert!(line.contains("warm lap"), "the worse lap: {line}");
    }

    /// The streaming section shows each round's arrivals held to their bound beside the waits (M5 Z2), bold above
    /// 0, and "not printed" for a reasons line from before the field.
    #[test]
    fn the_waits_show_the_arrivals_held_to_their_bound() {
        let manifest = manifest();
        let (run, tier) = (&manifest.runs[0], &manifest.tiers[0]);
        let line = |phase: &str, held: &str| {
            format!(
                "x INFO sapphire::sandbox: texture wait reasons decode=1 phase=\"{phase}\" tails_over_pool=0 \
                 arrived_at_update=41{held}\n"
            )
        };
        let walk = rounds(&[
            &format!(
                "{}{}",
                line("first", " arrival_held_to_bound=0"),
                line("cpu", " arrival_held_to_bound=1")
            ),
            &line("first", ""),
        ]);
        let mut out = String::new();
        section(&mut out, &mut Vec::new(), &manifest, run, tier, &walk, None);
        let held = out.lines().find(|line| line.contains("Arrivals held to their bound")).expect(&out);
        assert!(held.ends_with("r1 cpu **1**, r1 first 0, r2 first not printed."), "{held}");
    }

    /// A timed walk's counts move between rounds; a row over its budget in any round stays marked, with how far
    /// in the round that went furthest, and the header doesn't claim every count reads the same.
    #[test]
    fn a_count_that_differs_between_rounds_keeps_its_over_budget_mark() {
        let counted =
            |p50, worst| Counted { p50, p95: p50, worst, total: None, walked_dm: None, budget: Some(700) };
        let (text, over) =
            count_text(&CountCell::Differs(vec![(1, counted(650, 690)), (2, counted(660, 900))]));
        assert!(text.contains("r2 660 / 660 / **900**"), "the round over is bold: {text}");
        assert!(text.contains("[700]"), "the budget is shown: {text}");
        assert_eq!(over, "worst ×1.29 (r2)", "how far, and in which round");
        let (header, _) = generate(false);
        assert!(!header.contains("each count the same in every round"), "{header}");
        assert!(header.contains("A counting run reads the same counts in every round"), "{header}");
    }

    /// The GPU frame's timestamps leave out the streamer's copies; the report says so and prints the wait past
    /// the GPU frame on frames that raised a level against frames that raised none, which is where a copy's cost
    /// would show.
    #[test]
    fn the_gpu_table_says_it_leaves_out_the_streamers_copies() {
        let manifest = manifest();
        let (run, tier) = (&manifest.runs[0], &manifest.tiers[0]);
        assert!(run.read.contains(&Read::Gpu), "the fixture's tour reads the GPU");
        let mut walk = rounds(&[""]);
        walk.rounds[0].trace = Some(
            "phase,frame,wait,gpu_span,levels_raised\ngpu,0,1000,400,2\ngpu,1,1000,400,1\ngpu,2,1100,400,1\n\
             gpu,3,900,400,0\ngpu,4,900,400,0\ngpu,5,950,400,0\ncpu,0,5000,0,9\n"
                .into(),
        );
        let mut out = String::new();
        section(&mut out, &mut Vec::new(), &manifest, run, tier, &walk, None);
        let line = out.lines().find(|line| line.contains("streamer's copies")).expect(&out);
        assert!(line.contains("raised a level: p50 600 / p95 700 µs (3 frames)"), "{line}");
        assert!(line.contains("raised none: p50 500 / p95 550 µs (3 frames)"), "{line}");
    }

    /// B4's baseline drew the phones at their scenes' size; the report says so once, beside a baseline.
    #[test]
    fn a_baseline_says_the_phones_were_drawn_at_their_scenes_size() {
        let (with, _) = generate(true);
        assert_eq!(with.matches("Mobile-High at 1600x740 and Mobile-Low at 1280x590").count(), 1, "{with}");
        let (without, _) = generate(false);
        assert!(!without.contains("1600x740"), "no baseline, no note");
    }

    #[test]
    fn a_row_over_budget_is_marked_with_how_far() {
        let (text, _) = generate(true);
        // Desktop-Low's GPU frame: round 1's p95 15.20 against 14, the lesser of the rounds', with round 1's own
        // p50 and worst frame (M6 B1).
        let line = text.lines().find(|line| line.starts_with("| Gpu frame |")).expect(&text);
        assert!(line.contains("| 8.400 / **15.20** / **85.00** @ 312.0 [14] (r1) |"), "{line}");
        assert!(line.contains("p95 ×1.09, worst ×6.07"), "{line}");
        // The baseline beside it.
        assert!(line.contains("| 7.900 / 11.67 / **55.73** @ 312.0 [14] (r1) |"), "{line}");
        // Each statistic's least over the rounds, round 2's p50 and worst beside round 1's p95: a diagnostic,
        // unmarked, since no round read that frame.
        assert!(line.ends_with("| 8.021 / 15.20 / 79.97 @ 312.0 [14] |"), "{line}");
        let calls = text.lines().find(|line| line.starts_with("| Draw calls |")).unwrap();
        assert!(calls.contains("**7,135**") && calls.contains("p95 ×2.85"), "{calls}");
    }

    #[test]
    fn a_missing_row_is_reported_not_zero() {
        let (text, _) = generate(true);
        let line = text.lines().find(|line| line.starts_with("| Skinned vertices |")).unwrap();
        assert!(line.contains("| missing |"), "{line}");
        let simulation = text.lines().find(|line| line.starts_with("| Simulation |")).unwrap();
        assert!(simulation.contains("not exercised"), "{simulation}");
        assert!(text.contains("**Not run yet**, units by run: "), "the units with no log are counted");
    }

    #[test]
    fn counts_that_move_between_rounds_are_shown_and_fail_where_they_must_be_exact() {
        let (text, failures) = generate(false);
        let line = text.lines().find(|line| line.starts_with("| Triangles |")).unwrap();
        assert!(line.contains("differs between rounds: r1 7,275,723"), "{line}");
        assert!(
            failures.iter().any(
                |failure| failure.contains("counted on desktop-low") && failure.contains("\"triangles\"")
            ),
            "{failures:?}"
        );
        assert!(
            !failures.iter().any(|failure| failure.contains("tour on")),
            "a timed run's counts are marked only"
        );
    }

    #[test]
    fn a_stand_in_says_it_isnt_the_row_and_a_running_day_checks_its_hours() {
        let (text, failures) = generate(false);
        assert!(
            text.contains("**A stand-in, not measured as written:** a party of three kinds against twelve")
        );
        assert!(
            text.contains("each phase's first frame at hour cpu 19.7667, first 19.7667, gpu 19.7667"),
            "{text}"
        );
        assert!(!failures.iter().any(|failure| failure.contains("different hours")), "{failures:?}");
        assert!(
            text.contains("the `frame thread loads` row"),
            "the frame thread's loads are read from their row"
        );
        // The fixture's warm lap reads 41 frames and its cold lap 90: the bound is the cold lap's (M5 Z3, F3).
        assert!(text.contains("90 frames**, over 30"), "the debt bound from the trace: {text}");
        assert!(text.contains("on the cold lap, the worse"), "the lap is named: {text}");
    }

    /// A fight round's log (M6 B1): its CPU and GPU frames and its GPU reading, `lock` as the bench prints it
    /// (`-` for too few slow frames) or `None` for a log with no reading.
    fn fight_round(cpu_p95: f64, gpu: [f64; 3], lock: Option<(&str, u32)>) -> String {
        let row = |phase: &str, row: &str, [p50, p95, worst]: [f64; 3], budget: u32| {
            format!(
                "x INFO sapphire::sandbox: fight row phase=\"{phase}\" row=\"{row}\" samples=100 p50_us={p50:.1} \
                 p95_us={p95:.1} p99_us={worst:.1} worst_us={worst:.1} worst_tick=1 budget_us=Some({budget}) \
                 proposed_us=None over_p50=false over_worst=false\n"
            )
        };
        let reading = lock.map_or_else(String::new, |(lock, slow)| {
            format!(
                "x INFO sapphire::sandbox: fight gpu reading phase=\"gpu\" backend=\"metal\" overlaps=true \
                 timed_frames=1000 no_upload_frames=800 in_flight_pct=20.0 taa_slow_in_flight_pct=0.0 \
                 taa_slow_no_upload_pct=0.0 post_slow_in_flight_pct=0.0 post_slow_no_upload_pct=0.0 \
                 slow_frames={slow} lock={lock} lock_period_us=16667.0\n"
            )
        });
        format!(
            "{}{}{reading}",
            row("cpu", "cpu frame", [500.0, cpu_p95, 2_000.0], 22_000),
            row("gpu", "gpu frame", gpu, 26_000)
        )
    }

    /// The lock's floor is about 1/√N for N slow frames, and its chance with no lock is Rayleigh's: near
    /// e^(−N·lock²), 1 at no lock, and small far above the floor. The rule still voids at 0.10.
    #[test]
    fn a_lock_reads_beside_its_floor_and_its_chance() {
        let lock = |lock, slow_frames| Lock { lock, slow_frames };
        assert_eq!(lock(0.05, 100).floor(), Some(0.1));
        assert_eq!(lock(0.05, 0).floor(), None, "a log from before the slow frames were printed");
        assert!((lock(0.0, 100).chance().unwrap() - 1.0).abs() < 1e-9);
        // Desktop-High's district-fight at f4efa213e: round 2 valid at 0.094, round 1 void at 0.205, 154 slow
        // frames each.
        let valid = lock(0.094, 154).chance().unwrap();
        assert!((valid - (-154.0 * 0.094_f64.powi(2)).exp()).abs() < 0.01, "{valid}");
        assert!(lock(0.205, 154).chance().unwrap() < 0.002);
        // A phone's short unit: 0.126 over 120 frames is void by the rule, and chance reads it one time in seven.
        let phone = lock(0.126, 120);
        assert!(phone.void() && (0.14..0.16).contains(&phone.chance().unwrap()));
        assert!(!lock(0.099, 120).void() && lock(0.10, 120).void());
        assert_eq!(
            lock(0.205, 154).describe(),
            "0.205 (floor 0.081 for 154 slow frames, chance 0.001 with no lock)"
        );
    }

    /// Each timed run's rounds table has a row a round (M6 B1): its lock, the lock's floor and chance, a void
    /// round marked and its GPU frame withheld, a lock too few frames read and a log with no reading said so.
    #[test]
    fn the_rounds_table_has_a_row_a_round_with_its_lock() {
        let fight = rounds(&[
            &fight_round(1_400.0, [3_000.0, 13_010.0, 14_000.0], Some(("0.205", 154))),
            &fight_round(1_450.0, [3_100.0, 12_030.0, 13_360.0], Some(("0.094", 154))),
            &fight_round(1_450.0, [3_100.0, 12_500.0, 13_360.0], Some(("-", 12))),
            &fight_round(1_450.0, [3_100.0, 12_500.0, 13_360.0], None),
        ]);
        let mut out = String::new();
        rounds_table(&mut out, &fight);
        let lines: Vec<&str> = out.lines().filter(|line| line.starts_with("| r")).collect();
        assert_eq!(
            lines,
            [
                "| r1 **void** | 0.205 | 0.081 (154 slow frames) | 0.001 | void |",
                "| r2 | 0.094 | 0.081 (154 slow frames) | 0.26 | 3.100 / 12.03 / 13.36 [26] |",
                "| r3 | not read (12 slow frames, too few) |  |  | 3.100 / 12.50 / 13.36 [26] |",
                "| r4 | not logged |  |  | 3.100 / 12.50 / 13.36 [26] |",
            ],
            "{out}"
        );
        let mut note = String::new();
        locks_note(&mut note, &fight, "Desktop-High on");
        assert!(note.starts_with("Desktop-High on's rounds: r1 **void** lock 0.205 (floor 0.081"), "{note}");
        assert!(note.contains("r3 lock not read (12 slow frames, too few); r4 lock not logged."), "{note}");
    }

    /// A void round's GPU figures are no figure a verdict could rest on (M6 B1): a GPU row reads the valid round,
    /// whole, and the least of every round, which takes the void round's lesser p95, shows beside it only as the
    /// diagnostic. A CPU row reads every round, the display's lock voiding none. Mobile-Low's district-fight at
    /// f4efa213e: round 1 valid at 3.65 ms, round 2 void at 3.63.
    #[test]
    fn a_void_round_shows_only_in_the_diagnostic() {
        let fight = rounds(&[
            &fight_round(1_480.0, [2_900.0, 3_650.0, 4_100.0], Some(("0.041", 160))),
            &fight_round(1_470.0, [2_950.0, 3_631.0, 3_900.0], Some(("0.126", 120))),
        ]);
        let table = |phase: &str| {
            let mut out = String::new();
            timed_table(&mut out, &fight, None, "baseline", phase, |_| true);
            out
        };
        let gpu = table("gpu");
        let frame = gpu.lines().find(|line| line.starts_with("| Gpu frame |")).expect(&gpu);
        assert_eq!(
            frame, "| Gpu frame | 2.900 / 3.650 / 4.100 [26] (r1) | none |  | 2.900 / 3.631 / 3.900 [26] |",
            "the valid round read, the void round's 3.63 only as the diagnostic"
        );
        assert!(gpu.contains("| The least of every round (diagnostic) |"), "{gpu}");
        let cpu = table("cpu");
        let frame = cpu.lines().find(|line| line.starts_with("| Cpu frame |")).expect(&cpu);
        assert_eq!(
            frame, "| Cpu frame | 0.500 / 1.470 / 2.000 [22] (r2) | none |  |  |",
            "the void round read on the CPU, and the least of every round no other frame"
        );
        // Every round void: no figure, and the diagnostic still shown.
        let void = rounds(&[&fight_round(1_480.0, [2_900.0, 3_650.0, 4_100.0], Some(("0.300", 160)))]);
        assert!(matches!(void.judged("gpu", "gpu frame"), TimedCell::Void));
        assert!(matches!(void.judged("cpu", "cpu frame"), TimedCell::Round(1, _)));
        assert_eq!(pair_cell(&void, "gpu frame"), "every round void");
        assert_eq!(pair_cell(&fight, "gpu frame"), "2.900 / 3.650 (r1)");
    }

    #[test]
    fn a_pair_reads_each_variant_beside_the_other() {
        let (text, _) = generate(false);
        let line = text.lines().find(|line| line.starts_with("| Desktop-Low | gpu main |")).expect(&text);
        assert_eq!(line, "| Desktop-Low | gpu main | 2.786 / 4.905 (r1) | 3.100 / 5.200 (r1) |");
        assert!(text.contains("**read at a load of 9.5,**"), "a unit above the bound is marked");
    }

    /// An unbudgeted CPU row's line, as the sandbox writes it.
    fn cpu_line(row: &str, [p50, p95, worst]: [f64; 3]) -> String {
        format!(
            "x INFO sapphire::sandbox: fight row phase=\"cpu\" row=\"{row}\" samples=100 p50_us={p50:.1} \
             p95_us={p95:.1} p99_us={worst:.1} worst_us={worst:.1} worst_tick=1 budget_us=None proposed_us=None \
             over_p50=false over_worst=false\n"
        )
    }

    /// Each span's wall time stands beside the thread's CPU time over it, both from the round the wall row is
    /// judged on, and that round's frames by core with theirs (M6 B3); the spans' own rows leave the CPU table.
    #[test]
    fn a_spans_wall_time_reads_beside_its_time_on_a_core() {
        let round = |cpu_p95: f64, on_core_p50: f64| {
            [
                cpu_line("cpu frame", [1_000.0, cpu_p95, 9_000.0]),
                cpu_line("gpu wait", [3_000.0, 4_000.0, 6_000.0]),
                cpu_line("cpu frame on core", [on_core_p50, 1_000.0, 2_000.0]),
                cpu_line("record", [400.0, 600.0, 6_660.0]),
                cpu_line("record on core", [350.0, 500.0, 900.0]),
                cpu_line("gpu wait on core", [20.0, 40.0, 80.0]),
                "x INFO sapphire::sandbox: frames by core phase=\"cpu\" core=\"performance\" samples=980 share=0.980 \
                 cpu_us_p50=\"990.0\" cpu_us_p95=\"1400.0\" cpu_us_worst=\"9000.0\" on_core_us_p50=\"900.0\" \
                 on_core_us_p95=\"1000.0\"\n"
                    .to_owned(),
            ]
            .concat()
        };
        let fight = rounds(&[&round(1_500.0, 700.0), &round(1_400.0, 800.0)]);
        let mut out = String::new();
        spans_table(&mut out, &fight);
        let line =
            |span: &str| out.lines().find(|line| line.starts_with(&format!("| {span} |"))).expect(&out);
        // Round 2 has the lesser p95, so both figures are its own.
        assert_eq!(
            line("Cpu frame"),
            "| Cpu frame | 1.000 / 1.400 / 9.000 (r2) | 0.800 / 1.000 / 2.000 | 0.200 |"
        );
        assert_eq!(line("Record"), "| Record | 0.400 / 0.600 / 6.660 (r1) | 0.350 / 0.500 / 0.900 | 0.050 |");
        assert_eq!(
            line("Gpu wait"),
            "| Gpu wait | 3.000 / 4.000 / 6.000 (r1) | 0.020 / 0.040 / 0.080 | 2.980 |"
        );
        assert!(out.contains("| Performance | 980 | 98.0% | 0.990 / 1.400 | 0.900 / 1.000 |"), "{out}");
        assert!(out.contains("CPU frames by the kind of core they ended on (r2)"), "{out}");
        let mut table = String::new();
        timed_table(&mut table, &fight, None, "baseline", "cpu", |row| {
            !row.starts_with("gpu ") && !row.ends_with(" on core") && row != "record"
        });
        assert!(
            table.contains("| Cpu frame |") && !table.contains("On core") && !table.contains("| Record |")
        );
        // A log from before the thread's CPU time was read prints no such table.
        let mut none = String::new();
        spans_table(&mut none, &rounds(&[&fight_round(1_400.0, [3_000.0, 12_000.0, 13_000.0], None)]));
        assert!(none.is_empty(), "{none}");
    }

    /// The frames a phase left out are rows of their own, every round's, by reason (M6 B3): the restart with its
    /// own time, a reason with no frames said as none, and nothing at all from a log from before them.
    #[test]
    fn the_frames_left_out_are_rows_of_their_own() {
        let log = "x INFO sapphire::sandbox: fight excluded phase=\"cpu\" reason=\"restart\" frames=2 \
                   cpu_us_p50=\"8100.0\" cpu_us_worst=\"9200.0\" worst_tick=1 on_core_us_worst=\"8900.0\" \
                   restart_us_p50=\"7850.0\" restart_us_worst=\"7880.0\" allocations=22200 allocations_worst=11100 \
                   bytes=900000 step_allocations=0\n\
                   x INFO sapphire::sandbox: fight excluded phase=\"gpu\" reason=\"untimed\" frames=0\n\
                   x INFO sapphire::sandbox: bench excluded phase=\"cpu\" reason=\"warm-up\" frames=60 \
                   cpu_us_p50=\"1200.0\" cpu_us_worst=\"45000.0\" worst_frame=0 allocations=6000 \
                   allocations_worst=5000\n";
        let mut out = String::new();
        excluded_table(&mut out, &rounds(&[log]));
        assert!(
            out.contains(
                "| r1 | cpu | restart | 2 | 8.100 / 9.200 @ tick 1 | 8.900 | 7.850 / 7.880 | 22,200 / 11,100 |"
            ),
            "{out}"
        );
        assert!(out.contains("| r1 | gpu | untimed | none |"), "{out}");
        assert!(
            out.contains("| r1 | cpu | warm-up | 60 | 1.200 / 45.00 | not read |  | 6,000 / 5,000 |"),
            "a plain bench's warm-up, which has no tick: {out}"
        );
        let mut none = String::new();
        excluded_table(&mut none, &rounds(&[&fight_round(1_400.0, [3_000.0, 12_000.0, 13_000.0], None)]));
        assert!(none.is_empty(), "{none}");
    }
}
