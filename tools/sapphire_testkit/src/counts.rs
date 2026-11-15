//! The scale lane, behind `cargo xtask counts` (M5 X4): the benchmark district's `ci` preset walked by the
//! sandbox's zone walk bench at fixed settings, and what each walk counted held to a committed table
//! (`tests/counts/district-ci.txt`, [`table`]).
//!
//! - **Counts, not times.** A walk is `--bench <frames> --draw-every <n>` of the district's tour on one tier's
//!   settings at one hour: every frame steps, streams, prepares and records, every `n`th runs its graph, and
//!   the bench logs what the frames it drew submitted (`count row`), what the load built and what the zone
//!   placed ([`WALK_ROWS`]). The walk draws without simulating, so the same build draws the same frames on any
//!   machine at any load: no GPU time is read, and none is a result. The walk holds at the tour's start through
//!   the bench's warm-up and counts from its first step, which is drawn (M5 Z3): before, the warm-up walked the
//!   first 6 m uncounted.
//! - **Every row is exact,** held both ways: a count that goes down fails too, until the change that lowered
//!   it writes it down. Two things made the timing-paced rows exact: a counting bench sees every texture whole,
//!   as a capture does (the sandbox's `--draw-every`), so the texture pool's bytes and what the asset server
//!   admitted, held back and had waiting don't depend on when a read finishes; and the bytes uploaded during
//!   the walk, which did (an asset's late upload landed inside one walk in sixteen on an M5), aren't a row.
//!   The table can still hold a row under a ceiling ([`CEILINGS`]), for a row that can't be made exact.
//! - **Only what no timing moves is held.** The rows that follow the order the district's cells arrived in
//!   (the materials and vertex buffers a pass binds, [`REPORTED`]) are in `counts.txt` and the table's header
//!   says why, but no walk is held to them.
//! - **The radius.** The `ci` district is 128 m square: the desktop tiers' 256 m place all of it at the load,
//!   and the phones' 128 m place its far cells as the tour nears them, 32 entities a frame within a frame's
//!   time budget. Which frame a cell lands on came out the same in every run on an M5, at loads of 14 to 19,
//!   though a walk now and then spent its budget on a frame or two (four of the 24 walks of three runs,
//!   none of whose counts moved). A frame over the budget is timing, not a count, so it isn't a row; but a walk whose
//!   counts moved and that went over says so beside them, since a cell may have landed late.
//! - Each walk's sandbox writes `<out>/<walk>.log`, has a deadline, and the run writes what it saw as a table,
//!   `<out>/counts.txt`, which CI uploads. [`Mode::Update`] rewrites the table's rows for the walks it ran.

pub mod table;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sapphire_time::Stopwatch;

use self::table::{Counted, Reading, Table};
use crate::scenes::{CONTENT_DIR_ENV, Mode, log_tail, run_with_deadline};

/// The preset the lane walks.
pub const PRESET: &str = "ci";
/// How long a walk may take before it's stopped and fails: over the 45 minutes CI gives a walk's job.
pub const DEFAULT_WALK_TIMEOUT: Duration = Duration::from_secs(50 * 60);
/// What a ceiling row's ceiling is over what `--update` saw: five fourths.
pub const HEADROOM: (u64, u64) = (5, 4);

/// The rows held under a ceiling rather than exactly, at [`HEADROOM`] over what `--update` saw: none today, since
/// every row the walks log was made exact (see the module docs).
pub const CEILINGS: [&str; 0] = [];

/// Rows the walks log that the lane reports, in `counts.txt`, and doesn't hold, each with why: their counts
/// follow the order cells arrived in, which is timing. The streamer places a place's entities once its meshes
/// have landed, nearest first among those that have, within a frame's time budget, so the instance slots, and
/// with them the order the main view and the prepasses draw a group's draws in, follow when each read
/// finished, on the desktops at the load as on the phones mid-walk. A group binds a mesh's streams and a
/// material only when the next draw's differ, so one more or one fewer run of the same mesh or material moves
/// these rows with nothing else changed (one in 624 on the budget district's Mobile-Low). Draws, instances,
/// triangles and the pipelines set, once per group drawn, don't depend on the order, and are held.
pub const REPORTED: [(&str, &str); 2] = [
    ("materials bound", "bound when the next draw's material differs, so it follows the draws' order"),
    ("vertex buffers bound", "bound when the next draw's mesh differs, so it follows the draws' order"),
];

/// One walk of the lane: the district's tour at a tier's settings and an hour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walk {
    /// Its name in the table: the tier, then the key.
    pub name: &'static str,
    pub tier: &'static str,
    /// `--time-of-day`: dusk's key, 19:45, or night's, 23:00.
    pub time_of_day: &'static str,
}

/// The frames each walk runs: the whole tour, 372 m at the game's run speed of 6 m/s, one step a frame, and
/// the frames a 30 Hz tier takes at two steps a frame after it reach the tour's end and stand there.
pub const FRAMES: u32 = 3_720;
/// Every how many frames a walk draws: a frame in seven, every 0.7 m of the tour at a step a frame, so a
/// software GPU draws 531 of its frames. Seven shares no factor with the renderer's time-sliced work (its
/// quarters, faces and cascades every second or fourth frame), so the frames drawn see every slice.
pub const DRAW_EVERY: u32 = 7;
/// The size the walks draw at: what they count doesn't depend on it, and a software GPU's time does.
pub const SIZE: &str = "320x180";

/// The lane's walks: each tier at dusk and at night.
pub const WALKS: [Walk; 8] = [
    Walk { name: "desktop-high-dusk", tier: "desktop-high", time_of_day: "19.75" },
    Walk { name: "desktop-high-night", tier: "desktop-high", time_of_day: "23" },
    Walk { name: "desktop-low-dusk", tier: "desktop-low", time_of_day: "19.75" },
    Walk { name: "desktop-low-night", tier: "desktop-low", time_of_day: "23" },
    Walk { name: "mobile-high-dusk", tier: "mobile-high", time_of_day: "19.75" },
    Walk { name: "mobile-high-night", tier: "mobile-high", time_of_day: "23" },
    Walk { name: "mobile-low-dusk", tier: "mobile-low", time_of_day: "19.75" },
    Walk { name: "mobile-low-night", tier: "mobile-low", time_of_day: "23" },
];

impl Walk {
    /// The sandbox's arguments.
    pub fn args(&self) -> Vec<String> {
        [
            "--bench",
            &FRAMES.to_string(),
            "--draw-every",
            &DRAW_EVERY.to_string(),
            "--tier",
            self.tier,
            "--size",
            SIZE,
            "--zone",
            "district",
            "--zone-walk",
            "tour",
            "--time-of-day",
            self.time_of_day,
        ]
        .map(str::to_owned)
        .to_vec()
    }
}

/// Where the lane finds and puts things.
pub struct Paths {
    /// The sandbox executable.
    pub sandbox: PathBuf,
    /// The desktop cook with the district's pack beside it, and a release manifest naming both.
    pub content: PathBuf,
    /// The committed table.
    pub table: PathBuf,
    /// The district's generated sources, whose digest the table's header names: checked when given.
    pub sources: Option<PathBuf>,
    /// The walks' logs and what they counted; emptied first.
    pub out: PathBuf,
}

/// Runs `walks` (every walk if empty) and returns one message per row that moved, or, updating, rewrites the
/// table's rows for those walks.
pub fn run(paths: &Paths, mode: Mode, walks: &[String], timeout: Duration) -> Result<Vec<String>, String> {
    let selected = select(walks)?;
    crate::output_paths::disjoint(&paths.out, &[&paths.content, &paths.sandbox, &paths.table])?;
    if paths.out.exists() {
        fs::remove_dir_all(&paths.out)
            .map_err(|e| format!("failed to empty {}: {e}", paths.out.display()))?;
    }
    fs::create_dir_all(&paths.out).map_err(|e| format!("failed to create {}: {e}", paths.out.display()))?;
    let text = fs::read_to_string(&paths.table).or_else(|error| match (mode, error.kind()) {
        (Mode::Update, std::io::ErrorKind::NotFound) => Ok(String::new()),
        _ => Err(format!("failed to read {}: {error}", paths.table.display())),
    })?;
    let table = Table::parse(&text).map_err(|error| format!("{}: {error}", paths.table.display()))?;
    let digest = paths.sources.as_deref().map(sources_digest).transpose()?;

    let mut failures = Vec::new();
    if let Some(digest) = &digest
        && mode == Mode::Compare
        && header_digest(&table.header).is_some_and(|written| written != digest)
    {
        failures.push(format!(
            "the district's sources digest {digest}, where the table was written from {}: a change to the \
             generator re-baselines the counts (`cargo xtask counts --update`)",
            header_digest(&table.header).unwrap_or_default()
        ));
    }
    let mut seen = BTreeMap::new();
    let mut over_budget = Vec::new();
    let clock = Stopwatch::start();
    for walk in &selected {
        let walked = Stopwatch::start();
        let log = paths.out.join(format!("{}.log", walk.name));
        let mut command = Command::new(&paths.sandbox);
        command.env(CONTENT_DIR_ENV, &paths.content).args(walk.args());
        let exit = run_with_deadline(&mut command, &log, timeout)
            .map_err(|e| format!("failed to run the sandbox for {} ({}): {e}", walk.name, log.display()))?;
        let failed = |what: String| {
            format!("{}: {what}; its log, {}, ends:\n{}", walk.name, log.display(), log_tail(&log))
        };
        match exit {
            None => failures.push(failed(format!("timed out after {} s", timeout.as_secs()))),
            Some(exit) if !exit.success() => failures.push(failed(format!("the sandbox failed ({exit})"))),
            Some(_) => {
                let text =
                    fs::read_to_string(&log).map_err(|e| format!("failed to read {}: {e}", log.display()))?;
                let rows = read_log(&text);
                over_budget.push((walk.name, streamed_over_budget(&text)));
                if rows.is_empty() {
                    failures.push(failed("logged no count rows".to_owned()));
                }
                eprintln!(
                    "walked   {} {:.1} s: {} rows",
                    walk.name,
                    walked.elapsed().as_secs_f64(),
                    rows.len()
                );
                seen.extend(rows.into_iter().map(|(row, reading)| ((walk.name.to_owned(), row), reading)));
            }
        }
    }
    let names: Vec<&str> = selected.iter().map(|walk| walk.name).collect();
    let header = header(digest.as_deref().or_else(|| header_digest(&table.header)));
    // Everything the walks counted goes in `counts.txt`; the table holds all but the rows only reported.
    let counted = table::update(&Table::default(), &seen, &names, header.clone(), is_ceiling, HEADROOM);
    seen.retain(|(_, row), _| !is_reported(row));
    let written = paths.out.join("counts.txt");
    fs::write(&written, counted.format())
        .map_err(|e| format!("failed to write {}: {e}", written.display()))?;
    eprintln!(
        "{} walk(s) in {:.1} min; what they counted is in {}",
        selected.len(),
        clock.elapsed().as_secs_f64() / 60.0,
        written.display()
    );
    match mode {
        Mode::Compare => {
            let moved = table::compare(&table, &seen, &names);
            for (walk, frames) in over_budget.into_iter().filter(|&(_, frames)| frames > 0) {
                let line = format!(
                    "{walk}: the zone spent its streaming budget on {frames} frame(s), so a cell may have landed a \
                     frame late on this machine"
                );
                if moved.iter().any(|message| message.starts_with(&format!("{walk} |"))) {
                    failures.push(line);
                } else {
                    eprintln!("note     {line}; no count moved");
                }
            }
            failures.extend(moved);
        }
        Mode::Update if failures.is_empty() => {
            let updated = table::update(&table, &seen, &names, header, is_ceiling, HEADROOM);
            fs::write(&paths.table, updated.format())
                .map_err(|e| format!("failed to write {}: {e}", paths.table.display()))?;
            eprintln!("updated  {}", paths.table.display());
        }
        Mode::Update => {}
    }
    Ok(failures)
}

fn is_ceiling(row: &str) -> bool {
    CEILINGS.contains(&row)
}

fn is_reported(row: &str) -> bool {
    REPORTED.iter().any(|&(reported, _)| reported == row)
}

/// The walks `names` names, in [`WALKS`]' order: every walk if none.
fn select(names: &[String]) -> Result<Vec<Walk>, String> {
    if let Some(unknown) = names.iter().find(|name| !WALKS.iter().any(|walk| walk.name == name.as_str())) {
        let known: Vec<&str> = WALKS.iter().map(|walk| walk.name).collect();
        return Err(format!("no walk is called {unknown:?}; the walks are {}", known.join(", ")));
    }
    Ok(WALKS
        .into_iter()
        .filter(|walk| names.is_empty() || names.iter().any(|name| name == walk.name))
        .collect())
}

/// The header `--update` writes.
fn header(digest: Option<&str>) -> Vec<String> {
    let mut header = vec![
        "The benchmark district's counts (M5 X4): what CI's nightly scale lane holds each walk of the `ci`".to_owned(),
        "preset's tour to, a walk a job. A row fails on any change, up or down; an `at most` row, which none".to_owned(),
        "is today, only past its ceiling.".to_owned(),
        "Rewrite it with `cargo xtask counts --update` when a count moves on purpose, and say why in the change.".to_owned(),
        String::new(),
        format!("preset: {PRESET}"),
    ];
    if let Some(digest) = digest {
        header.push(format!(
            "sources: {digest} (the generated district and cast, FNV-1a 64 over paths, lengths and bytes)"
        ));
    }
    header.push(format!(
        "walks: --bench {FRAMES} --draw-every {DRAW_EVERY} --size {SIZE} --zone district --zone-walk tour, at each tier's settings"
    ));
    header.push("reported in counts.txt, not held (the order cells arrive in is timing):".to_owned());
    header.extend(REPORTED.iter().map(|(row, why)| format!("  {row}: {why}")));
    header
}

/// The sources digest a table's header names.
fn header_digest(header: &[String]) -> Option<&str> {
    header.iter().find_map(|line| line.strip_prefix("sources: ")?.split(' ').next())
}

/// A digest of every file under `dir`: FNV-1a 64 over each one's path from `dir`, its length and its bytes, in
/// path order. The generator writes the same bytes on every machine, so this names what it generated.
pub fn sources_digest(dir: &Path) -> Result<String, String> {
    fn files(dir: &Path, root: &Path, into: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("failed to read {}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| format!("failed to read {}: {e}", dir.display()))?.path();
            if path.is_dir() {
                files(&path, root, into)?;
            } else {
                let relative = path.strip_prefix(root).expect("under the root");
                into.push((relative.to_string_lossy().replace('\\', "/"), path));
            }
        }
        Ok(())
    }
    let mut all = Vec::new();
    files(dir, dir, &mut all)?;
    if all.is_empty() {
        return Err(format!(
            "{} holds no generated sources: `cargo xtask bench-content` makes them",
            dir.display()
        ));
    }
    all.sort();
    let mut hash = Fnv::default();
    for (relative, path) in all {
        let bytes = fs::read(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        hash.write(relative.as_bytes());
        hash.write(&(bytes.len() as u64).to_le_bytes());
        hash.write(&bytes);
    }
    Ok(format!("{:016x}", hash.0))
}

/// FNV-1a, 64 bits.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// The rows a walk logs once, from its `zone walk` and `streaming` lines: `(line, field, row)`. A pipeline built on
/// first use fails the walk itself (exit test 7), and the table holds the load's pipelines and what it placed.
pub const WALK_ROWS: [(&str, &str, &str); 8] = [
    ("zone walk", "prewarmed", "pipelines prewarmed"),
    ("zone walk", "first_use", "pipelines built on first use"),
    ("zone walk", "rebuilt", "pipelines rebuilt"),
    ("zone walk", "frame_thread", "pipelines built on the frame's thread"),
    ("zone walk", "frame_waits", "frames waiting on a pipeline"),
    ("streaming", "entities", "entities in the zone"),
    ("streaming", "placed", "entities placed"),
    ("streaming", "removed", "entities removed"),
];

/// The rows a walk's log reads: each `count row` (or `count row not exercised`), and [`WALK_ROWS`], one
/// reading each, `not exercised` at zero.
pub fn read_log(text: &str) -> BTreeMap<String, Reading> {
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        let fields = fields(line);
        let field = |name: &str| fields.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
        let number = |name: &str| field(name).and_then(|value| value.parse::<u64>().ok());
        if line.contains(" count row not exercised") {
            if let Some(row) = field("row") {
                rows.insert(row.to_owned(), Reading::NotExercised);
            }
        } else if line.contains(" count row ") {
            let (Some(row), Some(p50), Some(p95), Some(worst), Some(total)) =
                (field("row"), number("p50"), number("p95"), number("worst"), number("total"))
            else {
                continue;
            };
            rows.insert(row.to_owned(), Reading::Counted(Counted { p50, p95, worst, total }));
        } else {
            for (message, name, row) in WALK_ROWS {
                // The message ends where the fields begin: `<message> <first field>=`.
                let Some(at) = line.find(&format!(" {message} ")) else { continue };
                if line[at + message.len() + 2..].split(' ').next().is_none_or(|first| !first.contains('=')) {
                    continue;
                }
                if let Some(count) = number(name) {
                    let reading = if count == 0 {
                        Reading::NotExercised
                    } else {
                        Reading::Counted(Counted { p50: count, p95: count, worst: count, total: count })
                    };
                    rows.insert(row.to_owned(), reading);
                }
            }
        }
    }
    rows
}

/// The frames a walk's zone spent its streaming budget on, from its `zone walk` line.
fn streamed_over_budget(text: &str) -> u64 {
    text.lines()
        .filter(|line| line.contains(" zone walk "))
        .find_map(|line| {
            fields(line).into_iter().find(|(key, _)| *key == "over_budget_frames")?.1.parse().ok()
        })
        .unwrap_or(0)
}

/// A log line's `key=value` fields, a quoted value without its quotes.
fn fields(line: &str) -> Vec<(&str, &str)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_walk_log_reads_its_rows() {
        let log = "\
2026-11-14T03:00:00.000000Z  INFO sapphire::sandbox: zone walk frames=3780 streaming_us_p50=0.7 over_budget_frames=0 upload_bytes=33515424 placed=244 prewarmed=56 first_use=0 rebuilt=0 warmed_off_frame=36 frame_thread=0 frame_waits=0
2026-11-14T03:00:00.000000Z  INFO sapphire::sandbox: streaming places=16 active=16 failed=0 entities=244 placed=244 removed=0 peak_us=180.0
2026-11-14T03:00:00.000000Z  INFO sapphire::sandbox: count row phase=\"drawn\" row=\"draw calls\" frames=744 p50=1180 p95=1342 worst=1406 total=456789 worst_tick=300 worst_walked_m=\"30.0\" budget=Some(2500) over_p50=false over_worst=false
2026-11-14T03:00:00.000000Z  INFO sapphire::sandbox: count row not exercised phase=\"drawn\" row=\"lod1+ draws\"
2026-11-14T03:00:00.000000Z  INFO sapphire::sandbox: fight row not exercised phase=\"first\" row=\"script\"
2026-11-14T03:00:00.000000Z DEBUG sapphire::render: zone streaming started
";
        let rows = read_log(log);
        assert_eq!(
            rows["draw calls"],
            Reading::Counted(Counted { p50: 1180, p95: 1342, worst: 1406, total: 456789 })
        );
        assert_eq!(rows["lod1+ draws"], Reading::NotExercised);
        assert_eq!(rows["pipelines built on first use"], Reading::NotExercised);
        let once = |count| Reading::Counted(Counted { p50: count, p95: count, worst: count, total: count });
        assert_eq!(rows["pipelines prewarmed"], once(56));
        assert_eq!(rows["entities placed"], once(244), "the streaming line's, not the zone walk's");
        assert_eq!(rows["entities removed"], Reading::NotExercised);
        assert!(!rows.contains_key("script"), "a fight row isn't a count row");
        assert_eq!(rows.len(), 2 + WALK_ROWS.len());
        assert_eq!(streamed_over_budget(log), 0);
        assert_eq!(streamed_over_budget(&log.replace("over_budget_frames=0", "over_budget_frames=2")), 2);
    }

    #[test]
    fn the_rows_only_reported_are_named_in_the_header_and_nothing_held_is_among_them() {
        let header = header(Some("0123456789abcdef"));
        for (row, why) in REPORTED {
            assert!(is_reported(row) && !is_ceiling(row), "{row}");
            assert!(header.iter().any(|line| line.trim() == format!("{row}: {why}")), "{row} in {header:?}");
        }
        for held in ["draw calls", "pipelines set", "draw instances", "triangles"] {
            assert!(!is_reported(held), "{held}");
        }
        assert_eq!(header_digest(&header), Some("0123456789abcdef"));
    }

    #[test]
    fn walks_are_named_by_tier_and_key_and_selected_by_name() {
        assert_eq!(select(&[]).unwrap().len(), WALKS.len());
        let one = select(&["mobile-low-night".to_owned()]).unwrap();
        assert_eq!(one, [WALKS[7]]);
        assert!(select(&["mobile-low".to_owned()]).unwrap_err().contains("no walk is called"));
        for walk in WALKS {
            assert_eq!(
                walk.name,
                format!("{}-{}", walk.tier, if walk.time_of_day == "23" { "night" } else { "dusk" })
            );
            assert!(walk.args().windows(2).any(|pair| pair == ["--tier", walk.tier]));
        }
    }

    #[test]
    fn the_sources_digest_names_every_file() {
        let dir = std::env::temp_dir().join(format!("sapphire_counts_digest_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("district")).unwrap();
        fs::write(dir.join("district/a.ron"), "a").unwrap();
        fs::write(dir.join("cast.ron"), "b").unwrap();
        let first = sources_digest(&dir).unwrap();
        assert_eq!(sources_digest(&dir).unwrap(), first, "the same files, the same digest");
        fs::write(dir.join("district/a.ron"), "c").unwrap();
        assert_ne!(sources_digest(&dir).unwrap(), first, "a byte changed");
        fs::rename(dir.join("district/a.ron"), dir.join("district/b.ron")).unwrap();
        fs::write(dir.join("district/b.ron"), "a").unwrap();
        assert_ne!(sources_digest(&dir).unwrap(), first, "a file renamed");
        fs::remove_dir_all(&dir).unwrap();
        assert!(sources_digest(&dir).is_err());
    }
}
