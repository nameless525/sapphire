//! The verdicts (M5 Z2): one line for each of m5-plan §1's exit tests and each tier, by the exit run's rules
//! (`docs/m5-exit-run.md`), from the runs the manifest says serve each test. A verdict is a pass, a fail with
//! what failed, or "not judged" with why: a run not made yet, a stand-in, a figure the manifest doesn't hold, or a
//! test the batch doesn't measure (goldens, GPU tests, recipes). The report fails on any fail.
//!
//! - **A timed row** passes on the least of the rounds' 95th percentiles at or under its budget (the plan's "runs
//!   alternated and the least kept"); a worst frame over it fails nothing. A row with only a proposed figure
//!   (M5 B4's Desktop-High and Mobile-High columns of §3.3) is shown against it, marked proposed, and fails
//!   nothing: adopting a column is moving its figure into the bench's budget, data the sandbox logs.
//! - **A count** fails on any round's worst frame over its budget.
//! - **A wait for a texture's level** fails on a phase's 99th percentile over 30 frames (`32+` is over), or a
//!   wait open at a run's end for 30 frames or more.
//! - **Pairs** (runs serving exit test 3, each package's on against off) are costs to read beside each other, so
//!   no verdict reads them.

use std::fmt::Write as _;

use super::{CountCell, Rounds, TimedCell, gather, ms, thousands, title};
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
                    if !run.serves.contains(&test) || run.serves.contains(&3) || !run.on(&tier.name) {
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
                        let rounds = gather(inputs, units, statuses, run_index, tier_index, variant);
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
                waits(&mut findings, name, rounds);
                if !run.timed {
                    floor(&mut findings, name, rounds);
                }
            }
            if run.read.contains(&Read::Memory) {
                memory(&mut findings, name, tier, rounds);
            }
        }
        5 => timed_rows(&mut findings, name, rounds, |row| {
            ["animation and ik", "secondary motion", "gpu skinning", "gpu main", "gpu prepasses"]
                .contains(&row)
        }),
        6 => {
            if run.timed {
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

/// Every timed row `keep` takes, in the `cpu` and `gpu` phases: the least round's p95 against its budget.
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
    for (phase, row) in rows {
        let TimedCell::Read(timed) = rounds.timed(&phase, &row) else { continue };
        if let Some(budget) = timed.budget {
            if timed.p95 > budget {
                findings.fail(format!("{name} {row} p95 {} ms over {} ms", ms(timed.p95), ms(budget)));
            } else {
                findings.pass();
            }
        } else if let Some(proposed) = timed.proposed
            && timed.p95 > proposed
        {
            findings.notes.push(format!(
                "{name} {row} p95 {} ms over {} ms *proposed*",
                ms(timed.p95),
                ms(proposed)
            ));
        }
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

/// The waits for a texture's level: each phase's line in each round, its p99 at 30 frames or under and no wait
/// open 30 frames or more at its end. A streaming run with no line fails, as the sandbox logs one while
/// streaming.
fn waits(findings: &mut Findings, name: &str, rounds: &Rounds) {
    for round in &rounds.rounds {
        if round.log.waits.is_empty() {
            findings.fail(format!("{name} r{}: no texture waits line", round.round));
            continue;
        }
        for (phase, waits) in &round.log.waits {
            let mut over = Vec::new();
            if waits.p99.over(WAIT_FRAMES) {
                over.push(format!("p99 {} frames", waits.p99));
            }
            if waits.longest_open >= WAIT_FRAMES {
                over.push(format!("a wait open {} frames at its end", waits.longest_open));
            }
            if over.is_empty() {
                findings.pass();
            } else {
                findings.fail(format!("{name} r{} {phase} waits: {}", round.round, over.join(", ")));
            }
        }
    }
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
        .filter(|run| run.serves.contains(&test) && !run.serves.contains(&3))
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
             longest_open_frames={longest_open} pool_open=0 pool_longest_open_frames=0\n"
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
