//! A GPU family's differences from the counts table (M5 X4's lane on lavapipe): the rows a family counts
//! differently from the table, each as a difference from the table's figure, under the reason it differs.
//!
//! ```text
//! # A header of comment lines, up to the first blank line.
//!
//! # A reason: why this family counts these rows differently, and the code the difference comes from.
//! desktop-low-dusk | draw calls | total -120
//! desktop-low-dusk | main draws | total -48 worst -2
//! ```
//!
//! The table is taken on one machine (an M5's Metal, aarch64); a family on another architecture counts a few
//! rows differently by design (`sapphire_math::det`: results identical across architectures are a non-goal), so
//! the lane there holds each walk to the table's figure plus the difference. Holding a difference rather than a
//! second table means a change that moves a row equally everywhere is written once, in the table, and needs no
//! round trip through CI; and a new difference has to be written down here, with its reason.
//!
//! The file can't rot: a line naming a walk or row the table doesn't hold, a difference of zero, a difference that
//! would take a stat below zero, a row given twice, or a row with no reason above it in its block, fails.
//! [`refresh`] rewrites the file from what a finished run counted, keeping each line's reason, and puts a new
//! difference under [`NEEDS_A_REASON`], which fails to read until someone replaces it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::table::{Counted, Expected, Key, Reading, Table};

/// The reason [`refresh`] writes above a difference it found and the file didn't have: a file holding it fails
/// to read until the line is given its own.
pub const NEEDS_A_REASON: &str =
    "NEEDS A REASON: say why this family counts these rows differently, and from which code";

/// The stats a difference may name, in the order a line writes them.
const STATS: [&str; 4] = ["total", "worst", "p95", "p50"];

/// How a family's reading of a row differs from the table's: a signed difference for each stat that differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Delta {
    pub total: i64,
    pub worst: i64,
    pub p95: i64,
    pub p50: i64,
}

impl Delta {
    fn stats(self) -> [(&'static str, i64); 4] {
        [("total", self.total), ("worst", self.worst), ("p95", self.p95), ("p50", self.p50)]
    }

    fn is_zero(self) -> bool {
        self == Self::default()
    }

    /// `seen` less `expected`, stat by stat: not exercised counts as zero.
    pub fn between(expected: Reading, seen: Reading) -> Self {
        let (a, b) = (spread(expected), spread(seen));
        let less =
            |x: u64, y: u64| i64::try_from(x).unwrap_or(i64::MAX) - i64::try_from(y).unwrap_or(i64::MAX);
        Self {
            total: less(b.total, a.total),
            worst: less(b.worst, a.worst),
            p95: less(b.p95, a.p95),
            p50: less(b.p50, a.p50),
        }
    }

    /// `reading` with this added: `None` if a stat would go below zero. All zero is not exercised.
    pub fn apply(self, reading: Reading) -> Option<Reading> {
        let base = spread(reading);
        let add = |value: u64, by: i64| value.checked_add_signed(by);
        let counted = Counted {
            p50: add(base.p50, self.p50)?,
            p95: add(base.p95, self.p95)?,
            worst: add(base.worst, self.worst)?,
            total: add(base.total, self.total)?,
        };
        Some(if counted == Counted::default() { Reading::NotExercised } else { Reading::Counted(counted) })
    }
}

/// A reading's stats, not exercised as zeros.
fn spread(reading: Reading) -> Counted {
    match reading {
        Reading::NotExercised => Counted::default(),
        Reading::Counted(counted) => counted,
    }
}

/// A block of the file: the reason's comment lines, then the differences it explains.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Block {
    pub reason: Vec<String>,
    pub rows: BTreeMap<Key, Delta>,
}

/// The file: its header, then its blocks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Differences {
    pub header: Vec<String>,
    pub blocks: Vec<Block>,
}

impl Differences {
    /// Reads the file's text and checks each line against `table`. Every problem is an error naming its line.
    pub fn parse(text: &str, table: &Table) -> Result<Self, String> {
        Self::parse_with(text, table, false)
    }

    /// As [`Self::parse`], but a block whose reason is still [`NEEDS_A_REASON`] is read: for [`refresh`] alone,
    /// which writes the placeholder back.
    pub fn parse_owing(text: &str, table: &Table) -> Result<Self, String> {
        Self::parse_with(text, table, true)
    }

    fn parse_with(text: &str, table: &Table, owing: bool) -> Result<Self, String> {
        let mut differences = Self::default();
        let mut block: Option<Block> = None;
        let mut in_header = true;
        let mut seen = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            let error = |problem: String| format!("line {}: {problem}", index + 1);
            if line.trim().is_empty() {
                in_header = false;
                differences.blocks.extend(block.take().filter(|block| !block.rows.is_empty()));
                continue;
            }
            if let Some(comment) = line.strip_prefix('#') {
                let comment = comment.strip_prefix(' ').unwrap_or(comment).to_owned();
                if in_header {
                    differences.header.push(comment);
                    continue;
                }
                let current = block.get_or_insert_with(Block::default);
                if !current.rows.is_empty() {
                    return Err(error(
                        "a block's reason comes before its rows: leave a blank line between blocks".into(),
                    ));
                }
                current.reason.push(comment);
                continue;
            }
            in_header = false;
            let current = block.get_or_insert_with(Block::default);
            if current.reason.iter().all(|line| line.trim().is_empty()) {
                return Err(error(format!(
                    "{line:?} has no reason above it: a difference is written down with why"
                )));
            }
            if !owing && current.reason.iter().any(|line| line.trim() == NEEDS_A_REASON) {
                return Err(error(format!(
                    "{line:?} still needs its reason, in place of the line `{NEEDS_A_REASON}`"
                )));
            }
            let fields: Vec<&str> = line.split(" | ").map(str::trim).collect();
            let [walk, row, delta] = fields[..] else {
                return Err(error(format!(
                    "expected `<walk> | <row> | <stat> <difference>...`, got {line:?}"
                )));
            };
            let key = (walk.to_owned(), row.to_owned());
            let Some(expected) = table.rows.get(&key) else {
                return Err(error(format!("the table holds no row {walk} | {row}")));
            };
            let Expected::Exact(expected) = *expected else {
                return Err(error(format!(
                    "{walk} | {row} is a ceiling in the table, which takes no difference"
                )));
            };
            let delta = parse_delta(delta).map_err(error)?;
            if delta.is_zero() {
                return Err(error(format!("{walk} | {row} differs by nothing: take the line out")));
            }
            if delta.apply(expected).is_none() {
                return Err(error(format!("{walk} | {row}: the difference takes a stat below zero")));
            }
            if seen.insert(key.clone(), ()).is_some() {
                return Err(error(format!("{walk} | {row} is given twice")));
            }
            current.rows.insert(key, delta);
        }
        differences.blocks.extend(block.filter(|block| !block.rows.is_empty()));
        Ok(differences)
    }

    /// Every difference, by walk and row.
    pub fn rows(&self) -> BTreeMap<Key, Delta> {
        self.blocks
            .iter()
            .flat_map(|block| block.rows.iter().map(|(key, delta)| (key.clone(), *delta)))
            .collect()
    }

    /// The file as text: the header, then each block's reason and rows, a blank line between.
    pub fn format(&self) -> String {
        let mut text = String::new();
        let comment = |text: &mut String, line: &str| {
            if line.is_empty() {
                text.push_str("#\n");
            } else {
                let _ = writeln!(text, "# {line}");
            }
        };
        for line in &self.header {
            comment(&mut text, line);
        }
        for block in self.blocks.iter().filter(|block| !block.rows.is_empty()) {
            if !text.is_empty() {
                text.push('\n');
            }
            for line in &block.reason {
                comment(&mut text, line);
            }
            for ((walk, row), delta) in &block.rows {
                let _ = writeln!(text, "{walk} | {row} | {}", format_delta(*delta));
            }
        }
        text
    }
}

fn parse_delta(text: &str) -> Result<Delta, String> {
    let words: Vec<&str> = text.split(' ').filter(|word| !word.is_empty()).collect();
    if words.is_empty() || !words.len().is_multiple_of(2) {
        return Err(format!(
            "expected `<stat> <difference>` pairs (stats: {}), got {text:?}",
            STATS.join(", ")
        ));
    }
    let mut delta = Delta::default();
    let mut named = Vec::new();
    for pair in words.chunks(2) {
        let [stat, value] = [pair[0], pair[1]];
        let value: i64 =
            value.parse().map_err(|_| format!("expected a whole number after {stat}, got {value:?}"))?;
        if value == 0 {
            return Err(format!("{stat} differs by 0: leave it out"));
        }
        let slot = match stat {
            "total" => &mut delta.total,
            "worst" => &mut delta.worst,
            "p95" => &mut delta.p95,
            "p50" => &mut delta.p50,
            other => return Err(format!("no stat is called {other:?}; the stats are {}", STATS.join(", "))),
        };
        if named.contains(&stat) {
            return Err(format!("{stat} is given twice"));
        }
        named.push(stat);
        *slot = value;
    }
    Ok(delta)
}

fn format_delta(delta: Delta) -> String {
    let parts: Vec<String> = delta
        .stats()
        .into_iter()
        .filter(|&(_, value)| value != 0)
        .map(|(stat, value)| format!("{stat} {value:+}"))
        .collect();
    parts.join(" ")
}

/// The table a family on which `differences` holds expects: each row with a difference, at the table's figure plus
/// it. [`Differences::parse`] checked that each applies.
pub fn apply(table: &Table, differences: &Differences) -> Table {
    let mut expected = table.clone();
    for (key, delta) in differences.rows() {
        if let Some(Expected::Exact(reading)) = expected.rows.get(&key).cloned()
            && let Some(applied) = delta.apply(reading)
        {
            expected.rows.insert(key, Expected::Exact(applied));
        }
    }
    expected
}

/// `old` rewritten from what a finished run counted (`seen`, each walk's `counts.txt` read back) for `walks`: each of
/// those walks' rows held exactly (not `reported`) that differs from the table, as a difference. Every other walk's
/// lines are kept. A line that still differs keeps its block and reason; one that no longer differs goes; a new one
/// goes into a block under [`NEEDS_A_REASON`]. Returns the file and one message per new line.
///
/// Fails if the run's rows aren't the table's: a row the table lacks or the run didn't log is the table's to fix,
/// on the machine that takes it (`cargo xtask counts --update`), not a difference.
pub fn refresh(
    table: &Table,
    old: &Differences,
    seen: &BTreeMap<Key, Reading>,
    walks: &[&str],
    reported: impl Fn(&str) -> bool,
) -> Result<(Differences, Vec<String>), String> {
    let mut problems = Vec::new();
    let mut found = BTreeMap::new();
    for ((walk, row), expected) in table.rows.iter().filter(|((walk, _), _)| walks.contains(&walk.as_str())) {
        let key = (walk.clone(), row.clone());
        let Some(&reading) = seen.get(&key) else {
            problems.push(format!("{walk} | {row}: in the table, but the run counted no such row"));
            continue;
        };
        let Expected::Exact(expected) = *expected else { continue };
        let delta = Delta::between(expected, reading);
        if !delta.is_zero() {
            found.insert(key, delta);
        }
    }
    for (walk, row) in seen.keys().filter(|(walk, row)| walks.contains(&walk.as_str()) && !reported(row)) {
        if !table.rows.contains_key(&(walk.clone(), row.clone())) {
            problems.push(format!("{walk} | {row}: counted by the run, but not in the table"));
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "the run's rows aren't the table's; update the table first (`cargo xtask counts --update`):\n  {}",
            problems.join("\n  ")
        ));
    }
    let ours = |key: &Key| walks.contains(&key.0.as_str());
    let mut blocks: Vec<Block> = old
        .blocks
        .iter()
        .map(|block| Block {
            reason: block.reason.clone(),
            rows: block
                .rows
                .iter()
                .filter_map(|(key, delta)| {
                    if ours(key) {
                        found.remove(key).map(|delta| (key.clone(), delta))
                    } else {
                        Some((key.clone(), *delta))
                    }
                })
                .collect(),
        })
        .collect();
    let new: Vec<String> = found
        .iter()
        .map(|((walk, row), delta)| format!("{walk} | {row} | {}", format_delta(*delta)))
        .collect();
    if !found.is_empty() {
        blocks.push(Block { reason: vec![NEEDS_A_REASON.to_owned()], rows: found });
    }
    blocks.retain(|block| !block.rows.is_empty());
    Ok((Differences { header: old.header.clone(), blocks }, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
# The counts.

dh | draw calls | p50 10 p95 12 worst 14 total 100
dh | culled by view | p50 3 p95 4 worst 5 total 30
dh | lod1+ draws | not exercised
dh | bytes admitted | at most total 500 (measured 380 to 400)
dl | draw calls | p50 9 p95 11 worst 13 total 90
";

    const DIFFERENCES: &str = "\
# lavapipe's differences.

# The crowd's arithmetic.
dh | draw calls | total -5 worst +1
dh | culled by view | total 5
";

    fn table() -> Table {
        Table::parse(TABLE).unwrap()
    }

    fn key(walk: &str, row: &str) -> Key {
        (walk.to_owned(), row.to_owned())
    }

    fn counted(p50: u64, p95: u64, worst: u64, total: u64) -> Reading {
        Reading::Counted(Counted { p50, p95, worst, total })
    }

    fn exact(table: &Table, walk: &str, row: &str) -> Reading {
        match table.rows[&key(walk, row)] {
            Expected::Exact(reading) => reading,
            Expected::Ceiling { .. } => panic!("a ceiling"),
        }
    }

    /// A row with a difference is expected at the table's figure plus it; a row without one at the table's.
    #[test]
    fn the_differences_apply_to_their_rows_alone() {
        let table = table();
        let differences = Differences::parse(DIFFERENCES, &table).unwrap();
        assert_eq!(differences.header, ["lavapipe's differences."]);
        assert_eq!(differences.blocks.len(), 1);
        assert_eq!(differences.blocks[0].reason, ["The crowd's arithmetic."]);
        let expected = apply(&table, &differences);
        assert_eq!(exact(&expected, "dh", "draw calls"), counted(10, 12, 15, 95));
        assert_eq!(exact(&expected, "dh", "culled by view"), counted(3, 4, 5, 35));
        assert_eq!(
            exact(&expected, "dl", "draw calls"),
            counted(9, 11, 13, 90),
            "no difference, the table's"
        );
        assert_eq!(expected.rows[&key("dh", "bytes admitted")], table.rows[&key("dh", "bytes admitted")]);
        assert_eq!(
            Differences::parse(&differences.format(), &table).unwrap(),
            differences,
            "reads back the same"
        );
    }

    /// A line naming a walk or a row the table doesn't hold fails, and so does a difference of nothing.
    #[test]
    fn a_line_the_table_does_not_hold_or_a_zero_difference_fails() {
        let table = table();
        let with = |line: &str| Differences::parse(&format!("# header\n\n# why\n{line}\n"), &table);
        assert!(
            with("dx | draw calls | total -1")
                .unwrap_err()
                .contains("the table holds no row dx | draw calls")
        );
        assert!(with("dh | no such row | total -1").unwrap_err().contains("the table holds no row"));
        assert!(with("dh | draw calls | total 0").unwrap_err().contains("differs by 0"));
        assert!(with("dh | draw calls | total -101").unwrap_err().contains("below zero"));
        assert!(with("dh | bytes admitted | total 1").unwrap_err().contains("ceiling"));
        assert!(with("dh | draw calls | total -1\ndh | draw calls | worst 1").unwrap_err().contains("twice"));
        assert!(with("dh | draw calls | frames 1").unwrap_err().contains("no stat is called"));
        // Not exercised in the table, counted by the family: a difference from zero.
        let differences = with("dh | lod1+ draws | total 3 worst 1 p95 1").unwrap();
        assert_eq!(exact(&apply(&table, &differences), "dh", "lod1+ draws"), counted(0, 1, 1, 3));
    }

    /// Every difference is written down with a reason: a row with none above it in its block fails, and so does
    /// one under the refresh's placeholder.
    #[test]
    fn a_difference_without_its_reason_fails() {
        let table = table();
        let bare = Differences::parse("# header\n\ndh | draw calls | total -1\n", &table).unwrap_err();
        assert!(bare.contains("no reason above it"), "{bare}");
        let placeholder = Differences::parse(
            &format!("# header\n\n# {NEEDS_A_REASON}\ndh | draw calls | total -1\n"),
            &table,
        )
        .unwrap_err();
        assert!(placeholder.contains("still needs its reason"), "{placeholder}");
    }

    /// A refresh from what a run counted keeps a line's reason where it still differs, drops one that no longer
    /// does, leaves other walks' lines, and puts a new one under the placeholder, saying so.
    #[test]
    fn a_refresh_from_a_run_keeps_the_reasons() {
        let table = table();
        let old = Differences::parse(DIFFERENCES, &table).unwrap();
        let mut seen = BTreeMap::new();
        // dh's draw calls differ by a new amount; its culled by view no longer differs; its lod1+ draws now do.
        seen.insert(key("dh", "draw calls"), counted(10, 12, 14, 93));
        seen.insert(key("dh", "culled by view"), counted(3, 4, 5, 30));
        seen.insert(key("dh", "lod1+ draws"), counted(1, 1, 1, 2));
        seen.insert(key("dh", "bytes admitted"), counted(1, 1, 1, 400));
        seen.insert(key("dh", "materials bound"), counted(1, 1, 1, 7));
        let reported = |row: &str| row == "materials bound";
        let (fresh, new) = refresh(&table, &old, &seen, &["dh"], reported).unwrap();
        assert_eq!(fresh.header, old.header);
        assert_eq!(fresh.blocks[0].reason, ["The crowd's arithmetic."]);
        assert_eq!(
            fresh.blocks[0].rows,
            BTreeMap::from([(key("dh", "draw calls"), Delta { total: -7, ..Delta::default() })])
        );
        assert_eq!(fresh.blocks[1].reason, [NEEDS_A_REASON]);
        assert_eq!(new, ["dh | lod1+ draws | total +2 worst +1 p95 +1 p50 +1"]);
        assert!(Differences::parse(&fresh.format(), &table).unwrap_err().contains("still needs its reason"));

        // A walk the run didn't count keeps its lines; a run whose rows aren't the table's is refused.
        let (kept, _) = refresh(&table, &old, &BTreeMap::new(), &[], reported).unwrap();
        assert_eq!(kept, old);
        seen.remove(&key("dh", "culled by view"));
        assert!(refresh(&table, &old, &seen, &["dh"], reported).unwrap_err().contains("no such row"));
    }
}
