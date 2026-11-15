//! The batch's report (M5 Z1): a commit's logs turned into the Markdown spec §3.8 takes, each row beside its
//! budget and beside the baseline's, with what is over said plainly.
//!
//! - **Times** are the lesser of the rounds (B4's reading): each of p50, p95 and p99 the least any round read,
//!   and the worst the least of the rounds' worst frames, with the metres walked where it fell, so a point on
//!   the walk that is the worst in every round keeps its name. A unit read above the manifest's load bound is
//!   marked, never dropped.
//! - **Counts** of a counting run are exact: every round must read the same. A count that differs is shown with
//!   each round's, each marked against its budget, and on a run of counts alone that isn't streaming it fails
//!   the report, as the scale lane would; a timed walk's may differ, and a round over its budget stays marked.
//! - **Nothing is zero that wasn't read:** a row a run should have and no round logged says `missing`, and a row
//!   the bench logged as not exercised says so.
//! - **Budgets** are the bench's own (spec §3.1 to §3.4, logged beside each row), so nothing is compared with a
//!   budget it wasn't logged against; a Mac is none of §3.1's devices, which the report says once.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::log::{Counted, Entry, Log, Timed, percentile, trace_column};
use super::manifest::{Read, Run, ScaleBench, Tier, Unit};
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

/// A run's rounds on a tier, one variant: what the tables read from.
struct Rounds {
    rounds: Vec<Round>,
}

/// A cell of a timed row, combined over rounds.
enum TimedCell {
    Read(Timed),
    NotExercised,
    Missing,
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
    fn timed(&self, phase: &str, row: &str) -> TimedCell {
        let entries: Vec<Entry<Timed>> =
            self.rounds.iter().filter_map(|round| round.log.timed(phase, row)).collect();
        let read: Vec<Timed> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Read(timed) => Some(*timed),
                Entry::NotExercised => None,
            })
            .collect();
        let Some(first) = read.first() else {
            return if entries.is_empty() { TimedCell::Missing } else { TimedCell::NotExercised };
        };
        let least = |pick: fn(&Timed) -> f64| read.iter().map(pick).fold(f64::INFINITY, f64::min);
        let worst = read.iter().min_by(|a, b| a.worst.total_cmp(&b.worst)).expect("a round");
        TimedCell::Read(Timed {
            p50: least(|t| t.p50),
            p95: least(|t| t.p95),
            p99: least(|t| t.p99),
            worst: worst.worst,
            walked: worst.walked,
            budget: first.budget,
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
    let units: Vec<Unit> = manifest.units(None).into_iter().chain(manifest.counting(None)).collect();
    let statuses: BTreeMap<&str, Option<Status>> =
        units.iter().map(|unit| (unit.name.as_str(), Status::read(inputs.logs, &unit.name))).collect();
    let done = statuses.values().filter(|status| status.as_ref().is_some_and(Status::complete)).count();

    let _ = writeln!(out, "# Benches at scale: {}\n", inputs.commit);
    let _ = writeln!(
        out,
        "The `{}` district{}, {} round(s) of each run, each time the lesser of the rounds and a counting run's \
         counts the same in every round; a timed walk's counts follow what streaming landed when, so they can \
         differ between rounds, each round's shown and a round over its budget marked. {done} of {} units \
         complete. **This is a Mac:** none of spec §3.1's devices; every figure \
         is this machine at a tier's settings, size and cook, and each phone figure is still owed to a phone. On \
         Metal passes overlap, so of the GPU rows only the frame is exact; a pass's figure is what it added. A unit \
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
            if !run.on(&tier.name) || !run.variants.is_empty() {
                continue;
            }
            let rounds = gather(inputs, &units, &statuses, run_index, tier_index, None);
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
            "Each pair's variants alternated inside one sitting a round, the order turned each round. Times the lesser \
             of the rounds (p50 / p95), counts p50 / worst.\n"
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
                .map(|variant| gather(inputs, &units, &statuses, run_index, tier_index, Some(variant)))
                .collect();
            for row in &run.rows {
                let cells: Vec<String> = variants.iter().map(|rounds| pair_cell(rounds, row)).collect();
                let _ = writeln!(out, "| {} | {row} | {} |", title(&tier.name), cells.join(" | "));
            }
            for (variant, rounds) in run.variants.iter().zip(&variants) {
                load_note(&mut notes, manifest, rounds, &format!("{} {}", title(&tier.name), variant.name));
            }
        }
        let _ = writeln!(out);
        out.push_str(&notes);
    }
    (out, failures)
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
    let beside_name = run.beside.as_deref().unwrap_or("baseline");
    for read in &run.read {
        match read {
            Read::Cpu => timed_table(out, rounds, beside, beside_name, "cpu", |row| !row.starts_with("gpu ")),
            Read::Gpu => timed_table(out, rounds, beside, beside_name, "gpu", |row| row.starts_with("gpu ")),
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
                    let (cell, over) = timed_text(&rounds.timed(phase, row));
                    let _ = writeln!(out, "| {} | {cell} | {over} |", title(row));
                }
                count_rows(out, failures, run, tier, rounds, None, &CAST_COUNTED, false);
                let _ = writeln!(out);
            }
            Read::Memory => memory_table(out, rounds, beside, beside_name),
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
    let _ = writeln!(out, "| Row | Read | {beside_name} | Over |");
    let _ = writeln!(out, "|---|---|---|---|");
    for row in rows {
        let (cell, over) = timed_text(&rounds.timed(phase, row));
        let base = beside.map_or_else(|| "none".to_owned(), |beside| timed_text(&beside.timed(phase, row)).0);
        let _ = writeln!(out, "| {} | {cell} | {base} | {over} |", title(row));
    }
    let _ = writeln!(out);
}

/// A timed cell's text and its over-budget note.
fn timed_text(cell: &TimedCell) -> (String, String) {
    match cell {
        TimedCell::Missing => ("missing".to_owned(), String::new()),
        TimedCell::NotExercised => ("not exercised".to_owned(), String::new()),
        TimedCell::Read(timed) => {
            let mark = |value: f64| {
                let text = ms(value);
                if timed.budget.is_some_and(|budget| value > budget) { format!("**{text}**") } else { text }
            };
            let at = timed.walked.map_or_else(String::new, |metres| format!(" @ {metres:.1}"));
            let budget =
                timed.budget.map_or_else(String::new, |budget| format!(" [{}]", trim(budget / 1000.0)));
            let over = timed.budget.map_or_else(String::new, |budget| {
                over_note(&[("p50", timed.p50), ("p95", timed.p95), ("worst", timed.worst)], budget)
            });
            (format!("{} / {} / {}{at}{budget}", mark(timed.p50), mark(timed.p95), mark(timed.worst)), over)
        }
    }
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

fn memory_table(out: &mut String, rounds: &Rounds, beside: Option<&Rounds>, beside_name: &str) {
    let _ = writeln!(
        out,
        "Memory through the cold walk (`first` phase), MiB, start / most / end, the most of the rounds:\n"
    );
    let _ = writeln!(out, "| | Read | {beside_name} |");
    let _ = writeln!(out, "|---|---|---|");
    let memory = |rounds: &Rounds, field: &str| -> String {
        let read: Vec<[f64; 3]> = rounds
            .rounds
            .iter()
            .filter_map(|round| round.log.memory.get("first")?.fields.get(field).copied())
            .collect();
        if read.is_empty() {
            return "missing".to_owned();
        }
        let most = |index: usize| read.iter().map(|values| values[index]).fold(f64::MIN, f64::max);
        format!("{:.1} / {:.1} / {:.1}", most(0), most(1), most(2))
    };
    for (field, name) in MEMORY {
        let base = beside.map_or_else(|| "none".to_owned(), |beside| memory(beside, field));
        let _ = writeln!(out, "| {name} | {} | {base} |", memory(rounds, field));
    }
    let budget = rounds.rounds.iter().find_map(|round| round.log.memory.get("first")?.texture_budget_mib);
    if let Some(budget) = budget {
        let _ = writeln!(out, "| Texture pool's budget | {budget:.1} | |");
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
    let _ = writeln!(out, "| Load, warm (the least of the rounds) | {} | {base} |\n", startup(rounds));
}

/// The laps a timed walk's streaming and trips are read over (M5 Z3): the cold one, where every surface comes into
/// view from its tail and every pack is read for the first time, and the warm one after it, which finds resident
/// what the first raised. Exit test 2 reads the worse.
const LAPS: [(&str, &str); 2] = [("first", "cold lap"), ("cpu", "warm lap")];

/// Exit test 2's rows: a counting walk's one phase, or a timed walk's two laps side by side, the worse lap's
/// figure over its budget named.
#[allow(clippy::too_many_arguments, reason = "one table's inputs, each named")]
fn streaming_table(
    out: &mut String,
    failures: &mut Vec<String>,
    run: &Run,
    tier: &Tier,
    rounds: &Rounds,
    beside: Option<&Rounds>,
    beside_name: &str,
) {
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
            None =>
                "no trace (a counting walk writes none): the worst texture debt frames row's worst bounds it"
                    .to_owned(),
        }
    );
    let _ = writeln!(
        out,
        "- **Nothing loaded on the frame's thread:** by construction (reads and decodes run on the asset server's \
         workers), not counted.\n"
    );
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
        // A counting bench fails its own run on any build after the load, and logs no phase line.
        let _ = writeln!(
            out,
            "- no phase line: a counting bench exits non-zero on any pipeline built after its load, so a complete unit built none\n"
        );
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
        match rounds.timed(phase, row) {
            TimedCell::Read(timed) => return format!("{} / {}", ms(timed.p50), ms(timed.p95)),
            TimedCell::NotExercised => return "not exercised".to_owned(),
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

/// A run's rounds on a tier, one variant: each round whose unit completed.
fn gather(
    inputs: &Inputs<'_>,
    units: &[Unit],
    statuses: &BTreeMap<&str, Option<Status>>,
    run: usize,
    tier: usize,
    variant: Option<usize>,
) -> Rounds {
    let mut rounds = Vec::new();
    for unit in units.iter().filter(|unit| unit.run == run && unit.tier == tier && unit.variant == variant) {
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
        assert!(header.contains("a counting run's counts the same in every round"), "{header}");
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
        // Desktop-Low's GPU frame: p95 15.20 against 14, worst 79.97.
        let line = text.lines().find(|line| line.starts_with("| Gpu frame |")).expect(&text);
        assert!(line.contains("**15.20**") && line.contains("**79.97** @ 312.0 [14]"), "{line}");
        assert!(line.contains("p95 ×1.09, worst ×5.71"), "{line}");
        // The lesser of the rounds: round 2 read p50 8.021, round 1 8.400.
        assert!(line.contains("| 8.021 /"), "{line}");
        // The baseline beside it.
        assert!(line.contains("| 7.900 / 11.67 / **55.73** @ 312.0 [14] |"), "{line}");
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
        assert!(text.contains("by construction"), "the frame thread's loads are named as uncounted");
        // The fixture's warm lap reads 41 frames and its cold lap 90: the bound is the cold lap's (M5 Z3, F3).
        assert!(text.contains("90 frames**, over 30"), "the debt bound from the trace: {text}");
        assert!(text.contains("on the cold lap, the worse"), "the lap is named: {text}");
    }

    #[test]
    fn a_pair_reads_each_variant_beside_the_other() {
        let (text, _) = generate(false);
        let line = text.lines().find(|line| line.starts_with("| Desktop-Low | gpu main |")).expect(&text);
        assert_eq!(line, "| Desktop-Low | gpu main | 2.786 / 4.905 | 3.100 / 5.200 |");
        assert!(text.contains("**read at a load of 9.5,**"), "a unit above the bound is marked");
    }
}
