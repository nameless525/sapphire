//! A run's journal (M4 F3): a session keeps a run a part at a time, each keep adding only the ticks since the
//! last, so a keep near the four-hour cap writes seconds of play rather than hours.
//!
//! # The file
//!
//! `runs/<n>/<name>.journal` begins with [`MAGIC`] and its version, then holds one record a keep, each
//! appended where the last ended:
//!
//! | Bytes | What |
//! |---|---|
//! | 4 | the payload's length |
//! | … | the payload |
//! | 8 | XXH3-64 of the length and the payload |
//!
//! And the payload, little-endian:
//!
//! | Bytes | What |
//! |---|---|
//! | 4 | the run's ticks once this record is added |
//! | 1 | 1 if the run had ended, else 0 |
//! | 4, n | the replay: the whole file in the first record, then [`Replay::encode_since`]'s part from the record before |
//! | 4, n | the checksums' lines that stay: a [`ChecksumFile`]'s lines for each checkpoint a whole minute's ticks in, past the record before's (and tick 0 in the first) |
//! | 4, n | the line for this record's last tick, if it isn't one of those: what the file's last line would be if the run ended here |
//!
//! So the records up to any one, its own last line among them, are the run's replay and its checksums file as
//! a whole write at that keep would have had them, byte for byte. A record a kill cut short fails its digest,
//! and it and anything after it are left out; [`SessionDocument`](crate::SessionDocument) says how many ticks
//! a run has and whether it had ended, and a reader takes the records up to the one that says so (a run's end
//! can follow a keep at the same tick). Every keep re-sends at most a minute's 59
//! fingerprints, 236 bytes, so that the lines that stay are the ones a whole file holds.

use std::fmt::Write as _;

use sapphire_gameplay::replay::{
    CHECKPOINT_EVERY, ChecksumFile, Reached, Recorder, Replay, ReplayError, Trail,
};
use xxhash_rust::xxh3::xxh3_64;

/// What a journal starts with.
pub const MAGIC: [u8; 8] = *b"SJOURNL\0";

/// The journal's layout's version, after the magic.
pub const VERSION: u32 = 1;

const HEADER_LEN: usize = MAGIC.len() + 4;

/// Why a run's journal doesn't give its run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JournalError {
    #[error("it isn't a run's journal, version {VERSION}")]
    NotAJournal,
    #[error("its records reach tick {reached}, short of the session's {ticks}")]
    Short { reached: u32, ticks: u32 },
    #[error("its records skip past tick {ticks}, which the session names")]
    Missed { ticks: u32 },
    #[error("record {record}: {source}")]
    Replay { record: usize, source: ReplayError },
    #[error("record {record} holds text that isn't UTF-8")]
    Text { record: usize },
    #[error("its checksums don't read: {0}")]
    Checksums(String),
}

/// How far a run's journal is on the disk: its length, and what its records hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Written {
    pub(crate) bytes: u64,
    /// `None` before its first record.
    pub(crate) reached: Option<Reached>,
}

/// The record that takes a journal at `written` to `replay` and `trail`, with the header before it in a new
/// file, and where the journal stands once it's written. `None` if `replay` is behind what's written.
pub(crate) fn record(
    written: Written,
    replay: &Replay,
    trail: &Trail,
    ended: bool,
) -> Option<(Vec<u8>, Written)> {
    let ticks = replay.ticks();
    let (replay_bytes, lines_from) = match written.reached {
        None => (replay.encode(), None),
        Some(from) => (replay.encode_since(from)?, Some(from.ticks)),
    };
    let (kept, last) = lines(trail, lines_from, ticks);
    let mut payload = Vec::with_capacity(17 + replay_bytes.len() + kept.len() + last.len());
    payload.extend_from_slice(&ticks.to_le_bytes());
    payload.push(u8::from(ended));
    for part in [&replay_bytes[..], kept.as_bytes(), last.as_bytes()] {
        payload.extend_from_slice(&(part.len() as u32).to_le_bytes());
        payload.extend_from_slice(part);
    }
    let mut out = Vec::with_capacity(HEADER_LEN + 12 + payload.len());
    if written.bytes == 0 {
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
    }
    let start = out.len();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    let digest = xxh3_64(&out[start..]);
    out.extend_from_slice(&digest.to_le_bytes());
    let bytes = written.bytes + out.len() as u64;
    Some((out, Written { bytes, reached: Some(replay.reached()) }))
}

/// A checkpoint's line, as [`ChecksumFile`] writes one: the tick, its checksum, and the low 16 bits of each
/// tick's since `previous`.
fn line(out: &mut String, trail: &Trail, previous: Option<u32>, tick: u32) {
    let at = |tick| trail.at(tick).expect("within the trail");
    let _ = write!(out, "{tick} {:016x}", at(tick));
    let first = previous.map_or(tick, |previous| previous + 1);
    if first < tick {
        out.push(' ');
        for between in first..tick {
            let _ = write!(out, "{:04x}", at(between) as u16);
        }
    }
    out.push('\n');
}

/// The lines that stay, past those a journal at `from` holds (all of them, from tick 0, for `None`), and the
/// line for tick `ticks` if it isn't one of them.
fn lines(trail: &Trail, from: Option<u32>, ticks: u32) -> (String, String) {
    let every = CHECKPOINT_EVERY;
    let mut kept = String::new();
    let mut tick = match from {
        None => {
            line(&mut kept, trail, None, 0);
            every
        }
        Some(from) => (from / every + 1) * every,
    };
    while tick <= ticks {
        line(&mut kept, trail, Some(tick - every), tick);
        tick += every;
    }
    let mut last = String::new();
    if !ticks.is_multiple_of(every) {
        line(&mut last, trail, Some(ticks / every * every), ticks);
    }
    (kept, last)
}

/// One record, read.
struct Record<'a> {
    ticks: u32,
    ended: bool,
    replay: &'a [u8],
    kept: &'a [u8],
    last: &'a [u8],
}

/// Every whole record of `bytes`, up to the first a kill cut short or spoiled.
fn records(bytes: &[u8]) -> Result<Vec<Record<'_>>, JournalError> {
    let header = bytes.get(..HEADER_LEN).ok_or(JournalError::NotAJournal)?;
    if header[..MAGIC.len()] != MAGIC || header[MAGIC.len()..] != VERSION.to_le_bytes() {
        return Err(JournalError::NotAJournal);
    }
    let mut rest = &bytes[HEADER_LEN..];
    let mut records = Vec::new();
    while let Some((record, after)) = one(rest) {
        records.push(record);
        rest = after;
    }
    Ok(records)
}

/// The record at the front of `bytes`, if it's whole, with the bytes after it.
fn one(bytes: &[u8]) -> Option<(Record<'_>, &[u8])> {
    let length = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let end = 4usize.checked_add(length)?;
    let digest = u64::from_le_bytes(bytes.get(end..end.checked_add(8)?)?.try_into().ok()?);
    if xxh3_64(&bytes[..end]) != digest {
        return None;
    }
    let mut payload = &bytes[4..end];
    let ticks = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?);
    let ended = *payload.get(4)? == 1;
    payload = &payload[5..];
    let mut part = || {
        let length = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?) as usize;
        let taken = payload.get(4..4usize.checked_add(length)?)?;
        payload = &payload[4 + length..];
        Some(taken)
    };
    let (replay, kept, last) = (part()?, part()?, part()?);
    Some((Record { ticks, ended, replay, kept, last }, &bytes[end + 8..]))
}

/// A run as its journal holds it at `ticks`: its replay, its checksums file under `name` for `architecture`,
/// as of the record that holds `ticks` and says whether the run had `ended`: a keep that lands on a run's last
/// tick is followed by the run's end at the same tick, and the session's document names one or the other.
pub fn read(
    bytes: &[u8],
    (ticks, ended): (u32, bool),
    name: &str,
    architecture: &str,
) -> Result<(Replay, ChecksumFile), JournalError> {
    let records = records(bytes)?;
    let mut recorder: Option<Recorder> = None;
    let mut text = format!("architecture {architecture}\n\nreplay {name} {ticks}\n");
    let utf8 = |record, bytes| std::str::from_utf8(bytes).map_err(|_| JournalError::Text { record });
    let mut reached = 0;
    for (index, record) in records.iter().enumerate() {
        let replay_error = |source| JournalError::Replay { record: index, source };
        match &mut recorder {
            None => recorder = Some(Recorder::resume(Replay::decode(record.replay).map_err(replay_error)?)),
            Some(recorder) => recorder.extend(record.replay).map_err(replay_error)?,
        }
        text.push_str(utf8(index, record.kept)?);
        reached = record.ticks;
        if record.ticks > ticks {
            return Err(JournalError::Missed { ticks });
        }
        if record.ticks == ticks && record.ended == ended {
            text.push_str(utf8(index, record.last)?);
            let replay = recorder.take().expect("made above").finish();
            if replay.ticks() != ticks {
                return Err(JournalError::Missed { ticks });
            }
            let checksums =
                ChecksumFile::parse(&text).map_err(|error| JournalError::Checksums(error.to_string()))?;
            return Ok((replay, checksums));
        }
    }
    Err(JournalError::Short { reached, ticks })
}

#[cfg(test)]
mod tests {
    use sapphire_gameplay::replay::{RecordedTrail, Recording, SCENARIOS, find_script};

    use super::*;

    /// A run recorded a few hundred ticks, copied as it was after each tick.
    fn copies(ticks: u32) -> Vec<(Replay, Trail)> {
        let scenario = &SCENARIOS[0];
        let mut recording = Recording::new(scenario, scenario.config).unwrap();
        let mut source = find_script("melee-play").unwrap();
        let mut copies = Vec::new();
        for _ in 0..ticks {
            recording.step(&mut source).unwrap();
            copies.push((recording.so_far(), recording.trail().clone()));
        }
        copies
    }

    /// A journal kept at any ticks gives back, at each record, the replay and the checksums file a whole
    /// write would have had then, byte for byte; and nothing past what it holds.
    #[test]
    fn records_up_to_any_keep_are_the_run_kept_whole() {
        let copies = copies(250);
        for every in [1, 7, 59, 60, 61, 125] {
            let mut journal = Vec::new();
            let mut written = Written::default();
            let mut kept = Vec::new();
            for index in (every - 1..copies.len()).step_by(every) {
                let (replay, trail) = &copies[index];
                let (bytes, now) = record(written, replay, trail, false).unwrap();
                journal.extend_from_slice(&bytes);
                assert_eq!(now.bytes, journal.len() as u64);
                written = now;
                kept.push(index);
            }
            for &index in &kept {
                let (replay, trail) = &copies[index];
                let ticks = replay.ticks();
                let (read, checksums) = read(&journal, (ticks, false), "melee-1", "aarch64").unwrap();
                assert_eq!(read.encode(), replay.encode(), "every {every}, at {ticks}");
                let mut whole = ChecksumFile::new("aarch64");
                whole.set("melee-1", RecordedTrail::from_trail(trail));
                assert_eq!(checksums.to_text(), whole.to_text(), "every {every}, at {ticks}");
            }
            let past = copies[*kept.last().unwrap()].0.ticks() + 1;
            assert!(matches!(
                read(&journal, (past, false), "melee-1", "aarch64"),
                Err(JournalError::Short { .. })
            ));
            assert!(matches!(
                read(&journal, (0, false), "melee-1", "aarch64"),
                Err(JournalError::Missed { .. })
            ));
        }
    }

    /// A record cut anywhere is left out with all after it, and the records before it still read.
    #[test]
    fn a_torn_record_is_left_out() {
        let copies = copies(130);
        let (first, written) = record(Written::default(), &copies[69].0, &copies[69].1, false).unwrap();
        let (second, written_twice) = record(written, &copies[129].0, &copies[129].1, false).unwrap();
        // The run's end, at the tick a keep had just reached.
        let (third, _) = record(written_twice, &copies[129].0, &copies[129].1, true).unwrap();
        let whole = [first.clone(), second.clone(), third].concat();
        let (going, _) = read(&whole, (130, false), "melee-1", "aarch64").unwrap();
        let (ended, checksums) = read(&whole, (130, true), "melee-1", "aarch64").unwrap();
        assert_eq!((going.encode(), ended.encode()), (copies[129].0.encode(), copies[129].0.encode()));
        assert_eq!(checksums.get("melee-1").unwrap().ticks(), 130);
        let whole = [first.clone(), second].concat();
        assert!(matches!(read(&whole, (130, true), "melee-1", "aarch64"), Err(JournalError::Short { .. })));
        for cut in first.len()..whole.len() {
            let torn = &whole[..cut];
            assert!(read(torn, (70, false), "melee-1", "aarch64").is_ok(), "cut at {cut}");
            let short = read(torn, (130, false), "melee-1", "aarch64");
            assert!(matches!(short, Err(JournalError::Short { reached: 70, .. })));
        }
        for cut in [0, 5, HEADER_LEN] {
            assert!(read(&whole[..cut], (70, false), "melee-1", "aarch64").is_err());
        }
        let mut spoiled = whole.clone();
        spoiled[first.len() + 10] ^= 1;
        assert!(matches!(
            read(&spoiled, (130, false), "melee-1", "aarch64"),
            Err(JournalError::Short { .. })
        ));
        assert_eq!(record(written, &copies[10].0, &copies[10].1, false), None, "behind what's written");
    }
}
