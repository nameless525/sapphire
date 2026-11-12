//! Edits a minidump the monitor has written, before it's kept (spec §5.2). `minidump-writer` copies in
//! what the player's machine holds that reading a crash doesn't need, and on macOS leaves out a name that
//! it does:
//!
//! - **The environment** (Linux's copy of `/proc/<pid>/environ`): the player's, not the game's, with their
//!   user name, their home and whatever tokens their session exports. Zeroed.
//! - **The executable's name** (macOS): the writer can't read the main image's path, so it names the
//!   module `<Unknown>` and gives it no file. Both become the executable's file name.
//!
//! Every edit keeps the offsets and sizes the dump already has; a name that's added goes at the end of the
//! file, where only the module record points.

use std::io::{self, Read, Seek, SeekFrom, Write};

/// What [`apply`] changes.
#[derive(Debug, Default)]
pub(crate) struct Edits<'a> {
    /// Zero the environment's stream.
    pub environment: bool,
    /// The executable's file name, for a first module the writer left unnamed.
    pub executable: Option<&'a str>,
}

/// A minidump's signature, `MDMP`, as its first four bytes read in little-endian order.
const SIGNATURE: u32 = 0x504d_444d;

/// The stream that holds the crashed process's environment (Breakpad's `MD_LINUX_ENVIRON`).
const ENVIRONMENT_STREAM: u32 = 0x4767_0007;

/// The module list's stream, a count, then a `MINIDUMP_MODULE` per module.
const MODULE_LIST_STREAM: u32 = 4;

/// A `MINIDUMP_MODULE`'s size, and where its name's and CodeView record's locations are in it.
const MODULE_SIZE: usize = 108;
const MODULE_NAME_AT: usize = 20;
const MODULE_CODEVIEW_AT: usize = 76;

/// A CodeView record that names a PDB (`RSDS`), and its size before the name: signature, GUID, age.
const CODEVIEW_PDB70: u32 = 0x5344_5352;
const CODEVIEW_HEADER_SIZE: usize = 24;

/// The name `minidump-writer` gives a module with no path.
const UNKNOWN_MODULE: &[u8] = b"<Unknown>";

/// Applies `edits` to the minidump in `file`, in place. A file that isn't a minidump, or whose stream
/// directory runs past its end, is an error, and nothing is written.
pub(crate) fn apply(file: &mut (impl Read + Write + Seek), edits: &Edits<'_>) -> io::Result<()> {
    let mut dump = Vec::new();
    file.seek(SeekFrom::Start(0))?;
    file.read_to_end(&mut dump)?;
    edit(&mut dump, edits)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&dump)?;
    file.flush()
}

/// [`apply`]'s edits, on the dump's bytes.
fn edit(dump: &mut Vec<u8>, edits: &Edits<'_>) -> io::Result<()> {
    let streams = directory(dump)?;
    if edits.environment {
        for stream in streams.iter().filter(|stream| stream.kind == ENVIRONMENT_STREAM) {
            dump[stream.at..stream.at + stream.size].fill(0);
        }
    }
    if let Some(executable) = edits.executable {
        name_executable(dump, &streams, executable)?;
    }
    Ok(())
}

/// One entry of a minidump's stream directory, checked to lie inside the file.
struct Stream {
    kind: u32,
    at: usize,
    size: usize,
}

fn directory(dump: &[u8]) -> io::Result<Vec<Stream>> {
    if word(dump, 0)? != SIGNATURE {
        return Err(invalid("without its signature"));
    }
    let (count, at) = (index(word(dump, 8)?), index(word(dump, 12)?));
    (0..count)
        .map(|entry| {
            let entry =
                entry.checked_mul(12).and_then(|offset| at.checked_add(offset)).ok_or_else(past_end)?;
            let (kind, size, at) =
                (word(dump, entry)?, index(word(dump, entry + 4)?), index(word(dump, entry + 8)?));
            if at.checked_add(size).is_none_or(|end| end > dump.len()) {
                return Err(past_end());
            }
            Ok(Stream { kind, at, size })
        })
        .collect()
}

/// Names the module list's first module `executable`, if the writer left it without a name: its file's
/// name, and the CodeView record's PDB name where that's `<Unknown>`. Returns whether it named it.
fn name_executable(dump: &mut Vec<u8>, streams: &[Stream], executable: &str) -> io::Result<bool> {
    let Some(list) = streams.iter().find(|stream| stream.kind == MODULE_LIST_STREAM) else {
        return Ok(false);
    };
    if list.size < 4 + MODULE_SIZE || word(dump, list.at)? == 0 {
        return Ok(false);
    }
    // macOS's writer puts the main executable first.
    let module = list.at + 4;
    let name_at = index(word(dump, module + MODULE_NAME_AT)?);
    if word(dump, name_at)? != 0 {
        return Ok(false);
    }
    // A MINIDUMP_STRING: its size in bytes, then UTF-16, then a NUL the size doesn't count.
    let units: Vec<u16> = executable.encode_utf16().collect();
    let name = append(dump, |dump| {
        dump.extend_from_slice(&rva(units.len() * 2)?.to_le_bytes());
        dump.extend(units.iter().chain([&0]).flat_map(|unit| unit.to_le_bytes()));
        Ok(())
    })?;
    set_word(dump, module + MODULE_NAME_AT, name);

    let (size, at) = (
        index(word(dump, module + MODULE_CODEVIEW_AT)?),
        index(word(dump, module + MODULE_CODEVIEW_AT + 4)?),
    );
    let record = dump.get(at..at.saturating_add(size)).ok_or_else(past_end)?;
    let pdb_name = record.get(CODEVIEW_HEADER_SIZE..).unwrap_or_default();
    let pdb_name = pdb_name.iter().position(|&byte| byte == 0).map_or(pdb_name, |end| &pdb_name[..end]);
    if size >= CODEVIEW_HEADER_SIZE
        && word(record, 0)? == CODEVIEW_PDB70
        && (pdb_name.is_empty() || pdb_name == UNKNOWN_MODULE)
    {
        let header = record[..CODEVIEW_HEADER_SIZE].to_vec();
        let codeview = append(dump, |dump| {
            dump.extend_from_slice(&header);
            dump.extend_from_slice(executable.as_bytes());
            dump.push(0);
            Ok(())
        })?;
        set_word(dump, module + MODULE_CODEVIEW_AT, rva(CODEVIEW_HEADER_SIZE + executable.len() + 1)?);
        set_word(dump, module + MODULE_CODEVIEW_AT + 4, codeview);
    }
    Ok(true)
}

/// Appends what `write` writes at the end of `dump`, aligned to four bytes, and returns where it starts.
fn append(dump: &mut Vec<u8>, write: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> io::Result<u32> {
    dump.resize(dump.len().next_multiple_of(4), 0);
    let at = rva(dump.len())?;
    write(dump)?;
    Ok(at)
}

/// The little-endian `u32` at `at`.
fn word(bytes: &[u8], at: usize) -> io::Result<u32> {
    let word = bytes.get(at..at.saturating_add(4)).ok_or_else(past_end)?;
    Ok(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

/// Writes `value` at `at`, which [`word`] has read already.
fn set_word(dump: &mut [u8], at: usize, value: u32) {
    dump[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn index(word: u32) -> usize {
    usize::try_from(word).unwrap_or(usize::MAX)
}

/// An offset or size as a minidump records one.
fn rva(value: usize) -> io::Result<u32> {
    u32::try_from(value).map_err(|_| invalid("past 4 GiB"))
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("a minidump {what}"))
}

fn past_end() -> io::Error {
    invalid("whose directory or streams run past its end")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minidump laid out as `minidump-writer` lays one out: the header, then a directory naming each of
    /// `streams`, then their contents in order. Returns it and where each stream starts.
    fn dump_of(streams: &[(u32, &[u8])]) -> (Vec<u8>, Vec<usize>) {
        let count = u32::try_from(streams.len()).unwrap();
        let mut dump = Vec::new();
        for word in [SIGNATURE, 0xa793, count, 32, 0, 0, 0, 0] {
            dump.extend_from_slice(&word.to_le_bytes());
        }
        let mut at = 32 + streams.len() * 12;
        let mut starts = Vec::new();
        for (kind, contents) in streams {
            for word in [*kind, u32::try_from(contents.len()).unwrap(), u32::try_from(at).unwrap()] {
                dump.extend_from_slice(&word.to_le_bytes());
            }
            starts.push(at);
            at += contents.len();
        }
        for (_, contents) in streams {
            dump.extend_from_slice(contents);
        }
        (dump, starts)
    }

    /// The UTF-16 MINIDUMP_STRING at `at`.
    fn string_at(dump: &[u8], at: usize) -> String {
        let size = index(word(dump, at).unwrap());
        let units: Vec<u16> = dump[at + 4..at + 4 + size]
            .chunks(2)
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn a_dump_keeps_no_environment() {
        let environment = b"USER=you\0HOME=/home/you\0TOKEN=secret\0";
        let command_line = b"game_app\0--crash\0fault\0";
        let (dump, starts) = dump_of(&[(0x4767_0006, command_line), (ENVIRONMENT_STREAM, environment)]);
        let mut file = io::Cursor::new(dump);
        apply(&mut file, &Edits { environment: true, ..Edits::default() }).unwrap();
        let dump = file.into_inner();
        assert_eq!(dump[starts[1]..], vec![0; environment.len()][..]);
        assert_eq!(&dump[starts[0]..starts[1]], command_line);

        let mut other = io::Cursor::new(b"PK\x03\x04 not a dump".to_vec());
        assert!(apply(&mut other, &Edits::default()).is_err());
        assert_eq!(other.into_inner(), b"PK\x03\x04 not a dump", "nothing is written");
        let mut cut = dump[..40].to_vec();
        assert!(apply(&mut io::Cursor::new(&mut cut), &Edits::default()).is_err());
    }

    /// The first module as macOS's writer leaves the executable: no file name, and a PDB name of `<Unknown>`.
    #[test]
    fn the_executable_is_named() {
        let (mut dump, _) =
            dump_of(&[(MODULE_LIST_STREAM, &[0; 4 + 2 * MODULE_SIZE]), (0x1234, &[0; 4 + 24 + 10])]);
        let empty_name = dump.len() - 38;
        let codeview = empty_name + 4;
        dump[codeview..codeview + 4].copy_from_slice(&CODEVIEW_PDB70.to_le_bytes());
        dump[codeview + 4..codeview + 20].copy_from_slice(&[7; 16]);
        dump[codeview + 24..codeview + 34].copy_from_slice(b"<Unknown>\0");
        let list = 32 + 2 * 12;
        let module = list + 4;
        set_word(&mut dump, list, 2);
        set_word(&mut dump, module + MODULE_NAME_AT, rva(empty_name).unwrap());
        set_word(&mut dump, module + MODULE_CODEVIEW_AT, 34);
        set_word(&mut dump, module + MODULE_CODEVIEW_AT + 4, rva(codeview).unwrap());
        let before = dump.clone();

        edit(&mut dump, &Edits { executable: Some("game_app"), ..Edits::default() }).unwrap();
        assert_eq!(dump[..before.len()].len(), before.len());
        assert_eq!(string_at(&dump, index(word(&dump, module + MODULE_NAME_AT).unwrap())), "game_app");
        let (size, at) = (
            index(word(&dump, module + MODULE_CODEVIEW_AT).unwrap()),
            index(word(&dump, module + MODULE_CODEVIEW_AT + 4).unwrap()),
        );
        assert_eq!(&dump[at..at + 24], &before[codeview..codeview + 24], "the same signature, GUID and age");
        assert_eq!(&dump[at + 24..at + size], b"game_app\0");

        // A module with a name keeps it.
        let named = dump.clone();
        edit(&mut dump, &Edits { executable: Some("other"), ..Edits::default() }).unwrap();
        assert_eq!(dump, named);
    }
}
