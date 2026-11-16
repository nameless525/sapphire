//! The verdicts (M5 Z2): one line for each of m5-plan §1's exit tests and each tier, by the exit run's rules
//! (`docs/m5-exit-run.md`), from the runs the manifest says serve each test. A verdict is a pass, a fail with
//! what failed, or "not judged" with why: a run not made yet, a stand-in, a figure the manifest doesn't hold, or a
//! test the batch doesn't measure (goldens, GPU tests, recipes). The report fails on any fail.
//!
//! - **A timed row** passes on the least of the rounds' 95th percentiles at or under its budget (the plan's "runs
//!   alternated and the least kept"); a worst frame over it fails nothing. A row with only a proposed figure
//!   (M5 B4's Desktop-High and Mobile-High columns of §3.3) is shown against it, marked proposed, and fails
//!   nothing: adopting a column is moving its figure into the bench's budget, data the sandbox logs. On a tier
//!   whose pass rows carry no budget (no adopted column), the verdict's why says so and names the rows shown.
//! - **On a backend that overlaps its passes (Metal, M5 Z2)** the prepasses and shadows are judged as one
//!   depth-only row, each frame's sum against the sum of their figures, and shown apart, unjudged; a GPU pass row
//!   is read on the frames with no upload in flight (the bench's `fight row no upload`), and a round where fewer
//!   than a tenth of the timed frames have none judges no pass row. The frame row and the CPU rows read every
//!   frame. Elsewhere every row is judged as logged.
//! - **A round with the display's 60 Hz lock is void** (M5 Z2): its GPU rows are left out of the least of the
//!   rounds and shown as void with the lock's figure, and a row with every round void isn't judged.
//! - **A count** fails on any round's worst frame over its budget.
//! - **A wait for a texture's level**, on a run paced at the tier's rate (M5 Z2: an unpaced run's waits are shown
//!   with its frame time and judged on nothing), fails on a phase's 99th percentile over 30 frames (an `N+` bin is over), a
//!   wait open at a run's end for 30 frames or more, the pool's too, or no line; its reasons line on the counting
//!   walk fails on the pool ever too small for the tails or a zone that never arrived.
//! - **Exit test 6** (M5 Z2, the orchestrator's ruling) judges the crowd on: a variant whose arguments turn it off
//!   is shown beside, and a plain bench, which its log says holds the camera and logs no fight row, is read in its
//!   own table; both are judged on nothing. A fight bench that logs no rows still fails.
//! - **Pairs** (runs serving exit test 3, each package's on against off) are costs to read beside each other, so
//!   no verdict reads them.
//! - **A device's walk** (a run naming `devices`, M5 Z2) has lines of its own, a tier and a device each, by the
//!   same rules; a tier it hasn't walked is "not run". This Mac's lines never read it.

use std::fmt::Write as _;

use super::{CountCell, Rounds, TimedCell, gather, ms, thousands, title};
use crate::scale::log::{Entry, WaitFrames, Waits};
use crate::scale::manifest::{Read, Run, Tier, Unit};
use crate::scale::status::Status;

/// What a check found on one run: what failed, what it couldn't judge, what it shows without judging.
#[derive(Debug, Default)]
pub(super) struct Findings {
    pub failed: Vec<String>,
    pub unjudged: Vec<String>,
    pub notes: Vec<String>,
    /// How many checks passed or failed, as opposed to went unjudged.
    pub judged: usize,
    /// The timed rows logged with no budget on a tier with no adopted column (M5 B4's Desktop-High and
    /// Mobile-High), each with whether it carries a proposed figure: shown, failing nothing, and said in the
    /// verdict's why.
    pub unadopted: std::collections::BTreeMap<String, bool>,
}

impl Findings {
    fn fail(&mut self, what: String) {
        self.judged += 1;
        self.failed.push(what);
    }

    fn pass(&mut self) {
        self.judged += 1;
    }

    fn absorb(&mut self, other: Self) {
        self.failed.extend(other.failed);
        self.unjudged.extend(other.unjudged);
        self.notes.extend(other.notes);
        self.judged += other.judged;
        self.unadopted.extend(other.unadopted);
    }
}

/// An exit test's verdict on a tier.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    Pass,
    Fail,
    NotJudged,
}

impl Verdict {
    fn of(findings: &Findings) -> Self {
        if !findings.failed.is_empty() {
            Self::Fail
        } else if !findings.unjudged.is_empty() || findings.judged == 0 {
            Self::NotJudged
        } else {
            Self::Pass
        }
    }

    fn text(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "**fail**",
            Self::NotJudged => "not judged here",
        }
    }
}

/// What each exit test's verdict reads, and what it leaves to the runbook's other steps.
const TESTS: [(u32, &str, &str); 8] = [
    (
        1,
        "the counted walks' main draws and lights against §3.4, and every timed row against §3.2 and §3.3",
        "",
    ),
    (
        2,
        "the pools and the admission cap, the waits for a texture's level, the frame's thread's loads and the \
         process against §3.1",
        "",
    ),
    (3, "", "goldens, the on-and-off GPU tests and the gate at each switch: the runbook's steps"),
    (
        4,
        "",
        "goldens on three backends, each feature's tier behaviour, cost, recipe and off test: the runbook's",
    ),
    (5, "the cast's rows", "the cast's rows test, the conversation's and cloth's tests: the runbook's"),
    (
        6,
        "the crowd's count and the animation row with it",
        "the replays' checksums with the crowd on and off: the runbook's",
    ),
    (
        7,
        "every pipeline count after the load",
        "`zero_hitches` plays the ward, not the district (a ruling): the counting walks and B5's fight test stand \
         in; the fight test is the runbook's",
    ),
    (8, "", "the recipes, walked: the runbook's"),
];

/// The verdicts section, and a failure for each verdict that failed.
pub(super) fn section(
    out: &mut String,
    failures: &mut Vec<String>,
    inputs: &super::Inputs<'_>,
    units: &[Unit],
    statuses: &std::collections::BTreeMap<&str, Option<Status>>,
) {
    let manifest = inputs.manifest;
    let _ = writeln!(out, "## Verdicts\n");
    let _ = writeln!(
        out,
        "One line for each exit test and tier, by the exit run's rules (docs/m5-exit-run.md): a timed row on the \
         least of the rounds' p95, a count on every round's worst, a wait for a texture's level on each phase's \
         p99 and the longest still open. A figure marked *proposed* (M5 B4's Desktop-High and Mobile-High columns) \
         is shown and fails nothing. The report fails on any **fail**.\n"
    );
    for deviation in &manifest.deviations {
        let _ = writeln!(out, "**Deviation:** {deviation}\n");
    }
    let _ = writeln!(out, "| Exit test | Tier | Verdict | Judged here | Why |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    let mut beside = Vec::new();
    for (test, judged_here, elsewhere) in TESTS {
        for (tier_index, tier) in manifest.tiers.iter().enumerate() {
            let mut findings = Findings::default();
            if judged_here.is_empty() {
                findings.unjudged.push(elsewhere.to_owned());
            } else {
                for (run_index, run) in manifest.runs.iter().enumerate() {
                    if !run.serves.contains(&test)
                        || run.serves.contains(&3)
                        || !run.on(&tier.name)
                        || !run.devices.is_empty()
                    {
                        continue;
                    }
                    let variants: Vec<Option<usize>> = if run.variants.is_empty() {
                        vec![None]
                    } else {
                        (0..run.variants.len()).map(Some).collect()
                    };
                    for variant in variants {
                        let name = match variant {
                            Some(index) => format!("{} ({})", run.name, run.variants[index].name),
                            None => run.name.clone(),
                        };
                        if test == 6 && variant.is_some_and(|index| crowd_off(&run.variants[index].args)) {
                            // The crowd off is what the crowd on is read against (M5 Z2, the orchestrator's
                            // ruling): its arguments, not its name, say so.
                            findings.notes.push(format!(
                                "{name}: the crowd off, shown beside the crowd on, judged on nothing by exit test 6"
                            ));
                            continue;
                        }
                        let rounds = gather(inputs, units, statuses, run_index, tier_index, variant, None);
                        findings.absorb(judge(test, run, &name, tier, &rounds));
                    }
                }
            }
            let verdict = Verdict::of(&findings);
            if verdict == Verdict::Fail {
                failures.extend(
                    findings.failed.iter().map(|what| format!("exit test {test} on {}: {what}", tier.name)),
                );
            }
            let mut why: Vec<String> = findings.failed.clone();
            if verdict == Verdict::NotJudged && findings.judged > 0 {
                // A part-way batch: what it judged so far held, and the rest says why the verdict waits.
                let checks = if findings.judged == 1 { "check" } else { "checks" };
                why.push(format!("{} {checks} held so far", findings.judged));
            }
            if verdict != Verdict::Fail {
                why.extend(no_column(&findings, tier));
                why.extend(findings.unjudged.iter().cloned());
            }
            if verdict == Verdict::Pass && !elsewhere.is_empty() {
                why.push(format!("outside the batch: {elsewhere}"));
            }
            beside.extend(
                findings.notes.iter().map(|note| format!("exit test {test}, {}: {note}", title(&tier.name))),
            );
            let _ = writeln!(
                out,
                "| {test} | {} | {} | {} | {} |",
                title(&tier.name),
                verdict.text(),
                if judged_here.is_empty() { "nothing" } else { judged_here },
                if why.is_empty() { String::new() } else { short(&why) }
            );
        }
        for device in 0..manifest.devices.len() {
            for row in device_rows(inputs, units, statuses, device).into_iter().filter(|row| row.test == test)
            {
                if row.failed {
                    failures.extend(
                        row.why.iter().map(|what| format!("exit test {test} on {}: {what}", row.tier)),
                    );
                }
                let _ = writeln!(
                    out,
                    "| {test} | {} | {} | {judged_here} | {} |",
                    row.tier,
                    if row.failed { "**fail**" } else { row.verdict },
                    short(&row.why)
                );
            }
        }
    }
    let _ = writeln!(out);
    if !beside.is_empty() {
        let _ = writeln!(out, "Shown beside the verdicts, judged on nothing:\n");
        for note in beside {
            let _ = writeln!(out, "- {note}");
        }
        let _ = writeln!(out);
    }
}

/// A device's verdict on one exit test and tier (M5 Z2): its own line beside this Mac's.
pub(super) struct DeviceRow {
    pub test: u32,
    /// The tier and the device: "Mobile-High, the iOS Simulator".
    pub tier: String,
    /// "pass", "fail", "not run", or "not judged here".
    pub verdict: &'static str,
    pub failed: bool,
    /// What failed, or why it wasn't judged.
    pub why: Vec<String>,
}

impl DeviceRow {
    /// Its reasons in brackets after the verdict, if it has any.
    pub(super) fn why_suffix(&self) -> String {
        if self.why.is_empty() { String::new() } else { format!(" ({})", short(&self.why)) }
    }
}

/// Device `device`'s verdicts (by its index): one for each exit test a run on it serves and each tier it walks,
/// by the same rules as this Mac's, from its own units. A tier with no unit complete is "not run".
pub(super) fn device_rows(
    inputs: &super::Inputs<'_>,
    units: &[Unit],
    statuses: &std::collections::BTreeMap<&str, Option<Status>>,
    device: usize,
) -> Vec<DeviceRow> {
    let manifest = inputs.manifest;
    let about = &manifest.devices[device].about;
    let mut rows = Vec::new();
    for (test, judged_here, _) in TESTS {
        if judged_here.is_empty() {
            continue;
        }
        for (tier_index, tier) in manifest.tiers.iter().enumerate() {
            let walks: Vec<(usize, &Run)> = manifest
                .runs
                .iter()
                .enumerate()
                .filter(|(_, run)| {
                    run.devices.contains(&manifest.devices[device].name)
                        && run.serves.contains(&test)
                        && run.on(&tier.name)
                })
                .collect();
            if walks.is_empty() {
                continue;
            }
            let mut findings = Findings::default();
            let mut ran = false;
            for (run_index, run) in walks {
                let rounds = gather(inputs, units, statuses, run_index, tier_index, None, Some(device));
                ran |= !rounds.rounds.is_empty();
                findings.absorb(judge(test, run, &run.name, tier, &rounds));
            }
            let verdict = Verdict::of(&findings);
            let (text, why) = match (ran, &verdict) {
                (false, _) => ("not run", Vec::new()),
                (true, Verdict::Fail) => ("fail", findings.failed),
                (true, Verdict::Pass) => ("pass", Vec::new()),
                (true, Verdict::NotJudged) => (
                    "not judged here",
                    no_column(&findings, tier).into_iter().chain(findings.unjudged).collect(),
                ),
            };
            rows.push(DeviceRow {
                test,
                tier: format!("{}, {about}", title(&tier.name)),
                verdict: text,
                failed: verdict == Verdict::Fail,
                why,
            });
        }
    }
    rows
}

/// Whether a variant's arguments turn the crowd off (`--ambient off`).
fn crowd_off(args: &[String]) -> bool {
    args.windows(2).any(|pair| pair[0] == "--ambient" && pair[1] == "off")
}

/// Why a tier with no adopted column judges its timed rows on nothing (M5 Z2, the runbook's "not judged (no
/// column)"): which rows are shown beside M5 B4's proposed figures and which with none. None where every row the
/// verdict read has a budget.
fn no_column(findings: &Findings, tier: &Tier) -> Option<String> {
    if findings.unadopted.is_empty() {
        return None;
    }
    let named = |proposed: bool| {
        let rows: Vec<&str> = findings
            .unadopted
            .iter()
            .filter(|&(_, &has)| has == proposed)
            .map(|(row, _)| row.as_str())
            .collect();
        match rows.len() {
            0 => None,
            1..=3 => Some(format!("{} ({})", rows.len(), rows.join(", "))),
            n => Some(n.to_string()),
        }
    };
    let mut parts = Vec::new();
    if let Some(rows) = named(true) {
        parts.push(format!("{rows} beside M5 B4's proposed figures"));
    }
    if let Some(rows) = named(false) {
        parts.push(format!("{rows} with none"));
    }
    Some(format!(
        "{} has no adopted column, so its rows are shown and fail nothing (the runbook's \"not judged (no \
         column)\"): {}",
        title(&tier.name),
        parts.join(", ")
    ))
}

/// The first few of a verdict's reasons, and how many more.
fn short(why: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut text = why.iter().take(SHOWN).cloned().collect::<Vec<_>>().join("; ");
    if why.len() > SHOWN {
        let _ = write!(text, "; and {} more", why.len() - SHOWN);
    }
    text.replace('|', "/")
}

/// Exit test `test`'s checks on one run's rounds (one variant's) on `tier`.
pub(super) fn judge(test: u32, run: &Run, name: &str, tier: &Tier, rounds: &Rounds) -> Findings {
    let mut findings = Findings::default();
    if rounds.rounds.is_empty() {
        findings.unjudged.push(format!("{name} not run"));
        return findings;
    }
    if test == 2 {
        // On every run, a stand-in too: a trip's arrival is where a hold reaches its bound.
        arrivals(&mut findings, name, rounds);
    }
    if let Some(stands_in) = &run.stands_in {
        // Read beside the runs that judge the test, never a pass of what it stands in for (M5 Z2): the trip's
        // floor is judged where the counting walk closes the district.
        findings.notes.push(format!("{name} stands in for {stands_in}: read beside, judged on nothing"));
        return findings;
    }
    match test {
        1 => {
            if run.timed {
                timed_rows(&mut findings, name, rounds, |_| true);
            } else {
                counts(&mut findings, name, rounds, &["drawn"], &["main draws", "lights"]);
            }
        }
        2 => {
            let laps: &[&str] = if run.timed { &["first", "cpu"] } else { &["drawn"] };
            if run.read.contains(&Read::Streaming) {
                counts(
                    &mut findings,
                    name,
                    rounds,
                    laps,
                    &[
                        "bytes over budget in use",
                        "bytes decoded waiting",
                        "streamed texture bytes",
                        "frame thread loads",
                    ],
                );
                waits(&mut findings, name, rounds, run.pace(&tier.name));
                if !run.timed {
                    floor(&mut findings, name, rounds);
                }
            }
            if run.read.contains(&Read::Memory) {
                memory(&mut findings, name, tier, rounds);
            }
        }
        5 => timed_rows(&mut findings, name, rounds, |row| {
            [
                "animation and ik",
                "secondary motion",
                "gpu skinning",
                "gpu main",
                "gpu prepasses",
                "gpu depth only",
            ]
            .contains(&row)
        }),
        6 => {
            let plain = rounds.rounds.iter().all(|round| round.log.plain_bench && round.log.timed.is_empty());
            if run.timed && plain {
                // A held bench (M5 Z2, the orchestrator's ruling): its log, not its name, says it is one, and a
                // fight bench that logs no rows still fails below.
                findings.notes.push(format!(
                    "{name}: a plain bench, which logs no fight rows, read in its own table and judged on nothing \
                     by exit test 6"
                ));
            } else if run.timed {
                timed_rows(&mut findings, name, rounds, |row| row == "animation and ik");
            } else {
                crowd(&mut findings, name, tier, rounds);
            }
        }
        7 => pipelines(&mut findings, name, rounds),
        _ => {}
    }
    findings
}

/// The display's lock (`fight gpu reading`'s `lock`) at or over which a round is void (M5 Z2). Quiet sittings on
/// an M5 read 0.03 and 0.07; sittings with a video playing beside them 0.14 to 0.43, with 13 to 24% of their
/// frames with no upload in flight slow in TAA against none quiet.
pub(super) const LOCK_VOID: f64 = 0.10;

/// The rows judged as one depth-only row where the backend overlaps its passes, and the row they're judged as.
const DEPTH_ROWS: [&str; 2] = ["gpu prepasses", "gpu shadows"];
const DEPTH_ONLY: &str = "gpu depth only";

/// Every timed row `keep` takes, in the `cpu` and `gpu` phases: the least round's p95 against its budget, by the
/// rules the module's doc gives for a backend that overlaps its passes and a void round.
fn timed_rows(findings: &mut Findings, name: &str, rounds: &Rounds, keep: impl Fn(&str) -> bool) {
    let Some(first) = rounds.rounds.first() else { return };
    let rows: Vec<(String, String)> = first
        .log
        .timed
        .iter()
        .filter(|((phase, row), _)| (phase == "cpu" || phase == "gpu") && keep(row))
        .map(|(key, _)| key.clone())
        .collect();
    if rows.is_empty() {
        findings.fail(format!("{name}: no timed row logged"));
        return;
    }
    // A tier's column is adopted when its pass rows carry budgets (M5 B4): Desktop-High's and Mobile-High's carry
    // only proposed figures, so a row with no budget there is one the runbook leaves unjudged ("no column").
    let figures: Vec<(bool, bool)> = first
        .log
        .timed
        .iter()
        .filter(|((phase, row), _)| phase == "gpu" && row != "gpu frame")
        .filter_map(|(_, entry)| match entry {
            Entry::Read(timed) => Some((timed.budget.is_some(), timed.proposed.is_some())),
            _ => None,
        })
        .collect();
    let no_column =
        figures.iter().any(|&(_, proposed)| proposed) && figures.iter().all(|&(budget, _)| !budget);
    let unadopted = |findings: &mut Findings, row: &str, budget: Option<f64>, proposed: Option<f64>| {
        if no_column && budget.is_none() {
            findings.unadopted.insert(row.to_owned(), proposed.is_some());
        }
    };
    let reading = |round: &super::Round| round.log.gpu_reading.clone().unwrap_or_default();
    let overlaps = rounds.rounds.iter().any(|round| reading(round).overlaps);
    let void = |round: &super::Round| reading(round).lock.is_some_and(|lock| lock >= LOCK_VOID);
    if rows.iter().any(|(phase, _)| phase == "gpu") {
        for round in &rounds.rounds {
            let read = reading(round);
            if let Some(lock) = read.lock.filter(|_| void(round)) {
                let frame = match round.log.timed("gpu", "gpu frame") {
                    Some(Entry::Read(timed)) => format!(", its GPU frame p95 {} ms", ms(timed.p95)),
                    _ => String::new(),
                };
                findings.notes.push(format!(
                    "{name} r{} void: the display's 60 Hz lock {lock:.3} (void at {LOCK_VOID:.2}){frame}",
                    round.round
                ));
            }
            if read.overlaps {
                findings.notes.push(format!(
                    "{name} r{}: uploads in flight on {:.1}% of timed frames; slow in TAA on {:.1}% of those, \
                     {:.1}% of the rest; in post {:.1}% and {:.1}%",
                    round.round,
                    read.in_flight_pct,
                    read.taa_slow[0],
                    read.taa_slow[1],
                    read.post_slow[0],
                    read.post_slow[1]
                ));
            }
        }
    }
    for (phase, row) in rows {
        if phase == "cpu" {
            let TimedCell::Read(timed) = rounds.timed(&phase, &row) else { continue };
            unadopted(findings, &row, timed.budget, timed.proposed);
            against(findings, name, &row, timed.p95, timed.budget, timed.proposed);
            continue;
        }
        if overlaps && DEPTH_ROWS.contains(&row.as_str()) || !overlaps && row == DEPTH_ONLY {
            continue;
        }
        let pass_row = overlaps && row != "gpu frame";
        let logged = |round: &super::Round| match round.log.timed(&phase, &row) {
            Some(Entry::Read(timed)) => Some(timed),
            _ => None,
        };
        // A row no round read (not exercised) judges nothing, as a row with no budget fails nothing.
        let Some(first) = rounds.rounds.iter().find_map(logged) else { continue };
        let (budget, proposed) = (first.budget, first.proposed);
        unadopted(findings, &row, budget, proposed);
        let (mut read, mut few) = (Vec::new(), 0);
        for round in rounds.rounds.iter().filter(|round| !void(round)) {
            let Some(timed) = logged(round) else { continue };
            if !pass_row {
                read.push(timed.p95);
                continue;
            }
            let frames = reading(round);
            match round.log.no_upload(&phase, &row) {
                Some(quiet) if frames.no_upload_frames * 10 >= frames.timed_frames && quiet.samples > 0 => {
                    read.push(quiet.p95);
                }
                _ => few += 1,
            }
        }
        let Some(least) = read.into_iter().reduce(f64::min) else {
            if budget.is_some() {
                findings.unjudged.push(if few > 0 {
                    format!("{name} {row}: fewer than a tenth of the frames with no upload in flight")
                } else {
                    format!("{name} {row}: every round void")
                });
            }
            continue;
        };
        let row = if pass_row { format!("{row} (no upload in flight)") } else { row };
        against(findings, name, &row, least, budget, proposed);
    }
}

/// A timed row's p95 against its budget, or shown against the figure proposed for it.
fn against(
    findings: &mut Findings,
    name: &str,
    row: &str,
    p95: f64,
    budget: Option<f64>,
    proposed: Option<f64>,
) {
    if let Some(budget) = budget {
        if p95 > budget {
            findings.fail(format!("{name} {row} p95 {} ms over {} ms", ms(p95), ms(budget)));
        } else {
            findings.pass();
        }
    } else if let Some(proposed) = proposed
        && p95 > proposed
    {
        findings.notes.push(format!("{name} {row} p95 {} ms over {} ms *proposed*", ms(p95), ms(proposed)));
    }
}

/// Count rows `rows` in `phases`: every round's worst against its budget. A row the phase didn't log fails; one
/// logged as not exercised passes, as read nowhere above zero; one with no budget logged goes unjudged.
fn counts(findings: &mut Findings, name: &str, rounds: &Rounds, phases: &[&str], rows: &[&str]) {
    for phase in phases {
        for row in rows {
            let readings = match rounds.counted(phase, row) {
                CountCell::Read(counted) => vec![(None, counted)],
                CountCell::Differs(readings) => {
                    readings.into_iter().map(|(round, c)| (Some(round), c)).collect()
                }
                CountCell::NotExercised => {
                    findings.pass();
                    continue;
                }
                CountCell::Missing => {
                    findings.fail(format!("{name} {phase}: no \"{row}\" row"));
                    continue;
                }
            };
            let Some(budget) = readings.iter().find_map(|(_, counted)| counted.budget) else {
                findings.unjudged.push(format!("{name}: \"{row}\" logged no budget"));
                continue;
            };
            match readings.iter().filter(|(_, counted)| counted.worst > budget).max_by_key(|(_, c)| c.worst) {
                Some((round, counted)) => findings.fail(format!(
                    "{name} {row} worst {} over {}{}",
                    thousands(counted.worst),
                    thousands(budget),
                    round.map_or_else(String::new, |round| format!(" (r{round})"))
                )),
                None => findings.pass(),
            }
        }
    }
}

/// The waits for a texture's level (m5-plan exit test 2, M5 Z2): each phase's line in each round, on a run paced
/// at the tier's rate (`pace`, its `--bench-pace` on this tier) its judged p99 at 30 frames or under and no wait
/// open 30 frames or more at its end, the pool's held waits too, and on a run that isn't, those shown beside the
/// verdict with its frame time and judged on nothing (the orchestrator's ruling); and on the
/// counting walk (`drawn`) its reasons line, the pool never too small for the tails and the zone arrived. A
/// streaming run with no line fails, as the sandbox logs both while streaming. The reasons' other fields, and
/// `texture streaming cpu`, are shown only.
fn waits(findings: &mut Findings, name: &str, rounds: &Rounds, pace: Option<u32>) {
    for round in &rounds.rounds {
        if round.log.waits.is_empty() {
            findings.fail(format!("{name} r{}: no texture waits line", round.round));
            continue;
        }
        for (phase, waits) in &round.log.waits {
            if pace.is_none() {
                // The 30 frames are at the tier's rate (M5 Z2, the orchestrator's ruling): an unpaced run's frames
                // are as short as the Mac draws them, so its waits are shown beside the verdict, judged on nothing.
                let frame = frame_text(&round.log, phase);
                findings.notes.push(format!(
                    "{name} r{} {phase} waits, not judged: the run isn't paced at the tier's rate ({frame}), so 30 of \
                     its frames aren't the rule's; p99 {} frames, worst {}, longest open {}, the pool's longest open {}",
                    round.round, waits.p99, waits.worst, waits.longest_open, waits.pool_longest_open
                ));
            } else {
                let mut over = Vec::new();
                let p99 = judged_p99(waits);
                if p99.over(WAIT_FRAMES) {
                    over.push(format!("p99 {p99} frames"));
                }
                if waits.longest_open >= WAIT_FRAMES {
                    over.push(format!("a wait open {} frames at its end", waits.longest_open));
                }
                if waits.pool_longest_open >= WAIT_FRAMES {
                    over.push(format!(
                        "a wait the pool held open {} frames at its end",
                        waits.pool_longest_open
                    ));
                }
                if over.is_empty() {
                    findings.pass();
                } else {
                    findings.fail(format!("{name} r{} {phase} waits: {}", round.round, over.join(", ")));
                }
            }
            // The reasons line counts updates, not frames: judged paced or not.
            if phase != "drawn" {
                continue;
            }
            match round.log.wait_reasons.get(phase) {
                None => findings.fail(format!("{name} r{}: no texture wait reasons line", round.round)),
                Some(reasons) if reasons.tails_over_pool > 0 => findings.fail(format!(
                    "{name} r{}: the pool too small for the tails {} times",
                    round.round, reasons.tails_over_pool
                )),
                Some(reasons) if reasons.arrived_at_update == 0 => {
                    findings.fail(format!("{name} r{}: the zone never arrived", round.round));
                }
                Some(_) => findings.pass(),
            }
            if let Some(reasons) = round.log.wait_reasons.get(phase) {
                findings.notes.push(format!(
                    "{name} r{} texture wait reasons: {}",
                    round.round,
                    shown(&reasons.fields)
                ));
            }
        }
        if let Some(cpu) = &round.log.streaming_cpu {
            findings.notes.push(format!("{name} r{} texture streaming cpu: {}", round.round, shown(cpu)));
        }
    }
}

/// Arrivals held to their bound (M5 Z2, the orchestrator's ruling): any phase's reasons line, in any round,
/// with `arrival_held_to_bound` above 0 fails, a streamer fault that a wipe hid for its 300 updates. A line that
/// doesn't print the field (a batch from before it) judges nothing here.
fn arrivals(findings: &mut Findings, name: &str, rounds: &Rounds) {
    for round in &rounds.rounds {
        for (phase, reasons) in &round.log.wait_reasons {
            match reasons.arrival_held_to_bound {
                Some(held) if held > 0 => findings.fail(format!(
                    "{name} r{} {phase}: {held} {}",
                    round.round,
                    if held == 1 { "arrival held to its bound" } else { "arrivals held to their bound" }
                )),
                // At 0 it fails nothing and passes nothing of its own: a stand-in stays judged on nothing.
                Some(_) | None => {}
            }
        }
    }
}

/// About how long a phase's frames took, in ms: its `cpu frame` and `gpu wait` rows' p50 together, the frame's
/// CPU and its wait for the GPU.
fn frame_ms(log: &crate::scale::log::Log, phase: &str) -> Option<f64> {
    Some((p50_us(log, phase, "cpu frame")? + p50_us(log, phase, "gpu wait").unwrap_or(0.0)) / 1000.0)
}

/// A timed row's p50 in a phase, in µs.
fn p50_us(log: &crate::scale::log::Log, phase: &str, row: &str) -> Option<f64> {
    match log.timed(phase, row) {
        Some(Entry::Read(timed)) => Some(timed.p50),
        _ => None,
    }
}

/// A phase's frame time for its waits' note, its own where the log gives one: the CPU's frame and its wait for
/// the GPU, or in the `gpu` phase, which times no CPU row, the GPU's frame. Failing those, another lap's, named
/// as that lap's.
fn frame_text(log: &crate::scale::log::Log, phase: &str) -> String {
    if let Some(frame) = frame_ms(log, phase) {
        return format!("frames of about {frame:.1} ms");
    }
    if let Some(gpu) = p50_us(log, phase, "gpu frame") {
        return format!("the GPU's frames of about {:.1} ms, this lap timing no CPU frame", gpu / 1000.0);
    }
    ["cpu", "first", "drawn"]
        .into_iter()
        .filter(|&other| other != phase)
        .find_map(|other| {
            frame_ms(log, other).map(|frame| {
                format!("frames of about {frame:.1} ms in the {other} lap, this lap timing none")
            })
        })
        .unwrap_or_else(|| "a frame time not logged".to_owned())
}

/// A line's numeric fields as `name value`, for a note.
fn shown(fields: &std::collections::BTreeMap<String, f64>) -> String {
    fields.iter().map(|(name, value)| format!("{name} {value}")).collect::<Vec<_>>().join(", ")
}

/// The 99th percentile a phase's waits are judged on, in one place: whether the waits the pool held while the
/// views wanted more than it holds leave it is the orchestrator's open ruling (M5 Z2). Today it is every wait's.
fn judged_p99(waits: &Waits) -> WaitFrames {
    waits.p99
}

/// Exit test 2's 30 frames.
const WAIT_FRAMES: u32 = 30;

/// Exit test 2's floor (M5 Z2): every round's counting walk closes the zone at its end, and the mesh pool is back
/// at its floor within the frames the bench gives it.
fn floor(findings: &mut Findings, name: &str, rounds: &Rounds) {
    for round in &rounds.rounds {
        match round.log.zone_closed {
            None => findings.fail(format!("{name} r{}: no zone closed line", round.round)),
            Some(closed) if !closed.unmounted => {
                findings.fail(format!("{name} r{}: the zone's pack didn't unmount", round.round));
            }
            Some(closed) if !closed.at_floor || closed.capacity > closed.floor => findings.fail(format!(
                "{name} r{}: the mesh pool {} B, over its floor {} B, {} frames after the zone closed",
                round.round, closed.capacity, closed.floor, closed.frames
            )),
            Some(_) => findings.pass(),
        }
    }
}

/// The process's most through each lap, every round, against §3.1's figure for the tier.
fn memory(findings: &mut Findings, name: &str, tier: &Tier, rounds: &Rounds) {
    let most = super::process_most(rounds);
    match (most, tier.memory_mib) {
        (None, _) => findings.fail(format!("{name}: no memory line")),
        (Some(_), None) => {
            findings.unjudged.push(format!("the manifest holds no §3.1 figure for {}", tier.name))
        }
        (Some(most), Some(budget)) if most > f64::from(budget) => {
            findings.fail(format!("{name} process {most:.0} MiB over {budget} MiB"));
        }
        (Some(_), Some(_)) => findings.pass(),
    }
}

/// The crowd a counted walk stood: every round's worst `ambient characters spawned` the tier's figure.
fn crowd(findings: &mut Findings, name: &str, tier: &Tier, rounds: &Rounds) {
    let Some(want) = tier.crowd else {
        findings.unjudged.push(format!("the manifest holds no crowd for {}", tier.name));
        return;
    };
    let worst: Vec<u64> = match rounds.counted("drawn", "ambient characters spawned") {
        CountCell::Read(counted) => vec![counted.worst],
        CountCell::Differs(readings) => readings.iter().map(|(_, counted)| counted.worst).collect(),
        CountCell::NotExercised => vec![0],
        CountCell::Missing => {
            findings.fail(format!("{name}: no \"ambient characters spawned\" row"));
            return;
        }
    };
    match worst.iter().find(|&&spawned| spawned != u64::from(want)) {
        Some(spawned) => findings.fail(format!("{name} spawned {spawned} ambient characters, not {want}")),
        None => findings.pass(),
    }
}

/// Every round's pipelines after the load: no first use, none on the frame's thread, no frame waiting, none
/// rebuilt.
fn pipelines(findings: &mut Findings, name: &str, rounds: &Rounds) {
    for round in &rounds.rounds {
        if round.log.pipelines.is_empty() {
            findings.fail(format!("{name} r{}: no pipelines line", round.round));
            continue;
        }
        for (phase, built) in &round.log.pipelines {
            if built.first_use + built.frame_thread + built.frame_waits + built.rebuilt > 0 {
                findings.fail(format!(
                    "{name} r{} {phase}: first use {}, frame thread {}, frames waiting {}, rebuilt {}",
                    round.round, built.first_use, built.frame_thread, built.frame_waits, built.rebuilt
                ));
            } else {
                findings.pass();
            }
        }
    }
}

/// The manifest's runs that a verdict reads for `test`, by name: what the runbook lists beside it.
#[cfg(test)]
pub(super) fn serving(manifest: &crate::scale::manifest::ScaleBench, test: u32) -> Vec<&str> {
    manifest
        .runs
        .iter()
        .filter(|run| run.serves.contains(&test) && !run.serves.contains(&3) && run.devices.is_empty())
        .map(|run| run.name.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{Round, Rounds};
    use super::*;
    use crate::scale::log::Log;
    use crate::scale::manifest::ScaleBench;

    fn rounds(logs: &[&str]) -> Rounds {
        Rounds {
            rounds: (1..)
                .zip(logs)
                .map(|(round, text)| Round { round, log: Log::parse(text), load: None, trace: None })
                .collect(),
        }
    }

    fn manifest() -> ScaleBench {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/scale.ron");
        ScaleBench::parse(&std::fs::read_to_string(path).expect("the batch")).expect("the batch parses")
    }

    fn tier(manifest: &ScaleBench, name: &str) -> Tier {
        manifest.tier(name).expect("a tier").clone()
    }

    fn timed(phase: &str, row: &str, p95: f64, worst: f64, budget: &str, proposed: &str) -> String {
        format!(
            "x INFO sapphire::sandbox: fight row phase=\"{phase}\" row=\"{row}\" samples=100 p50_us=1.0 \
             p95_us={p95:.1} p99_us={worst:.1} worst_us={worst:.1} worst_tick=1 budget_us={budget} \
             proposed_us={proposed} over_p50=false over_worst=false\n"
        )
    }

    fn count(row: &str, worst: u64, budget: &str) -> String {
        format!(
            "x INFO sapphire::sandbox: count row phase=\"drawn\" row=\"{row}\" frames=10 p50=1 p95={worst} \
             worst={worst} total=10 worst_tick=1 budget={budget} over_p50=false over_worst=false\n"
        )
    }

    fn closed(at_floor: bool, capacity: u64, floor: u64) -> String {
        format!(
            "x INFO sapphire::sandbox: zone closed unmounted=\"unmounted\" frames=6 at_floor={at_floor} \
             capacity_before=67108864 floor_before=67108864 capacity={capacity} floor={floor} in_use=1 \
             meshes_freed=9\n"
        )
    }

    fn waits(p99: &str, longest_open: u32) -> String {
        format!(
            "x INFO sapphire::sandbox: texture waits phase=\"drawn\" completed=12 p50_frames=1 p99_frames={p99} \
             worst_frames={p99} pool_held=0 pool_held_p99_frames=- pool_held_worst_frames=- open=1 \
             longest_open_frames={longest_open} pool_open=0 pool_longest_open_frames=0\n{}",
            reasons(0, 41)
        )
    }

    fn reasons(tails_over_pool: u32, arrived_at_update: u32) -> String {
        format!(
            "x INFO sapphire::sandbox: texture wait reasons decode=1 decode_long=0 phase=\"drawn\" \
             long_over_frames=30 raised_bytes=9 reraised_bytes=0 reraised_within_updates=60 \
             tails_over_pool={tails_over_pool} arrival_waits=3 arrival_long=0 arrival_open=0 \
             arrived_at_update={arrived_at_update}\n"
        )
    }

    /// A reasons line as the bench prints it since the arrival's hold has a bound (M5 Z2).
    fn held(phase: &str, held: u32) -> String {
        reasons(0, 41).replace("phase=\"drawn\"", &format!("phase=\"{phase}\"")).replace(
            "arrived_at_update=41\n",
            &format!("arrived_at_update=41 arrival_held_to_bound={held}\n"),
        )
    }

    fn no_upload(row: &str, samples: u32, p95: f64) -> String {
        format!(
            "x INFO sapphire::sandbox: fight row no upload phase=\"gpu\" row=\"{row}\" samples={samples} \
             p50_us=1.0 p95_us={p95:.1} p99_us={p95:.1}\n"
        )
    }

    fn reading(overlaps: bool, no_upload_frames: u32, lock: &str) -> String {
        format!(
            "x INFO sapphire::sandbox: fight gpu reading phase=\"gpu\" backend=\"metal\" overlaps={overlaps} \
             timed_frames=1000 no_upload_frames={no_upload_frames} in_flight_pct=40.0 taa_slow_in_flight_pct=24.0 \
             taa_slow_no_upload_pct=1.0 post_slow_in_flight_pct=23.0 post_slow_no_upload_pct=0.5 slow_frames=90 \
             lock={lock} lock_period_us=16667.0\n"
        )
    }

    /// A timed row passes on the least of the rounds' p95 at or under its budget, whatever its worst frame; it
    /// fails when even the least round's p95 is over.
    #[test]
    fn a_timed_row_fails_on_the_least_rounds_p95_and_not_on_a_worst_frame() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("exit-walk").expect("a run"), tier(&manifest, "desktop-low"));
        let pass = rounds(&[
            &timed("gpu", "gpu frame", 15_000.0, 90_000.0, "Some(14000)", "None"),
            &timed("gpu", "gpu frame", 13_500.0, 95_000.0, "Some(14000)", "None"),
        ]);
        let findings = judge(1, run, "exit-walk", &tier, &pass);
        assert!(findings.failed.is_empty() && findings.judged == 1, "{findings:?}");
        assert_eq!(Verdict::of(&findings), Verdict::Pass);
        let fail = rounds(&[
            &timed("gpu", "gpu frame", 15_000.0, 16_000.0, "Some(14000)", "None"),
            &timed("gpu", "gpu frame", 14_500.0, 15_000.0, "Some(14000)", "None"),
        ]);
        let findings = judge(1, run, "exit-walk", &tier, &fail);
        assert_eq!(findings.failed, ["exit-walk gpu frame p95 14.50 ms over 14.00 ms"]);
        assert_eq!(Verdict::of(&findings), Verdict::Fail);
    }

    /// A row with only a proposed figure (B4's Desktop-High and Mobile-High columns) is shown over it and fails
    /// nothing.
    #[test]
    fn a_proposed_column_fails_nothing() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("exit-walk").expect("a run"), tier(&manifest, "desktop-high"));
        let log = [
            timed("gpu", "gpu frame", 10_000.0, 11_000.0, "Some(12000)", "None"),
            timed("gpu", "gpu main", 9_000.0, 9_500.0, "None", "Some(4000)"),
        ]
        .concat();
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&log]));
        assert!(findings.failed.is_empty(), "{findings:?}");
        assert_eq!(findings.notes, ["exit-walk gpu main p95 9.000 ms over 4.000 ms *proposed*"]);
        assert_eq!(Verdict::of(&findings), Verdict::Pass, "judged on the frame, which is adopted");
    }

    /// A tier with no adopted column (M5 B4's Desktop-High and Mobile-High) gives its why: its rows shown beside the
    /// proposed figures or with none, failing nothing. An adopted tier gives none, and no verdict changes.
    #[test]
    fn a_tier_with_no_adopted_column_says_why() {
        let manifest = manifest();
        let run = manifest.run("crowd-tour").expect("a run");
        let high = tier(&manifest, "desktop-high");
        let log = [
            timed("cpu", "animation and ik", 900.0, 1_000.0, "None", "None"),
            timed("gpu", "gpu frame", 9_000.0, 9_500.0, "Some(12000)", "None"),
            timed("gpu", "gpu main", 3_000.0, 3_500.0, "None", "Some(4000)"),
        ]
        .concat();
        let findings = judge(5, run, "crowd-tour (on)", &high, &rounds(&[&log]));
        assert_eq!(Verdict::of(&findings), Verdict::NotJudged);
        assert_eq!(
            no_column(&findings, &high).as_deref(),
            Some(
                "Desktop-High has no adopted column, so its rows are shown and fail nothing (the runbook's \"not \
                 judged (no column)\"): 1 (gpu main) beside M5 B4's proposed figures, 1 (animation and ik) with none"
            )
        );
        let findings = judge(6, run, "crowd-tour (on)", &high, &rounds(&[&log]));
        assert!(
            no_column(&findings, &high).is_some_and(|why| why.contains("1 (animation and ik) with none"))
        );
        let low = tier(&manifest, "desktop-low");
        let log = [
            timed("cpu", "animation and ik", 900.0, 1_000.0, "Some(1500)", "None"),
            timed("gpu", "gpu main", 3_000.0, 3_500.0, "Some(4900)", "None"),
            timed("gpu", "gpu skinning", 300.0, 400.0, "None", "Some(500)"),
        ]
        .concat();
        let findings = judge(5, run, "crowd-tour (on)", &low, &rounds(&[&log]));
        assert_eq!(no_column(&findings, &low), None, "{findings:?}");
        assert_eq!(Verdict::of(&findings), Verdict::Pass);
    }

    /// A wait for a texture's level over 30 frames at the 99th percentile fails, `32+` among them, and so does
    /// one still open for 30 frames at the run's end; `-` (none completed) passes.
    #[test]
    fn a_wait_over_30_frames_fails() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("counted-streaming").expect("a run"), tier(&manifest, "mobile-low"));
        let rows = [
            count("bytes over budget in use", 0, "Some(0)"),
            count("bytes decoded waiting", 1 << 20, "Some(67108864)"),
            count("streamed texture bytes", 300 << 20, "Some(347078656)"),
            count("frame thread loads", 0, "Some(0)"),
            closed(true, 8 << 20, 8 << 20),
        ]
        .concat();
        for (p99, open, failed) in
            [("12", 3, false), ("-", 0, false), ("31", 0, true), ("32+", 0, true), ("4", 30, true)]
        {
            let log = format!("{rows}{}", waits(p99, open));
            let findings = judge(2, run, "counted-streaming", &tier, &rounds(&[&log]));
            assert_eq!(!findings.failed.is_empty(), failed, "p99 {p99}, open {open}: {findings:?}");
        }
        let findings = judge(2, run, "counted-streaming", &tier, &rounds(&[&rows]));
        assert_eq!(
            findings.failed,
            ["counted-streaming r1: no texture waits line"],
            "a streaming run logs one"
        );
    }

    /// On a backend that overlaps its passes (M5 Z2) the prepasses and shadows are judged as one depth-only row,
    /// and a pass row on the frames with no upload in flight; elsewhere the two rows are judged apart and the
    /// depth-only row isn't.
    #[test]
    fn an_overlapping_backend_judges_depth_as_one_row_on_frames_with_no_upload() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("exit-walk").expect("a run"), tier(&manifest, "desktop-low"));
        let rows = [
            timed("gpu", "gpu frame", 13_000.0, 20_000.0, "Some(14000)", "None"),
            timed("gpu", "gpu prepasses", 2_600.0, 3_000.0, "Some(1500)", "None"),
            timed("gpu", "gpu shadows", 1_100.0, 3_000.0, "Some(2000)", "None"),
            timed("gpu", "gpu depth only", 3_400.0, 5_000.0, "Some(3500)", "None"),
            timed("gpu", "gpu taa", 1_100.0, 2_000.0, "Some(800)", "None"),
            no_upload("gpu prepasses", 600, 2_500.0),
            no_upload("gpu shadows", 600, 1_000.0),
            no_upload("gpu depth only", 600, 3_300.0),
            no_upload("gpu taa", 600, 600.0),
        ]
        .concat();
        let metal = format!("{rows}{}", reading(true, 600, "0.050"));
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&metal]));
        assert!(findings.failed.is_empty(), "{findings:?}");
        assert_eq!(
            (findings.judged, Verdict::of(&findings)),
            (3, Verdict::Pass),
            "frame, depth only and TAA"
        );
        assert!(
            findings.notes.iter().any(|note| note.contains("uploads in flight on 40.0%")),
            "{findings:?}"
        );
        let other = format!("{rows}{}", reading(false, 600, "0.050"));
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&other]));
        assert_eq!(
            findings.failed,
            [
                "exit-walk gpu prepasses p95 2.600 ms over 1.500 ms",
                "exit-walk gpu taa p95 1.100 ms over 0.800 ms"
            ]
        );
        // Too few frames with no upload in flight judge no pass row: never a pass.
        let few = format!("{rows}{}", reading(true, 90, "0.050"));
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&few]));
        assert_eq!(Verdict::of(&findings), Verdict::NotJudged, "{findings:?}");
        assert!(findings.unjudged[0].contains("fewer than a tenth"), "{findings:?}");
    }

    /// A round with the display's 60 Hz lock is void (M5 Z2): left out of the least of the rounds, shown with its
    /// figure, and a row with every round void isn't judged.
    #[test]
    fn a_round_with_the_displays_lock_is_void() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("exit-walk").expect("a run"), tier(&manifest, "desktop-low"));
        let round = |p95: f64, lock: &str| {
            format!(
                "{}{}",
                timed("gpu", "gpu frame", p95, 20_000.0, "Some(14000)", "None"),
                reading(false, 0, lock)
            )
        };
        let locked = round(13_000.0, "0.270");
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&locked, &round(14_500.0, "0.060")]));
        assert_eq!(
            findings.failed,
            ["exit-walk gpu frame p95 14.50 ms over 14.00 ms"],
            "the void round's 13 ms"
        );
        assert!(findings.notes[0].contains("r1 void: the display's 60 Hz lock 0.270"), "{findings:?}");
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&locked, &round(15_000.0, "0.100")]));
        assert_eq!(Verdict::of(&findings), Verdict::NotJudged, "{findings:?}");
        assert_eq!(findings.unjudged, ["exit-walk gpu frame: every round void"]);
        // A lock too few frames read voids nothing.
        let findings = judge(1, run, "exit-walk", &tier, &rounds(&[&round(13_000.0, "-")]));
        assert_eq!(Verdict::of(&findings), Verdict::Pass, "{findings:?}");
    }

    /// Exit test 2's lines (M5 Z2): the pool's longest open wait, and on the counting walk the reasons line, the
    /// pool too small for the tails or a zone that never arrived failing, and the line missing failing.
    #[test]
    fn the_waits_reasons_and_the_pools_open_waits_fail_as_given() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("counted-streaming").expect("a run"), tier(&manifest, "mobile-low"));
        let rows = [
            count("bytes over budget in use", 0, "Some(0)"),
            count("bytes decoded waiting", 1 << 20, "Some(67108864)"),
            count("streamed texture bytes", 300 << 20, "Some(347078656)"),
            count("frame thread loads", 0, "Some(0)"),
            closed(true, 8 << 20, 8 << 20),
        ]
        .concat();
        let line = waits("12", 0);
        let (waits_line, _) = line.split_at(
            line.find("texture wait reasons").and_then(|at| line[..at].rfind('\n')).expect("two lines") + 1,
        );
        for (log, failed) in [
            (format!("{rows}{line}"), None),
            (format!("{rows}{waits_line}{}", reasons(2, 41)), Some("too small for the tails")),
            (format!("{rows}{waits_line}{}", reasons(0, 0)), Some("never arrived")),
            (format!("{rows}{waits_line}"), Some("no texture wait reasons line")),
            (
                format!(
                    "{rows}{}",
                    line.replace("pool_longest_open_frames=0", "pool_longest_open_frames=30")
                ),
                Some("the pool held open 30"),
            ),
        ] {
            let findings = judge(2, run, "counted-streaming", &tier, &rounds(&[&log]));
            match failed {
                None => assert!(findings.failed.is_empty(), "{findings:?}"),
                Some(why) => {
                    assert!(findings.failed.iter().any(|what| what.contains(why)), "{why}: {findings:?}")
                }
            }
        }
    }

    /// An arrival held to its bound fails exit test 2 (M5 Z2, the orchestrator's ruling), named by run, round and
    /// phase, on any run, a stand-in too; at 0 it passes, and a line from before the field judges as before.
    #[test]
    fn an_arrival_held_to_its_bound_fails_exit_test_2() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("counted-streaming").expect("a run"), tier(&manifest, "mobile-low"));
        let rows = [
            count("bytes over budget in use", 0, "Some(0)"),
            count("bytes decoded waiting", 1 << 20, "Some(67108864)"),
            count("streamed texture bytes", 300 << 20, "Some(347078656)"),
            count("frame thread loads", 0, "Some(0)"),
            closed(true, 8 << 20, 8 << 20),
        ]
        .concat();
        let line = waits("12", 0);
        let waits_line =
            &line[..line.find("x INFO sapphire::sandbox: texture wait reasons").expect("two lines")];
        let judged = |reasons: &str| {
            judge(2, run, "counted-streaming", &tier, &rounds(&[&format!("{rows}{waits_line}{reasons}")]))
        };
        let at_zero = judged(&held("drawn", 0));
        assert!(at_zero.failed.is_empty(), "{at_zero:?}");
        assert_eq!(Verdict::of(&at_zero), Verdict::Pass);
        let held_once = judged(&held("drawn", 1));
        assert_eq!(held_once.failed, ["counted-streaming r1 drawn: 1 arrival held to its bound"]);
        let before = judged(&reasons(0, 41));
        assert!(before.failed.is_empty(), "no field, judged as before: {before:?}");
        assert_eq!(Verdict::of(&before), Verdict::Pass);
        // A trip stands in for the walk's floor and is judged on nothing else, but its arrival held fails.
        let trip = manifest.run("trip").expect("a run");
        let findings = judge(2, trip, "trip", &tier, &rounds(&[&held("first", 2)]));
        assert_eq!(findings.failed, ["trip r1 first: 2 arrivals held to their bound"]);
        let findings = judge(2, trip, "trip", &tier, &rounds(&[&held("first", 0)]));
        assert_eq!(Verdict::of(&findings), Verdict::NotJudged, "{findings:?}");
    }

    /// The 30 frames are at the tier's rate (M5 Z2, the orchestrator's ruling): an unpaced run's wait over them
    /// fails nothing and is shown with its frame time; a run paced on the tier still fails.
    #[test]
    fn an_unpaced_runs_waits_are_shown_and_a_paced_runs_judged() {
        let manifest = manifest();
        let tier = tier(&manifest, "desktop-high");
        let streaming = manifest.run("streaming").expect("a run");
        assert_eq!(streaming.pace("desktop-high"), None, "the timed walk isn't paced");
        let frame = |row: &str, p50: f64| {
            format!(
                "x INFO sapphire::sandbox: fight row phase=\"first\" row=\"{row}\" samples=100 p50_us={p50:.1} \
                 p95_us={p50:.1} p99_us={p50:.1} worst_us={p50:.1} worst_tick=1 budget_us=None proposed_us=None\n"
            )
        };
        let log = [
            frame("cpu frame", 7_000.0),
            frame("gpu wait", 3_000.0),
            waits("53", 31).replace("drawn", "first"),
        ]
        .concat();
        let findings = judge(2, streaming, "streaming", &tier, &rounds(&[&log]));
        assert!(!findings.failed.iter().any(|what| what.contains("waits")), "{findings:?}");
        let shown =
            findings.notes.iter().find(|note| note.contains("first waits, not judged")).expect("shown");
        assert!(shown.contains("frames of about 10.0 ms") && shown.contains("p99 53 frames"), "{shown}");
        assert!(shown.contains("longest open 31"), "{shown}");
        let paced = manifest.run("counted-streaming").expect("a run");
        assert_eq!(paced.pace("desktop-high"), Some(60));
        let findings = judge(2, paced, "counted-streaming", &tier, &rounds(&[&waits("53", 0)]));
        assert!(
            findings.failed.contains(&"counted-streaming r1 drawn waits: p99 53 frames".to_owned()),
            "{findings:?}"
        );
    }

    /// An unpaced run's `gpu` phase, which times no CPU frame, shows its waits with its own GPU frame, and a lap
    /// with no frame of its own names the lap whose frame it shows.
    #[test]
    fn the_gpu_phases_waits_carry_its_own_frame_time() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("streaming").expect("a run"), tier(&manifest, "desktop-high"));
        let frame = |phase: &str, row: &str, p50: f64| {
            format!(
                "x INFO sapphire::sandbox: fight row phase=\"{phase}\" row=\"{row}\" samples=100 p50_us={p50:.1} \
                 p95_us={p50:.1} p99_us={p50:.1} worst_us={p50:.1} worst_tick=1 budget_us=None proposed_us=None\n"
            )
        };
        let gpu_waits = waits("28", 0).replace("drawn", "gpu");
        let log = [frame("cpu", "cpu frame", 7_000.0), frame("cpu", "gpu wait", 3_000.0), gpu_waits.clone()]
            .concat();
        let own = [log.as_str(), &frame("gpu", "gpu frame", 9_100.0)].concat();
        let findings = judge(2, run, "streaming", &tier, &rounds(&[&own]));
        let shown = findings.notes.iter().find(|note| note.contains("gpu waits, not judged")).expect("shown");
        assert!(
            shown.contains("(the GPU's frames of about 9.1 ms, this lap timing no CPU frame)"),
            "{shown}"
        );
        let findings = judge(2, run, "streaming", &tier, &rounds(&[&log]));
        let shown = findings.notes.iter().find(|note| note.contains("gpu waits, not judged")).expect("shown");
        assert!(shown.contains("(frames of about 10.0 ms in the cpu lap, this lap timing none)"), "{shown}");
    }

    /// Exit test 6 (M5 Z2, the orchestrator's ruling): a held bench, which its log says logs no fight rows, is
    /// shown and judged on nothing; a fight bench that logs no rows still fails; the crowd off is told by its
    /// arguments.
    #[test]
    fn a_held_bench_is_shown_and_a_fight_bench_with_no_rows_fails() {
        let manifest = manifest();
        let tier = tier(&manifest, "desktop-high");
        let hold = manifest.run("crowd-hold").expect("a run");
        let held = "x INFO sapphire::sandbox: bench frames=1200 width=2560 height=1440 tier=\"Desktop-High\"\n\
                    x INFO sapphire::sandbox: bench gpu frames=239 gpu_frame_us=10539.5\n";
        let findings = judge(6, hold, "crowd-hold (on)", &tier, &rounds(&[held]));
        assert!(findings.failed.is_empty(), "{findings:?}");
        assert!(findings.notes[0].contains("a plain bench, which logs no fight rows"), "{findings:?}");
        let tour = manifest.run("crowd-tour").expect("a run");
        let fight = "x INFO sapphire::sandbox: bench of a fight tier=\"desktop-high\" startup_ms=900.0\n";
        let findings = judge(6, tour, "crowd-tour (on)", &tier, &rounds(&[fight]));
        assert_eq!(findings.failed, ["crowd-tour (on): no timed row logged"]);
        let off = tour.variants.iter().find(|variant| crowd_off(&variant.args)).expect("an off variant");
        assert_eq!(off.name, "off");
        assert!(
            !tour
                .variants
                .iter()
                .filter(|variant| variant.name == "on")
                .any(|variant| crowd_off(&variant.args))
        );
    }

    /// A count over its budget in any round fails, and the pool under a binding pool is the run's own.
    #[test]
    fn a_count_over_its_budget_fails_and_a_crowd_short_of_its_figure_fails() {
        let manifest = manifest();
        let (run, tier) = (manifest.run("counted").expect("a run"), tier(&manifest, "mobile-high"));
        let over = [count("main draws", 701, "Some(700)"), count("lights", 8, "Some(8)")].concat();
        let findings = judge(1, run, "counted", &tier, &rounds(&[&over]));
        assert_eq!(findings.failed, ["counted main draws worst 701 over 700"]);
        let crowd = count("ambient characters spawned", 29, "None");
        let findings = judge(6, run, "counted", &tier, &rounds(&[&crowd]));
        assert_eq!(findings.failed, ["counted spawned 29 ambient characters, not 30"]);
        let crowd = count("ambient characters spawned", 30, "None");
        assert_eq!(Verdict::of(&judge(6, run, "counted", &tier, &rounds(&[&crowd]))), Verdict::Pass);
    }

    /// A run not made, or a stand-in, judges nothing, and the verdict says so rather than pass.
    #[test]
    fn a_run_not_made_or_a_stand_in_is_not_judged() {
        let manifest = manifest();
        let tier = tier(&manifest, "desktop-low");
        let findings = judge(1, manifest.run("exit-walk").expect("a run"), "exit-walk", &tier, &rounds(&[]));
        assert_eq!(Verdict::of(&findings), Verdict::NotJudged);
        let trip = manifest.run("trip").expect("a run");
        let findings = judge(2, trip, "trip", &tier, &rounds(&["x"]));
        assert_eq!(Verdict::of(&findings), Verdict::NotJudged, "{findings:?}");
        // A stand-in is shown beside the runs that judge its test, and holds no verdict back.
        assert!(findings.unjudged.is_empty() && findings.notes[0].contains("stands in for"), "{findings:?}");
    }

    /// Exit test 2's floor (M5 Z2): the counting walk under a binding pool closes the district at its end, and
    /// the mesh pool must be back at its floor; a walk that logs no close fails.
    #[test]
    fn the_pools_floor_is_judged_where_the_counting_walk_closes_the_zone() {
        let manifest = manifest();
        let (run, tier) =
            (manifest.run("counted-streaming").expect("a run"), tier(&manifest, "desktop-high"));
        let rows = [
            count("bytes over budget in use", 0, "Some(0)"),
            count("bytes decoded waiting", 1 << 20, "Some(268435456)"),
            count("streamed texture bytes", 900 << 20, "Some(1342177280)"),
            count("frame thread loads", 0, "Some(0)"),
            waits("9", 0),
        ]
        .concat();
        for (closing, failed) in [
            (closed(true, 8 << 20, 8 << 20), None),
            (closed(false, 16 << 20, 8 << 20), Some("over its floor")),
            (String::new(), Some("no zone closed line")),
        ] {
            let findings = judge(2, run, "counted-streaming", &tier, &rounds(&[&format!("{rows}{closing}")]));
            match failed {
                None => assert!(findings.failed.is_empty(), "{findings:?}"),
                Some(why) => assert!(findings.failed.iter().any(|what| what.contains(why)), "{findings:?}"),
            }
        }
    }

    /// The counted walk at night, dry and wet, and the counted fight count draws and lights for exit test 1
    /// (M5 Z2), and their pipelines for exit test 7, a fight's under its phase.
    #[test]
    fn the_counted_night_and_fight_hold_draws_lights_and_pipelines() {
        let manifest = manifest();
        let tier = tier(&manifest, "mobile-low");
        let fight = manifest.run("counted-fight").expect("a run");
        let over = [count("main draws", 720, "Some(700)"), count("lights", 8, "Some(8)")].concat();
        let findings = judge(1, fight, "counted-fight", &tier, &rounds(&[&over]));
        assert_eq!(findings.failed, ["counted-fight main draws worst 720 over 700"]);
        let built = "x INFO sapphire::sandbox: fight pipelines phase=\"first\" at_start=9 after_first_frame=9 \
                     at_end=9 prewarmed=9 first_use=1 rebuilt=0 warmed_off_frame=0 frame_thread=0 frame_waits=0\n";
        let findings = judge(7, fight, "counted-fight", &tier, &rounds(&[built]));
        assert!(findings.failed[0].contains("first use 1"), "{findings:?}");
        let night = manifest.run("counted-night").expect("a run");
        assert_eq!(night.variants.len(), 2, "dry and wet");
        let within = [count("main draws", 650, "Some(700)"), count("lights", 8, "Some(8)")].concat();
        assert_eq!(
            Verdict::of(&judge(1, night, "counted-night (wet)", &tier, &rounds(&[&within]))),
            Verdict::Pass
        );
    }

    /// A device's walk (M5 Z2) has verdicts of its own, a line for each tier and exit test it serves, by the
    /// rules this Mac's are judged by: a tier it didn't walk is "not run", a pool over the run's fails, and the
    /// Mac's lines never read a device's units.
    #[test]
    fn a_devices_walk_has_verdicts_of_its_own() {
        let manifest = manifest();
        let out = std::env::temp_dir().join(format!("sapphire_scale_device_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let device = manifest.device("android").expect("the emulator");
        let unit = "device-streaming.mobile-high.android.r1";
        let walk = |streamed: u64| {
            [
                count("bytes over budget in use", 0, "Some(0)"),
                count("bytes decoded waiting", 1 << 20, "Some(67108864)"),
                count("streamed texture bytes", streamed, "Some(13631488)"),
                count("frame thread loads", 0, "Some(0)"),
                closed(true, 4 << 20, 4 << 20),
                waits("4", 0),
                "x INFO sapphire::sandbox: zone walk frames=7600 first_use=0 rebuilt=0 frame_thread=0 \
                 frame_waits=0\n"
                    .to_owned(),
            ]
            .concat()
        };
        let status = Status {
            exit: Some(0),
            timed_out: false,
            seconds: 90.0,
            load: None,
            load_end: None,
            command: String::new(),
        };
        status.write(&out, unit).unwrap();
        let inputs = super::super::Inputs {
            manifest: &manifest,
            logs: &out,
            baseline: None,
            commit: "",
            smoke: false,
        };
        let units = manifest.every_unit(None);
        let verdicts = |log: String| {
            std::fs::write(out.join(format!("{unit}.log")), log).unwrap();
            let statuses: std::collections::BTreeMap<&str, Option<Status>> =
                units.iter().map(|unit| (unit.name.as_str(), Status::read(&out, &unit.name))).collect();
            device_rows(&inputs, &units, &statuses, device)
                .into_iter()
                .map(|row| (row.test, row.tier, row.verdict))
                .collect::<Vec<_>>()
        };
        let android = |tier: &str| format!("{tier}, the Android emulator");
        assert_eq!(
            verdicts(walk(12 << 20)),
            [
                (2, android("Mobile-High"), "pass"),
                (2, android("Mobile-Low"), "not run"),
                (7, android("Mobile-High"), "pass"),
                (7, android("Mobile-Low"), "not run"),
            ]
        );
        assert_eq!(verdicts(walk(14 << 20))[0], (2, android("Mobile-High"), "fail"), "over the 13 MiB pool");
        // This Mac's exit test 2 on Mobile-High reads its own runs, none of them made here.
        let mut text = String::new();
        let mut failures = Vec::new();
        let statuses =
            units.iter().map(|unit| (unit.name.as_str(), Status::read(&out, &unit.name))).collect();
        section(&mut text, &mut failures, &inputs, &units, &statuses);
        let mac = text.lines().find(|line| line.starts_with("| 2 | Mobile-High | ")).expect(&text);
        assert!(mac.contains("not judged here") && !mac.contains("device-streaming"), "{mac}");
        assert!(text.contains("| 2 | Mobile-High, the Android emulator | **fail** |"), "{text}");
        assert!(failures.iter().any(|failure| failure.contains("the Android emulator")), "{failures:?}");
        std::fs::remove_dir_all(&out).unwrap();
    }

    /// The manifest's `serves` follow m5-plan §1 (M5 Z2): the crowd is exit test 6, the packages' pairs serve 3,
    /// and the counting walk serves 1, 6 and 7.
    #[test]
    fn the_runs_serve_the_plans_exit_tests() {
        let manifest = manifest();
        assert_eq!(
            serving(&manifest, 1),
            [
                "tour-dusk",
                "tour-night",
                "exit-walk",
                "night-weather",
                "district-fight",
                "counted",
                "counted-night",
                "counted-fight"
            ]
        );
        assert_eq!(
            serving(&manifest, 7),
            ["exit-walk", "district-fight", "counted", "counted-streaming", "counted-night", "counted-fight"]
        );
        assert_eq!(serving(&manifest, 6), ["crowd-hold", "crowd-tour", "counted"]);
        let pairs = [
            "lods",
            "character-lods",
            "character-lods-crowd",
            "draw-runs",
            "shadow-cache",
            "capsule-bins",
            "capsule-bins-hub",
            "upscaler",
        ];
        for pair in pairs {
            assert!(manifest.run(pair).expect("a pair").serves.contains(&3), "{pair}");
        }
        for tier in &manifest.tiers {
            assert!(tier.memory_mib.is_some() && tier.crowd.is_some(), "{}", tier.name);
        }
    }
}
