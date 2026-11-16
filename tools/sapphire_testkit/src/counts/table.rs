//! The counts table (`tests/counts/district-ci.txt`): what each of the lane's walks counted, a row to a line,
//! and how a walk's readings compare with it.
//!
//! ```text
//! # A header of comment lines, which `--update` writes: the preset, the district's sources and the command.
//! desktop-high-dusk | draw calls | p50 1180 p95 1342 worst 1406 total 456789
//! desktop-high-dusk | lod1+ draws | not exercised
//! mobile-low-night | bytes admitted | at most total 4194304 (measured 3001234 to 3001234)
//! ```
//!
//! Each line is a walk, a row the walk bench logs (`count row`, or one of the load's pipelines), and what it
//! must read. **An exact row** names its 50th and 95th percentile, its worst frame and its total over the
//! frames drawn, and any of them moving, up or down, fails it: a count that goes down on purpose is written
//! down in the same change. **A ceiling** holds a row that depends on timing (see [`super::CEILINGS`]) to at
//! most a total, with the readings it was set from beside it. A row the walk reads zero of on every frame is
//! `not exercised`, which is exact too.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// What a walk read of a row, from its log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Every frame drawn read zero.
    NotExercised,
    Counted(Counted),
}

/// A row's spread over the frames a walk drew.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counted {
    pub p50: u64,
    pub p95: u64,
    pub worst: u64,
    pub total: u64,
}

impl Reading {
    /// Its total: zero when nothing was counted.
    fn total(self) -> u64 {
        match self {
            Self::NotExercised => 0,
            Self::Counted(counted) => counted.total,
        }
    }
}

/// What a line of the table holds a row to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expected {
    /// Exactly this.
    Exact(Reading),
    /// A total of at most `total`, with the totals it was set from, least and most.
    Ceiling { total: u64, measured: (u64, u64) },
}

/// A walk's row: `(walk, row)`.
pub type Key = (String, String);

/// The table: its header's comment lines, and each row's line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub header: Vec<String>,
    pub rows: BTreeMap<Key, Expected>,
}

/// How a line of a table that can't be read is wrong.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {problem}")]
pub struct TableError {
    pub line: usize,
    pub problem: String,
}

impl Table {
    /// Reads a table's text. A line that isn't a comment, blank, or `<walk> | <row> | <expected>` is an error
    /// naming it, as is a row given twice.
    pub fn parse(text: &str) -> Result<Self, TableError> {
        let mut table = Self::default();
        let mut rows_begun = false;
        for (index, line) in text.lines().enumerate() {
            let error = |problem: String| TableError { line: index + 1, problem };
            if let Some(comment) = line.strip_prefix('#') {
                if !rows_begun {
                    table.header.push(comment.strip_prefix(' ').unwrap_or(comment).to_owned());
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            rows_begun = true;
            let fields: Vec<&str> = line.split(" | ").map(str::trim).collect();
            let [walk, row, expected] = fields[..] else {
                return Err(error(format!("expected `<walk> | <row> | <counts>`, got {line:?}")));
            };
            if walk.is_empty() || row.is_empty() {
                return Err(error(format!("a walk and a row name are needed, got {line:?}")));
            }
            let expected = parse_expected(expected).map_err(error)?;
            if table.rows.insert((walk.to_owned(), row.to_owned()), expected).is_some() {
                return Err(error(format!("{walk} | {row} is given twice")));
            }
        }
        Ok(table)
    }

    /// The table as text, rows in order of walk and row name, so a re-baseline's diff shows only what moved.
    pub fn format(&self) -> String {
        let mut text = String::new();
        for line in &self.header {
            if line.is_empty() {
                text.push_str("#\n");
            } else {
                let _ = writeln!(text, "# {line}");
            }
        }
        let mut walk = None;
        if !self.header.is_empty() {
            text.push('\n');
        }
        for ((name, row), expected) in &self.rows {
            if walk.is_some_and(|walk| walk != name) {
                text.push('\n');
            }
            walk = Some(name);
            let _ = writeln!(text, "{name} | {row} | {}", format_expected(expected));
        }
        text
    }
}

fn parse_expected(text: &str) -> Result<Expected, String> {
    if text == "not exercised" {
        return Ok(Expected::Exact(Reading::NotExercised));
    }
    let number = |word: Option<&str>, what: &str| -> Result<u64, String> {
        word.and_then(|word| word.parse().ok())
            .ok_or_else(|| format!("expected {what} as a whole number in {text:?}"))
    };
    if let Some(rest) = text.strip_prefix("at most total ") {
        let (total, measured) = rest
            .split_once(" (measured ")
            .and_then(|(total, measured)| Some((total, measured.strip_suffix(')')?)))
            .ok_or_else(|| format!("expected `at most total <n> (measured <n> to <n>)`, got {text:?}"))?;
        let (least, most) = measured
            .split_once(" to ")
            .ok_or_else(|| format!("expected `<n> to <n>` measured, got {text:?}"))?;
        return Ok(Expected::Ceiling {
            total: number(Some(total), "the ceiling")?,
            measured: (number(Some(least), "the least measured")?, number(Some(most), "the most measured")?),
        });
    }
    let words: Vec<&str> = text.split(' ').collect();
    let [p50, a, p95, b, worst, c, total, d] = words[..] else {
        return Err(format!(
            "expected `p50 <n> p95 <n> worst <n> total <n>`, `not exercised` or a ceiling, got {text:?}"
        ));
    };
    if [p50, p95, worst, total] != ["p50", "p95", "worst", "total"] {
        return Err(format!("expected `p50 <n> p95 <n> worst <n> total <n>`, got {text:?}"));
    }
    Ok(Expected::Exact(Reading::Counted(Counted {
        p50: number(Some(a), "p50")?,
        p95: number(Some(b), "p95")?,
        worst: number(Some(c), "worst")?,
        total: number(Some(d), "total")?,
    })))
}

fn format_expected(expected: &Expected) -> String {
    match expected {
        Expected::Exact(Reading::NotExercised) => "not exercised".to_owned(),
        Expected::Exact(Reading::Counted(Counted { p50, p95, worst, total })) => {
            format!("p50 {p50} p95 {p95} worst {worst} total {total}")
        }
        Expected::Ceiling { total, measured: (least, most) } => {
            format!("at most total {total} (measured {least} to {most})")
        }
    }
}

/// One line for each row of `walks` that moved against `table`: an exact row whose readings changed, each
/// stat with its expected value, the value seen and which way it went; a ceiling passed; a row the table has
/// and the walk didn't log; a row the walk logged and the table hasn't. Rows of other walks are left alone, so
/// a run of some walks compares only theirs.
pub fn compare(table: &Table, seen: &BTreeMap<Key, Reading>, walks: &[&str]) -> Vec<String> {
    let mut moved = Vec::new();
    let ours = |walk: &str| walks.contains(&walk);
    for ((walk, row), expected) in table.rows.iter().filter(|((walk, _), _)| ours(walk)) {
        let key = (walk.clone(), row.clone());
        let Some(&reading) = seen.get(&key) else {
            moved.push(format!("{walk} | {row}: in the table, but the walk logged no such row"));
            continue;
        };
        match expected {
            Expected::Exact(expected) => {
                if let Some(changes) = changes(*expected, reading) {
                    moved.push(format!("{walk} | {row}: {changes}"));
                }
            }
            Expected::Ceiling { total, .. } => {
                if reading.total() > *total {
                    moved.push(format!(
                        "{walk} | {row}: total {} is over its ceiling of {total} (up by {})",
                        reading.total(),
                        reading.total() - total
                    ));
                }
            }
        }
    }
    for (walk, row) in seen.keys().filter(|(walk, _)| ours(walk)) {
        if !table.rows.contains_key(&(walk.clone(), row.clone())) {
            moved.push(format!("{walk} | {row}: logged by the walk, but not in the table"));
        }
    }
    moved
}

/// How `seen` differs from `expected`, stat by stat: `None` if it doesn't.
fn changes(expected: Reading, seen: Reading) -> Option<String> {
    let direction = |from: u64, to: u64| if to > from { "up" } else { "down" };
    match (expected, seen) {
        (expected, seen) if expected == seen => None,
        (Reading::NotExercised, Reading::Counted(seen)) => {
            Some(format!("not exercised, now counted (up): total {}, worst {}", seen.total, seen.worst))
        }
        (Reading::Counted(expected), Reading::NotExercised) => {
            Some(format!("total {}, now not exercised (down)", expected.total))
        }
        (Reading::Counted(expected), Reading::Counted(seen)) => {
            let stats = [
                ("total", expected.total, seen.total),
                ("worst", expected.worst, seen.worst),
                ("p95", expected.p95, seen.p95),
                ("p50", expected.p50, seen.p50),
            ];
            let changed: Vec<String> = stats
                .into_iter()
                .filter(|(_, from, to)| from != to)
                .map(|(stat, from, to)| format!("{stat} {from} -> {to} ({})", direction(from, to)))
                .collect();
            Some(changed.join(", "))
        }
        (Reading::NotExercised, Reading::NotExercised) => None,
    }
}

/// The table `--update` writes for `walks` from what they read: every other walk's rows kept as they were, an
/// exact row as seen, and a ceiling row (`is_ceiling`) at `headroom` times its total, rounded up, measured at
/// what was seen.
pub fn update(
    table: &Table,
    seen: &BTreeMap<Key, Reading>,
    walks: &[&str],
    header: Vec<String>,
    is_ceiling: impl Fn(&str) -> bool,
    headroom: (u64, u64),
) -> Table {
    let mut rows: BTreeMap<Key, Expected> = table
        .rows
        .iter()
        .filter(|((walk, _), _)| !walks.contains(&walk.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for ((walk, row), reading) in seen.iter().filter(|((walk, _), _)| walks.contains(&walk.as_str())) {
        let expected = if is_ceiling(row) {
            let total = reading.total();
            let (times, over) = headroom;
            Expected::Ceiling { total: (total * times).div_ceil(over), measured: (total, total) }
        } else {
            Expected::Exact(*reading)
        };
        rows.insert((walk.clone(), row.clone()), expected);
    }
    Table { header, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counted(p50: u64, p95: u64, worst: u64, total: u64) -> Reading {
        Reading::Counted(Counted { p50, p95, worst, total })
    }

    fn key(walk: &str, row: &str) -> Key {
        (walk.to_owned(), row.to_owned())
    }

    const TABLE: &str = "\
# The counts.
#
# preset: ci

dh | draw calls | p50 10 p95 12 worst 14 total 100
dh | lod1+ draws | not exercised
dh | bytes admitted | at most total 500 (measured 380 to 400)

ml | draw calls | p50 5 p95 6 worst 7 total 50
";

    fn seen() -> BTreeMap<Key, Reading> {
        BTreeMap::from([
            (key("dh", "draw calls"), counted(10, 12, 14, 100)),
            (key("dh", "lod1+ draws"), Reading::NotExercised),
            (key("dh", "bytes admitted"), counted(1, 2, 3, 420)),
            (key("ml", "draw calls"), counted(5, 6, 7, 50)),
        ])
    }

    #[test]
    fn a_table_reads_and_writes_back_the_same() {
        let table = Table::parse(TABLE).unwrap();
        assert_eq!(table.header, ["The counts.", "", "preset: ci"]);
        assert_eq!(table.rows[&key("dh", "draw calls")], Expected::Exact(counted(10, 12, 14, 100)));
        assert_eq!(
            table.rows[&key("dh", "bytes admitted")],
            Expected::Ceiling { total: 500, measured: (380, 400) }
        );
        let text = table.format();
        assert_eq!(Table::parse(&text).unwrap(), table);
        assert!(text.starts_with("# The counts.\n#\n# preset: ci\n\ndh | bytes admitted |"), "{text}");
        assert!(text.contains("not exercised\n\nml | draw calls"), "a blank line between walks: {text}");
    }

    #[test]
    fn the_same_counts_move_nothing() {
        let table = Table::parse(TABLE).unwrap();
        assert_eq!(compare(&table, &seen(), &["dh", "ml"]), Vec::<String>::new());
    }

    /// A count up or down fails alike, naming the row, the stat, the value expected, the value seen and the
    /// direction: going down on purpose is written down too.
    #[test]
    fn a_row_up_or_down_moves() {
        let table = Table::parse(TABLE).unwrap();
        let mut up = seen();
        up.insert(key("dh", "draw calls"), counted(10, 12, 15, 101));
        assert_eq!(
            compare(&table, &up, &["dh", "ml"]),
            ["dh | draw calls: total 100 -> 101 (up), worst 14 -> 15 (up)"]
        );
        let mut down = seen();
        down.insert(key("ml", "draw calls"), counted(4, 6, 7, 49));
        assert_eq!(
            compare(&table, &down, &["dh", "ml"]),
            ["ml | draw calls: total 50 -> 49 (down), p50 5 -> 4 (down)"]
        );
        let mut exercised = seen();
        exercised.insert(key("dh", "lod1+ draws"), counted(0, 1, 2, 9));
        assert_eq!(
            compare(&table, &exercised, &["dh"]),
            ["dh | lod1+ draws: not exercised, now counted (up): total 9, worst 2"]
        );
    }

    /// A ceiling holds whatever it's at most of, and fails only past it.
    #[test]
    fn a_ceiling_holds_until_it_is_passed() {
        let table = Table::parse(TABLE).unwrap();
        let mut under = seen();
        under.insert(key("dh", "bytes admitted"), counted(9, 9, 9, 500));
        assert!(compare(&table, &under, &["dh"]).is_empty(), "at the ceiling holds");
        under.insert(key("dh", "bytes admitted"), Reading::NotExercised);
        assert!(compare(&table, &under, &["dh"]).is_empty(), "nothing at all holds");
        let mut over = seen();
        over.insert(key("dh", "bytes admitted"), counted(9, 9, 9, 501));
        assert_eq!(
            compare(&table, &over, &["dh"]),
            ["dh | bytes admitted: total 501 is over its ceiling of 500 (up by 1)"]
        );
    }

    #[test]
    fn a_missing_or_unknown_row_moves() {
        let table = Table::parse(TABLE).unwrap();
        let mut missing = seen();
        missing.remove(&key("ml", "draw calls"));
        assert_eq!(
            compare(&table, &missing, &["dh", "ml"]),
            ["ml | draw calls: in the table, but the walk logged no such row"]
        );
        let mut unknown = seen();
        unknown.insert(key("dh", "joints"), counted(1, 1, 1, 4));
        assert_eq!(
            compare(&table, &unknown, &["dh", "ml"]),
            ["dh | joints: logged by the walk, but not in the table"]
        );
        assert!(compare(&table, &unknown, &["ml"]).is_empty(), "another walk's rows are left alone");
        let mut walk = seen();
        walk.retain(|(walk, _), _| walk != "ml");
        assert_eq!(
            compare(&table, &walk, &["ml"]),
            ["ml | draw calls: in the table, but the walk logged no such row"]
        );
    }

    #[test]
    fn a_malformed_table_is_an_error_naming_its_line() {
        for (text, line, problem) in [
            ("dh | draw calls\n", 1, "expected `<walk> | <row> | <counts>`"),
            (
                "# a header\ndh | draw calls | p50 1 p95 2 worst 3\n",
                2,
                "expected `p50 <n> p95 <n> worst <n> total <n>`",
            ),
            ("dh | draw calls | p50 1 p95 2 worst x total 4\n", 1, "worst as a whole number"),
            ("dh | draw calls | p50 1 p96 2 worst 3 total 4\n", 1, "expected `p50"),
            ("dh | a | not exercised\ndh | a | not exercised\n", 2, "given twice"),
            ("dh | a | at most total 5\n", 1, "measured"),
            ("dh | a | at most total 5 (measured 1 and 2)\n", 1, "<n> to <n>"),
            (" | a | not exercised\n", 1, "a walk and a row name"),
        ] {
            let error = Table::parse(text).expect_err(text);
            assert_eq!(error.line, line, "{text}");
            assert!(error.problem.contains(problem), "{text}: {}", error.problem);
        }
    }

    /// An update rewrites the walks it ran and keeps the rest, and sets a ceiling with headroom over what it saw.
    #[test]
    fn an_update_rewrites_the_walks_it_ran() {
        let table = Table::parse(TABLE).unwrap();
        let mut now = seen();
        now.insert(key("dh", "draw calls"), counted(11, 12, 14, 110));
        now.insert(key("ml", "draw calls"), counted(1, 1, 1, 1));
        let updated =
            update(&table, &now, &["dh"], table.header.clone(), |row| row == "bytes admitted", (5, 4));
        assert_eq!(updated.rows[&key("dh", "draw calls")], Expected::Exact(counted(11, 12, 14, 110)));
        assert_eq!(
            updated.rows[&key("ml", "draw calls")],
            Expected::Exact(counted(5, 6, 7, 50)),
            "not run: kept"
        );
        assert_eq!(
            updated.rows[&key("dh", "bytes admitted")],
            Expected::Ceiling { total: 525, measured: (420, 420) }
        );
        assert!(compare(&updated, &now, &["dh"]).is_empty());
    }
}
