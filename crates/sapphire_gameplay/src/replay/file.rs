//! The replay file: a run's actions and what it starts from, as bytes. [`Replay`] has the format.

use std::time::Duration;

use xxhash_rust::xxh3::xxh3_64;

use crate::actions::{ActionsError, TickActions};
use crate::simulation::SimConfig;
use crate::timeline::{TimelineCommand, TimelineId};

const MAGIC: [u8; 8] = *b"SREPLAY\0";

/// The file format's version. A change to its layout takes a new one, and older files are refused.
///
/// A file with no [`TimelineCommand`]s is written in [`PLAIN_FORMAT_VERSION`] instead, which is this
/// layout without that section: most runs play no timeline, and they keep the bytes they've always had.
pub const FORMAT_VERSION: u32 = 2;

/// [`FORMAT_VERSION`]'s layout, without its commands.
pub const PLAIN_FORMAT_VERSION: u32 = 1;

/// The longest replay: four hours at 60 Hz, past any session anyone would keep. The bound on what a
/// crafted file can make a runner do.
pub const MAX_TICKS: u32 = 4 * 60 * 60 * 60;

/// The largest actor pool a replay may ask for: [`crate::Simulation::new`] allocates all of it at
/// once, so a crafted file mustn't get to pick.
pub const MAX_ACTORS: u32 = 4096;

/// The longest scenario or replay name.
pub const MAX_NAME_LEN: usize = 64;

const DIGEST_LEN: usize = 8;

/// A command in the file: its tick, its kind and its argument.
const COMMAND_LEN: usize = 4 + 1 + 4;
const COMMAND_PLAY: u8 = 0;
const COMMAND_SKIP: u8 = 1;

/// A run's length takes at most this many LEB128 bytes.
const MAX_RUN_BYTES: usize = 5;

/// Why bytes aren't a replay this build can play.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    #[error("this isn't a replay file: it doesn't start with the replay magic")]
    NotAReplay,
    #[error(
        "this replay is in format version {found}, and this build reads version {FORMAT_VERSION}: \
         record it again"
    )]
    FormatVersion { found: u32 },
    #[error(
        "this replay's actions are in layout version {found}, and this build reads version {}: \
         record it again",
        TickActions::LAYOUT_VERSION
    )]
    LayoutVersion { found: u32 },
    #[error("the file stops short: {what} needs {needed} bytes, and {found} are left")]
    Truncated { what: &'static str, needed: usize, found: usize },
    #[error("the file is damaged: its digest is {stored:016x}, and its bytes hash to {computed:016x}")]
    Damaged { stored: u64, computed: u64 },
    #[error("{name:?} can't be a name: 1 to {MAX_NAME_LEN} of a-z, 0-9, - and _")]
    BadName { name: String },
    #[error("the replay asks for a pool of {count} actors, past the limit of {MAX_ACTORS}")]
    TooManyActors { count: u32 },
    #[error("the replay is {count} ticks long, past the limit of {MAX_TICKS}")]
    TooManyTicks { count: u64 },
    #[error("the run at tick {tick} has a length that isn't canonical 32-bit LEB128")]
    BadRunLength { tick: u64 },
    #[error("the run at tick {tick} is empty")]
    EmptyRun { tick: u64 },
    #[error("the run at tick {tick} repeats the run before it, which one run would hold")]
    SplitRun { tick: u64 },
    #[error("the run at tick {tick} goes past the {ticks} ticks the file says it has")]
    RunPastEnd { tick: u64, ticks: u32 },
    #[error("the runs end at tick {found}, and the file says it has {ticks}")]
    RunsEndShort { found: u64, ticks: u32 },
    #[error("the run at tick {tick}: {source}")]
    Actions { tick: u64, source: ActionsError },
    #[error(
        "the file is in format version {FORMAT_VERSION} and holds no commands, which is version {PLAIN_FORMAT_VERSION}"
    )]
    EmptyCommands,
    #[error("command {index} has kind {kind}, which this build doesn't know")]
    BadCommand { index: u32, kind: u8 },
    #[error("command {index} is at tick {tick}, before the one before it or past the file's {ticks} ticks")]
    CommandOutOfOrder { index: u32, tick: u32, ticks: u32 },
}

/// Reads a file's fields in order, refusing to read past its end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, what: &'static str, needed: usize) -> Result<&'a [u8], ReplayError> {
        let (taken, rest) = self.0.split_at_checked(needed).ok_or(ReplayError::Truncated {
            what,
            needed,
            found: self.0.len(),
        })?;
        self.0 = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], ReplayError> {
        let taken = self.take(what, N)?;
        Ok(taken.try_into().expect("take gave N bytes"))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32, ReplayError> {
        self.array(what).map(u32::from_le_bytes)
    }

    fn u64(&mut self, what: &'static str) -> Result<u64, ReplayError> {
        self.array(what).map(u64::from_le_bytes)
    }
}

/// Whether `name` can name a scenario or a replay: short, and safe in a file name and a terminal.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    (1..=MAX_NAME_LEN).contains(&name.len())
        && name.bytes().all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'))
}

fn write_run_length(out: &mut Vec<u8>, mut length: u32) {
    while length >= 0x80 {
        out.push(length as u8 | 0x80);
        length >>= 7;
    }
    out.push(length as u8);
}

/// A run's length from the front of `bytes`, with the bytes after it. `None` for anything but the
/// one shortest encoding of a 32-bit number.
fn read_run_length(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let mut length = 0u32;
    for (index, &byte) in bytes.iter().take(MAX_RUN_BYTES).enumerate() {
        let bits = u32::from(byte & 0x7f);
        // The fifth byte holds the top four bits of 32.
        if index == MAX_RUN_BYTES - 1 && bits > 0x0f {
            return None;
        }
        length |= bits << (index * 7);
        if byte & 0x80 == 0 {
            // A last byte of zero means a shorter encoding exists.
            return (byte != 0 || index == 0).then_some((length, &bytes[index + 1..]));
        }
    }
    None
}

/// Reads the commands' section, checking all of it against the file's tick count. Allocates what the
/// bytes that are really there pay for.
fn read_commands(reader: &mut Reader<'_>, ticks: u32) -> Result<Vec<(u32, TimelineCommand)>, ReplayError> {
    let count = reader.u32("the commands' count")?;
    if count == 0 {
        return Err(ReplayError::EmptyCommands);
    }
    let needed = (count as usize).saturating_mul(COMMAND_LEN);
    let mut section = Reader(reader.take("the commands", needed)?);
    let mut commands = Vec::with_capacity(count as usize);
    let mut last = 0;
    for index in 0..count {
        let tick = section.u32("a command's tick")?;
        let [kind] = section.array("a command's kind")?;
        let argument = section.u32("a command's argument")?;
        if tick < last || tick > ticks {
            return Err(ReplayError::CommandOutOfOrder { index, tick, ticks });
        }
        last = tick;
        let command = match kind {
            COMMAND_PLAY => TimelineCommand::Play(TimelineId::from_raw(argument)),
            COMMAND_SKIP => TimelineCommand::Skip { at_ms: argument },
            kind => return Err(ReplayError::BadCommand { index, kind }),
        };
        commands.push((tick, command));
    }
    Ok(commands)
}

/// Checks a body's runs against its tick count, walking it once and keeping nothing.
fn validate_body(body: &[u8], ticks: u32) -> Result<(), ReplayError> {
    let mut rest = body;
    let mut tick = 0u64;
    let mut previous = None;
    while !rest.is_empty() {
        let (length, after) = read_run_length(rest).ok_or(ReplayError::BadRunLength { tick })?;
        if length == 0 {
            return Err(ReplayError::EmptyRun { tick });
        }
        let (actions, after) =
            TickActions::read_from(after).map_err(|source| ReplayError::Actions { tick, source })?;
        if previous == Some(actions) {
            return Err(ReplayError::SplitRun { tick });
        }
        if tick + u64::from(length) > u64::from(ticks) {
            return Err(ReplayError::RunPastEnd { tick, ticks });
        }
        tick += u64::from(length);
        previous = Some(actions);
        rest = after;
    }
    if tick == u64::from(ticks) { Ok(()) } else { Err(ReplayError::RunsEndShort { found: tick, ticks }) }
}

/// Where a walk through a validated body has got to. Holds no borrow, so a player can keep it next
/// to the replay it walks.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cursor {
    offset: usize,
    left_in_run: u32,
    actions: TickActions,
}

impl Cursor {
    pub(crate) const START: Self = Self { offset: 0, left_in_run: 0, actions: TickActions::NEUTRAL };

    /// The next tick of `replay`'s body, or `None` at its end. Allocates nothing.
    pub(crate) fn next(&mut self, replay: &Replay) -> Option<TickActions> {
        const VALIDATED: &str = "a replay's body is validated when it's made";
        if self.left_in_run == 0 {
            let rest = &replay.body[self.offset..];
            if rest.is_empty() {
                return None;
            }
            let (length, after) = read_run_length(rest).expect(VALIDATED);
            let (actions, after) = TickActions::read_from(after).expect(VALIDATED);
            self.offset = replay.body.len() - after.len();
            self.left_in_run = length;
            self.actions = actions;
        }
        self.left_in_run -= 1;
        Some(self.actions)
    }
}

/// A recorded run: what it starts from and every tick's actions, held as the file holds them, so a
/// long run costs its encoded size.
///
/// It holds no results. What a run must reach is per architecture and lives in a
/// [`ChecksumFile`](super::ChecksumFile), so only an intended change moves it.
///
/// # The file, format version [`FORMAT_VERSION`]
///
/// Little-endian throughout:
///
/// | Bytes | What |
/// |---|---|
/// | 8 | the magic, `SREPLAY` and a zero |
/// | 4 | the format version |
/// | 4 | [`TickActions::LAYOUT_VERSION`] of the actions in the body |
/// | 8 | the seed |
/// | 4 | the actor pool's size, at most [`MAX_ACTORS`] |
/// | 8 | the step budget, in nanoseconds |
/// | 8 | the move data's hash ([`crate::Simulation::tables_hash`]) |
/// | 4 | the tick count, at most [`MAX_TICKS`] |
/// | 1 | the scenario name's length |
/// | n | the scenario's name: `a-z`, `0-9`, `-`, `_` |
/// | 4 | how many commands, never 0 ([`FORMAT_VERSION`] alone) |
/// | 9 each | a command: its tick, 1 byte for its kind, 4 for its argument ([`FORMAT_VERSION`] alone) |
/// | … | the body: runs of equal ticks, up to the digest |
/// | 8 | XXH3-64 of every byte before it |
///
/// A run is its length in ticks, in LEB128, then one tick in [`TickActions`]' own layout. Runs add
/// up to the tick count, and a run is never empty or followed by a run of the same actions, so one
/// set of inputs has one encoding: encoding what was decoded gives the same bytes.
///
/// A [`TimelineCommand`] goes between two ticks: its tick is the step it comes before, or the tick count,
/// after the last. They come in order, and a file with none is in [`PLAIN_FORMAT_VERSION`], so that also
/// has one encoding.
///
/// The magic and both versions come first and never move, so any later format is refused by name.
/// There are no upgrades: a replay of another version says so, and is recorded again.
///
/// # Crafted files
///
/// [`Replay::decode`] takes anything. It answers with an error rather than a panic, and allocates
/// only what the file's own length pays for: every count in it is checked against what's really
/// there or against a limit. A file past those checks can still run for [`MAX_TICKS`] steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replay {
    scenario: String,
    config: SimConfig,
    tables_hash: u64,
    ticks: u32,
    commands: Vec<(u32, TimelineCommand)>,
    body: Vec<u8>,
}

impl Replay {
    /// Reads a replay file, checking all of it.
    pub fn decode(bytes: &[u8]) -> Result<Self, ReplayError> {
        let mut reader = Reader(bytes);
        if reader.array::<8>("the magic").ok() != Some(MAGIC) {
            return Err(ReplayError::NotAReplay);
        }
        let version = reader.u32("the format version")?;
        if version != FORMAT_VERSION && version != PLAIN_FORMAT_VERSION {
            return Err(ReplayError::FormatVersion { found: version });
        }
        let layout = reader.u32("the actions' layout version")?;
        if layout != TickActions::LAYOUT_VERSION {
            return Err(ReplayError::LayoutVersion { found: layout });
        }

        let Some((fields, stored)) = reader.0.split_last_chunk::<DIGEST_LEN>() else {
            return Err(ReplayError::Truncated {
                what: "the digest",
                needed: DIGEST_LEN,
                found: reader.0.len(),
            });
        };
        let stored = u64::from_le_bytes(*stored);
        let computed = xxh3_64(&bytes[..bytes.len() - DIGEST_LEN]);
        if stored != computed {
            return Err(ReplayError::Damaged { stored, computed });
        }

        let mut reader = Reader(fields);
        let seed = reader.u64("the seed")?;
        let max_actors = reader.u32("the actor pool's size")?;
        let step_budget = Duration::from_nanos(reader.u64("the step budget")?);
        let tables_hash = reader.u64("the move data's hash")?;
        let ticks = reader.u32("the tick count")?;
        let [name_len] = reader.array("the scenario name's length")?;
        let name = reader.take("the scenario's name", usize::from(name_len))?;
        let Some(scenario) = std::str::from_utf8(name).ok().filter(|name| is_valid_name(name)) else {
            let shown = &name[..name.len().min(MAX_NAME_LEN)];
            return Err(ReplayError::BadName { name: String::from_utf8_lossy(shown).into_owned() });
        };
        if max_actors > MAX_ACTORS {
            return Err(ReplayError::TooManyActors { count: max_actors });
        }
        if ticks > MAX_TICKS {
            return Err(ReplayError::TooManyTicks { count: u64::from(ticks) });
        }
        let commands =
            if version == FORMAT_VERSION { read_commands(&mut reader, ticks)? } else { Vec::new() };
        validate_body(reader.0, ticks)?;

        let config = SimConfig { seed, max_actors, step_budget, ..SimConfig::default() };
        Ok(Self {
            scenario: scenario.to_owned(),
            config,
            tables_hash,
            ticks,
            commands,
            body: reader.0.to_vec(),
        })
    }

    /// The file's bytes. Decoding them gives this replay back.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        const HEADER_LEN: usize = 49;
        let mut out = Vec::with_capacity(HEADER_LEN + self.scenario.len() + self.body.len() + DIGEST_LEN);
        out.extend_from_slice(&MAGIC);
        let version = if self.commands.is_empty() { PLAIN_FORMAT_VERSION } else { FORMAT_VERSION };
        out.extend_from_slice(&version.to_le_bytes());
        out.extend_from_slice(&TickActions::LAYOUT_VERSION.to_le_bytes());
        out.extend_from_slice(&self.config.seed.to_le_bytes());
        out.extend_from_slice(&self.config.max_actors.to_le_bytes());
        let budget = u64::try_from(self.config.step_budget.as_nanos()).unwrap_or(u64::MAX);
        out.extend_from_slice(&budget.to_le_bytes());
        out.extend_from_slice(&self.tables_hash.to_le_bytes());
        out.extend_from_slice(&self.ticks.to_le_bytes());
        // At most MAX_NAME_LEN: every way in checks the name.
        out.push(self.scenario.len() as u8);
        out.extend_from_slice(self.scenario.as_bytes());
        if !self.commands.is_empty() {
            out.extend_from_slice(&(self.commands.len() as u32).to_le_bytes());
            for &(tick, command) in &self.commands {
                let (kind, argument) = match command {
                    TimelineCommand::Play(id) => (COMMAND_PLAY, id.raw()),
                    TimelineCommand::Skip { at_ms } => (COMMAND_SKIP, at_ms),
                };
                out.extend_from_slice(&tick.to_le_bytes());
                out.push(kind);
                out.extend_from_slice(&argument.to_le_bytes());
            }
        }
        out.extend_from_slice(&self.body);
        let digest = xxh3_64(&out);
        out.extend_from_slice(&digest.to_le_bytes());
        out
    }

    /// The name of the scenario it starts from.
    #[must_use]
    pub fn scenario(&self) -> &str {
        &self.scenario
    }

    /// The simulation's config: the seed and the pool the run was recorded with.
    #[must_use]
    pub fn config(&self) -> SimConfig {
        self.config
    }

    /// The hash of the move data it was recorded against.
    #[must_use]
    pub fn tables_hash(&self) -> u64 {
        self.tables_hash
    }

    /// Stamps the replay with new move data's hash, keeping the actions: what `--update` does when a
    /// move changes on purpose.
    pub fn set_tables_hash(&mut self, tables_hash: u64) {
        self.tables_hash = tables_hash;
    }

    /// How many ticks it runs for.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    /// What it did with timelines: each with the step it comes before, in order. A [`super::Player`] carries
    /// out the [`TimelineCommand::Play`]s, and presentation beside it can read the skips here.
    #[must_use]
    pub fn commands(&self) -> &[(u32, TimelineCommand)] {
        &self.commands
    }

    /// Every tick's actions, in order.
    pub fn actions(&self) -> impl Iterator<Item = TickActions> + '_ {
        let mut cursor = Cursor::START;
        std::iter::from_fn(move || cursor.next(self))
    }
}

/// Builds a [`Replay`] one tick at a time, joining repeats into runs as it goes, so it costs what
/// the file will: nothing for a tick like the last.
#[derive(Clone, Debug)]
pub struct Recorder {
    replay: Replay,
    run: TickActions,
    run_len: u32,
}

impl Recorder {
    /// A recording with no ticks yet. Refuses what [`Replay::decode`] would.
    pub fn new(scenario: &str, config: SimConfig, tables_hash: u64) -> Result<Self, ReplayError> {
        if !is_valid_name(scenario) {
            return Err(ReplayError::BadName { name: scenario.to_owned() });
        }
        if config.max_actors > MAX_ACTORS {
            return Err(ReplayError::TooManyActors { count: config.max_actors });
        }
        let replay = Replay {
            scenario: scenario.to_owned(),
            config,
            tables_hash,
            ticks: 0,
            commands: Vec::new(),
            body: Vec::new(),
        };
        Ok(Self { replay, run: TickActions::NEUTRAL, run_len: 0 })
    }

    /// Adds a tick, or refuses at [`MAX_TICKS`] with the recording still good up to there.
    pub fn push(&mut self, actions: &TickActions) -> Result<(), ReplayError> {
        if self.replay.ticks == MAX_TICKS {
            return Err(ReplayError::TooManyTicks { count: u64::from(MAX_TICKS) + 1 });
        }
        self.replay.ticks += 1;
        if self.run_len > 0 && self.run == *actions {
            self.run_len += 1;
            return Ok(());
        }
        self.end_run();
        self.run = *actions;
        self.run_len = 1;
        Ok(())
    }

    /// Adds a command before the next tick.
    pub fn command(&mut self, command: TimelineCommand) {
        self.replay.commands.push((self.replay.ticks, command));
    }

    /// Stamps the recording with the move data's hash, once its world is set up.
    pub(crate) fn set_tables_hash(&mut self, tables_hash: u64) {
        self.replay.tables_hash = tables_hash;
    }

    fn end_run(&mut self) {
        if self.run_len > 0 {
            write_run_length(&mut self.replay.body, self.run_len);
            self.run.write_to(&mut self.replay.body);
        }
    }

    /// Ticks so far.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        self.replay.ticks
    }

    /// The scenario it's recording.
    #[must_use]
    pub fn scenario(&self) -> &str {
        self.replay.scenario()
    }

    /// The finished replay.
    #[must_use]
    pub fn finish(mut self) -> Replay {
        self.end_run();
        self.replay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lengths_take_one_encoding_each() {
        for length in [1, 127, 128, 300, 16_383, 16_384, MAX_TICKS, u32::MAX] {
            let mut bytes = Vec::new();
            write_run_length(&mut bytes, length);
            bytes.push(0xee);
            let (read, rest) = read_run_length(&bytes).expect("a length");
            assert_eq!(read, length);
            assert_eq!(rest, [0xee]);
        }
        assert_eq!(read_run_length(&[0]), Some((0, &[][..])), "zero has its one byte");
        assert_eq!(read_run_length(&[0x80, 0]), None, "zero in two bytes");
        assert_eq!(read_run_length(&[0x81, 0x80, 0]), None, "1 in three bytes");
        assert_eq!(read_run_length(&[0xff, 0xff, 0xff, 0xff, 0x10]), None, "33 bits");
        assert_eq!(read_run_length(&[0xff, 0xff, 0xff, 0xff, 0x8f, 0]), None, "six bytes");
        assert_eq!(read_run_length(&[0x80]), None, "stops in the middle");
        assert_eq!(read_run_length(&[]), None);
    }

    #[test]
    fn names_are_short_and_plain() {
        assert!(is_valid_name("melee-sweep_2"));
        assert!(is_valid_name(&"a".repeat(MAX_NAME_LEN)));
        for name in ["", "Drill", "a b", "a/b", "..", "\u{1b}[31m", "é"] {
            assert!(!is_valid_name(name), "{name:?}");
        }
        assert!(!is_valid_name(&"a".repeat(MAX_NAME_LEN + 1)));
    }

    #[test]
    fn a_recorder_refuses_past_its_limit_and_stays_good() {
        let mut recorder = Recorder::new("s", SimConfig::default(), 0).unwrap();
        recorder.push(&TickActions::NEUTRAL).unwrap();
        recorder.replay.ticks = MAX_TICKS;
        assert_eq!(
            recorder.push(&TickActions::NEUTRAL),
            Err(ReplayError::TooManyTicks { count: u64::from(MAX_TICKS) + 1 })
        );
        assert_eq!(recorder.ticks(), MAX_TICKS);
    }
}
