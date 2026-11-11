//! Android's two records of a native crash: `sapphire_crash`'s signal record, written in the signal
//! handler, and `debuggerd`'s tombstone, which the app copies from `ApplicationExitInfo` at its next launch
//! (Android 12 and later).
//!
//! The tombstone's fields are AOSP's `system/core/debuggerd/proto/tombstone.proto` (Apache-2.0), read by
//! number: only the few a crash's frames need.

use anyhow::{Context as _, ensure};

use super::panic::{Loaded, containing, hex};
use super::protobuf::{self, Value};
use super::{Crash, Frame, Reader, Thread};

pub(super) const SIGNAL_RECORD_TITLE: &str = "Sapphire native crash record v";

pub(super) fn read_signal_record(text: &str, reader: &Reader<'_>) -> anyhow::Result<Crash> {
    ensure!(
        text.starts_with("Sapphire native crash record v2\n"),
        "a signal record from before M4 C2, which names no build IDs: its modules' symbols can't be found"
    );
    let (lines, maps) = text.split_once("\nmaps:\n").context("a signal record with no maps")?;
    let field = |key: &str| lines.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(": "));
    let modules: Vec<Loaded> = lines
        .lines()
        .filter_map(|line| line.strip_prefix("module: "))
        .map(Loaded::parse)
        .collect::<anyhow::Result<_>>()?;

    let mut thread =
        Thread { name: field("tid").unwrap_or("?").to_owned(), crashed: true, frames: Vec::new() };
    // The faulting instruction, then where its function returns to: the one frame the handler can name
    // without unwinding (the tombstone has the rest).
    for (register, return_address) in [("pc", false), ("rip", false), ("lr", true)] {
        let Some(address) = field(register).and_then(hex) else { continue };
        let frame = match holding(&modules, maps, address) {
            Some(loaded) => {
                let offset = address - loaded.base;
                let lookup = if return_address { offset.saturating_sub(1) } else { offset };
                reader.frame(address, &loaded.name, reader.module(&loaded.id)?, offset, lookup)
            }
            None => Frame { address, ..Frame::default() },
        };
        thread.frames.push(frame);
    }
    let summary = vec![
        format!(
            "signal {} code {} at {}",
            field("signal").unwrap_or("?"),
            field("code").unwrap_or("?"),
            field("fault_address").unwrap_or("?")
        ),
        format!(
            "{} {} (run {})",
            field("app").unwrap_or("?"),
            field("release").unwrap_or("?"),
            field("run").unwrap_or("?")
        ),
    ];
    Ok(Crash { kind: "Android signal record", summary, threads: vec![thread] })
}

/// The module of `modules` that holds `address`: the nearest loaded below it, if `maps` says the address is
/// in that module's file. A library loaded after the record's modules were listed (a GPU driver, a plugin)
/// or code in no file has none: the module below it would name that module's symbols at an offset that
/// isn't theirs. A library loaded from inside the APK is named `<apk>!/lib/<abi>/<name>.so`, and maps name
/// the APK.
fn holding<'m>(modules: &'m [Loaded], maps: &str, address: u64) -> Option<&'m Loaded> {
    let path = mapped(maps, address)?;
    containing(modules, address).filter(|module| {
        module.name == path || module.name.strip_prefix(path).is_some_and(|rest| rest.starts_with("!/"))
    })
}

/// The path of the file `/proc/self/maps` says holds `address`.
fn mapped(maps: &str, address: u64) -> Option<&str> {
    maps.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let (start, end) = parts.next()?.split_once('-')?;
        let range = u64::from_str_radix(start, 16).ok()?..u64::from_str_radix(end, 16).ok()?;
        let path = parts.nth(4)?;
        range.contains(&address).then_some(path)
    })
}

// tombstone.proto's field numbers.
const TOMBSTONE_PID: u32 = 5;
const TOMBSTONE_TID: u32 = 6;
const TOMBSTONE_COMMAND_LINE: u32 = 9;
const TOMBSTONE_SIGNAL: u32 = 10;
const TOMBSTONE_ABORT_MESSAGE: u32 = 14;
const TOMBSTONE_THREADS: u32 = 16;
const SIGNAL_NAME: u32 = 2;
const SIGNAL_CODE_NAME: u32 = 4;
const SIGNAL_FAULT_ADDRESS: u32 = 9;
const THREAD_ID: u32 = 1;
const THREAD_NAME: u32 = 2;
const THREAD_BACKTRACE: u32 = 4;
const FRAME_REL_PC: u32 = 1;
const FRAME_PC: u32 = 2;
const FRAME_FUNCTION: u32 = 4;
const FRAME_FILE: u32 = 6;
const FRAME_BUILD_ID: u32 = 8;

pub(super) fn read_tombstone(bytes: &[u8], reader: &Reader<'_>) -> anyhow::Result<Crash> {
    let tombstone = protobuf::fields(bytes).context("not a tombstone")?;
    let get = |number: u32| tombstone.iter().find(|(n, _)| *n == number).map(|(_, value)| *value);
    let tid = get(TOMBSTONE_TID).and_then(Value::varint).context("a tombstone names no crashing thread")?;
    let mut summary = Vec::new();
    if let Some(signal) = get(TOMBSTONE_SIGNAL).and_then(Value::bytes) {
        let signal = protobuf::fields(signal)?;
        let get = |number: u32| signal.iter().find(|(n, _)| *n == number).map(|(_, value)| *value);
        let name = get(SIGNAL_NAME).and_then(Value::string).unwrap_or_default();
        let code = get(SIGNAL_CODE_NAME).and_then(Value::string).unwrap_or_default();
        let address = get(SIGNAL_FAULT_ADDRESS).and_then(Value::varint).unwrap_or(0);
        summary.push(format!("{name} ({code}) at {address:#x}"));
    }
    if let Some(message) =
        get(TOMBSTONE_ABORT_MESSAGE).and_then(Value::string).filter(|message| !message.is_empty())
    {
        summary.push(format!("abort message: {message}"));
    }
    let command: Vec<String> = tombstone
        .iter()
        .filter(|(n, _)| *n == TOMBSTONE_COMMAND_LINE)
        .filter_map(|(_, value)| value.string())
        .collect();
    let pid = get(TOMBSTONE_PID).and_then(Value::varint).unwrap_or(0);
    summary.push(format!("pid {pid} tid {tid}: {}", command.join(" ")));

    let mut threads = Vec::new();
    // `threads` is a map: each entry a message of its key (1) and the thread (2).
    for (_, entry) in tombstone.iter().filter(|(n, _)| *n == TOMBSTONE_THREADS) {
        let entry = protobuf::fields(entry.bytes().context("a thread entry isn't a message")?)?;
        let Some(thread) = entry.iter().find(|(n, _)| *n == 2).and_then(|(_, value)| value.bytes()) else {
            continue;
        };
        let thread = protobuf::fields(thread)?;
        let get = |number: u32| thread.iter().find(|(n, _)| *n == number).map(|(_, value)| *value);
        let id = get(THREAD_ID).and_then(Value::varint).unwrap_or(0);
        if id != tid {
            continue;
        }
        let name = get(THREAD_NAME).and_then(Value::string).unwrap_or_default();
        let mut read = Thread { name: format!("{id} {name}"), crashed: true, frames: Vec::new() };
        for (_, frame) in thread.iter().filter(|(n, _)| *n == THREAD_BACKTRACE) {
            read.frames.push(tombstone_frame(frame.bytes().context("a frame isn't a message")?, reader)?);
        }
        threads.push(read);
    }
    ensure!(!threads.is_empty(), "the tombstone has no thread {tid}, which crashed");
    Ok(Crash { kind: "Android tombstone", summary, threads })
}

/// A frame as `debuggerd` unwound it: its address in its file (`rel_pc`, already the call's for a return
/// address), and the file's build ID. Frames in no module of ours keep the name `debuggerd` found.
fn tombstone_frame(bytes: &[u8], reader: &Reader<'_>) -> anyhow::Result<Frame> {
    let fields = protobuf::fields(bytes)?;
    let get = |number: u32| fields.iter().find(|(n, _)| *n == number).map(|(_, value)| *value);
    let pc = get(FRAME_PC).and_then(Value::varint).unwrap_or(0);
    let rel_pc = get(FRAME_REL_PC).and_then(Value::varint).unwrap_or(0);
    let file = get(FRAME_FILE).and_then(Value::string).unwrap_or_default();
    let build_id = get(FRAME_BUILD_ID).and_then(Value::string).unwrap_or_default();
    let module = if build_id.is_empty() { None } else { reader.module(&build_id)? };
    let mut frame = reader.frame(pc, &file, module, rel_pc, rel_pc);
    if frame.location.function.is_none() {
        frame.location.function = get(FRAME_FUNCTION).and_then(Value::string).filter(|name| !name.is_empty());
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::super::protobuf::write;
    use super::*;
    use crate::store::Store;

    #[test]
    fn maps_name_the_file_an_address_is_in() {
        let maps = "\
7a00000000-7a00001000 r--p 00000000 fd:00 123  /data/app/x/lib/arm64/libgame_app.so
7a00001000-7a00100000 r-xp 00001000 fd:00 123  /data/app/x/lib/arm64/libgame_app.so
7b00000000-7b00001000 rw-p 00000000 00:00 0
";
        assert_eq!(mapped(maps, 0x7a_0000_2000), Some("/data/app/x/lib/arm64/libgame_app.so"));
        assert_eq!(mapped(maps, 0x7b_0000_0010), None);
        assert_eq!(mapped(maps, 0x10), None);
    }

    /// Only the module whose file holds an address reads it: one loaded after the record listed its modules
    /// names no module's symbols, and a library inside the APK is still its own.
    #[test]
    fn signal_records_read_addresses_only_in_their_modules_files() {
        let record = |pc: u64| {
            format!(
                "Sapphire native crash record v2\napp: game\nrelease: 1\nrun: 1794417504344-42\n\
                 module: 0x0000007a00000000 5fc134e7 /data/app/x/base.apk!/lib/arm64-v8a/libgame_app.so\n\
                 signal: 11 (SIGSEGV)\ncode: 1\nfault_address: 0x0000000000000010\npid: 42\ntid: 43\n\
                 pc: {pc:#018x}\n\nmaps:\n\
                 7a00000000-7a00100000 r-xp 00001000 fd:00 123  /data/app/x/base.apk\n\
                 7c00000000-7c00100000 r-xp 00000000 fd:00 456  /vendor/lib64/libgpu_driver.so\n"
            )
        };
        let store = Store { root: std::env::temp_dir() };
        let reader = Reader::new(&[], &store);
        let ours = read_signal_record(&record(0x7a_0000_1234), &reader).unwrap();
        assert_eq!(ours.threads[0].frames[0].module, Some(("libgame_app.so".to_owned(), 0x1234)));
        let driver = read_signal_record(&record(0x7c_0000_1234), &reader).unwrap();
        assert_eq!(driver.threads[0].frames[0].module, None, "{driver:?}");
    }

    /// A tombstone as `debuggerd` writes one: the crashing thread's frames are read, others skipped, and a
    /// frame in no package keeps the name `debuggerd` gave it.
    #[test]
    fn tombstones_read_the_crashing_threads_frames() {
        let frame = |rel_pc: u64, function: &str, file: &str| {
            let mut out = Vec::new();
            write::uint(&mut out, FRAME_REL_PC, rel_pc);
            write::uint(&mut out, FRAME_PC, 0x7a_0000_0000 + rel_pc);
            write::bytes(&mut out, FRAME_FUNCTION, function.as_bytes());
            write::bytes(&mut out, FRAME_FILE, file.as_bytes());
            write::bytes(&mut out, FRAME_BUILD_ID, b"5fc134e7");
            out
        };
        let thread = |id: u64, frames: &[Vec<u8>]| {
            let mut thread = Vec::new();
            write::uint(&mut thread, THREAD_ID, id);
            write::bytes(&mut thread, THREAD_NAME, b"SDLThread");
            for frame in frames {
                write::bytes(&mut thread, THREAD_BACKTRACE, frame);
            }
            let mut entry = Vec::new();
            write::uint(&mut entry, 1, id);
            write::bytes(&mut entry, 2, &thread);
            entry
        };
        let mut signal = Vec::new();
        write::bytes(&mut signal, SIGNAL_NAME, b"SIGSEGV");
        write::bytes(&mut signal, SIGNAL_CODE_NAME, b"SEGV_MAPERR");
        write::uint(&mut signal, SIGNAL_FAULT_ADDRESS, 0x10);
        let mut tombstone = Vec::new();
        write::uint(&mut tombstone, TOMBSTONE_PID, 4242);
        write::uint(&mut tombstone, TOMBSTONE_TID, 4250);
        write::bytes(&mut tombstone, TOMBSTONE_COMMAND_LINE, b"com.example.sapphire.game");
        write::bytes(&mut tombstone, TOMBSTONE_SIGNAL, &signal);
        write::bytes(
            &mut tombstone,
            TOMBSTONE_THREADS,
            &thread(4242, &[frame(1, "main", "/system/bin/app_process64")]),
        );
        let crashed = [
            frame(0x1234, "forced_fault", "/data/app/libgame_app.so"),
            frame(0x88, "start", "/apex/libc.so"),
        ];
        write::bytes(&mut tombstone, TOMBSTONE_THREADS, &thread(4250, &crashed));

        let store = Store { root: std::env::temp_dir() };
        let reader = Reader::new(&[], &store);
        let crash = read_tombstone(&tombstone, &reader).unwrap();
        assert_eq!(crash.summary[0], "SIGSEGV (SEGV_MAPERR) at 0x10");
        assert_eq!(crash.summary[1], "pid 4242 tid 4250: com.example.sapphire.game");
        let [thread] = crash.threads.as_slice() else { panic!("one thread: {crash:?}") };
        assert_eq!(thread.name, "4250 SDLThread");
        assert_eq!(thread.frames[0].module, Some(("libgame_app.so".to_owned(), 0x1234)));
        assert_eq!(thread.frames[0].location.function.as_deref(), Some("forced_fault"));
        assert_eq!(thread.frames[1].address, 0x7a_0000_0088);
        assert!(read_tombstone(&tombstone[..tombstone.len() - 3], &reader).is_err());
    }
}
