//! The replay file: a run's actions and what it starts from, as bytes. [`Replay`] has the format.

use std::time::Duration;

use sapphire_core::Name;
use sapphire_serialize::AssetId;
use xxhash_rust::xxh3::xxh3_64;

use crate::actions::{ActionsError, TickActions};
use crate::checkpoint::{CheckpointId, ZoneStart};
use crate::dialogue::DialogueEffect;
use crate::flags::{Flags, MAX_FLAGS};
use crate::simulation::SimConfig;
use crate::timeline::TimelineId;
use crate::zone::ZoneEntry;

/// What a replay holds beyond its actions, between two steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayCommand {
    /// [`Simulation::play_timeline`](crate::Simulation::play_timeline).
    PlayTimeline(TimelineId),
    /// Someone skipped, this far in. It changes nothing in the simulation, whose
    /// [`TimelineRun`](crate::TimelineRun) just runs out; it's here so a replay shown beside a renderer can
    /// show the skip.
    SkipTimeline { at_ms: u32 },
    /// [`Simulation::enter_zone`](crate::Simulation::enter_zone), for the cast's first.
    EnterZone(ZoneEntry),
    /// [`Simulation::return_to_checkpoint`](crate::Simulation::return_to_checkpoint) (M3 B4): the zone's
    /// player put back at a checkpoint.
    ReturnTo(CheckpointId),
    /// [`Simulation::apply_dialogue_effect`](crate::Simulation::apply_dialogue_effect) (M3 Q2).
    Dialogue(DialogueEffect),
}

const MAGIC: [u8; 8] = *b"SREPLAY\0";

/// The file format's newest version, for a run that starts from a save's place in its zone (M3 G1). A change
/// to its layout takes a new one, and older files are refused.
///
/// A file that starts where its scenario does is written in [`FLAGS_FORMAT_VERSION`] instead, which is this
/// layout without the zone's start; one with no starting flags either in [`COMMANDS_FORMAT_VERSION`]; and
/// one with no [`ReplayCommand`]s either in [`PLAIN_FORMAT_VERSION`]: most runs start from no save, play no
/// timeline and teleport nobody, and they keep the bytes they've always had.
pub const FORMAT_VERSION: u32 = 4;

/// [`FORMAT_VERSION`]'s layout, without its zone's start (M3 Q2), whose starting flags are never none.
pub const FLAGS_FORMAT_VERSION: u32 = 3;

/// The longest checkpoint name a start holds.
const MAX_CHECKPOINT_NAME: usize = 255;

/// [`FORMAT_VERSION`]'s layout, without its zone's start or its starting flags (M3 Q1).
pub const COMMANDS_FORMAT_VERSION: u32 = 2;

/// [`FORMAT_VERSION`]'s layout, without its zone's start, its starting flags or its commands.
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
const COMMAND_RETURN: u8 = 2;
const COMMAND_ENTER_ZONE: u8 = 3;
/// M3 Q2's, after B4's return to a checkpoint at 2 and W4's zone entry at 3.
const COMMAND_DIALOGUE: u8 = 4;

/// A starting flag in the file: its name's hash and its value.
const FLAG_LEN: usize = 8 + 4;

/// A run's length takes at most this many LEB128 bytes.
const MAX_RUN_BYTES: usize = 5;

/// Why bytes aren't a replay this build can play.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReplayError {
    #[error("this isn't a replay file: it doesn't start with the replay magic")]
    NotAReplay,
    #[error(
        "this replay is in format version {found}, and this build reads versions {PLAIN_FORMAT_VERSION} to \
         {FORMAT_VERSION}: record it again"
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
        "the file is in format version {COMMANDS_FORMAT_VERSION} and holds no commands, which is version \
         {PLAIN_FORMAT_VERSION}"
    )]
    EmptyCommands,
    #[error(
        "the file is in format version {FLAGS_FORMAT_VERSION} and holds no starting flags, which is version \
         {COMMANDS_FORMAT_VERSION} or {PLAIN_FORMAT_VERSION}"
    )]
    EmptyFlags,
    #[error("the run's start: {problem}")]
    BadStart { problem: &'static str },
    #[error("the file starts from {count} flags, past the simulation's {MAX_FLAGS}")]
    TooManyFlags { count: u32 },
    #[error(
        "starting flag {index} isn't after the one before it: they're in one order, by name, and each once"
    )]
    FlagOutOfOrder { index: u32 },
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

/// Reads the commands' section, checking all of it against the file's tick count, where `may_be_empty`
/// says whether it can be empty. Allocates what the bytes that are really there pay for.
fn read_commands(
    reader: &mut Reader<'_>,
    ticks: u32,
    may_be_empty: bool,
) -> Result<Vec<(u32, ReplayCommand)>, ReplayError> {
    let count = reader.u32("the commands' count")?;
    if count == 0 && !may_be_empty {
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
            COMMAND_PLAY => ReplayCommand::PlayTimeline(TimelineId::from_raw(argument)),
            COMMAND_SKIP => ReplayCommand::SkipTimeline { at_ms: argument },
            COMMAND_RETURN => ReplayCommand::ReturnTo(CheckpointId::from_raw(argument)),
            COMMAND_ENTER_ZONE => ReplayCommand::EnterZone(ZoneEntry::from_raw(argument)),
            COMMAND_DIALOGUE => ReplayCommand::Dialogue(DialogueEffect::from_raw(argument)),
            kind => return Err(ReplayError::BadCommand { index, kind }),
        };
        commands.push((tick, command));
    }
    Ok(commands)
}

/// Reads a zone's start section: the checkpoint's name and the cleared encounters, ascending.
fn read_start(reader: &mut Reader<'_>) -> Result<ZoneStart, ReplayError> {
    let [len] = reader.array("the start's checkpoint name's length")?;
    let name = reader.take("the start's checkpoint name", usize::from(len))?;
    let checkpoint = match std::str::from_utf8(name) {
        Ok("") => None,
        Ok(name) => Some(name.to_owned()),
        Err(_) => return Err(ReplayError::BadStart { problem: "its checkpoint's name isn't UTF-8" }),
    };
    let count = u16::from_le_bytes(reader.array("the start's cleared count")?);
    let mut section = Reader(reader.take("the start's cleared encounters", usize::from(count) * 16)?);
    let mut cleared: Vec<AssetId> = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let id = AssetId::from_bytes(section.array("an encounter's ID")?);
        if cleared.last().is_some_and(|&last| last >= id) {
            return Err(ReplayError::BadStart { problem: "its cleared encounters aren't ascending" });
        }
        cleared.push(id);
    }
    Ok(ZoneStart { checkpoint, cleared })
}

/// Writes what [`read_start`] reads.
fn write_start(out: &mut Vec<u8>, start: &ZoneStart) {
    let name = start.checkpoint.as_deref().unwrap_or("");
    // At most MAX_CHECKPOINT_NAME: every way in checks it.
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(&(start.cleared.len() as u16).to_le_bytes());
    for id in &start.cleared {
        out.extend_from_slice(&id.to_bytes());
    }
}

/// Whether a file can hold `start`: a checkpoint named, if at all, in 1 to 255 bytes, and at most
/// `u16::MAX` cleared encounters, ascending.
fn check_start(start: &ZoneStart) -> Result<(), ReplayError> {
    if start.checkpoint.as_ref().is_some_and(|name| name.is_empty() || name.len() > MAX_CHECKPOINT_NAME) {
        return Err(ReplayError::BadStart { problem: "its checkpoint's name is empty or past 255 bytes" });
    }
    if start.cleared.len() > usize::from(u16::MAX) {
        return Err(ReplayError::BadStart { problem: "it clears more encounters than a file holds" });
    }
    if start.cleared.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(ReplayError::BadStart { problem: "its cleared encounters aren't ascending" });
    }
    Ok(())
}

/// Reads the starting flags' section, which holds at least one, each after the one before by name, and
/// no more than the simulation's table.
fn read_flags(reader: &mut Reader<'_>, may_be_empty: bool) -> Result<Vec<(Name, i32)>, ReplayError> {
    let count = reader.u32("the starting flags' count")?;
    if count == 0 && !may_be_empty {
        return Err(ReplayError::EmptyFlags);
    }
    if count as usize > MAX_FLAGS {
        return Err(ReplayError::TooManyFlags { count });
    }
    let mut section = Reader(reader.take("the starting flags", count as usize * FLAG_LEN)?);
    let mut flags: Vec<(Name, i32)> = Vec::with_capacity(count as usize);
    for index in 0..count {
        let name = section.u64("a flag's name")?;
        let value = section.u32("a flag's value")? as i32;
        if flags.last().is_some_and(|&(last, _)| last.as_u64() >= name) || value == 0 {
            return Err(ReplayError::FlagOutOfOrder { index });
        }
        flags.push((Name::from_u64(name), value));
    }
    Ok(flags)
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
/// | 4 | how many commands, never 0 in [`COMMANDS_FORMAT_VERSION`] (that and [`FORMAT_VERSION`]) |
/// | 9 each | a command: its tick, 1 byte for its kind, 4 for its argument (likewise) |
/// | 4 | how many starting flags, up to [`MAX_FLAGS`], never 0 in [`FLAGS_FORMAT_VERSION`] (that and [`FORMAT_VERSION`]) |
/// | 12 each | a flag: its name's hash, then its value, never 0, ascending by hash (likewise) |
/// | 1 | the zone's start: its checkpoint name's length, 0 for none ([`FORMAT_VERSION`] alone) |
/// | n | the checkpoint's name, UTF-8 ([`FORMAT_VERSION`] alone) |
/// | 2 | how many encounters it has cleared ([`FORMAT_VERSION`] alone) |
/// | 16 each | a cleared encounter's ID, ascending ([`FORMAT_VERSION`] alone) |
///
/// A command's kind is 0 for a timeline played (its argument the timeline's ID), 1 for one skipped (how far
/// in, in milliseconds), 2 for a return to a checkpoint (the checkpoint's place in its zone's list), 3 for
/// entering a zone (the entry) and 4 for a dialogue's effect (the dialogue's ID in the high 16 bits and the
/// effect's in the low). A kind this build doesn't know is refused by its index.
/// | … | the body: runs of equal ticks, up to the digest |
/// | 8 | XXH3-64 of every byte before it |
///
/// A run is its length in ticks, in LEB128, then one tick in [`TickActions`]' own layout. Runs add
/// up to the tick count, and a run is never empty or followed by a run of the same actions, so one
/// set of inputs has one encoding: encoding what was decoded gives the same bytes.
///
/// A [`ReplayCommand`] goes between two ticks: its tick is the step it comes before, or the tick count,
/// after the last. They come in order.
///
/// The starting flags are what the run starts from ([`crate::Simulation::start_with_flags`]): a save's
/// story so far, which isn't anything anyone did in the run, so it's the header's and not a command. A
/// flag of 0 is one nobody set, so it isn't written.
///
/// The zone's start is a save's place in the scenario's zone (B4's [`ZoneStart`], M3 G1): its latest
/// checkpoint and the encounters it has cleared, which the scenario's setup takes before the first step.
///
/// A file with no zone's start is in [`FLAGS_FORMAT_VERSION`], with no flags either in
/// [`COMMANDS_FORMAT_VERSION`]. /// A file with no flags is in [`COMMANDS_FORMAT_VERSION`], and with no commands either in
/// [`PLAIN_FORMAT_VERSION`], so each run has one encoding.
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
    commands: Vec<(u32, ReplayCommand)>,
    flags: Vec<(Name, i32)>,
    /// Where in its zone it starts, if not where the scenario does.
    start: Option<ZoneStart>,
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
        if !(PLAIN_FORMAT_VERSION..=FORMAT_VERSION).contains(&version) {
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
        let commands = match version {
            PLAIN_FORMAT_VERSION => Vec::new(),
            COMMANDS_FORMAT_VERSION => read_commands(&mut reader, ticks, false)?,
            _ => read_commands(&mut reader, ticks, true)?,
        };
        let flags = match version {
            FLAGS_FORMAT_VERSION => read_flags(&mut reader, false)?,
            FORMAT_VERSION => read_flags(&mut reader, true)?,
            _ => Vec::new(),
        };
        let start = if version == FORMAT_VERSION { Some(read_start(&mut reader)?) } else { None };
        validate_body(reader.0, ticks)?;

        let config = SimConfig { seed, max_actors, step_budget, ..SimConfig::default() };
        Ok(Self {
            scenario: scenario.to_owned(),
            config,
            tables_hash,
            ticks,
            commands,
            flags,
            start,
            body: reader.0.to_vec(),
        })
    }

    /// The file's bytes. Decoding them gives this replay back.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        const HEADER_LEN: usize = 49;
        let mut out = Vec::with_capacity(HEADER_LEN + self.scenario.len() + self.body.len() + DIGEST_LEN);
        out.extend_from_slice(&MAGIC);
        let version = match (self.start.is_some(), self.flags.is_empty(), self.commands.is_empty()) {
            (true, _, _) => FORMAT_VERSION,
            (false, false, _) => FLAGS_FORMAT_VERSION,
            (false, true, false) => COMMANDS_FORMAT_VERSION,
            (false, true, true) => PLAIN_FORMAT_VERSION,
        };
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
        if version != PLAIN_FORMAT_VERSION {
            out.extend_from_slice(&(self.commands.len() as u32).to_le_bytes());
            for &(tick, command) in &self.commands {
                let (kind, argument) = match command {
                    ReplayCommand::PlayTimeline(id) => (COMMAND_PLAY, id.raw()),
                    ReplayCommand::SkipTimeline { at_ms } => (COMMAND_SKIP, at_ms),
                    ReplayCommand::ReturnTo(checkpoint) => (COMMAND_RETURN, checkpoint.raw()),
                    ReplayCommand::EnterZone(entry) => (COMMAND_ENTER_ZONE, entry.to_raw()),
                    ReplayCommand::Dialogue(effect) => (COMMAND_DIALOGUE, effect.to_raw()),
                };
                out.extend_from_slice(&tick.to_le_bytes());
                out.push(kind);
                out.extend_from_slice(&argument.to_le_bytes());
            }
        }
        if version >= FLAGS_FORMAT_VERSION {
            out.extend_from_slice(&(self.flags.len() as u32).to_le_bytes());
            for &(name, value) in &self.flags {
                out.extend_from_slice(&name.as_u64().to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        if let Some(start) = &self.start {
            write_start(&mut out, start);
        }
        out.extend_from_slice(&self.body);
        let digest = xxh3_64(&out);
        out.extend_from_slice(&digest.to_le_bytes());
        out
    }

    /// Where in its zone it starts, if not where its scenario does.
    #[must_use]
    pub fn zone_start(&self) -> Option<&ZoneStart> {
        self.start.as_ref()
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

    /// What was done between steps: each command with the step it comes before, in order. A
    /// [`super::Player`] carries out the timelines' starts, the zone entries and the dialogues' effects, and
    /// presentation beside it can read the skips here.
    #[must_use]
    pub fn commands(&self) -> &[(u32, ReplayCommand)] {
        &self.commands
    }

    /// The flags the run starts from, ascending by name.
    #[must_use]
    pub fn starting_flags(&self) -> &[(Name, i32)] {
        &self.flags
    }

    /// Every tick's actions, in order.
    pub fn actions(&self) -> impl Iterator<Item = TickActions> + '_ {
        let mut cursor = Cursor::START;
        std::iter::from_fn(move || cursor.next(self))
    }
}

/// The commands a recording has room for from its start: a game's run records a few a beat, each beat
/// once (M3 G1), so a beat's step pushes them with no heap, as a step that saves nothing promises. Past
/// this many, the list grows as a `Vec` does.
const COMMANDS_RESERVED: usize = 64;

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
            commands: Vec::with_capacity(COMMANDS_RESERVED),
            flags: Vec::new(),
            start: None,
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

    /// Makes the recording as long as a file may be, so the next tick is refused, without stepping a run
    /// four hours long.
    #[cfg(test)]
    pub(crate) fn fill(&mut self) {
        self.replay.ticks = MAX_TICKS;
    }

    /// Adds a command before the next tick: with no heap for a recording's first 64.
    pub fn command(&mut self, command: ReplayCommand) {
        self.replay.commands.push((self.replay.ticks, command));
    }

    /// Says what flags the run starts from, as a simulation took them: in order, by name, each once, and
    /// none that's 0, since that's what a flag nobody set reads.
    pub(crate) fn start_with_flags(&mut self, flags: &Flags) {
        self.replay.flags = flags.iter().filter(|&(_, value)| value != 0).collect();
    }

    /// Says where in its zone the run starts, before its first tick: a save's place (M3 G1).
    ///
    /// # Errors
    ///
    /// For a start a file can't hold, in which case it keeps none.
    pub fn start_at(&mut self, start: ZoneStart) -> Result<(), ReplayError> {
        check_start(&start)?;
        self.replay.start = Some(start);
        Ok(())
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

    /// A return to a checkpoint (M3 B4) is a command of its own kind beside a timeline's, in the same
    /// section, and a kind no build knows is refused by its index.
    #[test]
    fn a_return_reads_back_and_an_unknown_kind_is_refused() {
        let mut recorder = Recorder::new("s", SimConfig::default(), 0).unwrap();
        recorder.push(&TickActions::NEUTRAL).unwrap();
        let back = ReplayCommand::ReturnTo(CheckpointId::from_raw(0x0a0b_0c0d));
        recorder.command(back);
        recorder.command(ReplayCommand::SkipTimeline { at_ms: 5 });
        recorder.push(&TickActions::NEUTRAL).unwrap();
        let replay = recorder.finish();
        let bytes = replay.encode();
        let read = Replay::decode(&bytes).unwrap();
        assert_eq!(read.commands(), [(1, back), (1, ReplayCommand::SkipTimeline { at_ms: 5 })]);

        let at =
            bytes.windows(5).position(|window| window == [COMMAND_RETURN, 0x0d, 0x0c, 0x0b, 0x0a]).unwrap();
        let mut spoiled = bytes[..bytes.len() - DIGEST_LEN].to_vec();
        spoiled[at] = u8::MAX;
        let digest = xxh3_64(&spoiled);
        spoiled.extend_from_slice(&digest.to_le_bytes());
        assert_eq!(Replay::decode(&spoiled), Err(ReplayError::BadCommand { index: 0, kind: u8::MAX }));
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
