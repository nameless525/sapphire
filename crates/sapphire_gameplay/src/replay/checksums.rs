//! What a replay must reach: a run's checksums as a run makes them ([`Trail`]), and as they're kept
//! per architecture in `tests/replays/` ([`ChecksumFile`], which has the format).

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

/// The architecture this build runs on, as Rust names it (`std::env::consts::ARCH`): `x86_64` or
/// `aarch64`. Checksums are per architecture (spec §9, gate 2).
pub const ARCHITECTURE: &str = std::env::consts::ARCH;

/// Ticks between checkpoints with all 64 bits.
pub const CHECKPOINT_EVERY: u32 = 60;

/// How much of a trail a run keeps room for up front, so stepping doesn't reallocate while an app
/// draws: 18 minutes. Longer runs grow as they go, and a crafted tick count reserves no more.
pub(crate) const TRAIL_RESERVE: usize = 1 << 16;

/// The file holding `architecture`'s checksums, in a replay directory.
#[must_use]
pub fn checksums_file_name(architecture: &str) -> String {
    format!("checksums.{architecture}.txt")
}

/// A run's checksum at every tick so far, from tick 0, the starting world.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trail {
    checksums: Vec<u64>,
}

impl Trail {
    pub(crate) fn new(start: u64, expected_ticks: u32) -> Self {
        let room = (expected_ticks as usize).saturating_add(1).min(TRAIL_RESERVE);
        let mut checksums = Vec::with_capacity(room);
        checksums.push(start);
        Self { checksums }
    }

    pub(crate) fn push(&mut self, checksum: u64) {
        self.checksums.push(checksum);
    }

    /// Ticks run.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        (self.checksums.len() - 1) as u32
    }

    /// The checksum at `tick`, if the run got that far.
    #[must_use]
    pub fn at(&self, tick: u32) -> Option<u64> {
        self.checksums.get(tick as usize).copied()
    }

    /// The checksum at the last tick run.
    #[must_use]
    pub fn end(&self) -> u64 {
        *self.checksums.last().expect("a trail starts with the starting world")
    }
}

fn fingerprint(checksum: u64) -> u16 {
    checksum as u16
}

/// One line of a [`RecordedTrail`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct Checkpoint {
    tick: u32,
    checksum: u64,
    /// One per tick since the checkpoint before, up to and not including this one.
    fingerprints: Vec<u16>,
}

/// A replay's checksums as a [`ChecksumFile`] keeps them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTrail {
    ticks: u32,
    checkpoints: Vec<Checkpoint>,
}

/// Where a run left its recorded trail: see [`RecordedTrail::first_difference`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divergence {
    /// The first tick that differs: `tick` steps had run. The actions of tick `tick - 1`, counting
    /// from 0, went in last, and tick 0 is the scenario's own starting world.
    pub tick: u32,
    /// What was recorded there, and what this run has, in as many bits as were recorded.
    pub recorded: u64,
    pub actual: u64,
    /// Whether all 64 bits were recorded there, or a fingerprint's 16.
    pub whole: bool,
}

impl fmt::Display for Divergence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { tick, recorded, actual, whole } = *self;
        write!(f, "it first differs at tick {tick}: ")?;
        if whole {
            write!(f, "recorded {recorded:016x}, this run {actual:016x}")
        } else {
            write!(f, "recorded …{recorded:04x}, this run …{actual:04x}")
        }
    }
}

impl RecordedTrail {
    /// A run's trail as the file would hold it, whatever its length.
    #[must_use]
    pub fn from_trail(trail: &Trail) -> Self {
        let ticks = trail.ticks();
        let mut checkpoints = Vec::with_capacity((ticks / CHECKPOINT_EVERY + 2) as usize);
        let mut previous = None;
        let mut tick = 0;
        loop {
            let first = previous.map_or(0, |previous: u32| previous + 1);
            let fingerprints =
                (first..tick).map(|tick| fingerprint(trail.at(tick).expect("within the trail"))).collect();
            let checksum = trail.at(tick).expect("within the trail");
            checkpoints.push(Checkpoint { tick, checksum, fingerprints });
            if tick == ticks {
                return Self { ticks, checkpoints };
            }
            previous = Some(tick);
            tick = (tick + CHECKPOINT_EVERY).min(ticks);
        }
    }

    /// How many ticks the recorded run has.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    /// The checksum at the recorded run's last tick.
    #[must_use]
    pub fn end(&self) -> u64 {
        self.checkpoints.last().map_or(0, |checkpoint| checkpoint.checksum)
    }

    /// The first tick where `trail` isn't what's recorded, comparing every tick both have. So a run
    /// cut short, such as a capture at an earlier tick, is held to as far as it got; the caller
    /// compares [`Self::ticks`] if it wants the whole run.
    #[must_use]
    pub fn first_difference(&self, trail: &Trail) -> Option<Divergence> {
        for checkpoint in &self.checkpoints {
            let first = checkpoint.tick - checkpoint.fingerprints.len() as u32;
            for (tick, &recorded) in (first..).zip(&checkpoint.fingerprints) {
                let actual = trail.at(tick)?;
                if fingerprint(actual) != recorded {
                    return Some(Divergence {
                        tick,
                        recorded: u64::from(recorded),
                        actual: u64::from(fingerprint(actual)),
                        whole: false,
                    });
                }
            }
            let actual = trail.at(checkpoint.tick)?;
            if actual != checkpoint.checksum {
                return Some(Divergence {
                    tick: checkpoint.tick,
                    recorded: checkpoint.checksum,
                    actual,
                    whole: true,
                });
            }
        }
        None
    }
}

/// A checksum file that doesn't read. Reading never panics, and allocates only as the text pays for
/// it.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {problem}")]
pub struct ChecksumsError {
    pub line: usize,
    pub problem: String,
}

/// Every replay's checksums for one architecture: one file in `tests/replays/`.
///
/// # The file
///
/// It's text, so a re-record reviews like a golden, in a diff:
///
/// ```text
/// # comments
/// architecture aarch64
///
/// replay melee-sweep 1200
/// 0 d1e8a770c2add49f
/// 60 6319e4723dd39429 a1b2c3d4…
/// …
/// 1200 2a4877fd9e24ca60 e5f60718…
/// ```
///
/// Each `replay` holds its name and tick count, then checkpoints: a tick, that tick's whole checksum,
/// and the low 16 bits of each tick's since the checkpoint before, as 4 hex digits each. Tick 0 is
/// the starting world and tick N the world after N steps, and the first and last ticks are
/// always checkpoints. So a run that differs names the first tick it differs at, not only the end of
/// a window, for about 260 bytes per second of play. Fingerprints collide 1 time in 65,536: then the
/// tick named is a later one. Nothing is missed for good, since a difference stays in the world and
/// the next checkpoint holds all 64 bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumFile {
    architecture: String,
    replays: BTreeMap<String, RecordedTrail>,
}

impl ChecksumFile {
    /// A file with nothing recorded.
    #[must_use]
    pub fn new(architecture: &str) -> Self {
        Self { architecture: architecture.to_owned(), replays: BTreeMap::new() }
    }

    /// The architecture it says it's for.
    #[must_use]
    pub fn architecture(&self) -> &str {
        &self.architecture
    }

    /// What's recorded for a replay.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&RecordedTrail> {
        self.replays.get(name)
    }

    /// Records a replay's trail, in place of any before it.
    pub fn set(&mut self, name: &str, trail: RecordedTrail) {
        self.replays.insert(name.to_owned(), trail);
    }

    /// Forgets a replay. Returns whether it was there.
    pub fn remove(&mut self, name: &str) -> bool {
        self.replays.remove(name).is_some()
    }

    /// Every replay with something recorded, by name.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.replays.keys().map(String::as_str)
    }

    /// The file's text. One state has one text: replays by name, always the same layout.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = String::from(
            "# What each replay in this directory reaches on this architecture: written by\n\
             # `cargo xtask replay --update`, and reviewed like a golden. Each line under a replay is a\n\
             # tick, its checksum, and the low 16 bits of each tick's checksum since the line before.\n",
        );
        let _ = writeln!(text, "architecture {}", self.architecture);
        for (name, trail) in &self.replays {
            let _ = write!(text, "\nreplay {name} {}\n", trail.ticks);
            for checkpoint in &trail.checkpoints {
                let _ = write!(text, "{} {:016x}", checkpoint.tick, checkpoint.checksum);
                if !checkpoint.fingerprints.is_empty() {
                    text.push(' ');
                    for fingerprint in &checkpoint.fingerprints {
                        let _ = write!(text, "{fingerprint:04x}");
                    }
                }
                text.push('\n');
            }
        }
        text
    }

    /// Reads a file's text, refusing anything [`Self::to_text`] wouldn't write, line endings aside.
    pub fn parse(text: &str) -> Result<Self, ChecksumsError> {
        let mut file: Option<Self> = None;
        // The replay being read, and what its next line must carry on from.
        let mut open: Option<(String, RecordedTrail)> = None;
        let mut last_line = 0;
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            last_line = number;
            let fail = |problem: String| ChecksumsError { line: number, problem };
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split(' ');
            let first = words.next().unwrap_or_default();
            let Some(current) = file.as_mut() else {
                let architecture = (first == "architecture").then(|| words.next()).flatten();
                let Some(architecture) = architecture.filter(|_| words.next().is_none()) else {
                    return Err(fail("the file must start with `architecture <name>`".to_owned()));
                };
                file = Some(Self::new(architecture));
                continue;
            };
            if first == "replay" {
                close(current, open.take(), number)?;
                let (Some(name), Some(ticks), None) = (words.next(), words.next(), words.next()) else {
                    return Err(fail("expected `replay <name> <ticks>`".to_owned()));
                };
                let ticks = ticks.parse().map_err(|_| fail(format!("{ticks:?} isn't a tick count")))?;
                if current.replays.contains_key(name) {
                    return Err(fail(format!("replay `{name}` is listed twice")));
                }
                open = Some((name.to_owned(), RecordedTrail { ticks, checkpoints: Vec::new() }));
                continue;
            }
            let Some((_, trail)) = open.as_mut() else {
                return Err(fail("a checkpoint before any `replay` line".to_owned()));
            };
            let (Some(checksum), fingerprints, None) = (words.next(), words.next(), words.next()) else {
                return Err(fail("expected `<tick> <checksum> [<fingerprints>]`".to_owned()));
            };
            let checkpoint =
                parse_checkpoint(first, checksum, fingerprints.unwrap_or_default(), trail).map_err(fail)?;
            trail.checkpoints.push(checkpoint);
        }
        let mut file = file.ok_or(ChecksumsError {
            line: last_line.max(1),
            problem: "the file must start with `architecture <name>`".to_owned(),
        })?;
        close(&mut file, open, last_line + 1)?;
        Ok(file)
    }
}

/// Files a finished replay, once it has reached its last tick.
fn close(
    file: &mut ChecksumFile,
    open: Option<(String, RecordedTrail)>,
    line: usize,
) -> Result<(), ChecksumsError> {
    let Some((name, trail)) = open else { return Ok(()) };
    match trail.checkpoints.last() {
        Some(last) if last.tick == trail.ticks => {
            file.replays.insert(name, trail);
            Ok(())
        }
        _ => Err(ChecksumsError {
            line,
            problem: format!("replay `{name}` stops before its last tick, {}", trail.ticks),
        }),
    }
}

/// One checkpoint line, against the checkpoints before it in `trail`.
fn parse_checkpoint(
    tick: &str,
    checksum: &str,
    fingerprints: &str,
    trail: &RecordedTrail,
) -> Result<Checkpoint, String> {
    let tick: u32 = tick.parse().map_err(|_| format!("{tick:?} isn't a tick"))?;
    let previous = trail.checkpoints.last().map(|checkpoint| checkpoint.tick);
    let ticks_between = match previous {
        None if tick == 0 => 0,
        None => return Err("a replay's first checkpoint is tick 0".to_owned()),
        Some(previous) if tick > previous => tick - previous - 1,
        Some(previous) => return Err(format!("tick {tick} comes after tick {previous}")),
    };
    if tick > trail.ticks {
        return Err(format!("tick {tick} is past the replay's {} ticks", trail.ticks));
    }
    if checksum.len() != 16 {
        return Err(format!("{checksum:?} isn't a 64-bit checksum"));
    }
    let checksum = parse_hex(checksum).ok_or_else(|| format!("{checksum:?} isn't a 64-bit checksum"))?;

    if fingerprints.len() != ticks_between as usize * 4 {
        return Err(format!("expected {ticks_between} fingerprints of 4 digits before tick {tick}"));
    }
    let fingerprints = fingerprints
        .as_bytes()
        .chunks(4)
        .map(|digits| {
            std::str::from_utf8(digits)
                .ok()
                .and_then(parse_hex)
                .map(|value| value as u16)
                .ok_or_else(|| format!("bad fingerprints before tick {tick}"))
        })
        .collect::<Result<Vec<u16>, String>>()?;
    Ok(Checkpoint { tick, checksum, fingerprints })
}

/// Lowercase hex digits only, so one value has one text.
fn parse_hex(digits: &str) -> Option<u64> {
    if digits.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')) {
        u64::from_str_radix(digits, 16).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trail with no two checksums alike.
    fn trail(ticks: u32) -> Trail {
        let mut trail = Trail::new(0x1111, ticks);
        for tick in 1..=ticks {
            trail.push(u64::from(tick).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        }
        trail
    }

    #[test]
    fn the_first_and_last_ticks_are_always_checkpoints() {
        for (ticks, expected) in [
            (0, vec![0]),
            (1, vec![0, 1]),
            (59, vec![0, 59]),
            (60, vec![0, 60]),
            (61, vec![0, 60, 61]),
            (125, vec![0, 60, 120, 125]),
        ] {
            let recorded = RecordedTrail::from_trail(&trail(ticks));
            let got: Vec<u32> = recorded.checkpoints.iter().map(|checkpoint| checkpoint.tick).collect();
            assert_eq!(got, expected, "{ticks} ticks");
            assert_eq!(recorded.end(), trail(ticks).end());
            assert_eq!(recorded.first_difference(&trail(ticks)), None);
        }
    }

    #[test]
    fn a_files_text_round_trips() {
        let mut file = ChecksumFile::new("x86_64");
        file.set("short", RecordedTrail::from_trail(&trail(3)));
        file.set("long", RecordedTrail::from_trail(&trail(130)));
        let text = file.to_text();
        assert_eq!(ChecksumFile::parse(&text), Ok(file.clone()));
        assert_eq!(ChecksumFile::parse(&text.replace('\n', "\r\n")), Ok(file.clone()), "line endings aside");
        assert_eq!(ChecksumFile::parse(&text).unwrap().to_text(), text);
        assert_eq!(file.names().collect::<Vec<_>>(), ["long", "short"]);
        assert_eq!(checksums_file_name(file.architecture()), "checksums.x86_64.txt");
        assert!(file.remove("long"));
        assert!(!file.remove("long"));
        assert!(file.get("long").is_none());
    }

    /// A difference at any tick is named exactly, whether a fingerprint or a checkpoint sees it.
    #[test]
    fn the_first_difference_names_its_tick() {
        let recorded = RecordedTrail::from_trail(&trail(130));
        for changed in [0, 1, 2, 59, 60, 61, 119, 120, 129, 130] {
            let mut actual = trail(130);
            for tick in changed..=130 {
                actual.checksums[tick] ^= 0x8001;
            }
            let difference = recorded.first_difference(&actual).expect("it differs");
            assert_eq!(difference.tick as usize, changed);
            assert_eq!(difference.whole, changed % 60 == 0 || changed == 130, "tick {changed}");
            assert!(difference.to_string().contains(&format!("tick {changed}:")), "{difference}");
        }
    }

    /// A difference a fingerprint can't see still fails, at the checkpoint that can.
    #[test]
    fn a_fingerprint_collision_only_delays_the_tick_named() {
        let recorded = RecordedTrail::from_trail(&trail(130));
        let mut actual = trail(130);
        actual.checksums[45..=130].iter_mut().for_each(|checksum| *checksum ^= 0x1_0000);
        let difference = recorded.first_difference(&actual).expect("it differs");
        assert_eq!((difference.tick, difference.whole), (60, true));
    }

    #[test]
    fn a_run_cut_short_is_held_to_as_far_as_it_got() {
        let recorded = RecordedTrail::from_trail(&trail(130));
        assert_eq!(recorded.first_difference(&trail(70)), None);
        assert_eq!(recorded.first_difference(&trail(60)), None);
        assert_eq!(recorded.first_difference(&trail(0)), None);
        let mut actual = trail(70);
        actual.checksums[65] ^= 1;
        assert_eq!(recorded.first_difference(&actual).map(|difference| difference.tick), Some(65));
    }

    #[test]
    fn a_crafted_tick_count_reserves_no_more_than_a_run_needs() {
        let trail = Trail::new(0, u32::MAX);
        assert_eq!(trail.ticks(), 0);
        assert!(trail.checksums.capacity() <= TRAIL_RESERVE);
        assert!(trail.at(1).is_none());
    }

    /// Every row is a file that doesn't read, and each says on which line.
    #[test]
    fn malformed_files_are_refused() {
        const GOOD: &str = "0 0000000000000001";
        #[rustfmt::skip]
        let rows: &[(&str, &str, usize)] = &[
            ("empty", "", 1),
            ("only comments", "# one\n# two\n", 2),
            ("no architecture", "replay a 0\n", 1),
            ("architecture without a name", "architecture\n", 1),
            ("architecture with two names", "architecture a b\n", 1),
            ("checkpoint before a replay", "architecture a\n0 0000000000000001\n", 2),
            ("replay without ticks", "architecture a\nreplay x\n", 2),
            ("replay with extra", "architecture a\nreplay x 1 2\n", 2),
            ("ticks aren't a number", "architecture a\nreplay x y\n", 2),
            ("ticks are huge", "architecture a\nreplay x 99999999999\n", 2),
            ("a replay with no checkpoints", "architecture a\nreplay x 0\n", 3),
            ("a replay that stops short", "architecture a\nreplay x 5\n0 0000000000000001\n", 4),
            ("a replay that stops short before the next", "architecture a\nreplay x 5\n0 0000000000000001\nreplay y 0\n", 4),
            ("listed twice", "architecture a\nreplay x 0\n0 0000000000000001\nreplay x 0\n", 4),
            ("first checkpoint isn't 0", "architecture a\nreplay x 1\n1 0000000000000001\n", 3),
            ("ticks go back", "architecture a\nreplay x 9\n0 0000000000000001\n0 0000000000000001\n", 4),
            ("past the last tick", "architecture a\nreplay x 0\n0 0000000000000001\n1 0000000000000001\n", 4),
            ("tick isn't a number", "architecture a\nreplay x 1\nzero 0000000000000001\n", 3),
            ("no checksum", "architecture a\nreplay x 0\n0\n", 3),
            ("short checksum", "architecture a\nreplay x 0\n0 1\n", 3),
            ("long checksum", "architecture a\nreplay x 0\n0 00000000000000001\n", 3),
            ("checksum in upper case", "architecture a\nreplay x 0\n0 000000000000000A\n", 3),
            ("checksum with a sign", "architecture a\nreplay x 0\n0 +000000000000001\n", 3),
            ("checksum not hex", "architecture a\nreplay x 0\n0 000000000000000g\n", 3),
            ("too few fingerprints", "architecture a\nreplay x 2\n0 0000000000000001\n2 0000000000000001\n", 4),
            ("too many fingerprints", "architecture a\nreplay x 1\n0 0000000000000001\n1 0000000000000001 abcd\n", 4),
            ("part of a fingerprint", "architecture a\nreplay x 2\n0 0000000000000001\n2 0000000000000001 abc\n", 4),
            ("fingerprint not hex", "architecture a\nreplay x 2\n0 0000000000000001\n2 0000000000000001 abcg\n", 4),
            ("fingerprint not ASCII", "architecture a\nreplay x 2\n0 0000000000000001\n2 0000000000000001 éé\n", 4),
            ("words after the fingerprints", "architecture a\nreplay x 2\n0 0000000000000001\n2 0000000000000001 abcd e\n", 4),
        ];
        let mut failures = Vec::new();
        for &(name, text, line) in rows {
            match ChecksumFile::parse(text) {
                Err(error) if error.line == line => {}
                other => failures.push(format!("{name}: {other:?}, expected a refusal on line {line}")),
            }
        }
        assert_eq!(failures, Vec::<String>::new());
        assert!(ChecksumFile::parse(&format!("architecture a\nreplay x 0\n{GOOD}\n")).is_ok());
    }
}
